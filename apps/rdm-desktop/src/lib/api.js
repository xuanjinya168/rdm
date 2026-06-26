// 对 Tauri 命令(src-tauri/src/lib.rs)及后端事件的薄封装。
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const listTasks = () => invoke("list_tasks");
export const getSettings = () => invoke("get_settings");
export const takeLaunchUrl = () => invoke("take_launch_url");
export const saveSettings = (settings) => invoke("save_settings", { settings });

export const addDownload = ({ url, destination, connections, filename, sha256 }) =>
  invoke("add_download", { url, destination, connections, filename, sha256 });

export const startTask = (id) => invoke("start_task", { id });
export const pauseTask = (id) => invoke("pause_task", { id });
export const cancelTask = (id) => invoke("cancel_task", { id });
export const deleteTask = (id, deleteFile = false) =>
  invoke("delete_task", { id, deleteFile });
export const revealTaskFile = ({ id }) => invoke("reveal_task_file", { id });

// 将社交媒体 / 网页帖子的 URL 解析为可下载的媒体项。
export const resolveMedia = (url) => invoke("resolve_media", { url });

// 事件订阅(每个订阅返回一个 Promise<unlisten>)。
export const onTaskUpdate = (handler) =>
  listen("task://update", (event) => handler(event.payload));
export const onOpenUrl = (handler) =>
  listen("rdm://open-url", (event) => handler(event.payload));
export const onNewDownload = (handler) =>
  listen("rdm://new-download", () => handler());
// 浏览器扩展通过本地 HTTP 桥拦截下载后，后端发出该事件，
// 由前端弹出确认框（payload: { url, filename? }）。
export const onExternalDownload = (handler) =>
  listen("rdm://external-download", (event) => handler(event.payload));
