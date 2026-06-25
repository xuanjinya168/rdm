import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { access, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const desktopDir = path.join(root, "apps", "rdm-desktop");
const requireFromDesktop = createRequire(path.join(desktopDir, "package.json"));
const WebSocket = globalThis.WebSocket ?? requireFromDesktop("ws");
const executable = path.join(
  desktopDir,
  "src-tauri",
  "target",
  "release",
  "rdm-desktop.exe",
);
const packageInfo = JSON.parse(
  await readFile(path.join(desktopDir, "package.json"), "utf8"),
);

const results = [];
let app = null;
let browser = null;
let server = null;
let dataRoot = null;

function record(name, detail = "") {
  results.push({ name, detail });
  console.log(`PASS  ${name}${detail ? ` — ${detail}` : ""}`);
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function freePort() {
  return new Promise((resolve, reject) => {
    const socket = net.createServer();
    socket.once("error", reject);
    socket.listen(0, "127.0.0.1", () => {
      const { port } = socket.address();
      socket.close(() => resolve(port));
    });
  });
}

async function waitFor(label, probe, timeout = 20_000, interval = 100) {
  const deadline = Date.now() + timeout;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await probe();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await delay(interval);
  }
  throw new Error(
    `Timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}`,
  );
}

function assertNoRunningRdm() {
  const command =
    "if (Get-Process -Name rdm-desktop -ErrorAction SilentlyContinue) { exit 17 }";
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", command],
    { windowsHide: true },
  );
  if (result.status === 17) {
    throw new Error(
      "rdm-desktop.exe is already running. Exit it before running smoke:exe.",
    );
  }
  if (result.status !== 0) {
    throw new Error(`Could not inspect running processes (exit ${result.status}).`);
  }
}

async function stopProcess(child) {
  if (!child || child.exitCode !== null) return;
  child.kill();
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    delay(5_000),
  ]);
  if (child.exitCode === null) {
    spawnSync(
      "taskkill.exe",
      ["/PID", String(child.pid), "/T", "/F"],
      { windowsHide: true, stdio: "ignore" },
    );
  }
}

function closeMainWindow(child) {
  const script = `
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class RdmSmokeWindowFinder {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
}
"@
$targetPid = ${child.pid}
$script:rdmWindow = [IntPtr]::Zero
$callback = [RdmSmokeWindowFinder+EnumWindowsProc] {
  param($handle, $lParam)
  [uint32]$ownerPid = 0
  [RdmSmokeWindowFinder]::GetWindowThreadProcessId($handle, [ref]$ownerPid) | Out-Null
  if ($ownerPid -eq $targetPid) {
    $title = New-Object System.Text.StringBuilder 256
    [RdmSmokeWindowFinder]::GetWindowText($handle, $title, $title.Capacity) | Out-Null
    if ($title.ToString() -eq "RDM") {
      $script:rdmWindow = $handle
      return $false
    }
  }
  return $true
}
[RdmSmokeWindowFinder]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
if ($script:rdmWindow -eq [IntPtr]::Zero) { throw "RDM main window is unavailable" }
$root = [Windows.Automation.AutomationElement]::FromHandle($script:rdmWindow)
$condition = New-Object Windows.Automation.PropertyCondition(
  [Windows.Automation.AutomationElement]::AutomationIdProperty,
  "view_7"
)
$button = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
if (-not $button) { throw "RDM title-bar close button is unavailable" }
$pattern = $button.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)
$pattern.Invoke()
`;
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", script],
    { encoding: "utf8", windowsHide: true, timeout: 10_000 },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || "Could not close the RDM main window");
  }
}

