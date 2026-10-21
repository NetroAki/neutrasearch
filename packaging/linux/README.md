<!-- doc-topic: Linux packaging -->
# Linux packaging

## Index lifecycle

The first run creates the index from supported local native filesystems. The
helper uses each filesystem's native metadata interface and refuses an
unsupported filesystem instead of walking its directory tree. There is no
directory-walk fallback. Once an approved account has an index, its
`neutrasearch-watch@UID` service runs the delta watcher as that user. File changes are appended to the index's delta WAL and become
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
fails, or its event queue overflows on a filesystem without transaction
recovery, the helper marks the index stale and the GUI disables it until the
index is rebuilt. Btrfs recovers from overflow by replaying its transactions. These cases do not trigger an
automatic full rescan. Run `neutrasearch index` for a full rebuild. The package
does not enable a launch-time scan or a daily scan timer.

## Debian package

Build with the release binaries already present in `target/<triple>/release`:

```sh
python3 packaging/tools/package_installers.py linux-deb \
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
The opt-in example `49-neutrasearch.rules.example` (not installed by any package) grants the active local session
passwordless access to the indexing helper; install it only on a machine where
that access is intended:

```sh
install -Dm644 49-neutrasearch.rules.example \
  /etc/polkit-1/rules.d/49-neutrasearch.rules
```

## KDE quick-search shortcut

The package installs a KDE-only autostart entry for
`neutrasearch-register-shortcut`. The default is Alt+K. On KDE, registration checks
whether each requested combination is already assigned, registers quick search, and verifies
the saved shortcut. It leaves an existing shortcut owned by another action
alone and reports that conflict. Other desktops receive the application menu
entry and can assign `neutrasearch --spotlight` through their shortcut
settings. Search > Keyboard shortcuts lets users edit, add, remove or reset
bindings. Remove all and apply to disable global quick search. Choices persist
in `neutrasearch/shortcuts.json` under the user's configuration directory and
are restored at login. Invalid or conflicting bindings keep the previous
active shortcuts. Updating successfully releases the previous global Ctrl+K
and Super+K bindings. Ctrl+K inside the main Neutrasearch window still
focuses its search field; it does not intercept keys in other applications.

Alt+K is also the proposed cross-platform default, called Option+K on macOS.
No combination is guaranteed free across every application and keyboard layout.
Windows and macOS use a separate per-user shortcut agent, installed on first
GUI launch. Native runtime validation remains required on those platforms.
