//! Command-line surface: global flags + subcommands. Parsing only.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::device::{HoldConfig, Level, Release};

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
    /// User GPIO (0-3) wired to the flash CS (SS_B). Default: User GPIO 0 (header pin 11).
    #[arg(long, global = true, default_value_t = 0)]
    pub cs: u8,
    /// User GPIO (0-3) that holds another bus master (e.g. an FPGA's CRESET) off the
    /// shared SPI while we work. Default: User GPIO 1 (header pin 12).
    /// iCE40 example: `--hold-gpio 1 --hold-active low --hold-release hi-z`.
    #[arg(long, global = true, default_value_t = 1)]
    pub hold_gpio: u8,
    /// Level to hold the bus GPIO at.
    #[arg(long, global = true, value_enum, default_value_t = Level::Low)]
    pub hold_active: Level,
    /// What to do with the bus GPIO on release.
    #[arg(long, global = true, value_enum, default_value_t = Release::HiZ)]
    pub hold_release: Release,
    /// Machine-friendly output: drop the commentary, print IDs/addresses/OK/FAIL only.
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Print version.
    #[arg(short = 'V', long = "version", global = true)]
    pub version: bool,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

impl Cli {
    /// Build the bus-hold config from the flags (hold GPIO defaults to User GPIO 1).
    pub fn hold(&self) -> HoldConfig {
        HoldConfig {
            pin: self.hold_gpio,
            active: self.hold_active,
            release: self.hold_release,
        }
    }
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
