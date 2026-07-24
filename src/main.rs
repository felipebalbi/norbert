mod flash;
mod sfdp;
mod device;
mod voice;
mod catalog;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use device::{HoldConfig, Level, Release};
use flash::{FlashError, Flasher, Progress};

const NORBERT_VERSION: &str =
    concat!("Norbert ", env!("CARGO_PKG_VERSION"), "\nReliable since Tuesday.");

#[derive(Parser)]
#[command(name = "norbert", version = NORBERT_VERSION, about = "A patient SPI-NOR flasher")]
struct Cli {
    /// Pick a specific Pico de Gallo by USB serial number.
    #[arg(long, global = true)]
    serial: Option<String>,
    /// SPI clock in Hz (USB-FS bound; 10 MHz is plenty).
    #[arg(long, global = true, default_value_t = 10_000_000)]
    freq: u32,
    /// GPIO wired to the flash CS (SS_B).
    #[arg(long, global = true, default_value_t = 0)]
    cs: u8,
    /// GPIO to hold another bus master off the SPI while programming. Omit for a
    /// bare chip (no hold). iCE40 CRESET example: `--hold-gpio 1 --hold-active low --hold-release hi-z`.
    #[arg(long, global = true)]
    hold_gpio: Option<u8>,
    /// Level to hold the bus GPIO at.
    #[arg(long, global = true, value_enum, default_value_t = ActiveArg::Low)]
    hold_active: ActiveArg,
    /// What to do with the bus GPIO on release.
    #[arg(long, global = true, value_enum, default_value_t = ReleaseArg::HiZ)]
    hold_release: ReleaseArg,
    /// Machine-friendly output: drop the commentary, print IDs/addresses/OK/FAIL only.
    #[arg(long, global = true)]
    quiet: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum ActiveArg { High, Low }
#[derive(Clone, Copy, ValueEnum)]
enum ReleaseArg { DriveHigh, DriveLow, HiZ }

impl Cli {
    /// Build the bus-hold config from the flags (`None` = bare chip, no hold).
    fn hold(&self) -> Option<HoldConfig> {
        self.hold_gpio.map(|pin| HoldConfig {
            pin,
            active: match self.hold_active { ActiveArg::High => Level::High, ActiveArg::Low => Level::Low },
            release: match self.hold_release {
                ReleaseArg::DriveHigh => Release::DriveHigh,
                ReleaseArg::DriveLow => Release::DriveLow,
                ReleaseArg::HiZ => Release::HiZ,
            },
        })
    }
}

#[derive(Subcommand)]
enum Cmd {
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
}

fn build_flasher(cli: &Cli) -> Result<Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>> {
    let conn = device::connect(cli.serial.as_deref(), cli.freq, cli.cs, cli.hold())?;
    // Chunk to the firmware's max transfer for best throughput.
    let max_chunk = pico_de_gallo_lib::MAX_TRANSFER_SIZE.min(flash::PAGE_SIZE);
    let Connected2 { spi, bus } = keep_alive(conn);
    Ok(Flasher::with_config(
        spi, bus, max_chunk,
        Duration::from_millis(2), Duration::from_secs(120),
    ))
}

// Helper: keep the Hal handle alive for the process lifetime.
struct Connected2 { spi: pico_de_gallo_hal::SpiDev, bus: device::HostBus }
fn keep_alive(conn: device::Connected) -> Connected2 {
    // `Hal` owns the tokio runtime; SpiDev/Gpio only hold cloned Handles, which
    // do NOT keep it alive. Leak `Hal` so the runtime outlives the handles for
    // the whole process — this leak is required, not optional.
    Box::leak(Box::new(conn._hal));
    Connected2 { spi: conn.spi, bus: conn.bus }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.cmd {
        Cmd::Jedec => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let id = f.read_id().map_err(anyhow_from)?;
            let _ = f.release_bus();
            out.emit(
                &format!("{:02X} {:02X} {:02X}", id.manufacturer, id.mem_type, id.capacity_code),
                Some(&format!("{:02X}{:02X}{:02X}", id.manufacturer, id.mem_type, id.capacity_code)),
            );
        }
        Cmd::Info => {
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let out = (|| -> Result<()> {
                let id = f.read_id().map_err(anyhow_from)?;
                if !id.is_present() {
                    println!("no SPI-NOR flash detected (bus reads all 0x00/0xFF)");
                    return Ok(());
                }
                println!("JEDEC id: {id}");
                match f.detect_profile() {
                    Ok(p) => print_profile(&p),
                    Err(FlashError::UnsupportedChip { jedec }) => println!(
                        "unsupported: JEDEC {jedec:02X?} — no SFDP and no fallback-table entry (add one to FALLBACK_TABLE)"),
                    Err(e) => return Err(anyhow_from(e)),
                }
                Ok(())
            })();
            let _ = f.release_bus();
            out?;
        }
        Cmd::Detect => {
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let res = f.detect_profile().map_err(anyhow_from);
            let _ = f.release_bus();
            print_profile(&res?);
        }
        Cmd::Program { bitstream, offset, no_verify, chip_erase, unprotect } => {
            let image = std::fs::read(bitstream)
                .with_context(|| format!("reading {}", bitstream.display()))?;
            let mut f = build_flasher(&cli)?;
            let id = f.read_id().map_err(anyhow_from)?;
            println!("flash: {id}");
            let size = id.capacity_bytes();
            let pb = spinner();
            if *chip_erase {
                f.acquire_bus().map_err(anyhow_from)?;
                let r = f.detect_profile().map_err(anyhow_from);
                if let Err(e) = r { let _ = f.release_bus(); return Err(e); }
                if *unprotect { let r = f.unprotect().map_err(anyhow_from); if let Err(e) = r { let _ = f.release_bus(); return Err(e); } }
                pb.set_message("chip erase…");
                let r = f.chip_erase().map_err(anyhow_from);
                if let Err(e) = r { let _ = f.release_bus(); return Err(e); }
                let r = f.program(*offset, &image, |_| {}).map_err(anyhow_from);
                if let Err(e) = r { let _ = f.release_bus(); return Err(e); }
                if !*no_verify { f.verify(*offset, &image, |_| {}).map_err(anyhow_from)
                    .inspect_err(|_| { let _ = f.release_bus(); })?; }
                f.release_bus().map_err(anyhow_from)?;
            } else {
                let res = f.flash_bitstream(*offset, &image, true, *unprotect, !*no_verify, size, |p| {
                    pb.set_message(match p {
                        Progress::Detecting => "detecting flash (SFDP)…".to_string(),
                        Progress::Erasing => "erasing…".to_string(),
                        Progress::Programming(n) => format!("programming {n}/{}", image.len()),
                        Progress::Verifying(n) => format!("verifying {n}/{}", image.len()),
                        Progress::Done => "done".to_string(),
                    });
                });
                if res.is_err() { let _ = f.release_bus(); } // never leave FPGA in reset
                res.map_err(anyhow_from)?;
            }
            pb.finish_with_message("flashed ✓");
        }
        Cmd::Read { out, length, offset } => {
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let mut buf = vec![0u8; *length];
            let r = f.read(*offset, &mut buf).map_err(anyhow_from);
            let _ = f.release_bus();
            r?;
            std::fs::write(out, &buf).with_context(|| format!("writing {}", out.display()))?;
            println!("read {length} bytes → {}", out.display());
        }
        Cmd::Verify { bitstream, offset } => {
            let image = std::fs::read(bitstream)?;
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let r = f.verify(*offset, &image, |_| {}).map_err(anyhow_from);
            let _ = f.release_bus();
            r?;
            println!("verify ✓ ({} bytes)", image.len());
        }
        Cmd::Erase { offset, length, chip } => {
            // Validate the erase target BEFORE acquiring the bus, so a missing
            // argument can't leave a held master (e.g. the FPGA) stuck in reset.
            let len = if *chip {
                None
            } else {
                Some(length.context("erase needs --length N or --chip")?)
            };
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let r = match len {
                None => f.chip_erase().map_err(anyhow_from),
                Some(len) => f.erase_range(*offset, len).map_err(anyhow_from),
            };
            let _ = f.release_bus();
            r?;
            println!("erase ✓");
        }
        Cmd::List => {
            for c in sfdp::FALLBACK_TABLE {
                println!("{:02X} {:02X} {:02X}  {}", c.jedec[0], c.jedec[1], c.jedec[2],
                    catalog::describe(c.jedec));
            }
            println!("(any chip with valid SFDP is supported automatically.)");
        }
        Cmd::Sfdp => {
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let out = (|| -> Result<()> {
                let mut header = [0u8; 8];
                f.read_sfdp(0, &mut header).map_err(anyhow_from)?;
                if sfdp::SfdpHeader::parse(&header).is_none() {
                    println!("no SFDP (signature absent)");
                    return Ok(());
                }
                // Dump the first 256 bytes of SFDP space as hex.
                let mut blob = vec![0u8; 256];
                f.read_sfdp(0, &mut blob).map_err(anyhow_from)?;
                println!("SFDP (first 256 bytes):");
                for (i, chunk) in blob.chunks(16).enumerate() {
                    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
                    println!("  {:04X}: {}", i * 16, hex.join(" "));
                }
                Ok(())
            })();
            let _ = f.release_bus();
            out?;
        }
        Cmd::Protect => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let r = f.protect();
            let _ = f.release_bus();
            r.map_err(anyhow_from)?;
            out.emit(voice::protect_done(), Some("OK"));
        }
        Cmd::Unprotect => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let r = f.unprotect();
            let _ = f.release_bus();
            r.map_err(anyhow_from)?;
            out.emit(voice::unprotect_done(), Some("OK"));
        }
        Cmd::Reset => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().map_err(anyhow_from)?;
            let r = f.reset_flash();
            let _ = f.release_bus(); // also reboots a held master
            r.map_err(anyhow_from)?;
            out.emit(voice::reset_done(), Some("OK"));
        }
    }
    Ok(())
}

