# Norbert Idiomatic Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor Norbert into exemplary idiomatic Rust — route every human response through Norbert's voice, make invalid states unrepresentable in the type system, and give the tool a restrained-but-transparent Unicode progress UI — without changing a single byte on the SPI wire.

**Architecture:** A dedicated `ui` module owns all output and composes `voice` strings; a crate-wide `#![deny(clippy::print_stdout, clippy::print_stderr)]` makes the personality boundary a build error. Command logic moves out of `main.rs` into `commands/` handlers that only *sequence* `Ui`. The flash core gains `AddressWidth`/`EraseMenu`/`ErasePlan` newtypes that enforce today's comment-only invariants. A single `NorbertError` boundary replaces three ad-hoc converters.

**Tech Stack:** Rust (edition 2024), tokio 1 (multi-thread), embedded-hal-async 1, pico-de-gallo-hal 0.6, indicatif 0.17, clap 4 (derive + ValueEnum), anyhow 1.

**Spec:** `docs/superpowers/specs/2026-07-26-norbert-idiomatic-refactor-design.md`.

---

## Ground rules for every task

- **Wire-neutral.** No task may change the bytes `push_cmd_addr` emits or the order/opcodes of erase/program/read operations. This is a flasher; the SPI protocol is frozen.
- **Green gates.** Every task ends with `cargo test` (all pass), `cargo fmt`, and — from Phase 3 onward — `cargo clippy` clean under the new `deny`.
- **Machine contract.** Success tokens on **stdout** (`OK`, `EF4018`, byte counts, `EF 40 18`) stay byte-identical. Failures move to **stderr** as `FAIL: <reason>` (machine) / voice+fact (human); exit codes stay `1` (error) and `130` (Ctrl-C).
- **Personality only in `voice.rs`.** `ui` lays out and calls `voice`; handlers sequence `ui`. No `println!`/`eprintln!` anywhere (the lint enforces this from Phase 3).

## File structure (end state)

| File | Responsibility |
|---|---|
| `src/main.rs` | `#[tokio::main]`, parse, build `Ui`, dispatch, return `ExitCode`. ~40 lines. |
| `src/cli.rs` | clap `Cli`/`Cmd` + global args; uses `device::{Level, Release}`. |
| `src/ui/mod.rs` | `Ui` presenter over injected `Write` sinks; `Row`; `fail`. |
| `src/ui/progress.rs` | `Progress` over `indicatif::MultiProgress` (Unicode bars). |
| `src/voice.rs` | Pure personality strings (expanded). |
| `src/error.rs` | `NorbertError` + `From` conversions. |
| `src/commands/mod.rs` | `Cmd` dispatch + `with_bus`/`with_cancel` helpers. |
| `src/commands/inspect.rs` | `jedec`, `info`, `detect`, `sfdp`, `list`. |
| `src/commands/write.rs` | `program`, `erase`, `read`, `verify`. |
| `src/commands/maintain.rs` | `protect`, `unprotect`, `reset`. |
| `src/commands/diagnose.rs` | `doctor`, `test`. |
| `src/flash.rs` | Async flasher core (dead code removed); `AddressWidth`/`ErasePlan` threading; plan/execute erase. |
| `src/profile.rs` | `FlashProfile`, `EraseType`, `EraseMenu`, `AddressWidth`, `ProfileSource`, `EraseOp`, `ErasePlan`, `plan_erase`. |
| `src/sfdp.rs` | SFDP byte parsing: `SfdpHeader`, `ParamHeader`, `Bfpt`. |
| `src/catalog.rs` | Chip names + no-SFDP fallback table (`KnownChip`, `lookup_fallback`). |
| `src/device.rs` | `connect()`, `HostBus`, `HoldConfig`, `Level`, `Release`. |
| `src/testsupport.rs` | `#[cfg(test)]` shared `FakeFlash`/`FakeBus` + `Ui` buffer sink. |

---

# Phase 0 — Module split (mechanical, wire-neutral)

Splits today's overloaded `sfdp.rs` into a profile *model* (`profile.rs`) and a byte *parser* (`sfdp.rs`), and moves the fallback table into `catalog.rs`. Pure relocation; no logic changes.

## Task 0.1: Extract the profile model into `profile.rs`

**Files:**
- Create: `src/profile.rs`
- Modify: `src/sfdp.rs` (remove moved items, import `EraseType`)
- Modify: `src/main.rs:1-5` (add `mod profile;`)
- Modify: `src/flash.rs:7-9` (import paths)

- [ ] **Step 1: Create `src/profile.rs`** with the profile model moved verbatim from `sfdp.rs` (the `EraseType`, `ProfileSource`, `FlashProfile`, `plan_erase` items and their three tests):

```rust
//! Flash geometry model + the pure erase planner. No I/O, no SFDP bytes.

/// One supported erase granularity: `size` bytes via `opcode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseType {
    pub size: usize,
    pub opcode: u8,
}

/// How a `FlashProfile` was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Sfdp,
    Table,
}

/// How to talk to the flash: geometry + erase menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashProfile {
    pub page_size: usize,
    pub address_bytes: u8,           // 3 or 4
    pub capacity: Option<usize>,     // bytes, if known
    pub erase_types: Vec<EraseType>, // sorted largest-first
    pub source: ProfileSource,
}

impl FlashProfile {
    pub fn min_erase(&self) -> usize {
        self.erase_types
            .iter()
            .map(|e| e.size)
            .min()
            .unwrap_or(64 * 1024)
    }
}

/// Plan a minimal-ish erase covering `[offset, offset+len)`. Greedily takes the
/// largest address-aligned erase type that does not overshoot past the region
/// (rounded up to the smallest granularity), else the smallest. `(addr, opcode)`.
pub fn plan_erase(profile: &FlashProfile, offset: usize, len: usize) -> Vec<(usize, u8)> {
    let mut plan = Vec::new();
    if len == 0 || profile.erase_types.is_empty() {
        return plan;
    }
    let min = profile.min_erase();
    let mut sizes = profile.erase_types.clone();
    sizes.sort_by_key(|e| std::cmp::Reverse(e.size));
    let smallest = *sizes.last().unwrap();

    let end = offset + len;
    let end_aligned = end.div_ceil(min) * min;
    let mut a = offset - offset % min;
    while a < end {
        let choice = sizes
            .iter()
            .find(|e| a.is_multiple_of(e.size) && a + e.size <= end_aligned)
            .copied()
            .unwrap_or(smallest);
        plan.push((a, choice.opcode));
        a += choice.size;
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(sizes: &[(usize, u8)]) -> FlashProfile {
        FlashProfile {
            page_size: 256,
            address_bytes: 3,
            capacity: Some(2 * 1024 * 1024),
            erase_types: sizes
                .iter()
                .map(|(s, o)| EraseType {
                    size: *s,
                    opcode: *o,
                })
                .collect(),
            source: ProfileSource::Sfdp,
        }
    }

    #[test]
    fn single_granularity_plan_is_64k_blocks() {
        let p = profile_with(&[(64 * 1024, 0xD8)]);
        assert_eq!(
            plan_erase(&p, 0, 135_100),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0xD8)]
        );
    }

    #[test]
    fn mixed_granularity_uses_small_sector_at_tail() {
        let p = profile_with(&[(4 * 1024, 0x20), (32 * 1024, 0x52), (64 * 1024, 0xD8)]);
        assert_eq!(
            plan_erase(&p, 0, 131_072 + 100),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0x20)]
        );
    }

    #[test]
    fn zero_len_is_empty() {
        assert!(plan_erase(&profile_with(&[(64 * 1024, 0xD8)]), 0, 0).is_empty());
    }
}
```

- [ ] **Step 2: Remove the moved items from `src/sfdp.rs`.** Delete `EraseType`, `ProfileSource`, `FlashProfile`, its `impl`, `plan_erase`, and the three tests above (`single_granularity_plan_is_64k_blocks`, `mixed_granularity_uses_small_sector_at_tail`, `zero_len_is_empty`) from `sfdp.rs`. At the top of `sfdp.rs`, add:

```rust
use crate::profile::EraseType;
```

(`Bfpt` still constructs `EraseType`.) Update the module doc comment's first line to: `//! SFDP byte parsing: header, parameter headers, and the Basic Flash Parameter Table.`

- [ ] **Step 3: Register the module** in `src/main.rs`. The `mod` block at the top becomes:

```rust
mod catalog;
mod device;
mod flash;
mod profile;
mod sfdp;
mod voice;
```

- [ ] **Step 4: Fix `flash.rs` imports.** Change the `use crate::sfdp::{...}` block (currently lines 7-9) to pull the model from `profile` and the parser from `sfdp`:

```rust
use crate::profile::{FlashProfile, ProfileSource, plan_erase};
use crate::sfdp::{Bfpt, ParamHeader, SfdpHeader};
use crate::catalog::lookup_fallback;
```

Then rewrite every remaining `crate::sfdp::{EraseType,FlashProfile,ProfileSource}` reference elsewhere in the file (the `#[cfg(test)]` module uses both `use` forms and fully-qualified paths) to `crate::profile::…`, leaving the parser types alone:

Run: `sd 'crate::sfdp::(EraseType|FlashProfile|ProfileSource)' 'crate::profile::$1' src/flash.rs`
Then confirm nothing model-related still points at `sfdp`:
Run: `rg -n 'crate::sfdp::(EraseType|FlashProfile|ProfileSource)' src/flash.rs`
Expected: no matches. (`crate::sfdp::{Bfpt, ParamHeader, SfdpHeader}` remain and are correct.)

(`lookup_fallback` moves to `catalog` in Task 0.2; if Step 4 runs before 0.2, temporarily keep `use crate::sfdp::lookup_fallback;` and fix it in 0.2 Step 2. To avoid churn, do Task 0.2 immediately after.)

- [ ] **Step 5: Fix `main.rs` references to the moved model.** `main.rs` names `sfdp::FlashProfile` and `sfdp::ProfileSource`; both now live in `profile`. Change the `print_profile` signature:

```rust
fn print_profile(p: &profile::FlashProfile) {
```

and in `Cmd::Info` change the SFDP-source comparison:

