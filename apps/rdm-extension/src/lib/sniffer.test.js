import test from "node:test";
import assert from "node:assert/strict";

import {
  filterInvalidUrl,
  parseSrcset,
  inferKindAndExt,
  inferFilename,
  classifyNetwork,
  isMediaResponse,
  normalizeCandidates,
  dedupeCandidates,
} from "./sniffer.js";

const PAGE = "https://example.com/posts/123";

test("filterInvalidUrl rejects data/blob/javascript/#/about/mailto and empty", () => {
  for (const bad of [
    "",
    "   ",
    "data:image/png;base64,AAAA",
    "blob:https://example.com/abc",
    "javascript:alert(1)",
    "#anchor",
    "about:blank",
    "mailto:a@b.com",
  ]) {
    assert.equal(filterInvalidUrl(bad), false, bad);
  }
  assert.equal(filterInvalidUrl("https://example.com/a.mp4"), true);
  assert.equal(filterInvalidUrl("http://example.com/a.jpg"), true);
});

test("parseSrcset extracts URLs and ignores descriptors", () => {
  assert.deepEqual(parseSrcset("a.jpg 1x, b.jpg 2x"), ["a.jpg", "b.jpg"]);
  assert.deepEqual(parseSrcset("a.jpg 100w, b.jpg 200w"), ["a.jpg", "b.jpg"]);
  assert.deepEqual(parseSrcset("only.jpg"), ["only.jpg"]);
  assert.deepEqual(parseSrcset(""), []);
  assert.deepEqual(parseSrcset(null), []);
});

test("inferKindAndExt classifies by extension", () => {
  assert.deepEqual(inferKindAndExt("https://x.com/a.mp4"), { kind: "video", ext: "mp4" });
  assert.deepEqual(inferKindAndExt("https://x.com/a.JPG"), { kind: "image", ext: "jpg" });
  assert.deepEqual(inferKindAndExt("https://x.com/a.mp3?t=1"), { kind: "audio", ext: "mp3" });
  assert.deepEqual(inferKindAndExt("https://x.com/play.m3u8"), { kind: "manifest", ext: "m3u8" });
  assert.deepEqual(inferKindAndExt("https://x.com/play.mpd"), { kind: "manifest", ext: "mpd" });
});

test("inferKindAndExt falls back to Content-Type when extension unknown", () => {
  assert.deepEqual(inferKindAndExt("https://x.com/stream", "video/mp4"), { kind: "video", ext: "mp4" });
  assert.deepEqual(inferKindAndExt("https://x.com/a", "application/vnd.apple.mpegurl"), { kind: "manifest", ext: "m3u8" });
  assert.equal(inferKindAndExt("https://x.com/page.html", "text/html").kind, "");
});

test("inferFilename takes the last path segment, decoded, without query", () => {
  assert.equal(inferFilename("https://x.com/a/b/movie.mp4?token=1"), "movie.mp4");
  assert.equal(inferFilename("https://x.com/%E5%9B%BE.png"), "图.png");
  assert.equal(inferFilename("https://x.com/dir/"), "");
  assert.equal(
    inferFilename("https://img1.baidu.com/it/u=910271416,2899451860&fm=253&fmt=auto&app=138&f=JPEG?w=751&h=500", "jpg"),
    "u=910271416,2899451860&fm=253&fmt=auto&app=138&f=JPEG.jpg",
  );
  assert.equal(inferFilename("https://x.com/dir/", "png"), "download.png");
});

test("classifyNetwork / isMediaResponse", () => {
  assert.equal(classifyNetwork("https://x.com/a.png", "image/png"), "image");
  assert.equal(classifyNetwork("https://x.com/seg", "application/dash+xml"), "manifest");
  assert.equal(classifyNetwork("https://x.com/app.js", "application/javascript"), "");
  assert.equal(isMediaResponse("https://x.com/a.webm", ""), true);
  assert.equal(isMediaResponse("https://x.com/x.json", "application/json"), false);
});

test("classifies HLS/DASH segments as 'segment', not video", () => {
  assert.equal(inferKindAndExt("https://x.com/seg00012.ts").kind, "segment");
  assert.equal(inferKindAndExt("https://x.com/chunk.m4s").kind, "segment");
  assert.equal(inferKindAndExt("https://x.com/v.cmfv").kind, "segment");
  assert.equal(inferKindAndExt("https://x.com/a.cmfa").kind, "segment");
  // 即便 Content-Type 是 video/mp2t，也不能当成可下载视频。
  assert.equal(inferKindAndExt("https://x.com/seg", "video/mp2t").kind, "segment");
  assert.equal(classifyNetwork("https://x.com/seg.ts", "video/mp2t"), "segment");
});

