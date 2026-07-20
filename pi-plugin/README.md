# Neutrasearch Pi extension

Install the Rust binaries first, then link/copy `neutrasearch.ts` into Pi's extension directory:

```sh
cargo install --path crates/neutra-mcp
ln -s "$PWD/pi-plugin/neutrasearch.ts" ~/.pi/agent/extensions/neutrasearch.ts
```

Set `NEUTRA_MCP=/path/to/neutra-mcp` if it is not on `PATH`, and `NEUTRA_INDEX=/path/to/index.bin` to override the cross-platform default cache.

The extension registers `neutra_search`. Agents should use it for filename/path discovery instead of broad `grep`, `find`, or `rg --files`; content grep remains appropriate after narrowing the candidate files.
