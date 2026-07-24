//! Flash profile model: geometry + erase menu, the pure erase planner, and the
//! JEDEC-ID fallback table for chips without SFDP. SFDP byte parsing: Task 14.

/// One supported erase granularity: `size` bytes via `opcode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseType {
    pub size: usize,
    pub opcode: u8,
}

/// How a `FlashProfile` was resolved.
#[allow(dead_code)] // wired in Task 15 (detect tags each resolved profile's origin)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Sfdp,
    Table,
}

/// How to talk to the flash: geometry + erase menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashProfile {
    pub page_size: usize,
    pub address_bytes: u8,           // 3 or 4
    pub capacity: Option<usize>,     // bytes, if known
    pub erase_types: Vec<EraseType>, // sorted largest-first
    pub source: ProfileSource,
}

impl FlashProfile {
    pub fn min_erase(&self) -> usize {
        self.erase_types.iter().map(|e| e.size).min().unwrap_or(64 * 1024)
    }
}

/// Plan a minimal-ish erase covering `[offset, offset+len)`. Greedily takes the
/// largest address-aligned erase type that does not overshoot past the region
/// (rounded up to the smallest granularity), else the smallest. `(addr, opcode)`.
pub fn plan_erase(profile: &FlashProfile, offset: usize, len: usize) -> Vec<(usize, u8)> {
    let mut plan = Vec::new();
    if len == 0 || profile.erase_types.is_empty() {
        return plan;
    }
    let min = profile.min_erase();
    let mut sizes = profile.erase_types.clone();
    sizes.sort_by(|a, b| b.size.cmp(&a.size));
    let smallest = *sizes.last().unwrap();

    let end = offset + len;
    let end_aligned = end.div_ceil(min) * min;
    let mut a = offset - offset % min;
    while a < end {
        let choice = sizes
            .iter()
            .find(|e| a % e.size == 0 && a + e.size <= end_aligned)
            .copied()
            .unwrap_or(smallest);
        plan.push((a, choice.opcode));
        a += choice.size;
    }
    plan
}

/// A known SPI-NOR part that lacks SFDP, described from its datasheet.
#[allow(dead_code)] // wired in Task 15 (detect: JEDEC fallback-table lookup); `name` for detect logging
pub struct KnownChip {
    pub jedec: [u8; 3],
    pub name: &'static str,
    pub page_size: usize,
    pub address_bytes: u8,
    pub capacity: usize,
    pub erase_types: &'static [EraseType],
}

/// Fallback table — parts we support that don't self-describe via SFDP.
/// Add a row (datasheet values) to support a new no-SFDP chip.
#[allow(dead_code)] // wired in Task 15 (detect: JEDEC fallback-table lookup)
pub static FALLBACK_TABLE: &[KnownChip] = &[
    KnownChip {
        jedec: [0x20, 0x20, 0x15], name: "Micron/Numonyx M25P16",
        page_size: 256, address_bytes: 3, capacity: 2 * 1024 * 1024,
        erase_types: &[EraseType { size: 64 * 1024, opcode: 0xD8 }],
    },
    // add more no-SFDP parts here…
];

/// Build a `FlashProfile` for a chip in the fallback table, else `None`.
#[allow(dead_code)] // wired in Task 15 (detect: JEDEC fallback-table lookup)
pub fn lookup_fallback(jedec: [u8; 3]) -> Option<FlashProfile> {
    FALLBACK_TABLE.iter().find(|c| c.jedec == jedec).map(|c| FlashProfile {
        page_size: c.page_size,
        address_bytes: c.address_bytes,
        capacity: Some(c.capacity),
        erase_types: c.erase_types.to_vec(),
        source: ProfileSource::Table,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(sizes: &[(usize, u8)]) -> FlashProfile {
        FlashProfile {
            page_size: 256, address_bytes: 3, capacity: Some(2 * 1024 * 1024),
            erase_types: sizes.iter().map(|(s, o)| EraseType { size: *s, opcode: *o }).collect(),
            source: ProfileSource::Sfdp,
        }
    }

    #[test]
    fn single_granularity_plan_is_64k_blocks() {
        let p = profile_with(&[(64 * 1024, 0xD8)]);
        assert_eq!(plan_erase(&p, 0, 135_100), vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0xD8)]);
    }

    #[test]
    fn mixed_granularity_uses_small_sector_at_tail() {
        let p = profile_with(&[(4 * 1024, 0x20), (32 * 1024, 0x52), (64 * 1024, 0xD8)]);
        assert_eq!(plan_erase(&p, 0, 131_072 + 100),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0x20)]);
    }

    #[test]
    fn zero_len_is_empty() {
        assert!(plan_erase(&profile_with(&[(64 * 1024, 0xD8)]), 0, 0).is_empty());
    }

    #[test]
    fn m25p16_is_in_the_fallback_table() {
        let p = lookup_fallback([0x20, 0x20, 0x15]).expect("M25P16 is a known chip");
        assert_eq!(p.source, ProfileSource::Table);
        assert_eq!(p.address_bytes, 3);
        assert_eq!(p.capacity, Some(2 * 1024 * 1024));
        assert_eq!(p.erase_types, vec![EraseType { size: 64 * 1024, opcode: 0xD8 }]);
        assert!(lookup_fallback([0xAB, 0xCD, 0xEF]).is_none()); // unknown → not supported
    }
}
