import test from "node:test";
import assert from "node:assert/strict";

import {
  addToStore,
  readTab,
  pruneStore,
  clearTabInStore,
  NET_MEDIA_TTL_MS,
} from "./cache.js";

const T0 = 1_000_000;
const TTL = NET_MEDIA_TTL_MS;

test("addToStore isolates entries per tab", () => {
  const store = {};
  addToStore(store, 1, { url: "https://a.com/1.mp4", contentType: "video/mp4" }, T0);
  addToStore(store, 2, { url: "https://b.com/2.mp3", contentType: "audio/mpeg" }, T0);
  assert.deepEqual(
    readTab(store, 1, T0, TTL).map((e) => e.url),
    ["https://a.com/1.mp4"],
  );
  assert.deepEqual(
    readTab(store, 2, T0, TTL).map((e) => e.url),
    ["https://b.com/2.mp3"],
  );
  assert.deepEqual(readTab(store, 99, T0, TTL), []);
});

test("addToStore dedupes same URL and fills missing fields", () => {
  const store = {};
  addToStore(store, 1, { url: "https://a.com/x.mp4", contentType: "video/mp4" }, T0);
  addToStore(store, 1, { url: "https://a.com/x.mp4", contentLength: 4096 }, T0 + 5);
  const items = readTab(store, 1, T0 + 5, TTL);
  assert.equal(items.length, 1);
  assert.equal(items[0].contentType, "video/mp4");
  assert.equal(items[0].contentLength, 4096);
});

test("addToStore ignores invalid tabId and entries without url", () => {
  const store = {};
  addToStore(store, -1, { url: "https://a.com/x.mp4" }, T0);
  addToStore(store, 1, { contentType: "video/mp4" }, T0);
  addToStore(store, 1, null, T0);
  assert.deepEqual(readTab(store, -1, T0, TTL), []);
  assert.deepEqual(readTab(store, 1, T0, TTL), []);
});

test("readTab hides expired entries and strips internal ts", () => {
  const store = {};
  addToStore(store, 1, { url: "https://a.com/x.mp4" }, T0);
  // 边界：恰好 TTL 仍在；超过 TTL 即过期。
  assert.equal(readTab(store, 1, T0 + TTL, TTL).length, 1);
  assert.equal(readTab(store, 1, T0 + TTL + 1, TTL).length, 0);
  const [item] = readTab(store, 1, T0, TTL);
  assert.equal(item.ts, undefined);
  assert.equal(item.url, "https://a.com/x.mp4");
});

test("pruneStore drops expired entries and empties tabs", () => {
  const store = {};
  addToStore(store, 1, { url: "https://a.com/old.mp4" }, T0);
  addToStore(store, 1, { url: "https://a.com/new.mp4" }, T0 + TTL);
  addToStore(store, 2, { url: "https://b.com/old.mp4" }, T0);

  pruneStore(store, T0 + TTL + 1, TTL);
  assert.deepEqual(
    readTab(store, 1, T0 + TTL + 1, TTL).map((e) => e.url),
    ["https://a.com/new.mp4"],
  );
  // tab 2 全部过期 → 整个 tab 被移除
  assert.equal(Object.prototype.hasOwnProperty.call(store, "2"), false);
});

test("clearTabInStore removes one tab (navigation/close), keeps others", () => {
  const store = {};
  addToStore(store, 1, { url: "https://a.com/1.mp4" }, T0);
  addToStore(store, 2, { url: "https://b.com/2.mp4" }, T0);
  clearTabInStore(store, 1);
  assert.deepEqual(readTab(store, 1, T0, TTL), []);
  assert.equal(readTab(store, 2, T0, TTL).length, 1);
});
