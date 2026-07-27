# Fixed GPIO Pinout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Norbert's user-assignable GPIO flags with a fixed flash pin map (CS→GPIO0, /WP→GPIO1, /HOLD→GPIO2, CRESET→GPIO3), drive /WP and /HOLD high, gate CRESET behind a single `--reset` flag, and document the wiring in the README.

**Architecture:** The device layer (`device.rs`) hardcodes the four user GPIOs, drives /WP and /HOLD high on every bus `acquire`, and drives CRESET low/high on acquire/release only when `--reset` is set. The CLI (`cli.rs`) drops `--cs`/`--hold-gpio`/`--hold-active`/`--hold-release` for a single boolean `--reset`. `HostBus` acquire/release logic touches the real HAL and is not unit-tested (no fake HAL exists; consistent with the current codebase, which has no `device.rs` tests); the unit-testable seam is CLI parsing.

**Tech Stack:** Rust (edition 2024), clap 4 (derive), `pico-de-gallo-hal` 0.6, `embedded-hal` 1.0 (`OutputPin`), tokio, anyhow.

**Branch:** `fixed-gpio-pinout` (already created).

---

## File Structure

- `src/device.rs` — **rewrite** `HostBus` + `connect()`; delete `Level`/`Release`/`HoldConfig`; add fixed-pin constants. Owns the pin contract and the /WP,/HOLD,CRESET drive logic.
- `src/cli.rs` — **modify** the `Cli` struct: remove four flags + the `hold()` builder + the `device::{HoldConfig, Level, Release}` import; add `reset: bool`. Add a `#[cfg(test)]` parse-test module.
- `src/commands/mod.rs` — **modify** one line: `build_flasher_at` calls `device::connect(.., cli.reset)`.
- `src/voice.rs` — **modify** one string: `doctor_no_chip_hint` (drop stale flag names).
- `README.md` — **modify**: replace the "Connecting" section with a new "Wiring" section (pin table + 2×12 header diagram + notes) followed by a trimmed "Connecting" section.

**Coupling note:** `cli.rs`, `device.rs`, and `commands/mod.rs` are mutually dependent (the crate will not compile until all three agree on the new surface), so Task 1 changes them together in one commit. Task 2 then adds the CLI regression tests that lock the new contract.

---

### Task 1: Rewire the device layer, CLI, and callers to the fixed pinout

**Files:**
- Modify: `src/device.rs` (full rewrite of body)
- Modify: `src/cli.rs:7` (import), `src/cli.rs:23-36` (flags), `src/cli.rs:47-56` (`hold()`)
- Modify: `src/commands/mod.rs:38-40`
- Modify: `src/voice.rs:60-63`

- [ ] **Step 1: Rewrite `src/device.rs`**

Replace the **entire** contents of `src/device.rs` with:

```rust
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
        self.hold.set_config(GpioDirection::Output, GpioPull::None)?;
        self.hold.set_high()?;
        // Hold the shared bus master off while we work.
        if self.reset {
            self.creset.set_config(GpioDirection::Output, GpioPull::None)?;
            self.creset.set_low()?;
        }
        Ok(())
    }

    fn release(&mut self) -> Result<(), Self::Error> {
        // Boot the held master; drive high so it works even with no CRESET pull-up.
        if self.reset {
            self.creset.set_config(GpioDirection::Output, GpioPull::None)?;
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
```

- [ ] **Step 2: Edit `src/cli.rs` — remove the device import**

Delete this line (currently `src/cli.rs:7`):

```rust
use crate::device::{HoldConfig, Level, Release};
```

- [ ] **Step 3: Edit `src/cli.rs` — replace the four GPIO flags with `--reset`**

In the `Cli` struct, delete the `cs`, `hold_gpio`, `hold_active`, and `hold_release` fields (currently lines 23-36):

