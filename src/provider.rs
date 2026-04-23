use std::thread;
use std::time::Duration;

use anyhow::{Context, anyhow};
use chrono::Utc;
use regex::Regex;
use reqwest::blocking::Client;

use crate::APP_VERSION;
use crate::model::{
    MtProtoProxy, ProviderConfig, ProviderSource, ProviderSourceKind, ProxyKind, ProxyRecord,
    TelegramProxy,
};

#[derive(Debug)]
pub struct MtProtoProvider {
    client: Client,
    config: ProviderConfig,
    telegram_link_pattern: Regex,
}

#[derive(Debug, Default)]
struct HtmlProbe {
    proxy: Option<TelegramProxy>,
    saw_waiting_message: bool,
    saw_no_servers_message: bool,
}

impl MtProtoProvider {
    pub fn new(config: ProviderConfig) -> anyhow::Result<Self> {
        let client = Client::builder()
            .user_agent(format!("ProtoSwitch/{APP_VERSION}"))
            .cookie_store(true)
            .timeout(Duration::from_secs(10))
            .build()
            .context("Не удалось создать HTTP-клиент")?;

        let telegram_link_pattern =
            Regex::new(r#"(?i)(?:tg://|https?://t\.me/)(?:proxy|socks)\?[^"' <>\r\n]+"#)
                .context("Не удалось подготовить шаблон Telegram proxy links")?;

        Ok(Self {
            client,
            config,
            telegram_link_pattern,
        })
    }

    pub fn fetch_candidate(&self, recent: &[MtProtoProxy]) -> anyhow::Result<ProxyRecord> {
        let sources = self.config.active_sources();
        if sources.is_empty() {
            return Err(anyhow!("В config.toml нет включённых источников proxy"));
        }

        let mut source_errors = Vec::new();
        let mut only_recent = Vec::new();

        for source in sources {
            match self.fetch_source_candidates(&source) {
                Ok(candidates) => {
                    if candidates.is_empty() {
                        source_errors.push(format!("{}: пустой ответ", source.name));
                        continue;
                    }

                    let rotated = rotate_records(candidates);
                    let mut found_recent_only = false;
                    for record in rotated {
                        if recent.contains(&record.proxy) {
                            found_recent_only = true;
                            continue;
                        }
                        return Ok(record);
                    }

                    if found_recent_only {
                        only_recent.push(source.name.clone());
                        source_errors.push(format!("{}: только недавние кандидаты", source.name));
                    } else {
                        source_errors
                            .push(format!("{}: не удалось выбрать кандидата", source.name));
                    }
                }
                Err(error) => source_errors.push(format!("{}: {error}", source.name)),
            }
        }

        if !only_recent.is_empty() {
            return Err(anyhow!(
                "Источники вернули только недавние proxy: {}",
                only_recent.join(", ")
            ));
        }

        Err(anyhow!(
            "{}",
            if source_errors.is_empty() {
                "Источники proxy не вернули кандидатов".to_string()
            } else {
                source_errors.join(" | ")
            }
        ))
    }

    pub fn fetch_html(&self, url: &str) -> anyhow::Result<String> {
        let response = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("Не удалось открыть {url}"))?;

        let response = response
            .error_for_status()
            .with_context(|| format!("Источник {url} вернул ошибку"))?;

        response
            .text()
            .context("Не удалось прочитать ответ источника")
    }

    fn fetch_source_candidates(&self, source: &ProviderSource) -> anyhow::Result<Vec<ProxyRecord>> {
        match source.kind {
            ProviderSourceKind::MtprotoRuPage => self.fetch_mtproto_ru_candidates(source),
            ProviderSourceKind::TelegramLinkList => self.fetch_telegram_link_list(source),
            ProviderSourceKind::Socks5UrlList => self.fetch_socks5_url_list(source),
        }
    }

    fn fetch_mtproto_ru_candidates(
        &self,
        source: &ProviderSource,
    ) -> anyhow::Result<Vec<ProxyRecord>> {
        let attempts = self.config.fetch_attempts.max(8);
        let retry_delay = Duration::from_millis(self.config.fetch_retry_delay_ms.max(1_000));
        let mut saw_waiting_message = false;
        let mut saw_no_servers_message = false;
        let mut last_error = None;

        for attempt in 0..attempts {
            let html = match self.fetch_html(&source.url) {
                Ok(html) => html,
                Err(error) => {
                    last_error = Some(error);
                    if attempt + 1 < attempts {
                        thread::sleep(retry_delay);
                        continue;
                    }
                    break;
                }
            };

            let probe = match self.inspect_html(&html) {
                Ok(probe) => probe,
                Err(error) => {
                    last_error = Some(error);
                    if attempt + 1 < attempts {
                        thread::sleep(retry_delay);
                        continue;
                    }
                    break;
                }
            };

            saw_waiting_message |= probe.saw_waiting_message;
            saw_no_servers_message |= probe.saw_no_servers_message;

            if let Some(proxy) = probe.proxy {
                return Ok(vec![ProxyRecord::new(proxy, source.name.clone())]);
            }

            if attempt + 1 < attempts {
                thread::sleep(retry_delay);
            }
        }

        if saw_no_servers_message {
            return Err(anyhow!("свободных серверов нет"));
        }

        if saw_waiting_message {
            return Err(anyhow!(
                "источник не успел выдать proxy за {} попыток по {} мс",
                attempts,
                retry_delay.as_millis()
            ));
        }

        Err(last_error.unwrap_or_else(|| anyhow!("источник не вернул proxy")))
    }

