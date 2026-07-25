# Norbert async conversion — design

- Date: 2026-07-25
- Status: approved in brainstorm; pending spec review before planning

## 1. Goal & scope

Convert norbert from blocking `embedded-hal` to native async `embedded-hal-async`,
running on a single tokio runtime owned by norbert, and drop the `Box::leak` used
today to keep the HAL's internal runtime alive. Alongside the mechanical
conversion, add three async-enabled capabilities:

- WIP-wait timeout via `tokio::time::timeout`.
- Ctrl-C cancellation that always releases the bus / CRESET cleanly.
- Live progress via `indicatif` during erase / program / verify.

### Non-goals

- **Idiomatic-Rust refactor** — explicitly deferred to a dedicated session. No file
  splits, no reorganization of `run()`, no dead-code purge beyond what the async
  change mechanically requires.
- No CLI surface or output changes beyond the new progress bar.
- No async GPIO output (the HAL has none; the bus-hold stays blocking, bridged).

## 2. Background (current architecture)

- `flash.rs`: `Flasher<SPI: embedded_hal::spi::SpiDevice, RST: BusAccess>`; blocking
  SPI; `wait_ready()` uses `Instant` + `std::thread::sleep`; `wake()` uses
  `std::thread::sleep(50 µs)`.
- `device.rs`: `connect()` builds `Connected { _hal, spi: SpiDev, bus: HostBus }`;
  `HostBus` drives a hold GPIO via blocking `embedded_hal::digital::OutputPin`.
  `BusAccess::{acquire, release}` are sync.
- `main.rs`: sync `run()` (one large match); `build_flasher` → `keep_alive()` →
  `Box::leak(hal)` to keep the HAL's tokio runtime alive because `SpiDev`/`Gpio`
  only hold cloned tokio `Handle`s.
- HAL (`pico-de-gallo-hal` 0.6.0):
  - `SpiDev` implements **both** blocking and async `SpiDevice`; the async impl is a
    pure `.await` on `self.gallo.lock().await` (no `block_on`).
  - GPIO **output** is blocking-only (`set_config`/`set_low`/`set_high`); the only
    async GPIO trait is `Wait` (input edges).
  - Most methods guard blocking work with
    `if in_async_context() { block_in_place(inner) } else { inner }`.
  - `Hal::new_inner` reuses the current runtime when constructed inside one and then
    owns **no** `Runtime`; created outside a runtime it builds and owns its own.

## 3. Phase 0 — HAL fix (prerequisite)

`Hal::spi_device` (hal `lib.rs:427`) is the only device constructor that calls
`handle.block_on` **without** the `in_async_context`/`block_in_place` guard, so it
panics when called from inside a tokio runtime. This is a genuine latent bug.

Fix: extract `spi_device_inner(cs_pin)` holding the two `block_on` calls and guard
the public method exactly like every sibling:

```rust
pub fn spi_device(&self, cs_pin: u8) -> Result<SpiDev, SpiHalError> {
    if Self::in_async_context() {
        block_in_place(|| self.spi_device_inner(cs_pin))
    } else {
        self.spi_device_inner(cs_pin)
    }
}
```

Add a test in the crate's existing style that exercises `spi_device` from inside a
`#[tokio::test]` and asserts it does not panic at the `block_on` boundary (match how
the crate tests other bridged methods; a device-less smoke test is acceptable if
that mirrors existing tests).

**User constraint:** write the change only. **Do NOT commit it and do NOT release.**
The maintainer reviews, commits, and cuts `hal 0.6.1` separately.

### Development bridge

Until `hal 0.6.1` is on crates.io, norbert consumes the fix via a **temporary
path-dep** to the local `pico-de-gallo-hal` (and `pico-de-gallo-lib`, so the path
chain resolves) — the same bridge used for the `internal` buffer fix. Once
`hal 0.6.1` releases, norbert reverts to `hal = "0.6"` / `lib = "0.6"` and
`cargo update` pulls it. norbert's `internal` is already the released `0.6.1`.

## 4. norbert runtime & entrypoint

- `main` → `#[tokio::main(flavor = "multi_thread")] async fn main()`. Multi-thread is
  **required**: the bus-hold GPIO uses the HAL's blocking output, which bridges via
  `block_in_place`, valid only on a multi-thread runtime.
- `run()` → `async fn run()`; each subcommand arm `.await`s its flash calls (no
  structural change to the match — that is refactor scope).
- `device::connect()` stays **sync**, called from async. With `spi_device` guarded,
  every HAL call it makes is async-context-safe. It returns
  `Connected { hal, spi, bus }` owned in `run()`'s scope.
- **Remove `keep_alive`, `Connected2`, and the `Box::leak`.** Created inside
  norbert's runtime, the HAL owns no `Runtime`; `SpiDev`/`Gpio` hold `Handle`s to
  norbert's runtime, which lives for `main`. Keep `hal` bound in scope for the
  operation's duration.
