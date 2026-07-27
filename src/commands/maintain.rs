//! Protection + reset. Short bus sessions.

use embedded_hal_async::spi::SpiDevice;

use super::with_bus;
use crate::error::NorbertError;
use crate::flash::{BusAccess, Flasher};
use crate::ui::Ui;
use crate::voice;

pub async fn protect<SPI, RST>(f: &mut Flasher<SPI, RST>, ui: &mut Ui) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    with_bus(f, async |f| Ok(f.protect().await?)).await?;
    ui.line(voice::protect_done(), "OK");
    Ok(())
}

pub async fn unprotect<SPI, RST>(f: &mut Flasher<SPI, RST>, ui: &mut Ui) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    with_bus(f, async |f| Ok(f.unprotect().await?)).await?;
    ui.line(voice::unprotect_done(), "OK");
    Ok(())
}

pub async fn reset<SPI, RST>(f: &mut Flasher<SPI, RST>, ui: &mut Ui) -> Result<(), NorbertError>
where
    SPI: SpiDevice,
    RST: BusAccess,
{
    // release_bus (inside with_bus) also reboots a held master.
    with_bus(f, async |f| Ok(f.reset_flash().await?)).await?;
    ui.line(voice::reset_done(), "OK");
    Ok(())
}
