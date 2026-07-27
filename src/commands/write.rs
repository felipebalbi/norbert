//! Mutating commands. `program`/`verify` drive the restrained progress view;
//! all four run under `with_cancel` so the bus is always released.

use std::path::Path;

use embedded_hal_async::spi::SpiDevice;

use super::with_cancel;
use crate::error::NorbertError;
use crate::flash::{BusAccess, Flasher};
use crate::ui::Ui;
use crate::ui::progress::ProgressPlan;
use crate::voice;

/// Human-friendly byte size (KiB when it divides evenly, else bytes).
fn human_bytes(n: usize) -> String {
    if n >= 1024 && n.is_multiple_of(1024) {
        format!("{} KiB", n / 1024)
    } else {
        format!("{n} B")
    }
}

/// Erase + program (+ verify) an image, showing the three-phase view.
#[allow(clippy::too_many_arguments)] // flat CLI args; a struct would add no clarity
pub async fn program<SPI, RST>(
    f: &mut Flasher<SPI, RST>,
    ui: &mut Ui,
    bitstream: &Path,
    offset: usize,
    no_verify: bool,
    chip_erase: bool,
    unprotect: bool,
) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    let image = std::fs::read(bitstream)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", bitstream.display()))?;
    let name = bitstream
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string();
    let start = std::time::Instant::now();

    let blocks = with_cancel(f, async |f| {
        f.detect_profile().await?;
        if f.is_protected().await? && !unprotect {
            return Err(NorbertError::Protected);
        }
        if let Some(cap) = f.profile().and_then(|p| p.capacity)
            && offset + image.len() > cap
        {
            return Err(anyhow::anyhow!(
                "image needs {} bytes but flash is {cap} bytes",
                offset + image.len()
            )
            .into());
        }
        if unprotect {
            f.unprotect().await?;
        }

        let plan = if chip_erase {
            None
        } else {
            Some(f.erase_plan(offset, image.len())?)
        };
        let blocks = plan.as_ref().map(|p| p.blocks()).unwrap_or(1);

        ui.say(&voice::programming_intro(
            &name,
            &human_bytes(image.len()),
            offset,
        ));
        let prog = ui.progress(ProgressPlan {
            erase_blocks: Some(blocks as u64),
            program_bytes: Some(image.len() as u64),
            verify_bytes: if no_verify {
                None
            } else {
                Some(image.len() as u64)
            },
        });

        match &plan {
            Some(plan) => f.run_erase(plan, |b| prog.erase_to(b)).await?,
            None => {
                f.chip_erase().await?;
                prog.erase_to(1);
            }
        }
        f.program(offset, &image, |w| prog.program_to(w)).await?;
        if !no_verify {
            f.verify(offset, &image, |d| prog.verify_to(d)).await?;
        }
        prog.finish();
        Ok(blocks)
    })
    .await?;

    let secs = start.elapsed().as_secs_f64();
    ui.line(
        &voice::program_summary(blocks, &human_bytes(image.len()), secs),
        "OK",
    );
    Ok(())
}

/// Erase covered blocks for a size, or the whole chip.
pub async fn erase<SPI, RST>(
    f: &mut Flasher<SPI, RST>,
    ui: &mut Ui,
    offset: usize,
    length: Option<usize>,
    chip: bool,
) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    // Validate BEFORE acquiring the bus, so a missing argument can't leave a held
    // master (e.g. an FPGA) stuck in reset.
    let len = if chip {
        None
    } else {
        Some(length.ok_or_else(|| anyhow::anyhow!("erase needs --length N or --chip"))?)
    };
    with_cancel(f, async |f| {
        f.detect_profile().await?;
        match len {
            None => f.chip_erase().await?,
            Some(len) => f.erase_range(offset, len).await?,
        }
        Ok(())
    })
    .await?;
    ui.line(voice::erased(), "OK");
    Ok(())
}

/// Dump `length` bytes from `offset` to a file.
pub async fn read<SPI, RST>(
    f: &mut Flasher<SPI, RST>,
    ui: &mut Ui,
    out: &Path,
    length: usize,
    offset: usize,
) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    let mut buf = vec![0u8; length];
    with_cancel(f, async |f| {
        f.detect_profile().await?;
        f.read(offset, &mut buf).await?;
        Ok(())
    })
    .await?;
    std::fs::write(out, &buf).map_err(|e| anyhow::anyhow!("writing {}: {e}", out.display()))?;
    ui.line(&voice::read_done(length, out), &format!("{length}"));
    Ok(())
}

/// Compare flash contents against a file (single verify bar).
pub async fn verify<SPI, RST>(
    f: &mut Flasher<SPI, RST>,
    ui: &mut Ui,
    bitstream: &Path,
    offset: usize,
) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    let image = std::fs::read(bitstream)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", bitstream.display()))?;
    let len = image.len();
    with_cancel(f, async |f| {
        f.detect_profile().await?;
        let prog = ui.progress(ProgressPlan {
            erase_blocks: None,
            program_bytes: None,
            verify_bytes: Some(len as u64),
        });
        f.verify(offset, &image, |d| prog.verify_to(d)).await?;
        prog.finish();
        Ok(())
    })
    .await?;
    ui.line(voice::verify_ok(), "OK");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::testfakes::{FakeBus, FakeFlash, flasher};
    use crate::ui::{Mode, Ui};

    // A unique temp path per (test, process) so parallel runs don't collide.
    fn temp_image(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("norbert-{tag}-{}.bin", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[tokio::test]
    async fn program_refuses_a_protected_chip_and_writes_nothing() {
        let path = temp_image("prot", &[0xABu8; 512]);
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]); // M25P16 (fallback)
        flash.set_protected(true);
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        let (mut ui, _o, _e) = Ui::captured(Mode::Machine);

        let r = program(&mut f, &mut ui, &path, 0, true, false, false).await;
        std::fs::remove_file(&path).ok();

        assert!(matches!(r, Err(NorbertError::Protected)));
        // The safety property: a protected chip is neither erased nor written.
        assert!(
            probe.mem().iter().all(|&b| b == 0xFF),
            "protected chip was modified"
        );
    }

    #[tokio::test]
    async fn program_erases_writes_and_verifies_ok() {
        let image: Vec<u8> = (0..600).map(|i| (i % 256) as u8).collect();
        let path = temp_image("prog", &image);
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);

        // verify ON (no_verify = false); a mismatch would surface as Err here.
        program(&mut f, &mut ui, &path, 0, false, false, false)
            .await
            .unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(&probe.mem()[..600], &image[..]);
        assert_eq!(out.contents(), "OK\n"); // machine token; voice/progress suppressed
    }
}
