import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";

import {
  bundledPackageName,
  compactSearchResult,
  pathIsInside,
  queryArguments,
  resolveBundledInstallation,
  resolveNeutrasearch,
  resolveScope,
} from "./lib.js";

function temporaryTree() {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pi-neutrasearch-")));
  const workspace = path.join(root, "workspace");
  const sibling = path.join(root, "private");
  fs.mkdirSync(workspace);
  fs.mkdirSync(sibling);
  return { root, workspace, sibling };
}

test("maps every supported Pi runtime to a native package", () => {
  assert.equal(bundledPackageName("linux", "x64"), "neutrasearch-linux-x64");
  assert.equal(bundledPackageName("linux", "arm64"), "neutrasearch-linux-arm64");
  assert.equal(bundledPackageName("win32", "x64"), "neutrasearch-windows-x64");
  assert.equal(bundledPackageName("darwin", "x64"), "neutrasearch-darwin-x64");
  assert.equal(bundledPackageName("darwin", "arm64"), "neutrasearch-darwin-arm64");
  assert.equal(bundledPackageName("win32", "arm64"), null);
});

test("resolves a complete bundled installation", () => {
  const tree = temporaryTree();
  const packageRoot = path.join(tree.root, "neutrasearch-linux-x64");
  const bin = path.join(packageRoot, "bin");
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(path.join(packageRoot, "package.json"), "{}");
  for (const name of ["neutrasearch", "neutrasearch-query", "neutrasearch-helper", "neutrasearch-mcp"]) {
    fs.writeFileSync(path.join(bin, name), "#!/bin/sh\n", { mode: 0o700 });
  }
  const installation = resolveBundledInstallation(
    "linux",
    "x64",
    () => path.join(packageRoot, "package.json"),
  );
  assert.equal(installation.packageName, "neutrasearch-linux-x64");
  assert.equal(installation.app, path.join(bin, "neutrasearch"));
  fs.rmSync(tree.root, { recursive: true, force: true });
});

test("resolves explicitly configured query executable first", () => {
  const tree = temporaryTree();
  const executable = path.join(tree.root, process.platform === "win32" ? "query.cmd" : "query");
  fs.writeFileSync(executable, process.platform === "win32" ? "@exit /b 0\r\n" : "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  const resolved = resolveNeutrasearch({ NEUTRASEARCH_QUERY: executable, PATH: "" });
  assert.equal(resolved.command, executable);
  assert.equal(resolved.kind, "query");
  fs.rmSync(tree.root, { recursive: true, force: true });
});

test("scope defaults to workspace and rejects unapproved siblings", () => {
  const tree = temporaryTree();
  assert.equal(resolveScope(undefined, tree.workspace, {}).scope, tree.workspace);
  assert.throws(
    () => resolveScope(tree.sibling, tree.workspace, {}),
    /outside this Pi workspace/,
  );
  assert.equal(
    resolveScope(tree.sibling, tree.workspace, {
      NEUTRASEARCH_PI_ALLOWED_ROOTS: tree.sibling,
    }).scope,
    tree.sibling,
  );
  fs.rmSync(tree.root, { recursive: true, force: true });
});

test("component boundaries prevent sibling-prefix scope escape", () => {
  assert(pathIsInside(path.join("/work", "app", "src"), path.join("/work", "app")));
  assert(!pathIsInside(path.join("/work", "application"), path.join("/work", "app")));
});

test("compact output is relative, metadata-free, and scope filtered by default", () => {
  const scope = path.resolve("/workspace/project");
  const inside = path.join(scope, "src", "main.rs");
  const outside = path.resolve("/workspace/private/secret.txt");
  const compact = compactSearchResult(
    {
      paths: [inside, outside],
      matched: 2,
      search_us: 41,
      records: [{ path: inside, kind: "file", size: 99, mtime: 4 }],
    },
    { scope, maxChars: 6000 },
  );
  assert.match(compact.text, /src[/\\]main\.rs/);
  assert(!compact.text.includes("secret.txt"));
  assert(!compact.text.includes("\tfile\t99"));
  assert.equal(compact.details.rejected_outside_scope, 1);
});

test("character budget truncates path output deterministically", () => {
  const scope = path.resolve("/workspace/project");
  const paths = Array.from({ length: 50 }, (_, index) => path.join(scope, "nested", `${index}-${"x".repeat(40)}.txt`));
  const compact = compactSearchResult(
    { paths, matched: 50, search_us: 10 },
    { scope, maxChars: 500 },
  );
  assert(compact.text.length <= 560);
  assert(compact.details.omitted_by_budget > 0);
  assert.match(compact.text, /omitted_budget=/);
});

test("paths-only JSON is the token-efficient query default", () => {
  const scope = path.resolve("/workspace/project");
  const compact = queryArguments({ query: "needle" }, scope, {});
  assert.equal(compact.limit, 20);
  assert(compact.args.includes("--json-paths"));
  assert(!compact.args.includes("--json"));
  const metadata = queryArguments({ query: "needle", metadata: true }, scope, {});
  assert(metadata.args.includes("--json"));
});
