//! Flash geometry model + the pure erase planner. No I/O, no SFDP bytes.

use core::fmt;

/// Number of address bytes a part expects in read/erase/program headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressWidth {
    Three,
    Four,
}

impl AddressWidth {
    /// Header address length in bytes.
    pub fn bytes(self) -> u8 {
        match self {
            AddressWidth::Three => 3,
            AddressWidth::Four => 4,
        }
    }
    pub fn is_four_byte(self) -> bool {
        matches!(self, AddressWidth::Four)
    }
}

impl fmt::Display for AddressWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-byte", self.bytes())
    }
}

/// One supported erase granularity: `size` bytes via `opcode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseType {
    pub size: usize,
    pub opcode: u8,
}

/// A non-empty erase menu, sorted largest-size first, with unique sizes.
///
/// The invariant is upheld by `new`, so `largest`/`smallest` are total and the
/// planner never has to guess a fallback granularity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraseMenu(Vec<EraseType>);

impl EraseMenu {
    /// Build a menu, sorting largest-first and dropping duplicate sizes.
    /// `None` for empty input — a chip with no usable menu is rejected at
    /// detection, so a `FlashProfile` always holds a real menu.
    pub fn new(mut types: Vec<EraseType>) -> Option<Self> {
        if types.is_empty() {
            return None;
        }
        types.sort_by_key(|e| std::cmp::Reverse(e.size));
        types.dedup_by_key(|e| e.size);
        Some(EraseMenu(types))
    }

    /// Largest granularity (first, by the sort invariant).
    #[allow(dead_code)] // API pair of smallest(); exercised in tests, not yet read by the binary
    pub fn largest(&self) -> EraseType {
        self.0[0]
    }
    /// Smallest granularity (last, by the sort invariant).
    pub fn smallest(&self) -> EraseType {
        self.0[self.0.len() - 1]
    }
    pub fn iter(&self) -> impl Iterator<Item = EraseType> + '_ {
        self.0.iter().copied()
    }
}

/// How a `FlashProfile` was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Sfdp,
    Table,
}

/// How to talk to the flash: geometry + erase menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashProfile {
    pub page_size: usize,
    pub address_width: AddressWidth,
    pub capacity: Option<usize>, // bytes, if known
    pub erase: EraseMenu,
    pub source: ProfileSource,
    pub sfdp_revision: Option<(u8, u8)>, // (major, minor) when source == Sfdp
}

impl FlashProfile {
    pub fn min_erase(&self) -> usize {
        self.erase.smallest().size
    }
}

/// One planned erase: which granularity to apply at which address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseOp {
    pub addr: usize,
    pub ty: EraseType,
}

/// A resolved erase plan: the ordered ops covering a region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasePlan {
    ops: Vec<EraseOp>,
}

impl ErasePlan {
    #[allow(dead_code)] // exercised by the planner tests
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
    /// Number of erase operations — the progress-bar length.
    pub fn blocks(&self) -> usize {
        self.ops.len()
    }
    pub fn ops(&self) -> &[EraseOp] {
        &self.ops
    }
}

/// Plan a minimal-ish erase covering `[offset, offset+len)`. Greedily takes the
/// largest address-aligned erase type that does not overshoot past the region
/// (rounded up to the smallest granularity), else the smallest. The returned
/// `ErasePlan` is executed verbatim, in order, by `Flasher::run_erase`.
pub fn plan_erase(profile: &FlashProfile, offset: usize, len: usize) -> ErasePlan {
    let mut ops = Vec::new();
    if len == 0 {
        return ErasePlan { ops };
    }
    let min = profile.min_erase();
    let smallest = profile.erase.smallest();

    let end = offset + len;
    let end_aligned = end.div_ceil(min) * min;
    let mut a = offset - offset % min;
    while a < end {
        let ty = profile
            .erase
            .iter()
            .find(|e| a.is_multiple_of(e.size) && a + e.size <= end_aligned)
            .unwrap_or(smallest);
        ops.push(EraseOp { addr: a, ty });
        a += ty.size;
    }
    ErasePlan { ops }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(sizes: &[(usize, u8)]) -> FlashProfile {
        FlashProfile {
            page_size: 256,
            address_width: AddressWidth::Three,
            capacity: Some(2 * 1024 * 1024),
            erase: EraseMenu::new(
                sizes
                    .iter()
                    .map(|(s, o)| EraseType {
                        size: *s,
                        opcode: *o,
                    })
                    .collect(),
            )
            .expect("test menu is non-empty"),
            source: ProfileSource::Sfdp,
            sfdp_revision: None,
        }
    }

    fn plan_addrs(p: &ErasePlan) -> Vec<(usize, u8)> {
        p.ops().iter().map(|o| (o.addr, o.ty.opcode)).collect()
    }

    #[test]
    fn address_width_bytes_and_display() {
        assert_eq!(AddressWidth::Three.bytes(), 3);
        assert_eq!(AddressWidth::Four.bytes(), 4);
        assert!(AddressWidth::Four.is_four_byte());
        assert!(!AddressWidth::Three.is_four_byte());
        assert_eq!(AddressWidth::Three.to_string(), "3-byte");
    }

    #[test]
    fn erase_menu_rejects_empty() {
        assert!(EraseMenu::new(vec![]).is_none());
    }

    #[test]
    fn erase_menu_sorts_desc_and_dedups() {
        let m = EraseMenu::new(vec![
            EraseType {
                size: 4096,
                opcode: 0x20,
            },
            EraseType {
                size: 65536,
                opcode: 0xD8,
            },
            EraseType {
                size: 4096,
                opcode: 0x20,
            },
        ])
        .unwrap();
        assert_eq!(m.largest().size, 65536);
        assert_eq!(m.smallest().size, 4096);
        assert_eq!(m.iter().count(), 2); // duplicate 4096 dropped
    }

    #[test]
    fn single_granularity_plan_is_64k_blocks() {
        let p = profile_with(&[(64 * 1024, 0xD8)]);
        assert_eq!(
            plan_addrs(&plan_erase(&p, 0, 135_100)),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0xD8)]
        );
    }

    #[test]
    fn mixed_granularity_uses_small_sector_at_tail() {
        let p = profile_with(&[(4 * 1024, 0x20), (32 * 1024, 0x52), (64 * 1024, 0xD8)]);
        assert_eq!(
            plan_addrs(&plan_erase(&p, 0, 131_072 + 100)),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0x20)]
        );
    }

    #[test]
    fn zero_len_is_empty() {
        assert!(plan_erase(&profile_with(&[(64 * 1024, 0xD8)]), 0, 0).is_empty());
    }
}
