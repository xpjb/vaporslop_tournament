//! Deterministic 32-bit RNG used by the simulation.
//!
//! Single `u32` of state advanced by a knuth-multiplier xorshift hash. We keep
//! this in-tree (rather than pulling in `rand`'s seedable generators) so the
//! algorithm is pinned forever — a battle is byte-for-byte reproducible from
//! `(builds, seed)` across crate updates and platforms.
//!
//! Wrapping in a newtype prevents the foot-gun of accidentally `Copy`-ing
//! state and forking a stream silently.

/// PRNG with `u32` state. Clone-on-purpose only.
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// Seed from `rand`'s thread-local entropy pool. One `u32` tap, no
    /// syscalls, no clock-collision risk — `rand` is already in our deps
    /// for auth, so this costs nothing extra.
    pub fn new_random() -> Self {
        use rand::Rng as _;
        Self::new(rand::thread_rng().gen())
    }

    fn advance(&mut self) -> u32 {
        self.0 = step(self.0);
        self.0
    }

    pub fn next_u32(&mut self) -> u32 {
        self.advance()
    }

    /// Uniform in `[0.0, 1.0]`.
    pub fn next_f32(&mut self) -> f32 {
        self.advance() as f32 / u32::MAX as f32
    }

    /// `true` with probability `p` (clamped to `[0, 1]`).
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p.clamp(0.0, 1.0)
    }

    /// Uniform index in `[0, len)`. Returns `None` for an empty range so
    /// callers can't silently mod-by-zero.
    pub fn index(&mut self, len: usize) -> Option<usize> {
        if len == 0 {
            None
        } else {
            Some((self.advance() as usize) % len)
        }
    }

    pub fn choice<'a, T>(&mut self, slice: &'a [T]) -> Option<&'a T> {
        self.index(slice.len()).map(|i| &slice[i])
    }
}

#[inline]
fn step(seed: u32) -> u32 {
    let mut s = seed ^ 2_747_636_419;
    s = s.wrapping_mul(2_654_435_769);
    s = (s ^ (s >> 16)).wrapping_mul(2_654_435_769);
    s = (s ^ (s >> 16)).wrapping_mul(2_654_435_769);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_produces_same_stream() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn f32_stays_in_unit_interval() {
        let mut r = Rng::new(123);
        for _ in 0..10_000 {
            let x = r.next_f32();
            assert!((0.0..=1.0).contains(&x));
        }
    }
}
