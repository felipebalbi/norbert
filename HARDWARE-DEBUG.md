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
- The `--cs`/`--hold-gpio` mapping: confirmed correct against the v1.1 pinout table above.