function mainWindowVisible(child) {
  const script = `
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class RdmSmokeNativeVisibility {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
}
"@
$targetPid = ${child.pid}
$script:rdmWindow = [IntPtr]::Zero
$callback = [RdmSmokeNativeVisibility+EnumWindowsProc] {
  param($handle, $lParam)
  [uint32]$ownerPid = 0
  [RdmSmokeNativeVisibility]::GetWindowThreadProcessId($handle, [ref]$ownerPid) | Out-Null
  if ($ownerPid -eq $targetPid) {
    $title = New-Object System.Text.StringBuilder 256
    [RdmSmokeNativeVisibility]::GetWindowText($handle, $title, $title.Capacity) | Out-Null
    if ($title.ToString() -eq "RDM") {
      $script:rdmWindow = $handle
      return $false
    }
  }
  return $true
}
[RdmSmokeNativeVisibility]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
if ($script:rdmWindow -eq [IntPtr]::Zero) { throw "RDM main window is unavailable" }
[RdmSmokeNativeVisibility]::IsWindowVisible($script:rdmWindow)
`;
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", script],
    { encoding: "utf8", windowsHide: true, timeout: 5_000 },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || "Window visibility check failed");
  }
  return result.stdout.trim().toLowerCase() === "true";
}

class CdpPage {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 0;
    this.pending = new Map();
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (!message.id || !this.pending.has(message.id)) return;
      const { resolve, reject, timer } = this.pending.get(message.id);
      this.pending.delete(message.id);
      clearTimeout(timer);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result);
    });
    socket.addEventListener("close", () => {
      for (const { reject, timer } of this.pending.values()) {
        clearTimeout(timer);
        reject(new Error("WebView2 debugging connection closed"));
      }
      this.pending.clear();
    });
  }

  static async connect(port) {
    const target = await waitFor(
      "RDM WebView2 target",
      async () => {
        const response = await fetch(`http://127.0.0.1:${port}/json`);
        const targets = await response.json();
        return targets.find(
          (candidate) =>
            candidate.type === "page" &&
            candidate.url === "http://tauri.localhost/",
        );
      },
      20_000,
      200,
    );
    const socket = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    const page = new CdpPage(socket);
    await page.command("Runtime.enable");
    await page.waitForDom();
    return page;
  }

  command(method, params = {}) {
    const id = ++this.nextId;
    const result = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP command timed out: ${method}`));
      }, 10_000);
      this.pending.set(id, { resolve, reject, timer });
    });
    this.socket.send(JSON.stringify({ id, method, params }));
    return result;
  }

  async evaluate(expression) {
    const response = await this.command("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    if (response.exceptionDetails) {
      throw new Error(
        response.exceptionDetails.exception?.description ??
          response.exceptionDetails.text ??
          "WebView evaluation failed",
      );
    }
    return response.result.value;
  }

  invoke(command, args = {}) {
    return this.evaluate(
      `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
    );
  }

  waitForDom() {
    return waitFor(
      "RDM frontend",
      () =>
        this.evaluate(
          `document.readyState === "complete" && document.body.innerText.includes("下载中心")`,
        ),
      20_000,
      100,
    );
  }

  clickButton(text, scope = "document") {
    return this.evaluate(`(() => {
      const root = ${scope};
      const button = [...root.querySelectorAll("button")].find((candidate) =>
        candidate.innerText.trim() === ${JSON.stringify(text)} &&
        !candidate.disabled &&
        candidate.getClientRects().length > 0
      );
      if (!button) throw new Error(${JSON.stringify(`Enabled button not found: ${text}`)});
      button.click();
      return true;
    })()`);
  }

  setInput(selector, value) {
    return this.evaluate(`(() => {
      const input = document.querySelector(${JSON.stringify(selector)});
      if (!input) throw new Error(${JSON.stringify(`Input not found: ${selector}`)});
      const prototype = input instanceof HTMLTextAreaElement
        ? HTMLTextAreaElement.prototype
        : input instanceof HTMLSelectElement
          ? HTMLSelectElement.prototype
          : HTMLInputElement.prototype;
      const setter = Object.getOwnPropertyDescriptor(prototype, "value").set;
      setter.call(input, ${JSON.stringify(value)});
      input.dispatchEvent(new Event(
        input instanceof HTMLSelectElement ? "change" : "input",
        { bubbles: true }
      ));
      return input.value;
    })()`);
  }

  close() {
    this.socket.close();
  }
}

