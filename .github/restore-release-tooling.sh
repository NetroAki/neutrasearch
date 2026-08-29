#!/usr/bin/env bash
set -euo pipefail

git fetch --depth=1 origin refs/tags/v0.1.3
git archive FETCH_HEAD scripts packaging | tar -xf -
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

PY
