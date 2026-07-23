# Neutrasearch

Neutrasearch is a cross-platform filename and folder search app written in Rust. It builds a compact index from native filesystem metadata, then searches it without repeatedly walking directories.

## Features

- Fast filename, path, type, size, and filesystem filters
- Details, List, Grid, and hierarchical Treemap views
- GUI, CLI, MCP server, and Pi integration
- Checksummed compact index with durable updates
- No telemetry and no hidden directory-walking fallback

## Native indexing

| Platform/filesystem | Metadata source |
| --- | --- |
| Btrfs | Tree-search ioctl |
| EXT2/3/4 | libext2fs |
| Windows NTFS | MFT metadata |
| macOS | Spotlight or `getattrlistbulk` |
| ZFS | Experimental |

A fresh GUI install automatically indexes all local system drives. Indexed locations can be changed later in Settings.

## Install

Download the latest build from [GitHub Releases](https://github.com/NetroAki/neutrasearch/releases). Windows and macOS builds are currently unsigned, so the operating system may request confirmation.

Build from source:

```sh
cargo build --release --workspace --locked
cargo test --workspace --locked
```

Linux builds require libext2fs development files:

```sh
# Debian/Ubuntu
sudo apt install libext2fs-dev

# Arch Linux
sudo pacman -S e2fsprogs
```

## CLI

Search an existing index:

```sh
neutrasearch search 'report ext:pdf' --index /path/to/index.nsx --json
```

Build an index from a native filesystem:

```sh
neutrasearch index /mnt/data --output /path/to/index.nsx
```

Example filters:

```text
ext:rs,toml under:/home/user/projects
kind:dir photos
size:>1G
```

## MCP and Pi

Run the MCP server with an explicit index:

```sh
NEUTRASEARCH_INDEX=/path/to/index.nsx \
NEUTRASEARCH_MCP_ALLOWED_ROOTS=/home/user/projects \
neutrasearch-mcp
```

Install the Pi extension:

```sh
pi install npm:pi-neutrasearch
```

Neutrasearch indexes filenames, paths, and metadata—not file contents. Index files can reveal private paths and should remain accessible only to their owner.

## Support

- [Ko-fi](https://ko-fi.com/netroaki)
- [Patreon](https://www.patreon.com/NetroAki)

## License

[MIT](LICENSE)
