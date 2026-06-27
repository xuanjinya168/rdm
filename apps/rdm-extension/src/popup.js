import { pingHealth, postDownload, postMediaCandidates } from "./lib/bridge.js";
import { BRIDGE_BASE_URL, STORAGE_KEYS } from "./lib/config.js";
import { normalizeCandidates, dedupeCandidates } from "./lib/sniffer.js";
import { collectRawMedia } from "./content/media-sniffer.js";

const ALL_URLS = { origins: ["<all_urls>"] };

// --- 设置区元素 ---
const toggle = document.getElementById("intercept-toggle");
const hint = document.getElementById("toggle-hint");
const statusEl = document.getElementById("bridge-status");
const testBtn = document.getElementById("test-btn");
const urlEl = document.getElementById("bridge-url");

// --- 嗅探区元素 ---
const sniffBtn = document.getElementById("sniff-btn");
const netToggle = document.getElementById("net-toggle");
const netHint = document.getElementById("net-hint");
const filtersEl = document.getElementById("filters");
const bulkEl = document.getElementById("bulk");
const selectAllEl = document.getElementById("select-all");
const sendSelectedBtn = document.getElementById("send-selected");
const listEl = document.getElementById("media-list");
const sniffStatus = document.getElementById("sniff-status");

urlEl.textContent = BRIDGE_BASE_URL;

// --- 状态 ---
/** @type {import("./lib/sniffer.js").MediaCandidate[]} */
let candidates = [];
const selected = new Set(); // 选中的 url
let activeFilter = "all";

// ====================== 设置：拦截开关 + 桥状态 ======================

chrome.storage.local.get(STORAGE_KEYS.interceptEnabled).then((stored) => {
  toggle.checked = stored[STORAGE_KEYS.interceptEnabled] === true;
  updateHint();
});

toggle.addEventListener("change", async () => {
  await chrome.storage.local.set({ [STORAGE_KEYS.interceptEnabled]: toggle.checked });
  updateHint();
});

function updateHint() {
  hint.textContent = toggle.checked
    ? "开启：浏览器下载会自动交给 RDM。"
    : "关闭：仅使用右键菜单手动交给 RDM。";
}

function setStatus(state, text) {
  statusEl.classList.remove("ok", "off");
  if (state === "ok") statusEl.classList.add("ok");
  else if (state === "off") statusEl.classList.add("off");
  statusEl.textContent = text;
}

async function runHealthCheck() {
  setStatus("pending", "检测中…");
  const version = await pingHealth();
  if (version) setStatus("ok", `在线 · v${version}`);
  else setStatus("off", "离线（未检测到 RDM）");
}

testBtn.addEventListener("click", runHealthCheck);
runHealthCheck();

// ====================== 媒体嗅探 ======================

// 网络嗅探开关 = 是否已授予 <all_urls>（不默认全局授权）。
chrome.permissions.contains(ALL_URLS).then((granted) => {
  netToggle.checked = granted;
  updateNetHint();
});

netToggle.addEventListener("change", async () => {
  // permissions.request/remove 必须在用户手势内调用——change 事件即用户手势。
  try {
    if (netToggle.checked) {
      netToggle.checked = await chrome.permissions.request(ALL_URLS);
    } else {
      await chrome.permissions.remove(ALL_URLS);
      netToggle.checked = false;
    }
  } catch {
    netToggle.checked = await chrome.permissions.contains(ALL_URLS);
  }
  updateNetHint();
});

function updateNetHint() {
  netHint.textContent = netToggle.checked
    ? "网络嗅探已开启：会捕获当前页加载的视频/音频/m3u8 等网络请求。"
    : "开启网络嗅探需授权访问站点（用于捕获视频/m3u8 等网络请求）。";
}

sniffBtn.addEventListener("click", runSniff);

