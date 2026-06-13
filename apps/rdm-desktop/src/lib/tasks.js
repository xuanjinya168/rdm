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
