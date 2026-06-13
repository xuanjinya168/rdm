//! URL, checksum and filename validation. Port of the Python `validation`
//! module, keeping the same Windows-oriented rules.

use url::Url;

use crate::error::CoreError;

/// Characters Windows forbids in a file name, plus the C0 control range.
fn is_invalid_filename_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) <= 0x1f
}

/// Device names Windows reserves regardless of extension.
fn is_windows_reserved(stem_upper: &str) -> bool {
    matches!(stem_upper, "CON" | "PRN" | "AUX" | "NUL")
        || matches!(stem_upper.strip_prefix("COM").or_else(|| stem_upper.strip_prefix("LPT")),
            Some(n) if n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
}

/// True when `value` parses as an absolute http(s) URL with a host.
pub fn is_http_url(value: &str) -> bool {
    match Url::parse(value.trim()) {
        Ok(parsed) => {
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str().is_some_and(|h| !h.is_empty())
        }
        Err(_) => false,
    }
}

/// Normalize a user-supplied SHA-256: empty -> `None`, otherwise a lowercase
/// 64-char hex string, or [`CoreError::InvalidSha256`].
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

/// Make `value` safe to use as a Windows file name, never returning empty.
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

/// True when `value` is already a valid Windows file name as-is.
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
