//! Human names for flash chips — "Norbert's book". Pure tables; extend freely.

/// JEDEC manufacturer ID → name (JEP106, common subset).
pub fn manufacturer(id: u8) -> Option<&'static str> {
    Some(match id {
        0xEF => "Winbond",
        0x20 => "Micron/Numonyx",
        0xC2 => "Macronix",
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_manufacturers() {
        assert_eq!(describe([0xEF, 0x40, 0x18]), "Winbond W25Q128JV");
        assert_eq!(manufacturer(0xEF), Some("Winbond"));
        assert!(describe([0xEF, 0x40, 0x99]).starts_with("Winbond SPI NOR"));
        assert!(describe([0x77, 0x77, 0x77]).starts_with("unknown flash"));
    }
}
