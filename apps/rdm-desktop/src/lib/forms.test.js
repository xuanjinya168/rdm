import assert from "node:assert/strict";
import test from "node:test";

import { validateDownloadForm, validateSettingsForm } from "./forms.js";

const validDownload = {
  url: " https://example.com/archive.zip ",
  destination: " C:\\Downloads ",
  filename: " archive.zip ",
  connections: "8",
  sha256: "",
};

const validSettings = {
  downloadDir: " C:\\Downloads ",
  maxActive: "3",
  connections: "8",
  retry: "4",
  speedKb: "1024",
  clipboard: true,
  tray: false,
  hlsTranscode: false,
  theme: "dark",
  proxyEnabled: false,
  proxyUrl: "",
  proxyUsername: "",
  proxyPassword: "",
};

test("download form normalizes a valid request", () => {
  assert.deepEqual(validateDownloadForm(validDownload), {
    value: {
      url: "https://example.com/archive.zip",
      destination: "C:\\Downloads",
      connections: 8,
      filename: "archive.zip",
      sha256: "",
    },
  });
});

test("download form rejects unsupported URLs and Windows reserved names", () => {
  assert.match(validateDownloadForm({ ...validDownload, url: "ftp://example.com/a" }).error, /HTTP/);
  assert.match(validateDownloadForm({ ...validDownload, filename: "NUL.txt" }).error, /Windows/);
});

test("download form normalizes SHA-256 and rejects malformed values", () => {
  const checksum = "AB".repeat(32);
  assert.equal(validateDownloadForm({ ...validDownload, sha256: checksum }).value.sha256, checksum.toLowerCase());
  assert.match(validateDownloadForm({ ...validDownload, sha256: "abc" }).error, /64/);
});

test("settings form maps UI units to persisted settings", () => {
  assert.deepEqual(validateSettingsForm(validSettings), {
    value: {
      download_dir: "C:\\Downloads",
      max_active_downloads: 3,
      default_connections: 8,
      retry_count: 4,
      speed_limit_bytes: 1024 * 1024,
      clipboard_monitoring: true,
      minimize_to_tray: false,
      hls_transcode: false,
      theme: "dark",
      proxy_enabled: false,
      proxy_url: "",
      proxy_username: "",
      proxy_password: "",
    },
  });
});

test("settings form requires an enabled proxy URL", () => {
  assert.match(
    validateSettingsForm({ ...validSettings, proxyEnabled: true }).error,
    /代理地址/,
  );
});

test("settings form accepts HTTP and SOCKS proxies but rejects other schemes", () => {
  assert.ok(
    validateSettingsForm({
      ...validSettings,
      proxyEnabled: true,
      proxyUrl: "http://127.0.0.1:7890",
    }).value,
  );
  assert.ok(
    validateSettingsForm({
      ...validSettings,
      proxyEnabled: true,
      proxyUrl: "socks5://127.0.0.1:1080",
    }).value,
  );
  assert.match(
    validateSettingsForm({
      ...validSettings,
      proxyEnabled: true,
      proxyUrl: "ftp://127.0.0.1",
    }).error,
    /http/,
  );
});
