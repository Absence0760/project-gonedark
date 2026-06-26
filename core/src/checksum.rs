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