async function runSniff() {
  sniffBtn.disabled = true;
  setSniffStatus("正在嗅探当前页面…");
  listEl.replaceChildren();
  selected.clear();
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab || tab.id == null) {
      candidates = [];
      setSniffStatus("未找到活动标签页。");
      return;
    }

    // M1/M3：注入采集器到所有可访问帧（含同源 iframe；跨源 iframe 需 <all_urls>）。
    let frameResults;
    try {
      frameResults = await chrome.scripting.executeScript({
        target: { tabId: tab.id, allFrames: true },
        func: collectRawMedia,
      });
    } catch {
      candidates = [];
      render();
      setSniffStatus("无法在此页面嗅探（可能是浏览器内置页或扩展商店页）。");
      return;
    }

    const all = [];
    let top = null;
    for (const r of frameResults) {
      const res = r && r.result;
      if (!res) continue;
      if (!top) top = res; // 顶层帧最先返回
      all.push(...normalizeCandidates(res.items, { pageUrl: res.pageUrl, pageTitle: res.pageTitle }));
    }

    // M2：合并网络嗅探缓存（仅在已授予 <all_urls> 时有数据）。
    if (await chrome.permissions.contains(ALL_URLS)) {
      const resp = await chrome.runtime
        .sendMessage({ type: "getNetworkMedia", tabId: tab.id })
        .catch(() => null);
      const netItems = (resp && resp.items ? resp.items : []).map((e) => ({
        url: e.url,
        contentType: e.contentType || "",
        bytes: e.contentLength,
        source: "network",
      }));
      const ctx = {
        pageUrl: top ? top.pageUrl : tab.url || "",
        pageTitle: top ? top.pageTitle : tab.title || "",
      };
      all.push(...normalizeCandidates(netItems, ctx));
    }

    candidates = dedupeCandidates(all).sort(byKindThenName);
    render();
  } finally {
    sniffBtn.disabled = false;
  }
}

const KIND_ORDER = { video: 0, audio: 1, manifest: 2, image: 3 };
function byKindThenName(a, b) {
  const ka = KIND_ORDER[a.kind] ?? 9;
  const kb = KIND_ORDER[b.kind] ?? 9;
  if (ka !== kb) return ka - kb;
  return (a.filename || a.url).localeCompare(b.filename || b.url);
}

const KIND_LABEL = { image: "图片", video: "视频", audio: "音频", manifest: "流媒体" };
const SOURCE_LABEL = { dom: "页面", network: "网络", performance: "已加载" };

function isDownloadable(c) {
  if (c.kind !== "manifest") return true;
  return (c.ext || "").toLowerCase() === "m3u8";
}

function counts() {
  const c = { all: candidates.length, image: 0, video: 0, audio: 0, manifest: 0 };
  for (const x of candidates) c[x.kind] = (c[x.kind] || 0) + 1;
  return c;
}

function visibleCandidates() {
  return activeFilter === "all"
    ? candidates
    : candidates.filter((c) => c.kind === activeFilter);
}

function render() {
  const c = counts();
  for (const span of filtersEl.querySelectorAll(".chip-n")) {
    span.textContent = String(c[span.dataset.count] || 0);
  }
  const has = candidates.length > 0;
  filtersEl.hidden = !has;
  bulkEl.hidden = !has;

  listEl.replaceChildren();
  const vis = visibleCandidates();
  if (!has) {
    setSniffStatus("未发现可下载的媒体资源。");
  } else if (vis.length === 0) {
    setSniffStatus(`共 ${candidates.length} 项，当前筛选无匹配。`);
  } else {
    setSniffStatus(
      `共 ${candidates.length} 项${activeFilter === "all" ? "" : `，显示 ${vis.length} 项`}。`,
    );
    for (const cand of vis) listEl.appendChild(renderCard(cand));
  }
  updateBulk();
}

function renderCard(cand) {
  const card = document.createElement("div");
  card.className = "media-card";

  const check = document.createElement("input");
  check.type = "checkbox";
  check.className = "card-check";
  check.checked = selected.has(cand.url);
  check.addEventListener("change", () => {
    if (check.checked) selected.add(cand.url);
    else selected.delete(cand.url);
    updateBulk();
  });

  const body = document.createElement("div");
  body.className = "card-body";

  const topRow = document.createElement("div");
  topRow.className = "card-top";
  const badge = document.createElement("span");
  badge.className = `badge badge-${cand.kind}`;
  badge.textContent = KIND_LABEL[cand.kind] || cand.kind;
  const name = document.createElement("span");
  name.className = "card-name";
  name.textContent = cand.filename || cand.url;
  name.title = cand.url;
  topRow.append(badge, name);

  const meta = document.createElement("div");
  meta.className = "card-meta";
  meta.textContent = metaText(cand);

  const urlLine = document.createElement("div");
  urlLine.className = "card-url";
  urlLine.textContent = cand.url;

  body.append(topRow, meta, urlLine);

  const send = document.createElement("button");
  send.type = "button";
  send.className = "btn btn-send";
  if (!isDownloadable(cand)) {
    send.disabled = true;
    send.textContent = "仅识别";
    send.title = "DASH（mpd）暂不支持下载";
  } else {
    send.textContent = "发送";
    send.addEventListener("click", () => sendOne(cand, send));
  }

  card.append(check, body, send);
  return card;
}

