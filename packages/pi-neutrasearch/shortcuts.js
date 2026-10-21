import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

function quoteDesktop(value) {
  return `"${String(value).replace(/(["`$\\])/g, "\\$1")}"`;
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function lstatOrNull(file) {
  try {
    return fs.lstatSync(file);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

// Replace a target only when it is something this setup made earlier; anything
// else a user put there is left alone and reported.
function writeExecutable(file, content, isOurs, skipped) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const existing = lstatOrNull(file);
  if (existing && !(existing.isFile() && isOurs(fs.readFileSync(file, "utf8")))) {
    skipped.push(file);
    return false;
  }
  const temporary = `${file}.neutrasearch-${process.pid}-${crypto.randomBytes(4).toString("hex")}.tmp`;
  fs.writeFileSync(temporary, content, { mode: 0o755, flag: "wx" });
  fs.renameSync(temporary, file);
  return true;
}

function isNeutrasearchEntry(text) {
  return text.startsWith("[Desktop Entry]") && /^Name=Neutrasearch$/m.test(text);
}

// Only a symlink that already points at a file of the same name (a link this
// setup made earlier) is replaced; anything else is left alone.
function linkReplacingSymlink(target, link, skipped) {
  const existing = lstatOrNull(link);
  if (existing && !(existing.isSymbolicLink() && path.basename(fs.readlinkSync(link)) === path.basename(link))) {
    skipped.push(link);
    return false;
  }
  if (existing) fs.unlinkSync(link);
  fs.symlinkSync(target, link);
  return true;
}

function installLinux(application, home, packageRoot, skipped) {
  const binRoot = path.dirname(application);
  const icon = path.join(packageRoot, "assets", "neutrasearch.svg");
  const applications = path.join(home, ".local", "share", "applications");
  const desktopEntry = path.join(applications, "neutrasearch.desktop");
  fs.mkdirSync(applications, { recursive: true });
  const content = [
    "[Desktop Entry]",
    "Type=Application",
    "Version=1.0",
    "Name=Neutrasearch",
    "GenericName=Indexed File Search",
    "Comment=Search indexed filenames and paths without walking folders",
    `Exec=${quoteDesktop(application)}`,
    `Icon=${icon}`,
    "Terminal=false",
    "Categories=Utility;FileTools;System;",
    "Keywords=search;files;index;filename;",
    "StartupNotify=true",
    "StartupWMClass=neutrasearch",
    "",
  ].join("\n");
  const created = [];
  if (writeExecutable(desktopEntry, content, isNeutrasearchEntry, skipped)) created.push(desktopEntry);
  const desktop = path.join(home, "Desktop");
  if (fs.existsSync(desktop)) {
    const desktopShortcut = path.join(desktop, "Neutrasearch.desktop");
    if (writeExecutable(desktopShortcut, content, isNeutrasearchEntry, skipped)) created.push(desktopShortcut);
  }

  const localBin = path.join(home, ".local", "bin");
  fs.mkdirSync(localBin, { recursive: true });
  for (const name of ["neutrasearch", "neutrasearch-query", "neutrasearch-helper", "neutrasearch-mcp"]) {
    const source = path.join(binRoot, name);
    if (!fs.existsSync(source)) continue;
    const destination = path.join(localBin, name);
    if (linkReplacingSymlink(source, destination, skipped)) created.push(destination);
  }
  return created;
}

function installWindows(application, home, runPowerShell) {
  const script = [
    "$ErrorActionPreference = 'Stop'",
    "$shell = New-Object -ComObject WScript.Shell",
    "$targets = @(",
    "  [Environment]::GetFolderPath('Programs') + '\\Neutrasearch.lnk',",
    "  [Environment]::GetFolderPath('Desktop') + '\\Neutrasearch.lnk'",
    ")",
    "foreach ($target in $targets) {",
    "  if (Test-Path -LiteralPath $target) {",
    "    $existing = $shell.CreateShortcut($target)",
    "    if ((Split-Path $existing.TargetPath -Leaf) -ne (Split-Path $env:NEUTRASEARCH_APP -Leaf)) { continue }",
    "  }",
    "  $shortcut = $shell.CreateShortcut($target)",
    "  $shortcut.TargetPath = $env:NEUTRASEARCH_APP",
    "  $shortcut.WorkingDirectory = Split-Path $env:NEUTRASEARCH_APP",
    "  $shortcut.IconLocation = $env:NEUTRASEARCH_APP",
    "  $shortcut.Description = 'Indexed filename and path search'",
    "  $shortcut.Save()",
    "}",
  ].join("\n");
  runPowerShell("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
    env: { ...process.env, HOME: home, NEUTRASEARCH_APP: application },
    stdio: "ignore",
  });
  return ["Windows Start Menu/Neutrasearch.lnk", "Windows Desktop/Neutrasearch.lnk"];
}

