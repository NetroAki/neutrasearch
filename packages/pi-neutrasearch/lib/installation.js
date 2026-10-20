import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);

function executableNames(name, platform = process.platform, pathExt = process.env.PATHEXT) {
  if (platform !== "win32" || path.extname(name)) return [name];
  const extensions = (pathExt || ".EXE;.CMD;.BAT;.COM")
    .split(";")
    .filter(Boolean)
    .map((extension) => extension.toLowerCase());
  return [name, ...extensions.map((extension) => `${name}${extension}`)];
}

function canExecute(candidate, platform = process.platform) {
  try {
    fs.accessSync(candidate, platform === "win32" ? fs.constants.F_OK : fs.constants.X_OK);
    return fs.statSync(candidate).isFile();
  } catch {
    return false;
  }
}

function findOnPath(command, env = process.env, platform = process.platform) {
  if (path.isAbsolute(command) || command.includes("/") || command.includes("\\")) {
    const expanded = command.startsWith("~/") ? path.join(os.homedir(), command.slice(2)) : command;
    return canExecute(expanded, platform) ? path.resolve(expanded) : null;
  }
  const pathValue = env.PATH || env.Path || env.path || "";
  for (const directory of pathValue.split(path.delimiter).filter(Boolean)) {
    for (const name of executableNames(command, platform, env.PATHEXT)) {
      const candidate = path.join(directory.replace(/^~(?=$|[\\/])/, os.homedir()), name);
      if (canExecute(candidate, platform)) return candidate;
    }
  }
  return null;
}

export function bundledPackageName(platform = process.platform, architecture = process.arch) {
  return {
    "linux:x64": "neutrasearch-linux-x64",
    "linux:arm64": "neutrasearch-linux-arm64",
    "win32:x64": "neutrasearch-windows-x64",
    "darwin:x64": "neutrasearch-darwin-x64",
    "darwin:arm64": "neutrasearch-darwin-arm64",
  }[`${platform}:${architecture}`] || null;
}

export function resolveBundledInstallation(
  platform = process.platform,
  architecture = process.arch,
  packageResolver = (specifier) => require.resolve(specifier),
) {
  const packageName = bundledPackageName(platform, architecture);
  if (!packageName) return null;
  try {
    const packageJson = packageResolver(`${packageName}/package.json`);
    const root = path.dirname(packageJson);
    const suffix = platform === "win32" ? ".exe" : "";
    const installation = {
      packageName,
      root,
      query: path.join(root, "bin", `neutrasearch-query${suffix}`),
      app: path.join(root, "bin", `neutrasearch${suffix}`),
      helper: path.join(root, "bin", `neutrasearch-helper${suffix}`),
      mcp: path.join(root, "bin", `neutrasearch-mcp${suffix}`),
    };
    return canExecute(installation.query, platform) && canExecute(installation.app, platform)
      ? installation
      : null;
  } catch {
    return null;
  }
}

export function resolveNeutrasearch(
  env = process.env,
  platform = process.platform,
  architecture = process.arch,
) {
  const explicit = [
    env.NEUTRASEARCH_QUERY
      ? { command: env.NEUTRASEARCH_QUERY, prefix: [], kind: "query" }
      : null,
    env.NEUTRASEARCH_BIN
      ? { command: env.NEUTRASEARCH_BIN, prefix: ["search"], kind: "launcher" }
      : null,
  ].filter(Boolean);
  for (const candidate of explicit) {
    const resolved = findOnPath(candidate.command, env, platform);
    if (resolved) return { ...candidate, command: resolved };
  }

  const bundled = resolveBundledInstallation(platform, architecture);
  if (bundled) {
    return {
      command: bundled.app,
      prefix: ["search"],
      kind: "bundled-launcher",
      app: bundled.app,
      packageName: bundled.packageName,
    };
  }

  const pathCandidates = [
    { command: "neutrasearch", prefix: ["search"], kind: "launcher" },
    { command: "neutrasearch-query", prefix: [], kind: "query" },
  ];
  for (const candidate of pathCandidates) {
    const resolved = findOnPath(candidate.command, env, platform);
    if (resolved) return { ...candidate, command: resolved };
  }
  return null;
}

export function resolveNeutrasearchApp(env = process.env, platform = process.platform) {
  if (env.NEUTRASEARCH_BIN) {
    const explicit = findOnPath(env.NEUTRASEARCH_BIN, env, platform);
    if (explicit) return explicit;
  }
  const bundled = resolveBundledInstallation(platform, process.arch);
  if (bundled) return bundled.app;
  return findOnPath("neutrasearch", env, platform);
}

