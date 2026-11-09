# kvlog

A tiny key-value store that survives being killed. Every write is appended to a checksummed
log and synced to disk before it counts; on start the store replays the log and cuts off a
write the crash interrupted. A test kills it with SIGKILL hundreds of times at random moments
and checks that no acknowledged write is ever lost. The store is kept as small as that allows.

```console
$ kvlog set city Vienna
$ kvlog set country Austria
$ kvlog get city
Vienna
$ kvlog list
city	Vienna
country	Austria
$ kvlog rm city
$ kvlog list --prefix c
country	Austria
$ kvlog compact
$ kvlog get city
kvlog: no such key: city
```

`get` and `rm` of an unknown key exit 1. `list` prints `key<TAB>value`, sorted, with `\`, tab,
newline and CR escaped, so every entry is one line.

## Where the data lives

`--dir`, else `KVLOG_DIR`, else `$XDG_DATA_HOME/kvlog`, else `~/.local/share/kvlog` (the same
order as [todo](../todo/)). The folder holds:

- `wal`: the write-ahead log, one record per `set` or `rm`;
- `snapshot`: every key at the last `compact` (same record format, sets only);
- `snapshot.tmp`: while a `compact` runs, or left by a crash in one until the next `compact`
  (opening never reads it);
- `LOCK`: held exclusively while a store is open; a second process gets `the store is in use`.

## Record format

Little-endian, CRC-32 (IEEE):

| Bytes | Field |
|---|---|
| 0..4 | payload length |
| 4..8 | CRC of the 4 length bytes |
| 8..12 | CRC of the payload |
| payload | `op` (1 = set, 2 = remove), key length (u32), key, value |

The length has its own checksum so a damaged length is caught as corruption instead of looking
like a write that was cut short.

## Recovery

Opening scans the log record by record:

| What it finds | Verdict |
|---|---|
| end of file | clean |
| 1 to 11 bytes left, or a length running past the end | torn tail: dropped |
| a bad length checksum, anywhere | corruption |
| a bad payload checksum on the last record | torn tail: dropped |
| a bad payload checksum with more records after it | corruption |
| good checksums around a malformed payload | corruption |

A torn tail is a write that was never acknowledged: it is cut off (with a notice saying how many
bytes) and the store opens. Corruption is never "repaired": the store refuses to open, changes
nothing and names the file and byte offset. A torn or remove record in `snapshot` is corruption.

`compact` writes `snapshot.tmp`, syncs it, renames it over `snapshot`, syncs the folder, then
empties `wal`. A crash at any step leaves either the old snapshot and the full log, or the new
snapshot and a log that replays to the same keys.

## The crash test

`cargo test -p kvlog --test crash`:

- **Kill loop.** A hidden `kvlog stress` sets `key0`, `key1`, ... and prints `ack <i>` after each
  sync. The test reads acks, sends SIGKILL after a random number, reopens the store and checks
  that every acknowledged key holds its value, with at most one more key (the write in flight).
  200 rounds on one folder by default; every second round compacts every 25 writes and is killed
  around the 25th ack, as compaction starts. Those kills land while it builds the snapshot in
  memory, not in its file steps: those are covered by the crash points below. `KVLOG_CRASH_ROUNDS` raises the count (2,000 rounds pass in release mode),
  and `KVLOG_CRASH_SEED` replays a failing run.
- **Crash points.** SIGKILL rarely lands between two particular lines, so the store has named
  abort points for the moments that matter: `half-record` (half a record written),
  `after-write` (written, not synced), `after-snapshot-sync` and `after-rename` (inside
  `compact`). `KVLOG_CRASH_AT=<name>` aborts there. They exist only when the `crash-points`
  feature is on; the crate turns it on for its own tests through a dev-dependency on itself, so
  a normal build ignores `KVLOG_CRASH_AT`.

### What this does not prove

SIGKILL is not a power cut: the operating system's page cache survives it, so a build that
skipped `fsync` would still pass. The tests prove the write ordering and the recovery code, not
the disk. Where `fsync` happens is checked by reading the code: after every appended record, after
cutting a torn tail or a failed write, after creating the log, and three times in `compact`
(snapshot, folder, emptied log). Not synced: the parent of a store folder kvlog creates itself,
so after a power cut a brand-new store could be missing; a store that already exists is not
affected.

### Checked by breaking it

- Treating a bad checksum in the middle of the log as a torn tail fails
  `payload_crc_fail_mid_is_corruption`, `flipped_byte_mid_log_refuses_and_leaves_file` and
  `corrupt_wal_exit_1_names_offset`.
- Opening without replaying the log fails all five crash tests, the kill loop included.

## Limits

Keys up to 64 KiB, values up to 16 MiB. The whole store lives in memory. One writer at a time.

## Speed

One sync per write: 2,000 acknowledged sets in 1.66 s, about 1,200 per second, on an ext4
laptop disk (AMD Ryzen 5 2500U).

## Test

```sh
cargo test -p kvlog
```
