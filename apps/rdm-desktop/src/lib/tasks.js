import { ACTIVE_FILTER_STATUSES, ACTIVE_STATUSES } from "./format.js";

export function mergeTaskSnapshots(initialTasks, updates) {
  const merged = new Map(initialTasks.map((task) => [task.id, task]));
  for (const update of updates) {
    const task = update.task ?? update;
    const previous = merged.get(task.id);
    if (!previous || task.updated_at >= previous.updated_at) {
      merged.set(task.id, task);
    }
  }
  return [...merged.values()];
}

export function matchesTaskFilter(task, filter) {
  if (filter === "all") return true;
  if (filter === "active") return ACTIVE_STATUSES.has(task.status);
  if (filter === "completed") return task.status === "completed";
  return ["paused", "failed", "canceled"].includes(task.status);
}

export function canStartTask(task) {
  return Boolean(
    task &&
    !ACTIVE_STATUSES.has(task.status) &&
    task.status !== "completed",
  );
}

export function canPauseTask(task) {
  return Boolean(task && ACTIVE_FILTER_STATUSES.has(task.status));
}

export function canDeleteTask(task) {
  return Boolean(task && !ACTIVE_STATUSES.has(task.status));
}