test("segments are not cached nor downloadable; .m3u8 manifest still is", () => {
  // 缓存层：manifest 缓存，分片不缓存。
  assert.equal(isMediaResponse("https://x.com/play.m3u8", "application/vnd.apple.mpegurl"), true);
  assert.equal(isMediaResponse("https://x.com/seg001.ts", "video/mp2t"), false);
  assert.equal(isMediaResponse("https://x.com/chunk.m4s", "video/iso.segment"), false);

  // 候选层：.m3u8 进列表（manifest），.ts/.m4s 一律不进，哪怕 DOM 标成 video。
  const out = normalizeCandidates(
    [
      { url: "https://x.com/movie.mp4", contentType: "video/mp4", source: "network" },
      { url: "https://x.com/play.m3u8", source: "network" },
      { url: "https://x.com/seg001.ts", contentType: "video/mp2t", source: "network" },
      { url: "https://x.com/chunk.m4s", source: "network" },
      { url: "https://x.com/seg002.ts", kind: "video", source: "dom" },
    ],
    { pageUrl: PAGE },
  );
  const kinds = out.map((c) => `${c.kind}:${c.filename}`).sort();
  assert.deepEqual(kinds, ["manifest:play.m3u8", "video:movie.mp4"]);
});

test("normalizeCandidates resolves relative URLs against pageUrl", () => {
  const out = normalizeCandidates(
    [{ url: "/img/a.png", kind: "image", source: "dom" }],
    { pageUrl: PAGE, pageTitle: "T" },
  );
  assert.equal(out.length, 1);
  assert.equal(out[0].url, "https://example.com/img/a.png");
  assert.equal(out[0].kind, "image");
  assert.equal(out[0].filename, "a.png");
  assert.equal(out[0].pageUrl, PAGE);
  assert.equal(out[0].pageTitle, "T");
});

test("normalizeCandidates expands srcset into multiple candidates", () => {
  const out = normalizeCandidates(
    [{ srcset: "a.jpg 1x, sub/b.jpg 2x", kind: "image", source: "dom" }],
    { pageUrl: PAGE },
  );
  const urls = out.map((c) => c.url).sort();
  assert.deepEqual(urls, [
    "https://example.com/posts/a.jpg",
    "https://example.com/posts/sub/b.jpg",
  ]);
});

test("normalizeCandidates drops tracking pixels (<=2px images)", () => {
  const out = normalizeCandidates(
    [{ url: "https://x.com/t.gif", width: 1, height: 1, kind: "image", source: "dom" }],
    {},
  );
  assert.equal(out.length, 0);
});

test("normalizeCandidates honors kind hint when extension is unknown", () => {
  const out = normalizeCandidates(
    [{ url: "https://x.com/live/stream", kind: "video", source: "dom", width: 1280, height: 720 }],
    {},
  );
  assert.equal(out.length, 1);
  assert.equal(out[0].kind, "video");
  assert.equal(out[0].ext, "");
});

test("normalizeCandidates skips extensionless hinted resource without media evidence", () => {
  const out = normalizeCandidates(
    [{ url: "https://h2tcbox.baidu.com/ztbox?action=zpblog", kind: "image", source: "performance" }],
    {},
  );
  assert.equal(out.length, 0);
});

test("normalizeCandidates skips items with no determinable kind", () => {
  const out = normalizeCandidates(
    [{ url: "https://x.com/api/data", source: "performance" }],
    {},
  );
  assert.equal(out.length, 0);
});

test("normalizeCandidates classifies network items by Content-Type", () => {
  const out = normalizeCandidates(
    [{ url: "https://x.com/v?id=9", contentType: "video/mp4", bytes: 1024, source: "network" }],
    { pageUrl: PAGE },
  );
  assert.equal(out.length, 1);
  assert.equal(out[0].kind, "video");
  assert.equal(out[0].ext, "mp4");
  assert.equal(out[0].filename, "v.mp4");
  assert.equal(out[0].bytes, 1024);
  assert.equal(out[0].source, "network");
});

test("dedupeCandidates upgrades extensionless DOM filename with network content type", () => {
  const merged = dedupeCandidates([
    { url: "https://x.com/media/u=123&f=JPEG", kind: "image", ext: "", filename: "u=123&f=JPEG", contentType: "", pageUrl: PAGE, pageTitle: "", source: "dom" },
    { url: "https://x.com/media/u=123&f=JPEG", kind: "image", ext: "jpg", filename: "u=123&f=JPEG.jpg", contentType: "image/jpeg", pageUrl: PAGE, pageTitle: "", source: "network", bytes: 5000 },
  ]);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].ext, "jpg");
  assert.equal(merged[0].filename, "u=123&f=JPEG.jpg");
});

test("normalizeCandidates filters invalid (data/blob) URLs", () => {
  const out = normalizeCandidates(
    [
      { url: "data:image/png;base64,AAAA", kind: "image", source: "dom" },
      { url: "blob:https://x.com/abc", kind: "video", source: "dom" },
    ],
    {},
  );
  assert.equal(out.length, 0);
});

test("dedupeCandidates merges same URL and prefers richer source", () => {
  const merged = dedupeCandidates([
    { url: "https://x.com/a.mp4", kind: "video", ext: "mp4", filename: "a.mp4", contentType: "", pageUrl: PAGE, pageTitle: "", source: "dom", width: 1920, height: 1080 },
    { url: "https://x.com/a.mp4", kind: "video", ext: "mp4", filename: "a.mp4", contentType: "video/mp4", pageUrl: PAGE, pageTitle: "", source: "network", bytes: 5000 },
  ]);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].width, 1920);
  assert.equal(merged[0].contentType, "video/mp4");
  assert.equal(merged[0].bytes, 5000);
  assert.equal(merged[0].source, "network"); // network > dom
});
