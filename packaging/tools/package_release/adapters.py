"""Filesystem, archive, checksum, and npm package adapters."""
from __future__ import annotations
import gzip
import hashlib
import io
import json
import os
import shutil
import stat
import tarfile
import zipfile
from pathlib import Path
from .domain import BINARIES, DOCUMENTS, NPM_TARGETS, TARGETS, helper_artifact_name, sha256_bytes, sha256_file, validated_version

def source_epoch() -> int:
    raw = os.environ.get("SOURCE_DATE_EPOCH", "0")
    try:
        epoch = int(raw)
    except ValueError as error:
        raise ValueError("SOURCE_DATE_EPOCH must be an integer") from error
    if epoch < 0:
        raise ValueError("SOURCE_DATE_EPOCH must not be negative")
    return epoch


def regular_file_bytes(path: Path) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"required regular file is missing: {path}")
    return path.read_bytes()


def zfs_payload(target_dir: Path) -> list[tuple[str, bytes, int]]:
    manifest_path = target_dir / "zfs-manifest.json"
    manifest = json.loads(regular_file_bytes(manifest_path))
    if manifest.get("openzfs_version") != "2.2.2" or manifest.get("reader") != "neutrasearch-zfs":
        raise ValueError(f"invalid pinned OpenZFS manifest: {manifest_path}")
    files = manifest.get("files")
    required = {
        "neutrasearch-zfs",
        "zfs-LICENSE",
        "zfs-openzfs-LICENSE",
    }
    if not isinstance(files, dict) or not required.issubset(files):
        raise ValueError(f"OpenZFS manifest is incomplete: {manifest_path}")
    required_private = {
        "libzpool.so.5", "libzfs.so.4", "libzfs_core.so.3",
        "libnvpair.so.3", "libuutil.so.3",
    }
    runtime = {Path(name).name for name in files if name.startswith("zfs-runtime/")}
    if runtime != required_private:
        raise ValueError("OpenZFS runtime manifest must contain exactly the five pinned private SONAMEs")
    payload = []
    for relative, expected in sorted(files.items()):
        if not isinstance(relative, str) or not (relative in required or relative.startswith("zfs-runtime/")):
            raise ValueError(f"unexpected OpenZFS payload path: {relative!r}")
        source = target_dir / relative
        data = regular_file_bytes(source)
        if sha256_bytes(data) != expected:
            raise ValueError(f"OpenZFS payload hash mismatch: {relative}")
        mode = 0o755 if relative == "neutrasearch-zfs" or relative.startswith("zfs-runtime/") else 0o644
        payload.append((relative, data, mode))
    if len(payload) != len(files):
        raise ValueError("OpenZFS payload manifest contains duplicate files")
    payload.append(("zfs-manifest.json", regular_file_bytes(manifest_path), 0o644))
    return payload


def archive_payload(project_root: Path, target_dir: Path, target: str) -> list[tuple[str, bytes, int]]:
    try:
        _, _, executable_suffix = TARGETS[target]
    except KeyError as error:
        raise ValueError(f"unsupported release target: {target}") from error

    payload: list[tuple[str, bytes, int]] = []
    for source_name, archive_name in DOCUMENTS:
        data = regular_file_bytes(project_root / source_name)
        payload.append((archive_name, data, 0o644))
    for binary in BINARIES:
        filename = binary + executable_suffix
        data = regular_file_bytes(target_dir / filename)
        payload.append((filename, data, 0o755))

    helper_data = regular_file_bytes(target_dir / ("neutrasearch-helper" + executable_suffix))
    helper_name = helper_artifact_name(target)
    payload.append((f"helpers/{helper_name}", helper_data, 0o755))
    payload.append(
        (
            f"helpers/{helper_name}.sha256",
            f"{sha256_bytes(helper_data)}  {helper_name}\n".encode("utf-8"),
            0o644,
        )
    )

    if TARGETS[target][0] == "Linux":
        payload.extend(zfs_payload(target_dir))

    sums = "".join(
        f"{sha256_bytes(data)}  {name}\n" for name, data, _ in sorted(payload)
    ).encode("utf-8")
    payload.append(("SHA256SUMS", sums, 0o644))
    return sorted(payload)


