# Hardware bring-up — debug notes (for the deep-dive session)

Status: **software plan complete (Tasks 1–25).** On-hardware bring-up (plan Tasks 12 & 17)
is blocked on a USB/HAL-level error, captured here so we can resume cold.

## Setup

- Host CLI: `norbert` (this repo), talking to a **Pico de Gallo v1.1** over USB.
- Target: an **iCE40 FPGA that is running**, booting from its in-circuit SPI-NOR flash on a
  shared SPI bus. The FPGA must be held off the bus (CRESET low) during any flash access.
- Flash is powered by the FPGA board; only **GND is shared** with the Pico (no VREF/3V3 wire).

## Authoritative pin map (Pico de Gallo v1.1 box header)

CLI user-GPIO numbers `0–3` map to RP2350 GPIO 8–11 = header pins 11–14.

| Signal | Header pin | RP2350 GPIO | CLI meaning |
|---|---|---|---|
| GND | 2 | — | — |
| SPI_MISO (Pico input, ← flash SO/DO/IO1) | 5 | GPIO 4 | dedicated SPI pad |
| SPI_MOSI (Pico output, → flash SI/DI/IO0) | 6 | GPIO 7 | dedicated SPI pad |
| SPI_SCK | 7 | GPIO 6 | dedicated SPI pad |
| GPIO0 | 11 | GPIO 8 | `--cs 0` / `--hold-gpio 0` |
| GPIO1 | 12 | GPIO 9 | `--cs 1` / `--hold-gpio 1` |

Note: norbert software-drives CS on a **user GPIO** (`--cs`, default 0), NOT the hardware
SPI_CS (pin 8/GPIO5). SCK/MOSI/MISO are the fixed SPI0 pads on pins 7/6/5.

## User's wiring (as reported)

| Wire | Header pin | Decodes to |
|---|---|---|
| gnd | 2 | GND ✓ |
| sck | 7 | SPI_SCK ✓ |
| io0 | 5 | SPI_MISO (GPIO 4) — ⚠ see "open questions" |
| io1 | 6 | SPI_MOSI (GPIO 7) — ⚠ see "open questions" |
| creset | 11 | user GPIO 0 → `--hold-gpio 0` |
| cs | 12 | user GPIO 1 → `--cs 1` |

Correct invocation for this wiring (shared FPGA bus):
```
norbert --cs 1 --hold-gpio 0 --hold-active low --hold-release hi-z --freq 1000000 info
```

## Symptoms observed

1. `--cs 1 --hold-gpio 0 ... info` → `no SPI-NOR flash detected (bus reads all 0x00/0xFF)`
   (i.e. `spi_device(1)` SUCCEEDED, RDID returned all 0x00/0xFF).
2. On a subsequent run, `--cs 1 ... jedec` and `--cs 0 --hold-gpio 1 ... info` BOTH failed at
   connect time with:
   ```
   Error: spi_device(N) failed: Comms("CS init failed: Endpoint(WrongDirection)")
   ```
   for both N=0 and N=1.

**Key pattern: it worked once (spi_device succeeded), then subsequent runs fail at CS init.**
This is a USB/HAL transport error, NOT norbert logic or wiring — strongly suggests **device/USB
state is not reset between host process runs** (or a `pico-de-gallo-hal` 0.6 `spi_device`
CS-init quirk), leaving the SPI/CS endpoint in a bad direction after the first process exits.

## Hypotheses (untested)

1. **Stale device state between runs.** norbert leaks `Hal` (`keep_alive`) and never cleanly
   tears down the SPI/GPIO on exit; the firmware may keep the CS pin/endpoint configured, and
   the next `spi_device()` re-init hits `WrongDirection`. → Try a **Pico power-cycle**, then a
   single clean run. If that works once and fails again, it's a teardown/reset problem.
2. **`spi_device` CS-init endpoint bug** in `pico-de-gallo-hal` 0.6.0. → Read the HAL source for
   `spi_device` → the "CS init" USB command and its endpoint direction. Compare against firmware.
3. **`system_reset_subscriptions()` insufficient.** norbert calls it in `connect()`; maybe a
   fuller device reset is needed before `spi_set_config`/`spi_device`.

## MOSI/MISO open question (must confirm)

Per the table, pin 5 = MISO (Pico input, = flash SO/IO1) and pin 6 = MOSI (Pico output, = flash
SI/IO0). For single-lane SPI the flash's **IO0/SI must reach pin 6**, and **IO1/SO must reach
pin 5**. The user's `io0→pin5, io1→pin6` looks **reversed** if io0/io1 are the flash's data pins.
A swap here would also produce all-0x00/0xFF. → Confirm on the bench: flash IO0/DI → pin 6,
flash IO1/DO → pin 5.

