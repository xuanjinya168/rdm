import assert from "node:assert/strict";
import test from "node:test";

import { mergeTaskSnapshots } from "./tasks.js";

test("buffered updates replace older initial snapshots", () => {
  const initial = [{ id: "a", status: "downloading", updated_at: 10 }];
  const updates = [{ task: { id: "a", status: "completed", updated_at: 12 } }];

  assert.deepEqual(mergeTaskSnapshots(initial, updates), [
    { id: "a", status: "completed", updated_at: 12 },
  ]);
});

test("older buffered updates cannot overwrite newer initial snapshots", () => {
  const initial = [{ id: "a", status: "completed", updated_at: 12 }];
  const updates = [
    { task: { id: "a", status: "downloading", updated_at: 10 } },
    { task: { id: "b", status: "queued", updated_at: 11 } },
  ];

  assert.deepEqual(mergeTaskSnapshots(initial, updates), [
    { id: "a", status: "completed", updated_at: 12 },
    { id: "b", status: "queued", updated_at: 11 },
  ]);
});