```rust
                        println!("SFDP:     {}",
                            if p.source == profile::ProfileSource::Sfdp { "present" } else { "—" });
```

(`sfdp::SfdpHeader` in `Cmd::Sfdp`/`Cmd::Doctor` is unchanged — the header parser stays in `sfdp.rs`.)

- [ ] **Step 6: Verify + commit.**

Run: `cargo test`
Expected: all tests pass (the three planner tests now run from `profile::tests`).

```bash
cargo fmt
git add -A && git commit -m "refactor: split flash-profile model out of sfdp into profile.rs"
```

## Task 0.2: Move the fallback table into `catalog.rs`

**Files:**
- Modify: `src/sfdp.rs` (remove `KnownChip`/`FALLBACK_TABLE`/`lookup_fallback` + their test)
- Modify: `src/catalog.rs` (receive them)
- Modify: `src/flash.rs` (import `lookup_fallback` from `catalog`)
- Modify: `src/main.rs` (`Cmd::List` iterates `catalog::FALLBACK_TABLE`)

- [ ] **Step 1: Append to `src/catalog.rs`** the fallback table moved verbatim from `sfdp.rs`, with imports of the model types. Add at the top of `catalog.rs`:

```rust
use crate::profile::{EraseType, FlashProfile, ProfileSource};
```

and at the bottom:

```rust
/// A known SPI-NOR part that lacks SFDP, described from its datasheet.
pub struct KnownChip {
    pub jedec: [u8; 3],
    #[allow(dead_code)] // reserved for detect logging; not read yet
    pub name: &'static str,
    pub page_size: usize,
    pub address_bytes: u8,
    pub capacity: usize,
    pub erase_types: &'static [EraseType],
}

/// Fallback table — parts we support that don't self-describe via SFDP.
pub static FALLBACK_TABLE: &[KnownChip] = &[KnownChip {
    jedec: [0x20, 0x20, 0x15],
    name: "Micron/Numonyx M25P16",
    page_size: 256,
    address_bytes: 3,
    capacity: 2 * 1024 * 1024,
    erase_types: &[EraseType {
        size: 64 * 1024,
        opcode: 0xD8,
    }],
}];

/// Build a `FlashProfile` for a chip in the fallback table, else `None`.
pub fn lookup_fallback(jedec: [u8; 3]) -> Option<FlashProfile> {
    FALLBACK_TABLE
        .iter()
        .find(|c| c.jedec == jedec)
        .map(|c| FlashProfile {
            page_size: c.page_size,
            address_bytes: c.address_bytes,
            capacity: Some(c.capacity),
            erase_types: c.erase_types.to_vec(),
            source: ProfileSource::Table,
        })
}
```

Move the `m25p16_is_in_the_fallback_table` test from `sfdp.rs` into `catalog.rs`'s `#[cfg(test)] mod tests` (it references `lookup_fallback`, `ProfileSource`, `EraseType` — add `use crate::profile::{EraseType, ProfileSource};` inside the test module).

- [ ] **Step 2: Remove from `src/sfdp.rs`** the `KnownChip` struct, `FALLBACK_TABLE`, `lookup_fallback`, and the moved test. Ensure `flash.rs`'s import reads `use crate::catalog::lookup_fallback;` (from Task 0.1 Step 4).

- [ ] **Step 3: Fix `Cmd::List` in `main.rs`.** It currently iterates `sfdp::FALLBACK_TABLE`; change to `catalog::FALLBACK_TABLE`:

```rust
Cmd::List => {
    for c in catalog::FALLBACK_TABLE {
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
```

- [ ] **Step 4: Verify + commit.**

Run: `cargo test`
Expected: all pass. Run `cargo clippy` to confirm no unused-import warnings.

```bash
cargo fmt
git add -A && git commit -m "refactor: move no-SFDP fallback table into catalog.rs"
```

---

# Phase 1 — Dead-code purge & structural simplification (wire-neutral)

Removes the speculative "library-convenience" surface, unifies the CLI hold enums, and drops the leaky `_hal` field. All CLI behavior identical.

## Task 1.1: Remove `flash_bitstream`, `Progress`, and `TooLarge`

**Files:**
- Modify: `src/flash.rs` (delete the items + their 3 tests)

- [ ] **Step 1: Delete the `Progress` enum** (`flash.rs` ~lines 111-120), the `FlashError::TooLarge` variant (~lines 133-138) and its `Display` arm (~lines 163-165), and the entire `flash_bitstream` method (~lines 594-639).

- [ ] **Step 2: Delete the three tests** that exercise them: `flash_bitstream_end_to_end`, `flash_bitstream_rejects_oversize`, and `flash_bitstream_detects_first`.

- [ ] **Step 3: Verify + commit.**

Run: `cargo test`
Expected: passes; test count drops by 3 (34 → 31). No reference to `Progress`/`TooLarge`/`flash_bitstream` remains — confirm with:

Run: `rg -n 'flash_bitstream|Progress|TooLarge' src/`
Expected: no matches.

```bash
cargo fmt
git add -A && git commit -m "refactor(flash): drop dead flash_bitstream/Progress/TooLarge"
```

## Task 1.2: Remove `NoHold` and `Flasher::new`; gate `BLOCK_SIZE` to tests

**Files:**
- Modify: `src/flash.rs`

- [ ] **Step 1: Delete `NoHold`** (the struct + its `BusAccess` impl, ~lines 98-109) and the unused `Flasher::new` constructor (~lines 192-202).

- [ ] **Step 2: Move `BLOCK_SIZE`** into the test module. Delete the top-level `pub const BLOCK_SIZE` (with its `#[allow(dead_code)]`) and add, at the top of `flash.rs`'s `#[cfg(test)] mod tests`:

```rust
/// 64 KiB erase block (test reference constant; production geometry comes from FlashProfile).
const BLOCK_SIZE: usize = 64 * 1024;
```

- [ ] **Step 3: Verify + commit.**

Run: `cargo test`
Expected: pass (tests still reference `BLOCK_SIZE` from the test module). 

Run: `rg -n 'NoHold|fn new\(' src/flash.rs`
Expected: no `NoHold`; no `Flasher::new`.

```bash
cargo fmt
git add -A && git commit -m "refactor(flash): drop NoHold + Flasher::new; scope BLOCK_SIZE to tests"
```

## Task 1.3: Fold `Level`/`Release` into clap; make the hold non-optional

**Files:**
- Modify: `src/device.rs` (derive `ValueEnum`; non-optional `HostBus.gpio`)
- Modify: `src/main.rs` (delete `ActiveArg`/`ReleaseArg`; `hold()` returns `HoldConfig`)

- [ ] **Step 1: In `src/device.rs`,** derive clap `ValueEnum` on `Level`/`Release` and make `HostBus.gpio` non-optional. Replace the enum definitions and `HostBus` struct/impl:

```rust
use clap::ValueEnum;

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
        self.gpio.set_config(GpioDirection::Output, GpioPull::None)?;
        match self.active {
            Level::Low => self.gpio.set_low(),
            Level::High => self.gpio.set_high(),
        }
    }

    fn release(&mut self) -> Result<(), Self::Error> {
        match self.release {
            Release::HiZ => self.gpio.set_config(GpioDirection::Input, GpioPull::None),
            Release::DriveHigh => {
                self.gpio.set_config(GpioDirection::Output, GpioPull::None)?;
                self.gpio.set_high()
            }
            Release::DriveLow => {
                self.gpio.set_config(GpioDirection::Output, GpioPull::None)?;
                self.gpio.set_low()
            }
        }
    }
}
```

- [ ] **Step 2: Simplify `connect()` in `device.rs`** to take a non-optional `HoldConfig`, drop the HAL internally, and return `Connected { spi, bus }`. Replace `Connected` and `connect`:

```rust
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
    assert_ne!(cs_pin, hold.pin, "CS and hold GPIO must differ");
    let mut hal = match serial {
        Some(sn) => Hal::new_validated_with_serial_number(sn),
        None => Hal::new_validated(),
    }
    .map_err(|e| anyhow!("connect/validate failed (device attached? firmware current?): {e:?}"))?;

    let _ = hal.system_reset_subscriptions();

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
```

Remove the now-unused `Hal` import only if nothing else uses it (it is used above via `Hal::new_validated`; keep the import).

- [ ] **Step 3: Update `main.rs`.** Delete the `ActiveArg` and `ReleaseArg` enums entirely. Change the `Cli` fields to use `device::{Level, Release}` and simplify `hold()`:

In the `Cli` struct, the two hold-arg fields become:

```rust
    /// Level to hold the bus GPIO at.
    #[arg(long, global = true, value_enum, default_value_t = Level::Low)]
    hold_active: Level,
    /// What to do with the bus GPIO on release.
    #[arg(long, global = true, value_enum, default_value_t = Release::HiZ)]
    hold_release: Release,
```

Update the `use device::{...}` line to `use device::{HoldConfig, Level, Release};` and replace `impl Cli`'s `hold`:

```rust
impl Cli {
    /// Build the bus-hold config from the flags (hold GPIO defaults to User GPIO 1).
    fn hold(&self) -> HoldConfig {
        HoldConfig {
            pin: self.hold_gpio,
            active: self.hold_active,
            release: self.hold_release,
        }
    }
}
```

Update the two `build_flasher*` call sites that destructure `Connected` and call `connect`:

```rust
    let device::Connected { spi, bus } = device::connect(cli.serial.as_deref(), freq, cli.cs, cli.hold())?;
```

(Delete the `drop(_hal);` line and the `_hal` binding.)

- [ ] **Step 4: Verify + commit.**

Run: `cargo test && cargo run -- --help`
Expected: tests pass; `--help` still shows `--hold-active <high|low>` and `--hold-release <drive-high|drive-low|hi-z>` exactly as before.

Run: `rg -n 'ActiveArg|ReleaseArg|_hal|Option<HoldConfig>' src/`
Expected: no matches.

```bash
cargo fmt
git add -A && git commit -m "refactor: unify Level/Release into clap; non-optional hold; drop _hal"
```

---

# Phase 2 — Type newtypes (`AddressWidth`, `EraseMenu`, SFDP revision)