fn spinner() -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(ProgressStyle::with_template("{spinner} {msg}").unwrap());
    pb
}

fn anyhow_from<S: std::fmt::Debug, R: std::fmt::Debug>(e: FlashError<S, R>) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

struct Out { quiet: bool }
impl Out {
    fn new(cli: &Cli) -> Self { Out { quiet: cli.quiet || !std::io::stdout().is_terminal() } }
    /// Personality line for humans; optional machine line for `--quiet`/non-TTY.
    fn emit(&self, human: &str, machine: Option<&str>) {
        if self.quiet { if let Some(m) = machine { println!("{m}"); } }
        else { println!("{human}"); }
    }
}

/// Render a FlashError as Norbert's voice where personality applies, else technical.
#[allow(dead_code)] // used directly in Task 23b
fn norbert_error<S: std::fmt::Debug, R: std::fmt::Debug>(e: &FlashError<S, R>) -> String {
    match e {
        FlashError::NoFlash => voice::no_flash().to_string(),
        FlashError::UnsupportedChip { jedec } => voice::unsupported(*jedec),
        FlashError::VerifyMismatch { addr, .. } => voice::verify_fail(*addr),
        other => other.to_string(),
    }
}

/// Convert a FlashError into an anyhow error carrying Norbert's line (for user-facing failures).
#[allow(dead_code)] // used directly in Task 23b
fn norbert_from<S: std::fmt::Debug, R: std::fmt::Debug>(e: FlashError<S, R>) -> anyhow::Error {
    anyhow::anyhow!("{}", norbert_error(&e))
}

fn print_profile(p: &sfdp::FlashProfile) {
    println!("source:   {:?}", p.source);
    println!("page:     {} B", p.page_size);
    println!("address:  {}-byte", p.address_bytes);
    match p.capacity {
        Some(c) => println!("capacity: {} KiB", c / 1024),
        None => println!("capacity: unknown"),
    }
    println!("erase types:");
    for e in &p.erase_types {
        println!("  {:>7} B  op 0x{:02X}", e.size, e.opcode);
    }
}
