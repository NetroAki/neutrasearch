"""Linux service and retained authorization installation files."""
from pathlib import Path


def install_indexing_access(root: Path, control: Path, linux: Path) -> None:
    files = {
        'neutrasearch-authorize': ('usr/lib/neutrasearch', 0o755),
        'neutrasearch-register-shortcut': ('usr/lib/neutrasearch', 0o755),
        'neutrasearch-shortcut.desktop': ('etc/xdg/autostart', 0o644),
        'neutrasearch-watch@.service': ('usr/lib/systemd/system', 0o644),
        'com.neutrasearch.authorize.policy': ('usr/share/polkit-1/actions', 0o644),
    }
    for name, (directory, mode) in files.items():
        destination = root / directory / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        text = (linux / name).read_text()
        destination.write_text(text.replace('/usr/local/lib/', '/usr/lib/').replace('/usr/local/bin/', '/usr/bin/'))
        destination.chmod(mode)
    hooks = {
        'postinst': '''#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
 systemctl disable --now neutrasearch-index.timer neutrasearch-index.service neutrasearch-refresh.timer neutrasearch-watch.service 2>/dev/null || true
 systemctl daemon-reload
 /usr/lib/neutrasearch/neutrasearch-authorize --restore
 for marker in /var/lib/neutrasearch/authorized-users/*; do
  [ -f "$marker" ] || continue
  uid=${marker##*/}
  # Watcher health must never fail or stall a package upgrade.
  systemctl try-restart --no-block "neutrasearch-watch@$uid.service" || true
 done
fi
''',
        'prerm': '''#!/bin/sh
set -e
if [ -d /run/systemd/system ] && [ "$1" = remove ]; then
 for marker in /var/lib/neutrasearch/authorized-users/*; do
  [ -f "$marker" ] || continue
  uid=${marker##*/}
  systemctl disable --now "neutrasearch-watch@$uid.service" || true
 done
fi
''',
        'postrm': '''#!/bin/sh
set -e
if [ "$1" = purge ]; then
 rm -f /etc/polkit-1/rules.d/49-neutrasearch-*.rules
 rm -rf /var/lib/neutrasearch/authorized-users
 rm -f /var/lib/neutrasearch/access.lock
 rmdir /var/lib/neutrasearch 2>/dev/null || true
fi
''',
    }
    for name, text in hooks.items():
        (control / name).write_text(text)
        (control / name).chmod(0o755)


def install_zfs_reader(root: Path, target_dir: Path) -> None:
    from package_release.adapters import zfs_payload

    destination_root = root / "usr/lib/neutrasearch"
    for name, data, mode in zfs_payload(target_dir):
        destination = destination_root / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        destination.chmod(mode)