## Investigation plan for tomorrow

1. **Power-cycle the Pico.** Then ONE clean run: `norbert --cs 1 --hold-gpio 0 --hold-active low --hold-release hi-z --freq 1000000 jedec`. Note whether it succeeds or throws `WrongDirection`.
2. **Use `doctor`** (built in Task 24, read-only, resilient): `norbert --cs 1 --hold-gpio 0 --hold-active low --hold-release hi-z --freq 1000000 doctor`. It reports RDID, all-bytes-equal, SFDP, and a 1/5/10 MHz freq sweep. NOTE: doctor reconnects per freq step, so it will likely REPRODUCE `WrongDirection` on steps 2–3 — that's useful evidence of the "works once, fails on re-init" pattern.
3. **Confirm MOSI/MISO** orientation (io0→pin6, io1→pin5) on the bench; re-test.
4. **If `WrongDirection` persists on the first run after power-cycle:** instrument the HAL —
   read `pico-de-gallo-hal-0.6.0` `spi_device`/CS-init source (in the cargo registry) and add
   tracing at the USB boundary (systematic-debugging: log data at each layer).
5. Verify the FPGA actually tri-states when CRESET is driven low (scope/continuity), and that
   the CDONE LED behaves on release.

## What is NOT the problem

- norbert CLI/logic: build clean, 32/32 unit tests pass against the behavioral `FakeFlash`.
- Bus discipline: every command releases the hold GPIO on all paths (reviewed).
- The `--cs`/`--hold-gpio` mapping: user GPIO 0-3 = RP2350 GPIO 8-11 = header pins 11-14.

---

## Session 2 — firmware deep-dive (SPI/CS mechanics) — SUPERSEDES some guesses above

Read the firmware + gallo source in the workspace (`crates/pico-de-gallo-firmware`,
`crates/pico-de-gallo-app`, `crates/pico-de-gallo-lib`). Findings:

- **Raw SPI (`gallo spi write`/`read`/`transfer`) drives NO chip-select.** The firmware
  handlers just call `context.spi.{write,read,transfer}()`. SPI0 is created as
  `Spi::new(SPI0, PIN_6, PIN_7, PIN_4, DMA, DMA, ...)` — SCK=6 / MOSI=7 / MISO=4, **no CS pin**.
- **GPIO 5 / header pin 8 is never referenced** anywhere in the firmware. It is NOT routed as
  the SPI0 hardware CSn, and it is not a user GPIO (those are GPIO 8-11 = pins 11-14). So the
  flash CS **cannot** be on pin 8 with this firmware — nothing would drive it.
- **CS is only driven by `spi_batch(cs_pin)`** (= `hal.spi_device(cs_pin)`), which asserts
  `context.gpios[cs_pin]` — a **user GPIO 0-3 (pins 11-14)** — low around the whole op batch.
  This is the only mechanism that holds CS across a write-then-read (which flash RDID/read
  need). So norbert's `spi_device(cs_pin)` approach is correct; the flash CS MUST be on a user
  GPIO (pins 11-14), driven via spi_batch. (The earlier "hardware CS on pin 8" idea is wrong.)
