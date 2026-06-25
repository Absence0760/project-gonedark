//! Authoritative state serialization (D28, Phase 3 workstream C) — the hand-rolled
//! little-endian codec a reconnecting peer resumes from.
//!
//! This is the **second** snapshot in the engine and must not be confused with the render
//! [`snapshot`](crate::snapshot): that one is lossy (alive units only, `health.fraction()`,
//! no RNG, no free-list) and exists only for interpolation (invariant #4); it is *unfit for
//! resume*. The serialization here is **authoritative** — it captures every bit
//! [`Sim::checksum`](crate::sim::Sim::checksum) hashes (plus the liveness data the checksum
//! does not, see below), so a peer rebuilt from it computes a checksum stream
//! **bit-identical** to a never-interrupted run.
//!
//! ## Format discipline
//! [`Writer`] emits the **exact same** little-endian byte stream
//! [`checksum`](crate::checksum) folds (`u8`/`i32`/`u32`/`u64` → `to_le_bytes`), and [`Reader`]
//! is its precise inverse. This keeps `core`'s dependency list **empty** (invariant #2) — no
//! serde/bincode in the sim's determinism-critical resume path — and reuses the byte discipline
//! already proven by the checksum and the [`lockstep`](crate::lockstep) wire codec. `Fixed`
//! crosses as [`to_bits`](crate::fixed::Fixed::to_bits) / [`from_bits`](crate::fixed::Fixed::from_bits),
//! **never** as a float (invariant #1).
//!
//! ## The shared sink
//! [`StateSink`] is the abstraction that lets one field-walk drive **both** the checksum and the
//! serializer (see [`Sim::fold`](crate::sim::Sim::fold)). A [`Checksum`](crate::checksum::Checksum)
//! is a `StateSink` (it hashes the bytes); a [`Writer`] is a `StateSink` (it records them). So
//! anything folded into the checksum is serialized for free, and the two can never silently drift
//! (D28 §4).
//!
//! ## Decode discipline
//! [`Reader`] **never panics**: a malformed buffer (short read, bad enum tag, trailing bytes)
//! is a [`DeserializeError`] to handle, mirroring the [`lockstep`](crate::lockstep) decode codec
//! — a divergent world is never silently produced.

/// A sink that consumes the little-endian byte discipline shared by the checksum and the
/// serializer. The single field-walk in [`Sim::fold`](crate::sim::Sim::fold) writes to one of
/// these; a [`Checksum`](crate::checksum::Checksum) hashes the bytes, a [`Writer`] records them.
///
/// The method set is exactly [`Checksum`](crate::checksum::Checksum)'s primitive writers, so the
/// emitted byte order is identical for both sinks by construction.
pub trait StateSink {
    fn write_u8(&mut self, v: u8);
    fn write_i32(&mut self, v: i32);
    fn write_u32(&mut self, v: u32);
    fn write_u64(&mut self, v: u64);
}

impl StateSink for crate::checksum::Checksum {
    #[inline]
    fn write_u8(&mut self, v: u8) {
        crate::checksum::Checksum::write_u8(self, v)
    }
    #[inline]
    fn write_i32(&mut self, v: i32) {
        crate::checksum::Checksum::write_i32(self, v)
    }
    #[inline]
    fn write_u32(&mut self, v: u32) {
        crate::checksum::Checksum::write_u32(self, v)
    }
    #[inline]
    fn write_u64(&mut self, v: u64) {
        crate::checksum::Checksum::write_u64(self, v)
    }
}

/// A little-endian byte writer — the encode half of the codec. Emits the same `to_le_bytes`
/// stream [`checksum`](crate::checksum) folds, so a serialized field is byte-for-byte what the
/// checksum hashed.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    #[inline]
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    /// Consume the writer, yielding the encoded bytes.
    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

impl StateSink for Writer {
    #[inline]
    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    #[inline]
    fn write_i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    fn write_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
}

/// Why an authoritative snapshot could not be decoded. Decoding **never panics** — a malformed
/// buffer is an error to handle, not a crash, mirroring [`lockstep::DecodeError`](crate::lockstep)
/// (D28 §2). A peer fed a corrupt snapshot rejects it rather than silently resuming a divergent
/// world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeserializeError {
    /// The buffer ended mid-field — fewer bytes than the format requires.
    UnexpectedEof,
    /// The snapshot's format-version byte did not match the expected version.
    BadVersion(u8),
    /// An enum tag byte did not match any known variant.
    BadTag(u8),
    /// A length field named more elements than the remaining buffer could possibly hold — a
    /// garbage length, caught before it can drive a huge allocation or a runaway loop.
    LengthOverflow,
    /// The buffer parsed fully but left unconsumed trailing bytes — a sign of format/version
    /// skew. Rejecting it makes the skew loud here instead of a silent later desync.
    TrailingBytes,
    /// The buffer was well-formed (right length, known tags) but described a logically
    /// inconsistent world — e.g. a liveness/free-list mismatch, or a field value outside its
    /// valid domain. Distinct from a length/format error: the bytes parsed, the *state* is
    /// corrupt. Rejected rather than resumed (it would desync).
    CorruptState,
    /// The snapshot named a `map_id` this build does not know how to rebuild. A snapshot from a
    /// newer build that added a map is rejected **loudly** here rather than silently falling back
    /// to the wrong (default) terrain — which would desync on the first tick (invariant #7).
    UnknownMapId(crate::terrain::MapId),
}

