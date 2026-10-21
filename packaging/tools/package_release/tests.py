"""Release packaging self-test fixtures and verification."""
from __future__ import annotations
import json
import os
import tempfile
import zipfile
import tarfile
from pathlib import Path
from .domain import BINARIES, DOCUMENTS, NPM_TARGETS, TARGETS, check_tag, helper_artifact_name, sha256_file
from .adapters import archive_files, package, npm_platform_package, verify_inner_checksums, write_checksums
from .zfs_tests import run_zfs_tests

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
            if target.endswith("-unknown-linux-gnu"):
                from .zfs_tests import fixture

                fixture(built)
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
            if target.endswith("-unknown-linux-gnu"):
                from .zfs_tests import PRIVATE, LICENSES

                expected |= {
                    f"{root_name}/neutrasearch-zfs",
                    f"{root_name}/zfs-manifest.json",
                    *(f"{root_name}/{name}" for name in LICENSES),
                    *(f"{root_name}/zfs-runtime/{name}" for name in PRIVATE),
                }
            if set(files) != expected:
                raise AssertionError(f"unexpected archive manifest for {target}")
            verify_inner_checksums(files, root_name)

            npm_dir = npm_platform_package(
                project, built, root / "npm", target, "1.2.3-rc.1"
            )
            npm_metadata = json.loads((npm_dir / "package.json").read_text(encoding="utf-8"))
            expected_name, expected_os, expected_cpu = NPM_TARGETS[target]
            if (
                npm_metadata["name"],
                npm_metadata["os"],
                npm_metadata["cpu"],
            ) != (expected_name, [expected_os], [expected_cpu]):
                raise AssertionError(f"incorrect npm platform metadata for {target}")
            npm_expected = {
                binary + suffix for binary in BINARIES
            }
            if target.endswith("-unknown-linux-gnu"):
                npm_expected |= {"neutrasearch-zfs", "zfs-manifest.json", *LICENSES}
            if {path.name for path in (npm_dir / "bin").iterdir() if path.is_file()} != npm_expected:
                raise AssertionError(f"incorrect npm binary manifest for {target}")
            helper = npm_dir / "bin" / "helpers" / helper_artifact_name(target)
            if not helper.is_file() or not Path(f"{helper}.sha256").is_file():
                raise AssertionError(f"missing npm remote helper artifact for {target}")
            expected_checksums = len(BINARIES) + 2
            if target.endswith("-unknown-linux-gnu"):
                expected_checksums += len(PRIVATE) + len(LICENSES) + 2
            if len((npm_dir / "SHA256SUMS").read_text().splitlines()) != expected_checksums:
                raise AssertionError(f"incorrect npm checksums for {target}")

        zfs_target = root / "zfs-built"
        zfs_target.mkdir()
        for binary in BINARIES:
            (zfs_target / binary).write_bytes(binary.encode())
        run_zfs_tests(root, project, zfs_target)

        installer_names = (
            "neutrasearch-1.2.3-rc.1-windows-x64-setup.exe",
            "neutrasearch-1.2.3-rc.1-linux-amd64.deb",
            "neutrasearch-1.2.3-rc.1-linux-arm64.deb",
            "neutrasearch-1.2.3-rc.1-macos-x64.dmg",
            "neutrasearch-1.2.3-rc.1-macos-arm64.dmg",
        )
        for name in installer_names:
            (root / "one" / name).write_bytes(name.encode("utf-8"))
        checksum_file = root / "one" / "SHA256SUMS"
        write_checksums(root / "one", checksum_file)
        expected_artifacts = len(TARGETS) + len(installer_names)
        if len(checksum_file.read_text(encoding="utf-8").splitlines()) != expected_artifacts:
            raise AssertionError("release checksum manifest has the wrong artifact count")
    print("package_release.py self-test passed")
