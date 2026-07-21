import { spawn } from "node:child_process";
import { installShortcuts } from "./shortcuts.js";
import {
  compactSearchResult,
  limits,
  queryArguments,
  resolveNeutrasearch,
  resolveNeutrasearchApp,
  resolveScope,
} from "./lib.js";

const TOOL_NAME = "neutrasearch";

function shortError(value, max = 1600) {
  const text = String(value || "unknown error").replace(/\s+/g, " ").trim();
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

function result(text, details = {}) {
  return { content: [{ type: "text", text }], details };
}

function status(binary) {
  if (!binary) {
    return {
      ok: false,
      tool: TOOL_NAME,
      error: "Neutrasearch executable not found",
      setup: "Reinstall pi-neutrasearch to fetch its matching native binary package, or set NEUTRASEARCH_QUERY / NEUTRASEARCH_BIN.",
      token_defaults: { limit: limits.defaultLimit, max_chars: limits.defaultMaxChars },
    };
  }
  return {
    ok: true,
    tool: TOOL_NAME,
    executable: binary.command,
    executable_kind: binary.kind,
    bundled_package: binary.packageName || null,
    app: binary.app || resolveNeutrasearchApp() || null,
    index: process.env.NEUTRASEARCH_INDEX || "platform default",
    default_scope: "current Pi workspace",
    extra_scope_env: "NEUTRASEARCH_PI_ALLOWED_ROOTS",
    token_defaults: {
      limit: limits.defaultLimit,
      max_chars: limits.defaultMaxChars,
      relative_paths: true,
      metadata: false,
    },
    safety: "read-only index query; never scans, indexes, elevates, writes, or uses the network",
  };
}

async function search(pi, params, signal, ctx) {
  if (!String(params.query || "").trim()) throw new Error("query is required for action=search");
  const binary = resolveNeutrasearch();
  if (!binary) {
    throw new Error(
      "Neutrasearch executable not found. Install Neutrasearch or set NEUTRASEARCH_QUERY / NEUTRASEARCH_BIN.",
    );
  }
  const { scope } = resolveScope(params.scope, ctx.cwd);
  const { args, limit } = queryArguments(params, scope);
  const timeout = Math.max(1000, Math.min(30000, Math.trunc(Number(params.timeout_ms) || 10000)));
  let transport = params.metadata ? "metadata-json" : "paths-only-json";
  let execution = await pi.exec(binary.command, [...binary.prefix, ...args], {
    cwd: ctx.cwd,
    signal,
    timeout,
  });
  if (
    execution.code !== 0
    && !params.metadata
    && args.includes("--json-paths")
    && /(?:unknown|unrecognized|unexpected).*(?:--json-paths)|--json-paths.*(?:unknown|unrecognized|unexpected)/i
      .test(`${execution.stderr}\n${execution.stdout}`)
  ) {
    const compatibleArgs = args.map((argument) => argument === "--json-paths" ? "--json" : argument);
    execution = await pi.exec(binary.command, [...binary.prefix, ...compatibleArgs], {
      cwd: ctx.cwd,
      signal,
      timeout,
    });
    transport = "metadata-json-compat";
  }
  if (execution.code !== 0) {
    throw new Error(`Neutrasearch exited ${execution.code}: ${shortError(execution.stderr || execution.stdout)}`);
  }
  let payload;
  try {
    payload = JSON.parse(execution.stdout);
  } catch {
    throw new Error(`Neutrasearch returned non-JSON output: ${shortError(execution.stdout)}`);
  }
  const compact = compactSearchResult(payload, {
    scope,
    relative: params.relative_paths !== false,
    metadata: Boolean(params.metadata),
    maxChars: params.max_chars,
  });
  return result(compact.text, {
    ...compact.details,
    query: params.query,
    limit,
    executable_kind: binary.kind,
    transport,
    token_efficient: true,
  });
}

const PARAMETERS = {
  type: "object",
  properties: {
    action: {
      enum: ["search", "status"],
      description: "search (default) or status.",
    },
    query: {
      type: "string",
      description: "Filename/path query. Supports ext:, kind:, fs:, size:, and quoted terms.",
    },
    scope: {
      type: "string",
      description: "Existing directory inside the current workspace. Defaults to the workspace root.",
    },
    limit: {
      type: "integer",
      minimum: 1,
      maximum: limits.maxLimit,
      description: `Maximum matches. Default ${limits.defaultLimit}; keep small for token efficiency.`,
    },
    relative_paths: {
      type: "boolean",
      description: "Return paths relative to scope. Default true.",
    },
    metadata: {
      type: "boolean",
      description: "Include kind, size, and mtime columns. Default false.",
    },
    max_chars: {
      type: "integer",
      minimum: 500,
      maximum: limits.maxOutputChars,
      description: `Hard output budget. Default ${limits.defaultMaxChars}.`,
    },
    timeout_ms: {
      type: "integer",
      minimum: 1000,
      maximum: 30000,
      description: "Query timeout. Default 10000.",
    },
  },
  additionalProperties: false,
};

export default function register(pi) {
  pi.registerTool({
    name: TOOL_NAME,
    label: "Neutrasearch",
    description:
      "Token-efficient, read-only indexed filename/path search. Prefer this over find or broad filesystem scans when locating files. It does not search file contents; use grep only after locating candidate files.",
    parameters: PARAMETERS,
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const action = String(params?.action || "search");
      if (action === "status") {
        const current = status(resolveNeutrasearch());
        return result(JSON.stringify(current), current);
      }
      if (action !== "search") throw new Error(`unknown action: ${action}`);
      return search(pi, params || {}, signal, ctx);
    },
  });

  pi.registerCommand("neutrasearch-setup", {
    description: "Open the bundled Neutrasearch app to approve and build its index",
    handler: async (_args, ctx) => {
      const application = resolveNeutrasearchApp();
      if (!application) {
        ctx.ui.notify(
          "Neutrasearch app is missing. Reinstall pi-neutrasearch or set NEUTRASEARCH_BIN.",
          "error",
        );
        return;
      }
      try {
        const shortcuts = installShortcuts(application);
        const child = spawn(application, [], {
          detached: true,
          stdio: "ignore",
          windowsHide: false,
        });
        child.unref();
        ctx.ui.notify(
          `Neutrasearch opened and ${shortcuts.length} shortcuts were installed. Approve local indexing in the app, then return to Pi.`,
          "info",
        );
      } catch (error) {
        ctx.ui.notify(`Could not open Neutrasearch: ${shortError(error)}`, "error");
      }
    },
  });

  pi.registerCommand("neutrasearch", {
    description: "Show Neutrasearch Pi integration status",
    handler: async (_args, ctx) => {
      const current = status(resolveNeutrasearch());
      ctx.ui.notify(current.ok ? `Neutrasearch ready: ${current.executable}` : current.error, current.ok ? "info" : "warning");
    },
  });
}
