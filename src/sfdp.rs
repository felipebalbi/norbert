//! Flash profile model: geometry + erase menu, the pure erase planner, and the
//! JEDEC-ID fallback table for chips without SFDP. SFDP byte parsing: Task 14.

/// One supported erase granularity: `size` bytes via `opcode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseType {
    pub size: usize,
    pub opcode: u8,
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
    pub address_bytes: u8,           // 3 or 4
    pub capacity: Option<usize>,     // bytes, if known
    pub erase_types: Vec<EraseType>, // sorted largest-first
    pub source: ProfileSource,
}

impl FlashProfile {
    pub fn min_erase(&self) -> usize {
        self.erase_types
            .iter()
            .map(|e| e.size)
            .min()
            .unwrap_or(64 * 1024)
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
    sizes.sort_by_key(|e| std::cmp::Reverse(e.size));
    let smallest = *sizes.last().unwrap();

    let end = offset + len;
    let end_aligned = end.div_ceil(min) * min;
    let mut a = offset - offset % min;
    while a < end {
        let choice = sizes
            .iter()
            .find(|e| a.is_multiple_of(e.size) && a + e.size <= end_aligned)
            .copied()
            .unwrap_or(smallest);
        plan.push((a, choice.opcode));
        a += choice.size;
    }
    plan
}

/// A known SPI-NOR part that lacks SFDP, described from its datasheet.
pub struct KnownChip {
    pub jedec: [u8; 3],
    #[allow(dead_code)] // reserved for detect logging; not read yet
    pub name: &'static str,
    pub page_size: usize,
    pub address_bytes: u8,
    pub capacity: usize,
    pub erase_types: &'static [EraseType],
}

/// Fallback table — parts we support that don't self-describe via SFDP.
/// Add a row (datasheet values) to support a new no-SFDP chip.
pub static FALLBACK_TABLE: &[KnownChip] = &[
    KnownChip {
        jedec: [0x20, 0x20, 0x15],
        name: "Micron/Numonyx M25P16",
        page_size: 256,
        address_bytes: 3,
        capacity: 2 * 1024 * 1024,
        erase_types: &[EraseType {
            size: 64 * 1024,
            opcode: 0xD8,
        }],
    },
    // add more no-SFDP parts here…
];

/// Build a `FlashProfile` for a chip in the fallback table, else `None`.
pub fn lookup_fallback(jedec: [u8; 3]) -> Option<FlashProfile> {
    FALLBACK_TABLE
        .iter()
        .find(|c| c.jedec == jedec)
        .map(|c| FlashProfile {
            page_size: c.page_size,
            address_bytes: c.address_bytes,
            capacity: Some(c.capacity),
            erase_types: c.erase_types.to_vec(),
            source: ProfileSource::Table,
        })
}

pub const SFDP_SIGNATURE: [u8; 4] = *b"SFDP";

/// 8-byte SFDP header. `major`/`minor` (the SFDP revision) are decoded but not
/// yet read; only `nph` drives detection today.
#[derive(Debug, Clone, Copy)]
pub struct SfdpHeader {
    #[allow(dead_code)]
    pub major: u8,
    #[allow(dead_code)]
    pub minor: u8,
    pub nph: u8,
}

impl SfdpHeader {
    pub fn parse(b: &[u8]) -> Option<SfdpHeader> {
        if b.len() < 8 || b[0..4] != SFDP_SIGNATURE {
            return None;
        }
        Some(SfdpHeader {
            minor: b[4],
            major: b[5],
            nph: b[6],
        })
    }
    /// Parameter-header count (`nph` is the count minus one).
    pub fn param_header_count(&self) -> usize {
        self.nph as usize + 1
    }
}

/// 8-byte parameter header.
#[derive(Debug, Clone, Copy)]
pub struct ParamHeader {
    pub id: u16,
    pub length_dwords: u8,
    pub table_pointer: u32,
}

impl ParamHeader {
    pub const BFPT_ID: u16 = 0xFF00;
    pub fn parse(b: &[u8]) -> Option<ParamHeader> {
        if b.len() < 8 {
            return None;
        }
        Some(ParamHeader {
            id: ((b[7] as u16) << 8) | b[0] as u16,
            length_dwords: b[3],
            table_pointer: u32::from_le_bytes([b[4], b[5], b[6], 0]),
        })
    }
}

/// Fields decoded from the JEDEC Basic Flash Parameter Table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bfpt {
    pub address_bytes: u8,
    pub page_size: usize,
    pub capacity: Option<usize>,
    pub erase_types: Vec<EraseType>,
}

