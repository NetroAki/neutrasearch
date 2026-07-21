# Neutrasearch

Neutrasearch is a fast filename and folder search application written in Rust. It builds a compact index from native filesystem metadata and searches that index without repeatedly scanning directories.

Neutrasearch is pre-1.0. Linux, Windows, and macOS share the same GUI, compact-index, query, MCP, native initial-index, and manual-rebuild flows. Release confidence still differs because Windows/macOS signing and real-hardware scanner evidence are incomplete. See the [verified support matrix](docs/production.md#support-matrix) before deploying it.

## Highlights

- Fast filename, path, type, size, and filesystem filters
- Checksummed compact index with a CRC-protected durable update log
- Explicit failure instead of a hidden directory-walking fallback in native scanner lanes
- Dense desktop GUI with Details, List, Grid, and hierarchical Treemap views, plus CLI and MCP integration
- No telemetry
- Owner-only index/WAL state and persisted stale-index refusal

## Native metadata lanes

- **Btrfs:** tree-search ioctl
- **EXT2/3/4:** libext2fs
- **NTFS:** MFT metadata
- **macOS:** Spotlight, with native `getattrlistbulk` bulk traversal when explicitly selected
- **ZFS:** experimental; unsupported initial-index paths fail explicitly
- **Network shares:** preview server-side helper provisioning over the user's existing SSH identity

## Install

Tagged releases provide portable archives containing four sibling executables:

- `neutrasearch`
- `neutrasearch-helper`
- `neutrasearch-query`
- `neutrasearch-mcp`

Verify the release `SHA256SUMS` and GitHub artifact attestation, extract the archive, and keep its binaries together. Linux archives require the distribution's libext2fs runtime package (for example `libext2fs2` on Debian/Ubuntu or `e2fsprogs` on Arch). Windows and macOS artifacts remain unsigned until signing/notarization credentials and hardware smoke evidence are configured.

For Linux elevation, install the verified sibling binaries as root rather than making the helper setuid:

```sh
sudo install -o root -g root -m 0755 neutrasearch neutrasearch-helper \
  neutrasearch-query neutrasearch-mcp /usr/local/bin/
```

Build from source with:

```sh
cargo build --release --workspace --locked
cargo test --workspace --locked
```

Linux builds also need libext2fs development files:

```sh
# Debian/Ubuntu
sudo apt install libext2fs-dev

# Arch Linux
sudo pacman -S e2fsprogs
```

## First launch and privileges

```sh
./neutrasearch
```

Neutrasearch does not scan or modify remote hosts merely because the GUI opened. Choose **Build search index** during setup (or **File → Rebuild index**) to approve local metadata indexing. Choose **Tools → Enable network helpers** separately before SSH/SCP provisioning is allowed.

Some native metadata sources require elevated access. Never make the helper setuid. On Linux, opt in by setting `NEUTRASEARCH_PKEXEC=1`; elevated launch only accepts an installed, root-owned sibling helper that is not writable by group/others, and environment-selected helpers are refused. On Windows, raw NTFS metadata access may require Administrator rights. If access is denied, the GUI offers **Restart as Administrator** through the standard UAC prompt. Portable archives intentionally do not install a permissive polkit policy or system service.

## CLI

Search an existing index:

```sh
neutrasearch search 'report ext:pdf' --index /path/to/index.nsx --json
```

Build an index from one mounted native filesystem:

```sh
neutrasearch index /mnt/data --output /path/to/index.nsx
```

Query syntax examples:

```text
ext:rs,toml under:/home/user/projects
kind:dir photos
size:>1G
```

Linux fanotify update mode exists but its initial scan-to-watch handoff is not yet race-free. It is an experimental foreground mode, not a production daemon. Any uncertain event persists `INDEX.nsx.stale`; all readers then refuse the index until a full rebuild.

## MCP and agent integration

MCP fails startup unless an index is explicitly configured:

```sh
NEUTRASEARCH_INDEX=/path/to/index.nsx \
NEUTRASEARCH_MCP_ALLOWED_ROOTS=/home/user/projects \
neutrasearch-mcp
```

`NEUTRASEARCH_MCP_ALLOWED_ROOTS` uses the platform path-list separator and limits paths visible to agents. Omitting it allows the entire configured index. MCP returns filename/path metadata only; Neutrasearch is not a content-search replacement.

## Pi package

The token-efficient Pi extension lives in [`packages/pi-neutrasearch`](packages/pi-neutrasearch) and is published on [pi.dev/packages/pi-neutrasearch](https://pi.dev/packages/pi-neutrasearch). Install it with:

```sh
pi install npm:pi-neutrasearch
```

The install automatically includes the matching native Neutrasearch application for Linux x64/ARM64, Windows x64, or macOS x64/ARM64. Run `/neutrasearch-setup` once to open the bundled app and explicitly approve indexing—no separate download or compiler is required.

The package registers a read-only `neutrasearch` tool that defaults to 20 relative paths, omits metadata, enforces a 6,000-character output budget, and scopes every query to the current Pi workspace. It locates indexed filenames and paths; use targeted `grep` only for file contents after candidates are known.

## Security, operations, and recovery

Indexes contain privacy-sensitive absolute paths and metadata. Read [SECURITY.md](SECURITY.md) before enabling elevated scans, MCP, or remote provisioning. Deployment, data-location, recovery, uninstall, and release limitations are documented in [`docs/production.md`](docs/production.md). The on-disk format and freshness model are documented in [`docs/index-format.md`](docs/index-format.md).

## Support

- [Ko-fi](https://ko-fi.com/netroaki)
- [Patreon](https://www.patreon.com/NetroAki)

## License

[MIT](LICENSE) — use, modify, distribute, or sell Neutrasearch while preserving the license notice.
