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

## Connecting

Norbert talks to a raw SPI-NOR flash over a Pico de Gallo USB bridge.
Global flags select the device and the wiring:

- `--serial <SN>` — pick a specific Pico de Gallo
- `--freq <HZ>` — SPI clock (default 10 MHz)
- `--cs <GPIO>` — the user GPIO wired to the flash chip-select
- `--hold-gpio <GPIO> --hold-active <low|high> --hold-release <drive-high|drive-low|hi-z>` —
  hold another bus master (for example an FPGA's reset) off the shared SPI
  while programming, then release it

If another master shares the bus, hold it off first. On an iCE40, that means
driving CRESET low while programming and releasing it (Hi-Z) afterwards so the
FPGA reconfigures from the freshly written flash.

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

## License

MIT. See [LICENSE](LICENSE).
