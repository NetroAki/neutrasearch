# pi-neutrasearch

Token-efficient native indexed filename and path search for [Pi](https://pi.dev).

`pi-neutrasearch` registers one read-only `neutrasearch` tool. It queries an existing Neutrasearch compact index without walking directories, opening file contents, writing files, elevating privileges, or using the network.

## Install

Install the Pi package:

```sh
pi install npm:pi-neutrasearch
```

npm automatically installs the matching Neutrasearch application for Linux x64/ARM64, Windows x64, or macOS x64/ARM64. No separate application download or compiler is required.

Inside Pi, run:

```text
/neutrasearch-setup
```

This installs convenient shortcuts and opens the bundled Neutrasearch app:

- Linux: application menu, Desktop, and `~/.local/bin` CLI links
- Windows: Start Menu and Desktop shortcuts
- macOS: `~/Applications/Neutrasearch.app` and a Desktop alias

Approve local indexing in the app, then return to Pi. Indexing remains an explicit user action; package installation never scans disks or requests privileges.

Use `/neutrasearch` to inspect installation and index status.

The package resolves, in order:

1. `NEUTRASEARCH_QUERY` or `NEUTRASEARCH_BIN` when explicitly configured
2. the OS/architecture-specific binary package installed by npm
3. `neutrasearch-query` or `neutrasearch` on `PATH`

Set `NEUTRASEARCH_INDEX` to override Neutrasearch's platform-default index path.

## Agent tool

```json
{
  "action": "search",
  "query": "invoice ext:pdf",
  "scope": "/workspace/project",
  "limit": 20,
  "relative_paths": true,
  "metadata": false,
  "max_chars": 6000
}
```

The output is deliberately compact:

```text
scope=/workspace/project
matched=2 returned=2 search_us=184
records/2025/invoice-1042.pdf
records/2024/invoice-991.pdf
```

Defaults are optimized for agent context:

- 20 results
- relative paths
- no metadata columns
- 6,000-character hard output budget
- two-line numeric header

Use `metadata: true` only when kind, size, or modification time is needed.

## Search language

- words and `"quoted phrases"`
- `ext:rs,toml`
- `kind:file`, `kind:dir`, or `kind:link`
- `fs:btrfs,ext4,ntfs`
- `size:>10M`, `size:<4K`, or `size:1M..20M`

Neutrasearch searches indexed names and paths, not file contents. Locate candidates with this tool, then use targeted `read` or `grep` calls.

## Scope and privacy

Every search is scoped to the current Pi workspace by default. A requested scope must resolve inside that workspace.

To authorize additional roots explicitly, set an OS-path-delimited list:

```sh
export NEUTRASEARCH_PI_ALLOWED_ROOTS="$HOME/Documents:$HOME/Projects"
```

The extension verifies returned paths again and drops any path outside the approved scope. It never starts the scanner or helper daemon.

## Development

```sh
cd packages/pi-neutrasearch
npm test
pi -e .
```

## License

MIT
