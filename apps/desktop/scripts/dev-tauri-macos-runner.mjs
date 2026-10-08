#!/usr/bin/env node

import { spawn } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { stopOwned } from "./process-lifecycle.mjs";
import { runningMacOsDevAppPids } from "./macos-dev-app-process.mjs";

const desktopDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const executable = process.argv[2];
const appArgs = process.argv.slice(3);

if (!executable || !existsSync(executable)) {
  process.stderr.write("[tendi] macOS dev runner did not receive the compiled app path\n");
  process.exit(1);
}

const appBundle = resolve(dirname(executable), "bundle", "macos", "Tendi.app");
const contentsDir = resolve(appBundle, "Contents");
const executableDir = resolve(contentsDir, "MacOS");
const resourcesDir = resolve(contentsDir, "Resources");
const appExecutable = resolve(executableDir, "tendi-desktop");
const iconSource = resolve(desktopDir, "src-tauri", "icons", "icon.icns");
const appIcon = resolve(resourcesDir, "icon.icns");
const bundleIdentifier = "dev.tendi.desktop.dev";

mkdirSync(executableDir, { recursive: true });
mkdirSync(resourcesDir, { recursive: true });
copyFileSync(executable, appExecutable);
chmodSync(appExecutable, 0o755);
copyFileSync(iconSource, appIcon);
writeFileSync(
  resolve(contentsDir, "Info.plist"),
  `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>tendi-desktop</string>
  <key>CFBundleIdentifier</key><string>${bundleIdentifier}</string>
  <key>CFBundleDisplayName</key><string>Tendi</string>
  <key>CFBundleName</key><string>Tendi</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>0.0.0</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
`,
);

function runningBundlePids() {
  return runningMacOsDevAppPids(appExecutable);
}

function isRunning(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

function signalPids(pids, signal) {
  for (const pid of pids) {
    try {
      process.kill(pid, signal);
    } catch (error) {
      if (error?.code !== "ESRCH") throw error;
    }
  }
}

async function terminatePids(pids) {
  const targets = [...new Set(pids)];
  signalPids(targets.filter(isRunning), "SIGTERM");
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline && targets.some(isRunning)) {
    await new Promise((done) => setTimeout(done, 100));
  }
  signalPids(targets.filter(isRunning), "SIGKILL");
  const killDeadline = Date.now() + 1_000;
  while (Date.now() < killDeadline && targets.some(isRunning)) {
    await new Promise((done) => setTimeout(done, 100));
  }
}

const previousPids = new Set(runningBundlePids());
await terminatePids([...previousPids]);

const managedPids = new Set();

function discoverManagedPids() {
  for (const pid of runningBundlePids()) {
    if (!previousPids.has(pid)) managedPids.add(pid);
  }
  return [...managedPids];
}

const openArgs = [];
for (const name of ["TENDI_CWD", "CARGO_TARGET_DIR"]) {
  if (process.env[name]) openArgs.push("--env", `${name}=${process.env[name]}`);
}
openArgs.push(appBundle);
if (appArgs.length > 0) openArgs.push("--args", ...appArgs);

const child = spawn("open", openArgs, {
  cwd: process.cwd(),
  env: process.env,
  stdio: "inherit",
});
let shuttingDown = false;
let launchExitCode = null;

async function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  await terminatePids([...managedPids]);
  await stopOwned(child);
  process.exit(code);
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"]) {
  process.once(signal, () => void shutdown());
}

child.once("error", (error) => {
  process.stderr.write(`[tendi] failed to open the macOS dev app: ${error.message}\n`);
  void shutdown(1);
});

child.once("exit", (code, signal) => {
  launchExitCode = code ?? (signal ? 1 : 0);
  if (launchExitCode !== 0) {
    void shutdown(launchExitCode);
    return;
  }
  void waitForBundleApp();
});

async function waitForBundleApp() {
  const launchDeadline = Date.now() + 10_000;
  while (!shuttingDown && Date.now() < launchDeadline && discoverManagedPids().length === 0) {
    await new Promise((done) => setTimeout(done, 100));
  }

  if (shuttingDown) return;
  if (discoverManagedPids().length === 0) {
    process.stderr.write("[tendi] macOS dev app did not stay running after launch\n");
    await shutdown(1);
    return;
  }

  while (!shuttingDown && [...managedPids].some(isRunning)) {
    await new Promise((done) => setTimeout(done, 250));
  }
  if (!shuttingDown) await shutdown(0);
}