def npm_platform_package(
    project_root: Path,
    target_dir: Path,
    output_dir: Path,
    target: str,
    version: str,
) -> Path:
    validated_version(version)
    try:
        package_name, npm_os, npm_cpu = NPM_TARGETS[target]
        _, _, executable_suffix = TARGETS[target]
    except KeyError as error:
        raise ValueError(f"unsupported npm release target: {target}") from error

    package_dir = output_dir / package_name
    if package_dir.exists():
        shutil.rmtree(package_dir)
    binary_dir = package_dir / "bin"
    binary_dir.mkdir(parents=True)
    checksums: list[str] = []
    binary_links: dict[str, str] = {}
    built_binaries: dict[str, bytes] = {}
    for binary in BINARIES:
        filename = binary + executable_suffix
        source = target_dir / filename
        data = regular_file_bytes(source)
        built_binaries[binary] = data
        destination = binary_dir / filename
        destination.write_bytes(data)
        destination.chmod(0o755)
        checksums.append(f"{sha256_bytes(data)}  bin/{filename}\n")
        binary_links[binary] = f"bin/{filename}"

    helper_dir = binary_dir / "helpers"
    helper_dir.mkdir()
    helper_name = helper_artifact_name(target)
    helper_data = built_binaries["neutrasearch-helper"]
    helper_path = helper_dir / helper_name
    helper_path.write_bytes(helper_data)
    helper_path.chmod(0o755)
    helper_digest = sha256_bytes(helper_data)
    (helper_dir / f"{helper_name}.sha256").write_text(
        f"{helper_digest}  {helper_name}\n", encoding="utf-8"
    )
    checksums.extend(
        [
            f"{helper_digest}  bin/helpers/{helper_name}\n",
            f"{sha256_bytes((helper_dir / f'{helper_name}.sha256').read_bytes())}  bin/helpers/{helper_name}.sha256\n",
        ]
    )

    if TARGETS[target][0] == "Linux":
        for name, data, mode in zfs_payload(target_dir):
            destination = binary_dir / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            destination.chmod(mode)
            checksums.append(f"{sha256_bytes(data)}  bin/{name}\n")

    metadata = {
        "name": package_name,
        "version": version,
        "description": f"Neutrasearch native binaries for {npm_os}-{npm_cpu}",
        "license": "MIT",
        "author": "NetroAki",
        "os": [npm_os],
        "cpu": [npm_cpu],
        "preferUnplugged": True,
        "files": ["bin", "SHA256SUMS", "README.md", "LICENSE"],
        "bin": binary_links,
        "repository": {
            "type": "git",
            "url": "git+https://github.com/NetroAki/neutrasearch.git",
        },
    }
    (package_dir / "package.json").write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (package_dir / "SHA256SUMS").write_text("".join(sorted(checksums)), encoding="utf-8")
    (package_dir / "README.md").write_text(
        f"# {package_name}\n\n"
        "Internal native binary package for `pi-neutrasearch`. Install "
        "`pi-neutrasearch` rather than this package directly.\n\n"
        "Installation only places executables; indexing still requires explicit "
        "approval in the Neutrasearch GUI.\n",
        encoding="utf-8",
    )
    shutil.copyfile(project_root / "LICENSE", package_dir / "LICENSE")
    print(f"prepared {package_name}@{version} for {target}: {package_dir}")
    return package_dir


def write_zip(path: Path, root_name: str, payload: list[tuple[str, bytes, int]], epoch: int) -> None:
    # ZIP timestamps cannot represent dates before 1980-01-01.
    import time

    timestamp = time.gmtime(max(epoch, 315532800))[:6]
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, data, mode in payload:
            info = zipfile.ZipInfo(f"{root_name}/{name}", date_time=timestamp)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | mode) << 16
            archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)


def write_tar_gz(path: Path, root_name: str, payload: list[tuple[str, bytes, int]], epoch: int) -> None:
    with path.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for name, data, mode in payload:
                    info = tarfile.TarInfo(f"{root_name}/{name}")
                    info.size = len(data)
                    info.mode = mode
                    info.mtime = epoch
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    archive.addfile(info, io.BytesIO(data))


def package(
    project_root: Path,
    target_dir: Path,
    output_dir: Path,
    target: str,
    version: str,
    archive_format: str,
) -> Path:
    validated_version(version)
    if target not in TARGETS:
        raise ValueError(f"unsupported release target: {target}")
    expected_format = "zip" if TARGETS[target][0] == "Windows" else "tar.gz"
    if archive_format != expected_format:
        raise ValueError(f"target {target} must use {expected_format}, not {archive_format}")

    project_root = project_root.resolve()
    target_dir = target_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    root_name = f"neutrasearch-{version}-{target}"
    extension = ".zip" if archive_format == "zip" else ".tar.gz"
    output = output_dir.resolve() / f"{root_name}{extension}"
    payload = archive_payload(project_root, target_dir, target)
    epoch = source_epoch()
    if archive_format == "zip":
        write_zip(output, root_name, payload, epoch)
    else:
        write_tar_gz(output, root_name, payload, epoch)
    print(output)
    return output


def write_checksums(input_dir: Path, output: Path) -> None:
    release_suffixes = (".zip", ".tar.gz", ".exe", ".deb", ".dmg")
    artifacts = sorted(
        path
        for path in input_dir.iterdir()
        if path.is_file() and path.name.endswith(release_suffixes)
    )
    if not artifacts:
        raise ValueError(f"no release artifacts found in {input_dir}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        "".join(f"{sha256_file(path)}  {path.name}\n" for path in artifacts),
        encoding="utf-8",
        newline="\n",
    )
    print(output)



def archive_files(path: Path) -> dict[str, bytes]:
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as archive:
            return {name: archive.read(name) for name in archive.namelist() if not name.endswith("/")}
    with tarfile.open(path, "r:gz") as archive:
        return {
            member.name: archive.extractfile(member).read()  # type: ignore[union-attr]
            for member in archive.getmembers()
            if member.isfile()
        }


def verify_inner_checksums(files: dict[str, bytes], root_name: str) -> None:
    checksum_name = f"{root_name}/SHA256SUMS"
    lines = files[checksum_name].decode("utf-8").splitlines()
    for line in lines:
        digest, name = line.split("  ", 1)
        actual = sha256_bytes(files[f"{root_name}/{name}"])
        if actual != digest:
            raise AssertionError(f"inner checksum mismatch for {name}")