- `Cargo.toml`: add
  `tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "signal"] }`,
  `embedded-hal-async = "1"`, `indicatif = "0.17"`. Keep `embedded-hal` (blocking
  `OutputPin` for the hold GPIO).

## 5. flash.rs async core

- `use embedded_hal_async::spi::{Operation, SpiDevice}`; bound
  `SPI: embedded_hal_async::spi::SpiDevice`.
- Make `async fn` (callers `.await`): `read_id`, `read_sfdp`, `read_status`,
  `write_enable`, `enter_4byte`, `erase_op`, `chip_erase`, `erase_range`, `program`,
  `read`, `verify`, `detect_profile`, `is_protected`, `unprotect`, `wake`,
  `acquire_bus`, `flash_bitstream`. `release_bus` stays sync (only sync GPIO).
- `wait_ready()`:
  `tokio::time::timeout(self.poll_timeout, async { loop { if wip clear { break } tokio::time::sleep(self.poll_interval).await } }).await`,
  mapping elapsed to `FlashError::Timeout`. `poll_interval`/`poll_timeout` config
  unchanged.
- `wake()`: `tokio::time::sleep(Duration::from_micros(50)).await`.
- `BusAccess` trait stays **sync**; `HostBus` unchanged (blocking `OutputPin`,
  bridged). `acquire_bus` is `async` solely to `.await self.wake()`.

## 6. Ctrl-C cancellation + guaranteed bus release

Chosen: explicit `select!` + always-release helper. (Drop-guard rejected — `Drop` is
sync and a blocking-GPIO release via `block_in_place` during task-drop on
cancellation is unsafe. May revisit if the explicit form reads oddly.)

- A helper in `main.rs` establishes the invariant: **acquire → race op vs
  `tokio::signal::ctrl_c()` → always release**. Sketch (exact signature settled in
  the plan; async-closure vs `Future` arg TBD):

  ```rust
  // acquire_bus, then:
  let outcome = tokio::select! {
      r = op(&mut f) => Some(r),      // op borrows &mut f
      _ = tokio::signal::ctrl_c() => None,
  };
  let _ = f.release_bus();            // ALWAYS: success / error / cancel
  // None => cancelled: print a norbert-voiced line, exit code 130
  ```

- On cancel the op future is dropped (cooperative cancel at the next `.await`, e.g.
  between page-programs); the bus is released to its configured state; the CLI prints
  a norbert-voiced cancellation line and exits `130`.
- Applied to the bus-holding long ops: `program`, `erase`, `verify`, and `read`
  (a full-chip dump can be long). Short ops (`info`/`jedec`/`detect`) keep the plain
  acquire/…/release; cancellation there is unnecessary.
- Interrupting mid-erase/mid-program leaves the flash partially written (inherent to
  flashing). The guarantee is a clean **bus** release, not atomic flash content.

## 7. Progress via indicatif

- Flash core stays UI-agnostic: keep the per-method `progress: impl FnMut(usize)`
  callbacks (today `|_| {}`).
- `main.rs` owns an `indicatif::ProgressBar` (length = bytes to erase/program/verify)
  and passes a closure that advances it. Separate sequential bars for the erase,
  program, and verify phases; style shows bytes + rate + ETA. No progress types enter
  `flash.rs`.

## 8. Tests

- `FakeFlash` implements `embedded_hal_async::spi::SpiDevice` (async `transaction`,
  identical in-memory logic, returns ready). `FakeBus`/`NoHold` stay sync
  (`BusAccess` is sync).
- The 34 tests become `#[tokio::test]` and `.await` the flash calls; assertions
  unchanged. `wait_ready_times_out` still uses `Duration::ZERO` and times out via
  `tokio::time::timeout`.

## 9. Sequencing & verification

1. Write HAL `spi_device` fix locally (no commit, no release).
2. norbert: temporary path-dep to local `hal` + `lib`; add
   tokio/embedded-hal-async/indicatif deps.
3. Convert `flash.rs` + `device.rs` + `main.rs` to async; drop the leak; add timeout,
   cancellation, progress.
4. Convert tests; `cargo test` green (34).
5. `cargo build` + `cargo clippy` clean.
6. Hardware re-validation (operator holds CRESET): `info` (read path), then `program`
   (write path + progress + Ctrl-C behavior).
7. Later (maintainer): review HAL change, commit, release `hal 0.6.1`; norbert
   reverts to registry `hal`/`lib`, `cargo update`.

## 10. Risks

- **Multi-thread runtime requirement** (block_in_place bridge) — documented;
  `#[tokio::main]` defaults to multi-thread.
- **Unreleased HAL fix during dev** — mitigated by the path-dep bridge (proven).
- **Cooperative cancellation granularity** — cancel takes effect at the next
  `.await` (between chunks); acceptable for a flasher.
- **indicatif vs norbert's voice** — keep progress bars visually distinct from voice
  lines; the no-exclamation rule applies only to `voice.rs` strings.
