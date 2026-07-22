#!/usr/bin/env python3
"""Build native desktop installers on their matching release runners."""

from __future__ import annotations

import argparse
import gzip
import io
import os
import plistlib
import shutil
import stat
import subprocess
import tarfile
import tempfile
from pathlib import Path

LINUX_ARCH = {
    "x86_64-unknown-linux-gnu": "amd64",
    "aarch64-unknown-linux-gnu": "arm64",
}
MAC_ARCH = {
    "x86_64-apple-darwin": "x64",
    "aarch64-apple-darwin": "arm64",
}
BINARIES = ("neutrasearch", "neutrasearch-helper", "neutrasearch-query", "neutrasearch-mcp")


def require_file(path: Path) -> Path:
    path = path.resolve()
    if not path.is_file():
        raise FileNotFoundError(path)
    return path


def copy_executable(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(require_file(source), destination)
    destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def tar_gz_tree(root: Path, *, exclude: str | None = None) -> bytes:
    raw = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=315532800, compresslevel=9) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for path in sorted(root.rglob("*")):
                relative = path.relative_to(root)
                if exclude and relative.parts and relative.parts[0] == exclude:
                    continue
                if not relative.parts:
                    continue
                info = tarfile.TarInfo(f"./{relative.as_posix()}")
                info.uid = 0
                info.gid = 0
                info.uname = "root"
                info.gname = "root"
                info.mtime = 315532800
                info.mode = stat.S_IMODE(path.stat().st_mode)
                if path.is_dir():
                    info.type = tarfile.DIRTYPE
                    archive.addfile(info)
                elif path.is_file():
                    data = path.read_bytes()
                    info.size = len(data)
                    archive.addfile(info, io.BytesIO(data))
    return raw.getvalue()


def write_ar(path: Path, members: list[tuple[str, bytes]]) -> None:
    with path.open("wb") as archive:
        archive.write(b"!<arch>\n")
        for name, data in members:
            if len(name) > 15:
                raise ValueError(f"ar member name is too long: {name}")
            header = (
                f"{name + '/':<16}"
                f"{315532800:<12}"
                f"{0:<6}"
                f"{0:<6}"
                f"{'100644':<8}"
                f"{len(data):<10}`\n"
            ).encode("ascii")
            if len(header) != 60:
                raise AssertionError(f"invalid ar header length for {name}: {len(header)}")
            archive.write(header)
            archive.write(data)
            if len(data) % 2:
                archive.write(b"\n")