function metaText(c) {
  const parts = [];
  if (c.width && c.height) parts.push(`${c.width}×${c.height}`);
  if (c.duration) parts.push(formatDuration(c.duration));
  if (c.bytes) parts.push(formatBytes(c.bytes));
  if (c.ext) parts.push(c.ext.toUpperCase());
  parts.push(`来源:${SOURCE_LABEL[c.source] || c.source}`);
  return parts.join(" · ");
}

function formatDuration(sec) {
  const s = Math.round(sec);
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

function updateBulk() {
  const vis = visibleCandidates();
  const selVisible = vis.filter((c) => selected.has(c.url)).length;
  selectAllEl.checked = vis.length > 0 && selVisible === vis.length;
  selectAllEl.indeterminate = selVisible > 0 && selVisible < vis.length;
  const n = selected.size;
  sendSelectedBtn.textContent = n > 0 ? `全部发送至 RDM（${n}）` : "全部发送至 RDM";
  sendSelectedBtn.disabled = n === 0;
}

selectAllEl.addEventListener("change", () => {
  const vis = visibleCandidates();
  if (selectAllEl.checked) for (const c of vis) selected.add(c.url);
  else for (const c of vis) selected.delete(c.url);
  render();
});

filtersEl.addEventListener("click", (e) => {
  const chip = e.target.closest(".chip");
  if (!chip) return;
  activeFilter = chip.dataset.kind;
  for (const ch of filtersEl.querySelectorAll(".chip")) {
    ch.classList.toggle("is-active", ch === chip);
  }
  render();
});

async function sendOne(cand, btn) {
  btn.disabled = true;
  const prev = btn.textContent;
  btn.textContent = "发送中…";
  const res = await postDownload(cand.url, {
    ...(cand.filename ? { filename: cand.filename } : {}),
    ...(cand.pageUrl ? { referrer: cand.pageUrl } : {}),
  });
  if (res.ok) {
    btn.textContent = "已发送";
  } else {
    btn.textContent = "失败";
    btn.disabled = false;
    setSniffStatus(`发送失败：${res.error || "桌面端未运行？"}`);
    setTimeout(() => {
      btn.textContent = prev;
    }, 1500);
  }
}

sendSelectedBtn.addEventListener("click", async () => {
  const targets = candidates.filter((c) => selected.has(c.url) && isDownloadable(c));
  const skipped = selected.size - targets.length;
  if (targets.length === 0) {
    setSniffStatus("所选项目暂不支持下载。");
    return;
  }
  sendSelectedBtn.disabled = true;
  setSniffStatus(`正在发送 ${targets.length} 项…`);
  const payload = targets.map((c) => ({
    url: c.url,
    filename: c.filename,
    kind: c.kind,
    ext: c.ext,
    width: c.width,
    height: c.height,
    duration: c.duration,
    bytes: c.bytes,
    pageUrl: c.pageUrl,
  }));
  const meta = { pageUrl: targets[0].pageUrl, pageTitle: targets[0].pageTitle };
  const res = await postMediaCandidates(payload, meta);
  if (res.ok) {
    setSniffStatus(
      `已发送 ${targets.length} 项到 RDM 确认${skipped ? `，跳过暂不支持项 ${skipped}` : ""}。`,
    );
  } else {
    setSniffStatus(`发送失败：${res.error || "桌面端未运行？"}`);
  }
  sendSelectedBtn.disabled = false;
});

function setSniffStatus(text) {
  sniffStatus.textContent = text || "";
}