    fn fetch_telegram_link_list(
        &self,
        source: &ProviderSource,
    ) -> anyhow::Result<Vec<ProxyRecord>> {
        let body = self.fetch_html(&source.url)?;
        let proxies = self.extract_telegram_link_candidates(&body)?;
        if proxies.is_empty() {
            return Err(anyhow!("в списке нет Telegram proxy ссылок"));
        }

        Ok(proxies
            .into_iter()
            .map(|proxy| ProxyRecord::new(proxy, source.name.clone()))
            .collect())
    }

    fn fetch_socks5_url_list(&self, source: &ProviderSource) -> anyhow::Result<Vec<ProxyRecord>> {
        let body = self.fetch_html(&source.url)?;
        let mut proxies = Vec::new();
        for line in body.lines() {
            if let Ok(proxy) = parse_socks5_line(line) {
                proxies.push(ProxyRecord::new(proxy, source.name.clone()));
            }
        }

        if proxies.is_empty() {
            return Err(anyhow!("в списке нет валидных SOCKS5 строк"));
        }

        Ok(proxies)
    }

    fn inspect_html(&self, html: &str) -> anyhow::Result<HtmlProbe> {
        let proxy = self
            .telegram_link_pattern
            .find(html)
            .map(|entry| parse_telegram_proxy_link(entry.as_str()))
            .transpose()?;

        Ok(HtmlProbe {
            proxy,
            saw_waiting_message: html.contains("Идёт выбор сервера"),
            saw_no_servers_message: html.contains("Свободных серверов"),
        })
    }

    fn extract_telegram_link_candidates(&self, body: &str) -> anyhow::Result<Vec<TelegramProxy>> {
        let mut proxies = Vec::new();
        for entry in self.telegram_link_pattern.find_iter(body) {
            let raw = sanitize_url_fragment(entry.as_str());
            if let Ok(proxy) = parse_telegram_proxy_link(&raw)
                && !proxies.contains(&proxy)
            {
                proxies.push(proxy);
            }
        }

        Ok(proxies)
    }
}

#[cfg(test)]
pub fn parse_tg_link(tg_link: &str) -> anyhow::Result<MtProtoProxy> {
    let proxy = parse_telegram_proxy_link(tg_link)?;
    if proxy.kind != ProxyKind::MtProto {
        return Err(anyhow!("Ссылка не является tg://proxy"));
    }
    Ok(proxy)
}

pub fn parse_telegram_proxy_link(value: &str) -> anyhow::Result<TelegramProxy> {
    let sanitized = sanitize_url_fragment(value);
    let url = url::Url::parse(&sanitized)
        .with_context(|| format!("Невалидная proxy-ссылка: {sanitized}"))?;

    let (kind, pairs) = match url.scheme() {
        "tg" => {
            let kind = match url.host_str() {
                Some("proxy") => ProxyKind::MtProto,
                Some("socks") => ProxyKind::Socks5,
                _ => return Err(anyhow!("Ссылка не является tg://proxy или tg://socks")),
            };
            (kind, url.query_pairs().collect::<Vec<_>>())
        }
        "http" | "https" => {
            let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
            let path = url.path().trim_start_matches('/').to_ascii_lowercase();
            if host != "t.me" {
                return Err(anyhow!("Ссылка не является t.me/proxy или t.me/socks"));
            }
            let kind = match path.as_str() {
                "proxy" => ProxyKind::MtProto,
                "socks" => ProxyKind::Socks5,
                _ => return Err(anyhow!("Ссылка не является t.me/proxy или t.me/socks")),
            };
            (kind, url.query_pairs().collect::<Vec<_>>())
        }
        _ => return Err(anyhow!("Неподдерживаемая схема proxy-ссылки")),
    };

    let mut server = None;
    let mut port = None;
    let mut secret = None;
    let mut username = None;
    let mut password = None;

    for (key, value) in pairs {
        match key.as_ref() {
            "server" => server = Some(value.into_owned()),
            "port" => port = Some(value.parse::<u16>().context("Невалидный port")?),
            "secret" => secret = Some(value.into_owned()),
            "user" | "username" => username = Some(value.into_owned()),
            "pass" | "password" => password = Some(value.into_owned()),
            _ => {}
        }
    }

    let server = server.ok_or_else(|| anyhow!("В proxy-ссылке нет server"))?;
    let port = port.ok_or_else(|| anyhow!("В proxy-ссылке нет port"))?;

    Ok(match kind {
        ProxyKind::MtProto => TelegramProxy::mtproto(
            server,
            port,
            secret.ok_or_else(|| anyhow!("В tg://proxy нет secret"))?,
        ),
        ProxyKind::Socks5 => TelegramProxy::socks5(server, port, username, password),
    })
}

