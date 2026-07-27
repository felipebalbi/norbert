//! Check-up + self-test. `doctor` is read-only and steps the SPI clock.

use embedded_hal_async::spi::SpiDevice;

use super::{build_flasher, build_flasher_at, with_cancel};
use crate::catalog;
use crate::cli::Cli;
use crate::error::NorbertError;
use crate::flash::{BusAccess, Flasher};
use crate::sfdp::SfdpHeader;
use crate::ui::Ui;
use crate::voice;

/// Wiring/power/speed check-up. Never destructive.
pub async fn doctor(cli: &Cli, ui: &mut Ui) -> Result<(), NorbertError> {
    ui.say(voice::doctor_intro());

    let mut f = build_flasher(cli)?;
    f.acquire_bus().await?;
    let id_res = f.read_id().await;
    let sfdp_res = {
        let mut hdr = [0u8; 8];
        f.read_sfdp(0, &mut hdr)
            .await
            .map(|_| SfdpHeader::parse(&hdr).is_some())
    };
    let _ = f.release_bus();

    let id = match id_res {
        Ok(id) => id,
        Err(e) => {
            ui.line(&format!("RDID: failed ({e})"), "rdid=fail");
            ui.line(voice::doctor_rdid_fail(), "FAIL: rdid");
            return Ok(());
        }
    };
    ui.line(
        &format!(
            "RDID @ {} Hz: {:02X} {:02X} {:02X}",
            cli.freq, id.manufacturer, id.mem_type, id.capacity_code
        ),
        &format!(
            "rdid={:02X}{:02X}{:02X}",
            id.manufacturer, id.mem_type, id.capacity_code
        ),
    );
    if !id.is_present() {
        ui.line(voice::no_flash(), "FAIL: no chip");
        ui.say("  Check CS (--cs), MISO, GND, power, and that any other bus master is held off (--hold-gpio).");
        return Ok(());
    }
    ui.line(
        &format!("chip: {}", catalog::describe(id.jedec())),
        &format!("chip={}", catalog::describe(id.jedec())),
    );

    let mut warned = false;
    if id.manufacturer == id.mem_type && id.mem_type == id.capacity_code {
        ui.line(
            &format!(
                "WARNING: all three JEDEC bytes are 0x{:02X} — MISO may be stuck or power/wiring is wrong.",
                id.manufacturer
            ),
            "warn=miso",
        );
        warned = true;
    }
    match sfdp_res {
        Ok(true) => ui.line("SFDP: present", "sfdp=present"),
        Ok(false) => ui.line("SFDP: absent (will use the fallback table)", "sfdp=absent"),
        Err(e) => ui.line(&format!("SFDP: read failed ({e})"), "sfdp=error"),
    }

    let mut stable = true;
    for freq in [1_000_000u32, 5_000_000, 10_000_000] {
        match build_flasher_at(cli, freq) {
            Ok(mut ff) => {
                if ff.acquire_bus().await.is_err() {
                    ui.line(
                        &format!("  {freq} Hz: bus acquire failed"),
                        &format!("{freq}=acquire-fail"),
                    );
                    stable = false;
                    continue;
                }
                let r = ff.read_id().await;
                let _ = ff.release_bus();
                match r {
                    Ok(fid) if fid.jedec() == id.jedec() => ui.line(
                        &format!(
                            "  {freq} Hz: {:02X} {:02X} {:02X} OK",
                            fid.manufacturer, fid.mem_type, fid.capacity_code
                        ),
                        &format!("{freq}=ok"),
                    ),
                    Ok(fid) => {
                        ui.line(
                            &format!(
                                "  {freq} Hz: {:02X} {:02X} {:02X} MISMATCH",
                                fid.manufacturer, fid.mem_type, fid.capacity_code
                            ),
                            &format!("{freq}=mismatch"),
                        );
                        stable = false;
                    }
                    Err(e) => {
                        ui.line(
                            &format!("  {freq} Hz: read failed ({e})"),
                            &format!("{freq}=error"),
                        );
                        stable = false;
                    }
                }
            }
            Err(e) => {
                ui.line(
                    &format!("  {freq} Hz: connect failed ({e:#})"),
                    &format!("{freq}=connect-fail"),
                );
                stable = false;
            }
        }
    }

    if stable && !warned {
        ui.line(voice::nothing_unusual(), "OK");
    } else {
        ui.line(voice::doctor_unstable(), "WARN");
    }
    Ok(())
}

