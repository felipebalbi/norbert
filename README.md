# Hello.

I'm **Norbert**.

I program SPI NOR flash chips.

I've been doing this sort of thing for a while. I like to take my time,
read the datasheet, and verify my work before declaring success.
Computers are fast enough already.

If you're looking for blinking spinners, motivational quotes, or
RGB lighting, I'm probably not your tool.

If you need to identify a flash chip, erase it, program it, verify it,
or read it back, we'll get along just fine.

## About me

I have been around long enough to know that verification is faster than
debugging. I prefer simple tools, clear diagnostics, and datasheets over
forum posts. I don't mind waiting another second if it means getting the
right answer.

## What I can do

- Detect SPI NOR flash devices and read JEDEC IDs
- Parse SFDP tables (and fall back to a table of chips I already know)
- Erase sectors, blocks, or the whole device
- Program firmware images and verify what was written
- Read flash contents back to disk
- Protect / unprotect, reset, and run a check-up (`doctor`, `test`)

## What I won't do

- Guess.
- Skip verification because "it's probably fine."
- Pretend every flash chip behaves the same.
- Rush.

If something looks unusual, I'll tell you. If something doesn't match,
we'll figure it out.

## Getting started

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

  erase    [██████████████████████████]  8/8 blocks
  program  [██████████████████████████]  512 KiB/512 KiB
  verify   [██████████████████████████]  512 KiB/512 KiB

Done. Have a nice boot.  (erased 8 blocks, wrote 512 KiB in 4.2s)

$ norbert verify firmware.bin
Everything checks out.
```

For scripts, `--quiet` drops the commentary and prints machine-friendly
lines instead (IDs, addresses, `OK`/`FAIL`). Norbert also goes quiet
automatically when output is not a terminal.

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
pin  2 │ GND                 │ pin  1
pin  4 │                     │ pin  3
pin  6 │ SPI_MOSI   SPI_MISO │ pin  5
pin  8 │             SPI_SCK │ pin  7
pin 10 │                     │ pin  9
pin 12 │ /WP              CS │ pin 11
pin 14 │ CRESET        /HOLD │ pin 13
pin 16 │                     │ pin 15
pin 18 │                     │ pin 17
pin 20 │                     │ pin 19
pin 22 │                     │ pin 21
pin 24 │                     │ pin 23
       └─────────────────────┘

 Only the pins Norbert connects are labelled; every other pin is unused.
 /WP and /HOLD are held high; CRESET (pin 14) is driven only with --reset.
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

## Design principles

Programming flash memory should be **predictable**, **transparent**,
**reliable**, and — most of all — **boring**. Exciting firmware tools
usually become interesting for all the wrong reasons.

If a command succeeds, you should know why. If it fails, you should know
where. The code stays professional; only I get to have a personality.

## A note from Norbert

> Datasheets are usually more reliable than forum posts.
> We'll ask the flash chip what it supports before making assumptions.
> Measure twice. Program once.

Happy flashing.

## Contributing

Found a chip Norbert doesn't know yet, or want to add a feature? See
[CONTRIBUTING.md](CONTRIBUTING.md). Adding a chip to the table is the easiest
place to start — there's a short PR template just for it.

## License

MIT. See [LICENSE](LICENSE).
