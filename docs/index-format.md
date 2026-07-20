# Neutrasearch index format direction

## Performance contract

- A filesystem-native bulk scan builds the base index once.
- Normal searches never scan a filesystem and never linearly scan every indexed record.
- Steady state is event-driven and uses effectively zero CPU while idle.
- The read-only base is memory-mapped on Unix, so clean pages are reclaimable by the OS. Windows snapshots currently use owned immutable bytes because Windows blocks atomic replacement of files with live mapped views; this cross-platform compaction trade-off must be optimized before claiming large-index Windows memory parity.
- Mutable state is bounded: 32 MiB delta target, 64 MiB hard compaction threshold, 32 MiB decompression cache, and bounded per-query scratch space.
- Compaction runs only after the hard threshold is crossed. The current implementation runs synchronously while holding the durable-store write lock, so searches wait; low-priority and interruptible scheduling remain future work.

## Evidence behind the design

- plocate uses a trigram inverted index and reports a 466 MiB database for 27 million paths, with a selective query completing in about 8 ms. Its default groups 32 filenames into each compressed block, reducing posting-list size while accepting a bounded false-positive verification cost: <https://plocate.sesse.net/> and <https://manpages.debian.org/testing/plocate/plocate-build.8.en.html>.
- Russ Cox's description of Google Code Search explains why three-byte grams are the useful balance, why sorted posting-list intersections identify candidates, and why candidates must still be verified against source text: <https://swtch.com/~rsc/regexp/regexp4.html>.
- SQLite FTS5 independently provides a trigram tokenizer for general substring matching and documents the same under-three-character linear fallback. It also documents segment merging/tombstones, supporting a base-plus-delta model: <https://sqlite.org/fts5.html#the_trigram_tokenizer>.

## Base file

The production base is a versioned, checksummed, little-endian file written to a temporary owner-only file, fsynced, and atomically renamed.

1. **Header and section directory**
   - magic, format version, build generation, record/block counts
   - source filesystem identity and freshness checkpoint
   - offsets, lengths, and checksums for every section
2. **Path blocks**
   - 32 paths per independently compressed block by default
   - front-code shared prefixes before compression
   - carry fixed-width metadata beside each path: size, mtime, mode, kind, filesystem, and source ID
3. **Trigram dictionary**
   - sorted packed 24-bit ASCII-folded gram keys (with a Unicode fallback namespace)
   - posting offset, compressed length, and document frequency
4. **Posting lists**
   - sorted block IDs, not record IDs
   - delta encoded and SIMD-/varint-friendly compressed
   - query planner intersects the rarest lists first
5. **Short-query accelerator**
   - sorted folded basename keys for exact and prefix lookup
   - one- and two-character unconstrained substrings use a bounded block scan and are explicitly the slower case
6. **Optional filter columns**
   - compact bitmaps/ranges for kind, filesystem and extension where measurements justify their size

A trigram hit identifies candidate path blocks. Neutrasearch decompresses only those blocks, verifies exact case-insensitive substring semantics, applies metadata filters, and maintains a bounded top-N heap for ranking.

## Incremental state

- Append change events to an owner-only framed write-ahead log before publishing them. `neutra_core::DeltaIndex` implements replayable path upserts/tombstones, CRC32-protected frames, byte accounting, explicit syncing, and the 64 MiB compaction threshold.
- Every base build carries a nonzero generation. The delta header must contain the same generation or opening fails; an old WAL is never replayed over a replacement base. `neutrasearch serve --index` owns this pair under a cross-platform exclusive writer lock and syncs each accepted update batch before acknowledging it. CLI/MCP readers use non-locking snapshots and tail newly completed CRC-verified frames before each persistent query. On writer recovery, an incomplete final frame is truncated to the last CRC-verified boundary; interior corruption still fails closed.
- Keep additions/updates in a small mutable map and deletions in a tombstone set. CLI and MCP queries merge this overlay with the mmap base and suppress shadowed paths before ranking, so limits and match counts remain exact.
- Linux can run `neutrasearch serve --index INDEX.nsx --watch MOUNT`. It uses a filesystem fanotify mark with file handles and names, performs only direct event-target metadata reads, batches/coalesces changes, and enters an explicit stale state on queue overflow, directory rename, a legacy unpaired move event, commit failure, or compaction failure. A stale service refuses further searches and updates until the index is rebuilt and the service restarted. Resolved event paths are rejected unless they remain inside the canonical watched mount. The watch thread does not emit unsolicited protocol frames, so it cannot interleave notifications with command responses. Watch mode requires `CAP_SYS_ADMIN`, `CAP_DAC_READ_SEARCH`, kernel/filesystem file-handle support, and an explicit source ID for multi-source bases. The race-free initial-scan-to-watch handoff is not complete yet, so this is not a full freshness claim.
- Windows USN Journal checkpoints and macOS FSEvents/Spotlight state remain to be wired into the same update service.
- When the WAL crosses its hard threshold, the helper merges base records with the in-memory tombstones/upserts, builds a staged compact base, writes and syncs a compaction marker, resets the WAL under the existing exclusive writer lock, and atomically publishes the staged base. Startup recovery completes either side of the marker/reset/publication sequence. Persistent CLI/MCP readers validate the WAL generation on every refresh and reopen the base+delta pair after compaction. Any unrecoverable replacement failure marks the helper stale and fails closed rather than serving an uncertain pair.

## Resource policy

- Do not deserialize all paths into a permanent `Vec<FileRecord>` in the service.
- Do not retain the bulk scanner's temporary inode/MFT structures after publishing the base.
- Use bounded worker pools rather than one worker per logical CPU during normal queries.
- Do not use `mlock`; mapped clean pages must remain reclaimable.
- Do not repeatedly call `madvise(DONTNEED)` or `posix_fadvise(DONTNEED)` after every query; the kernel page cache is shared and should manage pressure. Bound application-owned caches instead.
- Expose base bytes, mapped resident estimate, delta bytes, cache bytes, last checkpoint, stale state, and last compaction in status output.

## Initial size target

For approximately 12.4 million paths, target **under 750 MiB** including metadata, with a stretch target under 500 MiB after format tuning. The existing bincode `index.bin` is transitional and must not become the production format.

A 2026-07-20 synthetic release benchmark of the first working compact implementation produced 1,000,000 generated records in 1.451 s, a 47,627,533-byte index (47.63 bytes/record), and a 536 µs best selective library query. MCP measured 1.646–1.866 ms for the same query. After querying and idling, the MCP process had 7.8 MiB RSS, 0.8 MiB anonymous RSS, and one thread; the mapped file remained reclaimable. These results validate the design direction but are not real-volume claims. Reproduce with:

```sh
cargo run --release -p neutra-core --example compact_benchmark -- \
  1000000 /tmp/neutra-compact-benchmark.idx
```
