import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";

import register from "./index.js";

test("registers one compact search tool and a status command", () => {
  let tool;
  const commands = [];
  register({
    registerTool(value) { tool = value; },
    registerCommand(name, value) { commands.push({ name, ...value }); },
  });
  assert.equal(tool.name, "neutrasearch");
  assert.match(tool.description, /Token-efficient/);
  assert.equal(tool.parameters.properties.limit.default, undefined);
  assert.deepEqual(commands.map((command) => command.name), ["neutrasearch-setup", "neutrasearch"]);
});

test("tool executes paths-only search and emits bounded relative paths", async () => {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pi-neutrasearch-tool-")));
  const executable = path.join(root, process.platform === "win32" ? "query.cmd" : "query");
  fs.writeFileSync(executable, process.platform === "win32" ? "@exit /b 0\r\n" : "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  const previousQuery = process.env.NEUTRASEARCH_QUERY;
  const previousBin = process.env.NEUTRASEARCH_BIN;
  delete process.env.NEUTRASEARCH_QUERY;
  process.env.NEUTRASEARCH_BIN = executable;
  let tool;
  register({
    registerTool(value) { tool = value; },
    registerCommand() {},
    async exec(command, args) {
      assert.equal(command, executable);
      assert.equal(args[0], "search");
      assert(args.includes("--json-paths"));
      assert(args.includes("--scope"));
      return {
        code: 0,
        stderr: "",
        stdout: JSON.stringify({
          paths: [path.join(root, "src", "needle.rs")],
          matched: 1,
          returned: 1,
          search_us: 12,
        }),
      };
    },
  });
  const output = await tool.execute("call-1", { query: "needle" }, undefined, undefined, { cwd: root });
  assert.match(output.content[0].text, /src[/\\]needle\.rs/);
  assert.equal(output.details.returned, 1);
  assert.equal(output.details.token_efficient, true);
  if (previousQuery === undefined) delete process.env.NEUTRASEARCH_QUERY;
  else process.env.NEUTRASEARCH_QUERY = previousQuery;
  if (previousBin === undefined) delete process.env.NEUTRASEARCH_BIN;
  else process.env.NEUTRASEARCH_BIN = previousBin;
  fs.rmSync(root, { recursive: true, force: true });
});

test("tool falls back to legacy JSON transport without expanding model output", async () => {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pi-neutrasearch-compat-")));
  const executable = path.join(root, process.platform === "win32" ? "query.cmd" : "query");
  fs.writeFileSync(executable, process.platform === "win32" ? "@exit /b 0\r\n" : "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  const previous = process.env.NEUTRASEARCH_QUERY;
  process.env.NEUTRASEARCH_QUERY = executable;
  let calls = 0;
  let tool;
  register({
    registerTool(value) { tool = value; },
    registerCommand() {},
    async exec(_command, args) {
      calls += 1;
      if (calls === 1) {
        assert(args.includes("--json-paths"));
        return { code: 2, stdout: "", stderr: "unknown option --json-paths" };
      }
      assert(args.includes("--json"));
      return {
        code: 0,
        stderr: "",
        stdout: JSON.stringify({
          paths: [path.join(root, "needle.rs")],
          records: [{ path: path.join(root, "needle.rs"), kind: "file", size: 42, mtime: 1 }],
          matched: 1,
          search_us: 9,
        }),
      };
    },
  });
  const output = await tool.execute("call-compat", { query: "needle" }, undefined, undefined, { cwd: root });
  assert.equal(calls, 2);
  assert.equal(output.details.transport, "metadata-json-compat");
  assert(!output.content[0].text.includes("\tfile\t42"));
  if (previous === undefined) delete process.env.NEUTRASEARCH_QUERY;
  else process.env.NEUTRASEARCH_QUERY = previous;
  fs.rmSync(root, { recursive: true, force: true });
});
