#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
lanes=(
  crates/neutra-btrfs
  crates/neutra-ntfs
  crates/neutra-ext4
  crates/neutra-zfs
  crates/neutra-macos
  crates/neutra-helper/src
)
if rg -n 'std::fs::read_dir|WalkDir|walkdir::|globwalk|jwalk|\b(opendir|readdir|readdir64|scandir|nftw|fts_open)\s*\(' "${lanes[@]}" -g '*.rs' -g '*.c' -g '*.h'; then
  echo "filesystem-walking API found in a native lane" >&2
  exit 1
fi
echo "no filesystem-walking APIs found in native lanes"