Replaces the two comment-only invariants (`address_bytes: u8 // 3 or 4`,
`erase_types: Vec // non-empty, sorted`) with enforcing types, and surfaces the
already-decoded SFDP revision. `ErasePlan`/plan-execute is deferred to Phase 4,
where the progress UI consumes it (avoids migrating the planner tests twice).

Because these change `FlashProfile`'s fields, every `FlashProfile` literal in the
crate updates in lock-step; the whole task lands in one green commit.

## Task 2.1: Introduce `AddressWidth` + `EraseMenu`; modernize `FlashProfile`

**Files:**
- Modify: `src/profile.rs` (new types + fields + planner + tests)
- Modify: `src/sfdp.rs` (`Bfpt.address_width`; un-`allow` `major`/`minor`)
- Modify: `src/catalog.rs` (`lookup_fallback` builds the new shape)
- Modify: `src/flash.rs` (thread `AddressWidth`; build the new profile; tests)
- Modify: `src/main.rs` (`print_profile` reads the new fields)

- [ ] **Step 1: Rewrite `src/profile.rs`** to define the new types, the modernized `FlashProfile`, the planner, and unit tests for the new types:

```rust
//! Flash geometry model + the pure erase planner. No I/O, no SFDP bytes.

use core::fmt;

/// Number of address bytes a part expects in read/erase/program headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressWidth {
    Three,
    Four,
}

impl AddressWidth {
    /// Header address length in bytes.
    pub fn bytes(self) -> u8 {
        match self {
            AddressWidth::Three => 3,
            AddressWidth::Four => 4,
        }
    }
    pub fn is_four_byte(self) -> bool {
        matches!(self, AddressWidth::Four)
    }
}

impl fmt::Display for AddressWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-byte", self.bytes())
    }
}

/// One supported erase granularity: `size` bytes via `opcode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseType {
    pub size: usize,
    pub opcode: u8,
}

/// A non-empty erase menu, sorted largest-size first, with unique sizes.
///
/// The invariant is upheld by `new`, so `largest`/`smallest` are total and the
/// planner never has to guess a fallback granularity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraseMenu(Vec<EraseType>);

impl EraseMenu {
    /// Build a menu, sorting largest-first and dropping duplicate sizes.
    /// `None` for empty input — a chip with no usable menu is rejected at
    /// detection, so a `FlashProfile` always holds a real menu.
    pub fn new(mut types: Vec<EraseType>) -> Option<Self> {
        if types.is_empty() {
            return None;
        }
        types.sort_by_key(|e| std::cmp::Reverse(e.size));
        types.dedup_by_key(|e| e.size);
        Some(EraseMenu(types))
    }

    /// Largest granularity (first, by the sort invariant).
    pub fn largest(&self) -> EraseType {
        self.0[0]
    }
    /// Smallest granularity (last, by the sort invariant).
    pub fn smallest(&self) -> EraseType {
        self.0[self.0.len() - 1]
    }
    pub fn iter(&self) -> impl Iterator<Item = EraseType> + '_ {
        self.0.iter().copied()
    }
}

/// How a `FlashProfile` was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Sfdp,
    Table,
}

/// How to talk to the flash: geometry + erase menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashProfile {
    pub page_size: usize,
    pub address_width: AddressWidth,
    pub capacity: Option<usize>, // bytes, if known
    pub erase: EraseMenu,
    pub source: ProfileSource,
    pub sfdp_revision: Option<(u8, u8)>, // (major, minor) when source == Sfdp
}

impl FlashProfile {
    pub fn min_erase(&self) -> usize {
        self.erase.smallest().size
    }
}

/// Plan a minimal-ish erase covering `[offset, offset+len)`. Greedily takes the
/// largest address-aligned erase type that does not overshoot past the region
/// (rounded up to the smallest granularity), else the smallest. `(addr, opcode)`.
pub fn plan_erase(profile: &FlashProfile, offset: usize, len: usize) -> Vec<(usize, u8)> {
    let mut plan = Vec::new();
    if len == 0 {
        return plan;
    }
    let min = profile.min_erase();
    let smallest = profile.erase.smallest();

    let end = offset + len;
    let end_aligned = end.div_ceil(min) * min;
    let mut a = offset - offset % min;
    while a < end {
        let choice = profile
            .erase
            .iter()
            .find(|e| a.is_multiple_of(e.size) && a + e.size <= end_aligned)
            .unwrap_or(smallest);
        plan.push((a, choice.opcode));
        a += choice.size;
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(sizes: &[(usize, u8)]) -> FlashProfile {
        FlashProfile {
            page_size: 256,
            address_width: AddressWidth::Three,
            capacity: Some(2 * 1024 * 1024),
            erase: EraseMenu::new(
                sizes
                    .iter()
                    .map(|(s, o)| EraseType {
                        size: *s,
                        opcode: *o,
                    })
                    .collect(),
            )
            .expect("test menu is non-empty"),
            source: ProfileSource::Sfdp,
            sfdp_revision: None,
        }
    }

    #[test]
    fn address_width_bytes_and_display() {
        assert_eq!(AddressWidth::Three.bytes(), 3);
        assert_eq!(AddressWidth::Four.bytes(), 4);
        assert!(AddressWidth::Four.is_four_byte());
        assert!(!AddressWidth::Three.is_four_byte());
        assert_eq!(AddressWidth::Three.to_string(), "3-byte");
    }

    #[test]
    fn erase_menu_rejects_empty() {
        assert!(EraseMenu::new(vec![]).is_none());
    }

    #[test]
    fn erase_menu_sorts_desc_and_dedups() {
        let m = EraseMenu::new(vec![
            EraseType { size: 4096, opcode: 0x20 },
            EraseType { size: 65536, opcode: 0xD8 },
            EraseType { size: 4096, opcode: 0x20 },
        ])
        .unwrap();
        assert_eq!(m.largest().size, 65536);
        assert_eq!(m.smallest().size, 4096);
        assert_eq!(m.iter().count(), 2); // duplicate 4096 dropped
    }

    #[test]
    fn single_granularity_plan_is_64k_blocks() {
        let p = profile_with(&[(64 * 1024, 0xD8)]);
        assert_eq!(
            plan_erase(&p, 0, 135_100),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0xD8)]
        );
    }

    #[test]
    fn mixed_granularity_uses_small_sector_at_tail() {
        let p = profile_with(&[(4 * 1024, 0x20), (32 * 1024, 0x52), (64 * 1024, 0xD8)]);
        assert_eq!(
            plan_erase(&p, 0, 131_072 + 100),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0x20)]
        );
    }

    #[test]
    fn zero_len_is_empty() {
        assert!(plan_erase(&profile_with(&[(64 * 1024, 0xD8)]), 0, 0).is_empty());
    }
}
```

- [ ] **Step 2: Update `Bfpt` in `src/sfdp.rs`** to decode an `AddressWidth` and stop hiding the header revision. Add `AddressWidth` to the profile import (top of `sfdp.rs`):

```rust
use crate::profile::{AddressWidth, EraseType};
```

In `SfdpHeader`, delete the two `#[allow(dead_code)]` attributes on `major` and `minor` (they are read now). In `Bfpt`, change the field and its decode:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bfpt {
    pub address_width: AddressWidth,
    pub page_size: usize,
    pub capacity: Option<usize>,
    pub erase_types: Vec<EraseType>,
}
```

and in `Bfpt::parse` replace the `address_bytes` line:

```rust
        let address_width = if (d1 >> 17) & 0b11 == 2 {
            AddressWidth::Four
        } else {
            AddressWidth::Three
        };
```

and the returned struct's first field `address_bytes,` → `address_width,`. In the `bfpt_decodes_geometry_and_erase_menu` test, change `assert_eq!(bfpt.address_bytes, 3);` to `assert_eq!(bfpt.address_width, AddressWidth::Three);` and add `use crate::profile::AddressWidth;` inside that test module if not already imported via `super::*` (it is, since `Bfpt` re-exports through `super`; add the explicit `use crate::profile::AddressWidth;` to the `mod tests` block to be safe).

- [ ] **Step 3: Update `lookup_fallback` in `src/catalog.rs`** to build the new profile shape. Replace the mapping closure:

```rust
pub fn lookup_fallback(jedec: [u8; 3]) -> Option<FlashProfile> {
    FALLBACK_TABLE
        .iter()
        .find(|c| c.jedec == jedec)
        .map(|c| FlashProfile {
            page_size: c.page_size,
            address_width: if c.address_bytes == 4 {
                AddressWidth::Four
            } else {
                AddressWidth::Three
            },
            capacity: Some(c.capacity),
            erase: EraseMenu::new(c.erase_types.to_vec())
                .expect("fallback-table entries have a non-empty erase menu"),
            source: ProfileSource::Table,
            sfdp_revision: None,
        })
}
```

Update the top import to `use crate::profile::{AddressWidth, EraseMenu, EraseType, FlashProfile, ProfileSource};`. In the `m25p16_is_in_the_fallback_table` test, change `assert_eq!(p.address_bytes, 3);` → `assert_eq!(p.address_width, AddressWidth::Three);` and replace the `p.erase_types` assertion with:

```rust
        assert_eq!(
            p.erase.iter().collect::<Vec<_>>(),
            vec![EraseType { size: 64 * 1024, opcode: 0xD8 }]
        );
```

Add `use crate::profile::AddressWidth;` to that test module.

- [ ] **Step 4: Thread `AddressWidth` through `src/flash.rs`.** Update the profile import and the four address-emitting sites.

Top import:

```rust
use crate::profile::{AddressWidth, EraseMenu, FlashProfile, ProfileSource, plan_erase};
```

`push_cmd_addr`:

```rust
fn push_cmd_addr(cmd: &mut Vec<u8>, opcode: u8, addr: u32, width: AddressWidth) {
    cmd.push(opcode);
    if width.is_four_byte() {
        cmd.push((addr >> 24) as u8);
    }
    cmd.push((addr >> 16) as u8);
    cmd.push((addr >> 8) as u8);
    cmd.push(addr as u8);
}
```

In `erase_op`, `page_program`, and `read`, replace the `let ab = self.require_profile()?.address_bytes;` + `Vec::with_capacity(1 + ab as usize)` + `push_cmd_addr(&mut header, …, ab)` trio with:

```rust
        let width = self.require_profile()?.address_width;
        // …
        let mut header = Vec::with_capacity(1 + width.bytes() as usize);
        push_cmd_addr(&mut header, /* opcode */, addr, width);
