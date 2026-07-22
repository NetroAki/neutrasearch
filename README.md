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

Tagged releases also provide native installers alongside the portable archives:

- Windows x64: `neutrasearch-<version>-windows-x64-setup.exe`
- Debian/Ubuntu x64 and ARM64: `neutrasearch-<version>-linux-<architecture>.deb`
- macOS Intel and Apple Silicon: `neutrasearch-<version>-macos-<architecture>.dmg`

The Windows setup asks for Administrator approval once, installs all four executables plus Start-menu/uninstall entries, and registers an automatic privileged scanner service so normal GUI scans do not request UAC again. The Linux package installs the GUI/CLI tools, desktop entry, icon, documentation, and the helper in its trusted root-owned location. The macOS disk image contains `Neutrasearch.app` and an Applications shortcut. Verify the release `SHA256SUMS` and GitHub artifact attestation before installing or extracting. Linux packages require the distribution's libext2fs runtime package (`libext2fs2` on Debian/Ubuntu). Windows and macOS artifacts remain unsigned until signing/notarization credentials and hardware smoke evidence are configured, so Windows SmartScreen or macOS Gatekeeper may require explicit confirmation for this pre-1.0 build.

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

On first launch, choose **Add folder**, then **Allow access and scan**. The selected roots are persisted and setup completes only after a usable index is published; add or remove roots later through **File → Locations and index**. A native lane still reads filesystem metadata at volume speed, but only records beneath the selected roots are published into the index.

Neutrasearch does not contact remote hosts merely because the GUI opened. Network-helper monitoring is separately enabled in settings. Offline mounted servers are shown as waiting, retried every 30 seconds, and do not put the local index into an error state. Remote helper provisioning is still preview-only; see [`docs/production.md`](docs/production.md) before relying on network-share search.

Some native metadata sources require elevated access. Never make the helper setuid. On Linux, the first scan requests `pkexec` and accepts only an installed, root-owned helper that is not writable by group/others; environment-selected helpers are refused. Settings also provides **Rebuild as administrator**. On Windows, the setup-owned LocalSystem scanner accepts only local connections from the installed GUI beside it; approved roots are still enforced inside the service. Its bounded logs are `%ProgramData%\Neutrasearch\helper.log` and `helper.previous.log`. Portable archives intentionally install neither this service nor a permissive polkit policy, so portable Windows raw-NTFS scans may still require an explicit Administrator restart.

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
