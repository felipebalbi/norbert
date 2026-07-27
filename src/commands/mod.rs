//! Command handlers + the bus-session helpers that guarantee bus release.
//!
//! Release is done by an explicit `release_bus()` after the op completes or is
//! cancelled — deliberately NOT via an RAII `Drop` guard. `release_bus()` drives
//! a blocking-GPIO write through the HAL's `block_in_place` bridge, and running
//! that during an async task-drop (unwind/cancellation) is unsafe (see the
//! async-conversion design). The invariant that keeps this sound: `with_cancel`
//! must remain the TOP-LEVEL canceller — never compose these helpers under an
//! outer `select!`/`timeout` that could drop their future mid-op and skip the
//! release line.

// Submodules are declared by their creating tasks (5.3 inspect, 5.4 write,
// 5.5 maintain + diagnose). The module-level allow above cascades to them.
pub mod diagnose;
pub mod inspect;
pub mod maintain;
pub mod write;

use std::time::Duration;

use embedded_hal_async::spi::SpiDevice;

use crate::cli::Cli;
use crate::device::{self, HostBus};
use crate::error::NorbertError;
use crate::flash::{self, BusAccess, Flasher};

/// The concrete flasher the CLI drives.
pub type HostFlasher = Flasher<pico_de_gallo_hal::SpiDev, HostBus>;

/// Build a flasher at the CLI's `--freq`.
pub fn build_flasher(cli: &Cli) -> anyhow::Result<HostFlasher> {
    build_flasher_at(cli, cli.freq)
}

/// Build a flasher at a caller-chosen SPI frequency (doctor steps the clock).
pub fn build_flasher_at(cli: &Cli, freq: u32) -> anyhow::Result<HostFlasher> {
    let device::Connected { spi, bus } =
        device::connect(cli.serial.as_deref(), freq, cli.cs, cli.hold())?;
    let max_chunk = pico_de_gallo_lib::MAX_TRANSFER_SIZE.min(flash::PAGE_SIZE);
    Ok(Flasher::with_config(
        spi,
        bus,
        max_chunk,
        Duration::from_millis(2),
        Duration::from_secs(120),
    ))
}

/// Acquire the bus, run `work`, and ALWAYS release. For short read-only ops.
pub async fn with_bus<SPI, RST, T>(
    f: &mut Flasher<SPI, RST>,
    work: impl AsyncFnOnce(&mut Flasher<SPI, RST>) -> Result<T, NorbertError>,
) -> Result<T, NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    f.acquire_bus().await?;
    let r = work(f).await;
    let _ = f.release_bus();
    r
}

/// Acquire, run `work` racing Ctrl-C, and ALWAYS release. On Ctrl-C, drop the
/// in-flight op at its next await and return `Cancelled` (exit 130). For long ops.
pub async fn with_cancel<SPI, RST, T>(
    f: &mut Flasher<SPI, RST>,
    work: impl AsyncFnOnce(&mut Flasher<SPI, RST>) -> Result<T, NorbertError>,
) -> Result<T, NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    f.acquire_bus().await?;
    let outcome = tokio::select! {
        r = work(f) => Some(r),
        _ = tokio::signal::ctrl_c() => None,
    };
    let _ = f.release_bus();
    match outcome {
        Some(r) => r,
        None => Err(NorbertError::Cancelled),
    }
}

use crate::cli::Cmd;
use crate::ui::Ui;

/// Route a parsed command to its handler.
pub async fn dispatch(cli: &Cli, ui: &mut Ui) -> Result<(), NorbertError> {
    let Some(cmd) = &cli.cmd else {
        return Ok(());
    };
    match cmd {
        Cmd::Jedec => {
            let mut f = build_flasher(cli)?;
            inspect::jedec(&mut f, ui).await
        }
        Cmd::Info => {
            let mut f = build_flasher(cli)?;
            inspect::info(&mut f, ui).await
        }
        Cmd::Detect => {
            let mut f = build_flasher(cli)?;
            inspect::detect(&mut f, ui).await
        }
        Cmd::Sfdp => {
            let mut f = build_flasher(cli)?;
            inspect::sfdp(&mut f, ui).await
        }
        Cmd::List => {
            inspect::list(ui);
            Ok(())
        }
        Cmd::Program {
            bitstream,
            offset,
            no_verify,
            chip_erase,
            unprotect,
        } => {
            let mut f = build_flasher(cli)?;
            write::program(
                &mut f,
                ui,
                bitstream,
                *offset,
                *no_verify,
                *chip_erase,
                *unprotect,
            )
            .await
        }
        Cmd::Erase {
            offset,
            length,
            chip,
        } => {
            let mut f = build_flasher(cli)?;
            write::erase(&mut f, ui, *offset, *length, *chip).await
        }
        Cmd::Read {
            out,
            length,
            offset,
        } => {
            let mut f = build_flasher(cli)?;
            write::read(&mut f, ui, out, *length, *offset).await
        }
        Cmd::Verify { bitstream, offset } => {
            let mut f = build_flasher(cli)?;
            write::verify(&mut f, ui, bitstream, *offset).await
        }
        Cmd::Protect => {
            let mut f = build_flasher(cli)?;
            maintain::protect(&mut f, ui).await
        }
        Cmd::Unprotect => {
            let mut f = build_flasher(cli)?;
            maintain::unprotect(&mut f, ui).await
        }
        Cmd::Reset => {
            let mut f = build_flasher(cli)?;
            maintain::reset(&mut f, ui).await
        }
        Cmd::Doctor => diagnose::doctor(cli, ui).await,
        Cmd::Test { sector } => {
            let mut f = build_flasher(cli)?;
            diagnose::test(&mut f, ui, *sector).await
        }
    }
}
