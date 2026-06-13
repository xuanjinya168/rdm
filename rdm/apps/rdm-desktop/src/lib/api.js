// Wrappers over the Tauri commands (src-tauri/src/lib.rs) and the events the
// backend emits.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const listTasks = () => invoke("list_tasks");
export const getSettings = () => invoke("get_settings");
export const saveSettings = (settings) => invoke("save_settings", { settings });

export const addDownload = ({ url, destination, connections, filename, sha256 }) =>
  invoke("add_download", { url, destination, connections, filename, sha256 });

export const startTask = (id) => invoke("start_task", { id });
export const pauseTask = (id) => invoke("pause_task", { id });
export const cancelTask = (id) => invoke("cancel_task", { id });
export const deleteTask = (id, deleteFile = false) =>
  invoke("delete_task", { id, deleteFile });
export const openFolder = (path) => invoke("open_folder", { path });

// Subscriptions (each returns a Promise<unlisten>).
export const onTaskUpdate = (handler) =>
  listen("task://update", (event) => handler(event.payload));
export const onOpenUrl = (handler) =>
  listen("rdm://open-url", (event) => handler(event.payload));
export const onNewDownload = (handler) =>
  listen("rdm://new-download", () => handler());
