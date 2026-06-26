// 媒体嗅探的纯逻辑：把页面/网络采集到的「原始项」归一化为统一的
// MediaCandidate。不依赖任何 chrome.* / DOM API，便于用 node --test 单测，
// 对齐 ./intercept.js 的模式（纯函数 + 同名 .test.js）。

/**
 * @typedef {Object} MediaCandidate
 * @property {string} url                                   资源完整地址
 * @property {"image"|"video"|"audio"|"manifest"} kind      资源类型
 * @property {string} ext                                   文件后缀（小写，不带点）
 * @property {string} filename                              推断文件名
 * @property {number} [width]                               图片/视频宽
 * @property {number} [height]                              图片/视频高
 * @property {number} [duration]                            音视频时长（秒）
 * @property {number} [bytes]                               Content-Length（字节）
 * @property {string} contentType                           Response Content-Type
 * @property {string} pageUrl                               来源页面地址
 * @property {string} pageTitle                             来源页面标题
 * @property {"dom"|"network"|"performance"} source         采集来源
 */

/**
 * @typedef {Object} RawMediaItem
 * @property {string} [url]
 * @property {string} [srcset]
 * @property {""|"image"|"video"|"audio"|"manifest"} [kind] 采集侧的类型提示（如 DOM 标签）
 * @property {number} [width]
 * @property {number} [height]
 * @property {number} [duration]
 * @property {number} [bytes]
 * @property {string} [contentType]
 * @property {"dom"|"network"|"performance"} [source]
 */

export const MEDIA_KINDS = ["image", "video", "audio", "manifest"];

// 无效/不可下载的链接前缀，统一过滤。
const INVALID_PREFIXES = ["data:", "blob:", "javascript:", "about:", "mailto:", "#"];

// 后缀 → 类型。manifest 仅识别展示（HLS/DASH），下载引擎后置迭代。
const EXT_KIND = {
  jpg: "image", jpeg: "image", png: "image", gif: "image", webp: "image",
  avif: "image", bmp: "image", svg: "image", ico: "image", tif: "image", tiff: "image",
  mp4: "video", webm: "video", mov: "video", mkv: "video", avi: "video",
  m4v: "video", ogv: "video", flv: "video", "3gp": "video",
  mp3: "audio", m4a: "audio", aac: "audio", ogg: "audio", oga: "audio",
  opus: "audio", flac: "audio", wav: "audio",
  m3u8: "manifest", mpd: "manifest",
};

const CONTENT_TYPE_EXT = {
  "image/jpeg": "jpg",
  "image/jpg": "jpg",
  "image/png": "png",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/avif": "avif",
  "image/bmp": "bmp",
  "image/svg+xml": "svg",
  "image/tiff": "tif",
  "video/mp4": "mp4",
  "video/webm": "webm",
  "video/quicktime": "mov",
  "audio/mpeg": "mp3",
  "audio/mp4": "m4a",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "application/vnd.apple.mpegurl": "m3u8",
  "application/x-mpegurl": "m3u8",
  "application/dash+xml": "mpd",
};

// HLS/DASH 分片：不是可单独下载的完整文件（一个视频会拆成成百上千个分片）。
// 识别为 "segment" 后在候选阶段直接丢弃，避免把一堆分片当文件下载；也不缓存。
// 真正可下载的是 .m3u8/.mpd manifest（下载引擎后置迭代）。
const SEGMENT_EXT = new Set(["ts", "m4s", "cmfv", "cmfa"]);
const SEGMENT_CONTENT_TYPES = new Set([
  "video/mp2t",
  "video/iso.segment",
  "audio/iso.segment",
]);

/**
 * 是否为应保留的链接（过滤 data:/blob:/javascript:/# 等）。
 * @param {string} url
 * @returns {boolean}
 */
export function filterInvalidUrl(url) {
  if (!url || typeof url !== "string") return false;
  const u = url.trim().toLowerCase();
  if (!u) return false;
  return !INVALID_PREFIXES.some((p) => u.startsWith(p));
}

/**
 * 解析 srcset 字符串，取出其中的 URL（忽略 1x/2x/100w 等描述符）。
 * @param {string} srcset
 * @returns {string[]}
 */
export function parseSrcset(srcset) {
  if (!srcset || typeof srcset !== "string") return [];
  return srcset
    .split(",")
    .map((part) => part.trim().split(/\s+/)[0])
    .filter(Boolean);
}

