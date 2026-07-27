//! Pico de Gallo glue: binds the HAL's SpiDevice + GPIO to the flash core,
//! and implements a runtime-configurable bus hold.

use anyhow::{Result, anyhow};
use clap::ValueEnum;
use embedded_hal::digital::OutputPin;
use pico_de_gallo_hal::{GpioDirection, GpioPull, Hal, SpiDev, SpiPhase, SpiPolarity};

use crate::flash::BusAccess;

/// Level to hold the bus-arbitration GPIO at while programming.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Level {
    High,
    Low,
}
/// What to do with the GPIO on release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
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

/// Bus access over a Pico de Gallo GPIO: held at `active` while we work, then
/// driven/tri-stated per `release`.
pub struct HostBus {
    gpio: pico_de_gallo_hal::Gpio,
    active: Level,
    release: Release,
}

impl BusAccess for HostBus {
    type Error = pico_de_gallo_hal::GpioHalError;

    fn acquire(&mut self) -> Result<(), Self::Error> {
        self.gpio
            .set_config(GpioDirection::Output, GpioPull::None)?;
        match self.active {
            Level::Low => self.gpio.set_low(),
            Level::High => self.gpio.set_high(),
        }
    }

    fn release(&mut self) -> Result<(), Self::Error> {
        match self.release {
            // Hi-Z: let the board's pull-up/down take the line (e.g. iCE40 CRESET).
            Release::HiZ => self.gpio.set_config(GpioDirection::Input, GpioPull::None),
            Release::DriveHigh => {
                self.gpio
                    .set_config(GpioDirection::Output, GpioPull::None)?;
                self.gpio.set_high()
            }
            Release::DriveLow => {
                self.gpio
                    .set_config(GpioDirection::Output, GpioPull::None)?;
                self.gpio.set_low()
            }
        }
    }
}

/// Live connection. `spi`/`bus` each own an `Arc<Mutex<PicoDeGallo>>` clone that
/// keeps the USB client (and its worker) alive after the `Hal` is dropped.
pub struct Connected {
    pub spi: SpiDev,
    pub bus: HostBus,
}

/// Connect, validate firmware schema, configure SPI (mode 0), and build handles.
/// `cs_pin` is a Pico de Gallo GPIO (0–3); `hold` must use a different GPIO.
pub fn connect(
    serial: Option<&str>,
    freq_hz: u32,
    cs_pin: u8,
    hold: HoldConfig,
) -> Result<Connected> {
    if cs_pin == hold.pin {
        return Err(anyhow!(
            "CS and hold GPIO must differ (both are User GPIO {cs_pin}); pick a different --cs or --hold-gpio"
        ));
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
    let bus = HostBus {
        gpio: hal.gpio(hold.pin),
        active: hold.active,
        release: hold.release,
    };

    // `spi`/`bus` hold Arc clones that keep the client alive; the Hal handle is
    // no longer needed. No Box::leak — norbert owns the runtime for `main`.
    drop(hal);
    Ok(Connected { spi, bus })
}
