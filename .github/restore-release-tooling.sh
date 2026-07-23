#!/usr/bin/env bash
set -euo pipefail

git fetch --depth=1 origin refs/tags/v0.1.3
git archive FETCH_HEAD scripts packaging packages/pi-neutrasearch | tar -xf -

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
PY
