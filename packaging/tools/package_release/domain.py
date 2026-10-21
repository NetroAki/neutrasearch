"""Release target rules and validation."""
from __future__ import annotations
import hashlib
import platform
import re
import tomllib
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
    ("CHANGELOG.md", "CHANGELOG.md"),
    ("THIRD-PARTY-NOTICES.txt", "THIRD-PARTY-NOTICES.txt"),
)
TARGETS = {
    "x86_64-unknown-linux-gnu": ("Linux", "x86_64", ""),
    "aarch64-unknown-linux-gnu": ("Linux", "aarch64", ""),
    "x86_64-pc-windows-msvc": ("Windows", "x86_64", ".exe"),
    "x86_64-apple-darwin": ("Darwin", "x86_64", ""),
    "aarch64-apple-darwin": ("Darwin", "aarch64", ""),
}
NPM_TARGETS = {
    "x86_64-unknown-linux-gnu": ("neutrasearch-linux-x64", "linux", "x64"),
    "aarch64-unknown-linux-gnu": ("neutrasearch-linux-arm64", "linux", "arm64"),
    "x86_64-pc-windows-msvc": ("neutrasearch-windows-x64", "win32", "x64"),
    "x86_64-apple-darwin": ("neutrasearch-darwin-x64", "darwin", "x64"),
    "aarch64-apple-darwin": ("neutrasearch-darwin-arm64", "darwin", "arm64"),
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


def helper_artifact_name(target: str) -> str:
    system, machine, suffix = TARGETS[target]
    os_name = {"Linux": "linux", "Windows": "windows", "Darwin": "macos"}[system]
    return f"neutrasearch-helper-{os_name}-{machine}{suffix}"
