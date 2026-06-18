import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { access, mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const desktopDir = path.join(root, "apps", "rdm-desktop");
const bundleDir = path.join(
  desktopDir,
  "src-tauri",
  "target",
  "release",
  "bundle",
);
const packageInfo = JSON.parse(
  await readFile(path.join(desktopDir, "package.json"), "utf8"),
);
const msi = path.join(
  bundleDir,
  "msi",
  `RDM_${packageInfo.version}_x64_en-US.msi`,
);
const nsis = path.join(
  bundleDir,
  "nsis",
  `RDM_${packageInfo.version}_x64-setup.exe`,
);

let extractionDir = null;
let installDir = null;

function run(file, args) {
  const result = spawnSync(file, args, {
    encoding: "utf8",
    windowsHide: true,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${path.basename(file)} exited ${result.status}\n${result.stderr || result.stdout}`,
    );
  }
  return result.stdout.trim();
}

function powershell(script) {
  return run("powershell.exe", [
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    script,
  ]);
}

function assertNoInstalledRdm() {
  const script = `
$paths = @(
  "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*",
  "HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*",
  "HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*"
)
$installed = Get-ItemProperty $paths -ErrorAction SilentlyContinue |
  Where-Object { $_.DisplayName -eq "RDM" }
if ($installed) { exit 17 }
`;
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", script],
    { windowsHide: true },
  );
  if (result.status === 17) {
    throw new Error(
      "An installed RDM copy was detected. Uninstall it before running smoke:bundle.",
    );
  }
  if (result.status !== 0) {
    throw new Error(`Could not inspect installed applications (exit ${result.status}).`);
  }
}

async function findFile(rootDir, filename) {
  const entries = await readdir(rootDir, {
    recursive: true,
    withFileTypes: true,
  });
  const entry = entries.find(
    (candidate) => candidate.isFile() && candidate.name === filename,
  );
  return entry ? path.join(entry.parentPath, entry.name) : null;
}

function fileVersion(filename) {
  return powershell(
    `(Get-Item -LiteralPath ${JSON.stringify(filename)}).VersionInfo.FileVersion`,
  );
}

try {
  if (process.platform !== "win32") {
    throw new Error("smoke:bundle requires Windows.");
  }
  await access(msi);
  await access(nsis);
  assertNoInstalledRdm();

  extractionDir = await mkdtemp(path.join(tmpdir(), "rdm-msi-admin-"));
  run("msiexec.exe", [
    "/a",
    msi,
    "/qn",
    `TARGETDIR=${extractionDir}`,
  ]);
  const extractedExecutable = await findFile(extractionDir, "rdm-desktop.exe");
  assert.ok(extractedExecutable, "MSI did not contain rdm-desktop.exe");
  assert.equal(fileVersion(extractedExecutable), packageInfo.version);
  console.log(
    `PASS  MSI administrative extraction — ${packageInfo.version}`,
  );

  installDir = await mkdtemp(path.join(tmpdir(), "rdm-nsis-install-"));
  await rm(installDir, { recursive: true, force: true });
  run(nsis, ["/S", `/D=${installDir}`]);
  const installedExecutable = await findFile(installDir, "rdm-desktop.exe");
  const uninstaller = await findFile(installDir, "uninstall.exe");
  assert.ok(installedExecutable, "NSIS did not install rdm-desktop.exe");
  assert.ok(uninstaller, "NSIS did not install uninstall.exe");
  assert.equal(fileVersion(installedExecutable), packageInfo.version);
  run(uninstaller, ["/S"]);
  await new Promise((resolve) => setTimeout(resolve, 500));
  await assert.rejects(
    access(installDir),
    (error) => error.code === "ENOENT",
    "NSIS uninstaller left the installation directory behind",
  );
  installDir = null;
  console.log(
    `PASS  NSIS silent install and uninstall — ${packageInfo.version}`,
  );
} catch (error) {
  console.error(`FAIL  ${error.stack || error.message}`);
  process.exitCode = 1;
} finally {
  for (const directory of [extractionDir, installDir]) {
    if (!directory) continue;
    const resolved = path.resolve(directory);
    const expectedPrefix = path.resolve(tmpdir()) + path.sep;
    if (
      !resolved.startsWith(expectedPrefix) ||
      !/^rdm-(msi-admin|nsis-install)-/.test(path.basename(resolved))
    ) {
      throw new Error(`Refusing to remove unexpected bundle path: ${resolved}`);
    }
    await rm(resolved, { recursive: true, force: true });
  }
}
