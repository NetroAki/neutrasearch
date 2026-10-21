#!/usr/bin/env python3
"""Build deterministic, dependency-free Neutrasearch portable archives."""
from __future__ import annotations
import argparse
import tarfile
import zipfile
from pathlib import Path
from .domain import (BINARIES, DOCUMENTS, TARGETS, NPM_TARGETS, SEMVER, sha256_bytes, sha256_file, validated_version, workspace_version, check_tag, normalized_machine, verify_host, helper_artifact_name)
from .adapters import (source_epoch, regular_file_bytes, archive_payload, npm_platform_package, write_zip, write_tar_gz, package, write_checksums, archive_files, verify_inner_checksums)
from .tests import self_test

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

    npm_package = commands.add_parser(
        "npm-platform", help="prepare one OS/CPU-specific npm binary package"
    )
    npm_package.add_argument("--project-root", type=Path, default=Path("."))
    npm_package.add_argument("--target-dir", type=Path, required=True)
    npm_package.add_argument("--output-dir", type=Path, default=Path("dist/npm"))
    npm_package.add_argument("--target", required=True, choices=sorted(NPM_TARGETS))
    npm_package.add_argument("--version", required=True)

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
        elif args.command == "npm-platform":
            npm_platform_package(
                args.project_root,
                args.target_dir,
                args.output_dir,
                args.target,
                args.version,
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
