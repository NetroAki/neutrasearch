import fs from "node:fs";
import path from "node:path";


const DEFAULT_LIMIT = 20;
const MAX_LIMIT = 200;
const DEFAULT_MAX_CHARS = 6000;
const MAX_OUTPUT_CHARS = 30000;

function clampInteger(value, fallback, minimum, maximum) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.max(minimum, Math.min(maximum, Math.trunc(parsed)));
}

export { bundledPackageName, resolveBundledInstallation, resolveNeutrasearch, resolveNeutrasearchApp } from "./lib/installation.js";

function canonicalDirectory(input) {
  const resolved = fs.realpathSync(path.resolve(input));
  if (!fs.statSync(resolved).isDirectory()) throw new Error(`scope is not a directory: ${input}`);
  return resolved;
}

function comparable(value, platform = process.platform) {
  const normalized = path.resolve(value).replace(/[\\/]+$/, "");
  return platform === "win32" || platform === "darwin" ? normalized.toLowerCase() : normalized;
}

export function pathIsInside(candidate, root, platform = process.platform) {
  const normalizedCandidate = comparable(candidate, platform);
  const normalizedRoot = comparable(root, platform);
  if (normalizedCandidate === normalizedRoot) return true;
  return normalizedCandidate.startsWith(`${normalizedRoot}${path.sep}`);
}

export function resolveScope(requested, cwd, env = process.env, platform = process.platform) {
  const workspace = canonicalDirectory(cwd);
  const configured = String(env.NEUTRASEARCH_PI_ALLOWED_ROOTS || "")
    .split(path.delimiter)
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map(canonicalDirectory);
  const allowed = [workspace, ...configured];
  const scope = canonicalDirectory(requested || workspace);
  if (!allowed.some((root) => pathIsInside(scope, root, platform))) {
    throw new Error(
      "scope is outside this Pi workspace; add its canonical root to NEUTRASEARCH_PI_ALLOWED_ROOTS to opt in",
    );
  }
  return { scope, workspace, allowed };
}

function pathForOutput(candidate, scope, relative) {
  if (!relative) return candidate;
  const rel = path.relative(scope, candidate);
  return rel || ".";
}

export function compactSearchResult(payload, options) {
  if (!payload || !Array.isArray(payload.paths)) throw new Error("Neutrasearch returned invalid JSON");
  const scope = options.scope;
  const relative = options.relative !== false;
  const maxChars = clampInteger(options.maxChars, DEFAULT_MAX_CHARS, 500, MAX_OUTPUT_CHARS);
  const recordsByPath = new Map(
    Array.isArray(payload.records) ? payload.records.map((record) => [record.path, record]) : [],
  );
  const safe = [];
  let rejected = 0;
  for (const candidate of payload.paths) {
    if (typeof candidate !== "string" || !path.isAbsolute(candidate) || !pathIsInside(candidate, scope)) {
      rejected += 1;
      continue;
    }
    safe.push(candidate);
  }

  const lines = [
    `scope=${scope}`,
    `matched=${Number(payload.matched || 0)} returned=${safe.length} search_us=${Number(payload.search_us || 0)}`,
  ];
  let omittedByBudget = 0;
  for (let index = 0; index < safe.length; index += 1) {
    const candidate = safe[index];
    const record = recordsByPath.get(candidate);
    const displayPath = pathForOutput(candidate, scope, relative);
    const line = options.metadata && record
      ? `${displayPath}\t${record.kind}\t${record.size}\t${record.mtime}`
      : displayPath;
    if (lines.join("\n").length + line.length + 1 > maxChars) {
      omittedByBudget = safe.length - index;
      break;
    }
    lines.push(line);
  }
  if (omittedByBudget || rejected) {
    lines.push(`omitted_budget=${omittedByBudget} rejected_outside_scope=${rejected}`);
  }
  return {
    text: lines.join("\n"),
    details: {
      scope,
      matched: Number(payload.matched || 0),
      returned: safe.length,
      emitted: safe.length - omittedByBudget,
      omitted_by_budget: omittedByBudget,
      rejected_outside_scope: rejected,
      search_us: Number(payload.search_us || 0),
    },
  };
}

export function queryArguments(params, scope, env = process.env) {
  const limit = clampInteger(params.limit, DEFAULT_LIMIT, 1, MAX_LIMIT);
  const args = [
    String(params.query || ""),
    "--scope",
    scope,
    "--limit",
    String(limit),
    params.metadata ? "--json" : "--json-paths",
  ];
  if (env.NEUTRASEARCH_INDEX) args.push("--index", env.NEUTRASEARCH_INDEX);
  return { args, limit };
}

export const limits = {
  defaultLimit: DEFAULT_LIMIT,
  maxLimit: MAX_LIMIT,
  defaultMaxChars: DEFAULT_MAX_CHARS,
  maxOutputChars: MAX_OUTPUT_CHARS,
};