pub fn parse_socks5_line(line: &str) -> anyhow::Result<TelegramProxy> {
    let trimmed = sanitize_url_fragment(line.trim());
    if trimmed.is_empty() {
        return Err(anyhow!("Пустая SOCKS5-строка"));
    }

    if trimmed.contains("://") {
        let url = url::Url::parse(&trimmed)
            .with_context(|| format!("Невалидный SOCKS5 URL: {trimmed}"))?;
        let scheme = url.scheme().to_ascii_lowercase();
        if scheme != "socks5" && scheme != "socks" {
            return Err(anyhow!("Строка не является socks5:// URL"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("В SOCKS5 URL нет host"))?
            .to_string();
        let port = url.port().ok_or_else(|| anyhow!("В SOCKS5 URL нет port"))?;
        let username = if url.username().is_empty() {
            None
        } else {
            Some(url.username().to_string())
        };
        let password = url.password().map(|value| value.to_string());
        return Ok(TelegramProxy::socks5(host, port, username, password));
    }

    let Some((host, port)) = trimmed.rsplit_once(':') else {
        return Err(anyhow!("Ожидался формат host:port"));
    };

    Ok(TelegramProxy::socks5(
        host.trim(),
        port.trim().parse::<u16>().context("Невалидный port")?,
        None,
        None,
    ))
}

fn sanitize_url_fragment(value: &str) -> String {
    value
        .trim()
        .trim_end_matches([')', ']', '}', ',', ';'])
        .to_string()
}

fn rotate_records(records: Vec<ProxyRecord>) -> Vec<ProxyRecord> {
    if records.len() <= 1 {
        return records;
    }

    let offset = (Utc::now().timestamp().unsigned_abs() as usize) % records.len();
    let mut rotated = records[offset..].to_vec();
    rotated.extend_from_slice(&records[..offset]);
    rotated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderConfig;

    #[test]
    fn parses_mtproto_html() {
        let provider = MtProtoProvider::new(ProviderConfig::default()).unwrap();
        let html = r#"
        <p>Your server - <a href="tg://proxy?server=example.com&port=443&secret=abcdef123456">proxy</a></p>
        "#;

        let proxy = provider.inspect_html(html).unwrap().proxy.unwrap();
        assert_eq!(proxy.kind, ProxyKind::MtProto);
        assert_eq!(proxy.server, "example.com");
        assert_eq!(proxy.port, 443);
        assert_eq!(proxy.secret.as_deref(), Some("abcdef123456"));
    }

    #[test]
    fn parses_tg_proxy_link() {
        let proxy = parse_tg_link("tg://proxy?server=127.0.0.1&port=8080&secret=abc123").unwrap();
        assert_eq!(proxy.kind, ProxyKind::MtProto);
        assert_eq!(proxy.server, "127.0.0.1");
        assert_eq!(proxy.port, 8080);
        assert_eq!(proxy.secret.as_deref(), Some("abc123"));
    }

    #[test]
    fn parses_t_me_proxy_link() {
        let proxy = parse_telegram_proxy_link(
            "https://t.me/proxy?server=example.com&port=443&secret=abcdef123456",
        )
        .unwrap();
        assert_eq!(proxy.kind, ProxyKind::MtProto);
        assert_eq!(proxy.server, "example.com");
    }

    #[test]
    fn parses_socks_link() {
        let proxy = parse_telegram_proxy_link(
            "tg://socks?server=127.0.0.1&port=1080&user=demo&pass=secret",
        )
        .unwrap();
        assert_eq!(proxy.kind, ProxyKind::Socks5);
        assert_eq!(proxy.username.as_deref(), Some("demo"));
        assert_eq!(proxy.password.as_deref(), Some("secret"));
    }

    #[test]
    fn parses_socks5_feed_line() {
        let proxy = parse_socks5_line("socks5://demo:secret@example.com:1080").unwrap();
        assert_eq!(proxy.kind, ProxyKind::Socks5);
        assert_eq!(proxy.server, "example.com");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.username.as_deref(), Some("demo"));
        assert_eq!(proxy.password.as_deref(), Some("secret"));
    }

    #[test]
    fn inspects_mtproto_waiting_html() {
        let provider = MtProtoProvider::new(ProviderConfig::default()).unwrap();
        let html = r#"
        <p id="info-message">Идёт выбор сервера, подождите... (проверка занимает меньше 5 секунд)</p>
        <p id="get-message" style="display:none;">Свободных серверов на данный момент нет.</p>
        "#;

        let probe = provider.inspect_html(html).unwrap();
        assert!(probe.proxy.is_none());
        assert!(probe.saw_waiting_message);
        assert!(probe.saw_no_servers_message);
    }
}
