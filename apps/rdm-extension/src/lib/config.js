// 桌面端 HTTP 桥的固定地址。必须与 apps/rdm-desktop/src-tauri/src/bridge.rs
// 中的 BRIDGE_PORT 保持一致（v1 不做可配置）。
export const BRIDGE_HOST = "127.0.0.1";
export const BRIDGE_PORT = 43721;
export const BRIDGE_BASE_URL = `http://${BRIDGE_HOST}:${BRIDGE_PORT}`;

export const HEALTH_URL = `${BRIDGE_BASE_URL}/health`;
export const DOWNLOADS_URL = `${BRIDGE_BASE_URL}/downloads`;

// chrome.storage.local 中的键。
export const STORAGE_KEYS = {
  // 是否自动拦截浏览器下载（默认关闭，避免一装就抢走所有下载）。
  interceptEnabled: "interceptEnabled",
  // 通知用户「桥不可达」的静默期（毫秒），避免连续刷屏。
  lastOfflineNoticeAt: "lastOfflineNoticeAt",
};

// 默认设置：导入后合并到 chrome.storage.local。
export const DEFAULT_SETTINGS = {
  [STORAGE_KEYS.interceptEnabled]: true,
};

// 拦截过滤规则：低于此字节数的下载不交给 RDM（避免为小文件启动桌面端）。
export const MIN_INTERCEPT_BYTES = 1024 * 100; // 100 KB

// 两次「RDM 未运行」通知之间的最小间隔。
export const OFFLINE_NOTICE_COOLDOWN_MS = 60_000;
