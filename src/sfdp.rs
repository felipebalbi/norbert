//! SFDP byte parsing: header, parameter headers, and the Basic Flash Parameter Table.

use crate::profile::{AddressWidth, EraseType};

pub const SFDP_SIGNATURE: [u8; 4] = *b"SFDP";

/// 8-byte SFDP header. `major`/`minor` carry the SFDP revision (surfaced on the
/// resulting `FlashProfile`); `nph` drives parameter-header discovery.
#[derive(Debug, Clone, Copy)]
pub struct SfdpHeader {
    pub major: u8,
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
    pub address_width: AddressWidth,
    pub page_size: usize,
    pub capacity: Option<usize>,
    /// Decoded erase granularities, sorted largest-size first (see `parse`).
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
        let address_width = if (d1 >> 17) & 0b11 == 2 {
            AddressWidth::Four
        } else {
            AddressWidth::Three
        };

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
            address_width,
            page_size,
            capacity,
            erase_types,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::AddressWidth;

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
        assert_eq!(bfpt.address_width, AddressWidth::Three);
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
