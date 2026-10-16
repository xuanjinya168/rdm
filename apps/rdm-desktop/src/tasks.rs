//! 任务列表的筛选与按钮可用性。规则与原先的桌面界面一致。

use std::collections::HashMap;

use rdm_domain::{DownloadTask, TaskStatus};

use crate::format::is_queued_or_active;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TaskFilter {
    All,
    Active,
    Completed,
    Other,
}

pub fn matches_filter(task: &DownloadTask, filter: TaskFilter) -> bool {
    match filter {
        TaskFilter::All => true,
        TaskFilter::Active => is_queued_or_active(task.status),
        TaskFilter::Completed => task.status == TaskStatus::Completed,
        TaskFilter::Other => matches!(
            task.status,
            TaskStatus::Paused | TaskStatus::Failed | TaskStatus::Canceled
        ),
    }
}

pub fn can_start(task: &DownloadTask) -> bool {
    !is_queued_or_active(task.status) && task.status != TaskStatus::Completed
}

pub fn can_pause(task: &DownloadTask) -> bool {
    task.status.is_active()
}

pub fn can_delete(task: &DownloadTask) -> bool {
    !is_queued_or_active(task.status)
}

/// 用更新时间合并初始快照和随后到达的进度，避免旧事件覆盖新状态。
pub fn merge_task_snapshots(
    initial: Vec<DownloadTask>,
    updates: impl IntoIterator<Item = DownloadTask>,
) -> Vec<DownloadTask> {
    let mut order: Vec<String> = initial.iter().map(|task| task.id.clone()).collect();
    let mut merged: HashMap<String, DownloadTask> = initial
        .into_iter()
        .map(|task| (task.id.clone(), task))
        .collect();
    for task in updates {
        let replace = merged
            .get(&task.id)
            .map(|previous| task.updated_at >= previous.updated_at)
            .unwrap_or(true);
        if replace {
            if !merged.contains_key(&task.id) {
                order.push(task.id.clone());
            }
            merged.insert(task.id.clone(), task);
        }
    }
    order
        .into_iter()
        .filter_map(|id| merged.remove(&id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TaskStatus, updated_at: f64) -> DownloadTask {
        let mut task =
            DownloadTask::create("https://example.com/file.bin", "/tmp", 1, "file.bin", "")
                .unwrap();
        task.id = id.to_string();
        task.status = status;
        task.updated_at = updated_at;
        task
    }

    #[test]
    fn newer_update_replaces_initial_snapshot() {
        let merged = merge_task_snapshots(
            vec![task("a", TaskStatus::Downloading, 10.0)],
            [task("a", TaskStatus::Completed, 12.0)],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, TaskStatus::Completed);
    }

    #[test]
    fn older_update_does_not_overwrite_newer_snapshot() {
        let merged = merge_task_snapshots(
            vec![task("a", TaskStatus::Completed, 12.0)],
            [
                task("a", TaskStatus::Downloading, 10.0),
                task("b", TaskStatus::Queued, 11.0),
            ],
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a");
        assert_eq!(merged[0].status, TaskStatus::Completed);
        assert_eq!(merged[1].id, "b");
    }

    #[test]
    fn filters_and_actions_follow_status() {
        let downloading = task("a", TaskStatus::Downloading, 1.0);
        let queued = task("b", TaskStatus::Queued, 1.0);
        let paused = task("c", TaskStatus::Paused, 1.0);
        let completed = task("d", TaskStatus::Completed, 1.0);

        assert!(matches_filter(&downloading, TaskFilter::Active));
        assert!(matches_filter(&completed, TaskFilter::Completed));
        assert!(matches_filter(&paused, TaskFilter::Other));
        assert!(!matches_filter(&queued, TaskFilter::Other));

        assert!(!can_start(&queued));
        assert!(!can_pause(&queued));
        assert!(!can_delete(&queued));
        assert!(can_pause(&downloading));
        assert!(can_start(&paused));
        assert!(can_delete(&paused));
        assert!(can_delete(&completed));
        assert!(!can_start(&completed));
    }
}