```

(Keep each method's own opcode: `opcode` in `erase_op`, `CMD_PP` in `page_program`, `CMD_READ` in `read`.)

In `adopt_profile`, change the 4-byte check:

```rust
        if profile.address_width.is_four_byte() {
            self.enter_4byte().await?;
        }
```

In `try_sfdp_profile`, replace the erase-empty guard and the returned literal:

```rust
        let bfpt = Bfpt::parse(&bytes);
        let Some(erase) = EraseMenu::new(bfpt.erase_types) else {
            return Ok(None); // SFDP present but no usable erase menu → fall through to the table
        };
        Ok(Some(FlashProfile {
            page_size: bfpt.page_size,
            address_width: bfpt.address_width,
            capacity: bfpt.capacity.or(id.capacity_bytes()),
            erase,
            source: ProfileSource::Sfdp,
            sfdp_revision: Some((h.major, h.minor)),
        }))
```

- [ ] **Step 5: Update `flash.rs` test literals.** Four tests build or assert `FlashProfile`:

`flasher()` helper — the default profile becomes:

```rust
    f.set_profile(crate::profile::FlashProfile {
        page_size: 256,
        address_width: crate::profile::AddressWidth::Three,
        capacity: Some(2 * 1024 * 1024),
        erase: crate::profile::EraseMenu::new(vec![crate::profile::EraseType {
            size: 64 * 1024,
            opcode: 0xD8,
        }])
        .unwrap(),
        source: crate::profile::ProfileSource::Table,
        sfdp_revision: None,
    });
```

`erase_uses_profile_small_sector_at_tail` — its `use` becomes `use crate::profile::{AddressWidth, EraseMenu, EraseType, FlashProfile, ProfileSource};` and the profile literal:

```rust
        f.set_profile(FlashProfile {
            page_size: 256,
            address_width: AddressWidth::Three,
            capacity: Some(size),
            erase: EraseMenu::new(vec![
                EraseType { size: 64 * 1024, opcode: 0xD8 },
                EraseType { size: 4 * 1024, opcode: 0x20 },
            ])
            .unwrap(),
            source: ProfileSource::Sfdp,
            sfdp_revision: None,
        });
```

`four_byte_addressing_roundtrips` — its `use` becomes the same, and the literal uses `address_width: AddressWidth::Four`, `erase: EraseMenu::new(vec![EraseType { size: 64 * 1024, opcode: 0xD8 }]).unwrap()`, `source: ProfileSource::Sfdp`, `sfdp_revision: None`.

`detect_via_sfdp_builds_profile` — replace the erase assertions:

```rust
        assert_eq!(p.erase.iter().count(), 3);
        assert_eq!(p.erase.largest().size, 64 * 1024); // largest first
        assert_eq!(p.sfdp_revision, Some((1, 6))); // "SFDP" rev 1.6 from the header
```

`detect_uses_fallback_table_for_m25p16` — replace the `p.erase_types` assertion with:

```rust
        assert_eq!(
            p.erase.iter().collect::<Vec<_>>(),
            vec![EraseType { size: 64 * 1024, opcode: 0xD8 }]
        );
```

- [ ] **Step 6: Update `print_profile` in `main.rs`** to read the new fields:

```rust
fn print_profile(p: &profile::FlashProfile) {
    println!("source:   {:?}", p.source);
    println!("page:     {} B", p.page_size);
    println!("address:  {}", p.address_width);
    match p.capacity {
        Some(c) => println!("capacity: {} KiB", c / 1024),
        None => println!("capacity: unknown"),
    }
    println!("erase types:");
    for e in p.erase.iter() {
        println!("  {:>7} B  op 0x{:02X}", e.size, e.opcode);
    }
}
```

- [ ] **Step 7: Verify + commit.**

Run: `cargo test`
Expected: all pass (31 + 3 new type tests = 34). Confirm the invariants are gone:

Run: `rg -n 'address_bytes|erase_types|unwrap_or\(64 \* 1024\)' src/flash.rs src/profile.rs`
Expected: no matches in `profile.rs`/`flash.rs` (the field `c.address_bytes` remains only in `catalog.rs`'s `KnownChip` table row, which is fine).

```bash
cargo fmt
git add -A && git commit -m "refactor: AddressWidth + EraseMenu newtypes; surface SFDP revision"
```

---

# Phase 3 — Presentation foundation (`error`, `ui`, voice)

Creates the modules the handlers will depend on. Nothing is wired into `main.rs`
yet, so the new modules carry a **temporary** `#![allow(dead_code)]` that Phase 5's
cleanup task removes once every method is in use. The crate-wide `deny` lint is
also added in Phase 5 (once the last `println!` is gone).

## Task 3.1: `error.rs` — the one application error

**Files:**
- Create: `src/error.rs`
- Modify: `src/main.rs` (add `mod error;`)

- [ ] **Step 1: Create `src/error.rs`:**

```rust
//! The single application-level error. `flash::FlashError` stays generic in the
//! core; this non-generic type is what handlers return and `ui` renders.
#![allow(dead_code)] // wired up in Phase 5; the lint is added there too

use crate::flash::FlashError;
use core::fmt;

#[derive(Debug)]
pub enum NorbertError {
    NoFlash,
    Unsupported([u8; 3]),
    VerifyMismatch { addr: usize },
    Protected,
    Timeout,
    NotDetected,
    Cancelled,
    Other(anyhow::Error),
}

impl<S: fmt::Debug, R: fmt::Debug> From<FlashError<S, R>> for NorbertError {
    fn from(e: FlashError<S, R>) -> Self {
        match e {
            FlashError::NoFlash => NorbertError::NoFlash,
            FlashError::UnsupportedChip { jedec } => NorbertError::Unsupported(jedec),
            FlashError::VerifyMismatch { addr, .. } => NorbertError::VerifyMismatch { addr },
            FlashError::Timeout => NorbertError::Timeout,
            FlashError::NotDetected => NorbertError::NotDetected,
            other => NorbertError::Other(anyhow::anyhow!("{other}")),
        }
    }
}

impl From<anyhow::Error> for NorbertError {
    fn from(e: anyhow::Error) -> Self {
        NorbertError::Other(e)
    }
}

impl From<std::io::Error> for NorbertError {
    fn from(e: std::io::Error) -> Self {
        NorbertError::Other(e.into())
    }
}

impl fmt::Display for NorbertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NorbertError::NoFlash => write!(f, "no SPI-NOR flash detected"),
            NorbertError::Unsupported(j) => write!(f, "unsupported flash {j:02X?}"),
            NorbertError::VerifyMismatch { addr } => {
                write!(f, "verify mismatch at 0x{addr:06X}")
            }
            NorbertError::Protected => write!(f, "write protection enabled"),
            NorbertError::Timeout => write!(f, "timed out waiting for the flash"),
            NorbertError::NotDetected => write!(f, "flash geometry unknown; run detect first"),
            NorbertError::Cancelled => write!(f, "cancelled"),
            NorbertError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for NorbertError {}
```

- [ ] **Step 2: Register the module** — add `mod error;` to the `mod` block in `main.rs`.

- [ ] **Step 3: Verify + commit.**

Run: `cargo test`
Expected: builds and passes (no warnings — the `allow` covers the not-yet-used type).

```bash
cargo fmt
git add -A && git commit -m "feat(error): add NorbertError app-error boundary"
```

## Task 3.2: `voice.rs` — expand to cover every command

**Files:**
- Modify: `src/voice.rs`

- [ ] **Step 1: Add the new builders** to `voice.rs`. First add a temporary crate-lint escape at the very top of the file (the new builders are unused until Phase 5; a binary crate flags unused `pub fn`s — Task 5.6 removes this line):

```rust
#![allow(dead_code)] // new builders are wired up in Phase 5
```

Then append the builders before the `#[cfg(test)]` block. All obey the rules: no exclamation points, dry, every failure carries a fact.

```rust
pub fn info_opener() -> &'static str {
    "Let me see what this one says about itself."
}
pub fn info_sfdp_note(present: bool) -> &'static str {
    if present {
        "It told me all that itself. I appreciate a chip that keeps notes."
    } else {
        "It stayed quiet on the details, so I filled in from memory."
    }
}
pub fn sfdp_opener() -> &'static str {
    "Here's what the chip told me, byte for byte."
}
pub fn no_sfdp() -> &'static str {
    "This one has no SFDP to show.\n\nI'll rely on what I already know."
}
pub fn list_opener() -> &'static str {
    "The parts I keep notes on, in case they don't speak SFDP:"
}
pub fn list_note() -> &'static str {
    "Anything with valid SFDP, I can work out on my own."
}
pub fn read_done(bytes: usize, path: &std::path::Path) -> String {
    format!("Done. {bytes} bytes, saved to {}.", path.display())
}
pub fn programming_intro(name: &str, size: &str, offset: usize) -> String {
    format!("Programming {name} — {size} at 0x{offset:06X}.")
}
pub fn program_summary(blocks: usize, size: &str, secs: f64) -> String {
    format!("Done. Have a nice boot.  (erased {blocks} blocks, wrote {size} in {secs:.1}s)")
}
pub fn doctor_intro() -> &'static str {
    "Let's have a look. I'll take my time."
}
pub fn timeout() -> &'static str {
    "The chip stopped answering.\n\nI waited as long as I reasonably could."
}
pub fn not_detected() -> &'static str {
    "I haven't identified this chip yet.\n\nRun detect first."
}
```

- [ ] **Step 2: Extend `norbert_never_shouts`** — add the new lines to the `lines` array in the test so the no-exclamation rule covers them:

```rust
            info_opener().to_string(),
            info_sfdp_note(true).to_string(),
            info_sfdp_note(false).to_string(),
            sfdp_opener().to_string(),
            no_sfdp().to_string(),
            list_opener().to_string(),
            list_note().to_string(),
            read_done(4096, std::path::Path::new("dump.bin")),
            programming_intro("Winbond W25Q128JV", "512 KiB", 0),
            program_summary(3, "512 KiB", 4.2),
            doctor_intro().to_string(),
            timeout().to_string(),
            not_detected().to_string(),
```

- [ ] **Step 3: Verify + commit.**

Run: `cargo test voice`
Expected: `norbert_never_shouts` and `failures_carry_the_fact` pass with the new lines.

```bash
cargo fmt
git add -A && git commit -m "feat(voice): add lines for info/sfdp/list/read/program/doctor/errors"
```

## Task 3.3: `ui` module — the presenter

**Files:**
- Create: `src/ui/mod.rs`
- Create: `src/testsupport.rs`
- Modify: `src/main.rs` (add `mod ui;` and `#[cfg(test)] mod testsupport;`)

- [ ] **Step 1: Create `src/ui/mod.rs`:**

```rust
//! Norbert's presentation layer: the only module that writes to the terminal.
//! Handlers gather data and sequence these methods; personality text comes from
//! `voice`; this module owns layout and the Human/Machine decision.
#![allow(dead_code)] // fully wired in Phase 5, where the print lint is also added

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use crate::error::NorbertError;
use crate::voice;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Human,
    Machine,
}

/// One labeled field. `human`/`machine` differ where units do (e.g. capacity).
pub struct Row {
    pub key: &'static str,
    pub label: &'static str,
    pub human: String,
    pub machine: String,
}

impl Row {
    /// Same text in both modes (chip name, source, …).
    pub fn new(key: &'static str, label: &'static str, value: impl Into<String>) -> Row {
        let v = value.into();
        Row { key, label, human: v.clone(), machine: v }
    }
    /// Different text per mode (Human "16384 KiB" vs Machine "16777216").
    pub fn split(
        key: &'static str,
        label: &'static str,
        human: impl Into<String>,
        machine: impl Into<String>,
    ) -> Row {
        Row { key, label, human: human.into(), machine: machine.into() }
    }
}

pub struct Ui {
    mode: Mode,
    out: Box<dyn Write>,
    err: Box<dyn Write>,
}

impl Ui {
    /// Machine mode when `--quiet` or stdout is not a terminal.
    pub fn from_cli(quiet: bool) -> Ui {
        let mode = if quiet || !std::io::stdout().is_terminal() {
            Mode::Machine
        } else {
            Mode::Human
        };
        Ui {
            mode,
            out: Box::new(std::io::stdout()),
            err: Box::new(std::io::stderr()),
        }
    }

    /// Test/injection constructor.
    #[cfg(test)]
    pub fn from_parts(mode: Mode, out: Box<dyn Write>, err: Box<dyn Write>) -> Ui {
        Ui { mode, out, err }
    }

    /// A voice aside (opener/closer/note). Human only.
    pub fn say(&mut self, line: &str) {
        if self.mode == Mode::Human {
            let _ = writeln!(self.out, "{line}");
        }
    }

    /// A terminal outcome: voice for humans, a stable token for scripts.
    pub fn line(&mut self, human: &str, machine: &str) {
        let _ = match self.mode {
            Mode::Human => writeln!(self.out, "{human}"),
            Mode::Machine => writeln!(self.out, "{machine}"),
        };
    }

    /// A labeled data block. Human aligns `label:` columns; Machine emits `key=value`.
    pub fn rows(&mut self, rows: &[Row]) {
        match self.mode {
            Mode::Human => {
                let w = rows.iter().map(|r| r.label.len()).max().unwrap_or(0) + 1;
                for r in rows {
                    let label = format!("{}:", r.label);
                    let _ = writeln!(self.out, "{label:<w$} {}", r.human, w = w);
                }
            }
            Mode::Machine => {
                for r in rows {
                    let _ = writeln!(self.out, "{}={}", r.key, r.machine);
                }
            }
        }
    }

    /// A hex dump (raw SFDP). Same 16-per-row layout in both modes.
    pub fn hexdump(&mut self, base: usize, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            let _ = writeln!(self.out, "  {:04X}: {}", base + i * 16, hex.join(" "));
        }
    }

    /// Render a failure to stderr; return the process exit code.
    pub fn fail(&mut self, e: &NorbertError) -> ExitCode {
        let (human, machine, code): (String, String, u8) = match e {
            NorbertError::NoFlash => (voice::no_flash().to_string(), "FAIL: no chip".into(), 1),
            NorbertError::Unsupported(j) => (
                voice::unsupported(*j),
                format!("FAIL: unsupported {:02X} {:02X} {:02X}", j[0], j[1], j[2]),
                1,
            ),
            NorbertError::VerifyMismatch { addr } => (
                voice::verify_fail(*addr),
                format!("FAIL: verify @0x{addr:06X}"),
                1,
            ),
            NorbertError::Protected => {
                (voice::protected().to_string(), "FAIL: protected".into(), 1)
            }
            NorbertError::Timeout => (voice::timeout().to_string(), "FAIL: timeout".into(), 1),
            NorbertError::NotDetected => {
                (voice::not_detected().to_string(), "FAIL: not detected".into(), 1)
            }
            NorbertError::Cancelled => {
                (voice::cancelled().to_string(), "FAIL: cancelled".into(), 130)
            }
            NorbertError::Other(err) => (format!("{err:#}"), format!("FAIL: {err}"), 1),
        };
        let _ = match self.mode {
            Mode::Human => writeln!(self.err, "{human}"),
            Mode::Machine => writeln!(self.err, "{machine}"),
        };
        ExitCode::from(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_dropped_in_machine_but_shown_in_human() {
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        ui.say("Hmm.");
        assert_eq!(out.contents(), "");
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        ui.say("Hmm.");
        assert_eq!(out.contents(), "Hmm.\n");
    }

    #[test]
    fn line_picks_the_right_channel() {
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        ui.line("Found it.", "EF4018");
        assert_eq!(out.contents(), "EF4018\n");
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        ui.line("Found it.", "EF4018");
        assert_eq!(out.contents(), "Found it.\n");
    }

    #[test]
    fn rows_align_in_human_and_kv_in_machine() {
        let rows = [
            Row::split("capacity", "capacity", "16384 KiB", "16777216"),
            Row::split("page", "page", "256 B", "256"),
        ];
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        ui.rows(&rows);
        assert_eq!(out.contents(), "capacity=16777216\npage=256\n");
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        ui.rows(&rows);
        assert_eq!(out.contents(), "capacity: 16384 KiB\npage:     256 B\n");
    }

    #[test]
    fn fail_is_voice_in_human_and_token_in_machine() {
        let (mut ui, _o, err) = Ui::captured(Mode::Machine);
        let _ = ui.fail(&NorbertError::Protected);
        assert_eq!(err.contents(), "FAIL: protected\n");
        let (mut ui, _o, err) = Ui::captured(Mode::Human);
        let _ = ui.fail(&NorbertError::Protected);
        assert_eq!(err.contents(), format!("{}\n", voice::protected()));
    }
}
```

- [ ] **Step 2: Create `src/testsupport.rs`** with the captured-`Ui` sink (the `FakeFlash`/`FakeBus` promotion happens in Phase 5):

```rust
//! Shared test doubles.
#![cfg(test)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::ui::{Mode, Ui};

/// A `Write` sink that captures bytes for assertions.
#[derive(Clone, Default)]
pub struct Buf(Arc<Mutex<Vec<u8>>>);

impl Buf {
    pub fn contents(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Buf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Ui {
    /// A `Ui` in `mode` writing to captured buffers; returns `(ui, out, err)`.
    pub fn captured(mode: Mode) -> (Ui, Buf, Buf) {
        let out = Buf::default();
        let err = Buf::default();
        let ui = Ui::from_parts(mode, Box::new(out.clone()), Box::new(err.clone()));
        (ui, out, err)
    }
}
```

- [ ] **Step 3: Register the modules** in `main.rs`:

```rust
mod ui;
```

and, alongside the other `mod` lines, the test-only support module:

```rust
#[cfg(test)]
mod testsupport;
```

- [ ] **Step 4: Verify + commit.**

Run: `cargo test ui::`
Expected: the five `ui::tests` pass.

Run: `cargo test`
Expected: whole suite green.

```bash
cargo fmt
git add -A && git commit -m "feat(ui): presenter over injected sinks + captured test Ui"
```

---

# Phase 4 — Erase plan/execute split + progress infrastructure

Turns the erase from a blind spinner into a determinate view by splitting the pure
*plan* from the *execution*, then builds the restrained Unicode `Progress`. Both are
wire-neutral: the same erase ops run in the same order.

## Task 4.1: `ErasePlan` + plan/execute in `flash.rs`

**Files:**
- Modify: `src/profile.rs` (`EraseOp`, `ErasePlan`; `plan_erase` returns `ErasePlan`; planner tests)
- Modify: `src/flash.rs` (`erase_plan`, `run_erase`; `erase_range` becomes a wrapper)

- [ ] **Step 1: Add `EraseOp`/`ErasePlan` to `profile.rs`** (just above `plan_erase`):

```rust
/// One planned erase: which granularity to apply at which address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EraseOp {
    pub addr: usize,
    pub ty: EraseType,
}

/// A resolved erase plan: the ordered ops covering a region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasePlan {
    ops: Vec<EraseOp>,
}

impl ErasePlan {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
    /// Number of erase operations — the progress-bar length.
    pub fn blocks(&self) -> usize {
        self.ops.len()
    }
    /// Total bytes the plan erases.
    pub fn bytes(&self) -> usize {
        self.ops.iter().map(|o| o.ty.size).sum()
    }
    pub fn ops(&self) -> &[EraseOp] {
        &self.ops
    }
}
```

- [ ] **Step 2: Change `plan_erase` to return `ErasePlan`:**

