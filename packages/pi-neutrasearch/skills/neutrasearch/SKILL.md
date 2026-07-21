---
name: use-neutrasearch
description: Locate filenames and paths through the local Neutrasearch index instead of walking the filesystem
---

# Use Neutrasearch

Use the `neutrasearch` tool before `find`, directory walking, or broad globbing when the task is to locate files or folders by name, extension, type, size, or indexed path.

## Token-efficient defaults

1. Start with `limit: 20`, `relative_paths: true`, and `metadata: false`.
2. Scope defaults to the current Pi workspace. Narrow `scope` further when possible.
3. Refine with query filters such as `ext:rs,toml`, `kind:file`, `kind:dir`, `size:>10M`, or quoted terms before increasing the limit.
4. Request metadata only when size, kind, filesystem, or modification time is needed.
5. Increase `max_chars` or `limit` only after a narrow query still omits needed candidates.

## Boundary

Neutrasearch searches indexed metadata—filenames and paths—not file contents. After locating a small candidate set, use `read` or targeted `grep` for content inspection.

The tool is read-only. It must not initiate indexing, request privileges, write files, or contact the network. Outside-workspace searches require the user to pre-authorize canonical roots through `NEUTRASEARCH_PI_ALLOWED_ROOTS`.
