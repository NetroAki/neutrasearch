# Neutrasearch Pi extension

Install the Rust binaries first, then link/copy `neutrasearch.ts` into Pi's extension directory:

```sh
cargo install --path crates/neutra-mcp --bin neutrasearch-mcp
ln -s "$PWD/pi-plugin/neutrasearch.ts" ~/.pi/agent/extensions/neutrasearch.ts
```

Set `NEUTRASEARCH_MCP=/path/to/neutrasearch-mcp` if it is not on `PATH`, and `NEUTRASEARCH_INDEX=/path/to/index.nsx` to override the cross-platform default cache.

The extension registers `neutra_search`. Agents should use it for filename/path discovery instead of broad `grep`, `find`, or `rg --files`; content grep remains appropriate after narrowing the candidate files.
