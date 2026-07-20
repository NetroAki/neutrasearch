#!/usr/bin/env python3
"""Build deterministic, dependency-free Neutrasearch portable archives."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import os
import platform
import re
import shutil
import stat
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path

BINARIES = (
    "neutrasearch",
    "neutrasearch-helper",
    "neutrasearch-mcp",
    "neutrasearch-query",
)
DOCUMENTS = (
    ("README.md", "README.md"),
    ("LICENSE", "LICENSE"),
    ("SECURITY.md", "SECURITY.md"),
    ("CHANGELOG.md", "CHANGELOG.md"),
    ("docs/production.md", "docs/production.md"),
)
TARGETS = {
    "x86_64-unknown-linux-gnu": ("Linux", "x86_64", ""),
    "aarch64-unknown-linux-gnu": ("Linux", "aarch64", ""),
    "x86_64-pc-windows-msvc": ("Windows", "x86_64", ".exe"),
    "x86_64-apple-darwin": ("Darwin", "x86_64", ""),
    "aarch64-apple-darwin": ("Darwin", "aarch64", ""),
}
SEMVER = re.compile(
    r"^(?P<major>0|[1-9][0-9]*)\."
    r"(?P<minor>0|[1-9][0-9]*)\."
    r"(?P<patch>0|[1-9][0-9]*)"
    r"(?:-(?P<pre>(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+(?P<build>[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def validated_version(value: str) -> re.Match[str]:
    match = SEMVER.fullmatch(value)
    if match is None:
        raise ValueError(f"not a strict SemVer version: {value!r}")
    return match


def workspace_version(cargo_toml: Path) -> str:
    with cargo_toml.open("rb") as source:
        data = tomllib.load(source)
    try:
        version = data["workspace"]["package"]["version"]
    except (KeyError, TypeError) as error:
        raise ValueError("Cargo.toml has no [workspace.package] version") from error
    if not isinstance(version, str):
        raise ValueError("Cargo workspace version is not a string")
    validated_version(version)
    return version


def check_tag(tag: str, cargo_toml: Path, github_output: Path | None) -> None:
    if not tag.startswith("v") or tag.startswith("vv"):
        raise ValueError("release tag must be exactly v followed by a SemVer version")
    tagged_version = tag[1:]
    match = validated_version(tagged_version)
    cargo_version = workspace_version(cargo_toml)
    if tagged_version != cargo_version:
        raise ValueError(
            f"tag {tag!r} does not equal Cargo workspace version {cargo_version!r}"
        )
    prerelease = "true" if match.group("pre") is not None else "false"
    if github_output is not None:
        with github_output.open("a", encoding="utf-8", newline="\n") as output:
            output.write(f"version={tagged_version}\n")
            output.write(f"prerelease={prerelease}\n")
    print(f"validated tag {tag} (prerelease={prerelease})")


def normalized_machine(machine: str) -> str:
    value = machine.strip().lower()
    if value in {"amd64", "x64", "x86_64"}:
        return "x86_64"
    if value in {"arm64", "aarch64"}:
        return "aarch64"
    return value


def verify_host(target: str) -> None:
    try:
        expected_system, expected_machine, _ = TARGETS[target]
    except KeyError as error:
        raise ValueError(f"unsupported release target: {target}") from error
    actual_system = platform.system()
    actual_machine = normalized_machine(platform.machine())
    if (actual_system, actual_machine) != (expected_system, expected_machine):
        raise ValueError(
            f"target {target} requires native {expected_system}/{expected_machine}, "
            f"runner is {actual_system}/{actual_machine}"
        )
    print(f"verified native runner {actual_system}/{actual_machine} for {target}")


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


def helper_artifact_name(target: str) -> str:
    system, machine, suffix = TARGETS[target]
    os_name = {"Linux": "linux", "Windows": "windows", "Darwin": "macos"}[system]
    return f"neutrasearch-helper-{os_name}-{machine}{suffix}"


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

    sums = "".join(
        f"{sha256_bytes(data)}  {name}\n" for name, data, _ in sorted(payload)
    ).encode("utf-8")
    payload.append(("SHA256SUMS", sums, 0o644))
    return sorted(payload)


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
    archives = sorted(
        path
        for path in input_dir.iterdir()
        if path.is_file() and (path.name.endswith(".zip") or path.name.endswith(".tar.gz"))
    )
    if not archives:
        raise ValueError(f"no release archives found in {input_dir}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        "".join(f"{sha256_file(path)}  {path.name}\n" for path in archives),
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


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="neutrasearch-package-test-") as temporary:
        root = Path(temporary)
        project = root / "project"
        project.mkdir()
        for source_name, _ in DOCUMENTS:
            path = project / source_name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"fixture for {source_name}\n", encoding="utf-8")
        (project / "Cargo.toml").write_text(
            '[workspace]\n[workspace.package]\nversion = "1.2.3-rc.1"\n',
            encoding="utf-8",
        )
        check_tag("v1.2.3-rc.1", project / "Cargo.toml", None)
        for invalid in ("1.2.3", "v01.2.3", "v1.2.3-01"):
            try:
                check_tag(invalid, project / "Cargo.toml", None)
            except ValueError:
                pass
            else:
                raise AssertionError(f"invalid tag accepted: {invalid}")

        os.environ["SOURCE_DATE_EPOCH"] = "1700000000"
        built = root / "built"
        built.mkdir()
        for target, (_, _, suffix) in TARGETS.items():
            for binary in BINARIES:
                (built / (binary + suffix)).write_bytes(f"{target}:{binary}\n".encode())
            archive_format = "zip" if suffix == ".exe" else "tar.gz"
            first = package(project, built, root / "one", target, "1.2.3-rc.1", archive_format)
            second = package(project, built, root / "two", target, "1.2.3-rc.1", archive_format)
            if sha256_file(first) != sha256_file(second):
                raise AssertionError(f"non-deterministic archive for {target}")
            root_name = f"neutrasearch-1.2.3-rc.1-{target}"
            files = archive_files(first)
            helper_name = helper_artifact_name(target)
            expected = {
                f"{root_name}/{name}" for _, name in DOCUMENTS
            } | {
                f"{root_name}/{binary}{suffix}" for binary in BINARIES
            } | {
                f"{root_name}/helpers/{helper_name}",
                f"{root_name}/helpers/{helper_name}.sha256",
                f"{root_name}/SHA256SUMS",
            }
            if set(files) != expected:
                raise AssertionError(f"unexpected archive manifest for {target}")
            verify_inner_checksums(files, root_name)

        checksum_file = root / "one" / "SHA256SUMS"
        write_checksums(root / "one", checksum_file)
        if len(checksum_file.read_text(encoding="utf-8").splitlines()) != len(TARGETS):
            raise AssertionError("release checksum manifest has the wrong archive count")
    print("package_release.py self-test passed")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)

    tag = commands.add_parser("check-tag", help="validate vTAG against Cargo workspace version")
    tag.add_argument("--tag", required=True)
    tag.add_argument("--cargo-toml", type=Path, default=Path("Cargo.toml"))
    tag.add_argument("--github-output", type=Path)

    host = commands.add_parser("verify-host", help="require a native runner for a release target")
    host.add_argument("--target", required=True, choices=sorted(TARGETS))

    pack = commands.add_parser("package", help="create one portable release archive")
    pack.add_argument("--project-root", type=Path, default=Path("."))
    pack.add_argument("--target-dir", type=Path, required=True)
    pack.add_argument("--output-dir", type=Path, default=Path("dist"))
    pack.add_argument("--target", required=True, choices=sorted(TARGETS))
    pack.add_argument("--version", required=True)
    pack.add_argument("--format", required=True, choices=("zip", "tar.gz"))

    sums = commands.add_parser("checksums", help="write SHA256SUMS for release archives")
    sums.add_argument("--input-dir", type=Path, required=True)
    sums.add_argument("--output", type=Path, required=True)

    commands.add_parser("self-test", help="exercise tag validation and both archive formats")
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "check-tag":
            check_tag(args.tag, args.cargo_toml, args.github_output)
        elif args.command == "verify-host":
            verify_host(args.target)
        elif args.command == "package":
            package(
                args.project_root,
                args.target_dir,
                args.output_dir,
                args.target,
                args.version,
                args.format,
            )
        elif args.command == "checksums":
            write_checksums(args.input_dir, args.output)
        else:
            self_test()
    except (OSError, ValueError, KeyError, tarfile.TarError, zipfile.BadZipFile) as error:
        parser().error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
