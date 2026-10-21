#!/usr/bin/env python3
"""Write THIRD-PARTY-NOTICES.txt: licence texts for every dependency in Cargo.lock.

Run from the repository root after changing dependencies:
    python3 packaging/tools/third_party_notices.py

Identical licence texts are written once with the crates that use them.
Bundled font licences from crates/neutra-gui/assets/fonts are appended.
"""
from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

LICENSE_NAME = re.compile(r"^(licen[sc]e|copying|notice|unlicense)", re.IGNORECASE)
HEADER = (
    "Neutrasearch bundles the third-party software listed below. Each section\n"
    "gives a licence text followed by the components that use it.\n"
)


def metadata(root: Path) -> list[dict]:
    out = subprocess.run(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)["packages"]


def licence_files(package_dir: Path) -> list[Path]:
    found = [p for p in sorted(package_dir.iterdir()) if p.is_file() and LICENSE_NAME.match(p.name)]
    for sub in ("licenses", "LICENSES", "license"):
        folder = package_dir / sub
        if folder.is_dir():
            found.extend(p for p in sorted(folder.iterdir()) if p.is_file())
    return found


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    groups: dict[str, dict] = {}
    missing: list[str] = []
    for package in sorted(metadata(root), key=lambda p: (p["name"], p["version"])):
        if package["name"].startswith("neutra-"):
            continue
        label = f"{package['name']} {package['version']} ({package.get('license') or 'see text'})"
        files = licence_files(Path(package["manifest_path"]).parent)
        if not files:
            who = ", ".join(package.get("authors") or []) or "authors not declared"
            missing.append(f"{label} by {who}")
            continue
        for path in files:
            text = path.read_text(encoding="utf-8", errors="replace").strip()
            key = hashlib.sha256(text.encode()).hexdigest()
            groups.setdefault(key, {"text": text, "users": []})["users"].append(label)
    fonts = root / "crates/neutra-gui/assets/fonts"
    for path in sorted(fonts.glob("LICENSE-*")):
        text = path.read_text(encoding="utf-8", errors="replace").strip()
        key = hashlib.sha256(text.encode()).hexdigest()
        groups.setdefault(key, {"text": text, "users": []})["users"].append(f"bundled fonts ({path.name})")
    parts = [HEADER]
    for index, group in enumerate(sorted(groups.values(), key=lambda g: g["users"][0].lower()), 1):
        users = sorted(set(group["users"]))
        parts.append(f"{'=' * 78}\nLicence text {index}\nUsed by:\n" + "\n".join(f"  - {u}" for u in users)
                     + f"\n{'-' * 78}\n{group['text']}\n")
    if missing:
        parts.append(f"{'=' * 78}\nCrates that ship no licence file; their declared licence applies:\n"
                     + "\n".join(f"  - {m}" for m in sorted(set(missing))) + "\n")
    (root / "THIRD-PARTY-NOTICES.txt").write_text("\n".join(parts), encoding="utf-8")
    print(f"{len(groups)} licence texts, {len(missing)} crates without a licence file", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
