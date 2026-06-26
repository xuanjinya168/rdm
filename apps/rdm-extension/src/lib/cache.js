// 网络嗅探缓存：按 tabId 隔离、按 url 去重、带 TTL。
//
// 持久化到 chrome.storage.session：随浏览器会话存活，跨 MV3 service worker
// 重启不丢（旧版用内存 Map，SW 被回收即清零）；session 存储不落盘，符合
// 「不留痕」的隐私取向。
//
// 纯逻辑（addToStore/readTab/pruneStore/clearTabInStore）不依赖 chrome.*，
// 可用 node --test 单测；下方 tabMediaCache 是基于 chrome.storage.session 的
// 异步封装，所有操作经一个 Promise 队列串行化，避免 webRequest 高频事件下的
// 读改写竞态。

// 缓存存活时间：超过则视为过期（导航通常已清理，这是兜底）。取 20 分钟（10~30 区间）。
export const NET_MEDIA_TTL_MS = 20 * 60 * 1000;

const STORAGE_KEY = "netMediaCache";

// store 形状：{ [tabId]: { [url]: { url, contentType, contentLength?, ts } } }

/**
 * 合并一条记录到 store（按 url 去重、补全字段、刷新 ts）。原地修改并返回 store。
 * @param {object} store
 * @param {number} tabId
 * @param {{url: string, contentType?: string, contentLength?: number}} entry
 * @param {number} now - 时间戳（Date.now()）
 */
export function addToStore(store, tabId, entry, now) {
  if (tabId == null || tabId < 0 || !entry || !entry.url) return store;
  const key = String(tabId);
  const tab = store[key] || (store[key] = {});
  const prev = tab[entry.url];
  if (prev) {
    if (!prev.contentType && entry.contentType) prev.contentType = entry.contentType;
    if (prev.contentLength == null && entry.contentLength != null) {
      prev.contentLength = entry.contentLength;
    }
    prev.ts = now;
  } else {
    tab[entry.url] = {
      url: entry.url,
      contentType: entry.contentType || "",
      ...(entry.contentLength != null ? { contentLength: entry.contentLength } : {}),
      ts: now,
    };
  }
  return store;
}

/**
 * 读取某 tab 未过期的记录（去掉内部 ts 字段），不修改 store。
 * @returns {Array<{url: string, contentType: string, contentLength?: number}>}
 */
export function readTab(store, tabId, now, ttlMs) {
  const tab = store[String(tabId)];
  if (!tab) return [];
  const out = [];
  for (const rec of Object.values(tab)) {
    if (now - rec.ts > ttlMs) continue;
    const { ts, ...rest } = rec;
    out.push(rest);
  }
  return out;
}

/** 删除所有过期记录与因此变空的 tab。原地修改并返回 store。 */
export function pruneStore(store, now, ttlMs) {
  for (const [key, tab] of Object.entries(store)) {
    for (const [url, rec] of Object.entries(tab)) {
      if (now - rec.ts > ttlMs) delete tab[url];
    }
    if (Object.keys(tab).length === 0) delete store[key];
  }
  return store;
}

/** 清掉某 tab（导航/关闭时）。原地修改并返回 store。 */
export function clearTabInStore(store, tabId) {
  delete store[String(tabId)];
  return store;
}

// --- chrome.storage.session 异步封装（操作串行化） -------------------------

let chain = Promise.resolve();
/** 把一个异步任务排入串行队列，保证读改写原子，且不让异常断裂队列。 */
function enqueue(task) {
  const run = chain.then(task, task);
  chain = run.then(
    () => {},
    () => {},
  );
  return run;
}

function hasSession() {
  return typeof chrome !== "undefined" && chrome.storage && chrome.storage.session;
}

async function loadStore() {
  if (!hasSession()) return {};
  const got = await chrome.storage.session.get(STORAGE_KEY);
  return (got && got[STORAGE_KEY]) || {};
}

async function saveStore(store) {
  if (!hasSession()) return;
  await chrome.storage.session.set({ [STORAGE_KEY]: store });
}

export const tabMediaCache = {
  /** 记录一条网络媒体（异步写入；调用方可忽略返回值）。 */
  add(tabId, entry) {
    return enqueue(async () => {
      const now = Date.now();
      const store = pruneStore(await loadStore(), now, NET_MEDIA_TTL_MS);
      addToStore(store, tabId, entry, now);
      await saveStore(store);
    });
  },

  /** 取某 tab 未过期的记录（顺手清理过期项）。 */
  get(tabId) {
    return enqueue(async () => {
      const now = Date.now();
      const store = await loadStore();
      const items = readTab(store, tabId, now, NET_MEDIA_TTL_MS);
      await saveStore(pruneStore(store, now, NET_MEDIA_TTL_MS));
      return items;
    });
  },

  /** 清掉某 tab 的缓存。 */
  clear(tabId) {
    return enqueue(async () => {
      const store = clearTabInStore(await loadStore(), tabId);
      await saveStore(store);
    });
  },

  /** 清空全部缓存。 */
  clearAll() {
    return enqueue(async () => {
      if (hasSession()) await chrome.storage.session.remove(STORAGE_KEY);
    });
  },
};
