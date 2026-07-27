//! Pico de Gallo glue over a fixed pin map: binds the HAL's SpiDevice + GPIOs to
//! the flash core, drives the flash's static control lines (/WP, /HOLD) high, and
//! implements an optional CRESET-based bus hold.

use anyhow::{Result, anyhow};
use embedded_hal::digital::OutputPin;
use pico_de_gallo_hal::{Gpio, GpioDirection, GpioPull, Hal, SpiDev, SpiPhase, SpiPolarity};

use crate::flash::BusAccess;

// Fixed Pico de Gallo v1.1 user-GPIO assignments (box-header pins 11-14).
const GPIO_CS: u8 = 0; // header pin 11 — flash CS (SS_B), software-driven per SPI txn
const GPIO_WP: u8 = 1; // header pin 12 — flash IO2 (/WP), held high (deasserted)
const GPIO_HOLD: u8 = 2; // header pin 13 — flash IO3 (/HOLD), held high (deasserted)
const GPIO_CRESET: u8 = 3; // header pin 14 — target CRESET, driven only with --reset

/// The flash's control lines plus an optional bus hold, all on fixed GPIOs.
///
/// `/WP` (IO2) and `/HOLD` (IO3) are driven **high** (their deasserted state) so a
/// bare chip with no external pull-ups still responds and its status register stays
/// writable. When `reset` is set, CRESET is driven **low** while we own the bus and
/// **high** on release, so a shared master (e.g. an iCE40) is held off during
/// programming and boots afterwards — with or without an external CRESET pull-up.
pub struct HostBus {
    wp: Gpio,
    hold: Gpio,
    creset: Gpio,
    reset: bool,
}

impl BusAccess for HostBus {
    type Error = pico_de_gallo_hal::GpioHalError;

    fn acquire(&mut self) -> Result<(), Self::Error> {
        // Deassert /WP and /HOLD (high): the flash responds and WRSR isn't blocked.
        self.wp.set_config(GpioDirection::Output, GpioPull::None)?;
        self.wp.set_high()?;
        self.hold
            .set_config(GpioDirection::Output, GpioPull::None)?;
        self.hold.set_high()?;
        // Hold the shared bus master off while we work.
        if self.reset {
            self.creset
                .set_config(GpioDirection::Output, GpioPull::None)?;
            self.creset.set_low()?;
        }
        Ok(())
    }

    fn release(&mut self) -> Result<(), Self::Error> {
        // Boot the held master; drive high so it works even with no CRESET pull-up.
        if self.reset {
            self.creset
                .set_config(GpioDirection::Output, GpioPull::None)?;
            self.creset.set_high()?;
        }
        Ok(())
    }
}

/// Live connection. `spi`/`bus` each own `Arc<Mutex<PicoDeGallo>>` clones that keep
/// the USB client (and its worker) alive after the `Hal` is dropped.
pub struct Connected {
    pub spi: SpiDev,
    pub bus: HostBus,
}

/// Connect, validate firmware schema, configure SPI (mode 0), and build handles
/// over the fixed pin map. `reset` enables the CRESET bus hold on User GPIO 3.
pub fn connect(serial: Option<&str>, freq_hz: u32, reset: bool) -> Result<Connected> {
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

    // CS on User GPIO 0; spi_device asserts it low around each SPI batch.
    let spi = hal
        .spi_device(GPIO_CS)
        .map_err(|e| anyhow!("spi_device({GPIO_CS}) failed: {e:?}"))?;

    // Each hal.gpio() clones the Arc that keeps the USB client alive, so these three
    // handles outlive the dropped `hal` (same lifetime trick as `spi`).
    let bus = HostBus {
        wp: hal.gpio(GPIO_WP),
        hold: hal.gpio(GPIO_HOLD),
        creset: hal.gpio(GPIO_CRESET),
        reset,
    };

    // `spi`/`bus` hold Arc clones that keep the client alive; the Hal handle is no
    // longer needed. No Box::leak — norbert owns the runtime for `main`.
    drop(hal);
    Ok(Connected { spi, bus })
}
