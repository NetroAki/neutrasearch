# Neutrasearch

A cross-platform filename/metadata searcher built in Rust with `egui` + [`egui_expressive`](https://github.com/NetroAki/egui_expressive). Neutrasearch follows the Everything/FSearch model: build a compact namespace index once, maintain it incrementally, then answer queries without touching the filesystem. Query latency is benchmarked rather than assumed.

The native lanes are deliberately forbidden from VFS directory walking.

## UX

- one Everything-style search field
- dense, virtualized filename/path result table
- inline filters rather than an advanced-settings maze
- one rescan action and explicit per-lane permission/error state
- resizable bottom **Size Map**: drag its top edge, click its heading to collapse, and click WinDirStat-like extension blocks to filter results

Query examples:

```text
report
ext:rs,toml under:/home/zac/projects
kind:dir photos
fs:btrfs size:>1G
```

## Native lanes

| Host | Filesystem | Initial namespace source | Walking fallback |
|---|---|---|---|
| Linux | Btrfs | `BTRFS_IOC_TREE_SEARCH` inode items/refs | Never |
| Linux | EXT2/3/4 | libext2fs inode table + directory blocks | Never |
| Linux | NTFS | raw `$MFT`, including fragmented MFT data runs | Never |
| Windows | NTFS | raw volume `$MFT` parser | Never |
| Linux | ZFS | snapshot + DMU/ZAP through version-matched libzpool | Never; `zdb` parsing rejected |
| macOS | APFS/HFS+ | Spotlight (`mdfind`) | `getattrlistbulk`, never `readdir` |
| Network | NFS/SMB/SSHFS | helper auto-provisioned on the server over SSH | Never client-side |

### Current implementation status

- **EXT4:** implemented and exercised against a real generated ext4 image without mounting it.
- **NTFS:** raw parser, fragmented data runs, strict USA fixups, sequence-checked path reconstruction, multiple hardlink names, and unnamed `$DATA` sizes are implemented; synthetic tests pass. `$ATTRIBUTE_LIST` extension-record resolution remains incomplete. Windows raw-volume builds still need a Windows-host integration run; `FSCTL_ENUM_USN_DATA`/journal checkpoints are the planned fast initial/incremental path.
- **Btrfs:** mounted-subvolume TREE_SEARCH_V2 lane implemented with compact filename storage and parallel object-ID shards. On the approved `/home` volume it returned 12,391,241 records in 5.854 s; a serial run returned the exact same count in 14.647 s. This is one host result, not a universal speed claim. Nested unmounted subvolume coverage remains to be proven.
- **macOS:** Spotlight lane and `getattrlistbulk` fallback are implemented behind macOS cfg; they require macOS CI/host validation.
- **ZFS:** snapshot naming/diff parser is implemented. The initial ZAP scanner intentionally refuses to run unless exact-version OpenZFS/libzpool bindings are supplied. It does **not** claim completed ZFS initial indexing yet.
- The Linux NTFS target contains a 12.88 GiB `$MFT` on a spinning disk; its cold raw scan remains throughput-bound and exceeds 60 seconds. The intended steady state is a durable base plus USN-driven updates, not repeated MFT reads.
- The current bincode `index.bin` is transitional. The compact mmap trigram base + bounded delta format and resource budgets are specified in [`docs/index-format.md`](docs/index-format.md).

This distinction is intentional: an explicit error is safer than a hidden directory walk.

## Workspace

```text
crates/neutra-core     records, query engine, resident index, framed protocol
crates/neutra-btrfs    BTRFS_IOC_TREE_SEARCH lane
crates/neutra-ext4     libext2fs lane
crates/neutra-ntfs     cross-platform raw $MFT parser
crates/neutra-zfs      snapshot/diff + feature-gated libzpool lane
crates/neutra-macos    Spotlight/getattrlistbulk lane
crates/neutra-helper   cross-platform scanner helper
crates/neutra-remote   SSH detection/provisioning for network mounts
crates/neutra-gui      custom egui/egui_expressive desktop app
crates/neutra-mcp      persistent, compact MCP stdio server
crates/neutra-query    scriptable CLI + persistent NDJSON query API
pi-plugin              persistent Pi extension
```

## Build

Linux needs the libext2fs development package:

```sh
# Arch
sudo pacman -S e2fsprogs
# Debian/Ubuntu
sudo apt install libext2fs-dev

cargo build --release --workspace
cargo test --workspace
bash scripts/check-no-walk.sh
```

Windows and macOS do not link libext2fs because the EXT4 implementation is target-gated. Cross-platform CI and signed release packaging are planned; the current source has not yet passed every release-target integration run.

## Run

Place `neutrasearch` and `neutra-helper` together, then:

```sh
./target/release/neutrasearch
```

Raw block devices and Btrfs tree search need privilege on Linux. Packaging should use a narrow polkit rule/capability-managed helper; during development only, opt into a `pkexec` launch:

```sh
NEUTRA_PKEXEC=1 ./target/release/neutrasearch
```

Safe UI-only smoke mode (no scan and no remote SSH discovery):

```sh
NEUTRA_NO_AUTOSCAN=1 NEUTRA_NO_REMOTE=1 ./target/release/neutrasearch
```

Explicitly build a compact index from one mounted native lane (the output contains privacy-sensitive paths):

```sh
neutra-helper --build-index /mount/point /chosen/private/location/index.nsx
```

The GUI also publishes `index.nsx` after its first successful scan, then drops the decoded bulk records and searches the mmap store.

Run the durable query/update helper against an existing base with:

```sh
neutra-helper --serve-index /path/to/index.nsx
```

It opens the mmap base plus a generation-bound sibling `index.delta`, answers framed `Search` commands, and accepts framed `ApplyDelta` batches. An update acknowledgement is sent only after the WAL is synced. Platform event producers are still an active implementation area; this service command is infrastructure, not a claim that every OS watcher is complete.

## Network mounts

The GUI polls mounted NFS/CIFS/SSHFS volumes. On first sight it uses `ssh -o BatchMode=yes` and the user's existing SSH key/agent to:

1. detect Linux, Windows OpenSSH, or macOS and architecture;
2. compare the remote helper build number;
3. upload the matching prebuilt helper from `NEUTRA_HELPER_ARTIFACTS` when missing/stale;
4. report provisioning state in the lane list.

It never handles passwords and never falls back to scanning a network mount from the client. Packaging names are:

```text
neutra-helper-linux-x86_64
neutra-helper-linux-aarch64
neutra-helper-windows-x86_64.exe
neutra-helper-macos-x86_64
neutra-helper-macos-aarch64
```

Remote share-to-local-path mapping and merged remote result streaming are the next integration gate; provisioning exists, but the GUI does not yet over-index an entire server just because one share was mounted.

## MCP and Pi

`neutra-mcp` opens `index.nsx` once with mmap and stays available. If a generation-matched `index.delta` exists beside it, MCP and `neutra-query` merge its upserts/tombstones into every search. Each call is one newline JSON request and one compact response. Path-only output is the default, the default limit is 50, and metadata is opt-in. Clean mapped pages remain OS-reclaimable; legacy `index.bin` remains a migration fallback.

```json
{"name":"neutra_search","arguments":{"query":"parser ext:rs under:/src","limit":30,"metadata":false}}
```

The Pi extension keeps one MCP child alive for the Pi runtime—no per-call process spawn and no repeated index decode. See [`pi-plugin/README.md`](pi-plugin/README.md).

Use Neutrasearch instead of broad `find`, `rg --files`, or filename grep. Content grep remains appropriate after Neutrasearch narrows the files.

## Programming API

All interfaces search an existing index; none invokes a filesystem scan.

- Rust: `neutra_core::{CompactIndex, Query}` then `CompactIndex::open(path)?.search(&query)`.
- One-shot CLI: `neutra-query --json 'parser ext:rs under:/src'`.
- Persistent NDJSON: run `neutra-query --stdio`; send `{"query":"parser ext:rs","limit":30,"metadata":false}` per line.
- MCP: use `neutra_search` and `neutra_status` from any MCP client.

NDJSON responses contain `paths`, `matched`, `returned`, and `search_us`; optional records include kind, size, mtime, and filesystem.

## Cache and environment

- Linux: `$XDG_CACHE_HOME/neutrasearch/index.nsx` or `~/.cache/neutrasearch/index.nsx`
- macOS: `~/Library/Caches/Neutrasearch/index.nsx`
- Windows: `%LOCALAPPDATA%\Neutrasearch\index.nsx`
- Legacy `index.bin` files remain readable during migration.
- override: `NEUTRA_INDEX`
- helper binary: `NEUTRA_HELPER`
- remote helper artifacts: `NEUTRA_HELPER_ARTIFACTS`

Paths and filenames are privacy-sensitive. Neutrasearch performs no telemetry and has no hosted control plane. Remote SSH provisioning happens only for network mounts and uses hosts already present in those mount definitions.

## Release targets

Public releases are intended to include native builds for:

- Linux x86_64 and ARM64
- Windows x86_64 and ARM64
- macOS Intel and Apple Silicon

A target is not considered supported until its native scanner, installer, permissions flow, update path, and packaged application pass on real hardware or an equivalent native runner.

## Support

The About window displays the running package version and these optional support links:

- [Ko-fi](https://ko-fi.com/netroaki)
- [Patreon](https://www.patreon.com/NetroAki)

## License

Neutrasearch is available under the [MIT License](LICENSE). You may use, modify, distribute, sublicense, or sell copies, subject to preserving the license and copyright notice.
