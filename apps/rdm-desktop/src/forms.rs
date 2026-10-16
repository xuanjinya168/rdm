//! 新建下载和设置表单的校验。提示文案与原先的桌面界面一致。

use rdm_domain::config::AppSettings;
use rdm_domain::validation::{is_http_url, is_valid_windows_filename, normalize_sha256};

pub struct DownloadForm {
    pub url: String,
    pub destination: String,
    pub filename: String,
    pub connections: u32,
    pub sha256: String,
}

#[derive(Debug)]
pub struct DownloadValues {
    pub url: String,
    pub destination: String,
    pub filename: String,
    pub connections: u32,
    pub sha256: String,
}

pub fn validate_download(form: &DownloadForm) -> Result<DownloadValues, String> {
    let url = form.url.trim();
    let destination = form.destination.trim();
    let filename = form.filename.trim();
    if !is_http_url(url) {
        return Err("请输入有效的 HTTP 或 HTTPS 地址。".to_string());
    }
    if destination.is_empty() {
        return Err("请选择保存目录。".to_string());
    }
    if !filename.is_empty() && !is_valid_windows_filename(filename) {
        return Err("文件名不符合 Windows 命名规则。".to_string());
    }
    let sha256 = normalize_sha256(&form.sha256).map_err(|error| error.to_string())?;
    Ok(DownloadValues {
        url: url.to_string(),
        destination: destination.to_string(),
        filename: filename.to_string(),
        connections: form.connections,
        sha256: sha256.unwrap_or_default(),
    })
}

pub struct SettingsForm {
    pub download_dir: String,
    pub max_active: i64,
    pub connections: i64,
    pub retry: i64,
    pub speed_kb: i64,
    pub clipboard: bool,
    pub tray: bool,
    pub hls_transcode: bool,
    pub theme: String,
    pub proxy_enabled: bool,
    pub proxy_url: String,
    pub proxy_username: String,
    pub proxy_password: String,
}

pub fn validate_settings(form: &SettingsForm) -> Result<AppSettings, String> {
    let download_dir = form.download_dir.trim();
    let proxy_url = form.proxy_url.trim();
    if download_dir.is_empty() {
        return Err("请选择默认下载目录。".to_string());
    }
    if form.proxy_enabled && proxy_url.is_empty() {
        return Err("已启用代理，请填写代理地址。".to_string());
    }
    if !proxy_url.is_empty() && !valid_proxy_url(proxy_url) {
        return Err("代理地址需以 http://、https:// 或 socks5:// 开头。".to_string());
    }
    Ok(AppSettings {
        download_dir: download_dir.to_string(),
        max_active_downloads: form.max_active,
        default_connections: form.connections,
        speed_limit_bytes: form.speed_kb.saturating_mul(1024),
        retry_count: form.retry,
        clipboard_monitoring: form.clipboard,
        minimize_to_tray: form.tray,
        hls_transcode: form.hls_transcode,
        theme: form.theme.clone(),
        proxy_enabled: form.proxy_enabled,
        proxy_url: proxy_url.to_string(),
        proxy_username: form.proxy_username.trim().to_string(),
        proxy_password: form.proxy_password.clone(),
    })
}

fn valid_proxy_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("socks5://")
        || lower.starts_with("socks5h://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download() -> DownloadForm {
        DownloadForm {
            url: " https://example.com/archive.zip ".into(),
            destination: " C:\\Downloads ".into(),
            filename: " archive.zip ".into(),
            connections: 8,
            sha256: String::new(),
        }
    }

    fn settings() -> SettingsForm {
        SettingsForm {
            download_dir: " C:\\Downloads ".into(),
            max_active: 3,
            connections: 8,
            retry: 4,
            speed_kb: 1024,
            clipboard: true,
            tray: false,
            hls_transcode: false,
            theme: "dark".into(),
            proxy_enabled: false,
            proxy_url: String::new(),
            proxy_username: String::new(),
            proxy_password: String::new(),
        }
    }

    #[test]
    fn download_form_normalizes_a_valid_request() {
        let value = validate_download(&download()).unwrap();
        assert_eq!(value.url, "https://example.com/archive.zip");
        assert_eq!(value.destination, "C:\\Downloads");
        assert_eq!(value.filename, "archive.zip");
        assert_eq!(value.connections, 8);
        assert_eq!(value.sha256, "");
    }

    #[test]
    fn download_form_rejects_bad_url_and_reserved_names() {
        let mut form = download();
        form.url = "ftp://example.com/a".into();
        assert!(validate_download(&form).unwrap_err().contains("HTTP"));
        form = download();
        form.filename = "NUL.txt".into();
        assert!(validate_download(&form).unwrap_err().contains("Windows"));
    }

    #[test]
    fn download_form_checks_sha256() {
        let checksum = "AB".repeat(32);
        let mut form = download();
        form.sha256 = checksum.clone();
        assert_eq!(
            validate_download(&form).unwrap().sha256,
            checksum.to_lowercase()
        );
        form.sha256 = "abc".into();
        assert!(validate_download(&form).unwrap_err().contains("64"));
    }

    #[test]
    fn settings_form_maps_units() {
        let value = validate_settings(&settings()).unwrap();
        assert_eq!(value.download_dir, "C:\\Downloads");
        assert_eq!(value.max_active_downloads, 3);
        assert_eq!(value.default_connections, 8);
        assert_eq!(value.retry_count, 4);
        assert_eq!(value.speed_limit_bytes, 1024 * 1024);
        assert!(value.clipboard_monitoring);
        assert!(!value.minimize_to_tray);
        assert_eq!(value.theme, "dark");
    }

    #[test]
    fn settings_form_requires_proxy_url_and_scheme() {
        let mut form = settings();
        form.proxy_enabled = true;
        assert!(validate_settings(&form).unwrap_err().contains("代理地址"));
        form.proxy_url = "socks5://127.0.0.1:1080".into();
        assert!(validate_settings(&form).is_ok());
        form.proxy_url = "ftp://127.0.0.1:21".into();
        assert!(validate_settings(&form).unwrap_err().contains("socks5"));
    }
}
