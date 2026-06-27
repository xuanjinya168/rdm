//! 应用程序设置及其磁盘存储。保留「拒绝非法值并回退为默认值」的语义。

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const APP_DIR_NAME: &str = "RDM";
pub const MAX_ACTIVE_DOWNLOADS: i64 = 20;
pub const MAX_CONNECTIONS: i64 = 32;
pub const MAX_RETRY_COUNT: i64 = 20;
const MAX_SPEED_LIMIT: i64 = 1024i64.pow(4);

fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 当前用户的应用数据目录（`%LOCALAPPDATA%\RDM`，若不可用则为 `~/.rdm`）。
pub fn app_data_dir() -> PathBuf {
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        if !base.is_empty() {
            return PathBuf::from(base).join(APP_DIR_NAME);
        }
    }
    home_dir().join(".rdm")
}

/// 用户可配置的应用程序设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    pub download_dir: String,
    pub max_active_downloads: i64,
    pub default_connections: i64,
    pub speed_limit_bytes: i64,
    pub retry_count: i64,
    pub clipboard_monitoring: bool,
    pub minimize_to_tray: bool,
    /// 为 true 时，HLS 下载完成后用硬件编码器（GPU）重新编码为 mp4；
    /// 默认 false：仅无损封装（remux）。检测不到可用 GPU 时自动回退为封装。
    pub hls_transcode: bool,
    pub theme: String,
    /// 为 true 时，出站下载 / 解析请求会通过 `proxy_url` 转发。
    pub proxy_enabled: bool,
    /// 代理地址，例如 `http://127.0.0.1:7890` 或 `socks5://127.0.0.1:1080`。
    pub proxy_url: String,
    /// 代理认证的可选用户名。
    pub proxy_username: String,
    /// 代理认证的可选密码。
    pub proxy_password: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            download_dir: home_dir().join("Downloads").to_string_lossy().into_owned(),
            max_active_downloads: 3,
            default_connections: 8,
            speed_limit_bytes: 0,
            retry_count: 4,
            clipboard_monitoring: true,
            minimize_to_tray: true,
            hls_transcode: false,
            theme: "dark".to_string(),
            proxy_enabled: false,
            proxy_url: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
        }
    }
}

impl AppSettings {
    /// 返回应用可接受的规范化设置（非法值会被替换为默认值）。
    pub fn validated(&self) -> Self {
        let defaults = Self::default();
        Self {
            download_dir: if self.download_dir.trim().is_empty() {
                defaults.download_dir
            } else {
                self.download_dir.clone()
            },
            max_active_downloads: if (1..=MAX_ACTIVE_DOWNLOADS).contains(&self.max_active_downloads)
            {
                self.max_active_downloads
            } else {
                defaults.max_active_downloads
            },
            default_connections: if (1..=MAX_CONNECTIONS).contains(&self.default_connections) {
                self.default_connections
            } else {
                defaults.default_connections
            },
            speed_limit_bytes: if (0..=MAX_SPEED_LIMIT).contains(&self.speed_limit_bytes) {
                self.speed_limit_bytes
            } else {
                defaults.speed_limit_bytes
            },
            retry_count: if (0..=MAX_RETRY_COUNT).contains(&self.retry_count) {
                self.retry_count
            } else {
                defaults.retry_count
            },
            clipboard_monitoring: self.clipboard_monitoring,
            minimize_to_tray: self.minimize_to_tray,
            hls_transcode: self.hls_transcode,
            theme: if matches!(self.theme.as_str(), "light" | "dark") {
                self.theme.clone()
            } else {
                defaults.theme
            },
            // 代理 URL/凭据在此处去除首尾空格；协议合法性留给
            // reqwest 在构建客户端时检查，非法值会回退为直连，
            // 而非中断下载。
            proxy_enabled: self.proxy_enabled,
            proxy_url: self.proxy_url.trim().to_string(),
            proxy_username: self.proxy_username.trim().to_string(),
            proxy_password: self.proxy_password.clone(),
        }
    }

