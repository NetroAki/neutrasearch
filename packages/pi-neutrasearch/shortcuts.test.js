import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";

import { installShortcuts } from "./shortcuts.js";

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "neutrasearch-shortcuts-"));
  const home = path.join(root, "home");
  const packageRoot = path.join(root, "package");
  const bin = path.join(root, "native", "bin");
  fs.mkdirSync(path.join(home, "Desktop"), { recursive: true });
  fs.mkdirSync(path.join(packageRoot, "assets"), { recursive: true });
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(path.join(packageRoot, "assets", "neutrasearch.svg"), "<svg/>");
  fs.writeFileSync(path.join(packageRoot, "assets", "neutrasearch.png"), "png");
  fs.writeFileSync(path.join(packageRoot, "assets", "Neutrasearch.icns"), "icns");
  for (const name of ["neutrasearch", "neutrasearch-query", "neutrasearch-helper", "neutrasearch-mcp"]) {
    fs.writeFileSync(path.join(bin, name), "binary", { mode: 0o755 });
  }
  return { root, home, packageRoot, application: path.join(bin, "neutrasearch") };
}

test("Linux setup installs menu, desktop, and CLI shortcuts", () => {
  const value = fixture();
  const created = installShortcuts(value.application, {
    platform: "linux",
    home: value.home,
    packageRoot: value.packageRoot,
  });
  assert.equal(created.length, 6);
  const entry = fs.readFileSync(
    path.join(value.home, ".local", "share", "applications", "neutrasearch.desktop"),
    "utf8",
  );
  assert.match(entry, /Name=Neutrasearch/);
  assert.match(entry, /Terminal=false/);
  assert(fs.lstatSync(path.join(value.home, ".local", "bin", "neutrasearch-query")).isSymbolicLink());
  fs.rmSync(value.root, { recursive: true, force: true });
});

test("macOS setup creates an app bundle and desktop alias", () => {
  const value = fixture();
  const created = installShortcuts(value.application, {
    platform: "darwin",
    home: value.home,
    packageRoot: value.packageRoot,
  });
  assert.equal(created.length, 2);
  const app = path.join(value.home, "Applications", "Neutrasearch.app");
  const info = path.join(app, "Contents", "Info.plist");
  assert(fs.existsSync(info));
  assert(fs.existsSync(path.join(app, "Contents", "Resources", "Neutrasearch.icns")));
  assert.match(fs.readFileSync(info, "utf8"), /CFBundleIconFile/);
  assert(fs.lstatSync(path.join(value.home, "Desktop", "Neutrasearch.app")).isSymbolicLink());
  fs.rmSync(value.root, { recursive: true, force: true });
});

test("Windows setup requests Start Menu and desktop links without interpolating paths", () => {
  const value = fixture();
  let invocation;
  const created = installShortcuts(value.application, {
    platform: "win32",
    home: value.home,
    packageRoot: value.packageRoot,
    runPowerShell(command, args, options) { invocation = { command, args, options }; },
  });
  assert.equal(created.length, 2);
  assert.equal(invocation.command, "powershell.exe");
  assert.equal(invocation.options.env.NEUTRASEARCH_APP, value.application);
  assert(!invocation.args.at(-1).includes(value.application));
  fs.rmSync(value.root, { recursive: true, force: true });
});

test("Linux setup leaves user-owned files and foreign links in place", () => {
  const value = fixture();
  const bin = path.join(value.home, ".local", "bin");
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(path.join(bin, "neutrasearch"), "my own script");
  fs.symlinkSync("/usr/bin/true", path.join(bin, "neutrasearch-query"));
  fs.writeFileSync(path.join(value.home, "Desktop", "Neutrasearch.desktop"), "notes");
  const created = installShortcuts(value.application, {
    platform: "linux",
    home: value.home,
    packageRoot: value.packageRoot,
  });
  assert.equal(fs.readFileSync(path.join(bin, "neutrasearch"), "utf8"), "my own script");
  assert.equal(fs.readlinkSync(path.join(bin, "neutrasearch-query")), "/usr/bin/true");
  assert.equal(fs.readFileSync(path.join(value.home, "Desktop", "Neutrasearch.desktop"), "utf8"), "notes");
  assert.equal(created.skipped.length, 3);
  const again = installShortcuts(value.application, {
    platform: "linux",
    home: value.home,
    packageRoot: value.packageRoot,
  });
  assert.equal(again.skipped.length, 3);
  fs.rmSync(value.root, { recursive: true, force: true });
});

test("macOS setup keeps a user's own app bundle and desktop folder", () => {
  const value = fixture();
  const app = path.join(value.home, "Applications", "Neutrasearch.app");
  fs.mkdirSync(path.join(app, "Contents"), { recursive: true });
  fs.writeFileSync(path.join(app, "Contents", "Info.plist"), "someone else");
  fs.mkdirSync(path.join(value.home, "Desktop", "Neutrasearch.app"));
  fs.writeFileSync(path.join(value.home, "Desktop", "Neutrasearch.app", "keep"), "data");
  const created = installShortcuts(value.application, {
    platform: "darwin",
    home: value.home,
    packageRoot: value.packageRoot,
  });
  assert.equal(created.length, 0);
  assert.equal(fs.readFileSync(path.join(app, "Contents", "Info.plist"), "utf8"), "someone else");
  assert.equal(created.skipped.length, 1);
  fs.rmSync(value.root, { recursive: true, force: true });
});
