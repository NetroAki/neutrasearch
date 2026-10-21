#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
lanes=(
  crates/neutra-btrfs
  crates/neutra-ntfs
  crates/neutra-ext4
  crates/neutra-zfs
  crates/neutra-macos
  crates/neutra-helper/src
)
status=0
rg -n 'std::fs::read_dir|WalkDir|walkdir::|globwalk|jwalk|\b(opendir|readdir|readdir64|scandir|nftw|fts_open)\s*\(' "${lanes[@]}" -g '*.rs' -g '*.c' -g '*.h' || status=$?
if (( status == 0 )); then
  echo "filesystem-walking API found in a native lane" >&2
  exit 1
fi
if (( status != 1 )); then
  echo "could not verify native lanes (ripgrep exit $status)" >&2
  exit "$status"
fi
echo "no filesystem-walking APIs found in native lanes"
