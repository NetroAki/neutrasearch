# Linux packaging

## Index lifecycle

The first run creates the index from supported local native filesystems. The
helper uses each filesystem's native metadata interface and refuses an
unsupported filesystem instead of walking its directory tree. There is no
directory-walk fallback. Once an index exists, the watch service starts the
`neutrasearch-watch-all` supervisor. For each interactive account, it checks
for a missing index (and builds only in that case), then runs the delta watcher
as that user. File changes are appended to the index's delta WAL and become
visible to searches without a full rescan.

The service reports ready only after every account's native readers have
attached and its durable index has opened. Readers attach before index
recovery so writes during recovery queue for publication. Btrfs transaction
cursors are captured at attachment; first-ingest generation binding remains
tracked in `current-Debt.md`.

Unsupported filesystems have no native indexing lane and are refused; there is
no directory-walk fallback. Btrfs subvolume mounts use the native transaction
sweep when a filesystem-wide fanotify mark is unavailable. NTFS/FUSE mounts
that need a mount mark receive saved-file updates only; rename and delete
updates are unavailable, and the GUI reports that limitation. If a watcher
fails or its event queue overflows, the helper marks the index stale and the
GUI disables it until the index is rebuilt. These cases do not trigger an
automatic full rescan. Run `neutrasearch index` for a full rebuild. The package
does not enable a launch-time scan or a daily scan timer.

## Debian package

Build with the release binaries already present in `target/<triple>/release`:

```sh
python3 scripts/package_installers.py linux-deb \
  --project-root "$PWD" \
  --target-dir target/x86_64-unknown-linux-gnu/release \
  --output-dir dist \
  --target x86_64-unknown-linux-gnu \
  --version VERSION
```

The package installs the GUI, query CLI, and MCP binaries under `/usr/bin`,
the privileged helper and watch supervisor under `/usr/lib/neutrasearch`,
the watcher systemd unit under `/usr/lib/systemd/system`, and the desktop
entries and polkit action in their standard shared-data directories. Installing
the package enables the live-watch service when systemd is running.

## Privileged helper

`com.neutrasearch.helper.policy` registers polkit actions for the supported
root-owned helper paths. Debian installs the helper at
`/usr/lib/neutrasearch/neutrasearch-helper` and includes the matching action.
The standalone `49-neutrasearch.rules` grants the active local session
passwordless access to the indexing helper; install it only on a machine where
that access is intended:

```sh
install -Dm644 49-neutrasearch.rules \
  /etc/polkit-1/rules.d/49-neutrasearch.rules
```

## KDE quick-search shortcut

The package installs a KDE-only autostart entry for
`neutrasearch-register-shortcut`. On KDE, the script checks whether Ctrl+K is
already assigned, registers Neutrasearch's quick-search action, and verifies
the saved shortcut. It leaves an existing shortcut owned by another action
alone and reports that conflict. Other desktops receive the application menu
entry and can assign `neutrasearch --spotlight` through their shortcut
settings.