async function startFixtureServer(port) {
  let stdout = "";
  let stderr = "";
  const child = spawn(
    process.execPath,
    [path.join(root, "tools", "smoke-server.mjs")],
    {
      cwd: root,
      windowsHide: true,
      env: { ...process.env, RDM_SMOKE_PORT: String(port) },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  await waitFor(
    "local smoke server",
    async () => {
      if (child.exitCode !== null) {
        throw new Error(stderr || `server exited ${child.exitCode}`);
      }
      const response = await fetch(`http://127.0.0.1:${port}/`);
      return response.ok;
    },
    10_000,
    100,
  );
  const hashes = {};
  for (const match of stdout.matchAll(
    /^\/(\S+)\s+\d+\s+bytes\s+sha256=([0-9a-f]{64})$/gm,
  )) {
    hashes[`/${match[1]}`] = match[2];
  }
  return { child, hashes };
}

async function launchApp(args = []) {
  const debugPort = await freePort();
  const child = spawn(executable, args, {
    cwd: path.dirname(executable),
    windowsHide: true,
    env: {
      ...process.env,
      LOCALAPPDATA: dataRoot,
      NO_PROXY: "127.0.0.1,localhost",
      no_proxy: "127.0.0.1,localhost",
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        `--remote-debugging-port=${debugPort}`,
    },
    stdio: "ignore",
  });
  child.once("error", (error) => {
    console.error(`RDM process error: ${error.message}`);
  });
  try {
    const page = await CdpPage.connect(debugPort);
    return { child, page };
  } catch (error) {
    await stopProcess(child);
    throw error;
  }
}

async function replaceApp(args = []) {
  browser?.close();
  await stopProcess(app);
  const launched = await launchApp(args);
  app = launched.child;
  browser = launched.page;
}

async function tasks() {
  return browser.invoke("list_tasks");
}

async function waitForTask(id, status, timeout = 30_000) {
  return waitFor(
    `task ${id} to become ${status}`,
    async () => {
      const task = (await tasks()).find((candidate) => candidate.id === id);
      return task?.status === status ? task : false;
    },
    timeout,
    100,
  );
}

async function waitForTaskProgress(id, minimumBytes = 1) {
  return waitFor(
    `task ${id} to make progress`,
    async () => {
      const task = (await tasks()).find((candidate) => candidate.id === id);
      return task?.status === "downloading" && task.downloaded >= minimumBytes
        ? task
        : false;
    },
    20_000,
    50,
  );
}

async function waitForDialog(url) {
  await waitFor(
    "new-download dialog",
    () =>
      browser.evaluate(
        `document.querySelector('[aria-labelledby="add-title"]') !== null`,
      ),
  );
  if (url !== undefined) {
    const value = await browser.evaluate(
      `[...document.querySelectorAll('input')].find((input) => input.placeholder.includes("example.com"))?.value`,
    );
    assert.equal(value, url);
  }
}

async function addDownload({ url, filename = "", sha256 = "", dialogOpen = false }) {
  const previousIds = new Set((await tasks()).map((task) => task.id));
  if (!dialogOpen) {
    const quickInputVisible = await browser.evaluate(
      `document.querySelector('input[placeholder="粘贴文件地址，按 Enter 添加"]') !== null`,
    );
    if (!quickInputVisible) {
      await browser.clickButton("下载中心");
      await waitFor(
        "downloads quick-add input",
        () =>
          browser.evaluate(
            `document.querySelector('input[placeholder="粘贴文件地址，按 Enter 添加"]') !== null`,
          ),
      );
    }
    await browser.setInput(
      `input[placeholder="粘贴文件地址，按 Enter 添加"]`,
      url,
    );
    await browser.clickButton("添加");
    await waitForDialog(url);
  }
  if (filename) {
    await browser.setInput(
      `input[placeholder="留空则从服务器自动识别"]`,
      filename,
    );
  }
  if (sha256) {
    await browser.setInput(
      `input[placeholder="可选，64 位十六进制"]`,
      sha256,
    );
  }
  await browser.clickButton("开始下载");
  return waitFor(
    `new task for ${url}`,
    async () =>
      (await tasks()).find((task) => !previousIds.has(task.id)) || false,
    10_000,
    50,
  );
}

async function showAllTasks() {
  await browser.evaluate(`(() => {
    const button = [...document.querySelectorAll(".toolbar .chip")]
      .find((candidate) => candidate.innerText.trim().startsWith("全部"));
    if (!button) throw new Error("All-tasks filter is unavailable");
    if (!button.classList.contains("active")) button.click();
    return true;
  })()`);
}

async function selectTask(filename) {
  await showAllTasks();
  await waitFor(
    `task row ${filename}`,
    () =>
      browser.evaluate(`(() => {
        const row = [...document.querySelectorAll("tbody tr")]
          .find((candidate) => candidate.innerText.includes(${JSON.stringify(filename)}));
        if (!row) return false;
        row.click();
        return true;
      })()`),
  );
}

async function contextAction(filename, action) {
  await showAllTasks();
  await browser.evaluate(`(() => {
    const row = [...document.querySelectorAll("tbody tr")]
      .find((candidate) => candidate.innerText.includes(${JSON.stringify(filename)}));
    if (!row) throw new Error(${JSON.stringify(`Task row not found: ${filename}`)});
    row.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: 400,
      clientY: 300
    }));
    return true;
  })()`);
  await waitFor(
    `context action ${action}`,
    () =>
      browser.evaluate(
        `[...document.querySelectorAll(".ctx button")].some((button) => button.innerText.trim() === ${JSON.stringify(action)})`,
      ),
  );
  await browser.clickButton(action, `document.querySelector(".ctx")`);
}

async function fileHash(filename) {
  const content = await readFile(path.join(dataRoot, "downloads", filename));
  return createHash("sha256").update(content).digest("hex");
}

async function assertFileSize(filename, expected) {
  const info = await stat(path.join(dataRoot, "downloads", filename));
  assert.equal(info.size, expected);
}

async function assertMissing(filename) {
  await assert.rejects(
    access(path.join(dataRoot, "downloads", filename)),
    (error) => error.code === "ENOENT",
  );
}

async function main() {
  if (process.platform !== "win32") {
    throw new Error("smoke:exe requires Windows.");
  }
  await access(executable);
  assertNoRunningRdm();

  dataRoot = await mkdtemp(path.join(tmpdir(), "rdm-exe-smoke-"));
  const appData = path.join(dataRoot, "RDM");
  const downloadDir = path.join(dataRoot, "downloads");
  await mkdir(appData, { recursive: true });
  await mkdir(downloadDir, { recursive: true });
  await writeFile(
    path.join(appData, "settings.json"),
    JSON.stringify({
      download_dir: downloadDir,
      max_active_downloads: 2,
      default_connections: 2,
      speed_limit_bytes: 0,
      retry_count: 1,
      clipboard_monitoring: false,
      minimize_to_tray: false,
      theme: "dark",
      proxy_enabled: false,
      proxy_url: "",
      proxy_username: "",
      proxy_password: "",
    }),
  );

  const fixturePort = await freePort();
  const fixtures = await startFixtureServer(fixturePort);
  server = fixtures.child;
  const baseUrl = `http://127.0.0.1:${fixturePort}`;

  const initialUrl = `${baseUrl}/range.bin`;
  await replaceApp([initialUrl]);
  await waitForDialog(initialUrl);
  assert.equal(
    await browser.evaluate(
      `document.body.innerText.includes(${JSON.stringify(`RDM ${packageInfo.version}`)})`,
    ),
    true,
  );
  record("release EXE starts with matching UI version", packageInfo.version);
  record("first-launch URL opens a prefilled dialog");

  const firstRange = await addDownload({ url: initialUrl, dialogOpen: true });
  const firstRangeDone = await waitForTask(firstRange.id, "completed");
  assert.equal(firstRangeDone.supports_ranges, true);
  assert.equal(
    await fileHash(firstRangeDone.filename),
    fixtures.hashes["/range.bin"],
  );
  record("multi-connection Range download completes and matches SHA-256");

  const duplicate = await addDownload({ url: initialUrl });
  const duplicateDone = await waitForTask(duplicate.id, "completed");
  assert.notEqual(duplicateDone.filename, firstRangeDone.filename);
  assert.equal(
    await fileHash(duplicateDone.filename),
    fixtures.hashes["/range.bin"],
  );
  record("duplicate filenames do not overwrite existing files", duplicateDone.filename);

  const noRange = await addDownload({ url: `${baseUrl}/no-range.bin` });
  const noRangeDone = await waitForTask(noRange.id, "completed");
  assert.equal(noRangeDone.supports_ranges, false);
  assert.equal(
    await fileHash(noRangeDone.filename),
    fixtures.hashes["/no-range.bin"],
  );
  record("non-Range server falls back to one stream");

  const empty = await addDownload({ url: `${baseUrl}/empty.bin` });
  const emptyDone = await waitForTask(empty.id, "completed");
  await assertFileSize(emptyDone.filename, 0);
  record("empty file completes");

  const redirect = await addDownload({
    url: `${baseUrl}/redirect.bin`,
    filename: "redirect.bin",
  });
  const redirectDone = await waitForTask(redirect.id, "completed");
  assert.equal(
    await fileHash(redirectDone.filename),
    fixtures.hashes["/range.bin"],
  );
  record("HTTP redirect completes");

  const checksumOk = await addDownload({
    url: initialUrl,
    filename: "checksum-ok.bin",
    sha256: fixtures.hashes["/range.bin"],
  });
  const checksumOkDone = await waitForTask(checksumOk.id, "completed");
  assert.equal(checksumOkDone.actual_sha256, fixtures.hashes["/range.bin"]);
  record("correct SHA-256 is verified");

  const badChecksum = `${fixtures.hashes["/range.bin"].slice(0, -1)}0`;
  const checksumBad = await addDownload({
    url: initialUrl,
    filename: "checksum-bad.bin",
    sha256: badChecksum,
  });
  await waitForTask(checksumBad.id, "failed");
  await assertMissing("checksum-bad.bin");
  record("incorrect SHA-256 fails without publishing the final file");

  const serverError = await addDownload({
    url: `${baseUrl}/error.bin`,
    filename: "server-error.bin",
  });
  await waitForTask(serverError.id, "failed");
  record("server errors reach a terminal failed state");

  const pausable = await addDownload({
    url: `${baseUrl}/slow.bin`,
    filename: "pause-resume.bin",
  });
  await waitForTaskProgress(pausable.id, 512 * 1024);
  await selectTask("pause-resume.bin");
  await browser.clickButton(
    "暂停",
    `document.querySelector(".panel-heading-actions")`,
  );
  const paused = await waitForTask(pausable.id, "paused");
  const pausedBytes = paused.downloaded;
  await delay(800);
  assert.equal(
    (await tasks()).find((task) => task.id === pausable.id).downloaded,
    pausedBytes,
  );
  await browser.clickButton(
    "开始",
    `document.querySelector(".panel-heading-actions")`,
  );
  await waitForTask(pausable.id, "completed", 45_000);
  assert.equal(
    await fileHash("pause-resume.bin"),
    fixtures.hashes["/slow.bin"],
  );
  record("pause stops progress and continue completes");

  const cancelable = await addDownload({
    url: `${baseUrl}/slow.bin`,
    filename: "cancel.bin",
  });
  await waitForTaskProgress(cancelable.id, 256 * 1024);
  await contextAction("cancel.bin", "取消");
  await waitForTask(cancelable.id, "canceled");
  record("running task can be canceled");

  await selectTask(noRangeDone.filename);
  await browser.clickButton(
    "删除",
    `document.querySelector(".panel-heading-actions")`,
  );
  await browser.clickButton("仅删记录");
  await waitFor(
    "deleted no-range task record",
    async () => !(await tasks()).some((task) => task.id === noRange.id),
  );
  await access(path.join(downloadDir, noRangeDone.filename));
  record("delete-record keeps the completed file");

  await selectTask(duplicateDone.filename);
  await browser.clickButton(
    "删除",
    `document.querySelector(".panel-heading-actions")`,
  );
  await browser.clickButton("删除文件");
  await waitFor(
    "deleted duplicate task",
    async () => !(await tasks()).some((task) => task.id === duplicate.id),
  );
  await assertMissing(duplicateDone.filename);
  record("delete-file removes both task and file");

  await browser.clickButton("设置");
  await browser.clickButton("外观");
  await browser.evaluate(
    `document.querySelector('input[type="radio"][value="light"]').click()`,
  );
  await browser.clickButton("保存设置");
  await waitFor(
    "light theme",
    () =>
      browser.evaluate(`document.documentElement.dataset.theme === "light"`),
  );
  record("settings apply through the UI");

  const restartable = await addDownload({
    url: `${baseUrl}/slow.bin`,
    filename: "restart.bin",
  });
  const beforeRestart = await waitForTaskProgress(restartable.id, 512 * 1024);
  closeMainWindow(app);
  await waitFor(
    "graceful exit with an active task",
    () => app.exitCode !== null,
    15_000,
    100,
  );
  browser.close();
  browser = null;
  app = null;
  await delay(500);
  const relaunched = await launchApp();
  app = relaunched.child;
  browser = relaunched.page;
  await waitFor(
    "persisted light theme after restart",
    () => browser.evaluate(`document.documentElement.dataset.theme === "light"`),
  );
  const restored = (await tasks()).find((task) => task.id === restartable.id);
  assert.equal(restored.status, "paused");
  assert.ok(restored.downloaded >= beforeRestart.downloaded);
  await selectTask("restart.bin");
  await browser.clickButton(
    "开始",
    `document.querySelector(".panel-heading-actions")`,
  );
  await waitForTask(restartable.id, "completed", 45_000);
  assert.equal(await fileHash("restart.bin"), fixtures.hashes["/slow.bin"]);
  record("graceful exit preserves an active task for restart and resume");
  record("settings persist across restart");

  const secondUrl = `${baseUrl}/empty.bin`;
  const second = spawn(executable, [secondUrl], {
    cwd: path.dirname(executable),
    windowsHide: true,
    env: {
      ...process.env,
      LOCALAPPDATA: dataRoot,
      NO_PROXY: "127.0.0.1,localhost",
      no_proxy: "127.0.0.1,localhost",
    },
    stdio: "ignore",
  });
  await waitFor(
    "second instance to exit",
    () => second.exitCode !== null,
    10_000,
    50,
  );
  await waitForDialog(secondUrl);
  assert.equal(
    (await spawnSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "(Get-Process -Name rdm-desktop -ErrorAction SilentlyContinue | Measure-Object).Count",
      ],
      { encoding: "utf8", windowsHide: true },
    ).stdout.trim()),
    "1",
  );
  record("second launch exits and hands its URL to the running instance");
  await browser.clickButton("取消");

  const traySettings = await browser.invoke("get_settings");
  traySettings.minimize_to_tray = true;
  await browser.invoke("save_settings", { settings: traySettings });
  assert.equal(
    (await browser.invoke("get_settings")).minimize_to_tray,
    true,
  );
  closeMainWindow(app);
  await waitFor(
    "main window to hide in the tray",
    () => app.exitCode === null && !mainWindowVisible(app),
    10_000,
    200,
  );
  record("close-to-tray keeps the process running and hides the main window");

  const wake = spawn(executable, [], {
    cwd: path.dirname(executable),
    windowsHide: true,
    env: {
      ...process.env,
      LOCALAPPDATA: dataRoot,
      NO_PROXY: "127.0.0.1,localhost",
      no_proxy: "127.0.0.1,localhost",
    },
    stdio: "ignore",
  });
  await waitFor(
    "tray wake-up instance to exit",
    () => wake.exitCode !== null,
    10_000,
    50,
  );
  await waitFor(
    "main window to restore from the tray",
    () => mainWindowVisible(app),
    10_000,
    200,
  );
  record("a second launch restores the hidden tray window");

  const files = await readdir(downloadDir);
  assert.ok(files.includes(firstRangeDone.filename));
  console.log(`\n${results.length} automated release-EXE checks passed.`);
}

try {
  await main();
} catch (error) {
  console.error(`\nFAIL  ${error.stack || error.message}`);
  if (browser) {
    try {
      console.error(
        "\nCurrent tasks:",
        JSON.stringify(await tasks(), null, 2),
      );
      console.error(
        "\nCurrent UI:",
        await browser.evaluate(`document.body.innerText`),
      );
    } catch {}
  }
  process.exitCode = 1;
} finally {
  browser?.close();
  await stopProcess(app);
  await stopProcess(server);
  if (dataRoot) {
    const resolved = path.resolve(dataRoot);
    const expectedPrefix = path.resolve(tmpdir()) + path.sep;
    if (
      !resolved.startsWith(expectedPrefix) ||
      !path.basename(resolved).startsWith("rdm-exe-smoke-")
    ) {
      throw new Error(`Refusing to remove unexpected smoke path: ${resolved}`);
    }
    await rm(resolved, { recursive: true, force: true });
  }
}