def build_deb(project_root: Path, target_dir: Path, output_dir: Path, target: str, version: str) -> Path:
    try:
        architecture = LINUX_ARCH[target]
    except KeyError as error:
        raise ValueError(f"unsupported Debian target: {target}") from error
    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir.resolve() / f"neutrasearch-{version}-linux-{architecture}.deb"
    with tempfile.TemporaryDirectory(prefix="neutrasearch-deb-") as temporary:
        root = Path(temporary) / "root"
        control = root / "DEBIAN"
        control.mkdir(parents=True)
        (control / "control").write_text(
            "\n".join(
                [
                    "Package: neutrasearch",
                    f"Version: {version}",
                    f"Architecture: {architecture}",
                    "Maintainer: NetroAki",
                    "Section: utils",
                    "Priority: optional",
                    "Depends: libext2fs2",
                    "Homepage: https://github.com/NetroAki/neutrasearch",
                    "Description: Fast native-metadata filename and folder search",
                    " Neutrasearch builds a compact searchable index from filesystem metadata.",
                    "",
                ]
            ),
            encoding="utf-8",
        )

        for binary in ("neutrasearch", "neutrasearch-query", "neutrasearch-mcp"):
            copy_executable(target_dir / binary, root / "usr/bin" / binary)
        copy_executable(
            target_dir / "neutrasearch-helper",
            root / "usr/lib/neutrasearch/neutrasearch-helper",
        )

        applications = root / "usr/share/applications"
        applications.mkdir(parents=True)
        (applications / "neutrasearch.desktop").write_text(
            "\n".join(
                [
                    "[Desktop Entry]",
                    "Type=Application",
                    "Name=Neutrasearch",
                    "Comment=Fast filename and folder search",
                    "Exec=neutrasearch",
                    "Icon=neutrasearch",
                    "Terminal=false",
                    "Categories=Utility;FileTools;",
                    "StartupNotify=true",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        icon = root / "usr/share/icons/hicolor/scalable/apps/neutrasearch.svg"
        icon.parent.mkdir(parents=True)
        shutil.copyfile(require_file(project_root / "assets/neutrasearch.svg"), icon)
        docs = root / "usr/share/doc/neutrasearch"
        docs.mkdir(parents=True)
        for name in ("README.md", "LICENSE", "SECURITY.md", "CHANGELOG.md"):
            shutil.copyfile(require_file(project_root / name), docs / name)

        control_archive = tar_gz_tree(control)
        data_archive = tar_gz_tree(root, exclude="DEBIAN")
        write_ar(
            output,
            [
                ("debian-binary", b"2.0\n"),
                ("control.tar.gz", control_archive),
                ("data.tar.gz", data_archive),
            ],
        )
    print(output)
    return output


def build_dmg(project_root: Path, target_dir: Path, output_dir: Path, target: str, version: str) -> Path:
    try:
        architecture = MAC_ARCH[target]
    except KeyError as error:
        raise ValueError(f"unsupported macOS target: {target}") from error
    if shutil.which("hdiutil") is None:
        raise RuntimeError("hdiutil is required to build the macOS installer")

    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir.resolve() / f"neutrasearch-{version}-macos-{architecture}.dmg"
    with tempfile.TemporaryDirectory(prefix="neutrasearch-dmg-") as temporary:
        image_root = Path(temporary) / "image"
        app = image_root / "Neutrasearch.app"
        macos = app / "Contents/MacOS"
        resources = app / "Contents/Resources"
        resources.mkdir(parents=True)
        for binary in BINARIES:
            copy_executable(target_dir / binary, macos / binary)
        shutil.copyfile(
            require_file(project_root / "packages/pi-neutrasearch/assets/Neutrasearch.icns"),
            resources / "Neutrasearch.icns",
        )
        shutil.copyfile(require_file(project_root / "LICENSE"), resources / "LICENSE")
        info = {
            "CFBundleDevelopmentRegion": "en",
            "CFBundleDisplayName": "Neutrasearch",
            "CFBundleExecutable": "neutrasearch",
            "CFBundleIconFile": "Neutrasearch",
            "CFBundleIdentifier": "dev.netroaki.neutrasearch",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": "Neutrasearch",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "LSMinimumSystemVersion": "11.0",
            "NSHighResolutionCapable": True,
        }
        with (app / "Contents/Info.plist").open("wb") as stream:
            plistlib.dump(info, stream, sort_keys=True)
        os.symlink("/Applications", image_root / "Applications")
        subprocess.run(
            [
                "hdiutil",
                "create",
                "-volname",
                "Neutrasearch",
                "-srcfolder",
                str(image_root),
                "-ov",
                "-format",
                "UDZO",
                str(output),
            ],
            check=True,
        )
    print(output)
    return output


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    subparsers = result.add_subparsers(dest="command", required=True)
    for command in ("linux-deb", "macos-dmg"):
        command_parser = subparsers.add_parser(command)
        command_parser.add_argument("--project-root", type=Path, required=True)
        command_parser.add_argument("--target-dir", type=Path, required=True)
        command_parser.add_argument("--output-dir", type=Path, required=True)
        command_parser.add_argument("--target", required=True)
        command_parser.add_argument("--version", required=True)
    return result


def main() -> int:
    args = parser().parse_args()
    project_root = args.project_root.resolve()
    target_dir = args.target_dir.resolve()
    if args.command == "linux-deb":
        build_deb(project_root, target_dir, args.output_dir, args.target, args.version)
    else:
        build_dmg(project_root, target_dir, args.output_dir, args.target, args.version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
