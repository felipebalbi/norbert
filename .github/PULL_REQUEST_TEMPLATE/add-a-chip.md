<!--
Adding a chip Norbert hasn't met yet? Good. He appreciates a chip that keeps
notes — bring its datasheet and you'll get along fine.

This is the short path. If your change is more than catalog rows, use the
default template instead (open a plain PR).

A maintainer applies the `new-chip` and/or `new-manufacturer` label at triage.
-->

## Which chip?

- Manufacturer and part number: <!-- e.g. Winbond W25Q128JV -->
- JEDEC id (mfr / type / capacity): `0x__ 0x__ 0x__` <!-- from `norbert info` -->
- Datasheet URL:

## What this PR adds

<!-- Tick all that apply. -->

- [ ] A new manufacturer ID in `manufacturer()` (a vendor Norbert didn't name before).
- [ ] A new pretty name in `CHIP_NAMES` (the part reports SFDP correctly; it just needed a name).
- [ ] A new `FALLBACK_TABLE` row — the part has **no SFDP** and must be described from its datasheet.

## Fallback-table values

<!-- Only if you ticked the FALLBACK_TABLE box. Straight from the datasheet.
     Leave blank for SFDP-capable parts. -->

- Page size (bytes):
- Address width: <!-- 3-byte or 4-byte -->
- Capacity (bytes):
- Erase types (`size:opcode`): <!-- e.g. 65536:D8, 4096:20 -->

## Confirmations

- [ ] I kept the tables **sorted ascending by JEDEC id** (a test enforces this).
- [ ] `cargo test` passes locally (the sortedness and erase-menu guards are green).
- [ ] I tested on real hardware — output below — **or** these are datasheet values
      only (no chip in hand), and I've said so.

<!-- `norbert detect` / `norbert info` output, if you have the chip. -->
