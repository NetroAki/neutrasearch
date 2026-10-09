#!/usr/bin/env python3
"""Verify a native save survives blocked recovery using a disposable QA index.

Run with CAP_SYS_ADMIN or sudo; pass a fixture created by gui_fixture.
"""

import argparse
import json
import os
from pathlib import Path
import select
import shutil
import sqlite3
import struct
import subprocess
import tempfile
import time


def read_frame(child, log):
    deadline = time.monotonic() + 5

    def exact(length):
        output = b""
        while len(output) < length:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([child.stdout], [], [], remaining)[0]:
                raise RuntimeError("protocol frame timed out")
            part = os.read(child.stdout.fileno(), length - len(output))
            if not part:
                log.seek(0)
                raise RuntimeError("helper exited: " + log.read())
            output += part
        return output

    length = struct.unpack("<I", exact(4))[0]
    if length > 16 * 1024 * 1024:
        raise RuntimeError("oversized protocol frame")
    return exact(length)


def probe(helper, fixture, mount):
    with tempfile.TemporaryDirectory(prefix="startup-recovery-", dir=fixture.parent) as folder:
        index = Path(folder) / "probe.nsx"
        shutil.copyfile(fixture, index)
        shutil.copyfile(Path(str(fixture) + ".browse.base"), Path(str(index) + ".browse.base"))
        db = sqlite3.connect(str(index) + ".browse")
        source = sqlite3.connect(f"file:{fixture}.browse?mode=ro", uri=True)
        source.backup(db)
        source.close()
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("BEGIN IMMEDIATE")
        with (Path(folder) / "helper.log").open("w+") as log:
            child = subprocess.Popen(
                [str(helper), "--watch-index", str(index), mount, "0"],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
            )
            try:
                hello = struct.pack("<II", 0, 11)
                child.stdin.write(struct.pack("<I", len(hello)) + hello)
                child.stdin.flush()
                assert struct.unpack("<I", read_frame(child, log)[:4])[0] == 0
                deadline = time.monotonic() + 5
                while not index.with_suffix(".delta.lock").exists():
                    if child.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("reader attachment/store open did not complete")
                    time.sleep(0.01)
                assert not select.select([child.stdout], [], [], 0.1)[0], "premature WatchReady"
                marker = Path(folder) / "saved-during-recovery.txt"
                marker.write_text("closed write while the durable store was blocked")
                started = time.monotonic()
                db.rollback()
                assert struct.unpack("<I", read_frame(child, log))[0] == 10
                while time.monotonic() - started < 3:
                    row = db.execute("SELECT bytes FROM entries WHERE path=?", (str(marker),)).fetchone()
                    if row and int.from_bytes(row[0], "big") == marker.stat().st_blocks * 512:
                        return {"ready_withheld_during_recovery": True,
                                "save_during_recovery_preserved": True,
                                "publication_after_unblock_seconds": time.monotonic() - started}
                    time.sleep(0.02)
                raise RuntimeError("startup write was lost")
            finally:
                child.stdin.close()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
                db.rollback()
                db.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--mount", default="/home")
    args = parser.parse_args()
    print(json.dumps(probe(args.helper.resolve(), args.fixture.resolve(), args.mount)))
