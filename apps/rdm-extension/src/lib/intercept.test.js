import test from "node:test";
import assert from "node:assert/strict";

import { shouldIntercept } from "./intercept.js";
import { MIN_INTERCEPT_BYTES, DEFAULT_SETTINGS } from "./config.js";

const ON = { interceptEnabled: true };
const OFF = { interceptEnabled: false };

test("auto-intercept is disabled by default (matches README and popup)", () => {
  assert.equal(DEFAULT_SETTINGS.interceptEnabled, false);
});

function item(overrides = {}) {
  return { url: "https://example.com/file.zip", filename: "file.zip", ...overrides };
}

test("intercepts a normal http download when enabled", () => {
  assert.equal(shouldIntercept(item(), ON), true);
});

test("does not intercept when the master switch is off", () => {
  assert.equal(shouldIntercept(item(), OFF), false);
});

test("does not intercept when settings are missing", () => {
  assert.equal(shouldIntercept(item(), undefined), false);
  assert.equal(shouldIntercept(item(), {}), false);
});

test("rejects non-http schemes", () => {
  for (const url of [
    "data:text/plain,hello",
    "blob:https://example.com/abc",
    "ftp://example.com/file",
    "javascript:alert(1)",
    "about:blank",
  ]) {
    assert.equal(shouldIntercept(item({ url }), ON), false, url);
  }
});

test("prefers finalUrl over url", () => {
  const i = item({ url: "https://cdn.example.com/redirect", finalUrl: "https://real.example.com/data.bin" });
  assert.equal(shouldIntercept(i, ON), true);
});

test("skips files smaller than the minimum size", () => {
  assert.equal(
    shouldIntercept(item({ fileSize: MIN_INTERCEPT_BYTES - 1 }), ON),
    false,
  );
  assert.equal(shouldIntercept(item({ fileSize: MIN_INTERCEPT_BYTES }), ON), true);
});

test("uses totalBytes when fileSize is absent", () => {
  assert.equal(shouldIntercept(item({ totalBytes: 10 }), ON), false);
});

test("ignores size checks when size is unknown", () => {
  // 浏览器有时在创建时尚未知大小，不应因此放行。
  assert.equal(shouldIntercept(item({ fileSize: 0, totalBytes: 0 }), ON), true);
  assert.equal(shouldIntercept(item({ fileSize: -1 }), ON), true);
});

test("skips webpage and asset extensions", () => {
  for (const filename of ["page.html", "style.css", "app.js", "data.json", "logo.svg", "font.woff2"]) {
    assert.equal(shouldIntercept(item({ filename }), ON), false, filename);
  }
});

test("intercepts archives and media", () => {
  for (const filename of ["movie.mp4", "setup.exe", "archive.zip", "data.iso", "book.pdf"]) {
    assert.equal(shouldIntercept(item({ filename }), ON), true, filename);
  }
});

test("derives extension from url when filename has none", () => {
  assert.equal(shouldIntercept(item({ filename: "", url: "https://example.com/style.css" }), ON), false);
  assert.equal(shouldIntercept(item({ filename: "", url: "https://example.com/big.iso" }), ON), true);
});

test("handles missing url gracefully", () => {
  assert.equal(shouldIntercept({ filename: "a.zip" }, ON), false);
  assert.equal(shouldIntercept({ url: "" }, ON), false);
});

test("returns false for null item", () => {
  assert.equal(shouldIntercept(null, ON), false);
});