/** 从 URL 取小写后缀（不带点）；解析失败时退化到去掉 query/hash。 */
function extOf(url) {
  if (!url || typeof url !== "string") return "";
  let path = url;
  try {
    path = new URL(url).pathname;
  } catch {
    path = url.split(/[?#]/)[0];
  }
  const base = path.split("/").pop() || "";
  const dot = base.lastIndexOf(".");
  if (dot < 0) return "";
  return base.slice(dot + 1).toLowerCase();
}

function normalizedContentType(contentType) {
  return (contentType || "").toLowerCase().split(";")[0].trim();
}

function extFromContentType(contentType) {
  return CONTENT_TYPE_EXT[normalizedContentType(contentType)] || "";
}

function filenameExt(filename) {
  if (!filename || typeof filename !== "string") return "";
  const base = filename.split(/[\\/]/).pop() || "";
  const dot = base.lastIndexOf(".");
  if (dot < 0) return "";
  const ext = base.slice(dot + 1).toLowerCase();
  if (!ext || ext.length > 12 || !/^[a-z0-9_]+$/.test(ext)) return "";
  return ext;
}

function filenameWithExt(filename, ext) {
  if (!ext || filenameExt(filename)) return filename;
  const base = filename && filename.trim() ? filename.trim() : "download";
  return `${base}.${ext}`;
}

/** 由 Content-Type 推断类型（"" 表示非媒体）。 */
function kindFromContentType(contentType) {
  const ct = normalizedContentType(contentType);
  if (!ct) return "";
  if (SEGMENT_CONTENT_TYPES.has(ct)) return "segment";
  if (ct === "application/vnd.apple.mpegurl" || ct === "application/x-mpegurl") return "manifest";
  if (ct === "application/dash+xml") return "manifest";
  if (ct.startsWith("image/")) return "image";
  if (ct.startsWith("video/")) return "video";
  if (ct.startsWith("audio/")) return "audio";
  return "";
}

/**
 * 综合后缀与 Content-Type 推断 { kind, ext }（kind 为 "" 表示无法判定）。
 * @param {string} url
 * @param {string} [contentType]
 * @returns {{ kind: string, ext: string }}
 */
export function inferKindAndExt(url, contentType = "") {
  const ext = extOf(url);
  const contentExt = extFromContentType(contentType);
  // 分片优先判定，且压过后缀/Content-Type 的其它结论（如 .ts 会被 video/mp2t 误判为视频）。
  if (SEGMENT_EXT.has(ext) || SEGMENT_CONTENT_TYPES.has(normalizedContentType(contentType))) {
    return { kind: "segment", ext: ext || contentExt };
  }
  let kind = EXT_KIND[ext] || "";
  if (!kind) kind = kindFromContentType(contentType);
  return { kind, ext: ext || contentExt };
}

/**
 * 从 URL 推断文件名（取 path 最后一段并解码）。
 * @param {string} url
 * @param {string} [ext]
 * @returns {string}
 */
export function inferFilename(url, ext = "") {
  let path = url;
  try {
    path = new URL(url).pathname;
  } catch {
    path = String(url).split(/[?#]/)[0];
  }
  let base = path.split("/").pop() || "";
  try {
    base = decodeURIComponent(base);
  } catch {
    /* 保留原始串 */
  }
  return filenameWithExt(base.trim(), ext);
}

/**
 * 判断一个网络响应是否为媒体，返回类型（""=非媒体），供 background 的
 * webRequest 监听匹配。
 * @param {string} url
 * @param {string} [contentType]
 * @returns {""|"image"|"video"|"audio"|"manifest"|"segment"}
 */
export function classifyNetwork(url, contentType = "") {
  return /** @type any */ (inferKindAndExt(url, contentType).kind);
}

/**
 * 是否值得缓存的网络媒体：完整文件或 manifest 才缓存；HLS/DASH 分片
 * （segment）与非媒体一律不缓存，避免几百个分片塞满缓存。
 */
export function isMediaResponse(url, contentType = "") {
  const kind = classifyNetwork(url, contentType);
  return kind !== "" && kind !== "segment";
}

/** 把相对地址按 pageUrl 解析为绝对地址；失败返回 ""。 */
function toAbsolute(raw, pageUrl) {
  if (!raw || typeof raw !== "string") return "";
  const r = raw.trim();
  if (!r) return "";
  try {
    return pageUrl ? new URL(r, pageUrl).href : new URL(r).href;
  } catch {
    return "";
  }
}

/** 展开一个原始项的所有候选 URL（含 srcset），解析为绝对地址并过滤无效。 */
function expandUrls(item, pageUrl) {
  const raws = [];
  if (item.url) raws.push(item.url);
  if (item.srcset) raws.push(...parseSrcset(item.srcset));
  const out = [];
  for (const raw of raws) {
    const abs = toAbsolute(raw, pageUrl);
    if (abs && filterInvalidUrl(abs)) out.push(abs);
  }
  return out;
}

function normalizeKindHint(hint) {
  return MEDIA_KINDS.includes(hint) ? hint : "";
}

const SOURCE_RANK = { network: 3, performance: 2, dom: 1 };

/** 合并同一 URL 的候选：补全缺失字段，source 取更可靠者。 */
function mergeInto(byUrl, c) {
  const prev = byUrl.get(c.url);
  if (!prev) {
    byUrl.set(c.url, c);
    return;
  }
  if (!prev.contentType && c.contentType) prev.contentType = c.contentType;
  if (!prev.ext && c.ext) prev.ext = c.ext;
  if (!filenameExt(prev.filename) && filenameExt(c.filename)) prev.filename = c.filename;
  else if (!filenameExt(prev.filename) && prev.ext) prev.filename = filenameWithExt(prev.filename, prev.ext);
  if (prev.width == null && c.width != null) prev.width = c.width;
  if (prev.height == null && c.height != null) prev.height = c.height;
  if (prev.duration == null && c.duration != null) prev.duration = c.duration;
  if (prev.bytes == null && c.bytes != null) prev.bytes = c.bytes;
  if ((SOURCE_RANK[c.source] || 0) > (SOURCE_RANK[prev.source] || 0)) prev.source = c.source;
}

/** 1×1 / ≤2px 的图片视为追踪像素，过滤掉。 */
function isTrackingPixel(c) {
  return (
    c.kind === "image" &&
    Number.isFinite(c.width) &&
    Number.isFinite(c.height) &&
    c.width <= 2 &&
    c.height <= 2
  );
}

function hasIntrinsicMediaEvidence(item, kind) {
  if (kind === "image") {
    return Number.isFinite(item.width) && item.width > 0
      && Number.isFinite(item.height) && item.height > 0;
  }
  if (kind === "video") {
    return (Number.isFinite(item.width) && item.width > 0
      && Number.isFinite(item.height) && item.height > 0)
      || (Number.isFinite(item.duration) && item.duration > 0);
  }
  if (kind === "audio") {
    return Number.isFinite(item.duration) && item.duration > 0;
  }
  return false;
}

/** 把一个原始项转成 0..n 个 MediaCandidate（srcset 会展开多条）。 */
function itemToCandidates(item, pageUrl, pageTitle) {
  const out = [];
  if (!item) return out;
  const contentType = item.contentType || "";
  const hint = normalizeKindHint(item.kind);
  for (const url of expandUrls(item, pageUrl)) {
    const { kind: inferred, ext } = inferKindAndExt(url, contentType);
    // HLS/DASH 分片直接丢弃：即便 DOM 标签把它当 video，也不让其进入候选列表。
    if (inferred === "segment") continue;
    const kind = hint || inferred;
    if (!kind) continue; // 无法判定类型的资源跳过（如普通脚本/接口）
    if (hint && !inferred && !ext && !contentType && !hasIntrinsicMediaEvidence(item, kind)) {
      continue; // 无扩展/无响应类型/无尺寸或时长证据的 DOM/Performance 资源易误抓接口请求。
    }
    const candidate = {
      url,
      kind,
      ext,
      filename: inferFilename(url, ext),
      contentType,
      pageUrl,
      pageTitle,
      source: item.source || "dom",
    };
    if (Number.isFinite(item.width) && item.width > 0) candidate.width = item.width;
    if (Number.isFinite(item.height) && item.height > 0) candidate.height = item.height;
    if (Number.isFinite(item.duration) && item.duration > 0) candidate.duration = item.duration;
    if (Number.isFinite(item.bytes) && item.bytes > 0) candidate.bytes = item.bytes;
    out.push(candidate);
  }
  return out;
}

/**
 * 把一批已归一化的候选按 URL 去重合并（跨帧/跨来源时使用）。
 * @param {MediaCandidate[]} candidates
 * @returns {MediaCandidate[]}
 */
export function dedupeCandidates(candidates) {
  const byUrl = new Map();
  for (const c of candidates || []) {
    if (!c || !c.url) continue;
    mergeInto(byUrl, c);
  }
  return [...byUrl.values()].filter((c) => !isTrackingPixel(c));
}

/**
 * 把原始采集项归一化为去重、分类后的 MediaCandidate 列表。
 * @param {RawMediaItem[]} rawItems
 * @param {{pageUrl?: string, pageTitle?: string}} [ctx]
 * @returns {MediaCandidate[]}
 */
export function normalizeCandidates(rawItems, ctx = {}) {
  const pageUrl = ctx.pageUrl || "";
  const pageTitle = ctx.pageTitle || "";
  const all = [];
  for (const item of rawItems || []) {
    all.push(...itemToCandidates(item, pageUrl, pageTitle));
  }
  return dedupeCandidates(all);
}
