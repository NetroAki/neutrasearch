# Neutrasearch

Neutrasearch is a fast, cross-platform filename and folder search application written in Rust. It builds a compact index from native filesystem metadata and searches it without repeatedly scanning directories.

> Neutrasearch is in early development. Native scanning and packaging still need broader real-hardware testing.

## Highlights

- Fast filename, path, type, size, and filesystem filtering
- Compact memory-mapped index with a durable update log
- No directory-walking fallback in native scanner lanes
- Desktop GUI for Linux, Windows, and macOS
- CLI and MCP server for agent use; Pi package distributed separately
- No telemetry

## Native index sources

- **Btrfs:** tree-search ioctl
- **EXT2/3/4:** libext2fs
- **NTFS:** MFT metadata
- **macOS:** Spotlight, with `getattrlistbulk` fallback
- **ZFS:** experimental snapshot/diff lane
- **Network shares:** server-side helper over SSH

Unsupported native lanes fail explicitly rather than silently walking the filesystem.

## Build

Install Rust, then run:

```sh
cargo build --release --workspace
cargo test --workspace
```

Linux also needs libext2fs development files:

```sh
# Debian/Ubuntu
sudo apt install libext2fs-dev

# Arch Linux
sudo pacman -S e2fsprogs
```

## Run

```sh
cargo run --release --bin neutrasearch
```

Some native metadata sources require elevated permissions. Development-only Linux launch:

```sh
NEUTRASEARCH_PKEXEC=1 cargo run --release -p neutra-gui
```

## Search syntax

```text
report
ext:rs,toml under:/home/user/projects
kind:dir photos
size:>1G
```

Build and query an index from the command line:

```sh
neutrasearch index /mnt/data --output /path/to/index.nsx
neutrasearch search 'report ext:pdf' --index /path/to/index.nsx --json
```

On Linux, a live update service is available for supported local filesystems:

```sh
sudo neutrasearch serve --index /path/to/index.nsx --watch /mnt/data
```

## Agent integration

Neutrasearch includes an MCP server so agents can search an existing index instead of enumerating folders with grep or find. A Pi plugin that installs and drives Neutrasearch is published separately at [pi.dev/packages](https://pi.dev/packages).

## Documentation

Implementation notes and format details live in [`docs/`](docs/), including the [index format](docs/index-format.md).

## Support

- [Ko-fi](https://ko-fi.com/netroaki)
- [Patreon](https://www.patreon.com/NetroAki)

## License

[MIT](LICENSE) — use, modify, distribute, or sell Neutrasearch while preserving the license notice.
