//! Norbert's voice — the ONLY module with personality.
//!
//! Rules (see the plan's "Norbert's voice"): no exclamation points; dry,
//! understated, mildly paternal; every failure carries the technical fact.
//! Pure string builders — no I/O, no logic, and absolutely no jokes elsewhere.
#![allow(dead_code)] // new builders are wired up in Phase 5

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn detect_opener() -> &'static str {
    "Hmm... let's see what we've got here."
}
pub fn found(name: &str) -> String {
    format!("Found {name}.")
}
pub fn no_flash() -> &'static str {
    "I don't see a flash chip.\n\nIs it plugged in?"
}
pub fn programming() -> &'static str {
    "Programming..."
}
pub fn programmed() -> &'static str {
    "Done. Have a nice boot."
}
pub fn verify_ok() -> &'static str {
    "Everything checks out."
}
pub fn verify_fail(addr: usize) -> String {
    format!(
        "Interesting.\n\nByte mismatch at 0x{addr:06X}.\n\nLet's not program that into production."
    )
}
pub fn erased() -> &'static str {
    "Erasing...\nDone.\n\nYou can never be too careful."
}
pub fn cancelled() -> &'static str {
    "Stopped. I've let go of the bus."
}
pub fn protected() -> &'static str {
    "Write protection is enabled.\n\nThat's not happening today."
}
pub fn protect_done() -> &'static str {
    "That should hold."
}
pub fn unprotect_done() -> &'static str {
    "There. Wide open."
}
pub fn unsupported(jedec: [u8; 3]) -> String {
    format!(
        "I don't recognize this one.\n\nJEDEC {jedec:02X?} — no SFDP, and it's not in my book yet."
    )
}
pub fn reset_done() -> &'static str {
    "There. As if nothing happened."
}
pub fn nothing_unusual() -> &'static str {
    "Nothing unusual."
}
/// doctor: the transport is up but RDID came back unreadable.
pub fn doctor_rdid_fail() -> &'static str {
    "I couldn't read the chip id.\n\nCheck the wiring and try again."
}
/// doctor: RDID drifts as the SPI clock climbs — signal-integrity hint.
pub fn doctor_unstable() -> &'static str {
    "Something's off at higher speeds.\n\nTry shorter leads or a lower --freq."
}
pub fn version() -> String {
    format!("Norbert {VERSION}\nReliable since Tuesday.")
}
pub fn info_opener() -> &'static str {
    "Let me see what this one says about itself."
}
pub fn info_sfdp_note(present: bool) -> &'static str {
    if present {
        "It told me all that itself. I appreciate a chip that keeps notes."
    } else {
        "It stayed quiet on the details, so I filled in from memory."
    }
}
pub fn sfdp_opener() -> &'static str {
    "Here's what the chip told me, byte for byte."
}
pub fn no_sfdp() -> &'static str {
    "This one has no SFDP to show.\n\nI'll rely on what I already know."
}
pub fn list_opener() -> &'static str {
    "The parts I keep notes on, in case they don't speak SFDP:"
}
pub fn list_note() -> &'static str {
    "Anything with valid SFDP, I can work out on my own."
}
pub fn read_done(bytes: usize, path: &std::path::Path) -> String {
    format!("Done. {bytes} bytes, saved to {}.", path.display())
}
pub fn programming_intro(name: &str, size: &str, offset: usize) -> String {
    format!("Programming {name} — {size} at 0x{offset:06X}.")
}
pub fn program_summary(blocks: usize, size: &str, secs: f64) -> String {
    format!("Done. Have a nice boot.  (erased {blocks} blocks, wrote {size} in {secs:.1}s)")
}
pub fn doctor_intro() -> &'static str {
    "Let's have a look. I'll take my time."
}
pub fn timeout() -> &'static str {
    "The chip stopped answering.\n\nI gave it plenty of time. Check power and wiring, then try a lower --freq."
}
pub fn not_detected() -> &'static str {
    "I haven't identified this chip yet.\n\nRun detect first."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norbert_never_shouts() {
        let lines = [
            found("Winbond W25Q128JV"),
            no_flash().to_string(),
            programming().to_string(),
            programmed().to_string(),
            verify_ok().to_string(),
            verify_fail(0x3A1280),
            erased().to_string(),
            cancelled().to_string(),
            protected().to_string(),
            detect_opener().to_string(),
            protect_done().to_string(),
            unprotect_done().to_string(),
            unsupported([0xAB, 0xCD, 0xEF]),
            reset_done().to_string(),
            doctor_rdid_fail().to_string(),
            doctor_unstable().to_string(),
            nothing_unusual().to_string(),
            version(),
            info_opener().to_string(),
            info_sfdp_note(true).to_string(),
            info_sfdp_note(false).to_string(),
            sfdp_opener().to_string(),
            no_sfdp().to_string(),
            list_opener().to_string(),
            list_note().to_string(),
            read_done(4096, std::path::Path::new("dump.bin")),
            programming_intro("Winbond W25Q128JV", "512 KiB", 0),
            program_summary(3, "512 KiB", 4.2),
            doctor_intro().to_string(),
            timeout().to_string(),
            not_detected().to_string(),
        ];
        for line in lines {
            assert!(
                !line.contains('!'),
                "Norbert used an exclamation point: {line:?}"
            );
        }
    }

    #[test]
    fn failures_carry_the_fact() {
        assert!(verify_fail(0x3A1280).contains("0x3A1280"));
    }
}
