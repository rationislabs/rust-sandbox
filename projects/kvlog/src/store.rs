//! The store: snapshot plus write-ahead log, an exclusive lock, fsync before a write counts.

use crate::record::{HEADER_LEN, MAX_KEY, MAX_VALUE, Op, ScanEnd, encode, scan};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Error {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Locked,
    Corrupt {
        file: PathBuf,
        offset: u64,
        what: &'static str,
    },
    TooLarge {
        what: &'static str,
        len: usize,
        max: usize,
    },
    Poisoned,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Locked => f.write_str("the store is in use"),
            Error::Corrupt { file, offset, what } => {
                write!(
                    f,
                    "{}: corrupt at byte {offset} ({what}); not opening the store",
                    file.display()
                )
            }
            Error::TooLarge { what, len, max } => {
                write!(f, "{what} is {len} bytes; the limit is {max}")
            }
            Error::Poisoned => f.write_str("an earlier write failed; reopen the store"),
        }
    }
}

impl std::error::Error for Error {}

pub struct Store {
    dir: PathBuf,
    map: BTreeMap<Vec<u8>, Vec<u8>>,
    wal: File,
    wal_len: u64,
    _lock: File,
    torn_bytes: u64,
    poisoned: bool,
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

fn encoded_len(op: Op<'_>) -> u64 {
    let body = match op {
        Op::Set(k, v) => k.len() + v.len(),
        Op::Remove(k) => k.len(),
    };
    (HEADER_LEN + 5 + body) as u64
}

impl Store {
    /// Takes the lock, loads the snapshot, replays the log and cuts off a torn tail. Anything
    /// else that is wrong is corruption: nothing is changed and the store does not open.
    pub fn open(dir: &Path) -> Result<Store, Error> {
        fs::create_dir_all(dir).map_err(io_at(dir))?;
        let lock_path = dir.join("LOCK");
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(io_at(&lock_path))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(Error::Locked),
            Err(TryLockError::Error(e)) => return Err(io_at(&lock_path)(e)),
        }

        let mut map = BTreeMap::new();
        let snap_path = dir.join("snapshot");
        match fs::read(&snap_path) {
            Ok(bytes) => {
                let (mut pos, mut removed_at) = (0, None);
                let end = scan(&bytes, |op| {
                    match op {
                        Op::Set(k, v) => {
                            map.insert(k.to_vec(), v.to_vec());
                        }
                        Op::Remove(_) => {
                            removed_at.get_or_insert(pos);
                        }
                    }
                    pos += encoded_len(op);
                });
                let corrupt = |offset, what| Error::Corrupt {
                    file: snap_path.clone(),
                    offset,
                    what,
                };
                match (end, removed_at) {
                    (Err(c), _) => return Err(corrupt(c.offset, c.what)),
                    (Ok(ScanEnd::Torn { at }), _) => {
                        return Err(corrupt(at, "snapshot ends mid-record"));
                    }
                    (Ok(ScanEnd::Clean), Some(at)) => {
                        return Err(corrupt(at, "remove in snapshot"));
                    }
                    (Ok(ScanEnd::Clean), None) => {}
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_at(&snap_path)(e)),
        }

        let wal_path = dir.join("wal");
        let created = !wal_path.exists();
        let mut wal = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&wal_path)
            .map_err(io_at(&wal_path))?;
        let mut bytes = Vec::new();
        wal.read_to_end(&mut bytes).map_err(io_at(&wal_path))?;
        let end = scan(&bytes, |op| match op {
            Op::Set(k, v) => {
                map.insert(k.to_vec(), v.to_vec());
            }
            Op::Remove(k) => {
                map.remove(k);
            }
        });
        let len = bytes.len() as u64;
        let (wal_len, torn_bytes) = match end {
            Err(c) => {
                return Err(Error::Corrupt {
                    file: wal_path,
                    offset: c.offset,
                    what: c.what,
                });
            }
            Ok(ScanEnd::Torn { at }) => {
                wal.set_len(at)
                    .and_then(|()| wal.sync_all())
                    .map_err(io_at(&wal_path))?;
                (at, len - at)
            }
            Ok(ScanEnd::Clean) => (len, 0),
        };
        if created {
            // the new directory entry is durable too
            File::open(dir)
                .and_then(|d| d.sync_all())
                .map_err(io_at(dir))?;
        }
        Ok(Store {
            dir: dir.to_owned(),
            map,
            wal,
            wal_len,
            _lock: lock,
            torn_bytes,
            poisoned: false,
        })
    }

