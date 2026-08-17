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

function writeExecutable(file, content) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content, { mode: 0o755 });
  fs.chmodSync(file, 0o755);
}

function installLinux(application, home, packageRoot) {
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
  writeExecutable(desktopEntry, content);

  const created = [desktopEntry];
  const desktop = path.join(home, "Desktop");
  if (fs.existsSync(desktop)) {
    const desktopShortcut = path.join(desktop, "Neutrasearch.desktop");
    writeExecutable(desktopShortcut, content);
    created.push(desktopShortcut);
  }

  const localBin = path.join(home, ".local", "bin");
  fs.mkdirSync(localBin, { recursive: true });
  for (const name of ["neutrasearch", "neutrasearch-query", "neutrasearch-helper", "neutrasearch-mcp"]) {
    const source = path.join(binRoot, name);
    if (!fs.existsSync(source)) continue;
    const destination = path.join(localBin, name);
    try { fs.rmSync(destination, { force: true }); } catch {}
    fs.symlinkSync(source, destination);
    created.push(destination);
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

function installMac(application, home, packageRoot) {
  const app = path.join(home, "Applications", "Neutrasearch.app");
  const contents = path.join(app, "Contents");
  const launcher = path.join(contents, "MacOS", "Neutrasearch");
  writeExecutable(launcher, `#!/bin/sh\nexec ${shellQuote(application)} "$@"\n`);
  const resources = path.join(contents, "Resources");
  fs.mkdirSync(resources, { recursive: true });
  fs.copyFileSync(
    path.join(packageRoot, "assets", "Neutrasearch.icns"),
    path.join(resources, "Neutrasearch.icns"),
  );
  fs.writeFileSync(
    path.join(contents, "Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleName</key><string>Neutrasearch</string><key>CFBundleDisplayName</key><string>Neutrasearch</string><key>CFBundleIdentifier</key><string>dev.pi.neutrasearch</string><key>CFBundleExecutable</key><string>Neutrasearch</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleIconFile</key><string>Neutrasearch.icns</string></dict></plist>\n`,
  );
  const created = [app];
  const desktop = path.join(home, "Desktop");
  if (fs.existsSync(desktop)) {
    const link = path.join(desktop, "Neutrasearch.app");
    try { fs.rmSync(link, { recursive: true, force: true }); } catch {}
    fs.symlinkSync(app, link);
    created.push(link);
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
  if (platform === "linux") return installLinux(application, home, packageRoot);
  if (platform === "win32") {
    return installWindows(application, home, options.runPowerShell || execFileSync);
  }
  if (platform === "darwin") return installMac(application, home, packageRoot);
  throw new Error(`shortcut installation is unsupported on ${platform}`);
}
