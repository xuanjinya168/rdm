// 与桌面端 HTTP 桥通信的薄封装。所有 chrome.* 胶水在 background.js 中，
// 这里只负责 fetch，便于聚焦测试纯逻辑（intercept.js）。

import { DOWNLOADS_URL, HEALTH_URL, MEDIA_CANDIDATES_URL } from "./config.js";

/**
 * 把一个下载交给 RDM 桌面端。桌面端会弹出确认框，用户点「开始下载」
 * 后才真正创建任务。
 *
 * @param {string} url - http/https 下载地址。
 * @param {object} [opts]
 * @param {string} [opts.filename] - 建议文件名。
 * @param {string} [opts.referrer] - 来源页面，用于需要 Referer 的媒体源。
 * @param {string} [opts.pageUrl] - referrer 的兼容别名。
 * @returns {Promise<{ok: true, accepted: true} | {ok: false, error: string}>}
 */
export async function postDownload(url, opts = {}) {
  const referrer = opts.referrer || opts.pageUrl || "";
  const body = {
    url,
    ...(opts.filename ? { filename: opts.filename } : {}),
    ...(referrer ? { referrer } : {}),
  };
  try {
    const res = await fetch(DOWNLOADS_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (res.ok) {
      // 202 Accepted：桌面端已接收，正在等待用户在确认框中点「开始下载」。
      await res.json().catch(() => {});
      return { ok: true, accepted: true };
    }
    let detail = `HTTP ${res.status}`;
    try {
      const payload = await res.json();
      if (payload && payload.error) detail = payload.error;
    } catch {
      /* 非 JSON 响应，保留状态码 */
    }
    return { ok: false, error: detail };
  } catch (error) {
    // 通常是 ECONNREFUSED：桌面端未运行。
    return { ok: false, error: error?.message || String(error) };
  }
}

/**
 * 把一批嗅探到的媒体候选一次性交给 RDM 桌面端。桌面端会弹出批量确认对话框，
 * 用户勾选后才创建任务（对齐 postDownload 的「确认后下载」体验）。
 *
 * @param {Array<object>} candidates - MediaCandidate 数组（含 url/filename/kind 等）。
 * @param {object} [meta]
 * @param {string} [meta.pageUrl]
 * @param {string} [meta.pageTitle]
 * @returns {Promise<{ok: true, accepted: true} | {ok: false, error: string}>}
 */
export async function postMediaCandidates(candidates, meta = {}) {
  const body = {
    candidates,
    ...(meta.pageUrl ? { pageUrl: meta.pageUrl } : {}),
    ...(meta.pageTitle ? { pageTitle: meta.pageTitle } : {}),
  };
  try {
    const res = await fetch(MEDIA_CANDIDATES_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (res.ok) {
      await res.json().catch(() => {});
      return { ok: true, accepted: true };
    }
    let detail = `HTTP ${res.status}`;
    try {
      const payload = await res.json();
      if (payload && payload.error) detail = payload.error;
    } catch {
      /* 非 JSON 响应，保留状态码 */
    }
    return { ok: false, error: detail };
  } catch (error) {
    return { ok: false, error: error?.message || String(error) };
  }
}

/**
 * 探测桌面端桥是否在线，返回版本号（在线）或 null（离线）。
 * @returns {Promise<string | null>}
 */
export async function pingHealth() {
  try {
    const res = await fetch(HEALTH_URL, { method: "GET" });
    if (!res.ok) return null;
    const payload = await res.json();
    return payload && payload.status === "ok" ? payload.version || "?" : null;
  } catch {
    return null;
  }
}
