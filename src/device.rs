//! Pico de Gallo glue: binds the HAL's SpiDevice + GPIO to the flash core,
//! and implements a runtime-configurable bus hold (or no hold at all).

use anyhow::{Result, anyhow};
use embedded_hal::digital::OutputPin;
use pico_de_gallo_hal::{GpioDirection, GpioPull, Hal, SpiDev, SpiPhase, SpiPolarity};

use crate::flash::BusAccess;

/// Level to hold the bus-arbitration GPIO at while programming.
#[derive(Clone, Copy)]
pub enum Level {
    High,
    Low,
}
/// What to do with the GPIO on release.
#[derive(Clone, Copy)]
pub enum Release {
    DriveHigh,
    DriveLow,
    HiZ,
}

/// User-supplied bus-hold configuration (from CLI `--hold-*`).
pub struct HoldConfig {
    pub pin: u8,
    pub active: Level,
    pub release: Release,
}

/// Bus access over a Pico de Gallo GPIO — or a **no-op** when `gpio` is `None`
/// (bare chip / clip). One concrete type so the CLI has a single `BUS`.
pub struct HostBus {
    gpio: Option<pico_de_gallo_hal::Gpio>,
    active: Level,
    release: Release,
}

impl BusAccess for HostBus {
    type Error = pico_de_gallo_hal::GpioHalError;

    fn acquire(&mut self) -> Result<(), Self::Error> {
        let Some(g) = self.gpio.as_mut() else {
            return Ok(());
        };
        g.set_config(GpioDirection::Output, GpioPull::None)?;
        match self.active {
            Level::Low => g.set_low(),
            Level::High => g.set_high(),
        }
    }

    fn release(&mut self) -> Result<(), Self::Error> {
        let Some(g) = self.gpio.as_mut() else {
            return Ok(());
        };
        match self.release {
            // Hi-Z: let the board's pull-up/down take the line (e.g. iCE40 CRESET).
            Release::HiZ => g.set_config(GpioDirection::Input, GpioPull::None),
            Release::DriveHigh => {
                g.set_config(GpioDirection::Output, GpioPull::None)?;
                g.set_high()
            }
            Release::DriveLow => {
                g.set_config(GpioDirection::Output, GpioPull::None)?;
                g.set_low()
            }
        }
    }
}

/// Live connection. Holds `_hal` only for construction; once `spi`/`bus` exist
/// they each own an `Arc<Mutex<PicoDeGallo>>` clone that keeps the USB client
/// (and its worker) alive, so the caller may drop `_hal`.
pub struct Connected {
    pub _hal: Hal,
    pub spi: SpiDev,
    pub bus: HostBus,
}

/// Connect, validate firmware schema, configure SPI (mode 0), and build handles.
/// `cs_pin` is a Pico de Gallo GPIO (0–3); `hold` is optional and must use a
/// different GPIO than CS.
pub fn connect(
    serial: Option<&str>,
    freq_hz: u32,
    cs_pin: u8,
    hold: Option<HoldConfig>,
) -> Result<Connected> {
    if let Some(h) = &hold {
        assert_ne!(cs_pin, h.pin, "CS and hold GPIO must differ");
    }
    let mut hal = match serial {
        Some(sn) => Hal::new_validated_with_serial_number(sn),
        None => Hal::new_validated(),
    }
    .map_err(|e| anyhow!("connect/validate failed (device attached? firmware current?): {e:?}"))?;

    let _ = hal.system_reset_subscriptions();

    // SPI mode 0 (CPOL=0, CPHA=0): IdleLow + CaptureOnFirstTransition.
    hal.spi_set_config(
        freq_hz,
        SpiPhase::CaptureOnFirstTransition,
        SpiPolarity::IdleLow,
    )
    .map_err(|e| anyhow!("spi_set_config failed: {e:?}"))?;

    let spi = hal
        .spi_device(cs_pin)
        .map_err(|e| anyhow!("spi_device({cs_pin}) failed: {e:?}"))?;
    let bus = match hold {
        Some(h) => HostBus {
            gpio: Some(hal.gpio(h.pin)),
            active: h.active,
            release: h.release,
        },
        None => HostBus {
            gpio: None,
            active: Level::Low,
            release: Release::HiZ,
        },
    };

    Ok(Connected {
        _hal: hal,
        spi,
        bus,
    })
}