- **`WrongDirection` root cause (likely):** the firmware's `gpio_put` auto-switches an
  *unconfigured* pin to output, but returns `WrongDirection` ("pin configured in wrong
  direction") if the pin was *explicitly* set to INPUT — e.g. by a prior `--hold-release hi-z`,
  which does `set_config(Input)`. `hal.spi_device(cs)` inits with a bare `gpio_put(cs, High)`
  and never sets the CS pin to output first, so once a pin has been left explicitly-input, the
  next `spi_device` on it fails. That matches "works once, then fails on re-init."
  - **Candidate fix in norbert:** before `spi_device(cs)`, configure the CS user-GPIO as output
    (`hal.gpio(cs).set_config(Output, None)`), and/or power-cycle to clear latched pin state.
    (Not yet applied — decide after the next bench test.)

## Current defaults (Session 2)

- `--cs` default **0** = User GPIO 0 = header **pin 11**.
- `--hold-gpio` default **1** = User GPIO 1 = header **pin 12** (always holds now; no no-hold mode).
- CS and hold must differ (asserted). Adjust `--cs`/`--hold-gpio` to match your actual wiring.
- With matching wiring, the invocation simplifies to:
  `norbert --hold-active low --hold-release hi-z --freq 1000000 info`
- Still confirm the MOSI/MISO (io0/io1) orientation: flash IO0/SI → pin 6 (MOSI),
  flash IO1/SO → pin 5 (MISO).

---

## Session 3 — first real hardware success + two follow-ups

**IT WORKS (read path).** With the 0xAB deep-power-down wake + `--hold-release drive-high`
(no CRESET pull-up on the iCEbreaker), `norbert info` returns the real chip:
`EF 40 18` = Winbond W25Q128JV, SFDP-sourced, full erase menu (64K/32K/4K). So detect/
info/jedec/sfdp/list + the whole SFDP/BFPT/catalog stack are validated on real silicon.

Root cause chain that got us here (for the record):
- MOSI/MISO were swapped on the flash side (fixed by the user).
- The flash was in **Deep Power-Down**: the iCE40 sends **0xB9** to the flash after config
  (seen in the boot capture, frame 9). In DPD the flash ignores everything except **0xAB**.
  Fix landed: `Flasher::wake()` sends 0xAB after `acquire_bus` (all-or-nothing on error).

### Follow-up 1 (BUG) — `program` panics: `SpiBatchOp encode failed: SerializeBufferFull`
- **Root cause:** `pico-de-gallo-internal::encode_spi_batch_ops` encodes each `SpiBatchOp`
  into a **128-byte** scratch buffer (`tmp = [0u8; 128]`) and `.expect()`s — so a single
  `Write` op can carry only ~126 bytes. norbert's `max_chunk = MAX_TRANSFER_SIZE.min(PAGE_SIZE)`
  = `min(4096, 256)` = **256**, so `page_program` emits a 256-byte Write op → overflow → PANIC.
  (`MAX_TRANSFER_SIZE = 4096` bounds the *response*/read side, not a single write op.)
- **Why reads were fine:** a `Read` op carries only a length in the request (tiny); the data
  returns in the response (≤ 4096). Only *writes* hit the 128-byte per-op cap.
- **Fix (flash.rs, TDD):** chunk `page_program`'s data into <= ~120-byte `Write` ops (128 minus
  postcard framing), independent of `max_chunk` (which can stay large for reads). A 256-byte
  page → header + 3 write ops, well under `MAX_BATCH_OPS = 64`. Add a test that a >128-byte
  program stays within the per-op limit. The HAL *panics* (doesn't return Err) on overflow, so
  norbert must respect 128/op proactively. (Reads could later be widened to 4096/txn for speed.)

### Follow-up 2 (FEATURE, general robustness) — drive /WP (IO2) and /HOLD (IO3) high
- In single-SPI, IO2 = /WP and IO3 = /HOLD. Both must be **high**: /HOLD low pauses the flash
  (no response); /WP low (with SRP0 set) blocks WRSR so you can't clear block-protect bits.
- norbert currently *assumes* they're held high externally (true for breakouts/in-circuit
  boards like the iCEbreaker, but not for a bare chip on a clip with no pull-ups).
- **Proposed:** optional `--wp-gpio` / `--io3-gpio` (default User GPIO 2 / 3, driven high on
  connect/acquire, with an off switch). Harmless when redundant, essential for a bare chip.
  Makes norbert drive the full single-SPI signal set. Small addition to device.rs/main.rs.

### Current defaults reminder
`--cs 0` (pin 11), `--hold-gpio 1` (pin 12). iCEbreaker needs `--hold-release drive-high`
(no CRESET pull-up). Working invocation:
`norbert --cs 0 --hold-gpio 1 --hold-active low --hold-release drive-high --freq 1000000 info`

---

## Session 4 — `program` validated on hardware (write path works)

**IT WORKS (write path).** The operator ran, while manually holding CRESET:
```
norbert --cs 0 --hold-gpio 1 --hold-active low --hold-release drive-high --freq 1000000 \
        program ../hdl/mole/fpga/Mole/gen/MoleTop.bin
→ Programming...
→ Done. Have a nice boot.
```
`Done. Have a nice boot.` is emitted only after the full closure succeeds, so erase →
program → **verify** (no `--no-verify`, so readback ran and matched) → release all passed on a
104090-byte bitstream. The whole erase/page-program/verify stack is now validated on silicon.

