// Service worker：拦截浏览器下载并转发给 RDM，提供右键菜单，
// 并在桌面端不可达时给出通知。纯逻辑位于 ./lib/，便于单测。

import { shouldIntercept } from "./lib/intercept.js";
import { postDownload } from "./lib/bridge.js";
import {
  STORAGE_KEYS,
  DEFAULT_SETTINGS,
  OFFLINE_NOTICE_COOLDOWN_MS,
} from "./lib/config.js";

const NOTIFICATION_ID_OFFLINE = "rdm-bridge-offline";

/** 读取设置，合并默认值。 */
async function readSettings() {
  const stored = await chrome.storage.local.get(Object.keys(STORAGE_KEYS));
  return { ...DEFAULT_SETTINGS, ...stored };
}

/** 通知图标（用扩展自带的小图标）。 */
function iconUrl(size) {
  return chrome.runtime.getURL(`icons/icon${size}.png`);
}

/** 在冷却期内只通知一次，避免桌面端离线时刷屏。 */
async function maybeNotifyOffline() {
  const { [STORAGE_KEYS.lastOfflineNoticeAt]: last = 0 } = await chrome.storage.local.get(
    STORAGE_KEYS.lastOfflineNoticeAt,
  );
  if (Date.now() - last < OFFLINE_NOTICE_COOLDOWN_MS) return;
  await chrome.storage.local.set({ [STORAGE_KEYS.lastOfflineNoticeAt]: Date.now() });
  chrome.notifications.create(NOTIFICATION_ID_OFFLINE, {
    type: "basic",
    iconUrl: iconUrl(48),
    title: "RDM 未运行",
    message: "无法连接到 RDM 桌面端，请确认应用已启动。本次下载仍由浏览器处理。",
    priority: 2,
  });
}

/**
 * 把 URL 交给 RDM。成功则忽略；失败（桌面端未运行/拒绝）则可选地
 * 通知用户。这里不抛错，避免打断浏览器自身的下载流程。
 *
 * @param {string} url
 * @param {object} [opts]
 * @param {string} [opts.filename]
 * @param {boolean} [opts.notifyOnError] - 默认 true。
 */
async function handoffToRdm(url, opts = {}) {
  const { filename, notifyOnError = true } = opts;
  const result = await postDownload(url, filename ? { filename } : {});
  if (!result.ok && notifyOnError) {
    await maybeNotifyOffline();
  }
  return result;
}

// --- 自动拦截 -------------------------------------------------------------
// 当总开关开启时，新下载若满足条件则交给 RDM。
//
// 顺序很重要：必须先 POST 给桥并确认接收（202），成功后再取消浏览器
// 下载；否则桥不可达时（桌面端未运行）会丢失下载——浏览器侧已被取消，
// 桥侧又没接住。桥接收失败时浏览器下载照常进行。
chrome.downloads.onCreated.addListener(async (downloadItem) => {
  const settings = await readSettings();
  if (!shouldIntercept(downloadItem, settings)) return;

  const url = downloadItem.finalUrl || downloadItem.url;
  const filename = downloadItem.filename ? stripPath(downloadItem.filename) : undefined;
  const result = await handoffToRdm(url, { filename });
  if (!result.ok) return; // 桥不可达：保留浏览器下载，已（或将要）通知用户。

  // 桥已接收并弹出确认框：取消并移除浏览器自身的下载条目。
  await chrome.downloads.cancel(downloadItem.id).catch(() => {});
  await chrome.downloads.erase({ id: downloadItem.id }).catch(() => {});
});

// --- 右键菜单 -------------------------------------------------------------
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "rdm-download-link",
    title: "用 RDM 下载此链接",
    contexts: ["link"],
  });
  chrome.contextMenus.create({
    id: "rdm-download-media",
    title: "用 RDM 下载此媒体",
    contexts: ["image", "video", "audio"],
  });
  chrome.contextMenus.create({
    id: "rdm-download-page",
    title: "用 RDM 下载此页面",
    contexts: ["page"],
  });
});

chrome.contextMenus.onClicked.addListener(async (info) => {
  // link/media 优先取具体资源地址，页面则用当前页 URL。
  const url =
    info.linkUrl || info.srcUrl || info.frameUrl || info.pageUrl;
  if (!url) return;
  await handoffToRdm(url);
});

/** 取文件名部分（去掉目录路径），供桌面端命名。 */
function stripPath(filename) {
  const base = String(filename).split(/[\\/]/).pop() || filename;
  return base.trim() || undefined;
}
