// Display helpers, mirroring the Python ui/formatters.

const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(value) {
  if (!value || value <= 0) return "0 B";
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < UNITS.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(unit === 0 ? 0 : 1)} ${UNITS[unit]}`;
}

export function formatSpeed(value) {
  if (!value || value <= 0) return "—";
  return `${formatBytes(value)}/s`;
}

export function formatEta(task, speed) {
  if (!task.total_size || !speed || speed <= 0) return "—";
  const remaining = task.total_size - task.downloaded;
  if (remaining <= 0) return "—";
  let seconds = Math.round(remaining / speed);
  if (seconds < 60) return `${seconds} 秒`;
  if (seconds < 3600) {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return s ? `${m} 分 ${s} 秒` : `${m} 分`;
  }
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return m ? `${h} 时 ${m} 分` : `${h} 时`;
}

export function percent(task) {
  if (!task.total_size) return 0;
  return Math.min(100, (task.downloaded / task.total_size) * 100);
}

const STATUS_LABELS = {
  queued: "等待中",
  probing: "分析中",
  downloading: "下载中",
  verifying: "校验中",
  paused: "已暂停",
  completed: "已完成",
  failed: "失败",
  canceled: "已取消",
};

export function statusLabel(status) {
  return STATUS_LABELS[status] ?? status;
}

export const ACTIVE_STATUSES = new Set([
  "queued",
  "probing",
  "downloading",
  "verifying",
]);

// Statuses that count as "active" for the filter/pause controls (excludes the
// not-yet-started "queued"), matching ACTIVE_FILTER_STATUSES in Python.
export const ACTIVE_FILTER_STATUSES = new Set([
  "probing",
  "downloading",
  "verifying",
]);
