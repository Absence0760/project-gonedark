//! Deterministic, seeded PRNG for the lockstep sim (invariant #1). A PCG32 variant: same
//! seed and call sequence → identical stream on every peer and arch. Output is integer
//! only; the sim never derives a float from it.

/// PCG32 generator. Cheap, well-distributed, fully deterministic.
#[derive(Clone)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    /// Seed the generator. The increment is a fixed odd stream selector.
    pub const fn new(seed: u64) -> Self {
        Rng {
            state: seed ^ 0x853c49e6748fea9b,
            inc: 0xda3e39cb94b95bdb,
        }
    }

    /// Next 32 bits.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.inc | 1);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Next 64 bits.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        ((self.next_u32() as u64) << 32) | (self.next_u32() as u64)
    }

    /// Uniform integer in `[0, bound)` via Lemire's multiply-shift (bound > 0).
    #[inline]
    pub fn below(&mut self, bound: u32) -> u32 {
        ((self.next_u32() as u64 * bound as u64) >> 32) as u32
    }

    /// The raw generator state `(state, inc)`, for folding into the per-tick checksum
    /// (invariant #7). Folding it makes any divergence in the *number* of draws between peers —
    /// the classic lockstep desync symptom, e.g. a unit alive on one peer but dead on another
    /// skipping a roll — visible immediately, instead of only later through its downstream
    /// effect on health/positions. Read-only; does not advance the stream.
    #[inline]
    pub fn checksum_state(&self) -> (u64, u64) {
        (self.state, self.inc)
    }
}
