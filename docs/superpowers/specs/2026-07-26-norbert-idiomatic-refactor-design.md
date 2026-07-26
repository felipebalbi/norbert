# Norbert idiomatic refactor — design

- Date: 2026-07-26
- Status: approved in brainstorm; pending spec review before planning
- Supersedes the "Idiomatic-Rust refactor" explicitly deferred by
  `2026-07-25-norbert-async-conversion-design.md` (§1 Non-goals).

## 1. Goal & scope

Make Norbert read as an exemplary small Rust program. Three intertwined goals,
all in one cohesive effort ("full sweep"):

1. **Voice everywhere.** Norbert is the tool's personality; per the README he
   should be visible from *any* response. Today `voice.rs` claims to be "the ONLY
   module with personality," but that invariant is broken: several commands emit
   raw data, carry no voice, and ignore `--quiet`. Route *all* human output
   through a presentation layer that composes `voice`, and make the boundary
   structural (a lint), not a comment.
2. **Make invalid states unrepresentable.** Replace `u8`/`Vec` fields whose
   invariants live in comments with types that enforce them; remove dead
   "library-convenience" surface; unify the error and bus-session handling.
3. **Better, restrained UI.** A unified, transparent multi-phase progress view
   (Unicode block bars) that fits Norbert's anti-flash ethos.

### Decisions locked in the brainstorm

- **Scope:** full sweep — voice + type system + structure + UI.
- **Voice on data commands:** *light frame* — a brief opener/closer around clean,
  scannable data (not per-line personality, not pure data).
- **Machine/`--quiet` contract:** the existing machine output stays **byte-stable**
  (IDs, addresses, `OK`/`FAIL`, byte counts). Voice/UI changes apply to
  Human/TTY mode only. Commands that currently *ignore* `--quiet`
  (`info`/`list`/`sfdp`) gain a clean machine format — additive, not breaking.
- **Presentation architecture:** dedicated `ui` module owns all output; handlers
  print only through it; a crate lint enforces the boundary (Approach A).
- **Progress:** restrained & transparent, but **Unicode block glyphs** for smooth
  fills. No color, no emoji, no animation gimmicks.
- **Dead-code:** remove the speculative library surface (`flash_bitstream`,
  `Progress`, `FlashError::TooLarge`, `NoHold`, `Flasher::new`, the no-hold bus
  path).

### Non-goals

- No change to the async runtime or the SPI protocol. **Wire-neutral** (§10).
- No new runtime dependencies (the error type is hand-rolled; progress uses the
  already-present `indicatif`).
- No new CLI subcommands or flags. `--no-hold` is mentioned only as the *future*
  home of the (now-removed) no-hold path.
- No SFDP feature expansion beyond surfacing the already-decoded revision.

## 2. Target module architecture

```
src/
  main.rs        #[tokio::main] + parse + dispatch. ~40 lines, no command logic.
  cli.rs         clap types (Cli, Cmd, global args) — moved out of main.rs.

  ui/
    mod.rs       The presenter. Owns all output via injected Write sinks. Holds
                 output mode (Human | Machine), composes voice, renders data
                 blocks & errors.
    progress.rs  Restrained indicatif MultiProgress (erase/program/verify).
  voice.rs       Pure personality strings. Expanded to cover every command.

  commands/
    mod.rs       Cmd -> handler dispatch + bus-session helpers
                 (with_bus, with_cancel).
    inspect.rs   jedec, info, detect, sfdp, list
    write.rs     program, erase, read, verify
    maintain.rs  protect, unprotect, reset
    diagnose.rs  doctor, test

  flash.rs       Flasher core (async SPI). Dead code removed.
  profile.rs     FlashProfile, EraseType, EraseMenu, AddressWidth,
                 ProfileSource, ErasePlan, plan_erase (split out of sfdp.rs).
  sfdp.rs        SFDP byte parsing only: SfdpHeader, ParamHeader, Bfpt.
  catalog.rs     "Norbert's book": chip names + no-SFDP fallback table.
  device.rs      connect(), HostBus, HoldConfig, Level, Release
                 (Level/Release derive clap ValueEnum).
```

Division of labor: **`voice.rs` = personality strings, `ui` = layout + mode,
`commands` = sequencing + flash calls, `flash`/`profile`/`sfdp` = mechanism.**

## 3. Presentation layer (Approach A)

### 3.1 The `Ui` presenter