/// A little-endian byte reader — the decode half of the codec. The exact inverse of [`Writer`];
/// every primitive read is length-checked and returns [`DeserializeError::UnexpectedEof`] rather
/// than panicking on a short buffer.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// Bytes still unread.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Reject the buffer unless every byte has been consumed. Called at the end of a decode so
    /// trailing bytes (a format/version skew) fail loudly instead of desyncing later.
    #[inline]
    pub fn expect_end(&self) -> Result<(), DeserializeError> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(DeserializeError::TrailingBytes)
        }
    }

    #[inline]
    fn take(&mut self, n: usize) -> Result<&'a [u8], DeserializeError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(DeserializeError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(DeserializeError::UnexpectedEof);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    #[inline]
    pub fn read_u8(&mut self) -> Result<u8, DeserializeError> {
        Ok(self.take(1)?[0])
    }

    #[inline]
    pub fn read_i32(&mut self) -> Result<i32, DeserializeError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    #[inline]
    pub fn read_u32(&mut self) -> Result<u32, DeserializeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    #[inline]
    pub fn read_u64(&mut self) -> Result<u64, DeserializeError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Read a `u32` length and validate it against the bytes that remain: a count whose smallest
    /// possible encoding (`min_elem_bytes` each) would overrun the buffer is rejected as
    /// [`DeserializeError::LengthOverflow`]. This caps pre-allocation from a garbage length the
    /// same way the lockstep codec does, *before* a `Vec::with_capacity` or a long loop.
    #[inline]
    pub fn read_len(&mut self, min_elem_bytes: usize) -> Result<usize, DeserializeError> {
        let n = self.read_u32()? as usize;
        if let Some(max_possible) = self.remaining().checked_div(min_elem_bytes) {
            // `min_elem_bytes == 0` (a tag-only element) yields `None`, skipping the bound.
            if n > max_possible {
                return Err(DeserializeError::LengthOverflow);
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum::Checksum;

    #[test]
    fn writer_emits_little_endian_matching_checksum() {
        // The Writer must emit the exact LE byte stream the Checksum folds — that equivalence is
        // what makes the shared field-walk safe (a serialized field == the checksummed bytes).
        let mut w = Writer::new();
        w.write_u8(0xAB);
        w.write_i32(-2);
        w.write_u32(0xDEAD_BEEF);
        w.write_u64(0x0123_4567_89AB_CDEF);
        let bytes = w.into_bytes();

        let mut expected = Vec::new();
        expected.push(0xABu8);
        expected.extend_from_slice(&(-2i32).to_le_bytes());
        expected.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        expected.extend_from_slice(&0x0123_4567_89AB_CDEFu64.to_le_bytes());
        assert_eq!(bytes, expected);

        // And hashing those same bytes through a Checksum sink yields the same digest as folding
        // the primitives directly — i.e. Writer and Checksum agree on the byte stream.
        let mut from_bytes = Checksum::new();
        for &b in &bytes {
            from_bytes.write_u8(b);
        }
        let mut direct = Checksum::new();
        StateSink::write_u8(&mut direct, 0xAB);
        StateSink::write_i32(&mut direct, -2);
        StateSink::write_u32(&mut direct, 0xDEAD_BEEF);
        StateSink::write_u64(&mut direct, 0x0123_4567_89AB_CDEF);
        assert_eq!(from_bytes.finish(), direct.finish());
    }

    #[test]
    fn reader_round_trips_each_primitive() {
        let mut w = Writer::new();
        w.write_u8(7);
        w.write_i32(i32::MIN);
        w.write_u32(u32::MAX);
        w.write_u64(u64::MAX);
        let bytes = w.into_bytes();

        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_u8().unwrap(), 7);
        assert_eq!(r.read_i32().unwrap(), i32::MIN);
        assert_eq!(r.read_u32().unwrap(), u32::MAX);
        assert_eq!(r.read_u64().unwrap(), u64::MAX);
        assert_eq!(r.remaining(), 0);
        r.expect_end().unwrap();
    }

    #[test]
    fn reader_rejects_short_buffer() {
        let mut r = Reader::new(&[0u8, 1, 2]); // 3 bytes, ask for a u32 (4)
        assert_eq!(r.read_u32().unwrap_err(), DeserializeError::UnexpectedEof);
        // A u64 from an empty reader also fails cleanly.
        let mut r = Reader::new(&[]);
        assert_eq!(r.read_u64().unwrap_err(), DeserializeError::UnexpectedEof);
    }

    #[test]
    fn reader_rejects_trailing_bytes() {
        let bytes = [1u8, 0, 0, 0, 0xFF]; // a u32 then a stray byte
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_u32().unwrap(), 1);
        assert_eq!(r.expect_end().unwrap_err(), DeserializeError::TrailingBytes);
    }

    #[test]
    fn read_len_rejects_overflowing_count() {
        // Claim 1000 elements of 4 bytes each, but provide only 4 bytes after the length.
        let mut w = Writer::new();
        w.write_u32(1000);
        w.write_u32(0); // one element's worth of payload
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_len(4).unwrap_err(), DeserializeError::LengthOverflow);

        // A length that fits is accepted.
        let mut w = Writer::new();
        w.write_u32(1);
        w.write_u32(42);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_len(4).unwrap(), 1);
        assert_eq!(r.read_u32().unwrap(), 42);

        // A zero-min-size element (e.g. a tag-only variant) skips the bound check.
        let mut w = Writer::new();
        w.write_u32(5);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_len(0).unwrap(), 5);
    }
}