fn dword(b: &[u8], idx1: usize) -> Option<u32> {
    let off = (idx1 - 1) * 4;
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

impl Bfpt {
    /// Defensive: missing dwords fall back to sane defaults.
    pub fn parse(b: &[u8]) -> Bfpt {
        let d1 = dword(b, 1).unwrap_or(0);
        let address_bytes = if (d1 >> 17) & 0b11 == 2 { 4 } else { 3 };

        let capacity = dword(b, 2).and_then(|d2| {
            if d2 & 0x8000_0000 == 0 {
                Some((d2 as usize + 1) / 8)
            } else {
                // Defensive: garbage/corrupt SFDP can encode an out-of-range
                // exponent; only shift when representable, else unknown capacity.
                let n = d2 & 0x7FFF_FFFF;
                if (3..usize::BITS).contains(&n) {
                    Some(1usize << (n - 3))
                } else {
                    None
                }
            }
        });

        let mut erase_types = Vec::new();
        for (dw, lo) in [(8usize, true), (8, false), (9, true), (9, false)] {
            if let Some(d) = dword(b, dw) {
                let (size_field, opcode) = if lo {
                    ((d & 0xFF) as u8, ((d >> 8) & 0xFF) as u8)
                } else {
                    (((d >> 16) & 0xFF) as u8, ((d >> 24) & 0xFF) as u8)
                };
                if size_field != 0 && (size_field as u32) < usize::BITS {
                    erase_types.push(EraseType {
                        size: 1usize << size_field,
                        opcode,
                    });
                }
            }
        }
        erase_types.sort_by_key(|e| std::cmp::Reverse(e.size));

        let page_size = dword(b, 11)
            .map(|d11| 1usize << ((d11 >> 4) & 0xF))
            .filter(|&p| p >= 1)
            .unwrap_or(256);

        Bfpt {
            address_bytes,
            page_size,
            capacity,
            erase_types,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(sizes: &[(usize, u8)]) -> FlashProfile {
        FlashProfile {
            page_size: 256,
            address_bytes: 3,
            capacity: Some(2 * 1024 * 1024),
            erase_types: sizes
                .iter()
                .map(|(s, o)| EraseType {
                    size: *s,
                    opcode: *o,
                })
                .collect(),
            source: ProfileSource::Sfdp,
        }
    }

    #[test]
    fn single_granularity_plan_is_64k_blocks() {
        let p = profile_with(&[(64 * 1024, 0xD8)]);
        assert_eq!(
            plan_erase(&p, 0, 135_100),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0xD8)]
        );
    }

    #[test]
    fn mixed_granularity_uses_small_sector_at_tail() {
        let p = profile_with(&[(4 * 1024, 0x20), (32 * 1024, 0x52), (64 * 1024, 0xD8)]);
        assert_eq!(
            plan_erase(&p, 0, 131_072 + 100),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0x20)]
        );
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
        assert_eq!(
            p.erase_types,
            vec![EraseType {
                size: 64 * 1024,
                opcode: 0xD8
            }]
        );
        assert!(lookup_fallback([0xAB, 0xCD, 0xEF]).is_none()); // unknown → not supported
    }

    // Synthetic but spec-correct BFPT: 3-byte addr, 2 MiB, page 256,
    // erase 4K/0x20, 32K/0x52, 64K/0xD8.
    fn sample_bfpt() -> Vec<u8> {
        let mut b = vec![0u8; 11 * 4];
        b[0..4].copy_from_slice(&[0xE5, 0x20, 0x00, 0x00]); // dword1: 4K op=0x20, addr=3B
        b[4..8].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0x00]); // dword2: 16 Mbit density
        b[28..32].copy_from_slice(&[0x0C, 0x20, 0x0F, 0x52]); // dword8: 4K/0x20, 32K/0x52
        b[32..36].copy_from_slice(&[0x10, 0xD8, 0x00, 0x00]); // dword9: 64K/0xD8, none
        b[40..44].copy_from_slice(&[0x80, 0x00, 0x00, 0x00]); // dword11: page 2^8=256
        b
    }

    #[test]
    fn bfpt_decodes_geometry_and_erase_menu() {
        let bfpt = Bfpt::parse(&sample_bfpt());
        assert_eq!(bfpt.address_bytes, 3);
        assert_eq!(bfpt.page_size, 256);
        assert_eq!(bfpt.capacity, Some(2 * 1024 * 1024));
        assert_eq!(
            bfpt.erase_types,
            vec![
                EraseType {
                    size: 64 * 1024,
                    opcode: 0xD8
                },
                EraseType {
                    size: 32 * 1024,
                    opcode: 0x52
                },
                EraseType {
                    size: 4 * 1024,
                    opcode: 0x20
                },
            ]
        );
    }

    #[test]
    fn header_signature_check() {
        assert!(SfdpHeader::parse(&[0x53, 0x46, 0x44, 0x50, 0x06, 0x01, 0x00, 0xFF]).is_some());
        assert!(SfdpHeader::parse(&[0xFF; 8]).is_none());
    }

    #[test]
    fn bfpt_parse_survives_garbage_without_panicking() {
        // Corrupt / all-0xFF SFDP (e.g. a flaky bus or a chip with no SFDP):
        // parsing must not panic. Capacity is reported unknown and no absurd
        // erase types are produced.
        let bfpt = Bfpt::parse(&[0xFF; 11 * 4]);
        assert_eq!(bfpt.capacity, None);
        assert!(bfpt.erase_types.is_empty());
    }
}
