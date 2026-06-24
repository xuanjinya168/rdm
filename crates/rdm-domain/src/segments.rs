//! 分段规划与续传校验。从 Python `downloader.segments` 模块迁移而来。

use crate::models::Segment;

/// 将一个下载任务切分为大小接近的分段。当服务器不支持随机读取，
/// 或总大小未知时，会退化为单条开放式的分段。
pub fn build_segments(
    task_id: &str,
    total_size: Option<u64>,
    supports_ranges: bool,
    connections: u32,
    minimum_size: u64,
) -> Vec<Segment> {
    let total = total_size.unwrap_or(0);
    if !supports_ranges || total == 0 {
        let end = if total > 0 { Some(total - 1) } else { None };
        return vec![Segment::new(task_id, 0, 0, end)];
    }

    let max_by_size = total.div_ceil(minimum_size).max(1);
    let count = (connections as u64).min(max_by_size);
    let base_size = total / count;
    let remainder = total % count;

    let mut segments = Vec::with_capacity(count as usize);
    let mut start = 0u64;
    for index in 0..count {
        let size = base_size + if index < remainder { 1 } else { 0 };
        let end = start + size - 1;
        segments.push(Segment::new(task_id, index as u32, start, Some(end)));
        start = end + 1;
    }
    segments
}

/// 当持久化的分段能够无间隙地覆盖 `[0, total_size)`，
/// 且与磁盘上 `.part` 文件大小一致时返回 true，表示可以安全续传。
pub fn valid_resume_segments(
    segments: &[Segment],
    total_size: Option<u64>,
    part_size: u64,
) -> bool {
    let total = match total_size {
        Some(total) if total > 0 => total,
        _ => return false,
    };
    if part_size != total || segments.is_empty() {
        return false;
    }

    let mut indices: Vec<u32> = segments.iter().map(|s| s.index).collect();
    indices.sort_unstable();
    indices.dedup();
    if indices.len() != segments.len() {
        return false;
    }

    let mut ordered: Vec<&Segment> = segments.iter().collect();
    ordered.sort_by_key(|s| s.start);
    let mut position = 0u64;
    for segment in ordered {
        match segment.end {
            Some(end) if segment.start == position && end >= segment.start => {
                let size = end - segment.start + 1;
                if segment.downloaded > size {
                    return false;
                }
                position = end + 1;
            }
            _ => return false,
        }
    }
    position == total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_when_ranges_unsupported() {
        let segs = build_segments("t", Some(1000), false, 8, 512);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].end, Some(999));

        let unknown = build_segments("t", None, true, 8, 512);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].end, None);
    }

    #[test]
    fn splits_evenly_and_covers_range() {
        let segs = build_segments("t", Some(1003), true, 4, 1);
        assert_eq!(segs.len(), 4);
        // 1003 / 4 = 250 余 3 -> 前三个得到 251，最后一个得到 250。
        assert_eq!(segs[0].size(), Some(251));
        assert_eq!(segs[3].size(), Some(250));
        let covered: u64 = segs.iter().map(|s| s.size().unwrap()).sum();
        assert_eq!(covered, 1003);
        assert_eq!(segs.last().unwrap().end, Some(1002));
    }

    #[test]
    fn segment_count_bounded_by_minimum_size() {
        // 尽管有 8 个连接，但只有 2 个 >=512 字节的分段的空间。
        let segs = build_segments("t", Some(1024), true, 8, 512);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn resume_accepts_contiguous_cover() {
        let segs = build_segments("t", Some(1000), true, 4, 1);
        assert!(valid_resume_segments(&segs, Some(1000), 1000));
    }

    #[test]
    fn resume_rejects_mismatches() {
        let segs = build_segments("t", Some(1000), true, 4, 1);
        assert!(!valid_resume_segments(&segs, Some(1000), 999)); // 分段大小不一致
        assert!(!valid_resume_segments(&segs, Some(900), 900)); // 总大小不一致
        assert!(!valid_resume_segments(&[], Some(1000), 1000)); // 空列表

        let mut gapped = segs.clone();
        gapped.remove(1); // 留下空洞
        assert!(!valid_resume_segments(&gapped, Some(1000), 1000));

        let mut over = segs.clone();
        over[0].downloaded = over[0].size().unwrap() + 1;
        assert!(!valid_resume_segments(&over, Some(1000), 1000));
    }
}
