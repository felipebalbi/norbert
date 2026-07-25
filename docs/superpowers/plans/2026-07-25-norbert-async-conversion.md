# Norbert Async Conversion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert norbert from blocking `embedded-hal` to native async `embedded-hal-async` on a single tokio runtime (dropping `Box::leak`), and add a `tokio::time` WIP-wait timeout, Ctrl-C cancellation with guaranteed bus release, and `indicatif` progress.

**Architecture:** norbert becomes `#[tokio::main(flavor = "multi_thread")]`. `flash.rs` is generic over `embedded_hal_async::spi::SpiDevice`; every SPI/wait method is `async`. The bus-hold GPIO stays blocking (the HAL has no async GPIO output; it bridges via `block_in_place`, hence multi-thread). The USB client stays alive after the `Hal` is dropped because `SpiDev` and `Gpio` each hold an `Arc<Mutex<PicoDeGallo>>` clone.

**Tech Stack:** Rust (edition 2021), tokio 1, embedded-hal-async 1, pico-de-gallo-hal 0.6.1, indicatif 0.17, clap 4, anyhow 1.

**Prerequisite (DONE):** `pico-de-gallo-hal 0.6.1` (the `spi_device` `block_in_place` guard) is published to crates.io.

**Spec:** `docs/superpowers/specs/2026-07-25-norbert-async-conversion-design.md`.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `Cargo.toml` | deps | Add tokio, embedded-hal-async, indicatif; bump hal to 0.6.1 in lock |
| `src/flash.rs` | flasher core + `FakeFlash`/`FakeBus` tests | `async` SPI core; async `FakeFlash`; `#[tokio::test]` |
| `src/device.rs` | HAL glue (`connect`, `HostBus`) | `BusAccess` stays sync; comment tweak only |
| `src/main.rs` | CLI, `run()`, handlers | `#[tokio::main]`, async `run()`, drop `Box::leak`, cancellation, progress |
| `src/voice.rs` | norbert strings | Add `cancelled()`; extend the shouts test |

**Invariants to preserve:** `voice.rs` is the only place with personality; `norbert_never_shouts` must still pass; all 34 existing flash tests keep their assertions.

---

## Task 1: Dependencies + hal 0.6.1

**Files:**
- Modify: `Cargo.toml:8-13`

- [ ] **Step 1: Add the async/runtime/progress deps**

Replace the `[dependencies]` block so it reads:

```toml
[dependencies]
pico-de-gallo-hal = "0.6"
pico-de-gallo-lib = "0.6"          # MAX_TRANSFER_SIZE, list_devices
embedded-hal = "1.0"               # blocking OutputPin for the bus-hold GPIO
embedded-hal-async = "1.0"         # async SpiDevice for the flash core
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "signal"] }
indicatif = "0.17"                 # progress bars for erase/program/verify
clap = { version = "4", features = ["derive"] }
anyhow = "1"
```

- [ ] **Step 2: Resolve hal 0.6.1 and confirm the still-blocking code compiles**