```rust
    /// User GPIO (0-3) wired to the flash CS (SS_B). Default: User GPIO 0 (header pin 11).
    #[arg(long, global = true, default_value_t = 0)]
    pub cs: u8,
    /// User GPIO (0-3) that holds another bus master (e.g. an FPGA's CRESET) off the
    /// shared SPI while we work. Default: User GPIO 1 (header pin 12).
    /// iCE40 example: `--hold-gpio 1 --hold-active low --hold-release hi-z`.
    #[arg(long, global = true, default_value_t = 1)]
    pub hold_gpio: u8,
    /// Level to hold the bus GPIO at.
    #[arg(long, global = true, value_enum, default_value_t = Level::Low)]
    pub hold_active: Level,
    /// What to do with the bus GPIO on release.
    #[arg(long, global = true, value_enum, default_value_t = Release::HiZ)]
    pub hold_release: Release,
```

and replace them with the single `reset` flag:

```rust
    /// Hold a shared bus master off the SPI while programming, then release it so it
    /// boots. Wire the target's CRESET to User GPIO 3 (header pin 14). Omit for a
    /// bare chip or any flash with no other bus master on the SPI.
    #[arg(long, global = true)]
    pub reset: bool,
```

- [ ] **Step 4: Edit `src/cli.rs` — delete the `hold()` builder**

Remove the entire `impl Cli` block (currently lines 47-56):

```rust
impl Cli {
    /// Build the bus-hold config from the flags (hold GPIO defaults to User GPIO 1).
    pub fn hold(&self) -> HoldConfig {
        HoldConfig {
            pin: self.hold_gpio,
            active: self.hold_active,
            release: self.hold_release,
        }
    }
}
```

- [ ] **Step 5: Edit `src/commands/mod.rs` — update the `connect` call**

Replace the body of `build_flasher_at` (currently `src/commands/mod.rs:39-40`):

```rust
    let device::Connected { spi, bus } =
        device::connect(cli.serial.as_deref(), freq, cli.cs, cli.hold())?;
```

with:

```rust
    let device::Connected { spi, bus } =
        device::connect(cli.serial.as_deref(), freq, cli.reset)?;
```

- [ ] **Step 6: Edit `src/voice.rs` — refresh the no-chip hint**

Replace `doctor_no_chip_hint` (currently `src/voice.rs:60-63`):

```rust
/// doctor: what to check when no chip answers.
pub fn doctor_no_chip_hint() -> &'static str {
    "  Check CS (--cs), MISO, GND, power, and that any other bus master is held off (--hold-gpio)."
}
```

with:

```rust
/// doctor: what to check when no chip answers.
pub fn doctor_no_chip_hint() -> &'static str {
    "  Check CS, MISO, GND, and power — and if another bus master shares the SPI, add --reset."
}
```

- [ ] **Step 7: Build and verify the crate compiles**

Run: `cargo build`
Expected: `Finished` with no errors. (If clap complains about an unused import or `Level`/`Release`, re-check Steps 2-4 removed every reference.)

- [ ] **Step 8: Run the existing test suite**

Run: `cargo test`
Expected: `test result: ok.` — all existing tests pass (34 as of writing). The `FakeBus` tests are unaffected because `FakeBus` implements `BusAccess` directly, not via `HostBus`.

- [ ] **Step 9: Commit**

```bash
git add src/device.rs src/cli.rs src/commands/mod.rs src/voice.rs
git commit -m "feat(wiring): fixed GPIO pin map; drive /WP+/HOLD high; --reset for CRESET"
```

---

### Task 2: Lock the new CLI contract with parse tests

**Files:**
- Modify: `src/cli.rs` (append a `#[cfg(test)]` module at end of file)

- [ ] **Step 1: Write the CLI parse tests**

Append to the end of `src/cli.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_defaults_off_and_parses_when_set() {
        let cli = Cli::try_parse_from(["norbert", "jedec"]).expect("bare subcommand parses");
        assert!(!cli.reset, "--reset must default to false");

        let cli = Cli::try_parse_from(["norbert", "--reset", "jedec"]).expect("--reset parses");
        assert!(cli.reset, "--reset must set the flag");
    }

    #[test]
    fn removed_gpio_flags_are_rejected() {
        for bad in ["--cs", "--hold-gpio", "--hold-active", "--hold-release"] {
            let r = Cli::try_parse_from(["norbert", bad, "0", "jedec"]);
            assert!(r.is_err(), "{bad} should no longer be a valid flag");
        }
    }
}
```

