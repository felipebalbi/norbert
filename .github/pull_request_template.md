<!--
Thanks for contributing to Norbert. He likes tidy notes.

House rules:
- Conventional Commits with a scope: feat(catalog): …, fix(cli): …,
  refactor(voice): …, docs(readme): …. Scopes: catalog, sfdp, flash, device,
  profile, cli, ui/voice, commands, docs, ci, deps, repo.
- Each commit should build and pass CI on its own.

Just adding a chip or a manufacturer to the table? There is a shorter template
for that — open your PR with this link instead (swap HEAD for your branch):
https://github.com/felipebalbi/norbert/compare/main...HEAD?template=add-a-chip.md&labels=new-chip
-->

## Summary

<!-- What does this PR do, and why? One or two sentences. -->

## Linked issues

<!-- e.g. "Closes #123", "Refs #456". -->

## Affected areas

<!-- Tick all that apply. -->

- [ ] detect / SFDP parsing
- [ ] catalog (chip names / manufacturers / fallback table)
- [ ] erase
- [ ] program
- [ ] verify
- [ ] read
- [ ] protect / reset
- [ ] doctor / test
- [ ] CLI (flags, argument parsing, output)
- [ ] transport (Pico de Gallo bridge)
- [ ] documentation (README, HARDWARE-DEBUG, rustdoc)
- [ ] CI / tooling

## Testing performed

<!-- Norbert verifies before declaring success. So do we. Be specific. -->

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets --locked -- -D warnings`
- [ ] `cargo test --locked`
- [ ] Tested on real hardware — which Pico de Gallo (rev + firmware) and which
      flash chip? Paste the relevant `detect` / `info` / `verify` output below.
- [ ] Not tested on hardware — and here is why that is safe / not applicable:

<!-- Hardware notes, command output, logic-analyzer captures, etc. -->

## Documentation

- [ ] README / `HARDWARE-DEBUG.md` / rustdoc updated to match this change.
- [ ] No user-visible behavior changed; no docs needed.

## Checklist

- [ ] Commits follow Conventional Commits with a correct scope.
- [ ] `Cargo.lock` is committed alongside any `Cargo.toml` change; I ran with `--locked`.
- [ ] New public items have rustdoc.