Run: `cargo update -p pico-de-gallo-hal --precise 0.6.1 && cargo build`
Expected: builds clean. `pico-de-gallo-hal v0.6.1` in the compile output. New deps are unused for now (no warning — unused *deps* don't warn).

- [ ] **Step 3: Confirm the pull**

Run: `rg -n -A1 'name = "pico-de-gallo-hal"' Cargo.lock`
Expected: `version = "0.6.1"`.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build(deps): add tokio/embedded-hal-async/indicatif; hal 0.6.1"
```

---

## Task 2: Convert `flash.rs` to async (core + FakeFlash + tests)

This is one atomic task: the core and its inline test module share the file, so the SPI trait swap must land with the `FakeFlash` async impl and `#[tokio::test]` conversion together, or the file won't compile.

**Files:**
- Modify: `src/flash.rs` (imports, the whole `impl Flasher`, `FakeFlash`, `#[cfg(test)] mod tests`)

- [ ] **Step 1: Swap the SPI import (top of file, line 4)**

Change `src/flash.rs:4` from:
```rust
use embedded_hal::spi::{Operation, SpiDevice};
```
to:
```rust
use embedded_hal_async::spi::{Operation, SpiDevice};
```

- [ ] **Step 2: `wait_ready` → tokio timeout + async sleep (replace lines 272-284)**

```rust
    /// Poll RDSR until WIP clears, sleeping `poll_interval` between polls.
    /// Bounded by `poll_timeout` via `tokio::time::timeout`.
    async fn wait_ready(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        let poll = async {
            loop {
                if self.read_status().await? & SR_WIP == 0 {
                    return Ok::<(), FlashError<SPI::Error, RST::Error>>(());
                }
                tokio::time::sleep(self.poll_interval).await;
            }
        };
        match tokio::time::timeout(self.poll_timeout, poll).await {
            Ok(r) => r,
            Err(_) => Err(FlashError::Timeout),
        }
    }
```

- [ ] **Step 3: `wake` → async sleep (replace the body at lines 539-546)**

```rust
    pub async fn wake(&mut self) -> Result<(), FlashError<SPI::Error, RST::Error>> {
        self.spi
            .transaction(&mut [Operation::Write(&[CMD_RELEASE_PD])])
            .await
            .map_err(Self::spi_err)?;
        // tRES1 is ~3 us on a W25Q; a USB round-trip already covers it, but be explicit.
        tokio::time::sleep(Duration::from_micros(50)).await;
        Ok(())
    }
```

- [ ] **Step 4: Make every SPI/wait method `async` and `.await` its inner calls**

For each method below, add `async` before `fn` and append `.await` to each listed inner call. **No logic changes.** Signatures otherwise identical.

| Method (line) | Add `.await` to |
|---|---|
| `read_id` (225) | `self.spi.transaction(...)` |
| `read_sfdp` (239) | `self.spi.transaction(...)` |
| `read_status` (264) | `self.spi.transaction(...)` |
| `write_enable` (251) | `self.spi.transaction(...)` |
| `enter_4byte` (258) | `self.spi.transaction(...)` |
| `detect_profile` (301) | `self.read_id()`, `self.try_sfdp_profile(id)`, `self.adopt_profile(profile)` (both arms) |
| `adopt_profile` (320) | `self.enter_4byte()` |
| `try_sfdp_profile` (332) | both `self.read_sfdp(...)` calls |
| `erase_op` (373) | `self.write_enable()`, `self.spi.transaction(...)`, `self.wait_ready()` |
| `chip_erase` (389) | `self.write_enable()`, `self.spi.transaction(...)`, `self.wait_ready()` |
| `unprotect` (398) | `self.write_enable()`, `self.spi.transaction(...)`, `self.wait_ready()` |
| `is_protected` (407) | `self.read_status()` |
| `protect` (412) | `self.write_enable()`, `self.spi.transaction(...)`, `self.wait_ready()` |
| `reset_flash` (421) | both `self.spi.transaction(...)` calls |
| `erase_range` (432) | `self.erase_op(...)` in the loop |
| `page_program` (444) | `self.write_enable()`, `self.spi.transaction(...)`, `self.wait_ready()` |
| `program` (465) | `self.page_program(...)` in the loop |
| `read` (485) | `self.spi.transaction(...)` |
| `verify` (508) | `self.read(...)` in the loop |
| `acquire_bus` (550) | `self.wake()` (keep `self.reset.acquire()` **sync**) |

**Do NOT** change `release_bus` (560, only sync `self.reset.release()`), `profile`, `set_profile`, `require_profile`, `spi_err`, `bus_err`, `push_cmd_addr`, or the `FlashId`/`FlashError`/`BusAccess`/`NoHold` types — all stay sync.

- [ ] **Step 5: `flash_bitstream` → async (line 567)**

Add `async` to `pub fn flash_bitstream`. Inside it, `.await` every flasher call it makes: `self.acquire_bus()`, `self.detect_profile()`, `self.chip_erase()`/`self.erase_range()`, `self.program(...)`, `self.verify(...)`. Keep `self.release_bus()` sync. (Read lines 567-632 and append `.await` at each `self.<flash-method>(...)` call; the `progress(...)` closure calls stay sync.)

- [ ] **Step 6: `FakeFlash` → async `SpiDevice` (replace the impl at lines 1069-1186)**

Change the impl header only; the body is byte-for-byte identical (it does no real I/O):

```rust
    impl embedded_hal_async::spi::SpiDevice for FakeFlash {
        async fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), FakeErr> {
            // ... EXISTING BODY UNCHANGED (lines 1071-1184) ...
        }
    }
```

Verify the test module's `use` line imports `SpiDevice`, `Operation`, `ErrorType`, and the SPI error traits. `SpiDevice` and `Operation` must come from `embedded_hal_async::spi`; `ErrorType`/`ErrorKind`/`Error` (the `SpiErrorTrait` alias) stay from `embedded_hal::spi` (shared between sync and async). `FakeBus` (`BusAccess`, lines 1190-1209) and `flasher()` (1213-1232) stay **unchanged** (sync).

- [ ] **Step 7: Convert the 34 tests to `#[tokio::test]`**

For every `#[test]` in `mod tests` that calls a flasher method (read_id/read_status/wait_ready/write_enable/enter_4byte/erase_range/chip_erase/program/read/verify/detect_profile/is_protected/protect/unprotect/reset_flash/wake/acquire_bus/flash_bitstream): change `#[test]` → `#[tokio::test]`, `fn` → `async fn`, and append `.await` before `.unwrap()`/`?`/the match on each such call.

Example — `read_id_returns_jedec` (634-648) becomes:
```rust
    #[tokio::test]
    async fn read_id_returns_jedec() {
        let flash = FakeFlash::new(2 * 1024 * 1024, [0x20, 0x20, 0x15]);
        let mut f = flasher(flash, FakeBus::new(), 256);
        let id = f.read_id().await.unwrap();
        assert_eq!(id, FlashId { manufacturer: 0x20, mem_type: 0x20, capacity_code: 0x15 });
        assert_eq!(id.capacity_bytes(), Some(2 * 1024 * 1024));
    }
```

Example — `wait_ready_times_out` (660-667) becomes:
```rust
    #[tokio::test]
    async fn wait_ready_times_out() {
        let flash = FakeFlash::new(1024, [0x20, 0x20, 0x15]);
        flash.set_busy_reads(u32::MAX); // never clears
        let mut f = Flasher::with_config(flash, FakeBus::new(), 256, Duration::ZERO, Duration::ZERO);
        assert!(matches!(f.wait_ready().await, Err(FlashError::Timeout)));
    }
```

Pure tests that never touch a flasher method (e.g. `FlashId` capacity tests, SFDP-parse tests) stay plain `#[test]`. `FakeFlash`/`FakeBus` use `Rc<RefCell<…>>`, which is fine under the default single-threaded `#[tokio::test]`.

- [ ] **Step 8: Run the tests**

Run: `cargo test`
Expected: PASS, 34 tests (same names, now async where converted). If a test hangs, a `.await` was missed on a `wait_ready`/`sleep` path — re-check Step 4.

- [ ] **Step 9: Commit**

```bash
git add src/flash.rs
git commit -m "refactor(flash): convert flasher core + tests to embedded-hal-async"
```

---

## Task 3: Runtime + plumbing — `#[tokio::main]`, drop `Box::leak`, await handlers

**Files:**
- Modify: `src/device.rs:72-77` (comment only)
- Modify: `src/main.rs` (imports, `main`, `run`, `build_flasher*`, every handler)

- [ ] **Step 1: device.rs — clarify the `Connected` doc (lines 72-77)**

`BusAccess` stays sync, so `device.rs` is unchanged except the struct doc. Replace the `Connected` doc comment (line 72) with:
```rust
/// Live connection. Holds `_hal` only for construction; once `spi`/`bus` exist
/// they each own an `Arc<Mutex<PicoDeGallo>>` clone that keeps the USB client
/// (and its worker) alive, so the caller may drop `_hal`.
```
(No code change in device.rs — `HostBus`, `connect`, `acquire`/`release` stay exactly as-is.)

- [ ] **Step 2: main.rs — async entrypoint (lines 189-194)**

Replace `fn main()`:
```rust
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 3: main.rs — `run` becomes async (line 196)**

Change `fn run() -> Result<()>` to `async fn run() -> Result<()>`.

- [ ] **Step 4: main.rs — drop the leak (replace lines 156-187)**

Replace `build_flasher_at`, the `keep_alive` helper, and `Connected2` with:
```rust
/// Like `build_flasher`, but at a caller-chosen SPI frequency (doctor steps freq).
fn build_flasher_at(cli: &Cli, freq: u32) -> Result<Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>> {
    let device::Connected { _hal, spi, bus } = device::connect(cli.serial.as_deref(), freq, cli.cs, cli.hold())?;
    // Chunk to the firmware's max transfer for best throughput.
    let max_chunk = pico_de_gallo_lib::MAX_TRANSFER_SIZE.min(flash::PAGE_SIZE);
    // `_hal` owns no runtime (we run inside norbert's #[tokio::main]); `spi`/`bus`
    // hold Arc<Mutex<PicoDeGallo>> clones that keep the client alive, so dropping
    // `_hal` here is safe — no Box::leak needed.
    drop(_hal);
    Ok(Flasher::with_config(spi, bus, max_chunk, Duration::from_millis(2), Duration::from_secs(120)))
}
```
(`build_flasher` at 151-153 is unchanged. Remove the now-dead `keep_alive` fn and `Connected2` struct entirely.)

- [ ] **Step 5: main.rs — `.await` every flasher call in every handler**

In each `Cmd::*` arm of `run()` and in the `doctor`/`test` helpers, append `.await` to every call on a `Flasher` value: `acquire_bus`, `release_bus` is **sync** (no await), `read_id`, `detect_profile`, `is_protected`, `unprotect`, `chip_erase`, `erase_range`, `program`, `read`, `verify`, `wake`, `reset_flash`, `protect`. Leave `release_bus()` without `.await`. This touches the arms at lines ~210-audited: Jedec (213-216), Info (230-251), Detect (257-262), Program (286-332), Read (342-347), Verify (359-367), Erase (388-393), Sfdp, Protect, Unprotect, Reset, Doctor (466-555, incl. `build_flasher_at` loop), Test (555+). Read each arm and add `.await` at the flasher call sites; leave `Out`, `voice`, `catalog`, file I/O, and `release_bus()` untouched.

- [ ] **Step 6: Build and test**

Run: `cargo build && cargo test`
Expected: both clean. 34 tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/device.rs
git commit -m "refactor(cli): run on tokio, drop Box::leak, await the async flasher"
```

---

## Task 4: Ctrl-C cancellation with guaranteed bus release

**Files:**
- Modify: `src/voice.rs` (add `cancelled()` + extend the shouts test)
- Modify: `src/main.rs` (add `with_cancel` helper; wire program/erase/verify/read)

- [ ] **Step 1: voice.rs — add the cancellation line (after `erased`, ~line 34)**

```rust
pub fn cancelled() -> &'static str {
    "Stopped. I've let go of the bus."
}
```

- [ ] **Step 2: voice.rs — add it to `norbert_never_shouts` (the `lines` array ~73-91)**

Add `cancelled().to_string(),` to the array so the no-exclamation invariant covers it.

- [ ] **Step 3: voice.rs — run the voice tests**

Run: `cargo test --lib voice`
Expected: `norbert_never_shouts` and `failures_carry_the_fact` PASS.

- [ ] **Step 4: main.rs — add the `with_cancel` helper (near `build_flasher`)**

```rust
/// Acquire the bus, run `work` racing Ctrl-C, and ALWAYS release the bus.
/// On Ctrl-C: drop the in-flight op (cooperative cancel at the next await),
/// release the bus, print the cancellation line, and exit 130.
async fn with_cancel(
    f: &mut Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>,
    quiet: bool,
    work: impl AsyncFnOnce(&mut Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>) -> Result<()>,
) -> Result<()> {
    f.acquire_bus().await.map_err(anyhow_from)?;
    let outcome = tokio::select! {
        r = work(f) => Some(r),
        _ = tokio::signal::ctrl_c() => None,
    };
    let _ = f.release_bus();
    match outcome {
        Some(r) => r,
        None => {
            if quiet {
                println!("FAIL: cancelled");
            } else {
                println!("{}", voice::cancelled());
            }
            std::process::exit(130);
        }
    }
}
```
If `AsyncFnOnce` causes lifetime trouble, fall back to inlining the `tokio::select!` body directly in each handler below (same acquire → select → release → exit-130 shape).

- [ ] **Step 5: main.rs — wire Program through `with_cancel`**

Rework the `Cmd::Program` arm (currently 275-334) so the bus-held section runs inside `with_cancel`. The detect + protection pre-check and the erase/program/verify move into the `work` closure; the existing manual `acquire_bus`/`release_bus` lines are removed (the helper owns them):
```rust
Cmd::Program { bitstream, offset, no_verify, chip_erase, unprotect } => {
    let out = Out::new(&cli);
    let image = std::fs::read(bitstream).with_context(|| format!("reading {}", bitstream.display()))?;
    let mut f = build_flasher(&cli)?;
    with_cancel(&mut f, cli.quiet, async |f| {
        f.detect_profile().await.map_err(norbert_from)?;
        match f.is_protected().await {
            Ok(true) if !*unprotect => { out.emit(voice::protected(), Some("FAIL: protected")); std::process::exit(1); }
            Ok(_) => {}
            Err(e) => return Err(anyhow_from(e)),
        }
        if let Some(cap) = f.profile().and_then(|p| p.capacity) {
            if *offset + image.len() > cap {
                return Err(anyhow::anyhow!("image needs {} bytes but flash is {cap} bytes", *offset + image.len()));
            }
        }
        if *unprotect { f.unprotect().await.map_err(anyhow_from)?; }
        out.emit(voice::programming(), None);
        if *chip_erase { f.chip_erase().await.map_err(anyhow_from)?; }
        else { f.erase_range(*offset, image.len()).await.map_err(anyhow_from)?; }
        f.program(*offset, &image, |_| {}).await.map_err(anyhow_from)?;
        if !*no_verify { f.verify(*offset, &image, |_| {}).await.map_err(norbert_from)?; }
        Ok(())
    }).await?;
    out.emit(voice::programmed(), Some("OK"));
}
```
(Progress closures stay `|_| {}` here — Task 5 replaces them.)

- [ ] **Step 6: main.rs — wire Erase, Verify, Read through `with_cancel`**

Apply the same transform to `Cmd::Erase`, `Cmd::Verify`, and `Cmd::Read`: move the code currently between `acquire_bus` and `release_bus` into a `with_cancel(&mut f, cli.quiet, async |f| { … Ok(()) }).await?;` closure, deleting the manual acquire/release. Keep pre-acquire validation (e.g. Erase's `--length`/`--chip` check, Read's buffer alloc) **outside** the closure, exactly where it is now. `info`/`jedec`/`detect`/`sfdp`/`protect`/`unprotect`/`reset`/`doctor`/`test` keep their existing acquire/release (no cancellation).

- [ ] **Step 7: Build + test**

Run: `cargo build && cargo test`
Expected: clean; 34 tests pass. (Cancellation itself is validated on hardware in Task 6.)

- [ ] **Step 8: Commit**

```bash
git add src/main.rs src/voice.rs
git commit -m "feat(cli): Ctrl-C cancels long ops and always releases the bus"
```

---

## Task 5: indicatif progress (erase spinner + program/verify bars)

**Files:**
- Modify: `src/main.rs` (Program handler: erase spinner, program bar, verify bar)

- [ ] **Step 1: Add a progress-bar helper (near `build_flasher`)**

```rust
/// A determinate byte bar (or a hidden bar in --quiet / non-TTY).
fn byte_bar(len: u64, quiet: bool) -> indicatif::ProgressBar {
    if quiet {
        return indicatif::ProgressBar::hidden();
    }
    let pb = indicatif::ProgressBar::new(len);
    pb.set_style(
        indicatif::ProgressStyle::with_template(
            "{msg:>9} [{bar:32}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
        )
        .unwrap()
        .progress_chars("=> "),
    );
    pb
}
```

- [ ] **Step 2: Wire bars into the Program `work` closure (from Task 4 Step 5)**

Inside the `with_cancel` closure, replace the erase/program/verify block with:
```rust
        out.emit(voice::programming(), None);

        let erase = if cli.quiet { indicatif::ProgressBar::hidden() } else { indicatif::ProgressBar::new_spinner() };
        erase.set_message("erasing");
        erase.enable_steady_tick(std::time::Duration::from_millis(100));
        if *chip_erase { f.chip_erase().await.map_err(anyhow_from)?; }
        else { f.erase_range(*offset, image.len()).await.map_err(anyhow_from)?; }
        erase.finish_and_clear();

        let pb = byte_bar(image.len() as u64, cli.quiet);
        pb.set_message("program");
        f.program(*offset, &image, |w| pb.set_position(w as u64)).await.map_err(anyhow_from)?;
        pb.finish_and_clear();

        if !*no_verify {
            let vb = byte_bar(image.len() as u64, cli.quiet);
            vb.set_message("verify");
            f.verify(*offset, &image, |d| vb.set_position(d as u64)).await.map_err(norbert_from)?;
            vb.finish_and_clear();
        }
        Ok(())
```
(indicatif draws to stderr, so stdout/`Out` machine output stays clean; `--quiet` uses hidden bars.)

- [ ] **Step 3: Build + test**

Run: `cargo build && cargo test`
Expected: clean; 34 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(cli): indicatif progress for erase/program/verify"
```

---

## Task 6: Verify + hardware validation

**Files:** none (verification only)

- [ ] **Step 1: Full local gate**

Run: `cargo build && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: build clean, 34 tests pass, clippy clean. Fix any clippy findings introduced by the conversion (e.g. `needless_return`, unused imports of the old `embedded_hal::spi`).

- [ ] **Step 2: Confirm the leak is gone**

Run: `rg -n 'Box::leak|keep_alive|Connected2' src/`
Expected: no matches.

- [ ] **Step 3: Hardware validation (operator holds CRESET)**

Hand the operator these commands (one at a time, never back-to-back — USB single-session):
- `cargo run -- --cs 0 --hold-gpio 1 --hold-active low --hold-release drive-high --freq 1000000 info` → expect `EF 40 18`, read path OK on the new runtime.
- `cargo run -- … program <bitstream>` → expect the erase spinner, a program byte bar, a verify byte bar, then `Done. Have a nice boot.`
- During a `program`, press Ctrl-C mid-run → expect `Stopped. I've let go of the bus.`, exit 130, and the FPGA released (bus not left held).

- [ ] **Step 4: Update HARDWARE-DEBUG.md (Session 5)**

Note the async bring-up result, the multi-thread-runtime requirement, and the Ctrl-C behavior.

- [ ] **Step 5: Final commit (docs)**

```bash
git add HARDWARE-DEBUG.md
git commit -m "docs: session 5 — async conversion validated on hardware"
```

---

## Self-review notes

- **Spec coverage:** async SPI core (T2), drop leak + `#[tokio::main]` (T3), `tokio::time::timeout` WIP-wait (T2 Step 2), Ctrl-C + guaranteed release (T4), indicatif (T5), tests async (T2 Steps 6-7), hal 0.6.1 (T1). All spec sections mapped.
- **Type consistency:** `Flasher<pico_de_gallo_hal::SpiDev, device::HostBus>` is the one concrete CLI type (helpers `with_cancel`/`byte_bar` use it); `BusAccess`/`release_bus` stay sync throughout; `Operation`/`SpiDevice` uniformly from `embedded_hal_async::spi` in flash.rs.
- **Contingencies:** if `AsyncFnOnce` fights the borrow checker (T4 Step 4), inline the `select!` per handler. If dropping `_hal` (T3 Step 4) ever shows a runtime USB issue on hardware, switch `build_flasher_at` to return `(Flasher, Hal)` and bind `_hal` in each handler.
- **Out of scope (deferred):** the idiomatic refactor (file splits, `run()` reorg, dead-code purge).