(`Cli::try_parse_from` comes from the `clap::Parser` trait already imported at the top of the file and re-exported via `use super::*`.)

- [ ] **Step 2: Run the new tests and verify they pass**

Run: `cargo test --lib cli::tests`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "test(cli): lock the --reset flag and reject removed GPIO flags"
```

---

### Task 3: Document the wiring in the README

**Files:**
- Modify: `README.md:85-99` (replace the "Connecting" section)

- [ ] **Step 1: Replace the "Connecting" section**

In `README.md`, delete the current "Connecting" section (lines 85-99, from `## Connecting` through the paragraph ending `...freshly written flash.`) and replace it with the following. (The block below is the literal Markdown to insert.)

~~~markdown
## Wiring

Norbert talks to a raw SPI-NOR flash over a **Pico de Gallo v1.1** USB bridge.
The wiring is fixed — connect the flash to these header pins and every command
just works:

| Flash pin        | Wire to (Pico de Gallo) | Header pin | Notes                          |
|------------------|-------------------------|------------|--------------------------------|
| CS / SS_B        | GPIO0                   | 11         | chip-select, software-driven   |
| SI / DI / IO0    | SPI_MOSI                | 6          | data **into** the flash        |
| SO / DO / IO1    | SPI_MISO                | 5          | data **out of** the flash      |
| SCK              | SPI_SCK                 | 7          | serial clock                   |
| /WP / IO2        | GPIO1                   | 12         | held high for you              |
| /HOLD / IO3      | GPIO2                   | 13         | held high for you              |
| GND              | GND                     | 2 (or 24)  | common ground (required)       |
| VCC              | +3V3 / VREF             | 23 (or 1)  | only if the Pico powers it     |
| CRESET           | GPIO3                   | 14         | optional — see "Connecting"    |

Norbert drives the flash's `/WP` (write-protect) and `/HOLD` lines **high** for
you, so a bare chip on a clip works without external pull-ups. If your board
already pulls them up, just leave GPIO1 and GPIO2 unconnected.

Two things worth knowing:

- **Chip-select is on GPIO0 (pin 11), not the header's `SPI_CS` (pin 8).**
  Norbert software-drives CS across each transaction; the hardware `SPI_CS` pad
  is not used.
- **Mind the data direction:** the flash's input (IO0/SI) goes to **MOSI
  (pin 6)** and its output (IO1/SO) goes to **MISO (pin 5)**. Reversing them
  reads all `0x00`/`0xFF`.

### Header pins Norbert uses

The v1.1 connector is a keyed **2×12 (24-pin)** box header. Viewed from above
with the USB pointing up, pin 1 is top-right:

```
        ┌──────── USB ────────┐
 pin  2 │ GND *        VREF     │ pin  1
 pin  4 │ I2C_SCL      I2C_SDA  │ pin  3
 pin  6 │ SPI_MOSI *   SPI_MISO*│ pin  5
 pin  8 │ SPI_CS       SPI_SCK *│ pin  7
 pin 10 │ UART_RX      UART_TX  │ pin  9
 pin 12 │ GPIO1 *      GPIO0   *│ pin 11
 pin 14 │ GPIO3 *      GPIO2   *│ pin 13
 pin 16 │ PWM1         PWM0     │ pin 15
 pin 18 │ PWM3         PWM2     │ pin 17
 pin 20 │ ADC0         ONEWIRE  │ pin 19
 pin 22 │ ADC2         ADC1     │ pin 21
 pin 24 │ GND          +3V3     │ pin 23
        └─────────────────────┘

 * used by Norbert:
   pin  6  SPI_MOSI → flash IO0 / SI  (data in)
   pin  5  SPI_MISO → flash IO1 / SO  (data out)
   pin  7  SPI_SCK  → flash SCK
   pin 11  GPIO0    → flash CS
   pin 12  GPIO1    → flash /WP   (driven high)
   pin 13  GPIO2    → flash /HOLD  (driven high)
   pin 14  GPIO3    → target CRESET (only with --reset)
   pin  2  GND      → common ground (pin 24 is also GND)
   pin  1 / 23      → +3V3, optional flash power
```

