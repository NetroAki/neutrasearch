#!/usr/bin/env python3
"""Build and stage the pinned OpenZFS 2.2.2 native metadata reader."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess

VERSION = "2.2.2"
PRIVATE = ("libzpool", "libzfs", "libzfs_core", "libnvpair", "libuutil")
PRIVATE_SONAMES = {
    "libzpool": "libzpool.so.5",
    "libzfs": "libzfs.so.4",
    "libzfs_core": "libzfs_core.so.3",
    "libnvpair": "libnvpair.so.3",
    "libuutil": "libuutil.so.3",
}
COMMAND_TIMEOUT_SECONDS = 120


def run(args: list[str], *, capture: bool = True, timeout: int = COMMAND_TIMEOUT_SECONDS) -> str:
    result = subprocess.run(
        args,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        timeout=timeout,
    )
    return result.stdout.strip() if capture else ""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_vendor_pin(project_root: Path) -> dict[str, str]:
    vendor_root = project_root / "vendor/openzfs-2.2.2"
    metadata = json.loads((vendor_root / "source.json").read_text(encoding="utf-8"))
    if metadata.get("version") != VERSION or metadata.get("commit") != "494aaaed89cb9fe9f2da3b6c6f465a4bc9f6a7e1":
        raise ValueError("vendored OpenZFS source pin must identify 2.2.2 commit 494aaaed89cb9fe9f2da3b6c6f465a4bc9f6a7e1")
    expected = metadata.get("files")
    required = {"include/sys/zfs_ioctl.h", "LICENSE"}
    if not isinstance(expected, dict) or not required.issubset(expected):
        raise ValueError("vendored OpenZFS source.json must hash the header and upstream LICENSE")
    for relative, digest in expected.items():
        path = vendor_root / relative
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"vendored OpenZFS file is missing or not regular: {relative}")
        if sha256(path) != digest:
            raise ValueError(f"vendored OpenZFS hash mismatch: {relative}")
    return expected


def build(project_root: Path, target_dir: Path) -> Path:
    project_root, target_dir = project_root.resolve(), target_dir.resolve()
    vendor_hashes = verify_vendor_pin(project_root)
    version = run(["pkg-config", "--modversion", "libzfs"])
    if version != VERSION:
        raise ValueError(f"OpenZFS {VERSION} required; pkg-config reports {version!r}")
    cflags = run(["pkg-config", "--cflags", "libzfs"]).split()
    pkg_libs = run(["pkg-config", "--libs", "libzfs"]).split()
    link_search = [flag for flag in pkg_libs if not flag.startswith("-l")]
    libs = [
        *link_search,
        "-lzpool",
        "-lzfs",
        "-lzfs_core",
        "-lnvpair",
        "-luutil",
        "-lpthread",
    ]
    include_dirs = [Path(flag[2:]) for flag in cflags if flag.startswith("-I")]
    if not include_dirs or not all(path.is_dir() for path in include_dirs):
        raise ValueError("matching OpenZFS development headers are required")
    output = target_dir / "neutrasearch-zfs"
    output.parent.mkdir(parents=True, exist_ok=True)
    native = project_root / "crates/neutra-zfs/native"
    sources = sorted(native.glob("*.c"))
    if not sources or not (native / "reader.h").is_file():
        raise FileNotFoundError(f"native reader sources or reader.h missing under {native}")
    vendored_include = project_root / "vendor/openzfs-2.2.2/include"
    vendored_ioctl = vendored_include / "sys/zfs_ioctl.h"
    if not vendored_ioctl.is_file():
        raise FileNotFoundError(f"pinned upstream header missing: {vendored_ioctl}")
    vendor_root = project_root / "vendor/openzfs-2.2.2"
    upstream_license = vendor_root / "LICENSE"
    run(["cc", "-std=gnu11", "-D_GNU_SOURCE", "-D_LARGEFILE64_SOURCE",
         f'-DNEUTRA_ZFS_ABI_VERSION="{VERSION}"', "-I", str(vendored_include), *cflags, "-Wall", "-Wextra", "-Werror", "-O2",
         *(str(source) for source in sources),
         "-o", str(output), *libs, "-Wl,--disable-new-dtags", "-Wl,-rpath,$ORIGIN/zfs-runtime"], capture=False)

    runtime = target_dir / "zfs-runtime"
    if runtime.exists():
        shutil.rmtree(runtime)
    runtime.mkdir()
    dependencies = run(["ldd", str(output)])
    for line in dependencies.splitlines():
        match = re.search(r"\b(lib(?:zpool|zfs_core|zfs|nvpair|uutil)\.so(?:\.\S+)?)\s+=>\s+(\S+)", line)
        if match:
            soname, resolved = match.groups()
            expected_soname = PRIVATE_SONAMES[soname.split(".so", 1)[0]]
            if soname != expected_soname:
                raise ValueError(f"unexpected OpenZFS SONAME {soname}; expected {expected_soname}")
            source = Path(resolved).resolve(strict=True)
            destination = runtime / soname
            if destination.exists():
                continue
            shutil.copyfile(source, destination)
    for name in PRIVATE:
        if not (runtime / PRIVATE_SONAMES[name]).is_file():
            raise ValueError(f"ldd did not resolve required OpenZFS private library {name}")

    copyright_candidates = [
        Path("/usr/share/doc/libzpool5linux/copyright"),
        Path("/usr/share/doc/libzpool5/copyright"),
        Path("/usr/share/doc/libzfs4linux/copyright"),
        Path("/usr/share/doc/libzfs4/copyright"),
    ]
    copyright = next((candidate for candidate in copyright_candidates if candidate.is_file()), None)
    if copyright is None:
        raise FileNotFoundError("OpenZFS development package copyright/license file not found")
    license_path = target_dir / "zfs-LICENSE"
    shutil.copyfile(copyright, license_path)
    bundled_license = target_dir / "zfs-openzfs-LICENSE"
    shutil.copyfile(upstream_license, bundled_license)
    files = [output, license_path, bundled_license, *sorted(runtime.iterdir())]
    version_output = run([str(output), "--version"])
    if version_output != f"neutrasearch-zfs wire=1 openzfs={VERSION}":
        raise ValueError(f"reader reported unexpected version: {version_output!r}")
    manifest = {
        "openzfs_version": VERSION,
        "reader": output.name,
        "vendor_source": {
            "commit": "494aaaed89cb9fe9f2da3b6c6f465a4bc9f6a7e1",
            "files": vendor_hashes,
        },
        "files": {path.relative_to(target_dir).as_posix(): sha256(path) for path in files},
    }
    (target_dir / "zfs-manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"built {output}; staged {len(tuple(runtime.iterdir()))} private libraries")
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project-root", type=Path, default=Path.cwd())
    parser.add_argument("--target-dir", type=Path, default=Path("target/release"))
    args = parser.parse_args()
    build(args.project_root, args.target_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
