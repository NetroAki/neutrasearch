#!/usr/bin/env bash
set -euo pipefail

git fetch --depth=1 origin refs/tags/v0.1.3
git archive FETCH_HEAD scripts packaging packages/pi-neutrasearch | tar -xf -
cp .github/windows/install-service.ps1 packaging/windows/install-service.ps1
if grep -q 'Stop-Service -Name.*NeutrasearchHelper' packaging/windows/install-service.ps1; then
    echo 'restored service installer still uses the unbounded Stop-Service path' >&2
    exit 1
fi
if ! grep -q 'sc.exe.*stop.*\$serviceName' packaging/windows/install-service.ps1; then
    echo 'restored service installer is missing the bounded sc.exe stop path' >&2
    exit 1
fi

# v0.1.3 packaged internal documents that are intentionally absent from the
# cleaned public source tree. Keep the reusable packaging implementation while
# aligning its release manifest with files present at the new tag.
python - <<'PY'
from pathlib import Path

path = Path("scripts/package_release.py")
source = path.read_text()
source = source.replace('    ("SECURITY.md", "SECURITY.md"),\n', '')
source = source.replace('    ("docs/production.md", "docs/production.md"),\n', '')
path.write_text(source)

path = Path("scripts/package_installers.py")
source = path.read_text()
source = source.replace(
    '("README.md", "LICENSE", "SECURITY.md", "CHANGELOG.md")',
    '("README.md", "LICENSE", "CHANGELOG.md")',
)
path.write_text(source)

path = Path("packaging/windows/neutrasearch.iss")
source = path.read_text()
source = source.replace(
    'Source: "..\\..\\SECURITY.md"; DestDir: "{app}"; Flags: ignoreversion\n',
    '',
)
blocking_stop = "Stop-Service -Name ''NeutrasearchHelper'' -Force; $s.WaitForStatus"
if source.count(blocking_stop) != 1:
    raise SystemExit("expected exactly one blocking Windows service stop")
source = source.replace(
    blocking_stop,
    "sc.exe stop NeutrasearchHelper | Out-Null; if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 1062) { exit $LASTEXITCODE }; $s.WaitForStatus",
)
path.write_text(source)

path = Path("packages/pi-neutrasearch/lib.js")
source = path.read_text()
replacements = [
    (
        '      command: bundled.query,\n      prefix: [],\n      kind: "bundled-query",',
        '      command: bundled.app,\n      prefix: ["search"],\n      kind: "bundled-launcher",',
    ),
    (
        '    { command: "neutrasearch-query", prefix: [], kind: "query" },\n    { command: "neutrasearch", prefix: ["search"], kind: "launcher" },',
        '    { command: "neutrasearch", prefix: ["search"], kind: "launcher" },\n    { command: "neutrasearch-query", prefix: [], kind: "query" },',
    ),
]
for old, new in replacements:
    if source.count(old) != 1:
        raise SystemExit(f"expected one Pi launcher fragment: {old!r}")
    source = source.replace(old, new)
path.write_text(source)

path = Path("packages/pi-neutrasearch/index.js")
source = path.read_text()
replacements = [
    (
        '    safety: "read-only index query; never scans, indexes, elevates, writes, or uses the network",',
        '    safety: "queries the last index; if missing, the launcher performs one full native-metadata machine index without directory walking",',
    ),
    (
        '  const timeout = Math.max(1000, Math.min(30000, Math.trunc(Number(params.timeout_ms) || 10000)));',
        '  const timeout = Math.max(1000, Math.min(3700000, Math.trunc(Number(params.timeout_ms) || 3700000)));',
    ),
    (
        '      maximum: 30000,\n      description: "Query timeout. Default 10000.",',
        '      maximum: 3700000,\n      description: "Query or first full-machine index timeout. Default 3700000.",',
    ),
    (
        '      "Token-efficient, read-only indexed filename/path search. Prefer this over find or broad filesystem scans when locating files. It does not search file contents; use grep only after locating candidate files.",',
        '      "Token-efficient indexed filename/path search using the last index automatically. If no index exists, Neutrasearch builds a full native-metadata machine index first. It does not search file contents.",',
    ),
]
for old, new in replacements:
    if source.count(old) != 1:
        raise SystemExit(f"expected one Pi tool fragment: {old!r}")
    source = source.replace(old, new)
path.write_text(source)
PY
