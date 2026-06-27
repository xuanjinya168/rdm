// Service worker：拦截浏览器下载并转发给 RDM，提供右键菜单，
// 并在桌面端不可达时给出通知。纯逻辑位于 ./lib/，便于单测。
//
// 另含「媒体嗅探」的网络采集（M2）：当用户授予 <all_urls> 后，监听页面媒体
// 响应并按 Tab 缓存，供 popup 与 DOM/Performance 结果合并。该链路与上面的下载
// 拦截完全独立、互不干扰。

import { shouldIntercept } from "./lib/intercept.js";
import { postDownload } from "./lib/bridge.js";
import { tabMediaCache } from "./lib/cache.js";
import { isMediaResponse } from "./lib/sniffer.js";
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
  const { filename, referrer, notifyOnError = true } = opts;
  const result = await postDownload(url, {
    ...(filename ? { filename } : {}),
    ...(referrer ? { referrer } : {}),
  });
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
  await handoffToRdm(url, { referrer: info.pageUrl || info.frameUrl });
});

/** 取文件名部分（去掉目录路径），供桌面端命名。 */
function stripPath(filename) {
  const base = String(filename).split(/[\\/]/).pop() || filename;
  return base.trim() || undefined;
}

// --- 媒体嗅探：网络采集（M2，与下载拦截独立） ----------------------------
// 监听响应完成事件，命中媒体的存入按 Tab 隔离的缓存，供 popup 读取。
// 监听器在顶层同步注册（MV3 要求），但仅当用户已授予对应站点的 host 权限
// （optional <all_urls>）时浏览器才会投递这些事件——未授权即静默无数据，
// DOM/Performance 嗅探照常可用，符合「不默认全局抓包」的隐私约束。

function onMediaResponseCompleted(details) {
  const { tabId, url, responseHeaders } = details;
  if (tabId == null || tabId < 0 || !url) return;
  let contentType = "";
  let contentLength;
  for (const h of responseHeaders || []) {
    const name = (h.name || "").toLowerCase();
    if (name === "content-type") contentType = h.value || "";
    else if (name === "content-length") {
      const n = Number(h.value);
      if (Number.isFinite(n) && n > 0) contentLength = n;
    }
  }
  if (!isMediaResponse(url, contentType)) return;
  // 异步写入 chrome.storage.session（内部串行化），webRequest 监听无需等待。
  tabMediaCache.add(tabId, { url, contentType, contentLength }).catch(() => {});
}

if (chrome.webRequest) {
  chrome.webRequest.onCompleted.addListener(
    onMediaResponseCompleted,
    { urls: ["<all_urls>"] },
    ["responseHeaders"],
  );
}

// Tab 关闭或开始导航到新页面时，清掉其网络嗅探缓存。
chrome.tabs.onRemoved.addListener((tabId) => {
  tabMediaCache.clear(tabId).catch(() => {});
});
chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === "loading") tabMediaCache.clear(tabId).catch(() => {});
});

// popup 读取/清理某 Tab 的网络嗅探缓存（缓存读写为异步，返回 true 保持通道）。
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (!msg || typeof msg !== "object") return;
  if (msg.type === "getNetworkMedia") {
    tabMediaCache
      .get(msg.tabId)
      .then((items) => sendResponse({ items }))
      .catch(() => sendResponse({ items: [] }));
    return true;
  }
  if (msg.type === "clearNetworkMedia") {
    tabMediaCache
      .clear(msg.tabId)
      .then(() => sendResponse({ ok: true }))
      .catch(() => sendResponse({ ok: false }));
    return true;
  }
});
