# Neutrasearch

Read `DESIGN.md` before UI changes and `current-Debt.md` before extending a
feature with recorded limits. The owner's production requirements are saved
verbatim in `docs/feedback/production-readiness-request-2026-10-08.md`.

- Index from native filesystem metadata only. Unsupported or failed metadata
  readers must report an error; never fall back to directory enumeration.
- Keep one durable writer per user across all mounted drives. File events
  update the live delta and the disk sort catalog; do not schedule full rescans.
- Test GUI interactions on an owned virtual display, never the real desktop.
- Reuse `target` and bound build/test runtimes. Measure the full index before
  claiming the 100 MB memory or three-second interaction targets are met.
- Preserve file contents on failed moves, renames, trash and undo operations.
  Do not overwrite an existing destination implicitly.
