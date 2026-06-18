import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

function readJson(path) {
  return JSON.parse(readFileSync(join(root, path), "utf8"));
}

function tomlVersion(path, section) {
  const source = readFileSync(join(root, path), "utf8");
  const sectionPattern = section.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(
    `\\[${sectionPattern}\\][\\s\\S]*?^version\\s*=\\s*\"([^\"]+)\"`,
    "m",
  ).exec(source);
  assert.ok(match, `No version found in [${section}] of ${path}`);
  return match[1];
}

const versions = new Map([
  ["apps/rdm-desktop/package.json", readJson("apps/rdm-desktop/package.json").version],
  ["apps/rdm-desktop/src-tauri/tauri.conf.json", readJson("apps/rdm-desktop/src-tauri/tauri.conf.json").version],
  ["Cargo.toml [workspace.package]", tomlVersion("Cargo.toml", "workspace.package")],
  ["apps/rdm-desktop/src-tauri/Cargo.toml [package]", tomlVersion("apps/rdm-desktop/src-tauri/Cargo.toml", "package")],
]);

const unique = new Set(versions.values());
if (unique.size !== 1) {
  for (const [source, version] of versions) console.error(`${source}: ${version}`);
  throw new Error("RDM version declarations are inconsistent");
}

const version = unique.values().next().value;
if (process.env.GITHUB_REF_TYPE === "tag") {
  assert.equal(
    process.env.GITHUB_REF_NAME,
    `v${version}`,
    `Release tag must match manifest version v${version}`,
  );
}

console.log(`RDM version ${version} is consistent across manifests.`);
