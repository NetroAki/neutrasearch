# Neutrasearch compact index

This document describes the implemented format and its known limits. It is not a roadmap claim.

## Format v3

The immutable base is a little-endian `NEUTIDX1` file with format version 3. It is built into an exclusive owner-only temporary file, synced, checksummed, and atomically replaced.

The file contains:

1. a fixed header with format version, block size, generation, record/block/dictionary counts, and section offsets;
2. fixed-size descriptors for independently compressed path blocks (32 records per block);
3. zstd-compressed bincode blocks containing paths and fixed record metadata;
4. delta-varint posting lists keyed by folded trigrams and referencing block IDs;
5. a sorted fixed-width trigram dictionary;
6. a trailing CRC32 over every preceding byte.

Opening verifies magic, version, whole-file checksum, descriptor/dictionary bounds, and dictionary ordering before queries can use the file. Corrupt files fail closed. Version 2 indexes are intentionally incompatible and require a full rebuild.

On Unix the base is memory-mapped so clean pages remain reclaimable. Windows loads immutable bytes into owned memory because Windows blocks atomic replacement of files with live mapped views. Large-index Windows memory parity is therefore not yet claimed.

## Query behavior

Queries intersect trigram posting lists to select candidate blocks, decompress candidates, verify case-insensitive substring semantics, apply filters, rank matches, and apply a result limit.

One- and two-character terms, empty terms, and weakly selective filters can inspect every block. The privileged protocol requires a nonzero limit of at most 10,000 results, at most 32 text terms, and at most 32 KiB of query text. Library callers remain responsible for choosing bounded limits.

`under:` uses case-insensitive path-component containment: `/home/a` does not match `/home/ab`.

## Incremental WAL

Each base has a nonzero generation. Its sibling `.delta` WAL must carry the same generation or opening fails.

The WAL contains owner-only, length-prefixed bincode frames with CRC32. Writers:

1. acquire an exclusive sibling lock;
2. append and sync accepted batches before acknowledging them;
3. update an in-memory upsert map and tombstone set;
4. compact when the WAL crosses 64 MiB.

A genuinely partial final frame is truncated to the last verified boundary when a writer reopens it. A fully present frame with a bad checksum, any interior corruption, or a generation mismatch fails closed. Read-only snapshots validate the generation on every refresh.

## Compaction and recovery

Compaction temporarily materializes merged base and delta records. It currently runs synchronously under the durable-store write lock, so searches wait and peak memory rises during compaction.

The helper builds a staged v3 base, writes a synced `.compacting` journal marker, resets the WAL to the staged generation while retaining the writer lock, and atomically publishes the replacement. Startup recovery handles interruption before/after WAL reset and base publication, including torn reset headers. Persistent readers select a coherent current or staged pair during the transition and reopen after generation changes.

Low-priority, interruptible, streaming compaction remains future work.

## Stale state and freshness

Linux fanotify update mode uses file handles and direct event-target metadata reads; it does not rescan paths. Queue overflow, directory rename, an unpaired legacy move, commit failure, or uncertain watcher state writes an owner-only `INDEX.nsx.stale` marker. All readers reject the index while that marker exists. Only a successful full base rebuild clears it.

The initial scan-to-watch handoff is not race-free. Linux live updates are therefore experimental and are not a continuous-freshness guarantee. Windows USN Journal and macOS FSEvents live-update lanes are not implemented.

## Privacy and file safety

The base and WAL contain absolute paths, sizes, mtimes, modes, filesystem kinds, source IDs, and native identifiers. Treat them as sensitive user data.

Base temporary files use exclusive creation and no-follow semantics. WAL and lock files reject symlinks, non-regular files, multi-link files, and group/world permissions before mutation. Never place elevated helper state in an untrusted shared directory.

## Measured evidence

A 2026-07-20 synthetic benchmark of the earlier compact implementation produced 1,000,000 generated records in 1.451 seconds, a 47,627,533-byte index, and a 536 µs best selective library query. Those measurements predate the v3 whole-file checksum and are not real-volume or current-release performance claims.

Re-run the current format benchmark with:

```sh
cargo run --release -p neutra-core --example compact_benchmark -- \
  1000000 /tmp/neutra-compact-benchmark.idx
```
