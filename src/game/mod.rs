pub mod combat;
pub mod data;
pub mod rng;
pub mod shop;
pub mod types;

/// Bump on any change that could alter battle outcomes (items, characters, combat rules,
/// RNG usage). Stored on every `battles` row so a replay can detect when the current
/// build's rules no longer match the build that produced the recorded outcome.
pub const VERSION_HASH: u32 = 0x0000_0001;
