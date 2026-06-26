// 纯函数：判断一个 chrome.downloads.DownloadItem 是否应交给 RDM 拦截。
// 不依赖任何 chrome.* API，便于用 node --test 单测。
//
// 字段参考：https://developer.chrome.com/docs/extensions/reference/api/downloads#type-DownloadItem
//   item.url / item.finalUrl        下载地址
//   item.filename                   保存路径（含扩展名）
//   item.fileSize / item.totalBytes 文件大小
//   item.state                      "in_progress" | "interrupted" | "complete"

import { MIN_INTERCEPT_BYTES } from "./config.js";

const IGNORED_SCHEMES = ["data:", "blob:", "javascript:", "about:", "chrome:", "edge:"];

/** 从一个可能带路径的字符串里取扩展名（小写，不带点）。 */
function extensionOf(value) {
  if (!value) return "";
  const base = value.split(/[\\/]/).pop() || value;
  const dot = base.lastIndexOf(".");
  if (dot < 0) return "";
  return base.slice(dot + 1).toLowerCase();
}

/**
 * 判断 `item` 是否应被 RDM 拦截。
 *
 * @param {object} item - chrome.downloads.DownloadItem 的简化形态。
 * @param {object} settings - 来自 chrome.storage.local 的设置快照。
 * @param {boolean} settings.interceptEnabled - 总开关关闭时一律不拦截。
 * @returns {boolean}
 */
export function shouldIntercept(item, settings) {
  if (!item) return false;
  if (!settings || settings.interceptEnabled !== true) return false;

  const url = (item.finalUrl || item.url || "").trim();
  if (!url) return false;
  if (IGNORED_SCHEMES.some((scheme) => url.startsWith(scheme))) return false;
  // 仅 http/https 直链交给 RDM。
  if (!/^https?:\/\//i.test(url)) return false;

  // 跳过过小的文件（如统计像素、字体）。
  const size = item.fileSize ?? item.totalBytes;
  if (typeof size === "number" && size > 0 && size < MIN_INTERCEPT_BYTES) return false;

  // 跳过典型网页/脚本资源（避免拦截页面本身）。
  const ext = extensionOf(item.filename) || extensionOf(url);
  const skipExt = new Set([
    "html",
    "htm",
    "xhtml",
    "css",
    "js",
    "mjs",
    "json",
    "svg",
    "woff",
    "woff2",
    "ttf",
    "eot",
  ]);
  if (skipExt.has(ext)) return false;

  return true;
}