### How `SerializeBufferFull` was actually fixed (differs from Session 3's plan)
- Session 3 Follow-up 1 proposed a **norbert-side** fix (split `page_program` into ≤120-byte
  Write ops so each fits the HAL's 128-byte per-op scratch buffer).
- What we did instead: bumped the scratch buffer **128 → 1024** in the *local* pico-de-gallo
  (`pico-de-gallo-internal`, both `encode_spi_batch_ops` **and** `encode_i2c_batch_ops`; committed
  there as `Bump batch ops buffer to 1024`, HEAD `3a6aefc`). norbert's `Cargo.toml` was switched
  to **path deps** on the local `pico-de-gallo-hal` + `pico-de-gallo-lib`, which path-chain to the
  patched `internal`. norbert's 256-byte page-program op now serializes to ~258 B, well under 1024.
- **Consequence / loose end:** the fix lives in a *local* checkout. Against the **stock released
  0.6.0** crates (128-B buffer) norbert would still overflow. Durability deferred by decision —
  **keep the local path-dep for now**; the norbert-side ≤120-B chunk (Session 3) remains the
  portable alternative if we later want norbert to work against unmodified released crates.

### Debug note — the dep swap did NOT break the read path (software exonerated)
After switching to path deps, `info` regressed to `FF FF FF` ("no flash"). Proven to be
**hardware/firmware state, not the swap**:
- Local `lib`/`hal` are byte-identical to the released 0.6.0 — the last commit touching either is
  the `hal-v0.6.0`/`library-v0.6.0` release commit (`2f10b4d`); no post-release drift.
- The only `internal` delta is the scratch-buffer size, which cannot change a ~4-byte JEDEC
  batch's wire bytes (buffer bounds *capacity*, not output for small ops).
- `FF FF FF` = MISO idle-high = flash not selected/driving (not contention, which pulls toward
  `00`). A **power-cycle + manual CRESET hold** restored `EF 40 18`.

### Operational lessons (bench workflow)
- **CRESET is held by hand.** In this setup the operator physically holds the CRESET pin during
  the operation for norbert to own the shared bus; the `--hold-gpio` drive is not the effective
  reset control here. → **The operator runs the hardware commands**, timed with the hold; the
  agent hands over the exact command and asks.
- **Never run norbert invocations back-to-back.** Each process claims the USB interface and the
  next one races it → panic at `postcard-rpc raw_nusb.rs:330`:
  `Failed claiming interface: ResourceBusy (code 16)`. One run at a time; let each fully exit.
  (Same one-session-per-interface limit as pico-de-gallo AGENTS.md §13.17, 2026-07-20.)
- A host **panic mid-`program`** (the old `SerializeBufferFull`) can leave a Pico GPIO wedged
  until power-cycle; the buffer fix removes that panic, and program's error path releases the bus
  on a normal `Err`, so failures no longer wedge a pin.

---

## Session 5 — native async conversion validated on hardware

**IT WORKS (async).** norbert was converted from blocking `embedded-hal` to native
`embedded-hal-async` on a single tokio runtime, and the operator validated it on the iCEbreaker
(holding CRESET): `info` returns the chip on the new runtime, `program` runs the full
erase→program→verify with live progress and boots, and Ctrl-C mid-`program` prints
`Stopped. I've let go of the bus.` (exit 130) with the bus released. All green.

### What changed (7 commits on `main`, `d4839c9`..`ccc5c5a`)
- `flash.rs` flasher core + 34 tests → `embedded_hal_async::spi::SpiDevice`; `wait_ready` uses
  `tokio::time::timeout` + `tokio::time::sleep`; `wake` uses `tokio::time::sleep`. The bus-hold
  side (`BusAccess`/`HostBus`/`release_bus`) stays **sync** on purpose (no async GPIO output in the HAL).
- `main` is `#[tokio::main(flavor = "multi_thread")]`; `run()` is async; all handlers `.await`.
- **`Box::leak` dropped.** Created inside norbert's runtime, the `Hal` owns no `Runtime`, and
  `SpiDev`/`Gpio` each hold an `Arc<Mutex<PicoDeGallo>>` clone that keeps the USB client alive, so
  `build_flasher` just `drop(_hal)`s.
- Ctrl-C cancellation via a `with_cancel` helper: acquire → race op vs `tokio::signal::ctrl_c()` →
  **always** `release_bus()` → exit 130 (routed through `Out` so `--quiet`/non-TTY prints `FAIL: cancelled`).
- `indicatif` progress: erase spinner + program/verify byte bars (hidden under `--quiet`; on stderr
  so stdout machine output stays clean).
- Edition **2024**; `cargo clippy --all-targets -- -D warnings` clean; `cargo fmt --check` clean; 34 tests pass.

### Key requirement / gotcha
- **The runtime MUST be `multi_thread`.** The HAL bridges its blocking GPIO/SPI-config calls via
  `tokio::task::block_in_place`, which panics on a current-thread runtime.
- **Prerequisite:** `pico-de-gallo-hal 0.6.1` — a `block_in_place` guard on `Hal::spi_device`
  (it previously did a raw `handle.block_on` and panicked when called from inside a tokio runtime).
  Released to crates.io; norbert depends on `hal = "0.6"` and resolves it.
