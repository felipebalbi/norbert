# Contributing to Norbert

Norbert likes to take his time, read the datasheet, and verify his work before
declaring success. Contributions are welcome in the same spirit: small, clear,
and checked.

## Commit messages

Norbert uses [Conventional Commits](https://www.conventionalcommits.org/) with a
scope:

```
feat(catalog): name the GigaDevice GD25Q16 (C8 40 15)
fix(cli): reject unknown flags instead of ignoring them
docs(readme): document the fixed header pinout
```

Scopes in use: `catalog`, `sfdp`, `flash`, `device`, `profile`, `cli`,
`ui`/`voice`, `commands`, `docs`, `ci`, `deps`, `repo`.

Each commit should build and pass CI on its own.

## Adding a flash chip (the golden path)

This is the most common contribution, and Norbert has made it short.

1. **Read the chip's JEDEC id.** With the chip wired to a Pico de Gallo, run
   `norbert info`. It prints `mfr / type / capacity`.
2. **Find the datasheet** and keep the link — you'll cite it in the PR.
3. **Pick the right table in `src/catalog.rs`:**
   - The chip reports **SFDP** correctly and just needs a friendly name → add a
     row to `CHIP_NAMES`. If its manufacturer isn't named yet, add it to
     `manufacturer()` too.
   - The chip has **no SFDP** → add a row to `FALLBACK_TABLE` with values from
     the datasheet: page size, address width (3- or 4-byte), capacity, and erase
     types (`size` + `opcode`).
4. **Keep both tables sorted ascending by JEDEC id.** A test enforces this; an
   out-of-order row fails CI.
5. **Run the tests:** `cargo test`. The sortedness and erase-menu guards must be
   green.
6. **Open the PR** with the short catalog template (swap `HEAD` for your branch):
   <https://github.com/felipebalbi/norbert/compare/main...HEAD?template=add-a-chip.md&labels=new-chip>
   A maintainer applies the `new-chip` / `new-manufacturer` label at triage.

## Development

```
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The minimum supported Rust version (MSRV) is **1.97**.

## Testing on hardware

Norbert programs real silicon, and CI can't. If your change could affect what
gets written to a flash chip — the flash core, erase/program/verify, SFDP, or
the fallback table — test it on a real Pico de Gallo and a real chip, and paste
the `detect` / `info` / `verify` output into your PR.

If you *can't* test on hardware, say so plainly. Norbert doesn't guess, and
neither should a PR — a maintainer may hold it with `needs-hardware-test` until
someone can confirm it on silicon.

## Pull requests and issues

- Features, fixes, and refactors use the default PR template.
- Adding a chip or manufacturer uses the `add-a-chip` template (see the link
  above).
- Bugs and feature ideas have issue forms — pick one when you open an issue.

Thanks for helping Norbert keep good notes.
