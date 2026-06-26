//! URL、校验和、文件名校验。保留 Windows 平台规则。

use url::Url;

use crate::error::CoreError;

/// Windows 文件名中禁止使用的字符，以及 C0 控制字符范围。
fn is_invalid_filename_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) <= 0x1f
}

/// Windows 保留的设备名（不论扩展名）。
fn is_windows_reserved(stem_upper: &str) -> bool {
    matches!(stem_upper, "CON" | "PRN" | "AUX" | "NUL")
        || matches!(stem_upper.strip_prefix("COM").or_else(|| stem_upper.strip_prefix("LPT")),
            Some(n) if n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
}

/// 当 `value` 可解析为带主机的绝对 http(s) URL 时返回 true。
pub fn is_http_url(value: &str) -> bool {
    match Url::parse(value.trim()) {
        Ok(parsed) => {
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str().is_some_and(|h| !h.is_empty())
        }
        Err(_) => false,
    }
}

/// 规范化用户提供的 SHA-256 校验和：空字符串返回 `None`，
/// 否则返回 64 位小写十六进制字符串；非法值返回 [`CoreError::InvalidSha256`]。
pub fn normalize_sha256(value: &str) -> Result<Option<String>, CoreError> {
    let checksum = value.trim().to_lowercase();
    if checksum.is_empty() {
        return Ok(None);
    }
    if checksum.len() != 64 || !checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CoreError::InvalidSha256);
    }
    Ok(Some(checksum))
}

/// 将 `value` 处理为可在 Windows 下安全使用的文件名（永远不会为空）。
pub fn sanitize_filename(value: &str) -> String {
    let replaced: String = value
        .chars()
        .map(|c| if is_invalid_filename_char(c) { '_' } else { c })
        .collect();
    let trimmed = replaced.trim().trim_end_matches(['.', ' ']);
    let mut cleaned = if trimmed.is_empty() {
        "download".to_string()
    } else {
        trimmed.to_string()
    };
    let stem = cleaned.split('.').next().unwrap_or_default().to_uppercase();
    if is_windows_reserved(&stem) {
        cleaned = format!("_{cleaned}");
    }
    cleaned.chars().take(240).collect()
}

/// 当 `value` 本身已是合法的 Windows 文件名时返回 true。
pub fn is_valid_windows_filename(value: &str) -> bool {
    if value.is_empty() || value != value.trim_end_matches(['.', ' ']) {
        return false;
    }
    if value.chars().any(is_invalid_filename_char) {
        return false;
    }
    let stem = value.split('.').next().unwrap_or_default().to_uppercase();
    !is_windows_reserved(&stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https_urls() {
        assert!(is_http_url("https://example.com/file.zip"));
        assert!(is_http_url("  http://host:8080/a "));
        assert!(!is_http_url("ftp://example.com/file"));
        assert!(!is_http_url("not a url"));
        assert!(!is_http_url("https://"));
    }

    #[test]
    fn normalizes_checksums() {
        assert_eq!(normalize_sha256("   ").unwrap(), None);
        let hex = "A".repeat(64);
        assert_eq!(normalize_sha256(&hex).unwrap(), Some("a".repeat(64)));
        assert_eq!(
            normalize_sha256("abc").unwrap_err(),
            CoreError::InvalidSha256
        );
        let non_hex = "g".repeat(64);
        assert_eq!(
            normalize_sha256(&non_hex).unwrap_err(),
            CoreError::InvalidSha256
        );
    }

    #[test]
    fn sanitizes_filenames() {
        assert_eq!(sanitize_filename("a/b:c*.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_filename("   "), "download");
        assert_eq!(sanitize_filename("name...  "), "name");
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("com1.txt"), "_com1.txt");
        assert_eq!(sanitize_filename("report.txt"), "report.txt");
        assert_eq!(sanitize_filename(&"x".repeat(300)).chars().count(), 240);
    }

    #[test]
    fn validates_windows_filenames() {
        assert!(is_valid_windows_filename("report.txt"));
        assert!(!is_valid_windows_filename(""));
        assert!(!is_valid_windows_filename("trailing. "));
        assert!(!is_valid_windows_filename("bad/name"));
        assert!(!is_valid_windows_filename("nul.txt"));
        assert!(is_valid_windows_filename("com10.txt"));
    }
}
