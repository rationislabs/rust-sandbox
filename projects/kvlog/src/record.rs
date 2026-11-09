//! Record encoding and the recovery scan: clean end, torn tail or corruption.

pub const HEADER_LEN: usize = 12;
pub const MAX_KEY: usize = 64 * 1024;
pub const MAX_VALUE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op<'a> {
    Set(&'a [u8], &'a [u8]),
    Remove(&'a [u8]),
}

const SET: u8 = 1;
const REMOVE: u8 = 2;

/// Header and payload in one buffer, so a caller writes a record with one `write_all`.
/// Does not check the size limits.
///
/// # Panics
/// When the key or the whole record is 4 GiB or more (the store's limits keep them far below).
pub fn encode(op: Op<'_>) -> Vec<u8> {
    let (code, key, value) = match op {
        Op::Set(k, v) => (SET, k, v),
        Op::Remove(k) => (REMOVE, k, &[][..]),
    };
    let mut payload = Vec::with_capacity(5 + key.len() + value.len());
    payload.push(code);
    payload.extend(
        u32::try_from(key.len())
            .expect("key within the limit")
            .to_le_bytes(),
    );
    payload.extend(key);
    payload.extend(value);
    let len = u32::try_from(payload.len())
        .expect("record within the limits")
        .to_le_bytes();
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend(len);
    out.extend(crc32fast::hash(&len).to_le_bytes());
    out.extend(crc32fast::hash(&payload).to_le_bytes());
    out.extend(payload);
    out
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(buf[at..at + 4].try_into().expect("4 bytes"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ScanEnd {
    Clean,
    Torn { at: u64 },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Corrupt {
    pub offset: u64,
    pub what: &'static str,
}

/// Walks the records in `buf`, calling `apply` for each good one, and says how the bytes end.
/// A header checksum failure is always corruption; a short or bad record is a torn tail only
/// when nothing follows it.
pub fn scan<'a>(buf: &'a [u8], mut apply: impl FnMut(Op<'a>)) -> Result<ScanEnd, Corrupt> {
    let mut pos = 0;
    loop {
        let offset = pos as u64;
        let corrupt = |what| Err(Corrupt { offset, what });
        match buf.len() - pos {
            0 => return Ok(ScanEnd::Clean),
            1..HEADER_LEN => return Ok(ScanEnd::Torn { at: offset }),
            _ => {}
        }
        if crc32fast::hash(&buf[pos..pos + 4]) != u32_at(buf, pos + 4) {
            return corrupt("header checksum");
        }
        let len = u32_at(buf, pos) as usize;
        let end = match (pos + HEADER_LEN).checked_add(len) {
            Some(end) if end <= buf.len() => end,
            _ => return Ok(ScanEnd::Torn { at: offset }),
        };
        let payload = &buf[pos + HEADER_LEN..end];
        if crc32fast::hash(payload) != u32_at(buf, pos + 8) {
            return if end == buf.len() {
                Ok(ScanEnd::Torn { at: offset })
            } else {
                corrupt("payload checksum")
            };
        }
        let Some(op) = parse(payload) else {
            return corrupt("malformed record");
        };
        apply(op);
        pos = end;
    }
}

fn parse(payload: &[u8]) -> Option<Op<'_>> {
    if payload.len() < 5 {
        return None;
    }
    let key_len = u32_at(payload, 1) as usize;
    let rest = &payload[5..];
    if key_len > rest.len() {
        return None;
    }
    let (key, value) = rest.split_at(key_len);
    match payload[0] {
        SET => Some(Op::Set(key, value)),
        REMOVE if value.is_empty() => Some(Op::Remove(key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Applied = Vec<(Vec<u8>, Option<Vec<u8>>)>;

    fn log(ops: &[Op]) -> Vec<u8> {
        ops.iter().flat_map(|op| encode(*op)).collect()
    }

    fn collect(buf: &[u8]) -> (Result<ScanEnd, Corrupt>, Applied) {
        let mut seen = Vec::new();
        let r = scan(buf, |op| match op {
            Op::Set(k, v) => seen.push((k.to_vec(), Some(v.to_vec()))),
            Op::Remove(k) => seen.push((k.to_vec(), None)),
        });
        (r, seen)
    }

    /// A record with valid checksums around any payload.
    fn raw(payload: &[u8]) -> Vec<u8> {
        let len = u32::try_from(payload.len()).unwrap().to_le_bytes();
        let mut out = len.to_vec();
        out.extend(crc32fast::hash(&len).to_le_bytes());
        out.extend(crc32fast::hash(payload).to_le_bytes());
        out.extend(payload);
        out
    }

    fn xs() -> Vec<u8> {
        log(&[
            Op::Set(b"x", b"y"),
            Op::Set(b"x", b"y"),
            Op::Set(b"x", b"y"),
        ])
    }

    #[test]
    fn header_layout() {
        let r = encode(Op::Set(b"k", b"v"));
        assert_eq!(r.len(), 19);
        assert_eq!(r[0..4], [7, 0, 0, 0]);
        assert_eq!(r[4..8], crc32fast::hash(&[7, 0, 0, 0]).to_le_bytes());
        assert_eq!(r[12..], [1, 1, 0, 0, 0, b'k', b'v']);
        assert_eq!(
            r[8..12],
            crc32fast::hash(&[1, 1, 0, 0, 0, b'k', b'v']).to_le_bytes()
        );
    }

    #[test]
    fn round_trip() {
        let (r, seen) = collect(&log(&[
            Op::Set(b"a", b"1"),
            Op::Remove(b"a"),
            Op::Set(b"", b""),
        ]));
        assert_eq!(r, Ok(ScanEnd::Clean));
        assert_eq!(
            seen,
            [
                (b"a".to_vec(), Some(b"1".to_vec())),
                (b"a".to_vec(), None),
                (vec![], Some(vec![]))
            ]
        );
    }

    #[test]
    fn short_tail_is_torn() {
        for n in 1..=11 {
            let mut buf = log(&[Op::Set(b"a", b"1"), Op::Set(b"b", b"2")]);
            buf.extend(std::iter::repeat_n(0xAB, n));
            let (r, seen) = collect(&buf);
            assert_eq!(r, Ok(ScanEnd::Torn { at: 38 }), "n = {n}");
            assert_eq!(seen.len(), 2);
        }
    }

    #[test]
    fn len_past_end_is_torn() {
        let mut buf = log(&[Op::Set(b"a", b"1"), Op::Set(b"b", b"2")]);
        buf.extend(&encode(Op::Set(b"c", b"3"))[..12]);
        assert_eq!(collect(&buf).0, Ok(ScanEnd::Torn { at: 38 }));
    }

    #[test]
    fn payload_crc_fail_last_is_torn() {
        let mut buf = xs();
        buf[56] ^= 0xFF;
        assert_eq!(collect(&buf).0, Ok(ScanEnd::Torn { at: 38 }));
    }

    #[test]
    fn payload_crc_fail_mid_is_corruption() {
        let mut buf = xs();
        buf[37] ^= 0xFF;
        assert_eq!(
            collect(&buf).0,
            Err(Corrupt {
                offset: 19,
                what: "payload checksum"
            })
        );
    }

    #[test]
    fn header_crc_fail_is_corruption_even_last() {
        let mut buf = xs();
        buf[38] ^= 0xFF;
        assert_eq!(
            collect(&buf).0,
            Err(Corrupt {
                offset: 38,
                what: "header checksum"
            })
        );
        let mut buf = xs();
        buf[0] ^= 0xFF;
        assert_eq!(
            collect(&buf).0,
            Err(Corrupt {
                offset: 0,
                what: "header checksum"
            })
        );
    }

    #[test]
    fn malformed_is_corruption() {
        let bad_op = raw(&[9, 0, 0, 0, 0]);
        let long_key = raw(&[1, 10, 0, 0, 0, b'a']);
        let remove_with_value = raw(&[2, 1, 0, 0, 0, b'k', b'v']);
        for buf in [bad_op, long_key, remove_with_value] {
            assert_eq!(
                collect(&buf).0,
                Err(Corrupt {
                    offset: 0,
                    what: "malformed record"
                })
            );
        }
    }
}