```rust
pub fn plan_erase(profile: &FlashProfile, offset: usize, len: usize) -> ErasePlan {
    let mut ops = Vec::new();
    if len == 0 {
        return ErasePlan { ops };
    }
    let min = profile.min_erase();
    let smallest = profile.erase.smallest();

    let end = offset + len;
    let end_aligned = end.div_ceil(min) * min;
    let mut a = offset - offset % min;
    while a < end {
        let ty = profile
            .erase
            .iter()
            .find(|e| a.is_multiple_of(e.size) && a + e.size <= end_aligned)
            .unwrap_or(smallest);
        ops.push(EraseOp { addr: a, ty });
        a += ty.size;
    }
    ErasePlan { ops }
}
```

- [ ] **Step 3: Update the three planner tests** in `profile.rs` to read through `ops()`. Add a helper inside `mod tests` and adjust assertions:

```rust
    fn plan_addrs(p: &ErasePlan) -> Vec<(usize, u8)> {
        p.ops().iter().map(|o| (o.addr, o.ty.opcode)).collect()
    }

    #[test]
    fn single_granularity_plan_is_64k_blocks() {
        let p = profile_with(&[(64 * 1024, 0xD8)]);
        assert_eq!(
            plan_addrs(&plan_erase(&p, 0, 135_100)),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0xD8)]
        );
    }

    #[test]
    fn mixed_granularity_uses_small_sector_at_tail() {
        let p = profile_with(&[(4 * 1024, 0x20), (32 * 1024, 0x52), (64 * 1024, 0xD8)]);
        assert_eq!(
            plan_addrs(&plan_erase(&p, 0, 131_072 + 100)),
            vec![(0, 0xD8), (65_536, 0xD8), (131_072, 0x20)]
        );
    }

    #[test]
    fn zero_len_is_empty() {
        assert!(plan_erase(&profile_with(&[(64 * 1024, 0xD8)]), 0, 0).is_empty());
    }
```

- [ ] **Step 4: Split erase in `flash.rs`.** Add `ErasePlan` to the profile import:

```rust
use crate::profile::{AddressWidth, EraseMenu, ErasePlan, FlashProfile, ProfileSource, plan_erase};
```

Replace the existing `erase_range` method with plan + execute + wrapper:

```rust
    /// Compute the erase plan covering `[offset, offset+len)` from the detected profile.
    pub fn erase_plan(
        &self,
        offset: usize,
        len: usize,
    ) -> Result<ErasePlan, FlashError<SPI::Error, RST::Error>> {
        Ok(plan_erase(self.require_profile()?, offset, len))
    }

    /// Execute a precomputed plan, calling `progress(blocks_done)` after each op.
    pub async fn run_erase(
        &mut self,
        plan: &ErasePlan,
        mut progress: impl FnMut(usize),
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        for (i, op) in plan.ops().iter().enumerate() {
            self.erase_op(op.addr as u32, op.ty.opcode).await?;
            progress(i + 1);
        }
        Ok(())
    }

    /// Erase every block overlapping `[offset, offset+len)` (plan then execute).
    pub async fn erase_range(
        &mut self,
        offset: usize,
        len: usize,
    ) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let plan = self.erase_plan(offset, len)?;
        self.run_erase(&plan, |_| {}).await
    }
```

- [ ] **Step 5: Verify + commit.**

Run: `cargo test`
Expected: all pass (existing erase tests use `erase_range`, unchanged; planner tests use `plan_addrs`).

```bash
cargo fmt
git add -A && git commit -m "refactor(flash): split erase into plan (ErasePlan) + execute"
```

## Task 4.2: `ui/progress.rs` — the restrained Unicode view

**Files:**
- Create: `src/ui/progress.rs`
- Modify: `src/ui/mod.rs` (`pub mod progress;` + `Ui::progress`)

- [ ] **Step 1: Create `src/ui/progress.rs`:**

```rust
//! Restrained multi-phase progress: aligned Unicode bars for erase/program/verify.
//! Inert in Machine mode. Draws to stderr (indicatif's default), leaving stdout
//! (voice) clean.
#![allow(dead_code)] // wired by the write handlers in Phase 5

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use super::Mode;

/// Which phases a run will show, and their lengths.
pub struct ProgressPlan {
    pub erase_blocks: Option<u64>,
    pub program_bytes: Option<u64>,
    pub verify_bytes: Option<u64>,
}

pub struct Progress {
    erase: Option<ProgressBar>,
    program: Option<ProgressBar>,
    verify: Option<ProgressBar>,
    // Held only to keep the shared draw target alive; never read directly.
    #[allow(dead_code)]
    mp: Option<MultiProgress>,
}

fn bytes_bar(mp: &MultiProgress, label: &str, len: u64) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(len));
    pb.set_style(
        ProgressStyle::with_template(
            "  {prefix:<8} [{bar:26}] {bytes:>9}/{total_bytes:<9} {bytes_per_sec:>11}  {eta:>5}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb.set_prefix(label.to_string());
    pb
}

fn blocks_bar(mp: &MultiProgress, label: &str, blocks: u64) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(blocks));
    pb.set_style(
        ProgressStyle::with_template("  {prefix:<8} [{bar:26}] {pos:>4}/{len:<4} blocks")
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb.set_prefix(label.to_string());
    pb
}

impl Progress {
    /// Build a view for `plan`. In Machine mode everything is inert (no draw).
    pub fn new(mode: Mode, plan: ProgressPlan) -> Progress {
        if mode == Mode::Machine {
            return Progress {
                erase: None,
                program: None,
                verify: None,
                mp: None,
            };
        }
        let mp = MultiProgress::new();
        let erase = plan.erase_blocks.map(|n| blocks_bar(&mp, "erase", n));
        let program = plan.program_bytes.map(|n| bytes_bar(&mp, "program", n));
        let verify = plan.verify_bytes.map(|n| bytes_bar(&mp, "verify", n));
        Progress {
            erase,
            program,
            verify,
            mp: Some(mp),
        }
    }

    pub fn erase_to(&self, blocks: usize) {
        if let Some(b) = &self.erase {
            b.set_position(blocks as u64);
        }
    }
    pub fn program_to(&self, bytes: usize) {
        if let Some(b) = &self.program {
            b.set_position(bytes as u64);
        }
    }
    pub fn verify_to(&self, bytes: usize) {
        if let Some(b) = &self.verify {
            b.set_position(bytes as u64);
        }
    }

    /// Clear all bars so the summary voice line (stdout) prints cleanly.
    pub fn finish(self) {
        for b in [self.erase, self.program, self.verify]
            .into_iter()
            .flatten()
        {
            b.finish_and_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inert_in_machine_mode() {
        let p = Progress::new(
            Mode::Machine,
            ProgressPlan {
                erase_blocks: Some(3),
                program_bytes: Some(100),
                verify_bytes: None,
            },
        );
        p.erase_to(1);
        p.program_to(50);
        p.verify_to(0);
        p.finish(); // must not panic and must draw nothing
    }
}
```

- [ ] **Step 2: Wire `progress` into `ui/mod.rs`.** Add near the top (after the doc comment / `allow`):

```rust
pub mod progress;
```

and a constructor method on `Ui` (inside `impl Ui`):

```rust
    /// Build a progress view for this run (inert in Machine mode).
    pub fn progress(&self, plan: progress::ProgressPlan) -> progress::Progress {
        progress::Progress::new(self.mode, plan)
    }
```

- [ ] **Step 3: Verify + commit.**

Run: `cargo test ui::progress`
Expected: `inert_in_machine_mode` passes.

Run: `cargo build`
Expected: clean (indicatif already in `Cargo.toml`).

```bash
cargo fmt
git add -A && git commit -m "feat(ui): restrained Unicode MultiProgress (erase/program/verify)"
```

---

# Phase 5 — Command extraction, dispatch, and the print lint

Moves every command out of `main.rs` into `commands/` handlers that sequence `Ui`,
adds the `with_bus`/`with_cancel` bus-session helpers, then flips `main` to a thin
dispatcher and turns on `#![deny(clippy::print_stdout, clippy::print_stderr)]`.

Handlers are built (Tasks 5.3–5.5) while `main`'s old match still runs, so they
carry a **temporary** `#![allow(dead_code)]` removed in Task 5.6 when dispatch goes
live and the lint is added.

## Task 5.1: Promote the test fakes so command tests can use them

**Files:**
- Modify: `src/flash.rs` (wrap `FakeFlash`/`FakeBus`/`flasher` in a `pub(crate)` test submodule)

- [ ] **Step 1: Extract a `testfakes` submodule.** In `flash.rs`, move `FakeErr`, `FakeState`, `FakeFlash`, `FakeBus`, and the `flasher` helper **out** of `#[cfg(test)] mod tests` and into a new sibling module. A child module sees the parent's private items (`CMD_*`, `SR_WIP`, `PAGE_SIZE`, `Flasher`, …) through `use super::*`, so no constant needs to become `pub`:

```rust
#[cfg(test)]
pub(crate) mod testfakes {
    use super::*;
    use embedded_hal::spi::{Error as SpiErrorTrait, ErrorKind, ErrorType};
    use std::cell::RefCell;
    use std::convert::Infallible;
    use std::rc::Rc;

    // …move FakeErr, FakeState, FakeFlash (+ impls), FakeBus (+ impl), and
    //   `pub fn flasher(...)` here verbatim. Make `FakeFlash`, `FakeBus`, and
    //   `flasher` `pub(crate)`; keep their methods `pub(crate)` where the tests
    //   call them (new, set_busy_reads, set_sfdp, set_protected,
    //   set_powered_down, mem, preset, asserted).
}
```

- [ ] **Step 2: Point `mod tests` at the fakes.** At the top of `#[cfg(test)] mod tests` (which already has `use super::*;`), add:

```rust
    use super::testfakes::{FakeBus, FakeFlash, flasher};
```

Delete the now-moved definitions from `mod tests`. (`BLOCK_SIZE`, added to `mod tests` in Task 1.2, stays there.)

- [ ] **Step 3: Verify + commit.**

Run: `cargo test flash::`
Expected: the flash-core tests still pass (they now use the fakes from `testfakes`).

```bash
cargo fmt
git add -A && git commit -m "test(flash): expose FakeFlash/FakeBus as pub(crate) testfakes"
```