    /// Returns only after the record is on disk (`sync_data`); a failed write is cut back off.
    fn append(&mut self, rec: &[u8]) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let path = self.dir.join("wal");
        if let Err(e) = self.wal.write_all(rec).and_then(|()| self.wal.sync_data()) {
            if self
                .wal
                .set_len(self.wal_len)
                .and_then(|()| self.wal.sync_all())
                .is_err()
            {
                self.poisoned = true;
            }
            return Err(io_at(&path)(e));
        }
        self.wal_len += rec.len() as u64;
        Ok(())
    }

    pub fn set(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        if key.len() > MAX_KEY {
            return Err(Error::TooLarge {
                what: "key",
                len: key.len(),
                max: MAX_KEY,
            });
        }
        if value.len() > MAX_VALUE {
            return Err(Error::TooLarge {
                what: "value",
                len: value.len(),
                max: MAX_VALUE,
            });
        }
        self.append(&encode(Op::Set(key, value)))?;
        self.map.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.map.get(key).map(Vec::as_slice)
    }

    /// `Ok(false)` for an unknown key, and nothing is written.
    pub fn remove(&mut self, key: &[u8]) -> Result<bool, Error> {
        if !self.map.contains_key(key) {
            return Ok(false);
        }
        self.append(&encode(Op::Remove(key)))?;
        self.map.remove(key);
        Ok(true)
    }

    /// Sorted by key.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.map.iter().map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    /// Writes every key to a new snapshot, swaps it in, then empties the log. A crash at any
    /// point leaves a state that opens to the same keys.
    pub fn compact(&mut self) -> Result<(), Error> {
        let tmp = self.dir.join("snapshot.tmp");
        let snap = self.dir.join("snapshot");
        let wal = self.dir.join("wal");
        let bytes: Vec<u8> = self
            .map
            .iter()
            .flat_map(|(k, v)| encode(Op::Set(k, v)))
            .collect();
        let mut f = File::create(&tmp).map_err(io_at(&tmp))?;
        f.write_all(&bytes)
            .and_then(|()| f.sync_all())
            .map_err(io_at(&tmp))?;
        fs::rename(&tmp, &snap).map_err(io_at(&snap))?;
        File::open(&self.dir)
            .and_then(|d| d.sync_all())
            .map_err(io_at(&self.dir))?;
        self.wal
            .set_len(0)
            .and_then(|()| self.wal.sync_all())
            .map_err(io_at(&wal))?;
        self.wal_len = 0;
        Ok(())
    }

    /// Bytes of an unfinished write cut off the end of the log when the store opened.
    pub fn torn_bytes(&self) -> u64 {
        self.torn_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kvlog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn keys(dir: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
        let s = Store::open(dir).unwrap();
        s.iter().map(|(k, v)| (k.to_vec(), v.to_vec())).collect()
    }

    fn kv(pairs: &[(&str, &str)]) -> Vec<(Vec<u8>, Vec<u8>)> {
        pairs
            .iter()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    fn len(path: PathBuf) -> u64 {
        fs::metadata(path).unwrap().len()
    }

    /// set a=1, b=2, c=3: 57 bytes of wal, the last record at 38.
    fn abc(dir: &Path) -> Vec<u8> {
        let mut s = Store::open(dir).unwrap();
        for (k, v) in [("a", "1"), ("b", "2"), ("c", "3")] {
            s.set(k.as_bytes(), v.as_bytes()).unwrap();
        }
        drop(s);
        fs::read(dir.join("wal")).unwrap()
    }

    #[test]
    fn set_get_remove_reopen() {
        let d = tmp("sgr");
        let mut s = Store::open(&d).unwrap();
        s.set(b"a", b"1").unwrap();
        s.set(b"b", b"2").unwrap();
        assert_eq!(s.get(b"a"), Some(&b"1"[..]));
        assert!(s.remove(b"a").unwrap());
        let before = len(d.join("wal"));
        assert!(!s.remove(b"zz").unwrap());
        assert_eq!(len(d.join("wal")), before);
        drop(s);
        assert_eq!(keys(&d), kv(&[("b", "2")]));
    }

    #[test]
    fn cut_at_every_offset_of_last_record() {
        let full = abc(&tmp("cut-src"));
        assert_eq!(full.len(), 57);
        for cut in 38..57 {
            let d = tmp(&format!("cut-{cut}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("wal"), &full[..cut]).unwrap();
            let mut s = Store::open(&d).unwrap();
            assert_eq!(s.torn_bytes(), cut as u64 - 38, "cut {cut}");
            assert_eq!(s.iter().count(), 2, "cut {cut}");
            assert_eq!(len(d.join("wal")), 38, "cut {cut}");
            s.set(b"c", b"3").unwrap();
            drop(s);
            assert_eq!(
                keys(&d),
                kv(&[("a", "1"), ("b", "2"), ("c", "3")]),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn flipped_byte_mid_log_refuses_and_leaves_file() {
        let d = tmp("flip");
        let mut wal = abc(&d);
        wal[37] ^= 0xFF;
        fs::write(d.join("wal"), &wal).unwrap();
        match Store::open(&d) {
            Err(Error::Corrupt {
                file, offset: 19, ..
            }) => assert!(file.ends_with("wal")),
            other => panic!("{:?}", other.err()),
        }
        assert_eq!(fs::read(d.join("wal")).unwrap(), wal);
    }

    #[test]
    fn bad_snapshot_is_corruption() {
        let d = tmp("badsnap");
        fs::create_dir_all(&d).unwrap();
        let mut rec = crate::record::encode(crate::record::Op::Set(b"a", b"1"));
        *rec.last_mut().unwrap() ^= 0xFF;
        fs::write(d.join("snapshot"), &rec).unwrap();
        match Store::open(&d) {
            Err(Error::Corrupt {
                file, offset: 0, ..
            }) => assert!(file.ends_with("snapshot")),
            other => panic!("{:?}", other.err()),
        }
        assert!(!d.join("wal").exists());
        assert_eq!(fs::read(d.join("snapshot")).unwrap(), rec);

        let rec = crate::record::encode(crate::record::Op::Remove(b"a"));
        fs::write(d.join("snapshot"), &rec).unwrap();
        assert!(matches!(
            Store::open(&d),
            Err(Error::Corrupt {
                what: "remove in snapshot",
                ..
            })
        ));
        assert!(!d.join("wal").exists());
        assert_eq!(fs::read(d.join("snapshot")).unwrap(), rec);
    }

    #[test]
    fn second_open_is_locked() {
        let d = tmp("lock");
        let first = Store::open(&d).unwrap();
        assert!(matches!(Store::open(&d), Err(Error::Locked)));
        drop(first);
        assert!(Store::open(&d).is_ok());
    }

    #[test]
    fn compact_keeps_state() {
        let d = tmp("compact");
        let mut s = Store::open(&d).unwrap();
        for k in ["a", "b", "c"] {
            s.set(k.as_bytes(), k.as_bytes()).unwrap();
        }
        s.remove(b"a").unwrap();
        s.compact().unwrap();
        assert!(d.join("snapshot").exists() && !d.join("snapshot.tmp").exists());
        assert_eq!(len(d.join("wal")), 0);
        drop(s);
        assert_eq!(keys(&d), kv(&[("b", "b"), ("c", "c")]));
        Store::open(&d).unwrap().set(b"d", b"d").unwrap();
        assert_eq!(keys(&d), kv(&[("b", "b"), ("c", "c"), ("d", "d")]));
    }

    #[test]
    fn stale_snapshot_tmp_is_ignored() {
        let d = tmp("stale");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("snapshot.tmp"), b"junk").unwrap();
        Store::open(&d).unwrap().set(b"a", b"1").unwrap();
        assert_eq!(keys(&d), kv(&[("a", "1")]));
        Store::open(&d).unwrap().compact().unwrap();
        assert_eq!(keys(&d), kv(&[("a", "1")]));
        assert!(!d.join("snapshot.tmp").exists());
    }

    #[test]
    fn limits() {
        let d = tmp("limits");
        let mut s = Store::open(&d).unwrap();
        s.set(&vec![b'k'; 65_536], b"v").unwrap();
        let before = len(d.join("wal"));
        assert!(matches!(
            s.set(&vec![b'k'; 65_537], b"v"),
            Err(Error::TooLarge {
                what: "key",
                len: 65_537,
                max: 65_536
            })
        ));
        assert!(matches!(
            s.set(b"k", &vec![0; 16_777_217]),
            Err(Error::TooLarge { what: "value", .. })
        ));
        assert_eq!(len(d.join("wal")), before);
    }
}
