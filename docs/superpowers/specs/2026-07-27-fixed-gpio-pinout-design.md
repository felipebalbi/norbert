# Fixed GPIO pinout — design

- Date: 2026-07-27
- Status: approved in brainstorm; pending spec review before planning
- Branch: `fixed-gpio-pinout`

## 1. Goal & scope

Replace the free-form, user-assignable GPIO flags with a single **fixed pin
contract** so that every flash wires to a Pico de Gallo the same way, and remove
the now-unnecessary configuration surface from the CLI. Also start driving the
flash's `/WP` (IO2) and `/HOLD` (IO3) lines high, which Norbert previously only
*assumed* were held high externally.

Fixed mapping (Pico de Gallo v1.1 user GPIOs):

| Flash signal | Pico de Gallo | Header pin | RP2350 GPIO | How Norbert drives it |
|---|---|---|---|---|
| CS (SS_B)   | GPIO0 | 11 | GPIO 8  | low per SPI transaction (via `spi_device(0)`) |
| IO2 (/WP)   | GPIO1 | 12 | GPIO 9  | **high** always (deasserted) |
| IO3 (/HOLD) | GPIO2 | 13 | GPIO 10 | **high** always (deasserted) |
| CRESET      | GPIO3 | 14 | GPIO 11 | low while working, high on release — **only with `--reset`** |

Unchanged fixed SPI pads: SCK = pin 7 (GPIO 6), MOSI = pin 6 (GPIO 7, → flash
IO0/SI), MISO = pin 5 (GPIO 4, ← flash IO1/SO), GND = pin 2.

### Non-goals

- No change to the flash protocol / SFDP / catalog / command behavior.
- No change to `--serial`, `--freq`, `--quiet`, `-V/--version`.
- No off-switch for `/WP` / `/HOLD`. Users who don't want them driven simply leave
  those dupont wires unconnected.
- No async GPIO output (the bus-hold stays sync/blocking, as today).

## 2. Background (current state)

- `cli.rs` exposes `--cs` (default GPIO0), `--hold-gpio` (default GPIO1),
  `--hold-active` (`Level`), `--hold-release` (`Release`), plus a `hold()` builder
  returning `HoldConfig`.
- `device.rs`: `Level`, `Release`, `HoldConfig`, `HostBus { gpio, active, release }`
  implementing `BusAccess`; `connect(serial, freq_hz, cs_pin, hold)` validates
  `cs_pin != hold.pin`, grabs `spi_device(cs_pin)` and `gpio(hold.pin)`.
- `commands/mod.rs`: `build_flasher_at` calls `device::connect(.., cli.cs, cli.hold())`.
- `voice.rs:62`: a diagnostic hint string references `--cs` / `--hold-gpio`.
- `/WP` and `/HOLD` are **not driven** today — assumed high externally. Fine on
  breakouts/in-circuit boards, broken on a bare chip on a clip with no pull-ups
  (flagged as a TODO in `HARDWARE-DEBUG.md`, Session 3 Follow-up 2).

## 3. CLI changes (`cli.rs`)

- **Remove:** `--cs`, `--hold-gpio`, `--hold-active`, `--hold-release`, the `hold()`
  method, and the `HoldConfig`/`Level`/`Release` import.
- **Add:** `--reset` (bool, `global = true`). Help text: hold a shared bus master
  (e.g. an FPGA's CRESET) in reset while programming, then release it so it boots
  from the freshly-written flash. Omit it for a bare chip / no shared master.
- **Keep:** `--serial`, `--freq`, `--quiet`, `-V/--version`.

Naming note: a `reset` **subcommand** (flash soft-reset 0x66/0x99) already exists.
The global `--reset` flag does not collide in clap's parser (flag vs positional
subcommand), and is the name approved in brainstorm. `norbert --reset reset` is
legal-but-odd; accepted.

## 4. Device layer (`device.rs`)

- Delete `Level`, `Release`, `HoldConfig` (all dead after hardcoding).
- `HostBus` becomes:

  ```rust
  pub struct HostBus {
      wp: Gpio,      // GPIO1 (/WP)  — held high
      hold: Gpio,    // GPIO2 (/HOLD)— held high
      creset: Gpio,  // GPIO3 (CRESET)
      reset: bool,   // whether CRESET is used
  }
  ```

