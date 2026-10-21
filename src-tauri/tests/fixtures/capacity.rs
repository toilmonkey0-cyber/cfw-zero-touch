//! Scaled counterfeit and genuine cards for the quick probe.
//!
//! A 64 GiB claim with 8 GiB of NAND is the usual fake-card story. The files
//! here keep that 8:1 wrap and divide both sizes by 1 MiB, so the backing
//! file is 8 KiB. Nothing in this module opens a physical disk.

use super::card::{build_card, CardOptions};

/// Advertised capacity of the unscaled counterfeit card: 64 GiB.
pub const FAKE_CLAIM_BYTES: u64 = 64 << 30;
/// NAND the unscaled counterfeit card actually has: 8 GiB.
pub const FAKE_REAL_BYTES: u64 = 8 << 30;
/// Shrinks both sizes equally. 64 GiB / 1 MiB = 64 KiB claimed, 8 KiB real.
pub const CAPACITY_SCALE: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityFixture {
    pub bytes: Vec<u8>,
    pub reported_len: u64,
}

impl CapacityFixture {
    pub fn stored_len(&self) -> u64 {
        u64::try_from(self.bytes.len()).expect("fixture length")
    }
}

/// Blank card that claims 64 GiB and stores 8 GiB, scaled down, with wraparound.
pub fn fake_card() -> CapacityFixture {
    let reported = FAKE_CLAIM_BYTES / CAPACITY_SCALE;
    let stored = FAKE_REAL_BYTES / CAPACITY_SCALE;
    CapacityFixture {
        bytes: vec![0u8; usize::try_from(stored).expect("scaled real size")],
        reported_len: reported,
    }
}

/// Partitioned disk image whose reported size is the number of bytes it stores.
pub fn genuine_card() -> CapacityFixture {
    let card = build_card(CardOptions::disarmed_original());
    CapacityFixture {
        bytes: card.bytes,
        reported_len: card.reported_len,
    }
}
