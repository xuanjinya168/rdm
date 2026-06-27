//! HLS 下载完成后的容器化/转码后处理。
//!
//! 下载引擎把解密后的分片二进制拼接成原始的 MPEG-TS（`.ts`）或
//! fMP4（`.m4s`）流。这类裸流虽能在 VLC/mpv 播放，却不是规范的 `.mp4`
//! 容器（浏览器、QuickTime、移动端常无法播放，且没有 faststart）。
//!
//! 本模块用外部 `ffmpeg` 把裸流封装/转码为标准 `.mp4`：
//! - [`FinalizeMode::Remux`]：`-c copy` 无损封装（秒成，不用 GPU）；
//! - [`FinalizeMode::Transcode`]：用指定编码器（多为 GPU，如
//!   `h264_nvenc`/`h264_qsv`/`h264_amf`）重新编码视频；
//! - [`FinalizeMode::Off`]：不处理，保留裸流。
//!
//! GPU 是否可用由 [`detect_hw_encoder`] 在运行时实测：先列出 ffmpeg
//! 支持的编码器，再对候选项做一次极小的试编码，只有真正能初始化成功的
//! 才采用。检测一次的结果应由上层缓存（每个下载不必重复探测）。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 按优先级排列的硬件 H.264 编码器候选：NVIDIA → Intel → AMD。
pub const HW_ENCODER_CANDIDATES: &[&str] = &["h264_nvenc", "h264_qsv", "h264_amf"];

/// 拼接完成后对裸流的处理方式。由上层把策略 + GPU 检测结果解析为具体模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizeMode {
    /// 不做任何处理，保留原始 `.ts`/`.m4s`。
    Off,
    /// 仅重新封装为 `.mp4`（`-c copy`，无损、最快）。
    Remux,
    /// 重新编码视频为 `.mp4`，`video_encoder` 为 ffmpeg 编码器名。
    Transcode { video_encoder: String },
}

/// 已解析好的 ffmpeg 后处理配置：可执行文件 + 模式。
#[derive(Debug, Clone)]
pub struct PostProcess {
    pub ffmpeg: PathBuf,
    pub mode: FinalizeMode,
}

impl PostProcess {
    /// 是否需要实际产出 `.mp4`（`Off` 时无需调用 ffmpeg）。
    pub fn produces_mp4(&self) -> bool {
        !matches!(self.mode, FinalizeMode::Off)
    }
}

/// 构造 ffmpeg 命令行参数。`Off` 返回 `None`（无需执行）。
///
/// - Remux：`-c copy`；仅 MPEG-TS（非 fMP4）才加 `aac_adtstoasc`，
///   把 TS 的 ADTS 封装 AAC 转成 MP4 的 ASC 封装，否则音频可能不可播。
/// - Transcode：指定 `-c:v <encoder>` 重编码视频，音频统一 `-c:a aac`。
/// - 两者都加 `+faststart`，把 moov 前置以便边下边播 / 秒开拖动。
pub fn build_ffmpeg_args(
    mode: &FinalizeMode,
    input: &str,
    output: &str,
    fmp4: bool,
) -> Option<Vec<String>> {
    let mut args = vec![
        "-y".to_string(),
        "-nostdin".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-i".to_string(),
        input.to_string(),
    ];
    match mode {
        FinalizeMode::Off => return None,
        FinalizeMode::Remux => {
            args.push("-c".to_string());
            args.push("copy".to_string());
            if !fmp4 {
                args.push("-bsf:a".to_string());
                args.push("aac_adtstoasc".to_string());
            }
        }
        FinalizeMode::Transcode { video_encoder } => {
            args.push("-c:v".to_string());
            args.push(video_encoder.clone());
            args.push("-c:a".to_string());
            args.push("aac".to_string());
        }
    }
    args.push("-movflags".to_string());
    args.push("+faststart".to_string());
    // 输出路径常带 `.part` 后缀（发布前的占位名），ffmpeg 无法据此推断
    // 封装格式，因此显式指定 mp4。
    args.push("-f".to_string());
    args.push("mp4".to_string());
    args.push(output.to_string());
    Some(args)
}

