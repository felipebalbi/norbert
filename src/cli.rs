//! Command-line surface: global flags + subcommands. Parsing only.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "norbert",
    about = "A patient SPI-NOR flasher",
    disable_version_flag = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Pick a specific Pico de Gallo by USB serial number.
    #[arg(long, global = true)]
    pub serial: Option<String>,
    /// SPI clock in Hz (USB-FS bound; 10 MHz is plenty).
    #[arg(long, global = true, default_value_t = 10_000_000)]
    pub freq: u32,
    /// Hold a shared bus master off the SPI while programming, then release it so it
    /// boots. Wire the target's CRESET to User GPIO 3 (header pin 14). Omit for a
    /// bare chip or any flash with no other bus master on the SPI.
    #[arg(long, global = true)]
    pub reset: bool,
    /// Machine-friendly output: drop the commentary, print IDs/addresses/OK/FAIL only.
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Print version.
    #[arg(short = 'V', long = "version", global = true)]
    pub version: bool,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Read the raw 3-byte JEDEC ID.
    Jedec,
    /// Erase + program + verify a bitstream at an offset, then boot it.
    Program {
        bitstream: PathBuf,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Skip read-back verification.
        #[arg(long)]
        no_verify: bool,
        /// Full chip erase instead of just the covered 64 KiB blocks.
        #[arg(long)]
        chip_erase: bool,
        /// Clear status-register block-protection (BP) bits before erase/program.
        #[arg(long)]
        unprotect: bool,
    },
    /// Dump `length` bytes from `offset` to a file.
    Read {
        out: PathBuf,
        #[arg(long)]
        length: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Compare flash contents against a file.
    Verify {
        bitstream: PathBuf,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Erase (covered blocks for a size, or the whole chip).
    Erase {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        length: Option<usize>,
        #[arg(long)]
        chip: bool,
    },
    /// Detect and print the flash profile the tool will use (SFDP or fallback table).
    #[command(alias = "discover")]
    Detect,
    /// Read + print JEDEC ID and SFDP/profile info.
    Info,
    /// Dump raw SFDP and the decoded BFPT.
    Sfdp,
    /// List the chips Norbert knows without SFDP (the fallback table).
    List,
    /// Set status-register block-protection bits.
    Protect,
    /// Clear status-register block-protection bits.
    Unprotect,
    /// Flash soft-reset (0x66/0x99); also reboots a held master.
    Reset,
    /// Wiring/power/speed check-up (read-only).
    Doctor,
    /// Read-back consistency check; with --sector N, a destructive sector self-test.
    Test {
        /// Destructively test sector N (backup, erase, pattern, verify, restore).
        #[arg(long)]
        sector: Option<usize>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_defaults_off_and_parses_when_set() {
        let cli = Cli::try_parse_from(["norbert", "jedec"]).expect("bare subcommand parses");
        assert!(!cli.reset, "--reset must default to false");

        let cli = Cli::try_parse_from(["norbert", "--reset", "jedec"]).expect("--reset parses");
        assert!(cli.reset, "--reset must set the flag");
    }

    #[test]
    fn removed_gpio_flags_are_rejected() {
        use clap::error::ErrorKind;
        for bad in ["--cs", "--hold-gpio", "--hold-active", "--hold-release"] {
            let err = Cli::try_parse_from(["norbert", bad, "jedec"])
                .err()
                .unwrap_or_else(|| panic!("{bad} should no longer be a valid flag"));
            assert_eq!(
                err.kind(),
                ErrorKind::UnknownArgument,
                "{bad} should be rejected as an unknown argument"
            );
        }
    }
}
