//! Small URL helpers shared by resolvers.

/// Extension of the path component of `url`, lower-cased, falling back to
/// `default` when the path has no usable extension.
pub fn url_ext(url: &str, default: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .and_then(|seg| seg.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_or_defaults_extension() {
        assert_eq!(url_ext("https://h/media/AbC.png?x=1", "jpg"), "png");
        assert_eq!(url_ext("https://h/media/AbC", "jpg"), "jpg");
        assert_eq!(url_ext("https://h/v.MP4", "bin"), "mp4");
    }
}