/// Read-back consistency (no sector), or a destructive sector self-test.
pub async fn test<SPI, RST>(
    f: &mut Flasher<SPI, RST>,
    ui: &mut Ui,
    sector: Option<usize>,
) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    // with_cancel (not with_bus): the destructive --sector path does seconds of
    // erase/program/restore, so a Ctrl-C must still release the held bus master.
    with_cancel(f, async |f| {
        f.detect_profile().await?;
        match sector {
            None => {
                let n = 4096;
                let mut a = vec![0u8; n];
                let mut b = vec![0u8; n];
                f.read(0, &mut a).await?;
                f.read(0, &mut b).await?;
                if a != b {
                    return Err(anyhow::anyhow!(
                        "read-back inconsistent between two reads — signal integrity suspect"
                    )
                    .into());
                }
            }
            Some(n) => {
                if f.is_protected().await? {
                    return Err(NorbertError::Protected);
                }
                let sec = f.profile().map(|p| p.min_erase()).unwrap_or(4096);
                let cap = f.profile().and_then(|p| p.capacity);
                let base = n.saturating_mul(sec);
                if let Some(cap) = cap
                    && base.saturating_add(sec) > cap
                {
                    return Err(anyhow::anyhow!(
                        "sector {n} is out of range (chip holds {} sectors of {} bytes)",
                        cap / sec,
                        sec
                    )
                    .into());
                }
                let mut backup = vec![0u8; sec];
                f.read(base, &mut backup).await?;
                let pattern: Vec<u8> = (0..sec).map(|i| (i as u8) ^ 0xA5).collect();
                let test_res = async {
                    f.erase_range(base, sec).await?;
                    f.program(base, &pattern, |_| {}).await?;
                    f.verify(base, &pattern, |_| {}).await?;
                    Ok::<(), NorbertError>(())
                }
                .await;
                // ALWAYS attempt to restore the original, even if the test failed.
                let restore_res = async {
                    f.erase_range(base, sec).await?;
                    f.program(base, &backup, |_| {}).await?;
                    f.verify(base, &backup, |_| {}).await?;
                    Ok::<(), NorbertError>(())
                }
                .await;
                test_res?;
                restore_res.map_err(|e| {
                    anyhow::anyhow!(
                        "sector test passed but restoring the original contents failed: {e}"
                    )
                })?;
            }
        }
        Ok(())
    })
    .await?;
    ui.line(voice::nothing_unusual(), "OK");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::testfakes::{FakeBus, FakeFlash, flasher};
    use crate::ui::{Mode, Ui};

    // M25P16 (fallback): 64 KiB sectors, so sector 0 is [0, 65536).
    const SEC: usize = 64 * 1024;
    fn recognizable() -> Vec<u8> {
        (0..SEC).map(|i| (i % 251) as u8).collect()
    }

    #[tokio::test]
    async fn test_sector_round_trip_restores_original() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        let original = recognizable();
        flash.preset(0, &original);
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);

        test(&mut f, &mut ui, Some(0)).await.unwrap();

        assert_eq!(probe.mem()[..SEC], original[..], "sector not restored");
        assert_eq!(out.contents(), "OK\n");
    }

    #[tokio::test]
    async fn test_sector_refuses_protected_and_writes_nothing() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        flash.set_protected(true);
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        let (mut ui, _o, _e) = Ui::captured(Mode::Machine);

        let r = test(&mut f, &mut ui, Some(0)).await;

        assert!(matches!(r, Err(NorbertError::Protected)));
        assert!(
            probe.mem().iter().all(|&b| b == 0xFF),
            "a protected chip was modified"
        );
    }

    #[tokio::test]
    async fn test_sector_restores_even_when_the_pattern_test_fails() {
        // The safety crown-jewel: a failed pattern test must NOT leave the sector
        // destroyed — the original is always restored.
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        let original = recognizable();
        flash.preset(0, &original);
        flash.set_corrupt_next_program(); // the pattern PROGRAM silently fails
        let probe = flash.clone();
        let mut f = flasher(flash, FakeBus::new(), 256);
        let (mut ui, _o, _e) = Ui::captured(Mode::Machine);

        let r = test(&mut f, &mut ui, Some(0)).await;

        assert!(matches!(r, Err(NorbertError::VerifyMismatch { .. })));
        assert_eq!(
            probe.mem()[..SEC],
            original[..],
            "sector not restored after a failed test"
        );
    }

    #[tokio::test]
    async fn test_none_passes_on_consistent_reads() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        flash.preset(0, &[0x12u8; 4096]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);

        test(&mut f, &mut ui, None).await.unwrap();

        assert_eq!(out.contents(), "OK\n");
    }
}