- `BusAccess for HostBus`:
  - `acquire()`: configure `wp` + `hold` as output, drive both **high**; if
    `reset`, configure `creset` as output and drive **low**.
  - `release()`: if `reset`, configure `creset` as output and drive **high**;
    leave `wp` / `hold` high. When `reset` is false, `release()` is a no-op on
    CRESET.
- `connect` signature: `connect(serial: Option<&str>, freq_hz: u32, reset: bool)`.
  - Drops the `cs_pin == hold.pin` validation (pins are fixed and distinct).
  - `spi = hal.spi_device(0)` (CS on GPIO0).
  - `bus = HostBus { wp: hal.gpio(1), hold: hal.gpio(2), creset: hal.gpio(3), reset }`.
  - Keeps the existing "`drop(hal)`; the handles hold Arc clones that keep the USB
    client alive" pattern — must confirm each `hal.gpio(n)` clone independently
    keeps the client alive (as `spi_device` + one `gpio` do today).

Ordering: `acquire()` runs before `wake()` inside `Flasher::acquire_bus`, so
`/WP` and `/HOLD` are already high, and (when enabled) CRESET already low, before
the 0xAB wake and any RDID. No change needed in `flash.rs`.

## 5. Callers

- `commands/mod.rs`: `device::connect(cli.serial.as_deref(), freq, cli.reset)`.
- `voice.rs:62`: rewrite the hint to drop `--cs` / `--hold-gpio`; point at the
  fixed wiring and `--reset` instead (e.g. "Check wiring (CS/MISO/GND/power) and,
  if another bus master shares the SPI, pass `--reset`.").
- `diagnose.rs` (`doctor`) already takes `&Cli` and builds flashers via
  `build_flasher*`; it picks up the new mapping for free.

## 6. README

Add a **"Wiring"** section (and trim the existing "Connecting" section to match):

1. The signal-mapping table from §1 (flash pin → Pico net → header pin → GPIO).
2. An ASCII depiction of the **2×12 / 24-pin** v1.1 box header with Norbert's
   pins marked (the header is 2×12, not 2×20).
3. Notes:
   - CS is software-driven on **GPIO0** (header pin 11), *not* the hardware
     SPI_CS on pin 8 (the firmware doesn't route pin 8 as a usable CSn).
   - Data orientation: flash **IO0/SI/DI ← MOSI (pin 6)**, flash **IO1/SO/DO →
     MISO (pin 5)**.
   - `/WP` and `/HOLD` are held high; leave them unconnected if your board already
     pulls them up. Because both are driven to the *same* level, swapping GPIO1↔
     GPIO2 (IO2 vs IO3) is functionally harmless.
   - CRESET is optional: wire GPIO3 → target CRESET and pass `--reset` only when a
     shared bus master must be held off during programming.
   - Power: share GND (pin 2 / 24); VREF/+3V3 (pin 1 / 23) is available if you want
     the Pico to power the flash.
4. Update the personality-flavored `--cs`/`--hold-gpio` bullets in "Connecting"
   to the new `--reset`-only surface.

## 7. Testing

- Unit tests use `FakeBus` (implements `BusAccess` directly, not `HostBus`), so
  they are unaffected by the `HostBus`/`connect` changes and should stay green.
- `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`,
  and the existing test suite must all pass.
- Hardware validation (operator-run, out of scope for this change's CI): a bare
  chip with nothing on GPIO1/2/3 still detects/programs (WP/HOLD now actively
  high); an iCE40 with GPIO3→CRESET + `--reset` programs and boots.

## 8. Migration / breaking change

This is a deliberate breaking change to the CLI and the expected wiring:

- Removed flags (`--cs`, `--hold-gpio`, `--hold-active`, `--hold-release`) will
  now error as unknown arguments.
- Anyone previously wiring CRESET to GPIO1 (the old `--hold-gpio` default) must
  move it to **GPIO3** and pass `--reset`.
- The old iCEbreaker invocation
  `--cs 0 --hold-gpio 1 --hold-active low --hold-release drive-high ... program`
  becomes `--reset ... program`.