## Task 5.2: `cli.rs` + `commands/mod.rs` scaffolding

**Files:**
- Create: `src/cli.rs` (move `Cli`/`Cmd` out of `main.rs`)
- Create: `src/commands/mod.rs` (bus-session + flasher-build helpers; submodule decls)
- Modify: `src/main.rs` (register modules; use `cli::Cli`, `commands::build_flasher*`)

- [ ] **Step 1: Create `src/cli.rs`** with the clap types moved from `main.rs`, now using `device::{Level, Release}` directly:

```rust
//! Command-line surface: global flags + subcommands. Parsing only.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::device::{HoldConfig, Level, Release};

#[derive(Parser)]
#[command(
    name = "norbert",
    about = "A patient SPI-NOR flasher",
    disable_version_flag = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Pick a specific Pico de Gallo by USB serial number.
    #[arg(long, global = true)]
    pub serial: Option<String>,
    /// SPI clock in Hz (USB-FS bound; 10 MHz is plenty).
    #[arg(long, global = true, default_value_t = 10_000_000)]
    pub freq: u32,
    /// User GPIO (0-3) wired to the flash CS (SS_B). Default: User GPIO 0 (header pin 11).
    #[arg(long, global = true, default_value_t = 0)]
    pub cs: u8,
    /// User GPIO (0-3) that holds another bus master off the shared SPI while we work.
    /// Default: User GPIO 1 (header pin 12).
    #[arg(long, global = true, default_value_t = 1)]
    pub hold_gpio: u8,
    /// Level to hold the bus GPIO at.
    #[arg(long, global = true, value_enum, default_value_t = Level::Low)]
    pub hold_active: Level,
    /// What to do with the bus GPIO on release.
    #[arg(long, global = true, value_enum, default_value_t = Release::HiZ)]
    pub hold_release: Release,
    /// Machine-friendly output: drop the commentary, print IDs/addresses/OK/FAIL only.
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Print version.
    #[arg(short = 'V', long = "version", global = true)]
    pub version: bool,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

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

#[derive(Subcommand)]
pub enum Cmd {
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
```

- [ ] **Step 2: Create `src/commands/mod.rs`** with the shared helpers (dispatch is added in Task 5.6):

```rust
//! Command handlers + the bus-session helpers that guarantee bus release.
#![allow(dead_code)] // handlers/helpers wired live in Task 5.6

// Submodules are declared by their creating tasks (5.3 inspect, 5.4 write,
// 5.5 maintain + diagnose). The module-level allow above cascades to them.

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
```

- [ ] **Step 3: Rewire `main.rs` to the moved types (old match still in place).** In `main.rs`:
  - Add `mod cli;` and `mod commands;` to the `mod` block.
  - Delete the `Cli`, `Cmd` definitions and the `impl Cli { fn hold }` (now in `cli.rs`); delete `build_flasher`/`build_flasher_at` (now in `commands`).
  - Add `use cli::{Cli, Cmd};` and replace the two `build_flasher(&cli)?`/`build_flasher_at(&cli, freq)?` call prefixes so they resolve to `commands::build_flasher(&cli)?` / `commands::build_flasher_at(&cli, freq)?`.
  - Remove the now-unused `use device::{HoldConfig, Level, Release};` line (those types are used by `cli.rs` now). Let the compiler flag any other newly-unused import and delete it. The old `run()`, `Out`, `byte_bar`, `with_cancel`, `print_profile`, and error converters all stay in `main.rs` until Task 5.6.

Run: `sd '\bbuild_flasher' 'commands::build_flasher' src/main.rs`
Then fix the double-prefix that `sd` will create inside `commands/mod.rs` is not a concern (different file); but re-check `main.rs` for any `commands::commands::` and correct to a single prefix:
Run: `rg -n 'commands::commands' src/main.rs`
Expected: no matches. (If any, `sd 'commands::commands::' 'commands::' src/main.rs`.)

- [ ] **Step 4: Verify + commit.**

Run: `cargo test && cargo run -- --help`
Expected: tests pass; `--help` unchanged.

```bash
cargo fmt
git add -A && git commit -m "refactor: extract cli.rs + commands scaffolding (bus-session helpers)"
```

## Task 5.3: `commands/inspect.rs` — jedec, info, detect, sfdp, list

**Files:**
- Create: `src/commands/inspect.rs`

- [ ] **Step 1: Create `src/commands/inspect.rs`:**

```rust
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
        &format!("{:02X} {:02X} {:02X}", id.manufacturer, id.mem_type, id.capacity_code),
        &format!("{:02X}{:02X}{:02X}", id.manufacturer, id.mem_type, id.capacity_code),
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
            format!("{:02X}{:02X}{:02X}", id.manufacturer, id.mem_type, id.capacity_code),
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
        rows.push(Row::split("page", "page", format!("{} B", p.page_size), p.page_size.to_string()));
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
        let menu = p
            .erase
            .iter()
            .map(|e| format!("{}:{:02X}", e.size, e.opcode))
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(Row::new("erase", "erase", menu));
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
        let mut f = flasher(FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]), FakeBus::new(), 256);
        let (mut ui, out, _e) = Ui::captured(Mode::Human);
        detect(&mut f, &mut ui).await.unwrap();
        assert_eq!(out.contents(), format!("{}\nFound Micron M25P16.\n", voice::detect_opener()));

        let mut f = flasher(FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]), FakeBus::new(), 256);
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        detect(&mut f, &mut ui).await.unwrap();
        assert_eq!(out.contents(), "20 20 15\n");
    }

    #[tokio::test]
    async fn info_machine_is_key_value() {
        let mut f = flasher(FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]), FakeBus::new(), 256);
        let (mut ui, out, _e) = Ui::captured(Mode::Machine);
        info(&mut f, &mut ui).await.unwrap();
        let s = out.contents();
        assert!(s.contains("jedec=202015\n"), "got:\n{s}");
        assert!(s.contains("source=table\n"), "got:\n{s}");
        assert!(s.contains("address=3-byte\n"), "got:\n{s}");
    }
}
```

- [ ] **Step 2: Register the submodule** — add `pub mod inspect;` to `commands/mod.rs` (below the helpers-decl comment). The module-level `#![allow(dead_code)]` there covers the not-yet-called handlers.

- [ ] **Step 3: Verify + commit.**

Run: `cargo test commands::inspect`
Expected: the two wiring tests pass.

```bash
cargo fmt
git add -A && git commit -m "feat(commands): inspect handlers (jedec/info/detect/sfdp/list) + wiring tests"
```

## Task 5.4: `commands/write.rs` — program, erase, read, verify

**Files:**
- Create: `src/commands/write.rs`

- [ ] **Step 1: Create `src/commands/write.rs`:**

```rust
//! Mutating commands. `program`/`verify` drive the restrained progress view;
//! all four run under `with_cancel` so the bus is always released.

use std::path::Path;

use embedded_hal_async::spi::SpiDevice;

use super::with_cancel;
use crate::error::NorbertError;
use crate::flash::{BusAccess, Flasher};
use crate::ui::progress::ProgressPlan;
use crate::ui::Ui;
use crate::voice;

/// Human-friendly byte size (KiB when it divides evenly, else bytes).
fn human_bytes(n: usize) -> String {
    if n >= 1024 && n % 1024 == 0 {
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
    let image =
        std::fs::read(bitstream).map_err(|e| anyhow::anyhow!("reading {}: {e}", bitstream.display()))?;
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

        ui.say(&voice::programming_intro(&name, &human_bytes(image.len()), offset));
        let prog = ui.progress(ProgressPlan {
            erase_blocks: Some(blocks as u64),
            program_bytes: Some(image.len() as u64),
            verify_bytes: if no_verify { None } else { Some(image.len() as u64) },
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
    ui.line(&voice::program_summary(blocks, &human_bytes(image.len()), secs), "OK");
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
    let image =
        std::fs::read(bitstream).map_err(|e| anyhow::anyhow!("reading {}: {e}", bitstream.display()))?;
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
```

- [ ] **Step 2: Register the submodule** — add `pub mod write;` to `commands/mod.rs`.

- [ ] **Step 3: Verify + commit.**

Run: `cargo build && cargo test`
Expected: builds and passes. (`write` handlers are not yet called; the module
`allow(dead_code)` covers that until Task 5.6.)

```bash
cargo fmt
git add -A && git commit -m "feat(commands): write handlers (program/erase/read/verify) with progress"
```

## Task 5.5: `commands/maintain.rs` + `commands/diagnose.rs`

**Files:**
- Create: `src/commands/maintain.rs`
- Create: `src/commands/diagnose.rs`

- [ ] **Step 1: Create `src/commands/maintain.rs`:**

```rust
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
```

- [ ] **Step 2: Create `src/commands/diagnose.rs`** (doctor builds its own flashers at stepped frequencies; `test` uses the injected one):

```rust
//! Check-up + self-test. `doctor` is read-only and steps the SPI clock.

use embedded_hal_async::spi::SpiDevice;

use super::{build_flasher, build_flasher_at, with_bus};
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
        f.read_sfdp(0, &mut hdr).await.map(|_| SfdpHeader::parse(&hdr).is_some())
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
        &format!("rdid={:02X}{:02X}{:02X}", id.manufacturer, id.mem_type, id.capacity_code),
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
                    ui.line(&format!("  {freq} Hz: bus acquire failed"), &format!("{freq}=acquire-fail"));
                    stable = false;
                    continue;
                }
                let r = ff.read_id().await;
                let _ = ff.release_bus();
                match r {
                    Ok(fid) if fid.jedec() == id.jedec() => ui.line(
                        &format!("  {freq} Hz: {:02X} {:02X} {:02X} OK", fid.manufacturer, fid.mem_type, fid.capacity_code),
                        &format!("{freq}=ok"),
                    ),
                    Ok(fid) => {
                        ui.line(
                            &format!("  {freq} Hz: {:02X} {:02X} {:02X} MISMATCH", fid.manufacturer, fid.mem_type, fid.capacity_code),
                            &format!("{freq}=mismatch"),
                        );
                        stable = false;
                    }
                    Err(e) => {
                        ui.line(&format!("  {freq} Hz: read failed ({e})"), &format!("{freq}=error"));
                        stable = false;
                    }
                }
            }
            Err(e) => {
                ui.line(&format!("  {freq} Hz: connect failed ({e:#})"), &format!("{freq}=connect-fail"));
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
    with_bus(f, async |f| {
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
                    anyhow::anyhow!("sector test passed but restoring the original contents failed: {e}")
                })?;
            }
        }
        Ok(())
    })
    .await?;
    ui.line(voice::nothing_unusual(), "OK");
    Ok(())
}
```

