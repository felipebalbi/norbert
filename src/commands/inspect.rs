//! Read-only inspection commands. All output flows through `Ui`.

use embedded_hal_async::spi::SpiDevice;

use super::with_bus;
use crate::catalog;
use crate::error::NorbertError;
use crate::flash::{BusAccess, Flasher};
use crate::profile::ProfileSource;
use crate::sfdp::SfdpHeader;
use crate::ui::{Row, Ui};
use crate::voice;

/// Raw 3-byte JEDEC ID — deliberately terse in both modes.
pub async fn jedec<SPI, RST>(f: &mut Flasher<SPI, RST>, ui: &mut Ui) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    let id = with_bus(f, async |f| Ok(f.read_id().await?)).await?;
    ui.line(
        &format!(
            "{:02X} {:02X} {:02X}",
            id.manufacturer, id.mem_type, id.capacity_code
        ),
        &format!(
            "{:02X}{:02X}{:02X}",
            id.manufacturer, id.mem_type, id.capacity_code
        ),
    );
    Ok(())
}

/// Detect + name the part.
pub async fn detect<SPI, RST>(f: &mut Flasher<SPI, RST>, ui: &mut Ui) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    ui.say(voice::detect_opener());
    let jedec = with_bus(f, async |f| {
        let id = f.read_id().await?;
        f.detect_profile().await?;
        Ok(id.jedec())
    })
    .await?;
    let name = catalog::describe(jedec);
    ui.line(
        &voice::found(&name),
        &format!("{:02X} {:02X} {:02X}", jedec[0], jedec[1], jedec[2]),
    );
    Ok(())
}

/// Full profile. Always shows what it can; an unknown chip is a note, not a failure.
pub async fn info<SPI, RST>(f: &mut Flasher<SPI, RST>, ui: &mut Ui) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    ui.say(voice::info_opener());
    let (id, profile) = with_bus(f, async |f| {
        let id = f.read_id().await?;
        if !id.is_present() {
            return Err(NorbertError::NoFlash);
        }
        let profile = match f.detect_profile().await {
            Ok(p) => Some(p),
            Err(crate::flash::FlashError::UnsupportedChip { .. }) => None,
            Err(e) => return Err(e.into()),
        };
        Ok((id, profile))
    })
    .await?;

    let mut rows = vec![
        Row::split(
            "jedec",
            "JEDEC id",
            id.to_string(),
            format!(
                "{:02X}{:02X}{:02X}",
                id.manufacturer, id.mem_type, id.capacity_code
            ),
        ),
        Row::new("chip", "chip", catalog::describe(id.jedec())),
    ];
    if let Some(p) = &profile {
        rows.push(Row::new(
            "source",
            "source",
            match p.source {
                ProfileSource::Sfdp => "SFDP",
                ProfileSource::Table => "table",
            },
        ));
        rows.push(Row::split(
            "page",
            "page",
            format!("{} B", p.page_size),
            p.page_size.to_string(),
        ));
        rows.push(Row::new("address", "address", p.address_width.to_string()));
        match p.capacity {
            Some(c) => rows.push(Row::split(
                "capacity",
                "capacity",
                format!("{} KiB", c / 1024),
                c.to_string(),
            )),
            None => rows.push(Row::new("capacity", "capacity", "unknown")),
        }
        if let Some((maj, min)) = p.sfdp_revision {
            rows.push(Row::new("sfdp_rev", "SFDP rev", format!("{maj}.{min}")));
        }
        let menu: Vec<String> = p
            .erase
            .iter()
            .map(|e| format!("{}:{:02X}", e.size, e.opcode))
            .collect();
        // Human reads space-separated; machine is comma-separated (no spaces in a key=value).
        rows.push(Row::split("erase", "erase", menu.join(" "), menu.join(",")));
    }
    ui.rows(&rows);
    match &profile {
        Some(p) => ui.say(voice::info_sfdp_note(p.sfdp_revision.is_some())),
        None => ui.say(&voice::unsupported(id.jedec())),
    }
    Ok(())
}

/// Raw SFDP hex dump (first 256 bytes) or a note that there is none.
pub async fn sfdp<SPI, RST>(f: &mut Flasher<SPI, RST>, ui: &mut Ui) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    let blob = with_bus(f, async |f| {
        let mut header = [0u8; 8];
        f.read_sfdp(0, &mut header).await?;
        if SfdpHeader::parse(&header).is_none() {
            return Ok(None);
        }
        let mut blob = vec![0u8; 256];
        f.read_sfdp(0, &mut blob).await?;
        Ok(Some(blob))
    })
    .await?;
    match blob {
        None => ui.line(voice::no_sfdp(), "sfdp=absent"),
        Some(blob) => {
            ui.say(voice::sfdp_opener());
            ui.hexdump(0, &blob);
        }
    }
    Ok(())
}

/// The no-SFDP fallback table. No hardware needed.
pub fn list(ui: &mut Ui) {
    ui.say(voice::list_opener());
    for c in catalog::FALLBACK_TABLE {
        let row = format!(
            "{:02X} {:02X} {:02X}  {}",
            c.jedec[0],
            c.jedec[1],
            c.jedec[2],
            catalog::describe(c.jedec)
        );
        ui.line(&row, &row);
    }
    ui.say(voice::list_note());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::testfakes::{FakeBus, FakeFlash, flasher};
    use crate::ui::{Mode, Ui};

    #[tokio::test]
    async fn detect_speaks_in_human_and_is_terse_in_machine() {
        // M25P16 has no SFDP but is in the fallback table.
        let mut f = flasher(
            FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]),
            FakeBus::new(),
            256,
        );
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        detect(&mut f, &mut ui).await.unwrap();
        assert_eq!(
            out.contents(),
            format!("{}\nFound Micron M25P16.\n", voice::detect_opener())
        );

        let mut f = flasher(
            FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]),
            FakeBus::new(),
            256,
        );
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        detect(&mut f, &mut ui).await.unwrap();
        assert_eq!(out.contents(), "20 20 15\n");
    }

    #[tokio::test]
    async fn info_machine_is_key_value() {
        let mut f = flasher(
            FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]),
            FakeBus::new(),
            256,
        );
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        info(&mut f, &mut ui).await.unwrap();
        let s = out.contents();
        assert!(s.contains("jedec=202015\n"), "got:\n{s}");
        assert!(s.contains("source=table\n"), "got:\n{s}");
        assert!(s.contains("address=3-byte\n"), "got:\n{s}");
    }

    #[tokio::test]
    async fn sfdp_absent_emits_machine_token() {
        // Default FakeFlash has no SFDP, so read_sfdp returns 0xFF and parse fails.
        let mut f = flasher(
            FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]),
            FakeBus::new(),
            256,
        );
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        sfdp(&mut f, &mut ui).await.unwrap();
        assert_eq!(out.contents(), "sfdp=absent\n");
    }

    #[test]
    fn list_frames_in_human_and_is_bare_in_machine() {
        // Machine: rows only, voice opener/note suppressed.
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        list(&mut ui);
        let m = out.contents();
        assert!(m.contains("20 20 15  Micron M25P16\n"), "got:\n{m}");
        assert!(!m.contains(voice::list_opener()));
        assert!(!m.contains(voice::list_note()));

        // Human: opener + rows + note.
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        list(&mut ui);
        let h = out.contents();
        assert!(h.starts_with(voice::list_opener()));
        assert!(h.contains("20 20 15  Micron M25P16\n"));
        assert!(h.trim_end().ends_with(voice::list_note()));
    }
}