## Connecting

A few global flags tune the session:

- `--serial <SN>` — pick a specific Pico de Gallo by USB serial number
- `--freq <HZ>` — SPI clock (default 10 MHz)
- `--reset` — hold another bus master off the shared SPI while programming, then
  release it so it boots
- `--quiet` — machine-friendly output (IDs / addresses / `OK` / `FAIL` only)

If another master shares the bus, wire its reset to **CRESET (GPIO3, header
pin 14)** and pass `--reset`. Norbert drives CRESET low while programming and
high on release, so the master (for example an iCE40) is held off the SPI during
the write and reconfigures from the freshly written flash afterwards — whether or
not the board has a CRESET pull-up.
~~~

- [ ] **Step 2: Verify the README renders and reads correctly**

Run: `rg -n "GPIO0|/WP|--reset|2×12" README.md`
Expected: matches in the new Wiring/Connecting sections; no remaining `--cs` or `--hold-gpio` references.

Also confirm no stale flags survive anywhere in the README:

Run: `rg -n "\-\-cs|\-\-hold-gpio|\-\-hold-active|\-\-hold-release" README.md || echo "clean"`
Expected: `clean`.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs(readme): document the fixed flash wiring and header pinout"
```

---

### Task 4: Final verification gate

**Files:** none (verification only; commit any formatting fixes)

- [ ] **Step 1: Lint with warnings-as-errors**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: `Finished` with no warnings. (Common miss: a leftover `use clap::ValueEnum;` or an unused import in `device.rs`/`cli.rs` — remove it if flagged.)

- [ ] **Step 2: Check formatting**

Run: `cargo fmt --check`
Expected: no output (clean). If it reports diffs, run `cargo fmt` and stage the result.

- [ ] **Step 3: Full test suite**

Run: `cargo test`
Expected: `test result: ok.` for every test binary (existing tests + the 2 new CLI tests).

- [ ] **Step 4: Commit any formatting fixes (only if Step 2 changed files)**

```bash
git add -A
git commit -m "style: cargo fmt"
```

---

## Self-Review

**1. Spec coverage** (against `docs/superpowers/specs/2026-07-27-fixed-gpio-pinout-design.md`):
- §1 fixed pin contract → Task 1 Step 1 (`GPIO_CS/WP/HOLD/CRESET` constants + `connect`). ✓
- §3 CLI: remove four flags + `hold()` + import, add `--reset` → Task 1 Steps 2-4. ✓
- §4 device layer: delete `Level`/`Release`/`HoldConfig`, new `HostBus`, new `connect` signature, drop `cs==hold` check → Task 1 Step 1. ✓
- §4 /WP + /HOLD high, CRESET low/high gated on `reset` → Task 1 Step 1 (`acquire`/`release`). ✓
- §5 callers: `commands/mod.rs`, `voice.rs` → Task 1 Steps 5-6. ✓
- §6 README table + 2×12 diagram + notes + trimmed Connecting → Task 3. ✓
- §7 testing: build, clippy, fmt, tests → Task 1 Steps 7-8, Task 4; CLI parse tests → Task 2. ✓
- §8 breaking change (removed flags error) → verified by Task 2 `removed_gpio_flags_are_rejected`. ✓

**2. Placeholder scan:** No TBD/TODO/"handle appropriately"; every code and doc step contains literal content. ✓

**3. Type consistency:** `connect(serial: Option<&str>, freq_hz: u32, reset: bool)` defined in Task 1 Step 1 matches the call in Task 1 Step 5. `HostBus { wp, hold, creset, reset }` fields match their use in `acquire`/`release`. `cli.reset` (Task 1 Step 3) matches `cli.reset` in the `connect` call (Task 1 Step 5) and the tests (Task 2). `set_high`/`set_low` require the retained `use embedded_hal::digital::OutputPin;`. ✓
