# Production and release guide

This document separates verified support from experimental lanes. A successful build is not proof that a native scanner works on real hardware.

## Support matrix

| Platform | Portable archive | Installer | Native initial indexing | Freshness baseline | Release status |
|---|---:|---:|---|---|---|
| Linux x86_64 | Yes | Debian package | Btrfs, EXT2/3/4, NTFS; privileges commonly required | Manual atomic rebuild; fanotify is an experimental extra | Supported pre-1.0 |
| Linux ARM64 | Yes, native ARM runner | Debian package | Same compiled lanes; hardware evidence still limited | Manual atomic rebuild; fanotify is experimental | Preview |
| Windows x86_64 | Yes | Inno Setup EXE + scanner service | NTFS MFT lane; setup grants service access once | Manual atomic rebuild | Preview pending hardware/signing evidence |
| macOS x86_64 | Yes | DMG application image | Spotlight; native bulk fallback | Manual atomic rebuild | Preview pending hardware/signing evidence |
| macOS ARM64 | Yes | DMG application image | Spotlight; native bulk fallback | Manual atomic rebuild | Preview pending hardware/signing evidence |
| Windows ARM64 | No release artifact yet | No | Unverified | Manual rebuild code is portable but unverified | Unsupported |
| ZFS | Included as experimental code only | No | Initial indexing intentionally refuses unsupported paths | Not implemented | Unsupported |

The parity baseline is the dense GUI, native initial indexing, manual rebuild, compact-index search, CLI, MCP, and remote-helper provisioning protocol. Native CI builds and tests that baseline on Linux, Windows, and macOS. The GUI now scans only the native volumes containing user-selected roots and publishes the reachable selected records atomically. Unavailable native lanes are skipped and shown as degraded; if every requested lane fails, the last complete index remains active. CLI index builds remain single-mount and fail closed.

Linux fanotify is not part of that parity baseline. Windows USN-journal and macOS FSEvents freshness are not implemented, so all platforms must remain correct through manual rebuilds rather than pretending equivalent live-update guarantees.

Release archives and installers are currently unsigned. Stable Windows/macOS distribution requires code-signing/notarization credentials and real-hardware smoke evidence. Verify checksums and GitHub attestations.

## Archive layout

Every portable archive contains sibling binaries:

- `neutrasearch`
- `neutrasearch-helper`
- `neutrasearch-query`
- `neutrasearch-mcp`

It also contains `README.md`, `LICENSE`, `SECURITY.md`, this guide, `CHANGELOG.md`, and an inner `SHA256SUMS`. Keep the binaries together so companion discovery works.

## Privilege model

Run the desktop, CLI, query, and MCP processes as the normal user. Native metadata APIs may require elevated access. On Linux, Neutrasearch refuses to elevate a helper selected through an environment variable; an elevated helper must be root-owned in an approved system directory and must not be group/world writable.

The Windows installer registers `NeutrasearchHelper` as an automatic LocalSystem service. Its byte-mode named pipe rejects remote clients, has a bounded frame protocol, accepts only the installed `neutrasearch.exe` beside the protected helper, resolves requested volumes from the operating system, and filters records to approved roots before returning them. This grants the installed GUI raw NTFS metadata visibility by design; use it only on a trusted personal/workstation installation. Service logs are bounded to `helper.log` plus one previous file under `%ProgramData%\Neutrasearch`. Portable archives install no service and retain the explicit Administrator-restart fallback.

No platform installs a setuid binary or general passwordless-administration policy.

## Index privacy and data locations

Indexes contain absolute paths, sizes, mtimes, modes, filesystem kinds, and native identifiers. Backups, logs, bug reports, and MCP output can therefore disclose private filenames.

Set `NEUTRASEARCH_INDEX` to choose an explicit index. Default GUI/query data locations are `%LOCALAPPDATA%\Neutrasearch\index.nsx` on Windows, `~/Library/Application Support/Neutrasearch/index.nsx` on macOS, and `$XDG_DATA_HOME/neutrasearch/index.nsx` (or `~/.local/share/neutrasearch/index.nsx`) on Linux. MCP requires an explicit index variable and can be constrained with `NEUTRASEARCH_MCP_ALLOWED_ROOTS`, using the operating system path-list separator (`:` on Unix, `;` on Windows).

A successful full rebuild creates compact format v3 and clears an existing `.stale` marker. Readers reject corrupt, generation-mismatched, or stale index pairs. The helper takes the delta-writer lock and refuses a rebuild while a serving writer is active; stop long-lived readers too before replacing an index, which is required by Windows file-sharing semantics.

## Network helpers

Opening the GUI does not modify remote hosts. Selecting **Watch network servers** starts network-mount detection; matching servers may then receive an atomic helper update over the existing SSH identity. Offline servers remain non-fatal and are retried every 30 seconds; authentication, integrity, artifact, and unsupported-platform failures remain visible rather than being mislabeled as offline. `NEUTRASEARCH_AUTO_PROVISION_REMOTE=1` is the explicit unattended opt-in. Set `NEUTRASEARCH_HELPER_ARTIFACTS` if helpers are not in the executable's sibling `helpers/` directory.

Each archive includes its matching target helper and `.sha256` sidecar under `helpers/`. Provisioning refuses a missing/mismatched sidecar and verifies the uploaded temporary file on the server before atomic installation. To manage servers on other operating systems/architectures, collect their release helper+sidecar files into the configured helper directory.

Remote auto-provisioning remains preview functionality until remote install/uninstall smoke tests and signed release manifests exist. The current GUI provisions and verifies helpers but does not yet map a server export namespace back into the local mounted path or merge remote records. Do not claim network-share search until that mapping and end-to-end scan channel are implemented.

## Live-update limitation

Linux fanotify updates persist a `.stale` marker after overflow, directory rename, commit failure, or an uncertain event. CLI/MCP/GUI readers then refuse the index until a full rebuild.

The initial scan-to-watch handoff is not yet race-free. Do not promise continuous freshness or install `neutrasearch serve --watch` as an unattended production daemon. For authoritative results, rebuild the index after mounting/startup and after any stale error.

## Release procedure

1. Run the full CI suite on a clean commit.
2. Update `CHANGELOG.md` and the workspace version.
3. Create an annotated `v<workspace-version>` tag.
4. The release workflow validates the tag, builds on native runners, packages deterministic archives, writes `SHA256SUMS`, and creates attestations.
5. Do not call unsigned Windows/macOS artifacts stable. Add signing and notarization before that claim.

## Uninstall

Portable builds have no installer-owned state; remove the extracted application directory. Windows setup builds appear in Installed Apps and uninstall the `NeutrasearchHelper` service before removing its executable; service logs under `%ProgramData%\Neutrasearch` remain available for diagnosis and can be deleted manually. Debian packages can be removed with `sudo apt remove neutrasearch`, and macOS builds can be removed from Applications. Uninstalling does not delete indexes containing absolute-path metadata. Remote helpers are stored under `~/.local/lib/neutrasearch/` on Unix servers and `%LOCALAPPDATA%\Neutrasearch\` on Windows servers; remove those explicitly over the same trusted administrative channel.
