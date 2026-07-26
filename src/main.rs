mod catalog;
mod device;
mod flash;
mod sfdp;
mod voice;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use device::{HoldConfig, Level, Release};
use flash::{FlashError, Flasher};

#[derive(Parser)]
#[command(
    name = "norbert",
    about = "A patient SPI-NOR flasher",
    disable_version_flag = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Pick a specific Pico de Gallo by USB serial number.
    #[arg(long, global = true)]
    serial: Option<String>,
    /// SPI clock in Hz (USB-FS bound; 10 MHz is plenty).
    #[arg(long, global = true, default_value_t = 10_000_000)]
    freq: u32,
    /// User GPIO (0-3) wired to the flash CS (SS_B). Default: User GPIO 0 (header pin 11).
    #[arg(long, global = true, default_value_t = 0)]
    cs: u8,
    /// User GPIO (0-3) that holds another bus master (e.g. an FPGA's CRESET) off the
    /// shared SPI while we work. Default: User GPIO 1 (header pin 12).
    /// iCE40 example: `--hold-gpio 1 --hold-active low --hold-release hi-z`.
    #[arg(long, global = true, default_value_t = 1)]
    hold_gpio: u8,
    /// Level to hold the bus GPIO at.
    #[arg(long, global = true, value_enum, default_value_t = ActiveArg::Low)]
    hold_active: ActiveArg,
    /// What to do with the bus GPIO on release.
    #[arg(long, global = true, value_enum, default_value_t = ReleaseArg::HiZ)]
    hold_release: ReleaseArg,
    /// Machine-friendly output: drop the commentary, print IDs/addresses/OK/FAIL only.
    #[arg(long, global = true)]
    quiet: bool,
    /// Print version.
    #[arg(short = 'V', long = "version", global = true)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Clone, Copy, ValueEnum)]
enum ActiveArg {
    High,
    Low,
}
#[derive(Clone, Copy, ValueEnum)]
enum ReleaseArg {
    DriveHigh,
    DriveLow,
    HiZ,
}

