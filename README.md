# Neutrasearch

Neutrasearch is a fast, cross-platform filename and folder search application written in Rust. It builds a compact index from native filesystem metadata and searches it without repeatedly scanning directories.

> Neutrasearch is in early development. Native scanning and packaging still need broader real-hardware testing.

## Highlights

- Fast filename, path, type, size, and filesystem filtering
- Compact memory-mapped index with a durable update log
- No directory-walking fallback in native scanner lanes
- Desktop GUI for Linux, Windows, and macOS
- CLI, MCP server, and Pi plugin for agent use
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
cargo run --release -p neutra-gui
```

Some native metadata sources require elevated permissions. Development-only Linux launch:

```sh
NEUTRA_PKEXEC=1 cargo run --release -p neutra-gui
```

## Search syntax

```text
report
ext:rs,toml under:/home/user/projects
kind:dir photos
size:>1G
```

Query an existing index from the command line:

```sh
neutra-query --index /path/to/index.nsx --json 'report ext:pdf'
```

## Agent integration

Neutrasearch includes an MCP server and a persistent Pi plugin so agents can search an existing index instead of enumerating folders with grep or find.

See [`pi-plugin/README.md`](pi-plugin/README.md) for setup.

## Documentation

Implementation notes and format details live in [`docs/`](docs/), including the [index format](docs/index-format.md).

## Support

- [Ko-fi](https://ko-fi.com/netroaki)
- [Patreon](https://www.patreon.com/NetroAki)

## License

[MIT](LICENSE) — use, modify, distribute, or sell Neutrasearch while preserving the license notice.