    /// 从不可信的 JSON 中解析设置：缺失或越界的字段会替换为默认值，
    /// 不会因此返回错误。
    pub fn from_value(raw: &Value) -> Self {
        let defaults = Self::default();
        let Some(obj) = raw.as_object() else {
            return defaults;
        };

        // 仅接受范围内的 JSON 整数——浮点数和布尔值会被拒绝。
        let bounded = |name: &str, default: i64, min: i64, max: i64| -> i64 {
            match obj.get(name) {
                Some(Value::Number(n)) if n.is_i64() || n.is_u64() => match n.as_i64() {
                    Some(v) if (min..=max).contains(&v) => v,
                    _ => default,
                },
                _ => default,
            }
        };
        let boolean = |name: &str, default: bool| -> bool {
            match obj.get(name) {
                Some(Value::Bool(b)) => *b,
                _ => default,
            }
        };

        let download_dir = obj
            .get("download_dir")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or(defaults.download_dir);
        let theme = obj
            .get("theme")
            .and_then(Value::as_str)
            .filter(|theme| matches!(*theme, "light" | "dark"))
            .map(str::to_string)
            .unwrap_or(defaults.theme);
        let proxy_url = obj
            .get("proxy_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_string)
            .unwrap_or(defaults.proxy_url);
        let proxy_username = obj
            .get("proxy_username")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_string)
            .unwrap_or(defaults.proxy_username);
        let proxy_password = obj
            .get("proxy_password")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(defaults.proxy_password);

        Self {
            download_dir,
            max_active_downloads: bounded("max_active_downloads", 3, 1, MAX_ACTIVE_DOWNLOADS),
            default_connections: bounded("default_connections", 8, 1, MAX_CONNECTIONS),
            speed_limit_bytes: bounded("speed_limit_bytes", 0, 0, MAX_SPEED_LIMIT),
            retry_count: bounded("retry_count", 4, 0, MAX_RETRY_COUNT),
            clipboard_monitoring: boolean("clipboard_monitoring", true),
            minimize_to_tray: boolean("minimize_to_tray", true),
            hls_transcode: boolean("hls_transcode", false),
            theme,
            proxy_enabled: boolean("proxy_enabled", false),
            proxy_url,
            proxy_username,
            proxy_password,
        }
    }
}

/// 加载并以原子方式将 [`AppSettings`] 持久化为 JSON。
pub struct SettingsStore {
    pub path: PathBuf,
}

impl SettingsStore {
    /// 存储在 `path`，若未提供则使用 [`app_data_dir`] 下的默认 `settings.json`。
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: path.unwrap_or_else(|| app_data_dir().join("settings.json")),
        }
    }

    /// 读取设置；任何读取或解析错误都会回退为默认值。
    pub fn load(&self) -> AppSettings {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return AppSettings::default();
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => AppSettings::from_value(&value),
            Err(_) => AppSettings::default(),
        }
    }

    /// 重新校验设置后，通过临时文件 + 原子重命名写入。
    pub fn save(&self, settings: &AppSettings) -> std::io::Result<AppSettings> {
        let validated = settings.validated();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        let json = serde_json::to_string_pretty(&validated).expect("settings serialize");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Err(error) = replace_file(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(validated)
    }
}

