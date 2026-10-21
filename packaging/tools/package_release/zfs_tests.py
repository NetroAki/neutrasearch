"""Focused Linux ZFS payload checks for release packaging."""
from __future__ import annotations

import json
from pathlib import Path

from .adapters import archive_payload, npm_platform_package, sha256_bytes, zfs_payload

PRIVATE = (
    "libzpool.so.5",
    "libzfs.so.4",
    "libzfs_core.so.3",
    "libnvpair.so.3",
    "libuutil.so.3",
)
LICENSES = ("zfs-LICENSE", "zfs-openzfs-LICENSE")


def fixture(target: Path) -> None:
    files: dict[str, str] = {}
    payload = {"neutrasearch-zfs": b"reader"}
    payload.update({f"zfs-runtime/{name}": name.encode() for name in PRIVATE})
    payload.update({name: name.encode() for name in LICENSES})
    for relative, content in payload.items():
        path = target / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        files[relative] = sha256_bytes(content)
    (target / "zfs-manifest.json").write_text(json.dumps({
        "openzfs_version": "2.2.2",
        "reader": "neutrasearch-zfs",
        "files": files,
    }))


def run_zfs_tests(root: Path, project: Path, built: Path) -> None:
    fixture(built)
    payload = zfs_payload(built)
    names = {name for name, _, _ in payload}
    if not {"neutrasearch-zfs", "zfs-runtime/" + PRIVATE[0], *LICENSES}.issubset(names):
        raise AssertionError("Linux ZFS payload omitted required reader/runtime/license files")

    # Both archive and npm layouts keep the reader at the privileged helper's root.
    packaged = archive_payload(project, built, "x86_64-unknown-linux-gnu")
    if not any(name == "neutrasearch-zfs" for name, _, _ in packaged):
        raise AssertionError("Linux archive does not place ZFS reader beside helper")
    npm = npm_platform_package(project, built, root / "npm-zfs", "x86_64-unknown-linux-gnu", "1.2.3-rc.1")
    if not (npm / "bin/neutrasearch-zfs").is_file():
        raise AssertionError("Linux npm package does not place ZFS reader beside helper")

    (built / "zfs-manifest.json").unlink()
    try:
        zfs_payload(built)
    except (OSError, ValueError):
        pass
    else:
        raise AssertionError("missing ZFS manifest was accepted")

    fixture(built)
    (built / "neutrasearch-zfs").write_bytes(b"tampered")
    try:
        zfs_payload(built)
    except ValueError as error:
        if "hash mismatch" not in str(error):
            raise
    else:
        raise AssertionError("hash-mismatched ZFS reader was accepted")