`Ui` owns the Human/Machine decision and every byte of output. Handlers gather
data, then *sequence* `Ui` primitives; they never format or print.

```rust
pub enum Mode { Human, Machine }   // Machine = --quiet OR stdout not a TTY

pub struct Ui {
    mode: Mode,
    out: Box<dyn Write>,           // default io::stdout(); tests inject Vec<u8>
    err: Box<dyn Write>,           // default io::stderr()
}

impl Ui {
    pub fn from_cli(quiet: bool) -> Self;         // folds in the is_terminal() check
    pub fn is_human(&self) -> bool;

    fn say(&mut self, line: &str);                // voice aside; Human-only, dropped in Machine
    fn line(&mut self, human: &str, machine: &str);// terminal outcome (replaces Out::emit)
    fn rows(&mut self, rows: &[Row]);             // labeled block; Human aligns, Machine = key=value
    fn hexdump(&mut self, base: usize, bytes: &[u8]); // Human framed, Machine raw hex
    fn progress(&self, plan: ProgressPlan) -> Progress; // §6; inert in Machine
    fn fail(&mut self, e: &NorbertError) -> ExitCode;   // voice+fact (Human) / FAIL: reason (Machine)
}

struct Row { key: &'static str, label: &'static str, value: String }
// Human:  "capacity: 16384 KiB"      Machine: "capacity=16777216"
```

A data command reads like:

```rust
ui.say(voice::info_opener());        // dropped in Machine mode
ui.rows(&report.rows());             // aligned block / key=value
ui.say(voice::info_sfdp_note(present));
```

In Machine mode the two `say` frames vanish and only the stable rows remain.
Quiet-correctness is impossible to forget.

### 3.2 Enforced boundary + testability

- `Ui` writes via `writeln!(self.out, …)` to injected sinks — **not** `println!`.
- Crate root: `#![deny(clippy::print_stdout, clippy::print_stderr)]` with **zero
  exceptions**. Any `println!`/`eprintln!` anywhere (handlers, `flash.rs`, even
  `ui`) is a hard clippy error. `voice.rs` prints nothing (pure strings).
- `indicatif` draws to its own stderr target (not via `print*`), so it is
  unaffected by the lint.
- Because output flows through `Write` sinks, tests inject a `Vec<u8>` and assert
  exact bytes for each mode.

### 3.3 What each command says (Human = light frame · Machine = stable)

| cmd | Human | Machine |
|---|---|---|
| `jedec` | `EF 40 18` — deliberately terse (curtness is character) | `EF4018` (unchanged) |
| `detect` | opener + `Found Winbond W25Q128JV.` | `EF 40 18` (unchanged) |
| `info` | opener + aligned block + SFDP note (`SFDP rev 1.6`) | **new:** `key=value` block |
| `sfdp` | opener + framed hex (or `no_sfdp`) | raw hex (now honors `--quiet`) |
| `list` | opener + table + note | raw `jedec  name` lines |
| `program` | intro + phased progress + `Done. Have a nice boot.` + summary | `OK` (unchanged) |
| `erase` | `erasing…` + `erased` voice | `OK` (unchanged) |
| `read` | `read_done(n, path)` voice | byte count (unchanged) |
| `verify` | `verify_ok` / `verify_fail(addr)` | `OK` / `FAIL: verify @…` |
| `protect`/`unprotect`/`reset` | voice (unchanged) | `OK` (unchanged) |
| `doctor` | framed check-up + voice summary | check lines + `OK`/`WARN`/`FAIL` |
| `test` | `nothing_unusual` / failure voice | `OK`/`FAIL` |

`info`/`list`/`sfdp` gain a machine format they never had (they ignored
`--quiet`); this is additive. `jedec`/`detect`/`program`/etc. keep their exact
machine tokens.

### 3.4 `voice.rs` expansion

New pure builders (numbers/paths passed in): `info_opener`,
`info_sfdp_note(present)`, `sfdp_opener`, `no_sfdp`, `list_opener`, `list_note`,
`read_done(n, path)` (removes the hardcoded string in `main.rs`),
`programming_intro(name, bytes, offset)`, `program_summary(blocks, bytes, dur)`,
`doctor_intro`. Existing builders stay. `norbert_never_shouts` extends to the new
lines.

## 4. Type-system hardening

### 4.1 `AddressWidth` (replaces `address_bytes: u8 // 3 or 4`)