function bundleIsOurs(app) {
  const stat = lstatOrNull(app);
  if (!stat) return true;
  if (!stat.isDirectory()) return false;
  const info = path.join(app, "Contents", "Info.plist");
  const infoStat = lstatOrNull(info);
  return Boolean(infoStat && infoStat.isFile() && fs.readFileSync(info, "utf8").includes("dev.pi.neutrasearch"));
}

function installMac(application, home, packageRoot, skipped) {
  const applications = path.join(home, "Applications");
  const app = path.join(applications, "Neutrasearch.app");
  fs.mkdirSync(applications, { recursive: true });
  if (!bundleIsOurs(app)) {
    skipped.push(app);
    return [];
  }
  // Build the whole bundle in a fresh private directory, then swap it in, so no
  // write ever follows a link inside an existing bundle.
  const staging = fs.mkdtempSync(path.join(applications, ".Neutrasearch-"));
  const staged = path.join(staging, "Neutrasearch.app");
  const contents = path.join(staged, "Contents");
  const launcher = path.join(contents, "MacOS", "Neutrasearch");
  fs.mkdirSync(path.dirname(launcher), { recursive: true });
  fs.writeFileSync(launcher, `#!/bin/sh\nexec ${shellQuote(application)} "$@"\n`, { mode: 0o755, flag: "wx" });
  const resources = path.join(contents, "Resources");
  fs.mkdirSync(resources, { recursive: true });
  fs.copyFileSync(
    path.join(packageRoot, "assets", "Neutrasearch.icns"),
    path.join(resources, "Neutrasearch.icns"),
    fs.constants.COPYFILE_EXCL,
  );
  fs.writeFileSync(
    path.join(contents, "Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleName</key><string>Neutrasearch</string><key>CFBundleDisplayName</key><string>Neutrasearch</string><key>CFBundleIdentifier</key><string>dev.pi.neutrasearch</string><key>CFBundleExecutable</key><string>Neutrasearch</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleIconFile</key><string>Neutrasearch.icns</string></dict></plist>\n`,
  );
  if (lstatOrNull(app)) fs.renameSync(app, path.join(staging, "previous.app"));
  fs.renameSync(staged, app);
  fs.rmSync(staging, { recursive: true, force: true });
  const created = [app];
  const desktop = path.join(home, "Desktop");
  if (fs.existsSync(desktop)) {
    const link = path.join(desktop, "Neutrasearch.app");
    if (linkReplacingSymlink(app, link, skipped)) created.push(link);
  }
  return created;
}

export function installShortcuts(application, options = {}) {
  if (!application || !path.isAbsolute(application) || !fs.existsSync(application)) {
    throw new Error("Neutrasearch application is missing");
  }
  const platform = options.platform || process.platform;
  const home = options.home || os.homedir();
  const packageRoot = options.packageRoot || path.dirname(fileURLToPath(import.meta.url));
  const skipped = [];
  let created;
  if (platform === "linux") created = installLinux(application, home, packageRoot, skipped);
  else if (platform === "darwin") created = installMac(application, home, packageRoot, skipped);
  else if (platform === "win32") created = installWindows(application, home, options.runPowerShell || execFileSync);
  else throw new Error(`shortcut installation is unsupported on ${platform}`);
  created.skipped = skipped;
  return created;
}
