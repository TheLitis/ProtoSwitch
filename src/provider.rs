use std::thread;
use std::time::Duration;

use anyhow::{Context, anyhow};
use regex::Regex;
use reqwest::blocking::Client;

use crate::APP_VERSION;
use crate::model::{MtProtoProxy, ProviderConfig, ProxyRecord};

#[derive(Debug)]
pub struct MtProtoProvider {
    client: Client,
    config: ProviderConfig,
    tg_link_pattern: Regex,
}

#[derive(Debug, Default)]
struct HtmlProbe {
    proxy: Option<MtProtoProxy>,
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

        let tg_link_pattern = Regex::new(r#"tg://proxy\?[^"' <]+"#)
            .context("Не удалось подготовить шаблон tg://proxy")?;

        Ok(Self {
            client,
            config,
            tg_link_pattern,
        })
    }

    pub fn fetch_candidate(&self, recent: &[MtProtoProxy]) -> anyhow::Result<ProxyRecord> {
        let attempts = self.config.fetch_attempts.max(8);
        let retry_delay = Duration::from_millis(self.config.fetch_retry_delay_ms.max(1_000));
        let mut last_proxy = None;
        let mut last_error = None;
        let mut saw_waiting_message = false;
        let mut saw_no_servers_message = false;

        for attempt in 0..attempts {
            let html = match self.fetch_html() {
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

            let Some(proxy) = probe.proxy else {
                if attempt + 1 < attempts {
                    thread::sleep(retry_delay);
                    continue;
                }
                break;
            };

            if !recent.contains(&proxy) {
                return Ok(ProxyRecord::new(proxy, self.config.source_url.clone()));
            }

            last_proxy = Some(proxy);

            if attempt + 1 < attempts {
                thread::sleep(retry_delay);
            }
        }

        if let Some(proxy) = last_proxy {
            return Err(anyhow!(
                "Источник вернул только недавний proxy: {}",
                proxy.short_label()
            ));
        }

        if saw_no_servers_message {
            return Err(anyhow!(
                "mtproto.ru сейчас не отдаёт proxy: свободных серверов нет"
            ));
        }

        if saw_waiting_message {
            return Err(anyhow!(
                "mtproto.ru не успел выдать proxy за {} попыток по {} мс",
                attempts,
                retry_delay.as_millis()
            ));
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Источник не вернул ни одного proxy")))
    }

    pub fn fetch_html(&self) -> anyhow::Result<String> {
        let response = self
            .client
            .get(&self.config.source_url)
            .send()
            .with_context(|| format!("Не удалось открыть {}", self.config.source_url))?;

        let response = response
            .error_for_status()
            .with_context(|| format!("Источник {} вернул ошибку", self.config.source_url))?;

        response
            .text()
            .context("Не удалось прочитать HTML источника")
    }

    fn inspect_html(&self, html: &str) -> anyhow::Result<HtmlProbe> {
        let proxy = self
            .tg_link_pattern
            .find(html)
            .map(|entry| parse_tg_link(entry.as_str()))
            .transpose()?;

        Ok(HtmlProbe {
            proxy,
            saw_waiting_message: html.contains("Идёт выбор сервера"),
            saw_no_servers_message: html.contains("Свободных серверов"),
        })
    }
}

pub fn parse_tg_link(tg_link: &str) -> anyhow::Result<MtProtoProxy> {
    let url =
        url::Url::parse(tg_link).with_context(|| format!("Невалидный tg:// link: {tg_link}"))?;
    if url.scheme() != "tg" || url.host_str() != Some("proxy") {
        return Err(anyhow!("Ссылка не является tg://proxy"));
    }

    let mut server = None;
    let mut port = None;
    let mut secret = None;

    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "server" => server = Some(value.into_owned()),
            "port" => port = Some(value.parse::<u16>().context("Невалидный port")?),
            "secret" => secret = Some(value.into_owned()),
            _ => {}
        }
    }

    Ok(MtProtoProxy {
        server: server.ok_or_else(|| anyhow!("В tg://proxy нет server"))?,
        port: port.ok_or_else(|| anyhow!("В tg://proxy нет port"))?,
        secret: secret.ok_or_else(|| anyhow!("В tg://proxy нет secret"))?,
    })
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
        assert_eq!(proxy.server, "example.com");
        assert_eq!(proxy.port, 443);
        assert_eq!(proxy.secret, "abcdef123456");
    }

    #[test]
    fn parses_tg_proxy_link() {
        let proxy = parse_tg_link("tg://proxy?server=127.0.0.1&port=8080&secret=abc123").unwrap();
        assert_eq!(proxy.server, "127.0.0.1");
        assert_eq!(proxy.port, 8080);
        assert_eq!(proxy.secret, "abc123");
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
