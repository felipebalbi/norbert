//! "Norbert's book": human names for flash chips + the no-SFDP fallback table. Pure tables; extend freely.

use crate::profile::{AddressWidth, EraseMenu, EraseType, FlashProfile, ProfileSource};

/// JEDEC manufacturer ID → name (JEP106, common subset).
pub fn manufacturer(id: u8) -> Option<&'static str> {
    Some(match id {
        0xEF => "Winbond",
        0x20 => "Micron/Numonyx",
        0xC2 => "Macronix",
        0xC8 => "GigaDevice",
        0x1C => "EON",
        0x01 => "Cypress/Spansion",
        0xBF => "SST",
        0x9D => "ISSI",
        0x89 => "Intel",
        0x8C => "ESMT",
        0x68 => "Boya",
        _ => return None,
    })
}

/// Known part by full 3-byte JEDEC ID (pretty names for `detect`/`list`).
pub struct NamedChip {
    pub jedec: [u8; 3],
    pub name: &'static str,
}
pub static CHIP_NAMES: &[NamedChip] = &[
    NamedChip {
        jedec: [0xEF, 0x40, 0x18],
        name: "Winbond W25Q128JV",
    },
    NamedChip {
        jedec: [0xEF, 0x40, 0x17],
        name: "Winbond W25Q64",
    },
    NamedChip {
        jedec: [0xEF, 0x40, 0x16],
        name: "Winbond W25Q32",
    },
    NamedChip {
        jedec: [0x20, 0x20, 0x15],
        name: "Micron M25P16",
    },
    NamedChip {
        jedec: [0x1C, 0x70, 0x15],
        name: "EON EN25QH16B",
    },
    NamedChip {
        jedec: [0xC2, 0x20, 0x18],
        name: "Macronix MX25L128",
    },
    NamedChip {
        jedec: [0xC8, 0x40, 0x15],
        name: "GigaDevice GD25Q16",
    },
    // add more as you meet them…
];

/// Best-effort label: exact part if known, else "<Manufacturer> SPI NOR (id)", else raw id.
pub fn describe(jedec: [u8; 3]) -> String {
    if let Some(c) = CHIP_NAMES.iter().find(|c| c.jedec == jedec) {
        return c.name.to_string();
    }
    match manufacturer(jedec[0]) {
        Some(m) => format!(
            "{m} SPI NOR ({:02X} {:02X} {:02X})",
            jedec[0], jedec[1], jedec[2]
        ),
        None => format!(
            "unknown flash ({:02X} {:02X} {:02X})",
            jedec[0], jedec[1], jedec[2]
        ),
    }
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
            address_width: if c.address_bytes == 4 {
                AddressWidth::Four
            } else {
                AddressWidth::Three
            },
            capacity: Some(c.capacity),
            erase: EraseMenu::new(c.erase_types.to_vec())
                .expect("fallback-table entries have a non-empty erase menu"),
            source: ProfileSource::Table,
            sfdp_revision: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AddressWidth, EraseType, ProfileSource};

    #[test]
    fn m25p16_is_in_the_fallback_table() {
        let p = lookup_fallback([0x20, 0x20, 0x15]).expect("M25P16 is a known chip");
        assert_eq!(p.source, ProfileSource::Table);
        assert_eq!(p.address_width, AddressWidth::Three);
        assert_eq!(p.capacity, Some(2 * 1024 * 1024));
        assert_eq!(
            p.erase.iter().collect::<Vec<_>>(),
            vec![EraseType {
                size: 64 * 1024,
                opcode: 0xD8
            }]
        );
        assert!(lookup_fallback([0xAB, 0xCD, 0xEF]).is_none()); // unknown → not supported
    }

    #[test]
    fn every_fallback_row_has_a_usable_erase_menu() {
        // lookup_fallback `.expect`s a non-empty erase menu; guard the table so a
        // bad new row fails CI here instead of panicking in the field on `detect`.
        for c in FALLBACK_TABLE {
            assert!(
                lookup_fallback(c.jedec).is_some(),
                "fallback row {:02X?} has an empty erase menu",
                c.jedec
            );
        }
    }

    #[test]
    fn names_and_manufacturers() {
        assert_eq!(describe([0xEF, 0x40, 0x18]), "Winbond W25Q128JV");
        assert_eq!(manufacturer(0xEF), Some("Winbond"));
        assert!(describe([0xEF, 0x40, 0x99]).starts_with("Winbond SPI NOR"));
        assert!(describe([0x77, 0x77, 0x77]).starts_with("unknown flash"));

        // GigaDevice GD25Q16 (JEDEC C8 40 15): SFDP-capable but was unnamed.
        assert_eq!(manufacturer(0xC8), Some("GigaDevice"));
        assert_eq!(describe([0xC8, 0x40, 0x15]), "GigaDevice GD25Q16");
        // Known vendor, unknown exact part → vendor-tagged, not "unknown flash".
        assert!(describe([0xC8, 0x40, 0x99]).starts_with("GigaDevice SPI NOR"));
    }
}
