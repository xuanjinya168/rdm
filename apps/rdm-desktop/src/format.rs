//! 下载列表里用到的展示格式。语义与原先的桌面界面保持一致。

use rdm_domain::TaskStatus;

const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

pub fn format_bytes(value: u64) -> String {
    if value == 0 {
        return "0 B".to_string();
    }
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", size.round() as u64, UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub fn format_speed(value: f64) -> String {
    if value <= 0.0 {
        return "—".to_string();
    }
    format!("{}/s", format_bytes(value.round() as u64))
}

pub fn format_eta(total: Option<u64>, downloaded: u64, speed: f64) -> String {
    let Some(total) = total else {
        return "—".to_string();
    };
    if speed <= 0.0 {
        return "—".to_string();
    }
    let remaining = total.saturating_sub(downloaded);
    if remaining == 0 {
        return "—".to_string();
    }
    let seconds = (remaining as f64 / speed).round() as u64;
    if seconds < 60 {
        return format!("{seconds} 秒");
    }
    if seconds < 3600 {
        let minutes = seconds / 60;
        let rest = seconds % 60;
        return if rest == 0 {
            format!("{minutes} 分")
        } else {
            format!("{minutes} 分 {rest} 秒")
        };
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if minutes == 0 {
        format!("{hours} 时")
    } else {
        format!("{hours} 时 {minutes} 分")
    }
}

pub fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "等待中",
        TaskStatus::Probing => "分析中",
        TaskStatus::Downloading => "下载中",
        TaskStatus::Verifying => "校验中",
        TaskStatus::Paused => "已暂停",
        TaskStatus::Completed => "已完成",
        TaskStatus::Failed => "失败",
        TaskStatus::Canceled => "已取消",
    }
}

/// 占用队列或工作槽的状态。暂停按钮只覆盖真正在跑的三种。
pub fn is_queued_or_active(status: TaskStatus) -> bool {
    status.is_active() || status == TaskStatus::Queued
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_and_speed() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_speed(0.0), "—");
        assert_eq!(format_speed(2048.0), "2.0 KB/s");
    }

    #[test]
    fn formats_eta_buckets() {
        assert_eq!(format_eta(None, 0, 10.0), "—");
        assert_eq!(format_eta(Some(100), 100, 10.0), "—");
        assert_eq!(format_eta(Some(100), 50, 10.0), "5 秒");
        assert_eq!(format_eta(Some(120), 0, 1.0), "2 分");
        assert_eq!(format_eta(Some(90), 0, 1.0), "1 分 30 秒");
        assert_eq!(format_eta(Some(3600), 0, 1.0), "1 时");
        assert_eq!(format_eta(Some(3660), 0, 1.0), "1 时 1 分");
    }
}