/// 对 `input` 执行后处理并写出 `output`。`Off` 模式直接返回 `Ok(false)`
/// （表示未产出 mp4，调用方应保留裸流）；成功执行返回 `Ok(true)`。
pub async fn finalize(
    config: &PostProcess,
    input: &Path,
    output: &Path,
    fmp4: bool,
) -> Result<bool, String> {
    let Some(args) = build_ffmpeg_args(
        &config.mode,
        &input.to_string_lossy(),
        &output.to_string_lossy(),
        fmp4,
    ) else {
        return Ok(false);
    };
    let result = ffmpeg_command(&config.ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("无法启动 ffmpeg（{}）：{e}", config.ffmpeg.display()))?;
    if result.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&result.stderr);
    let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join("; ");
    Err(format!(
        "ffmpeg 退出码 {}：{tail}",
        result.status.code().unwrap_or(-1)
    ))
}

/// 实测可用的硬件 H.264 编码器（无则 `None`）。
///
/// 先用 `ffmpeg -encoders` 取得编译期支持的编码器列表，再对其中的候选项
/// 逐个做一次极小试编码（编译期支持 ≠ 当前机器有可用 GPU），返回第一个
/// 真正能跑通的。该探测有进程开销，结果应由调用方缓存。
pub async fn detect_hw_encoder(ffmpeg: &Path) -> Option<String> {
    let listed = list_encoders(ffmpeg).await?;
    for candidate in HW_ENCODER_CANDIDATES {
        if listed.contains(*candidate) && probe_encoder(ffmpeg, candidate).await {
            return Some((*candidate).to_string());
        }
    }
    None
}

/// `ffmpeg` 是否可执行（运行 `-version` 成功）。用于在启用后处理前确认
/// 二进制可用——不可用时应保留裸流，而非让下载因转封装失败而整单失败。
pub async fn probe_available(ffmpeg: &Path) -> bool {
    ffmpeg_command(ffmpeg)
        .args(["-hide_banner", "-version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn list_encoders(ffmpeg: &Path) -> Option<String> {
    let output = ffmpeg_command(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-encoders"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 对单个编码器做一次 ~0.2s 的试编码（lavfi 色块源 → null 输出）。
/// 退出码 0 表示该编码器在本机能初始化并实际编码成功。
async fn probe_encoder(ffmpeg: &Path, encoder: &str) -> bool {
    ffmpeg_command(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=128x72:r=5:d=0.2",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ffmpeg_command(ffmpeg: &Path) -> Command {
    let mut command = Command::new(ffmpeg);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_mode_produces_no_command() {
        assert!(build_ffmpeg_args(&FinalizeMode::Off, "in.ts", "out.mp4", false).is_none());
    }

    #[test]
    fn remux_ts_adds_adts_bitstream_filter_and_faststart() {
        let args = build_ffmpeg_args(&FinalizeMode::Remux, "in.ts", "out.mp4", false).unwrap();
        assert!(args.windows(2).any(|w| w == ["-c", "copy"]));
        assert!(args.windows(2).any(|w| w == ["-bsf:a", "aac_adtstoasc"]));
        assert!(args.windows(2).any(|w| w == ["-movflags", "+faststart"]));
        assert!(args.windows(2).any(|w| w == ["-f", "mp4"]));
        assert_eq!(args.last().unwrap(), "out.mp4");
    }

    #[test]
    fn remux_fmp4_omits_adts_bitstream_filter() {
        // fMP4 的音频已是 MP4 封装，aac_adtstoasc 不适用，必须省略。
        let args = build_ffmpeg_args(&FinalizeMode::Remux, "in.m4s", "out.mp4", true).unwrap();
        assert!(args.windows(2).any(|w| w == ["-c", "copy"]));
        assert!(!args.iter().any(|a| a == "aac_adtstoasc"));
    }

    #[test]
    fn transcode_sets_video_encoder_and_aac_audio() {
        let mode = FinalizeMode::Transcode {
            video_encoder: "h264_nvenc".to_string(),
        };
        let args = build_ffmpeg_args(&mode, "in.ts", "out.mp4", false).unwrap();
        assert!(args.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
        // 转码不做裸流的 ADTS 转换（重编码已重建封装）。
        assert!(!args.iter().any(|a| a == "aac_adtstoasc"));
        assert!(args.windows(2).any(|w| w == ["-movflags", "+faststart"]));
    }

    #[test]
    fn produces_mp4_reflects_mode() {
        let off = PostProcess {
            ffmpeg: PathBuf::from("ffmpeg"),
            mode: FinalizeMode::Off,
        };
        assert!(!off.produces_mp4());
        let remux = PostProcess {
            ffmpeg: PathBuf::from("ffmpeg"),
            mode: FinalizeMode::Remux,
        };
        assert!(remux.produces_mp4());
    }
}
