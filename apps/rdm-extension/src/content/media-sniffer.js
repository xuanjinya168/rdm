// 注入到页面执行的「原始媒体采集器」。
//
// 关键约束：collectRawMedia 必须**自包含**——只引用页面全局 API（document /
// location / performance / MutationObserver / Promise / Set / setTimeout），
// 不 import 任何模块、不引用模块级标识符。因为它经
// chrome.scripting.executeScript({ func: collectRawMedia }) 被 toString() 序列化
// 注入页面，外部作用域不会随之过去（仓库无打包步骤，源码即注入内容）。
//
// 只负责 DOM/Performance 原始读取，返回纯可序列化对象；归一化/去重/分类由扩展
// 侧的 ../lib/sniffer.js 完成。返回 Promise：先即时扫描，再用 MutationObserver
// 观察一小段时间窗，捕获懒加载/动态插入的媒体后 resolve。

export function collectRawMedia() {
  const OBSERVE_MS = 1500; // 动态新增的观察时间窗上限
  const RESCAN_THROTTLE_MS = 250;
  const items = [];
  const seen = new Set();

  const num = (v) => (typeof v === "number" && isFinite(v) && v > 0 ? v : undefined);

  const add = (entry) => {
    if (!entry || (!entry.url && !entry.srcset)) return;
    const key = entry.source + "|" + (entry.url || "") + "|" + (entry.srcset || "");
    if (seen.has(key)) return;
    seen.add(key);
    items.push(entry);
  };

  const scanDoc = (doc) => {
    if (!doc) return;
    let nodes;
    try {
      nodes = doc.querySelectorAll(
        "img, picture source, video, video source, audio, audio source",
      );
    } catch {
      return;
    }
    for (const el of nodes) {
      const tag = el.tagName ? el.tagName.toLowerCase() : "";
      const parentTag =
        el.parentElement && el.parentElement.tagName
          ? el.parentElement.tagName.toLowerCase()
          : "";
      let kind = "";
      if (tag === "img") kind = "image";
      else if (tag === "video") kind = "video";
      else if (tag === "audio") kind = "audio";
      else if (tag === "source") {
        if (parentTag === "video") kind = "video";
        else if (parentTag === "audio") kind = "audio";
        else kind = "image"; // <picture><source>
      }
      const url = el.currentSrc || el.src || el.getAttribute("src") || "";
      const srcset = el.srcset || el.getAttribute("srcset") || "";
      const entry = { url, srcset, kind, source: "dom" };
      if (tag === "img") {
        entry.width = num(el.naturalWidth);
        entry.height = num(el.naturalHeight);
      } else if (tag === "video") {
        entry.width = num(el.videoWidth);
        entry.height = num(el.videoHeight);
        entry.duration = num(el.duration);
      } else if (tag === "audio") {
        entry.duration = num(el.duration);
      }
      add(entry);
    }
    // 递归同源 iframe；跨源访问 contentDocument 抛 SecurityError，交给
    // executeScript 的 allFrames 注入处理。
    let frames;
    try {
      frames = doc.querySelectorAll("iframe, frame");
    } catch {
      frames = [];
    }
    for (const f of frames) {
      let childDoc = null;
      try {
        childDoc = f.contentDocument;
      } catch {
        childDoc = null;
      }
      if (childDoc) scanDoc(childDoc);
    }
  };

  const scanPerformance = () => {
    let entries = [];
    try {
      entries = performance.getEntriesByType("resource") || [];
    } catch {
      entries = [];
    }
    for (const e of entries) {
      const it = e.initiatorType;
      if (it === "img" || it === "image") add({ url: e.name, kind: "image", source: "performance" });
      else if (it === "video") add({ url: e.name, kind: "video", source: "performance" });
      else if (it === "audio") add({ url: e.name, kind: "audio", source: "performance" });
      else if (it === "fetch" || it === "xmlhttprequest" || it === "other") {
        // 类型未知：交给扩展侧按后缀判断，非媒体会被丢弃。
        add({ url: e.name, kind: "", source: "performance" });
      }
    }
  };

  scanDoc(document);
  scanPerformance();

  return new Promise((resolve) => {
    let observer = null;
    let pending = false;
    const finish = () => {
      if (observer) {
        try {
          observer.disconnect();
        } catch {
          /* ignore */
        }
      }
      resolve({
        pageUrl: location.href,
        pageTitle: document.title || "",
        items,
      });
    };
    try {
      observer = new MutationObserver(() => {
        if (pending) return;
        pending = true;
        setTimeout(() => {
          pending = false;
          scanDoc(document);
        }, RESCAN_THROTTLE_MS);
      });
      observer.observe(document.documentElement || document, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["src", "srcset"],
      });
    } catch {
      observer = null;
    }
    setTimeout(finish, OBSERVE_MS);
  });
}