```rust
pub enum AddressWidth { Three, Four }
impl AddressWidth { fn bytes(self) -> usize; fn is_four_byte(self) -> bool; }
```

Threads through `push_cmd_addr`, `adopt_profile` (enter-4B iff `Four`), the
`Display` for a profile (`"3-byte"`), and BFPT decode (dword1 bits 17–18 == 2 →
`Four`, else `Three`). An impossible width stops being representable.

### 4.2 `EraseMenu` (replaces `erase_types: Vec<EraseType> // non-empty, sorted`)

```rust
pub struct EraseMenu(Vec<EraseType>);           // invariant: non-empty, size-desc, unique sizes
impl EraseMenu {
    fn new(v: Vec<EraseType>) -> Option<Self>;  // None if empty; sorts desc, dedups
    fn largest(&self) -> EraseType;             // total — no unwrap
    fn smallest(&self) -> EraseType;            // total — replaces min_erase()'s unwrap_or
    fn iter(&self) -> impl Iterator<Item = EraseType>;
}
```

`FlashProfile.erase: EraseMenu` (non-optional). The empty case is handled once,
at detection (`detect_profile` already returns `UnsupportedChip` / falls through
to the table when the menu would be empty). Downstream, `plan_erase`'s
`.last().unwrap()` and `min_erase`'s `.unwrap_or(64 * 1024)` both disappear.

### 4.3 Fold `Level`/`Release` into clap

`main.rs`'s `ActiveArg`/`ReleaseArg` are 1:1 copies of `device.rs`'s
`Level`/`Release`, bridged by `Cli::hold()`. Derive `ValueEnum` on the `device`
enums, use them directly in `Cli`, delete both copies and the mapping (~30 lines,
and a "forgot to map a variant" failure mode).

### 4.4 Non-optional bus hold

`Cli::hold()` returns `Option<HoldConfig>` but is always `Some`; `HostBus.gpio:
Option<Gpio>` *and* `NoHold` both encode "no hold." Collapse to `hold() ->
HoldConfig`, `HostBus.gpio: Gpio`, `connect(.., HoldConfig)`. Drops the
CLI-unreachable bare-chip path; a future `--no-hold` would reintroduce it as an
explicit variant, not a nullable field.

### 4.5 Drop the public `_hal` field

`connect()` builds the HAL, makes `spi`/`bus` (each holding an
`Arc<Mutex<PicoDeGallo>>` clone that keeps the USB client alive), then drops the
HAL *internally* and returns `Connected { spi, bus }`. Same runtime behavior as
today's immediate `drop(_hal)`, minus the leaky public field and its comments.

### 4.6 One error type

```rust
pub enum NorbertError { Flash(FlashFault), Cancelled, Other(anyhow::Error) }
// From<FlashError<S,R>> maps domain faults (NoFlash, Unsupported{jedec},
// VerifyMismatch{addr}, Protected, Timeout); transport/io/arg -> Other(fact).
// From<io::Error>, From<anyhow::Error> so `?` works for file reads / arg checks.
// Hand-rolled Display + std::error::Error (no new dep), matching FlashError.
```

Handlers return `Result<(), NorbertError>`; `ui.fail(&e)` renders voice+fact
(Human) or `FAIL: reason` (Machine) and picks the exit code (`1`, or `130` for
`Cancelled`). The three scattered converters (`anyhow_from`, `norbert_from`,
`norbert_error`) collapse into one typed boundary. `FlashError` stays generic in
the `flash` core; `NorbertError` is the non-generic app error.

### 4.7 Surface the SFDP revision (uses today's dead `major`/`minor`)

`SfdpHeader.major`/`minor` are decoded but `#[allow(dead_code)]`. Add
`FlashProfile.sfdp_revision: Option<(u8, u8)>` — `Some((major, minor))` when
`source == Sfdp` (populated by `try_sfdp_profile`, which already parses the
header), `None` for the fallback table. `info`/`sfdp` render it as `SFDP rev 1.6`
(Human) / `sfdp_rev=1.6` (Machine). The `#[allow(dead_code)]` disappears.

## 5. Bus-session unification

Replace the scattered `acquire_bus … let _ = release_bus()` (repeated ~8×, plus
the `std::process::exit()`-bypasses-release hacks in the protection pre-checks)
with two helpers in `commands/mod.rs`:

- `with_bus(f, |f| async { … })` — acquire → run → **always** release. Short
  read-only ops (`jedec`/`info`/`detect`/`sfdp`, doctor's steps).
- `with_cancel(f, ui, |f| async { … })` — acquire → race `tokio::signal::ctrl_c()`
  → **always** release; on cancel render `voice::cancelled()` and yield exit code
  `130`. Long/destructive ops (`program`/`erase`/`read`/`verify`/`test`).

Both guarantee release in *one* place, closing a real footgun where a forgotten
release leaves a held master (e.g. an FPGA) stuck in reset. `main` returns
`ExitCode` and the helpers/handlers *propagate* the code — no more
`std::process::exit()` unwinding-bypass mid-stack. A Drop-guard is *not* used for
release (the async spec established that a blocking-GPIO release via
`block_in_place` during task-drop on cancellation is unsafe); the explicit
`select!` model stays.

## 6. Progress (restrained, Unicode)

`ui/progress.rs` wraps a single `indicatif::MultiProgress` holding up to three
aligned bars. `program` shows all three (they fill top-to-bottom); `erase` /
`verify` / `read` show just the relevant one.

```
Programming firmware.bin — 512 KiB at 0x000000.

  erase    [██████████████████████████]  3/3 blocks
  program  [███████████████▊──────────]  312/512 KiB   2.1 MiB/s   ETA 0s
  verify   [──────────────────────────]    0/512 KiB

Done. Have a nice boot.  (erased 3 blocks, wrote 512 KiB, verified in 4.2s)
```

- **Unicode block glyphs** for smooth sub-cell fills (`progress_chars` with the
  `█▉▊▋▌▍▎▏` ramp). No color, no emoji, no spinners-as-decoration.
- **Determinate erase bar** (today it's a blind spinner), enabled by an idiomatic
  *plan/execute* split in `flash.rs`:

  ```rust
  pub struct EraseOp { pub addr: usize, pub ty: EraseType } // carries size + opcode
  pub struct ErasePlan { ops: Vec<EraseOp> }
  impl ErasePlan {
      fn blocks(&self) -> usize;                       // ops.len()
      fn bytes(&self)  -> usize;                        // Σ op.ty.size — needs the size, not a bare opcode
  }

  // profile.rs — pure planner, now returns the typed plan (was Vec<(usize, u8)>):
  pub fn plan_erase(profile: &FlashProfile, offset: usize, len: usize) -> ErasePlan;

  impl Flasher<SPI, RST> {
      pub fn erase_plan(&self, offset, len) -> Result<ErasePlan, FlashError<..>>; // decision (needs profile)
      pub async fn run_erase(&mut self, plan: &ErasePlan, progress: impl FnMut(usize)) -> Result<..>; // action
  }
  // erase_range(offset, len) = { let p = self.erase_plan(offset, len)?; self.run_erase(&p, |_| {}).await }
  ```

  Two distinct names avoid collision: the **pure** `profile::plan_erase` (no I/O)
  and the **method** `Flasher::erase_plan` (reads `self`'s profile, hence
  `FlashError::NotDetected` on failure; the handler lifts it to `NorbertError` via
  `?`). The UI reads `plan.blocks()` for the erase-bar length. Norbert shows the
  plan before acting — transparency.
- **Byte bars** (program/verify) keep the UI-agnostic `impl FnMut(usize)`
  callback; `flash.rs` learns nothing about `indicatif`.
- **No mid-progress voice** → no draw collisions: intro prints before the bars,
  summary after. A stray mid-run line (if ever needed) routes through
  `MultiProgress::suspend`.
- **Machine mode:** the `Progress` is inert — bars hidden, callbacks no-op.

## 7. Testing

- **Keep** the 34 tests minus the 3 `flash_bitstream` ones. Adapt
  profile-constructing tests to `AddressWidth` + `EraseMenu::new(..).unwrap()`.
  The behaviors the removed tests covered (erase+program+verify end to end,
  oversize guard, detect-first) stay covered by the granular-method tests and the
  new wiring tests.
- **New unit tests:**
  - `profile`: `EraseMenu` (empty→`None`, sorts desc, dedups, total
    largest/smallest); `AddressWidth::{bytes,is_four_byte}`; `plan_erase`
    unchanged assertions adapted to the new types.
  - `ui`: `say` dropped in Machine, `line` picks human/machine, `rows` aligns vs
    `key=value`, `fail` renders voice vs `FAIL:` — all via `Vec<u8>` sinks.
- **New wiring tests:** drive a handler with `FakeFlash`/`FakeBus` + a
  buffer-backed `Ui`; assert e.g. `detect` emits opener+`Found …` in Human and
  `EF 40 18` in Machine. Handlers are generic over
  `SpiDevice + BusAccess`, so the existing fakes drive them with no hardware.
  `FakeFlash`/`FakeBus` (today private to `flash.rs`'s test module) are promoted
  to a shared `#[cfg(test)] pub(crate)` support module so `commands` tests can use
  them.
- **voice:** extend `norbert_never_shouts` to the new lines; keep
  `failures_carry_the_fact`.

## 8. Sequencing

Each phase ends green: `cargo build` + `cargo test` + `cargo clippy` (with the new
`deny`).

0. **Module split** — `sfdp.rs` → `profile.rs` + `sfdp.rs`; fallback table →
   `catalog.rs` (mechanical move; imports updated).
1. **Dead-code purge** — remove `flash_bitstream`/`Progress`/`TooLarge`/`NoHold`/
   `Flasher::new`; `BLOCK_SIZE` → `#[cfg(test)]`; unify `Level`/`Release` into
   clap; drop `_hal`.
2. **Type newtypes** — `AddressWidth`, `EraseMenu`, `ErasePlan` + plan/execute
   erase split; thread through `flash`/`profile`; adapt tests.
3. **`ui` + `NorbertError`** — sinks, `deny(print_stdout/stderr)`, error boundary.
   Port existing output from `Out`/`println!` to `Ui` (behavior preserved for
   already-voiced commands).
4. **`commands/` extraction** — four theme modules + `with_bus`/`with_cancel`;
   `main.rs` shrinks to ~40 lines.
5. **Voice expansion** — light frames + machine formats for `info`/`list`/`sfdp`;
   doctor framing; surface SFDP revision.
6. **Progress** — `MultiProgress` phased Unicode view.
7. **Verify** — clippy `deny` clean, full test run, maintainer hardware smoke
   (`info` read path, then `program` write path + progress + Ctrl-C).

## 9. File-by-file summary

| File | Change |
|---|---|
| `main.rs` | Shrinks to entrypoint + dispatch; crate lints; move clap types to `cli.rs`. |
| `cli.rs` | **New.** clap `Cli`/`Cmd`; uses `device::{Level,Release}` directly. |
| `ui/mod.rs` | **New.** `Ui` presenter over `Write` sinks; `Row`; `fail`. |
| `ui/progress.rs` | **New.** `Progress` over `MultiProgress` (Unicode bars). |
| `voice.rs` | +~10 builders; extended shouts test. |
| `commands/*.rs` | **New.** Handlers by theme + bus-session helpers. |
| `flash.rs` | Async core minus dead code; `AddressWidth`/`EraseMenu`/`ErasePlan`; plan/execute erase; erase-progress callback. |
| `profile.rs` | **New (from `sfdp.rs`).** Profile model + planner + new types. |
| `sfdp.rs` | Parser only. |
| `catalog.rs` | + fallback table (`KnownChip`, `lookup_fallback`). |
| `device.rs` | `Level`/`Release` derive `ValueEnum`; non-optional hold; `connect` drops HAL internally. |

## 10. Invariants & risks

- **Wire-neutrality (safety-critical).** Every SPI byte Norbert emits stays
  byte-identical: `push_cmd_addr` with `AddressWidth` emits the same bytes; the
  erase plan/execute runs the same ops in the same order. No protocol change —
  the hardware step is a smoke check, not a re-validation.
- **Personality-boundary invariant.** All personality text lives in `voice.rs`;
  `ui` composes it; the `deny(print_stdout)` lint makes a leak a build error.
- **Machine-contract invariant.** Existing `--quiet`/non-TTY tokens are
  byte-stable; only additive machine formats appear (`info`/`list`/`sfdp`).
- **Risk — churn.** Large surface; mitigated by the always-green phased sequence
  (§8) and preserved test assertions.
- **Risk — `EraseMenu`/`AddressWidth` ripple.** Every `FlashProfile` literal in
  tests changes; mechanical and compiler-guided.
- **Risk — Unicode bars in dumb terminals.** `indicatif` degrades on non-UTF-8
  targets; bars are hidden in Machine/non-TTY mode anyway, so scripts and logs are
  unaffected.
