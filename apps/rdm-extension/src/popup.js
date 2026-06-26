import { pingHealth } from "./lib/bridge.js";
import { BRIDGE_BASE_URL, STORAGE_KEYS } from "./lib/config.js";

const toggle = document.getElementById("intercept-toggle");
const hint = document.getElementById("toggle-hint");
const statusEl = document.getElementById("bridge-status");
const testBtn = document.getElementById("test-btn");
const urlEl = document.getElementById("bridge-url");

urlEl.textContent = BRIDGE_BASE_URL;

// 读取并显示当前拦截开关状态。
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
  if (version) {
    setStatus("ok", `在线 · v${version}`);
  } else {
    setStatus("off", "离线（未检测到 RDM）");
  }
}

testBtn.addEventListener("click", runHealthCheck);
// 打开 popup 时自动探测一次。
runHealthCheck();
