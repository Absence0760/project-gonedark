//! Per-tick state checksum (invariant #7). The sim folds its whole state into one of
//! these every tick, in stable index order; CI diffs the streams across the platform/arch
//! matrix (docs/plans/phase-1-plan.md §6). A mismatch is a desync — a real bug, never silenced.
//! FNV-1a over little-endian bytes so the hash is endianness-stable.

/// Incremental FNV-1a 64-bit hasher.
#[derive(Clone)]
pub struct Checksum(u64);

impl Checksum {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    #[inline]
    pub const fn new() -> Self {
        Checksum(Self::OFFSET)
    }

    #[inline]
    pub fn write_u8(&mut self, b: u8) {
        self.0 = (self.0 ^ b as u64).wrapping_mul(Self::PRIME);
    }

    #[inline]
    pub fn write_i32(&mut self, v: i32) {
        for b in v.to_le_bytes() {
            self.write_u8(b);
        }
    }

    #[inline]
    pub fn write_u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.write_u8(b);
        }
    }

    #[inline]
    pub fn write_u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.write_u8(b);
        }
    }

    #[inline]
    pub fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Checksum {
    fn default() -> Self {
        Self::new()
    }
}

/// Fold a whole per-tick checksum stream into a single 64-bit digest.
///
/// The cross-arch CI diff (`determinism.yml`) compares streams **against each other**, which
/// proves every target agrees but says nothing about *what* they agree on: a change that
/// shifts sim behaviour identically on every arch passes that gate untouched. A golden test
/// closes it by comparing a stream against a **pinned** value.
///
/// Pinning all 300 tick checksums would be unreadable, and pinning only the final one would
/// miss a divergence that re-converges before the end — so the golden gate pins this digest
/// (whole-stream, order-sensitive, length-sensitive) alongside the final tick.
///
/// FNV-1a over the little-endian tick checksums, same construction as [`Checksum`] itself, so
/// the digest is endianness-stable and float-free like everything else in the sim.
pub fn digest_stream(stream: &[u64]) -> u64 {
    let mut c = Checksum::new();
    // Fold the length FIRST so a truncated stream can never digest to the same value as a
    // longer one that shares its prefix — a desync that stops the stream early is exactly the
    // case we must not let collide with a healthy short run.
    c.write_u64(stream.len() as u64);
    for &tick in stream {
        c.write_u64(tick);
    }
    c.finish()
}