- [ ] **Step 3: Register the submodules** — add `pub mod diagnose;` and `pub mod maintain;` to `commands/mod.rs`.

- [ ] **Step 4: Verify + commit.**

Run: `cargo build && cargo test`
Expected: builds and passes.

```bash
cargo fmt
git add -A && git commit -m "feat(commands): maintain (protect/unprotect/reset) + diagnose (doctor/test)"
```

## Task 5.6: Flip `main` to dispatch; add the print lint; delete the legacy

**Files:**
- Modify: `src/commands/mod.rs` (add `dispatch`; remove the temporary `allow`)
- Modify: `src/main.rs` (thin entrypoint; crate lint)
- Modify: `src/ui/mod.rs`, `src/ui/progress.rs`, `src/error.rs` (remove temporary `allow`)
- Modify: `src/voice.rs` (delete now-unused `programming`/`programmed`)

- [ ] **Step 1: Add `dispatch` to `commands/mod.rs`** and delete the module-level `#![allow(dead_code)]`. Append:

```rust
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
        Cmd::Program { bitstream, offset, no_verify, chip_erase, unprotect } => {
            let mut f = build_flasher(cli)?;
            write::program(&mut f, ui, bitstream, *offset, *no_verify, *chip_erase, *unprotect).await
        }
        Cmd::Erase { offset, length, chip } => {
            let mut f = build_flasher(cli)?;
            write::erase(&mut f, ui, *offset, *length, *chip).await
        }
        Cmd::Read { out, length, offset } => {
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
```

Remove the `#![allow(dead_code)]` line at the top of `commands/mod.rs` (everything is used now).

- [ ] **Step 2: Rewrite `src/main.rs`** to the thin entrypoint. Replace the ENTIRE file with:

```rust
#![deny(clippy::print_stdout, clippy::print_stderr)]
//! Norbert — a patient SPI-NOR flasher. This entrypoint only parses, builds the
//! `Ui`, and dispatches; personality lives in `voice`, output in `ui`.

mod catalog;
mod cli;
mod commands;
mod device;
mod error;
mod flash;
mod profile;
mod sfdp;
mod ui;
mod voice;

#[cfg(test)]
mod testsupport;

use std::process::ExitCode;

use clap::Parser;

use cli::Cli;
use ui::Ui;

// multi_thread is REQUIRED: the HAL bridges blocking GPIO/SPI-config calls via
// tokio::task::block_in_place, which panics on a current-thread runtime.
#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut ui = Ui::from_cli(cli.quiet);

    if cli.version {
        ui.line(&voice::version(), env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if cli.cmd.is_none() {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        return ExitCode::SUCCESS;
    }

    match commands::dispatch(&cli, &mut ui).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => ui.fail(&e),
    }
}
```

- [ ] **Step 3: Remove the temporary `allow`s** now that everything is wired: delete the `#![allow(dead_code)]` line from the top of `src/error.rs`, `src/ui/mod.rs`, `src/ui/progress.rs`, and `src/voice.rs`. (Task 5.6 Step 1 already removed the one in `commands/mod.rs`. The field-level `#[allow(dead_code)]` on `progress::Progress::mp` stays — it is genuinely never read.)

- [ ] **Step 4: Delete now-unused voice builders.** In `voice.rs`, remove `programming()` and `programmed()` (replaced by `programming_intro`/`program_summary`) and delete their two entries from the `norbert_never_shouts` `lines` array. Keep `erased()` (used by `write::erase`).

- [ ] **Step 5: Verify — build, tests, and the lint.**

Run: `cargo build`
Expected: clean, no warnings.

Run: `cargo clippy --all-targets`
Expected: clean. In particular, `deny(clippy::print_stdout, clippy::print_stderr)` produces **no** errors — confirming no `println!`/`eprintln!` survives outside… nowhere (even `ui` uses `writeln!`).

Run: `rg -n 'println!|eprintln!|print!|eprint!' src/`
Expected: no matches anywhere in `src/`.

Run: `cargo test`
Expected: all pass (flash-core, profile, ui, voice, and command wiring tests).

Run: `cargo run -- --help` and `cargo run -- list`
Expected: `--help` unchanged; `list` prints Norbert's opener, the table, and the closing note.

- [ ] **Step 6: Commit.**

```bash
cargo fmt
git add -A && git commit -m "refactor(cli): thin main dispatch; deny print macros; remove legacy Out/converters"
```

---

# Phase 6 — Documentation & final verification

## Task 6.1: Refresh the README to match the new voice/UI

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the "Getting started" transcript** so it reflects the light-framed `info` and the progress view. Replace the fenced `console` block (currently lines 45-74) with:

````markdown
```console
$ norbert detect
Hmm... let's see what we've got here.
Found Winbond W25Q128JV.

$ norbert info
Let me see what this one says about itself.
JEDEC id: mfr=0xEF type=0x40 cap=0x18 (16384 KiB)
chip:     Winbond W25Q128JV
source:   SFDP
page:     256 B
address:  3-byte
capacity: 16384 KiB
SFDP rev: 1.6
erase:    65536:D8 4096:20
It told me all that itself. I appreciate a chip that keeps notes.

$ norbert erase --chip
Erasing...
Done.

You can never be too careful.

$ norbert program firmware.bin
Programming firmware.bin — 512 KiB at 0x000000.

  erase    [██████████████████████████]  3/3 blocks
  program  [██████████████████████████]  512 KiB/512 KiB
  verify   [██████████████████████████]  512 KiB/512 KiB

Done. Have a nice boot.  (erased 3 blocks, wrote 512 KiB in 4.2s)

$ norbert verify firmware.bin
Everything checks out.
```
````

- [ ] **Step 2: Reconcile the anti-flash disclaimer** with the new (honest, determinate) bars. Change the line (currently line 11-12):

```markdown
If you're looking for blinking spinners, motivational quotes, or
RGB lighting, I'm probably not your tool.
```

(A determinate, cleared-on-finish progress view that reports real bytes and a plan is *transparency*, not decoration — Norbert keeps mocking the flashy kind.)

- [ ] **Step 3: Verify + commit.**

Run: `rg -n 'animated progress' README.md`
Expected: no matches (the phrase is gone).

```bash
git add -A && git commit -m "docs: refresh README output for the new voice + progress view"
```

## Task 6.2: Whole-crate verification + wire-neutrality confirmation

**Files:** none (verification only)

- [ ] **Step 1: Format, lint, test.**

Run: `cargo fmt --check`
Expected: no diff.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings/errors — including the crate-level `deny(clippy::print_stdout, clippy::print_stderr)`.

Run: `cargo test`
Expected: all pass. Confirm the new tests exist:
Run: `cargo test 2>&1 | rg -n 'address_width|erase_menu|say_dropped|rows_align|detect_speaks|info_machine|inert_in_machine'`
Expected: those test names appear and pass.

- [ ] **Step 2: Confirm the invariants structurally.**

Run: `rg -n 'println!|eprintln!|print!|eprint!' src/`
Expected: no matches (all output is `writeln!` inside `ui`).

Run: `rg -n 'address_bytes|erase_types|\.unwrap_or\(64 \* 1024\)|flash_bitstream|struct Progress \{|enum Progress|TooLarge|NoHold|anyhow_from|norbert_from|struct Out' src/`
Expected: no matches (`address_bytes` only survives as the `KnownChip` datasheet field in `catalog.rs`; verify any hit is that one).

Run: `tokei src/ || wc -l src/*.rs src/**/*.rs`
Expected: `main.rs` is now ~45 lines (was 747); logic is spread across `commands/`, `ui/`, `error.rs`, `profile.rs`.

- [ ] **Step 3: Wire-neutrality sanity (no hardware).** The refactor must not change any SPI byte. Spot-check the two address-emitting changes are equivalent:
  - `AddressWidth::Three.bytes() == 3`, `Four.bytes() == 4`, and `push_cmd_addr` emits the 4th byte only for `Four` — identical to the old `addr_bytes == 4` path.
  - `plan_erase`/`run_erase` push the same `(addr, opcode)` sequence the old `erase_range` did (covered by `single_granularity_plan_is_64k_blocks` / `mixed_granularity_uses_small_sector_at_tail`).

  These are asserted by the existing planner + four-byte-roundtrip tests; if they pass, the wire is unchanged.

- [ ] **Step 4: Hardware smoke test (maintainer, with a real Pico de Gallo + flash).** Not runnable in CI. Operator holds CRESET as usual.
  1. `norbert info` — read path; expect the framed profile with `SFDP rev` on an SFDP part.
  2. `norbert --quiet info` — expect bare `key=value` lines, no voice.
  3. `norbert program firmware.bin` — expect the three-phase Unicode view, then the summary; Ctrl-C mid-program prints `Stopped. I've let go of the bus.` and exits 130.
  4. `norbert doctor` — expect the framed check-up; `norbert --quiet doctor` expects `key=value` check lines + a final `OK`/`WARN`.

- [ ] **Step 5: Final commit (if fmt/docs produced any diff).**

```bash
git status
# if clean, nothing to do; otherwise:
cargo fmt && git add -A && git commit -m "chore: fmt after idiomatic refactor"
```

---

## Done

Norbert now:
- speaks in every response (a `deny` lint keeps personality inside `voice`/`ui`),
- represents geometry with `AddressWidth`/`EraseMenu`/`ErasePlan` instead of
  comment-only invariants,
- carries no dead "library-convenience" surface,
- routes all failures through one `NorbertError` boundary with guaranteed bus
  release, and
- shows a restrained, transparent, Unicode progress view — all without changing a
  byte on the SPI wire.
