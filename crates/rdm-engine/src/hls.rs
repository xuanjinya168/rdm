//! HLS 清单解析、AES-128 密钥获取与分片解密工具。

use std::collections::HashMap;

use aes::Aes128;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use thiserror::Error;
use url::Url;

type Aes128CbcDecryptor = cbc::Decryptor<Aes128>;

#[derive(Debug, Error)]
pub enum HlsError {
    #[error("Invalid HLS playlist: {0}")]
    InvalidPlaylist(String),

    #[error("Unsupported HLS stream: {0}")]
    Unsupported(String),

    #[error("AES-128 key must be 16 bytes, got {0}")]
    InvalidKeyLength(usize),

    #[error("AES-128-CBC decrypt failed")]
    DecryptFailed,

    #[error(transparent)]
    Url(#[from] url::ParseError),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Playlist {
    Master(MasterPlaylist),
    Media(MediaPlaylist),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MasterPlaylist {
    pub variants: Vec<MasterVariant>,
}

impl MasterPlaylist {
    pub fn best_variant(&self) -> Option<&MasterVariant> {
        self.variants.iter().max_by_key(|variant| {
            let area = variant
                .resolution
                .map(|(width, height)| width as u64 * height as u64)
                .unwrap_or(0);
            (variant.bandwidth.unwrap_or(0), area)
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MasterVariant {
    pub uri: String,
    pub bandwidth: Option<u64>,
    pub resolution: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaPlaylist {
    pub segments: Vec<MediaSegment>,
    pub end_list: bool,
    pub media_sequence: u64,
    /// fMP4/CMAF 初始化分片（#EXT-X-MAP）。下载时需前置拼接，否则拼接出的
    /// .m4s 分片无法播放。
    pub init_segment: Option<InitSegment>,
}

/// #EXT-X-MAP 描述的初始化分片（fMP4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitSegment {
    pub uri: String,
    /// #EXT-X-MAP 的 BYTERANGE：init 段在某资源文件中的字节区间。
    pub byte_range: Option<ByteRange>,
    /// HEAD 探测后的字节数（仅在 probe_segment_sizes 调用后填充）。
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaSegment {
    pub index: usize,
    pub sequence: u64,
    pub duration: f64,
    pub uri: String,
    pub encryption: Option<Encryption>,
    /// #EXT-X-BYTERANGE：(offset, length)。length 为 None 表示读到资源末尾。
    pub byte_range: Option<ByteRange>,
    /// HEAD 探测后的实际字节数（仅在 probe_segment_sizes 调用后填充）。
    /// 用于精确进度估算。
    pub size: Option<u64>,
}

/// #EXT-X-BYTERANGE：在某个资源文件中的字节区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// 区间起始偏移（绝对）。
    pub offset: u64,
    /// 区间长度；None 表示读到资源末尾。
    pub length: Option<u64>,
}

impl MediaSegment {
    pub fn iv(&self) -> Option<[u8; 16]> {
        self.encryption
            .as_ref()
            .map(|encryption| encryption.iv.unwrap_or_else(|| sequence_iv(self.sequence)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encryption {
    pub uri: String,
    pub iv: Option<[u8; 16]>,
}

pub fn is_hls_url(url: &str) -> bool {
    let path = Url::parse(url)
        .map(|parsed| parsed.path().to_string())
        .unwrap_or_else(|_| url.split(['?', '#']).next().unwrap_or_default().to_string());
    path.to_ascii_lowercase().ends_with(".m3u8")
}

pub fn parse_playlist(content: &str, base_url: &str) -> Result<Playlist, HlsError> {
    let base = Url::parse(base_url)?;
    if !content.lines().any(|line| line.trim() == "#EXTM3U") {
        return Err(HlsError::InvalidPlaylist("missing #EXTM3U header".into()));
    }
    for line in content.lines().map(str::trim) {
        if let Some(attrs) = line
            .strip_prefix("#EXT-X-SESSION-KEY:")
            .or_else(|| line.strip_prefix("#EXT-X-KEY:"))
        {
            validate_key_tag(attrs)?;
        }
    }
    if content
        .lines()
        .any(|line| line.trim().starts_with("#EXT-X-STREAM-INF:"))
    {
        return parse_master(content, &base).map(Playlist::Master);
    }
    parse_media(content, &base).map(Playlist::Media)
}

pub fn decrypt_aes128_cbc(
    ciphertext: &[u8],
    key: &[u8; 16],
    iv: &[u8; 16],
) -> Result<Vec<u8>, HlsError> {
    Aes128CbcDecryptor::new(key.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .map_err(|_| HlsError::DecryptFailed)
}

impl MediaPlaylist {
    /// 当初始化分片与所有媒体分片的大小都已探测得到时，返回拼接后的总字节数；
    /// 任一未知则返回 None（调用方据此决定是否启用精确进度）。
    pub fn total_size(&self) -> Option<u64> {
        let mut total = self.init_segment.as_ref()?.size?;
        for segment in &self.segments {
            total = total.checked_add(segment.size?)?;
        }
        Some(total)
    }
}

/// 对初始化分片与所有媒体分片发起并发 HEAD 请求，填充各自的 `size`。
///
/// 采用 byterange 的分片，其大小由区间长度决定（无需 HEAD）；
/// 非 byterange 的分片，HEAD 失败或无 Content-Length 时 `size` 保持 None。
/// `concurrency` 限制并发请求数（受下载连接数约束）。
pub async fn probe_segment_sizes(
    client: &reqwest::Client,
    request_headers: &HeaderMap,
    media: &mut MediaPlaylist,
    concurrency: usize,
) {
    // 收集需要 HEAD 的目标：初始化分片 + 非区间限定的媒体分片。
    // 区间限定的分片大小已知（= length），直接填入。
    enum Target {
        Init,
        Segment(usize),
    }

    let mut targets = Vec::new();
    if let Some(init) = media.init_segment.as_mut() {
        if init.size.is_none() {
            // 带 BYTERANGE 的 init 段大小即区间长度，无需 HEAD。
            if let Some(range) = init.byte_range {
                init.size = range.length;
            } else {
                targets.push((Target::Init, init.uri.clone()));
            }
        }
    }
    for (i, segment) in media.segments.iter_mut().enumerate() {
        if segment.size.is_none() {
            if let Some(range) = segment.byte_range {
                segment.size = range.length;
            } else {
                targets.push((Target::Segment(i), segment.uri.clone()));
            }
        }
    }

    let headers = request_headers.clone();
    let results: Vec<(Target, Option<u64>)> = futures_util::stream::iter(targets)
        .map(|(target, url)| {
            let headers = headers.clone();
            let client = client.clone();
            async move {
                let size = head_content_length(&client, &headers, &url).await;
                (target, size)
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    for (target, size) in results {
        match target {
            Target::Init => {
                if let Some(init) = media.init_segment.as_mut() {
                    init.size = size;
                }
            }
            Target::Segment(i) => {
                if i < media.segments.len() {
                    media.segments[i].size = size;
                }
            }
        }
    }
}

/// 发送 HEAD 请求，返回 Content-Length（解析失败或请求出错时为 None）。
async fn head_content_length(
    client: &reqwest::Client,
    headers: &HeaderMap,
    url: &str,
) -> Option<u64> {
    // 给单个探测请求一个上限，避免某些服务器对 HEAD 不响应时整体卡死。
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.head(url).headers(headers.clone()).send(),
    )
    .await
    .ok()?
    .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn parse_master(content: &str, base: &Url) -> Result<MasterPlaylist, HlsError> {
    let mut variants = Vec::new();
    let mut pending_attrs: Option<HashMap<String, String>> = None;

    for line in content.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_attrs = Some(parse_attributes(attrs));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some(attrs) = pending_attrs.take() else {
            continue;
        };
        variants.push(MasterVariant {
            uri: absolutize(base, line)?,
            bandwidth: attrs.get("BANDWIDTH").and_then(|value| value.parse().ok()),
            resolution: attrs.get("RESOLUTION").and_then(|value| {
                let (width, height) = value.split_once('x')?;
                Some((width.parse().ok()?, height.parse().ok()?))
            }),
        });
    }

    if variants.is_empty() {
        return Err(HlsError::InvalidPlaylist(
            "master playlist has no variants".into(),
        ));
    }
    Ok(MasterPlaylist { variants })
}

fn parse_media(content: &str, base: &Url) -> Result<MediaPlaylist, HlsError> {
    let mut segments = Vec::new();
    let mut end_list = false;
    let mut media_sequence = 0u64;
    let mut current_key: Option<Encryption> = None;
    let mut pending_duration: Option<f64> = None;
    let mut pending_byte_range: Option<ByteRange> = None;
    // BYTERANGE 的隐式偏移：省略 @offset 时，沿用上一个区间末尾。
    let mut previous_range_end: Option<u64> = None;
    let mut init_segment: Option<InitSegment> = None;

    for line in content.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line == "#EXT-X-ENDLIST" {
            end_list = true;
            continue;
        }
        if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            media_sequence = value
                .parse()
                .map_err(|_| HlsError::InvalidPlaylist("invalid media sequence".into()))?;
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            init_segment = Some(parse_map(attrs, base)?);
            continue;
        }
        if let Some(spec) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending_byte_range = Some(parse_byterange(spec.trim(), previous_range_end)?);
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-KEY:") {
            current_key = parse_key(attrs, base)?;
            continue;
        }
        if let Some(value) = line.strip_prefix("#EXTINF:") {
            let duration = value.split(',').next().unwrap_or_default().trim();
            pending_duration = Some(
                duration
                    .parse()
                    .map_err(|_| HlsError::InvalidPlaylist("invalid segment duration".into()))?,
            );
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        let Some(duration) = pending_duration.take() else {
            return Err(HlsError::InvalidPlaylist(
                "segment URI appeared without preceding #EXTINF".into(),
            ));
        };
        let byte_range = pending_byte_range.take();
        // 记录该区间末尾，供后续省略 @offset 的 BYTERANGE 续算。
        if let Some(range) = byte_range {
            previous_range_end = Some(range.end_offset());
        }
        let index = segments.len();
        segments.push(MediaSegment {
            index,
            sequence: media_sequence + index as u64,
            duration,
            uri: absolutize(base, line)?,
            encryption: current_key.clone(),
            byte_range,
            size: None,
        });
    }

    if !end_list {
        return Err(HlsError::Unsupported(
            "live HLS playlists without #EXT-X-ENDLIST are not supported in P0".into(),
        ));
    }
    if segments.is_empty() {
        return Err(HlsError::InvalidPlaylist(
            "media playlist has no segments".into(),
        ));
    }
    Ok(MediaPlaylist {
        segments,
        end_list,
        media_sequence,
        init_segment,
    })
}

impl ByteRange {
    /// 该区间的绝对结束偏移（不含），即下一个区间的起点。
    pub fn end_offset(&self) -> u64 {
        self.offset + self.length.unwrap_or(0)
    }

    /// HTTP `Range` 头的值：`bytes=<offset>-<end>`（两端均含）。
    /// length 为 None 时表示读到资源末尾：`bytes=<offset>-`。
    pub fn http_range_value(&self) -> String {
        match self.length {
            Some(length) if length > 0 => {
                format!("bytes={}-{}", self.offset, self.offset + length - 1)
            }
            _ => format!("bytes={}-", self.offset),
        }
    }

    /// 当服务器忽略 Range（以 200 返回全量）时，按区间裁剪出本分片应有的字节。
    pub fn slice<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        let start = (self.offset as usize).min(bytes.len());
        let end = match self.length {
            Some(length) => start.saturating_add(length as usize).min(bytes.len()),
            None => bytes.len(),
        };
        &bytes[start..end]
    }
}

/// 解析 `#EXT-X-BYTERANGE` 的值：`length` 或 `length@offset`。
/// `implicit_offset` 为省略 @offset 时的回退（上一个区间的末尾）。
fn parse_byterange(spec: &str, implicit_offset: Option<u64>) -> Result<ByteRange, HlsError> {
    let (length_part, offset_part) = match spec.split_once('@') {
        Some((length, offset)) => (length, Some(offset)),
        None => (spec, None),
    };
    let length: u64 = length_part.trim().parse().map_err(|_| {
        HlsError::InvalidPlaylist(format!("invalid #EXT-X-BYTERANGE length: {spec}"))
    })?;
    let offset = match offset_part {
        Some(offset) => offset.trim().parse().map_err(|_| {
            HlsError::InvalidPlaylist(format!("invalid #EXT-X-BYTERANGE offset: {spec}"))
        })?,
        None => implicit_offset.ok_or_else(|| {
            HlsError::InvalidPlaylist(
                "#EXT-X-BYTERANGE without @offset must follow another byte-range segment".into(),
            )
        })?,
    };
    Ok(ByteRange {
        offset,
        length: Some(length),
    })
}

/// 解析 `#EXT-X-MAP`：提取 URI（必需）与可选的 BYTERANGE。
fn parse_map(attrs: &str, base: &Url) -> Result<InitSegment, HlsError> {
    let attrs = parse_attributes(attrs);
    let uri = attrs
        .get("URI")
        .ok_or_else(|| HlsError::InvalidPlaylist("#EXT-X-MAP is missing URI".into()))?;
    // #EXT-X-MAP 的 BYTERANGE 省略 @offset 时从资源开头算起（offset=0）。
    let byte_range = attrs
        .get("BYTERANGE")
        .map(|spec| parse_byterange(spec.trim(), Some(0)))
        .transpose()?;
    Ok(InitSegment {
        uri: absolutize(base, uri)?,
        byte_range,
        size: None,
    })
}

fn parse_key(attrs: &str, base: &Url) -> Result<Option<Encryption>, HlsError> {
    let attrs = parse_attributes(attrs);
    let method = attrs
        .get("METHOD")
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_default();
    if method == "NONE" {
        return Ok(None);
    }
    if method != "AES-128" {
        return Err(HlsError::Unsupported(format!(
            "{method} encryption is not supported in P0"
        )));
    }
    if attrs
        .get("KEYFORMAT")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(HlsError::Unsupported(
            "DRM HLS key formats are not supported in P0".into(),
        ));
    }
    let uri = attrs
        .get("URI")
        .ok_or_else(|| HlsError::InvalidPlaylist("AES-128 key is missing URI".into()))?;
    let iv = attrs.get("IV").map(|value| parse_iv(value)).transpose()?;
    Ok(Some(Encryption {
        uri: absolutize(base, uri)?,
        iv,
    }))
}

fn validate_key_tag(attrs: &str) -> Result<(), HlsError> {
    let attrs = parse_attributes(attrs);
    if attrs
        .get("KEYFORMAT")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(HlsError::Unsupported(
            "DRM HLS key formats are not supported in P0".into(),
        ));
    }
    let method = attrs
        .get("METHOD")
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_default();
    if !method.is_empty() && method != "NONE" && method != "AES-128" {
        return Err(HlsError::Unsupported(format!(
            "{method} encryption is not supported in P0"
        )));
    }
    Ok(())
}

fn parse_attributes(input: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let mut rest = input.trim();
    while !rest.is_empty() {
        let Some((raw_key, after_key)) = rest.split_once('=') else {
            break;
        };
        let key = raw_key.trim().to_ascii_uppercase();
        let after_key = after_key.trim_start();
        let (value, remaining) = if let Some(stripped) = after_key.strip_prefix('"') {
            match stripped.split_once('"') {
                Some((quoted, remaining)) => (quoted.to_string(), remaining),
                None => (stripped.to_string(), ""),
            }
        } else {
            let (value, remaining) = after_key.split_once(',').unwrap_or((after_key, ""));
            (value.trim().to_string(), remaining)
        };
        if !key.is_empty() {
            attrs.insert(key, value);
        }
        rest = remaining
            .strip_prefix(',')
            .unwrap_or(remaining)
            .trim_start();
    }
    attrs
}

fn parse_iv(value: &str) -> Result<[u8; 16], HlsError> {
    let value = value.trim();
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HlsError::InvalidPlaylist(
            "AES-128 IV must be a 128-bit hexadecimal value".into(),
        ));
    }
    let mut iv = [0u8; 16];
    for (i, byte) in iv.iter_mut().enumerate() {
        let start = i * 2;
        *byte = u8::from_str_radix(&hex[start..start + 2], 16)
            .map_err(|_| HlsError::InvalidPlaylist("AES-128 IV contains invalid hex".into()))?;
    }
    Ok(iv)
}

fn sequence_iv(sequence: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&sequence.to_be_bytes());
    iv
}

fn absolutize(base: &Url, value: &str) -> Result<String, HlsError> {
    Ok(base.join(value.trim())?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::{BlockEncryptMut, KeyIvInit};

    type Aes128CbcEncryptor = cbc::Encryptor<Aes128>;

    #[test]
    fn parses_media_playlist_and_resolves_relative_urls() {
        let playlist = parse_playlist(
            "#EXTM3U
#EXT-X-MEDIA-SEQUENCE:42
#EXT-X-TARGETDURATION:8
#EXT-X-KEY:METHOD=AES-128,URI=\"keys/main.key\",IV=0x000102030405060708090a0b0c0d0e0f
#EXTINF:8.0,
seg-1.ts
#EXTINF:6.5,
../seg-2.ts
#EXT-X-ENDLIST
",
            "https://example.test/video/path/index.m3u8",
        )
        .unwrap();
        let Playlist::Media(media) = playlist else {
            panic!("expected media playlist");
        };

        assert!(media.end_list);
        assert_eq!(media.media_sequence, 42);
        assert_eq!(media.segments.len(), 2);
        assert_eq!(
            media.segments[0].uri,
            "https://example.test/video/path/seg-1.ts"
        );
        assert_eq!(media.segments[0].sequence, 42);
        assert_eq!(media.segments[1].uri, "https://example.test/video/seg-2.ts");
        assert_eq!(
            media.segments[0].encryption.as_ref().unwrap().uri,
            "https://example.test/video/path/keys/main.key"
        );
    }

    #[test]
    fn parses_master_and_selects_highest_bandwidth_then_resolution() {
        let playlist = parse_playlist(
            "#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=640x360
low/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=1280x720
mid/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=1920x1080
high/index.m3u8
",
            "https://example.test/master.m3u8",
        )
        .unwrap();
        let Playlist::Master(master) = playlist else {
            panic!("expected master playlist");
        };

        assert_eq!(
            master.best_variant().unwrap().uri,
            "https://example.test/high/index.m3u8"
        );
    }

    #[test]
    fn rejects_live_sample_aes_and_drm() {
        let cases = [
            (
                "#EXTM3U\n#EXTINF:1,\nseg.ts\n",
                "live HLS playlists",
            ),
            (
                "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"k\"\n#EXTINF:1,\nseg.ts\n#EXT-X-ENDLIST\n",
                "SAMPLE-AES",
            ),
            (
                "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\",KEYFORMAT=\"com.apple.streamingkeydelivery\"\n#EXTINF:1,\nseg.ts\n#EXT-X-ENDLIST\n",
                "DRM HLS key formats",
            ),
        ];

        for (content, expected) in cases {
            let error = parse_playlist(content, "https://example.test/index.m3u8").unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn parses_fmp4_init_segment_and_byterange() {
        // 真实 fMP4 清单顺序：每个分片块为
        //   #EXTINF:.., (或 #EXT-X-BYTERANGE:.. 在前)
        //   #EXT-X-BYTERANGE:..  (可选)
        //   <URI>
        // BYTERANGE 紧跟其后的 URI；省略 @offset 时沿用上一区间末尾。
        let playlist = parse_playlist(
            "#EXTM3U
#EXT-X-TARGETDURATION:6
#EXT-X-MAP:URI=\"init.mp4\"
#EXTINF:6.0,
#EXT-X-BYTERANGE:1024@0
video.m4s
#EXTINF:6.0,
#EXT-X-BYTERANGE:2048
video.m4s
#EXTINF:6.0,
#EXT-X-BYTERANGE:2048
video.m4s
#EXT-X-ENDLIST
",
            "https://example.test/stream/index.m3u8",
        )
        .unwrap();
        let Playlist::Media(media) = playlist else {
            panic!("expected media playlist");
        };

        // fMP4：初始化分片被捕获，未拒绝。
        let init = media.init_segment.expect("init segment should be parsed");
        assert_eq!(init.uri, "https://example.test/stream/init.mp4");
        assert_eq!(init.size, None);

        // 三段共享同一个 video.m4s，通过 BYTERANGE 区分。
        assert_eq!(media.segments.len(), 3);
        // 第一段：显式 @offset=0，长度 1024。
        assert_eq!(
            media.segments[0].byte_range,
            Some(ByteRange {
                offset: 0,
                length: Some(1024)
            })
        );
        // 第二段：省略 @offset → 沿用上一区间末尾 (0+1024=1024)。
        assert_eq!(
            media.segments[1].byte_range,
            Some(ByteRange {
                offset: 1024,
                length: Some(2048)
            })
        );
        // 第三段：省略 @offset → 沿用上一区间末尾 (1024+2048=3072)。
        assert_eq!(
            media.segments[2].byte_range,
            Some(ByteRange {
                offset: 3072,
                length: Some(2048)
            })
        );
    }

    #[test]
    fn total_size_requires_all_sizes_known() {
        // 无 init / 无 size → None。
        let playlist = parse_playlist(
            "#EXTM3U\n#EXTINF:1,\nseg.ts\n#EXT-X-ENDLIST\n",
            "https://example.test/index.m3u8",
        )
        .unwrap();
        let Playlist::Media(media) = playlist else {
            panic!()
        };
        assert_eq!(media.total_size(), None);

        // 区间限定的分片 size 已知，但 init 缺失 → None。
        let mut media = media;
        media.init_segment = Some(InitSegment {
            uri: "u".into(),
            byte_range: None,
            size: None,
        });
        assert_eq!(media.total_size(), None);

        // init size 已知 + 区间分片 size 已知 → Some。
        media.init_segment.as_mut().unwrap().size = Some(64);
        // seg.ts 仍 size=None → None。
        assert_eq!(media.total_size(), None);
        media.segments[0].size = Some(128);
        assert_eq!(media.total_size(), Some(64 + 128));
    }

    #[test]
    fn decrypts_aes128_cbc_pkcs7() {
        let key = [7u8; 16];
        let iv = [9u8; 16];
        let plain = b"clear transport stream bytes";
        let encrypted = Aes128CbcEncryptor::new((&key).into(), (&iv).into())
            .encrypt_padded_vec_mut::<Pkcs7>(plain);

        assert_eq!(decrypt_aes128_cbc(&encrypted, &key, &iv).unwrap(), plain);
    }

    #[test]
    fn uses_media_sequence_as_default_iv() {
        let playlist = parse_playlist(
            "#EXTM3U
#EXT-X-MEDIA-SEQUENCE:7
#EXT-X-KEY:METHOD=AES-128,URI=\"k\"
#EXTINF:1,
seg.ts
#EXT-X-ENDLIST
",
            "https://example.test/index.m3u8",
        )
        .unwrap();
        let Playlist::Media(media) = playlist else {
            panic!("expected media playlist");
        };

        let mut expected = [0u8; 16];
        expected[15] = 7;
        assert_eq!(media.segments[0].iv().unwrap(), expected);
    }

    #[test]
    fn parses_map_byterange_and_builds_range_header() {
        let playlist = parse_playlist(
            "#EXTM3U
#EXT-X-TARGETDURATION:6
#EXT-X-MAP:URI=\"media.mp4\",BYTERANGE=\"800@0\"
#EXTINF:6.0,
#EXT-X-BYTERANGE:1200@800
media.mp4
#EXT-X-ENDLIST
",
            "https://example.test/s/index.m3u8",
        )
        .unwrap();
        let Playlist::Media(media) = playlist else {
            panic!("expected media playlist");
        };

        let init = media
            .init_segment
            .expect("init segment should carry a byte range");
        assert_eq!(
            init.byte_range,
            Some(ByteRange {
                offset: 0,
                length: Some(800),
            })
        );
        // #EXT-X-MAP 与媒体分片都转换为含两端的 HTTP Range 头。
        assert_eq!(init.byte_range.unwrap().http_range_value(), "bytes=0-799");
        assert_eq!(
            media.segments[0].byte_range.unwrap().http_range_value(),
            "bytes=800-1999"
        );
    }
}
