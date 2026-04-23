use std::fs;
use std::path::{Path, PathBuf};

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes256, Block};
use anyhow::{Context, anyhow};
use md5::{Digest, Md5};
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;

use crate::model::{ProxyKind, TelegramConfig, TelegramProxy};

const TDF_MAGIC: &[u8; 4] = b"TDF$";
const SETTINGS_FILE_STEM: &str = "settings";
const SETTINGS_FILE_NAME: &str = "settingss";
const LOCAL_ENCRYPT_SALT_SIZE: usize = 32;
const LOCAL_ENCRYPT_NO_PWD_ITER_COUNT: u32 = 4;
const DBI_CONNECTION_TYPE_OLD_OLD: u32 = 0x0f;
const DBI_AUTO_START: u32 = 0x06;
const DBI_START_MINIMIZED: u32 = 0x07;
const DBI_SEEN_TRAY_TOOLTIP: u32 = 0x0a;
const DBI_AUTO_UPDATE: u32 = 0x0c;
const DBI_LAST_UPDATE_CHECK: u32 = 0x0d;
const DBI_SEND_TO_MENU: u32 = 0x1d;
const DBI_DIALOG_LAST_PATH: u32 = 0x23;
const DBI_CONNECTION_TYPE_OLD: u32 = 0x4f;
const DBI_THEME_KEY: u32 = 0x54;
const DBI_TILE_BACKGROUND: u32 = 0x55;
const DBI_POWER_SAVING: u32 = 0x57;
const DBI_SCALE_PERCENT: u32 = 0x58;
const DBI_LANGUAGES_KEY: u32 = 0x5a;
const DBI_APPLICATION_SETTINGS: u32 = 0x5e;
const DBI_FALLBACK_PRODUCTION_CONFIG: u32 = 0x60;
const DBI_BACKGROUND_KEY: u32 = 0x61;
const DBI_LANG_PACK_KEY: u32 = 0x4e;
const LEGACY_PROXY_TYPE_SHIFT: i32 = 1024;
const DBICT_PROXIES_LIST: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopProxyType {
    None,
    Socks5,
    Http,
    Mtproto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopProxy {
    pub kind: DesktopProxyType,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

impl DesktopProxy {
    pub fn from_managed(proxy: &TelegramProxy) -> Self {
        match proxy.kind {
            ProxyKind::MtProto => Self {
                kind: DesktopProxyType::Mtproto,
                host: proxy.server.clone(),
                port: proxy.port,
                user: String::new(),
                password: proxy.secret.clone().unwrap_or_default(),
            },
            ProxyKind::Socks5 => Self {
                kind: DesktopProxyType::Socks5,
                host: proxy.server.clone(),
                port: proxy.port,
                user: proxy.username.clone().unwrap_or_default(),
                password: proxy.password.clone().unwrap_or_default(),
            },
        }
    }

    pub fn short_label(&self) -> String {
        match self.kind {
            DesktopProxyType::None => "прямое подключение".to_string(),
            DesktopProxyType::Mtproto => format!(
                "MTProto {}:{} ({})",
                self.host,
                self.port,
                mask_secret(&self.password)
            ),
            DesktopProxyType::Socks5 => format!(
                "SOCKS5 {}:{} ({})",
                self.host,
                self.port,
                if self.user.is_empty() {
                    "open".to_string()
                } else {
                    format!("user {}", compact(&self.user, 12))
                }
            ),
            DesktopProxyType::Http => format!("HTTP {}:{}", self.host, self.port),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopProxyMode {
    System,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopProxySettings {
    pub try_ipv6: bool,
    pub use_proxy_for_calls: bool,
    pub mode: DesktopProxyMode,
    pub selected: Option<DesktopProxy>,
    pub list: Vec<DesktopProxy>,
    pub check_ip_warning_shown: bool,
    pub proxy_rotation_enabled: bool,
    pub proxy_rotation_timeout: i32,
    pub proxy_rotation_preferred_indices: Vec<i32>,
}

impl Default for DesktopProxySettings {
    fn default() -> Self {
        Self {
            try_ipv6: !cfg!(windows),
            use_proxy_for_calls: false,
            mode: DesktopProxyMode::System,
            selected: None,
            list: Vec::new(),
            check_ip_warning_shown: false,
            proxy_rotation_enabled: false,
            proxy_rotation_timeout: 60,
            proxy_rotation_preferred_indices: Vec::new(),
        }
    }
}

impl DesktopProxySettings {
    pub fn upsert_managed_proxy(
        &mut self,
        proxy: &TelegramProxy,
        owned: &[TelegramProxy],
        cleanup_dead_owned: bool,
    ) {
        if cleanup_dead_owned {
            self.list.retain(|candidate| {
                !owned
                    .iter()
                    .any(|managed| desktop_proxy_matches_managed(candidate, managed))
            });
        }

        let candidate = DesktopProxy::from_managed(proxy);
        if let Some(index) = self.list.iter().position(|existing| *existing == candidate) {
            self.list[index] = candidate.clone();
        } else {
            self.list.push(candidate.clone());
        }
        self.selected = Some(candidate);
        self.mode = DesktopProxyMode::Enabled;
        self.proxy_rotation_preferred_indices.clear();
    }

    pub fn cleanup_owned(&mut self, owned: &[TelegramProxy]) -> usize {
        let before = self.list.len();
        self.list.retain(|candidate| {
            !owned
                .iter()
                .any(|managed| desktop_proxy_matches_managed(candidate, managed))
        });
        if self
            .selected
            .as_ref()
            .map(|selected| {
                owned
                    .iter()
                    .any(|managed| desktop_proxy_matches_managed(selected, managed))
            })
            .unwrap_or(false)
        {
            self.selected = None;
            if self.mode == DesktopProxyMode::Enabled {
                self.mode = DesktopProxyMode::System;
            }
        }
        before.saturating_sub(self.list.len())
    }

    pub fn selected_label(&self) -> String {
        match (&self.mode, &self.selected) {
            (DesktopProxyMode::System, _) => "системный proxy".to_string(),
            (DesktopProxyMode::Disabled, _) => "proxy отключён".to_string(),
            (_, Some(proxy)) => proxy.short_label(),
            (_, None) => "proxy не выбран".to_string(),
        }
    }

    pub fn from_proxy_blob(blob: &[u8]) -> anyhow::Result<Self> {
        let mut reader = QtReader::new(blob);
        let try_ipv6 = reader.read_i32()? == 1;
        let use_proxy_for_calls = reader.read_i32()? == 1;
        let mode = proxy_settings_from_i32(reader.read_i32()?)?;
        let selected = parse_proxy_blob(&reader.read_bytearray()?)?;
        let list_count = reader.read_i32()?;
        if list_count < 0 {
            return Err(anyhow!("Telegram вернул отрицательный размер списка proxy"));
        }
        let mut list = Vec::with_capacity(list_count as usize);
        for _ in 0..list_count {
            list.push(
                parse_proxy_blob(&reader.read_bytearray()?)?
                    .ok_or_else(|| anyhow!("Telegram вернул пустой proxy внутри списка"))?,
            );
        }
        let check_ip_warning_shown = reader
            .read_i32_opt()?
            .map(|value| value == 1)
            .unwrap_or(false);
        let proxy_rotation_enabled = reader
            .read_i32_opt()?
            .map(|value| value == 1)
            .unwrap_or(false);
        let proxy_rotation_timeout = reader.read_i32_opt()?.unwrap_or(60);
        let preferred_count = reader.read_i32_opt()?.unwrap_or(0);
        if preferred_count < 0 {
            return Err(anyhow!(
                "Telegram вернул отрицательный размер preferred proxy indices"
            ));
        }
        let mut proxy_rotation_preferred_indices = Vec::with_capacity(preferred_count as usize);
        for _ in 0..preferred_count {
            proxy_rotation_preferred_indices.push(reader.read_i32()?);
        }

        Ok(Self {
            try_ipv6,
            use_proxy_for_calls,
            mode,
            selected,
            list,
            check_ip_warning_shown,
            proxy_rotation_enabled,
            proxy_rotation_timeout,
            proxy_rotation_preferred_indices,
        })
    }

    #[cfg(test)]
    fn to_proxy_blob(&self) -> Vec<u8> {
        let mut writer = QtWriter::new();
        writer.write_i32(if self.try_ipv6 { 1 } else { 0 });
        writer.write_i32(if self.use_proxy_for_calls { 1 } else { 0 });
        writer.write_i32(proxy_settings_to_i32(self.mode));
        writer.write_bytearray(&serialize_proxy(self.selected.as_ref()));
        writer.write_i32(self.list.len() as i32);
        for proxy in &self.list {
            writer.write_bytearray(&serialize_proxy(Some(proxy)));
        }
        writer.write_i32(if self.check_ip_warning_shown { 1 } else { 0 });
        writer.write_i32(if self.proxy_rotation_enabled { 1 } else { 0 });
        writer.write_i32(self.proxy_rotation_timeout.max(1));
        writer.write_i32(self.proxy_rotation_preferred_indices.len() as i32);
        for index in &self.proxy_rotation_preferred_indices {
            writer.write_i32(*index);
        }
        writer.finish()
    }
}

#[derive(Debug, Clone)]
struct SettingsBlock {
    id: u32,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TelegramDesktopSettings {
    version: i32,
    salt: Vec<u8>,
    blocks: Vec<SettingsBlock>,
}

impl TelegramDesktopSettings {
    pub fn load_from_file(path: &Path) -> anyhow::Result<Self> {
        let bytes =
            fs::read(path).with_context(|| format!("Не удалось прочитать {}", path.display()))?;
        parse_settings_file(&bytes)
    }

    pub fn save_to_file(&self, path: &Path) -> anyhow::Result<()> {
        let bytes = self.to_file_bytes()?;
        fs::write(path, bytes)
            .with_context(|| format!("Не удалось записать {}", path.display()))?;
        Ok(())
    }

    pub fn load_from_tdata(tdata_dir: &Path) -> anyhow::Result<Self> {
        Self::load_from_file(&settings_file_path(tdata_dir))
    }

    pub fn save_to_tdata(&self, tdata_dir: &Path) -> anyhow::Result<()> {
        self.save_to_file(&settings_file_path(tdata_dir))
    }

    pub fn proxy_settings(&self) -> anyhow::Result<DesktopProxySettings> {
        let mut result = DesktopProxySettings::default();
        let mut touched = false;

        for block in &self.blocks {
            match block.id {
                DBI_APPLICATION_SETTINGS => {
                    if let Ok(proxy_blob) =
                        extract_proxy_blob_from_application_settings(&block.payload)
                    {
                        result = DesktopProxySettings::from_proxy_blob(&proxy_blob)?;
                        touched = true;
                    }
                }
                DBI_CONNECTION_TYPE_OLD => {
                    result = parse_legacy_connection_type_old(&block.payload)?;
                    touched = true;
                }
                DBI_CONNECTION_TYPE_OLD_OLD => {
                    result = parse_legacy_connection_type_old_old(&block.payload)?;
                    touched = true;
                }
                _ => {}
            }
        }

        if touched {
            Ok(result)
        } else {
            Ok(DesktopProxySettings::default())
        }
    }

    pub fn set_legacy_proxy_override(&mut self, settings: &DesktopProxySettings) {
        self.blocks.retain(|block| {
            block.id != DBI_CONNECTION_TYPE_OLD && block.id != DBI_CONNECTION_TYPE_OLD_OLD
        });
        self.blocks.push(SettingsBlock {
            id: DBI_CONNECTION_TYPE_OLD,
            payload: serialize_legacy_connection_type_old(settings),
        });
    }

    fn to_file_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let salt = if self.salt.len() == LOCAL_ENCRYPT_SALT_SIZE {
            self.salt.clone()
        } else {
            return Err(anyhow!("Telegram settings salt повреждён"));
        };

        let key = derive_legacy_local_key(&salt);
        let plaintext = serialize_settings_blocks(&self.blocks);
        let encrypted = encrypt_local_payload(&plaintext, &key);

        let mut safe_data = QtWriter::new();
        safe_data.write_bytearray(&salt);
        safe_data.write_bytearray(&encrypted);
        let safe_data = safe_data.finish();

        let mut bytes = Vec::with_capacity(TDF_MAGIC.len() + 4 + safe_data.len() + 16);
        bytes.extend_from_slice(TDF_MAGIC);
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(&safe_data);
        bytes.extend_from_slice(&settings_file_md5(&safe_data, self.version));
        Ok(bytes)
    }
}

pub fn resolve_telegram_data_dir(config: &TelegramConfig) -> anyhow::Result<PathBuf> {
    if let Some(override_path) = config.data_dir.as_deref() {
        let candidate = PathBuf::from(override_path);
        if candidate.join(SETTINGS_FILE_NAME).exists() {
            return Ok(candidate);
        }
        if candidate.join("tdata").join(SETTINGS_FILE_NAME).exists() {
            return Ok(candidate.join("tdata"));
        }
        return Err(anyhow!(
            "В telegram.data_dir не найден {} или tdata/{}",
            SETTINGS_FILE_NAME,
            SETTINGS_FILE_NAME
        ));
    }

    detect_telegram_data_dir()
        .ok_or_else(|| anyhow!("Не удалось определить каталог Telegram Desktop tdata"))
}

pub fn detect_telegram_data_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    #[cfg(windows)]
    {
        if let Ok(value) = std::env::var("APPDATA") {
            candidates.push(PathBuf::from(&value).join("Telegram Desktop").join("tdata"));
        }
        if let Ok(value) = std::env::var("LOCALAPPDATA") {
            let root = PathBuf::from(&value);
            candidates.push(root.join("Telegram Desktop").join("tdata"));
            candidates.push(root.join("Programs").join("Telegram Desktop").join("tdata"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
        candidates.push(home.join(".local/share/TelegramDesktop/tdata"));
        candidates.push(home.join(".local/share/Telegram Desktop/tdata"));
        candidates.push(home.join(".TelegramDesktop/tdata"));
        if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
            candidates.push(PathBuf::from(xdg_config_home).join("TelegramDesktop/tdata"));
        }
        if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
            candidates.push(PathBuf::from(xdg_data_home).join("TelegramDesktop/tdata"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
        candidates.push(
            home.join("Library")
                .join("Application Support")
                .join("Telegram Desktop")
                .join("tdata"),
        );
    }

    candidates.into_iter().find(|candidate| {
        candidate.join(SETTINGS_FILE_NAME).exists() || candidate.join(SETTINGS_FILE_STEM).exists()
    })
}

pub fn load_proxy_settings(config: &TelegramConfig) -> anyhow::Result<DesktopProxySettings> {
    let tdata_dir = resolve_telegram_data_dir(config)?;
    TelegramDesktopSettings::load_from_tdata(&tdata_dir)?.proxy_settings()
}

pub fn write_proxy_settings_override(
    config: &TelegramConfig,
    settings: &DesktopProxySettings,
) -> anyhow::Result<PathBuf> {
    let tdata_dir = resolve_telegram_data_dir(config)?;
    let mut file = TelegramDesktopSettings::load_from_tdata(&tdata_dir)?;
    file.set_legacy_proxy_override(settings);
    file.save_to_tdata(&tdata_dir)?;
    Ok(settings_file_path(&tdata_dir))
}

fn settings_file_path(tdata_dir: &Path) -> PathBuf {
    let modern = tdata_dir.join(SETTINGS_FILE_NAME);
    if modern.exists() {
        modern
    } else {
        tdata_dir.join(SETTINGS_FILE_STEM)
    }
}

fn parse_settings_file(bytes: &[u8]) -> anyhow::Result<TelegramDesktopSettings> {
    if bytes.len() < TDF_MAGIC.len() + 4 + 16 {
        return Err(anyhow!("Файл Telegram settings слишком короткий"));
    }
    if &bytes[..TDF_MAGIC.len()] != TDF_MAGIC {
        return Err(anyhow!("Файл Telegram settings имеет неверную сигнатуру"));
    }

    let version = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let data_end = bytes.len().saturating_sub(16);
    let safe_data = &bytes[8..data_end];
    let expected_md5 = settings_file_md5(safe_data, version);
    if expected_md5.as_slice() != &bytes[data_end..] {
        return Err(anyhow!("Файл Telegram settings повреждён: md5 не совпал"));
    }

    let mut reader = QtReader::new(safe_data);
    let salt = reader.read_bytearray()?;
    let encrypted = reader.read_bytearray()?;
    if salt.len() != LOCAL_ENCRYPT_SALT_SIZE {
        return Err(anyhow!("Telegram settings содержит salt неверной длины"));
    }

    let key = derive_legacy_local_key(&salt);
    let plaintext = decrypt_local_payload(&encrypted, &key)?;
    let blocks = parse_settings_blocks(&plaintext)?;

    Ok(TelegramDesktopSettings {
        version,
        salt,
        blocks,
    })
}

fn parse_settings_blocks(bytes: &[u8]) -> anyhow::Result<Vec<SettingsBlock>> {
    let mut reader = QtReader::new(bytes);
    let mut blocks = Vec::new();

    while !reader.is_at_end() {
        let id = reader.read_u32()?;
        let start = reader.position();
        match id {
            DBI_AUTO_START
            | DBI_START_MINIMIZED
            | DBI_SEEN_TRAY_TOOLTIP
            | DBI_AUTO_UPDATE
            | DBI_LAST_UPDATE_CHECK
            | DBI_SEND_TO_MENU
            | DBI_POWER_SAVING
            | DBI_SCALE_PERCENT => {
                reader.read_i32()?;
            }
            DBI_FALLBACK_PRODUCTION_CONFIG | DBI_APPLICATION_SETTINGS => {
                reader.skip_bytearray()?;
            }
            DBI_DIALOG_LAST_PATH => {
                reader.skip_string()?;
            }
            DBI_THEME_KEY => {
                reader.read_u64()?;
                reader.read_u64()?;
                reader.read_u32()?;
            }
            DBI_BACKGROUND_KEY => {
                reader.read_u64()?;
                reader.read_u64()?;
            }
            DBI_TILE_BACKGROUND => {
                reader.read_i32()?;
                reader.read_i32()?;
            }
            DBI_LANG_PACK_KEY | DBI_LANGUAGES_KEY => {
                reader.read_u64()?;
            }
            DBI_CONNECTION_TYPE_OLD => {
                skip_legacy_connection_type_old(&mut reader)?;
            }
            DBI_CONNECTION_TYPE_OLD_OLD => {
                skip_legacy_connection_type_old_old(&mut reader)?;
            }
            _ => {
                return Err(anyhow!("Неизвестный блок Telegram settings: 0x{id:02x}"));
            }
        }
        let end = reader.position();
        blocks.push(SettingsBlock {
            id,
            payload: bytes[start..end].to_vec(),
        });
    }

    Ok(blocks)
}

fn serialize_settings_blocks(blocks: &[SettingsBlock]) -> Vec<u8> {
    let mut writer = QtWriter::new();
    for block in blocks {
        writer.write_u32(block.id);
        writer.write_raw(&block.payload);
    }
    writer.finish()
}

fn extract_proxy_blob_from_application_settings(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut reader = QtReader::new(bytes);
    reader.skip_bytearray()?;
    for _ in 0..5 {
        reader.read_i32()?;
    }
    reader.skip_string()?;
    reader.skip_bytearray()?;
    for _ in 0..9 {
        reader.read_i32()?;
    }
    reader.skip_string()?;
    reader.skip_string()?;
    for _ in 0..5 {
        reader.read_i32()?;
    }
    let sound_overrides_count = reader.read_i32()?;
    if sound_overrides_count < 0 {
        return Err(anyhow!(
            "Telegram settings содержит отрицательный soundOverridesCount"
        ));
    }
    for _ in 0..sound_overrides_count {
        reader.skip_string()?;
        reader.skip_string()?;
    }
    for _ in 0..13 {
        reader.read_i32()?;
    }
    reader.skip_bytearray()?;
    let dictionaries_count = reader.read_i32()?;
    if dictionaries_count < 0 {
        return Err(anyhow!(
            "Telegram settings содержит отрицательный dictionaries count"
        ));
    }
    for _ in 0..dictionaries_count {
        reader.read_u64()?;
    }
    for _ in 0..12 {
        reader.read_i32()?;
    }
    reader.skip_string()?;
    for _ in 0..2 {
        reader.read_i32()?;
    }
    reader.skip_bytearray()?;
    reader.read_i64()?;
    for _ in 0..2 {
        reader.read_i32()?;
    }
    reader.skip_bytearray()?;
    let recent_emoji_count = reader.read_i32()?;
    if recent_emoji_count < 0 {
        return Err(anyhow!(
            "Telegram settings содержит отрицательный recentEmoji count"
        ));
    }
    for _ in 0..recent_emoji_count {
        reader.skip_string()?;
        reader.read_u16()?;
    }
    let emoji_variants_count = reader.read_i32()?;
    if emoji_variants_count < 0 {
        return Err(anyhow!(
            "Telegram settings содержит отрицательный emojiVariants count"
        ));
    }
    for _ in 0..emoji_variants_count {
        reader.skip_string()?;
        reader.read_u8()?;
    }
    reader.read_i32()?;
    reader.read_i32()?;
    reader.read_i32()?;
    reader.read_bytearray()
}

fn parse_legacy_connection_type_old(bytes: &[u8]) -> anyhow::Result<DesktopProxySettings> {
    let mut reader = QtReader::new(bytes);
    let connection_type = reader.read_i32()?;
    if connection_type != DBICT_PROXIES_LIST {
        return Err(anyhow!(
            "Telegram legacy proxy override имеет неподдерживаемый тип"
        ));
    }

    let count = reader.read_i32()?;
    let index = reader.read_i32()?;
    let settings = proxy_settings_from_i32(reader.read_i32()?)?;
    let use_proxy_for_calls = reader.read_i32()? == 1;
    if count < 0 {
        return Err(anyhow!(
            "Telegram legacy proxy override вернул отрицательный count"
        ));
    }

    let mut list = Vec::with_capacity(count as usize);
    for _ in 0..count {
        list.push(read_legacy_proxy(&mut reader)?);
    }

    let selected = if index > 0 && (index as usize) <= list.len() {
        Some(list[index as usize - 1].clone())
    } else {
        None
    };

    Ok(DesktopProxySettings {
        use_proxy_for_calls,
        mode: settings,
        selected,
        list,
        ..DesktopProxySettings::default()
    })
}

fn parse_legacy_connection_type_old_old(bytes: &[u8]) -> anyhow::Result<DesktopProxySettings> {
    let mut reader = QtReader::new(bytes);
    let connection_type = reader.read_i32()?;
    let proxy = match connection_type {
        2 | 3 => Some(DesktopProxy {
            kind: if connection_type == 3 {
                DesktopProxyType::Socks5
            } else {
                DesktopProxyType::Http
            },
            host: reader.read_string()?,
            port: reader.read_i32()?.try_into().unwrap_or_default(),
            user: reader.read_string()?,
            password: reader.read_string()?,
        }),
        _ => None,
    };

    Ok(DesktopProxySettings {
        mode: if proxy.is_some() {
            DesktopProxyMode::Enabled
        } else {
            DesktopProxyMode::System
        },
        selected: proxy.clone(),
        list: proxy.into_iter().collect(),
        ..DesktopProxySettings::default()
    })
}

fn serialize_legacy_connection_type_old(settings: &DesktopProxySettings) -> Vec<u8> {
    let mut writer = QtWriter::new();
    writer.write_i32(DBICT_PROXIES_LIST);
    writer.write_i32(settings.list.len() as i32);
    let selected_index = settings
        .selected
        .as_ref()
        .and_then(|selected| {
            settings
                .list
                .iter()
                .position(|candidate| candidate == selected)
        })
        .map(|index| index as i32 + 1)
        .unwrap_or(0);
    writer.write_i32(selected_index);
    writer.write_i32(proxy_settings_to_i32(settings.mode));
    writer.write_i32(if settings.use_proxy_for_calls { 1 } else { 0 });
    for proxy in &settings.list {
        write_legacy_proxy(&mut writer, proxy);
    }
    writer.finish()
}

fn skip_legacy_connection_type_old(reader: &mut QtReader<'_>) -> anyhow::Result<()> {
    let connection_type = reader.read_i32()?;
    if connection_type == DBICT_PROXIES_LIST {
        let count = reader.read_i32()?;
        reader.read_i32()?;
        reader.read_i32()?;
        reader.read_i32()?;
        if count < 0 {
            return Err(anyhow!(
                "Telegram legacy proxy override вернул отрицательный count"
            ));
        }
        for _ in 0..count {
            skip_legacy_proxy(reader)?;
        }
        Ok(())
    } else {
        Err(anyhow!(
            "Telegram legacy proxy override имеет неподдерживаемый тип"
        ))
    }
}

fn skip_legacy_connection_type_old_old(reader: &mut QtReader<'_>) -> anyhow::Result<()> {
    let connection_type = reader.read_i32()?;
    if matches!(connection_type, 2 | 3) {
        reader.skip_string()?;
        reader.read_i32()?;
        reader.skip_string()?;
        reader.skip_string()?;
    }
    Ok(())
}

fn read_legacy_proxy(reader: &mut QtReader<'_>) -> anyhow::Result<DesktopProxy> {
    let proxy_type = reader.read_i32()?;
    let kind = match proxy_type {
        3 | 1025 => DesktopProxyType::Socks5,
        2 | 1026 => DesktopProxyType::Http,
        1027 => DesktopProxyType::Mtproto,
        _ => DesktopProxyType::None,
    };

    Ok(DesktopProxy {
        kind,
        host: reader.read_string()?,
        port: reader.read_i32()?.try_into().unwrap_or_default(),
        user: reader.read_string()?,
        password: reader.read_string()?,
    })
}

fn write_legacy_proxy(writer: &mut QtWriter, proxy: &DesktopProxy) {
    let proxy_type = match proxy.kind {
        DesktopProxyType::None => 0,
        DesktopProxyType::Socks5 => LEGACY_PROXY_TYPE_SHIFT + 1,
        DesktopProxyType::Http => LEGACY_PROXY_TYPE_SHIFT + 2,
        DesktopProxyType::Mtproto => LEGACY_PROXY_TYPE_SHIFT + 3,
    };

    writer.write_i32(proxy_type);
    writer.write_string(&proxy.host);
    writer.write_i32(proxy.port as i32);
    writer.write_string(&proxy.user);
    writer.write_string(&proxy.password);
}

fn skip_legacy_proxy(reader: &mut QtReader<'_>) -> anyhow::Result<()> {
    reader.read_i32()?;
    reader.skip_string()?;
    reader.read_i32()?;
    reader.skip_string()?;
    reader.skip_string()?;
    Ok(())
}

fn parse_proxy_blob(bytes: &[u8]) -> anyhow::Result<Option<DesktopProxy>> {
    if bytes.is_empty() {
        return Ok(None);
    }

    let mut reader = QtReader::new(bytes);
    let proxy_type = reader.read_i32()?;
    let kind = match proxy_type {
        0 => DesktopProxyType::None,
        1 => DesktopProxyType::Socks5,
        2 => DesktopProxyType::Http,
        3 => DesktopProxyType::Mtproto,
        _ => {
            return Err(anyhow!(
                "Telegram вернул неизвестный proxy type: {proxy_type}"
            ));
        }
    };
    let proxy = DesktopProxy {
        kind,
        host: reader.read_string()?,
        port: reader.read_i32()?.try_into().unwrap_or_default(),
        user: reader.read_string()?,
        password: reader.read_string()?,
    };

    if proxy.kind == DesktopProxyType::None {
        Ok(None)
    } else {
        Ok(Some(proxy))
    }
}

#[cfg(test)]
fn serialize_proxy(proxy: Option<&DesktopProxy>) -> Vec<u8> {
    let mut writer = QtWriter::new();
    let proxy = proxy.cloned().unwrap_or(DesktopProxy {
        kind: DesktopProxyType::None,
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
    });
    let proxy_type = match proxy.kind {
        DesktopProxyType::None => 0,
        DesktopProxyType::Socks5 => 1,
        DesktopProxyType::Http => 2,
        DesktopProxyType::Mtproto => 3,
    };
    writer.write_i32(proxy_type);
    writer.write_string(&proxy.host);
    writer.write_i32(proxy.port as i32);
    writer.write_string(&proxy.user);
    writer.write_string(&proxy.password);
    writer.finish()
}

fn desktop_proxy_matches_managed(proxy: &DesktopProxy, managed: &TelegramProxy) -> bool {
    match (&proxy.kind, &managed.kind) {
        (DesktopProxyType::Mtproto, ProxyKind::MtProto) => {
            proxy.host.eq_ignore_ascii_case(&managed.server)
                && proxy.port == managed.port
                && proxy.password == managed.secret.clone().unwrap_or_default()
        }
        (DesktopProxyType::Socks5, ProxyKind::Socks5) => {
            proxy.host.eq_ignore_ascii_case(&managed.server)
                && proxy.port == managed.port
                && proxy.user == managed.username.clone().unwrap_or_default()
                && proxy.password == managed.password.clone().unwrap_or_default()
        }
        _ => false,
    }
}

fn derive_legacy_local_key(salt: &[u8]) -> [u8; 256] {
    let mut key = [0_u8; 256];
    pbkdf2_hmac::<Sha1>(b"", salt, LOCAL_ENCRYPT_NO_PWD_ITER_COUNT, &mut key);
    key
}

fn decrypt_local_payload(encrypted: &[u8], auth_key: &[u8; 256]) -> anyhow::Result<Vec<u8>> {
    if encrypted.len() <= 16 || !encrypted.len().is_multiple_of(16) {
        return Err(anyhow!("Зашифрованная часть Telegram settings повреждена"));
    }

    let (msg_key, ciphertext) = encrypted.split_at(16);
    let (aes_key, aes_iv) = prepare_aes_oldmtp(auth_key, msg_key, false);
    let decrypted = aes_ige_decrypt(ciphertext, &aes_key, &aes_iv)?;

    let mut sha1 = Sha1::new();
    sha1.update(&decrypted);
    let hash = sha1.finalize();
    if &hash[..16] != msg_key {
        return Err(anyhow!("Не удалось расшифровать Telegram settings"));
    }

    if decrypted.len() < 4 {
        return Err(anyhow!(
            "Telegram settings содержит пустой decrypted payload"
        ));
    }
    let data_len = u32::from_le_bytes(decrypted[..4].try_into().unwrap()) as usize;
    if data_len < 4 || data_len > decrypted.len() || data_len <= decrypted.len().saturating_sub(16)
    {
        return Err(anyhow!(
            "Telegram settings содержит неверный decrypted size"
        ));
    }

    Ok(decrypted[4..data_len].to_vec())
}

fn encrypt_local_payload(plaintext: &[u8], auth_key: &[u8; 256]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + plaintext.len() + 16);
    body.extend_from_slice(&((plaintext.len() + 4) as u32).to_le_bytes());
    body.extend_from_slice(plaintext);
    while body.len() % 16 != 0 {
        body.push(0);
    }

    let mut sha1 = Sha1::new();
    sha1.update(&body);
    let hash = sha1.finalize();
    let msg_key = &hash[..16];
    let (aes_key, aes_iv) = prepare_aes_oldmtp(auth_key, msg_key, false);
    let ciphertext = aes_ige_encrypt(&body, &aes_key, &aes_iv)
        .expect("aes_ige_encrypt receives padded local payload");

    let mut result = Vec::with_capacity(16 + ciphertext.len());
    result.extend_from_slice(msg_key);
    result.extend_from_slice(&ciphertext);
    result
}

fn prepare_aes_oldmtp(auth_key: &[u8; 256], msg_key: &[u8], send: bool) -> ([u8; 32], [u8; 32]) {
    let x = if send { 0 } else { 8 };

    let mut sha1_a = Sha1::new();
    sha1_a.update(msg_key);
    sha1_a.update(&auth_key[x..x + 32]);
    let sha1_a = sha1_a.finalize();

    let mut sha1_b = Sha1::new();
    sha1_b.update(&auth_key[32 + x..48 + x]);
    sha1_b.update(msg_key);
    sha1_b.update(&auth_key[48 + x..64 + x]);
    let sha1_b = sha1_b.finalize();

    let mut sha1_c = Sha1::new();
    sha1_c.update(&auth_key[64 + x..96 + x]);
    sha1_c.update(msg_key);
    let sha1_c = sha1_c.finalize();

    let mut sha1_d = Sha1::new();
    sha1_d.update(msg_key);
    sha1_d.update(&auth_key[96 + x..128 + x]);
    let sha1_d = sha1_d.finalize();

    let mut aes_key = [0_u8; 32];
    aes_key[..8].copy_from_slice(&sha1_a[..8]);
    aes_key[8..20].copy_from_slice(&sha1_b[8..20]);
    aes_key[20..32].copy_from_slice(&sha1_c[4..16]);

    let mut aes_iv = [0_u8; 32];
    aes_iv[..12].copy_from_slice(&sha1_a[8..20]);
    aes_iv[12..20].copy_from_slice(&sha1_b[..8]);
    aes_iv[20..24].copy_from_slice(&sha1_c[16..20]);
    aes_iv[24..32].copy_from_slice(&sha1_d[..8]);

    (aes_key, aes_iv)
}

fn aes_ige_encrypt(bytes: &[u8], key: &[u8; 32], iv: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    if !bytes.len().is_multiple_of(16) {
        return Err(anyhow!("AES-IGE требует размер, кратный 16"));
    }

    let cipher = Aes256::new_from_slice(key).unwrap();
    let mut result = vec![0_u8; bytes.len()];
    let mut iv1 = <[u8; 16]>::try_from(&iv[..16]).unwrap();
    let mut iv2 = <[u8; 16]>::try_from(&iv[16..32]).unwrap();

    for (index, chunk) in bytes.chunks(16).enumerate() {
        let mut block = xor_16(chunk.try_into().unwrap(), &iv1);
        let mut ga = Block::clone_from_slice(&block);
        cipher.encrypt_block(&mut ga);
        block = xor_16(&ga.into(), &iv2);
        result[index * 16..index * 16 + 16].copy_from_slice(&block);
        iv1 = block;
        iv2 = chunk.try_into().unwrap();
    }

    Ok(result)
}

fn aes_ige_decrypt(bytes: &[u8], key: &[u8; 32], iv: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    if !bytes.len().is_multiple_of(16) {
        return Err(anyhow!("AES-IGE требует размер, кратный 16"));
    }

    let cipher = Aes256::new_from_slice(key).unwrap();
    let mut result = vec![0_u8; bytes.len()];
    let mut iv1 = <[u8; 16]>::try_from(&iv[..16]).unwrap();
    let mut iv2 = <[u8; 16]>::try_from(&iv[16..32]).unwrap();

    for (index, chunk) in bytes.chunks(16).enumerate() {
        let mut block = xor_16(chunk.try_into().unwrap(), &iv2);
        let mut ga = Block::clone_from_slice(&block);
        cipher.decrypt_block(&mut ga);
        block = xor_16(&ga.into(), &iv1);
        result[index * 16..index * 16 + 16].copy_from_slice(&block);
        iv1 = chunk.try_into().unwrap();
        iv2 = block;
    }

    Ok(result)
}

fn xor_16(left: &[u8; 16], right: &[u8; 16]) -> [u8; 16] {
    let mut result = [0_u8; 16];
    for index in 0..16 {
        result[index] = left[index] ^ right[index];
    }
    result
}

fn settings_file_md5(safe_data: &[u8], version: i32) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(safe_data);
    hasher.update((safe_data.len() as i32).to_le_bytes());
    hasher.update(version.to_le_bytes());
    hasher.update(TDF_MAGIC);
    let hash = hasher.finalize();
    hash[..16].try_into().unwrap()
}

fn proxy_settings_from_i32(value: i32) -> anyhow::Result<DesktopProxyMode> {
    match value {
        0 => Ok(DesktopProxyMode::System),
        1 => Ok(DesktopProxyMode::Enabled),
        2 => Ok(DesktopProxyMode::Disabled),
        _ => Err(anyhow!(
            "Telegram вернул неизвестный proxy settings mode: {value}"
        )),
    }
}

fn proxy_settings_to_i32(value: DesktopProxyMode) -> i32 {
    match value {
        DesktopProxyMode::System => 0,
        DesktopProxyMode::Enabled => 1,
        DesktopProxyMode::Disabled => 2,
    }
}

fn mask_secret(value: &str) -> String {
    if value.chars().count() <= 12 {
        return value.to_string();
    }
    let prefix = value.chars().take(8).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

fn compact(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let head = max.saturating_sub(3) / 2;
    let tail = max.saturating_sub(3) - head;
    let prefix = value.chars().take(head).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

struct QtReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> QtReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn position(&self) -> usize {
        self.position
    }

    fn is_at_end(&self) -> bool {
        self.position >= self.bytes.len()
    }

    fn read_exact(&mut self, len: usize) -> anyhow::Result<&'a [u8]> {
        if self.position + len > self.bytes.len() {
            return Err(anyhow!("Telegram settings обрывается на середине блока"));
        }
        let slice = &self.bytes[self.position..self.position + len];
        self.position += len;
        Ok(slice)
    }

    fn read_u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_be_bytes(self.read_exact(2)?.try_into().unwrap()))
    }

    fn read_u32(&mut self) -> anyhow::Result<u32> {
        Ok(u32::from_be_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    fn read_i32(&mut self) -> anyhow::Result<i32> {
        Ok(i32::from_be_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    fn read_i32_opt(&mut self) -> anyhow::Result<Option<i32>> {
        if self.is_at_end() {
            Ok(None)
        } else {
            self.read_i32().map(Some)
        }
    }

    fn read_i64(&mut self) -> anyhow::Result<i64> {
        Ok(i64::from_be_bytes(self.read_exact(8)?.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> anyhow::Result<u64> {
        Ok(u64::from_be_bytes(self.read_exact(8)?.try_into().unwrap()))
    }

    fn read_bytearray(&mut self) -> anyhow::Result<Vec<u8>> {
        let len = self.read_u32()?;
        if len == u32::MAX {
            return Ok(Vec::new());
        }
        Ok(self.read_exact(len as usize)?.to_vec())
    }

    fn skip_bytearray(&mut self) -> anyhow::Result<()> {
        let len = self.read_u32()?;
        if len != u32::MAX {
            self.read_exact(len as usize)?;
        }
        Ok(())
    }

    fn read_string(&mut self) -> anyhow::Result<String> {
        let len = self.read_u32()?;
        if len == u32::MAX {
            return Ok(String::new());
        }
        if len % 2 != 0 {
            return Err(anyhow!(
                "Telegram settings содержит строку с нечётной длиной"
            ));
        }
        let raw = self.read_exact(len as usize)?;
        let mut units = Vec::with_capacity(raw.len() / 2);
        for chunk in raw.chunks_exact(2) {
            units.push(u16::from_be_bytes([chunk[0], chunk[1]]));
        }
        Ok(String::from_utf16_lossy(&units))
    }

    fn skip_string(&mut self) -> anyhow::Result<()> {
        let len = self.read_u32()?;
        if len != u32::MAX {
            self.read_exact(len as usize)?;
        }
        Ok(())
    }
}

struct QtWriter {
    bytes: Vec<u8>,
}

impl QtWriter {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn write_raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn write_u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn write_u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn write_i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn write_string(&mut self, value: &str) {
        let encoded = value.encode_utf16().collect::<Vec<_>>();
        self.write_u32((encoded.len() * 2) as u32);
        for unit in encoded {
            self.write_u16(unit);
        }
    }

    fn write_bytearray(&mut self, value: &[u8]) {
        self.write_u32(value.len() as u32);
        self.write_raw(value);
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn proxy_blob_roundtrip_preserves_mtproto_and_socks5() {
        let settings = DesktopProxySettings {
            try_ipv6: false,
            use_proxy_for_calls: true,
            mode: DesktopProxyMode::Enabled,
            selected: Some(DesktopProxy {
                kind: DesktopProxyType::Mtproto,
                host: "ovh.example.com".to_string(),
                port: 443,
                user: String::new(),
                password: "ee112233445566".to_string(),
            }),
            list: vec![
                DesktopProxy {
                    kind: DesktopProxyType::Mtproto,
                    host: "ovh.example.com".to_string(),
                    port: 443,
                    user: String::new(),
                    password: "ee112233445566".to_string(),
                },
                DesktopProxy {
                    kind: DesktopProxyType::Socks5,
                    host: "1.2.3.4".to_string(),
                    port: 1080,
                    user: "demo".to_string(),
                    password: "pass".to_string(),
                },
            ],
            check_ip_warning_shown: true,
            proxy_rotation_enabled: true,
            proxy_rotation_timeout: 90,
            proxy_rotation_preferred_indices: vec![1, 0],
        };

        let blob = settings.to_proxy_blob();
        let restored = DesktopProxySettings::from_proxy_blob(&blob).unwrap();
        assert_eq!(restored, settings);
    }

    #[test]
    fn application_settings_extractor_finds_proxy_blob() {
        let proxy_blob = DesktopProxySettings {
            selected: Some(DesktopProxy {
                kind: DesktopProxyType::Mtproto,
                host: "proxy.telegram".to_string(),
                port: 443,
                user: String::new(),
                password: "secret".to_string(),
            }),
            list: vec![DesktopProxy {
                kind: DesktopProxyType::Mtproto,
                host: "proxy.telegram".to_string(),
                port: 443,
                user: String::new(),
                password: "secret".to_string(),
            }],
            mode: DesktopProxyMode::Enabled,
            ..DesktopProxySettings::default()
        }
        .to_proxy_blob();
        let bytes = build_application_settings_prefix(&proxy_blob);
        let extracted = extract_proxy_blob_from_application_settings(&bytes).unwrap();
        assert_eq!(extracted, proxy_blob);
    }

    #[test]
    fn settings_file_roundtrip_keeps_legacy_override() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("settingss");
        let mut settings = TelegramDesktopSettings {
            version: 5_001_001,
            salt: vec![7_u8; LOCAL_ENCRYPT_SALT_SIZE],
            blocks: vec![
                SettingsBlock {
                    id: DBI_AUTO_START,
                    payload: {
                        let mut writer = QtWriter::new();
                        writer.write_i32(0);
                        writer.finish()
                    },
                },
                SettingsBlock {
                    id: DBI_APPLICATION_SETTINGS,
                    payload: {
                        let proxy_blob = DesktopProxySettings::default().to_proxy_blob();
                        let application_settings = build_application_settings_prefix(&proxy_blob);
                        let mut writer = QtWriter::new();
                        writer.write_bytearray(&application_settings);
                        writer.finish()
                    },
                },
            ],
        };
        settings.set_legacy_proxy_override(&DesktopProxySettings {
            mode: DesktopProxyMode::Enabled,
            selected: Some(DesktopProxy {
                kind: DesktopProxyType::Socks5,
                host: "127.0.0.1".to_string(),
                port: 9050,
                user: String::new(),
                password: String::new(),
            }),
            list: vec![DesktopProxy {
                kind: DesktopProxyType::Socks5,
                host: "127.0.0.1".to_string(),
                port: 9050,
                user: String::new(),
                password: String::new(),
            }],
            ..DesktopProxySettings::default()
        });
        settings.save_to_file(&path).unwrap();

        let restored = TelegramDesktopSettings::load_from_file(&path).unwrap();
        let proxy = restored.proxy_settings().unwrap();
        assert_eq!(proxy.mode, DesktopProxyMode::Enabled);
        assert_eq!(proxy.selected.unwrap().host, "127.0.0.1");
    }

    #[test]
    fn local_payload_crypto_roundtrip() {
        let salt = vec![7_u8; LOCAL_ENCRYPT_SALT_SIZE];
        let key = derive_legacy_local_key(&salt);
        let plaintext = vec![0, 0, 0, 6, 0, 0, 0, DBI_AUTO_START as u8];
        let encrypted = encrypt_local_payload(&plaintext, &key);
        let restored = decrypt_local_payload(&encrypted, &key).unwrap();
        assert_eq!(restored, plaintext);
    }

    #[test]
    fn settings_blocks_roundtrip() {
        let blocks = vec![
            SettingsBlock {
                id: DBI_AUTO_START,
                payload: {
                    let mut writer = QtWriter::new();
                    writer.write_i32(0);
                    writer.finish()
                },
            },
            SettingsBlock {
                id: DBI_APPLICATION_SETTINGS,
                payload: {
                    let mut writer = QtWriter::new();
                    writer.write_bytearray(&[1, 2, 3, 4]);
                    writer.finish()
                },
            },
            SettingsBlock {
                id: DBI_CONNECTION_TYPE_OLD,
                payload: serialize_legacy_connection_type_old(&DesktopProxySettings {
                    mode: DesktopProxyMode::Enabled,
                    selected: Some(DesktopProxy {
                        kind: DesktopProxyType::Mtproto,
                        host: "managed.example".to_string(),
                        port: 443,
                        user: String::new(),
                        password: "secret".to_string(),
                    }),
                    list: vec![DesktopProxy {
                        kind: DesktopProxyType::Mtproto,
                        host: "managed.example".to_string(),
                        port: 443,
                        user: String::new(),
                        password: "secret".to_string(),
                    }],
                    ..DesktopProxySettings::default()
                }),
            },
        ];

        let raw = serialize_settings_blocks(&blocks);
        let restored = parse_settings_blocks(&raw).unwrap();
        assert_eq!(restored[0].id, DBI_AUTO_START);
        assert_eq!(restored[1].id, DBI_APPLICATION_SETTINGS);
        assert_eq!(restored[2].id, DBI_CONNECTION_TYPE_OLD);
    }

    #[test]
    fn settings_file_bytes_restore_plain_blocks() {
        let settings = TelegramDesktopSettings {
            version: 5_001_001,
            salt: vec![7_u8; LOCAL_ENCRYPT_SALT_SIZE],
            blocks: vec![
                SettingsBlock {
                    id: DBI_AUTO_START,
                    payload: {
                        let mut writer = QtWriter::new();
                        writer.write_i32(0);
                        writer.finish()
                    },
                },
                SettingsBlock {
                    id: DBI_APPLICATION_SETTINGS,
                    payload: {
                        let mut writer = QtWriter::new();
                        writer.write_bytearray(&[1, 2, 3, 4]);
                        writer.finish()
                    },
                },
            ],
        };

        let bytes = settings.to_file_bytes().unwrap();
        let version = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let data_end = bytes.len() - 16;
        let safe_data = &bytes[8..data_end];
        let mut reader = QtReader::new(safe_data);
        let salt = reader.read_bytearray().unwrap();
        let encrypted = reader.read_bytearray().unwrap();
        let key = derive_legacy_local_key(&salt);
        let plaintext = decrypt_local_payload(&encrypted, &key).unwrap();
        assert_eq!(version, settings.version);
        assert_eq!(plaintext, serialize_settings_blocks(&settings.blocks));
    }

    #[test]
    fn managed_cleanup_keeps_http_entries() {
        let managed = TelegramProxy::mtproto("managed.example", 443, "secret");
        let mut settings = DesktopProxySettings {
            selected: Some(DesktopProxy::from_managed(&managed)),
            list: vec![
                DesktopProxy {
                    kind: DesktopProxyType::Http,
                    host: "user-http".to_string(),
                    port: 8080,
                    user: "alice".to_string(),
                    password: "token".to_string(),
                },
                DesktopProxy::from_managed(&managed),
            ],
            mode: DesktopProxyMode::Enabled,
            ..DesktopProxySettings::default()
        };

        let removed = settings.cleanup_owned(&[managed]);
        assert_eq!(removed, 1);
        assert_eq!(settings.list.len(), 1);
        assert_eq!(settings.list[0].kind, DesktopProxyType::Http);
    }

    fn build_application_settings_prefix(proxy_blob: &[u8]) -> Vec<u8> {
        let mut writer = QtWriter::new();
        writer.write_bytearray(&[]);
        for _ in 0..5 {
            writer.write_i32(0);
        }
        writer.write_string("");
        writer.write_bytearray(&[]);
        for _ in 0..9 {
            writer.write_i32(0);
        }
        writer.write_string("");
        writer.write_string("");
        for _ in 0..5 {
            writer.write_i32(0);
        }
        writer.write_i32(0);
        for _ in 0..13 {
            writer.write_i32(0);
        }
        writer.write_bytearray(&[]);
        writer.write_i32(0);
        for _ in 0..12 {
            writer.write_i32(0);
        }
        writer.write_string("");
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_bytearray(&[]);
        writer.write_raw(&0_i64.to_be_bytes());
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_bytearray(&[]);
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_bytearray(proxy_blob);
        writer.finish()
    }
}