impl Cli {
    /// Build the bus-hold config from the flags (hold GPIO defaults to User GPIO 1).
    fn hold(&self) -> Option<HoldConfig> {
        Some(HoldConfig {
            pin: self.hold_gpio,
            active: match self.hold_active {
                ActiveArg::High => Level::High,
                ActiveArg::Low => Level::Low,
            },
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
    /// Wiring/power/speed check-up (read-only).
    Doctor,
    /// Read-back consistency check; with --sector N, a destructive sector self-test.
    Test {
        /// Destructively test sector N (backup, erase, pattern, verify, restore).
        #[arg(long)]
        sector: Option<usize>,
    },
}

fn build_flasher(cli: &Cli) -> Result<Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>> {
    build_flasher_at(cli, cli.freq)
}

/// Like `build_flasher`, but at a caller-chosen SPI frequency (doctor steps freq).
fn build_flasher_at(
    cli: &Cli,
    freq: u32,
) -> Result<Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>> {
    let device::Connected { _hal, spi, bus } =
        device::connect(cli.serial.as_deref(), freq, cli.cs, cli.hold())?;
    // Chunk to the firmware's max transfer for best throughput.
    let max_chunk = pico_de_gallo_lib::MAX_TRANSFER_SIZE.min(flash::PAGE_SIZE);
    // `_hal` owns no runtime (we run inside norbert's #[tokio::main]); `spi`/`bus`
    // hold Arc<Mutex<PicoDeGallo>> clones that keep the client alive, so dropping
    // `_hal` here is safe — no Box::leak needed.
    drop(_hal);
    Ok(Flasher::with_config(
        spi,
        bus,
        max_chunk,
        Duration::from_millis(2),
        Duration::from_secs(120),
    ))
}

/// A determinate byte bar (or a hidden bar in --quiet / non-TTY).
fn byte_bar(len: u64, quiet: bool) -> indicatif::ProgressBar {
    if quiet {
        return indicatif::ProgressBar::hidden();
    }
    let pb = indicatif::ProgressBar::new(len);
    pb.set_style(
        indicatif::ProgressStyle::with_template(
            "{msg:>9} [{bar:32}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
        )
        .unwrap()
        .progress_chars("=> "),
    );
    pb
}

// multi_thread is REQUIRED: the HAL bridges its blocking GPIO/SPI-config calls
// via tokio::task::block_in_place, which panics on a current-thread runtime.
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

/// Acquire the bus, run `work` racing Ctrl-C, and ALWAYS release the bus.
/// On Ctrl-C: drop the in-flight op (cooperative cancel at the next await),
/// release the bus, print the cancellation line, and exit 130.
async fn with_cancel(
    f: &mut Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>,
    out: &Out,
    work: impl AsyncFnOnce(&mut Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>) -> Result<()>,
) -> Result<()> {
    f.acquire_bus().await.map_err(anyhow_from)?;
    let outcome = tokio::select! {
        r = work(f) => Some(r),
        _ = tokio::signal::ctrl_c() => None,
    };
    let _ = f.release_bus();
    match outcome {
        Some(r) => r,
        None => {
            out.emit(voice::cancelled(), Some("FAIL: cancelled"));
            std::process::exit(130);
        }
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    if cli.version {
        println!("{}", voice::version());
        return Ok(());
    }
    let Some(cmd) = cli.cmd.as_ref() else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    match cmd {
        Cmd::Jedec => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            let r = f.read_id().await;
            let _ = f.release_bus();
            let id = r.map_err(anyhow_from)?;
            out.emit(
                &format!(
                    "{:02X} {:02X} {:02X}",
                    id.manufacturer, id.mem_type, id.capacity_code
                ),
                Some(&format!(
                    "{:02X}{:02X}{:02X}",
                    id.manufacturer, id.mem_type, id.capacity_code
                )),
            );
        }
        Cmd::Info => {
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            let out = async {
                let id = f.read_id().await.map_err(anyhow_from)?;
                if !id.is_present() {
                    println!("no SPI-NOR flash detected (bus reads all 0x00/0xFF)");
                    return Ok(());
                }
                println!("JEDEC id: {id}");
                println!("chip:     {}", catalog::describe(id.jedec()));
                match f.detect_profile().await {
                    Ok(p) => {
                        print_profile(&p);
                        println!("SFDP:     {}",
                            if p.source == sfdp::ProfileSource::Sfdp { "present" } else { "—" });
                    }
                    Err(FlashError::UnsupportedChip { jedec }) => println!(
                        "unsupported: JEDEC {jedec:02X?} — no SFDP and no fallback-table entry (add one to FALLBACK_TABLE)"),
                    Err(e) => return Err(anyhow_from(e)),
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            let _ = f.release_bus();
            out?;
        }
        Cmd::Detect => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            out.emit(voice::detect_opener(), None);
            let res = async {
                let id = f.read_id().await.map_err(norbert_from)?;
                f.detect_profile().await.map_err(norbert_from)?;
                Ok::<[u8; 3], anyhow::Error>(id.jedec())
            }
            .await;
            let _ = f.release_bus();
            let jedec = res?;
            let name = catalog::describe(jedec);
            out.emit(
                &voice::found(&name),
                Some(&format!(
                    "{:02X} {:02X} {:02X}",
                    jedec[0], jedec[1], jedec[2]
                )),
            );
        }
        Cmd::Program {
            bitstream,
            offset,
            no_verify,
            chip_erase,
            unprotect,
        } => {
            let out = Out::new(&cli);
            let image = std::fs::read(bitstream)
                .with_context(|| format!("reading {}", bitstream.display()))?;
            let mut f = build_flasher(&cli)?;
            with_cancel(&mut f, &out, async |f| {
                f.detect_profile().await.map_err(norbert_from)?;
                // Protection pre-check: speak, don't silently no-op.
                match f.is_protected().await {
                    Ok(true) if !*unprotect => {
                        let _ = f.release_bus(); // exit() bypasses with_cancel's release
                        out.emit(voice::protected(), Some("FAIL: protected"));
                        std::process::exit(1);
                    }
                    Ok(_) => {}
                    Err(e) => return Err(anyhow_from(e)),
                }
                // Oversize guard from the detected capacity.
                if let Some(cap) = f.profile().and_then(|p| p.capacity) {
                    if *offset + image.len() > cap {
                        return Err(anyhow::anyhow!(
                            "image needs {} bytes but flash is {cap} bytes",
                            *offset + image.len()
                        ));
                    }
                }
                if *unprotect {
                    f.unprotect().await.map_err(anyhow_from)?;
                }
                out.emit(voice::programming(), None);

                let erase = if cli.quiet {
                    indicatif::ProgressBar::hidden()
                } else {
                    indicatif::ProgressBar::new_spinner()
                };
                erase.set_message("erasing");
                erase.enable_steady_tick(std::time::Duration::from_millis(100));
                if *chip_erase {
                    f.chip_erase().await.map_err(anyhow_from)?;
                } else {
                    f.erase_range(*offset, image.len())
                        .await
                        .map_err(anyhow_from)?;
                }
                erase.finish_and_clear();

                let pb = byte_bar(image.len() as u64, cli.quiet);
                pb.set_message("program");
                f.program(*offset, &image, |w| pb.set_position(w as u64))
                    .await
                    .map_err(anyhow_from)?;
                pb.finish_and_clear();

                if !*no_verify {
                    let vb = byte_bar(image.len() as u64, cli.quiet);
                    vb.set_message("verify");
                    f.verify(*offset, &image, |d| vb.set_position(d as u64))
                        .await
                        .map_err(norbert_from)?;
                    vb.finish_and_clear();
                }
                Ok(())
            })
            .await?;
            out.emit(voice::programmed(), Some("OK"));
        }
        Cmd::Read {
            out: outfile,
            length,
            offset,
        } => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            let mut buf = vec![0u8; *length];
            with_cancel(&mut f, &out, async |f| {
                f.detect_profile().await.map_err(norbert_from)?;
                f.read(*offset, &mut buf).await.map_err(anyhow_from)?;
                Ok(())
            })
            .await?;
            std::fs::write(outfile, &buf)
                .with_context(|| format!("writing {}", outfile.display()))?;
            out.emit(
                &format!("Done. {} bytes to {}", length, outfile.display()),
                Some(&format!("{length}")),
            );
        }
        Cmd::Verify { bitstream, offset } => {
            let out = Out::new(&cli);
            let image = std::fs::read(bitstream)
                .with_context(|| format!("reading {}", bitstream.display()))?;
            let mut f = build_flasher(&cli)?;
            with_cancel(&mut f, &out, async |f| {
                f.detect_profile().await.map_err(norbert_from)?;
                f.verify(*offset, &image, |_| {})
                    .await
                    .map_err(norbert_from)?;
                Ok(())
            })
            .await?;
            out.emit(voice::verify_ok(), Some("OK"));
        }
        Cmd::Erase {
            offset,
            length,
            chip,
        } => {
            let out = Out::new(&cli);
            // Validate the erase target BEFORE acquiring the bus, so a missing
            // argument can't leave a held master (e.g. the FPGA) stuck in reset.
            let len = if *chip {
                None
            } else {
                Some(length.context("erase needs --length N or --chip")?)
            };
            let mut f = build_flasher(&cli)?;
            with_cancel(&mut f, &out, async |f| {
                f.detect_profile().await.map_err(norbert_from)?;
                match len {
                    None => f.chip_erase().await.map_err(anyhow_from)?,
                    Some(len) => f.erase_range(*offset, len).await.map_err(anyhow_from)?,
                }
                Ok(())
            })
            .await?;
            out.emit(voice::erased(), Some("OK"));
        }
        Cmd::List => {
            for c in sfdp::FALLBACK_TABLE {
                println!(
                    "{:02X} {:02X} {:02X}  {}",
                    c.jedec[0],
                    c.jedec[1],
                    c.jedec[2],
                    catalog::describe(c.jedec)
                );
            }
            println!("(any chip with valid SFDP is supported automatically.)");
        }
        Cmd::Sfdp => {
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            let out = async {
                let mut header = [0u8; 8];
                f.read_sfdp(0, &mut header).await.map_err(anyhow_from)?;
                if sfdp::SfdpHeader::parse(&header).is_none() {
                    println!("no SFDP (signature absent)");
                    return Ok(());
                }
                // Dump the first 256 bytes of SFDP space as hex.
                let mut blob = vec![0u8; 256];
                f.read_sfdp(0, &mut blob).await.map_err(anyhow_from)?;
                println!("SFDP (first 256 bytes):");
                for (i, chunk) in blob.chunks(16).enumerate() {
                    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
                    println!("  {:04X}: {}", i * 16, hex.join(" "));
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            let _ = f.release_bus();
            out?;
        }
        Cmd::Protect => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            let r = f.protect().await;
            let _ = f.release_bus();
            r.map_err(anyhow_from)?;
            out.emit(voice::protect_done(), Some("OK"));
        }
        Cmd::Unprotect => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            let r = f.unprotect().await;
            let _ = f.release_bus();
            r.map_err(anyhow_from)?;
            out.emit(voice::unprotect_done(), Some("OK"));
        }
        Cmd::Reset => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            let r = f.reset_flash().await;
            let _ = f.release_bus(); // also reboots a held master
            r.map_err(anyhow_from)?;
            out.emit(voice::reset_done(), Some("OK"));
        }
        Cmd::Doctor => {
            let out = Out::new(&cli);
            // 1. RDID at --freq, bus held.
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            let id_res = f.read_id().await;
            // 3. SFDP readable? (still bus-held)
            let sfdp_res = {
                let mut hdr = [0u8; 8];
                f.read_sfdp(0, &mut hdr)
                    .await
                    .map(|_| sfdp::SfdpHeader::parse(&hdr).is_some())
            };
            let _ = f.release_bus();

            let id = match id_res {
                Ok(id) => id,
                Err(e) => {
                    // Connection worked but RDID failed → report and stop.
                    println!("RDID: FAILED ({})", anyhow_from(e));
                    out.emit(voice::doctor_rdid_fail(), Some("FAIL: rdid"));
                    return Ok(());
                }
            };
            println!(
                "RDID @ {} Hz: {:02X} {:02X} {:02X}",
                cli.freq, id.manufacturer, id.mem_type, id.capacity_code
            );
            // 1b. Not present?
            if !id.is_present() {
                out.emit(voice::no_flash(), Some("FAIL: no chip"));
                println!("  Check CS (--cs), MISO, GND, power, and that any other bus master is held off (--hold-gpio).");
                return Ok(());
            }
            println!("chip: {}", catalog::describe(id.jedec()));
            // 2. All three ID bytes equal → MISO/power suspicion.
            let mut warned = false;
            if id.manufacturer == id.mem_type && id.mem_type == id.capacity_code {
                println!("WARNING: all three JEDEC bytes are 0x{:02X} — MISO may be stuck or power/wiring is wrong.",
                    id.manufacturer);
                warned = true;
            }
            // 3. SFDP.
            match sfdp_res {
                Ok(true) => println!("SFDP: present"),
                Ok(false) => println!("SFDP: absent (will use the fallback table)"),
                Err(e) => println!("SFDP: read failed ({})", anyhow_from(e)),
            }
            // 4. Step SPI frequency and confirm RDID is identical each time.
            let mut stable = true;
            for freq in [1_000_000u32, 5_000_000, 10_000_000] {
                match build_flasher_at(&cli, freq) {
                    Ok(mut ff) => {
                        if ff.acquire_bus().await.is_err() {
                            println!("  {freq} Hz: bus acquire failed");
                            stable = false;
                            continue;
                        }
                        let r = ff.read_id().await;
                        let _ = ff.release_bus();
                        match r {
                            Ok(fid) if fid.jedec() == id.jedec() => println!(
                                "  {freq} Hz: {:02X} {:02X} {:02X} OK",
                                fid.manufacturer, fid.mem_type, fid.capacity_code
                            ),
                            Ok(fid) => {
                                println!(
                                    "  {freq} Hz: {:02X} {:02X} {:02X} MISMATCH",
                                    fid.manufacturer, fid.mem_type, fid.capacity_code
                                );
                                stable = false;
                            }
                            Err(e) => {
                                println!("  {freq} Hz: read failed ({})", anyhow_from(e));
                                stable = false;
                            }
                        }
                    }
                    Err(e) => {
                        println!("  {freq} Hz: connect failed ({e:#})");
                        stable = false;
                    }
                }
            }
            // 5. Summary.
            if stable && !warned {
                out.emit(voice::nothing_unusual(), Some("OK"));
            } else {
                out.emit(voice::doctor_unstable(), Some("WARN"));
            }
        }
        Cmd::Test { sector } => {
            let out = Out::new(&cli);
            let mut f = build_flasher(&cli)?;
            f.acquire_bus().await.map_err(anyhow_from)?;
            if let Err(e) = f.detect_profile().await {
                let _ = f.release_bus();
                return Err(norbert_from(e));
            }
            match sector {
                None => {
                    // Read-only bus-stability check: read the first 4 KiB twice, compare.
                    let res = async {
                        let n = 4096;
                        let mut a = vec![0u8; n];
                        let mut b = vec![0u8; n];
                        f.read(0, &mut a).await.map_err(anyhow_from)?;
                        f.read(0, &mut b).await.map_err(anyhow_from)?;
                        Ok::<bool, anyhow::Error>(a == b)
                    }
                    .await;
                    let _ = f.release_bus();
                    if !res? {
                        return Err(anyhow::anyhow!(
                            "read-back inconsistent between two reads — signal integrity suspect"
                        ));
                    }
                    out.emit(voice::nothing_unusual(), Some("OK"));
                }
                Some(n) => {
                    // Refuse a protected part before touching anything.
                    match f.is_protected().await {
                        Ok(true) => {
                            let _ = f.release_bus();
                            out.emit(voice::protected(), Some("FAIL: protected"));
                            std::process::exit(1);
                        }
                        Ok(false) => {}
                        Err(e) => {
                            let _ = f.release_bus();
                            return Err(anyhow_from(e));
                        }
                    }
                    let sec = f.profile().map(|p| p.min_erase()).unwrap_or(4096);
                    let cap = f.profile().and_then(|p| p.capacity);
                    let base = n.saturating_mul(sec);
                    if let Some(cap) = cap {
                        if base.saturating_add(sec) > cap {
                            let _ = f.release_bus();
                            return Err(anyhow::anyhow!(
                                "sector {n} is out of range (chip holds {} sectors of {} bytes)",
                                cap / sec,
                                sec
                            ));
                        }
                    }
                    // Back up the sector before destroying it.
                    let mut backup = vec![0u8; sec];
                    if let Err(e) = f.read(base, &mut backup).await {
                        let _ = f.release_bus();
                        return Err(anyhow_from(e));
                    }
                    // Destructive pattern test.
                    let pattern: Vec<u8> = (0..sec).map(|i| (i as u8) ^ 0xA5).collect();
                    let test_res = async {
                        f.erase_range(base, sec).await.map_err(anyhow_from)?;
                        f.program(base, &pattern, |_| {})
                            .await
                            .map_err(anyhow_from)?;
                        f.verify(base, &pattern, |_| {})
                            .await
                            .map_err(norbert_from)?;
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    // ALWAYS attempt to restore the original, even if the test failed.
                    let restore_res = async {
                        f.erase_range(base, sec).await.map_err(anyhow_from)?;
                        f.program(base, &backup, |_| {})
                            .await
                            .map_err(anyhow_from)?;
                        f.verify(base, &backup, |_| {}).await.map_err(anyhow_from)?;
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    let _ = f.release_bus();
                    test_res?;
                    restore_res
                        .context("sector test passed but restoring the original contents failed")?;
                    out.emit(voice::nothing_unusual(), Some("OK"));
                }
            }
        }
    }
    Ok(())
}

fn anyhow_from<S: std::fmt::Debug, R: std::fmt::Debug>(e: FlashError<S, R>) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

struct Out {
    quiet: bool,
}
impl Out {
    fn new(cli: &Cli) -> Self {
        Out {
            quiet: cli.quiet || !std::io::stdout().is_terminal(),
        }
    }
    /// Personality line for humans; optional machine line for `--quiet`/non-TTY.
    fn emit(&self, human: &str, machine: Option<&str>) {
        if self.quiet {
            if let Some(m) = machine {
                println!("{m}");
            }
        } else {
            println!("{human}");
        }
    }
}

/// Render a FlashError as Norbert's voice where personality applies, else technical.
fn norbert_error<S: std::fmt::Debug, R: std::fmt::Debug>(e: &FlashError<S, R>) -> String {
    match e {
        FlashError::NoFlash => voice::no_flash().to_string(),
        FlashError::UnsupportedChip { jedec } => voice::unsupported(*jedec),
        FlashError::VerifyMismatch { addr, .. } => voice::verify_fail(*addr),
        other => other.to_string(),
    }
}

/// Convert a FlashError into an anyhow error carrying Norbert's line (for user-facing failures).
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
