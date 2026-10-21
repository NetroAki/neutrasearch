# Neutrasearch

Neutrasearch is a cross-platform filename and folder search app written in Rust. It builds a compact index from native filesystem metadata, then searches it without repeatedly walking directories.

## Features

- Fast filename, path, type, size, and filesystem filters
- Details, List, Grid, and hierarchical Treemap views
- Persisted logical folder totals for responsive hierarchy views
- GUI, CLI, MCP server, and Pi integration
- Checksummed compact index; on Linux it follows file changes live
- No telemetry and no hidden directory-walking fallback

## Native indexing

| Platform/filesystem | Metadata source |
| --- | --- |
| Btrfs | Tree-search ioctl |
| EXT2/3/4 | libext2fs |
| Windows NTFS | MFT metadata |
| macOS | Spotlight index; a failed metadata lookup fails the scan |
| Linux ZFS | Pinned OpenZFS 2.2.2 reader; snapshot ZAP entries and SA attributes |

### Platform support

| | Linux | Windows | macOS |
| --- | --- | --- | --- |
| Native metadata index | yes | NTFS | Spotlight-indexed files only |
| Live updates after the first index | yes (fanotify) | not yet; re-index to refresh | not yet; re-index to refresh |
| Trash and restore | yes | not yet | not yet |

Rename, copy and move never replace an existing destination on any platform. Undo history is kept in memory and is lost when the app closes.

A fresh GUI install automatically indexes all local system drives. Indexed locations can be changed later in Settings. Compact indexes also write a generation-bound `.dirs` sidecar containing logical folder totals and direct children, so hierarchy views do not need to decode every search record.

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

The Linux ZFS reader also needs matching OpenZFS 2.2.2 development files
(`libzfslinux-dev` on Ubuntu 24.04). Build its bundled runtime before packaging:

```sh
python3 packaging/tools/build-zfs-reader.py --project-root . --target-dir target/release
```

Encrypted ZFS datasets and non-UTF-8 filenames currently fail explicitly.
Other OpenZFS versions and pool feature sets need qualification.

## CLI

Fully index the machine, then search the most recently used index:

```sh
neutrasearch index
neutrasearch search "example.exe"
```

An index location remains optional for both commands:

```sh
neutrasearch index --output /path/to/index.nsx
neutrasearch search 'report ext:pdf' --index /path/to/index.nsx --json
```

Indexing always covers the full depth of every supported local native filesystem; there is no directory-depth mode.

Example filters:

```text
ext:rs,toml under:/home/user/projects
kind:dir photos
size:>1G
```

## MCP and Pi

Run the MCP server with the last index automatically:

```sh
NEUTRASEARCH_MCP_ALLOWED_ROOTS=/home/user/projects neutrasearch mcp
```

Without `NEUTRASEARCH_MCP_ALLOWED_ROOTS` the server can return filenames, sizes and modification times for every indexed path, so set it whenever an agent should see only some folders. The server is read-only and sends no file contents.

`--index INDEX.nsx` and `NEUTRASEARCH_INDEX` remain optional overrides.

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


## Privacy on shared machines

Neutrasearch indexes every drive from filesystem tables, so the index lists names, sizes and dates of files that individual accounts may not be able to open. On Windows the installer approves its account for machine-wide metadata access; the service accepts only that account, SYSTEM and administrators. Upgrades preserve the approved account. Administrators deploying for another user can pass `-AuthorizedUserSid` to the installed `service/install-service.ps1`. On Linux the index belongs to the account running the watch service. Keep the index private: its metadata can include paths that the approved user cannot open.