#[cfg(not(windows))]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_values_and_keeps_good_ones() {
        let raw = serde_json::json!({
            "download_dir": "D:/dl",
            "max_active_downloads": 5,
            "default_connections": 999,      // 越界 -> 默认 8
            "speed_limit_bytes": 1.5,        // float -> default 0
            "retry_count": true,             // bool -> default 4
            "clipboard_monitoring": false,
            "minimize_to_tray": "yes",       // wrong type -> default true
            "theme": "light",
            "proxy_enabled": true,
            "proxy_url": "  socks5://127.0.0.1:1080  ",
            "proxy_username": "  user  ",
            "proxy_password": "pw"
        });
        let settings = AppSettings::from_value(&raw);
        assert_eq!(settings.download_dir, "D:/dl");
        assert_eq!(settings.max_active_downloads, 5);
        assert_eq!(settings.default_connections, 8);
        assert_eq!(settings.speed_limit_bytes, 0);
        assert_eq!(settings.retry_count, 4);
        assert!(!settings.clipboard_monitoring);
        assert!(settings.minimize_to_tray);
        assert_eq!(settings.theme, "light");
        assert!(settings.proxy_enabled);
        assert_eq!(settings.proxy_url, "socks5://127.0.0.1:1080");
        assert_eq!(settings.proxy_username, "user");
        assert_eq!(settings.proxy_password, "pw");
    }

    #[test]
    fn non_object_json_yields_defaults() {
        assert_eq!(
            AppSettings::from_value(&serde_json::json!([1, 2, 3])),
            AppSettings::default()
        );
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(Some(dir.path().join("absent.json")));
        assert_eq!(store.load(), AppSettings::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(Some(dir.path().join("settings.json")));
        let settings = AppSettings {
            download_dir: "E:/grabs".to_string(),
            max_active_downloads: 7,
            ..AppSettings::default()
        };
        assert_eq!(store.save(&settings).unwrap(), settings);
        assert_eq!(store.load(), settings);
    }

    #[test]
    fn save_returns_and_persists_validated_settings_repeatedly() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(Some(dir.path().join("settings.json")));
        let invalid = AppSettings {
            download_dir: " ".to_string(),
            max_active_downloads: 999,
            default_connections: -1,
            speed_limit_bytes: -1,
            retry_count: 999,
            clipboard_monitoring: false,
            minimize_to_tray: false,
            hls_transcode: false,
            theme: "unsupported".to_string(),
            proxy_enabled: false,
            proxy_url: "  ".to_string(),
            proxy_username: "  ".to_string(),
            proxy_password: String::new(),
        };

        let first = store.save(&invalid).unwrap();
        assert_eq!(
            first,
            AppSettings {
                clipboard_monitoring: false,
                minimize_to_tray: false,
                ..AppSettings::default()
            }
        );
        assert_eq!(store.load(), first);
        assert_eq!(first.proxy_url, ""); // 首尾空白已去除

        let replacement = AppSettings {
            download_dir: "D:/downloads".to_string(),
            max_active_downloads: 5,
            ..AppSettings::default()
        };
        assert_eq!(store.save(&replacement).unwrap(), replacement);
        assert_eq!(store.load(), replacement);
    }

    #[test]
    fn load_corrupt_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        let store = SettingsStore::new(Some(path));
        assert_eq!(store.load(), AppSettings::default());
    }

    #[test]
    fn proxy_fields_default_and_validate() {
        // 缺少代理字段时回退为默认值（代理禁用）。
        let settings = AppSettings::from_value(&serde_json::json!({}));
        assert!(!settings.proxy_enabled);
        assert_eq!(settings.proxy_url, "");
        assert_eq!(settings.proxy_username, "");
        assert_eq!(settings.proxy_password, "");

        // proxy_enabled 类型错误时 -> 默认为 false。
        let settings = AppSettings::from_value(&serde_json::json!({
            "proxy_enabled": "yes"
        }));
        assert!(!settings.proxy_enabled);

        // validated() 会去除 url/username 的首尾空格，但密码保持原样。
        let raw = AppSettings {
            proxy_enabled: true,
            proxy_url: "  http://127.0.0.1:7890  ".to_string(),
            proxy_username: "  user  ".to_string(),
            proxy_password: "  secret  ".to_string(),
            ..AppSettings::default()
        };
        let v = raw.validated();
        assert_eq!(v.proxy_url, "http://127.0.0.1:7890");
        assert_eq!(v.proxy_username, "user");
        assert_eq!(v.proxy_password, "  secret  ");
    }
}
