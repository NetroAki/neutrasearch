# Security policy

## Supported releases

Neutrasearch is pre-1.0. Security fixes are applied to the latest tagged release and `main`; older pre-1.0 releases are not maintained.

## Reporting a vulnerability

Do not open a public issue for a vulnerability that could expose indexed paths, cross a privilege boundary, or modify a remote host. Use GitHub private vulnerability reporting for `NetroAki/neutrasearch`. Include:

- affected version/commit and operating system;
- a minimal reproduction;
- whether elevated helper, MCP, or SSH provisioning is involved;
- expected and observed behavior.

Never include another person's real paths, filenames, mount table, raw device data, SSH details, or index files in a report. Synthetic fixtures are preferred.

## Security boundaries

- Indexes contain absolute paths and filesystem metadata. Treat `.nsx`, `.delta`, `.stale`, and compaction sidecars as private user data.
- The helper protocol is a child-process/SSH framing protocol, not a network authentication protocol. Do not expose it as a TCP service.
- Elevated scans resolve client requests against the operating system's mount table. An environment-selected helper is never accepted for `pkexec` elevation.
- MCP requires an explicitly configured index. Set `NEUTRASEARCH_MCP_ALLOWED_ROOTS` to a platform path-list to constrain paths visible to agents; omitting it permits the entire configured index.
- Network helper installation is disabled until the user chooses **Watch network servers** (or explicitly sets `NEUTRASEARCH_AUTO_PROVISION_REMOTE`). It runs SSH/SCP and modifies the selected remote user's application-data directory.
- Release archives are unsigned until platform signing is configured. Verify `SHA256SUMS` and GitHub artifact attestations before use.

## Operational guidance

Run the GUI and query tools as the normal user. Elevate only the packaged, root-owned helper when native metadata access requires it. Never make the helper setuid. Store indexes in an owner-controlled directory, not a shared writable directory. A `.stale` marker means the index must be fully rebuilt before any reader will accept it.
