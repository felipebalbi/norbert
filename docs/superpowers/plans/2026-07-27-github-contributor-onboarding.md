# GitHub contributor onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Norbert's complete `.github/` contributor surface — two PR templates, two issue forms + config, a label taxonomy file, a CI workflow, dependabot, and a CONTRIBUTING guide — modeled on the Pico de Gallo repo but retargeted to a single host binary crate.

**Architecture:** All deliverables are static config/docs files. They are independent of the Rust source; no code, catalog, or command behavior changes. Each file is created in its own task and committed separately with a Conventional-Commits message. Verification is per-file: YAML files are parse-checked with Python, the workflow is checked with `actionlint` (via Docker, matching CI), Markdown files are existence/non-empty checked, and a final task runs the exact CI commands locally to prove the repo is green under them.

**Tech Stack:** GitHub Actions, GitHub issue forms (YAML), dependabot v2, `dtolnay/rust-toolchain`, `rhysd/actionlint`, Cargo (Rust edition 2024, MSRV 1.97).

**Spec:** `docs/superpowers/specs/2026-07-27-github-contributor-onboarding-design.md`

**Repo facts the engineer needs:**
- Owner/repo: `felipebalbi/norbert`. Default branch: `main`. Working tree is clean.
- Single crate at repo root (`Cargo.toml`, `Cargo.lock` committed).
- Conventional Commits with scopes are already the norm. Scopes used here: `repo`, `ci`, `docs`, `readme`.
- Verification tools present locally: `cargo 1.97.1`, `python3` + PyYAML, `docker`, `yamllint`, `gh`.

**File structure created by this plan:**
```
.github/
├── labels.yml                            # Task 1
├── pull_request_template.md              # Task 2
├── PULL_REQUEST_TEMPLATE/add-a-chip.md   # Task 3
├── ISSUE_TEMPLATE/bug_report.yml         # Task 4
├── ISSUE_TEMPLATE/feature_request.yml    # Task 5
├── ISSUE_TEMPLATE/config.yml             # Task 6
├── workflows/ci.yml                      # Task 7
└── dependabot.yml                        # Task 8
CONTRIBUTING.md                           # Task 9
README.md                                 # Task 10 (edit: add Contributing pointer)
```

**Reusable verification commands (referenced by tasks):**
- YAML parses: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('OK')" <file>`
- Markdown non-empty: `test -s <file> && echo OK`
- Workflow lint: `docker run --rm -v "$(pwd)":/repo -w /repo rhysd/actionlint:latest -color` (needs network to pull the image the first time; if unavailable, fall back to the YAML-parse check above and rely on the CI `actionlint` job).

---

## Task 1: Label taxonomy source of truth

**Files:**
- Create: `.github/labels.yml`

- [ ] **Step 1: Create `.github/labels.yml`**

```yaml
# Norbert label taxonomy — the source of truth.
#
# This file DOCUMENTS the labels Norbert uses. By design it is NOT applied by any
# workflow; a maintainer creates/edits labels to match (web UI, or `gh label`).
# The shape is compatible with EndBug/label-sync and github-label-sync, should a
# sync step ever be added.
#
# Create one by hand with, e.g.:
#   gh label create new-chip --color 0e8a16 \
#     --description "Adds a chip to CHIP_NAMES or FALLBACK_TABLE"

# --- Type -------------------------------------------------------------------
- name: bug
  color: d73a4a
  description: Something is broken.
- name: enhancement
  color: a2eeef
  description: New capability or improvement.
- name: documentation
  color: 0075ca
  description: README, HARDWARE-DEBUG, or rustdoc.
- name: refactor
  color: fbca04
  description: Internal change, no behavior change.
- name: dependencies
  color: 0366d6
  description: Dependency or toolchain bumps (dependabot).

# --- Catalog ----------------------------------------------------------------
- name: new-chip
  color: 0e8a16
  description: Adds a chip to CHIP_NAMES or FALLBACK_TABLE.
- name: new-manufacturer
  color: 1d76db
  description: Adds a JEDEC vendor ID to manufacturer().

# --- Workflow ---------------------------------------------------------------
- name: triage
  color: ededed
  description: New issue, awaiting maintainer.
- name: needs-hardware-test
  color: b60205
  description: Unverified on real silicon; blocks merge.
- name: needs-datasheet
  color: fef2c0
  description: Catalog entry lacks a datasheet citation.
- name: good first issue
  color: 7057ff
  description: Approachable — e.g. adding a known chip.
- name: help wanted
  color: '008672'
  description: Maintainer would welcome help here.

# --- Resolution -------------------------------------------------------------
- name: duplicate
  color: cfd3d7
  description: Already tracked elsewhere.
- name: invalid
  color: e4e669
  description: Not actionable as filed.
- name: wontfix
  color: ffffff
  description: Deliberately out of scope.
- name: question
  color: d876e3
  description: A question, not tracked work.
```

Note: `'008672'` is quoted because a bare `008672` is fine but `008672`-style values that look numeric are safest quoted; keep the quotes to avoid any YAML numeric coercion surprises.

- [ ] **Step 2: Verify it parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('OK')" .github/labels.yml`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/labels.yml
git commit -m "chore(repo): document the Norbert label taxonomy (labels.yml)"
```

---

## Task 2: Default pull request template

**Files:**
- Create: `.github/pull_request_template.md`

- [ ] **Step 1: Create `.github/pull_request_template.md`**

```markdown
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
```

- [ ] **Step 2: Verify it is present and non-empty**

Run: `test -s .github/pull_request_template.md && echo OK`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/pull_request_template.md
git commit -m "chore(repo): add default pull request template"
```

---

## Task 3: Catalog ("add a chip") pull request template

**Files:**
- Create: `.github/PULL_REQUEST_TEMPLATE/add-a-chip.md`

- [ ] **Step 1: Create `.github/PULL_REQUEST_TEMPLATE/add-a-chip.md`**

```markdown
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
```

- [ ] **Step 2: Verify it is present and non-empty**

Run: `test -s .github/PULL_REQUEST_TEMPLATE/add-a-chip.md && echo OK`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/PULL_REQUEST_TEMPLATE/add-a-chip.md
git commit -m "chore(repo): add add-a-chip pull request template"
```

---

## Task 4: Bug report issue form

**Files:**
- Create: `.github/ISSUE_TEMPLATE/bug_report.yml`

- [ ] **Step 1: Create `.github/ISSUE_TEMPLATE/bug_report.yml`**

```yaml
name: 🐞 Bug report
description: Report something Norbert does that it shouldn't.
title: "bug: <short description>"
labels: ["bug", "triage"]
body:
  - type: markdown
    attributes:
      value: |
        Thanks for the report. Norbert would rather know.

        **Please don't report security vulnerabilities here.** Use a private
        [security advisory](https://github.com/felipebalbi/norbert/security/advisories/new) instead.

        The more detail about your chip, your Pico de Gallo, and the exact
        command you ran, the faster this gets fixed.

  - type: checkboxes
    id: preflight
    attributes:
      label: Preflight checks
      options:
        - label: I searched existing issues and did not find a duplicate.
          required: true
        - label: I am on the latest released Norbert and Pico de Gallo firmware, or I explain below why not.
          required: true

  - type: dropdown
    id: area
    attributes:
      label: Affected area(s)
      description: Select all that apply.
      multiple: true
      options:
        - detect / SFDP parsing
        - catalog (names / manufacturers / fallback table)
        - erase
        - program
        - verify
        - read
        - protect / reset
        - doctor / test
        - CLI (flags, output)
        - transport (Pico de Gallo bridge)
        - documentation
        - CI / tooling
        - other / not sure
    validations:
      required: true

  - type: input
    id: chip
    attributes:
      label: Chip under test
      description: JEDEC id and part, if known. `norbert info` prints the id.
      placeholder: "EF 40 18 — Winbond W25Q128JV"
    validations:
      required: true

  - type: input
    id: bridge
    attributes:
      label: Pico de Gallo firmware version + hardware revision
      placeholder: "e.g. firmware-v0.6.0 (hw-rev1)"
    validations:
      required: true

  - type: input
    id: norbert-version
    attributes:
      label: Norbert version
      description: Output of `norbert --version`.
      placeholder: "norbert 0.1.0"
    validations:
      required: true

  - type: dropdown
    id: os
    attributes:
      label: Host operating system
      options:
        - Linux
        - macOS
        - Windows
        - other
    validations:
      required: true

  - type: input
    id: wiring
    attributes:
      label: Wiring / setup
      description: Bare chip on a clip vs. breakout/in-circuit; --reset, --freq, shared bus master.
      placeholder: "bare W25Q128 on a SOIC-8 clip, default 10 MHz, no --reset"
    validations:
      required: false

  - type: textarea
    id: description
    attributes:
      label: Describe the bug
      description: What happened, and what did you expect instead?
    validations:
      required: true

  - type: textarea
    id: reproduction
    attributes:
      label: Steps to reproduce
      description: The exact `norbert` command(s) that trigger it.
      render: shell
    validations:
      required: true

  - type: textarea
    id: actual
    attributes:
      label: Actual output / error
      description: Paste the full output. `--quiet` output or a backtrace helps.
      render: shell
    validations:
      required: true

  - type: textarea
    id: logs
    attributes:
      label: Logs, captures, evidence
      description: Logic-analyzer captures, photos of the wiring, RUST_LOG output — anything.
    validations:
      required: false

  - type: textarea
    id: regression
    attributes:
      label: Regression info
      description: Did this used to work? Last known-good version? `git bisect` results are gold.
    validations:
      required: false
```

- [ ] **Step 2: Verify it parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('OK')" .github/ISSUE_TEMPLATE/bug_report.yml`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/ISSUE_TEMPLATE/bug_report.yml
git commit -m "chore(repo): add bug report issue form"
```

---

## Task 5: Feature request issue form

**Files:**
- Create: `.github/ISSUE_TEMPLATE/feature_request.yml`

- [ ] **Step 1: Create `.github/ISSUE_TEMPLATE/feature_request.yml`**

```yaml
name: ✨ Feature request
description: Suggest a new command, flag, chip-handling improvement, or other enhancement.
title: "feat: <short description>"
labels: ["enhancement", "triage"]
body:
  - type: markdown
    attributes:
      value: |
        Thanks for the idea. Norbert prefers simple, predictable tools — the more
        concrete the proposal, the better.

        Before filing, skim the [README](https://github.com/felipebalbi/norbert#readme)
        and [HARDWARE-DEBUG.md](https://github.com/felipebalbi/norbert/blob/main/HARDWARE-DEBUG.md).

  - type: checkboxes
    id: preflight
    attributes:
      label: Preflight checks
      options:
        - label: I searched existing issues and did not find a duplicate.
          required: true
        - label: I confirmed this isn't already supported.
          required: true

  - type: dropdown
    id: area
    attributes:
      label: Affected area(s)
      description: Select all that apply.
      multiple: true
      options:
        - detect / SFDP parsing
        - catalog (names / manufacturers / fallback table)
        - erase
        - program
        - verify
        - read
        - protect / reset
        - doctor / test
        - CLI (flags, output)
        - transport (Pico de Gallo bridge)
        - documentation
        - CI / tooling
        - other / not sure
    validations:
      required: true

  - type: textarea
    id: problem
    attributes:
      label: What problem does this solve?
      description: A concrete scenario. "I'm trying to … and Norbert …"
    validations:
      required: true

  - type: textarea
    id: solution
    attributes:
      label: Proposed solution
      description: Sketch the command line, flag, or output you'd like to see.
      placeholder: "e.g. `norbert dump --range 0x0000..0x1000` prints a hex view without writing a file."
    validations:
      required: true

  - type: textarea
    id: alternatives
    attributes:
      label: Alternatives considered
      description: Other approaches, and any workaround you use today.
    validations:
      required: false

  - type: textarea
    id: context
    attributes:
      label: Additional context
      description: Datasheets, links to similar tools (flashrom, Bus Pirate), captures.
    validations:
      required: false

  - type: checkboxes
    id: contribution
    attributes:
      label: Are you willing to contribute?
      options:
        - label: I'd like to implement this and open a PR.
        - label: I can help test on hardware once someone implements it.
```

- [ ] **Step 2: Verify it parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('OK')" .github/ISSUE_TEMPLATE/feature_request.yml`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/ISSUE_TEMPLATE/feature_request.yml
git commit -m "chore(repo): add feature request issue form"
```

---

## Task 6: Issue template chooser config

**Files:**
- Create: `.github/ISSUE_TEMPLATE/config.yml`

- [ ] **Step 1: Create `.github/ISSUE_TEMPLATE/config.yml`**

```yaml
blank_issues_enabled: false
contact_links:
  - name: 📖 Documentation
    url: https://github.com/felipebalbi/norbert#readme
    about: Read the README and HARDWARE-DEBUG.md first — many questions are answered there.
  - name: 🔒 Security advisory
    url: https://github.com/felipebalbi/norbert/security/advisories/new
    about: Report a security vulnerability privately. Do NOT open a public issue.
```

Note: if GitHub Discussions gets enabled on the repo later, add a third
`contact_links` entry pointing at `.../norbert/discussions`. Omitted for now
(spec §6.3).

- [ ] **Step 2: Verify it parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('OK')" .github/ISSUE_TEMPLATE/config.yml`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/ISSUE_TEMPLATE/config.yml
git commit -m "chore(repo): add issue template chooser config"
```

---

## Task 7: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Create `.github/workflows/ci.yml`**

```yaml
# CI for Norbert. Runs on PRs and on pushes to main.
#
#   fmt        — rustfmt check
#   clippy     — lints as errors (stable + beta early-warning)
#   doc        — rustdoc builds cleanly (broken intra-doc links fail)
#   test       — cargo test on Linux, macOS, and Windows
#   msrv       — builds on the pinned minimum supported Rust version (1.97)
#   lockfile   — Cargo.lock is in sync with Cargo.toml
#   actionlint — the workflow files themselves are valid
name: ci

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.head_ref || github.run_id }}
  cancel-in-progress: true

jobs:
  fmt:
    name: stable / fmt
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install stable
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --check

  clippy:
    name: ${{ matrix.toolchain }} / clippy
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        toolchain: [stable, beta]
    steps:
      - uses: actions/checkout@v4
      - name: Install ${{ matrix.toolchain }}
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ matrix.toolchain }}
          components: clippy
      - run: cargo clippy --all-targets --locked -- -D warnings

  doc:
    name: stable / doc
    runs-on: ubuntu-latest
    env:
      RUSTDOCFLAGS: -D warnings
    steps:
      - uses: actions/checkout@v4
      - name: Install stable
        uses: dtolnay/rust-toolchain@stable
      - run: cargo doc --no-deps --locked

  test:
    name: ${{ matrix.os }} / stable / test
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4
      - name: Install stable
        uses: dtolnay/rust-toolchain@stable
      - run: cargo test --locked

  msrv:
    name: ubuntu / 1.97 / check
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install 1.97
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: "1.97"
      - run: cargo check --locked

  lockfile:
    name: ubuntu / stable / lockfile
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install stable
        uses: dtolnay/rust-toolchain@stable
      - run: cargo check --locked

  actionlint:
    name: ubuntu / actionlint
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: docker://rhysd/actionlint:latest
        with:
          args: -color
```

- [ ] **Step 2: Verify it parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('OK')" .github/workflows/ci.yml`
Expected: `OK`

- [ ] **Step 3: Lint the workflow with actionlint**

Run: `docker run --rm -v "$(pwd)":/repo -w /repo rhysd/actionlint:latest -color`
Expected: no output, exit code 0 (actionlint prints nothing when clean).
If Docker can't pull the image (offline), skip this step — the CI `actionlint` job will run it — but you MUST have completed Step 2.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add CI workflow (fmt, clippy, doc, test, msrv, lockfile, actionlint)"
```

---

## Task 8: Dependabot

**Files:**
- Create: `.github/dependabot.yml`

- [ ] **Step 1: Create `.github/dependabot.yml`**

```yaml
version: 2

updates:
  - package-ecosystem: cargo
    directory: /
    schedule:
      interval: weekly
      day: monday
    open-pull-requests-limit: 5
    labels:
      - dependencies
    commit-message:
      prefix: "chore(deps)"
      include: scope

  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
      day: monday
    open-pull-requests-limit: 5
    labels:
      - dependencies
    commit-message:
      prefix: "chore(ci)"
      include: scope
```

- [ ] **Step 2: Verify it parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('OK')" .github/dependabot.yml`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/dependabot.yml
git commit -m "ci: add dependabot for cargo and github-actions"
```

---

## Task 9: CONTRIBUTING guide

**Files:**
- Create: `CONTRIBUTING.md`

- [ ] **Step 1: Create `CONTRIBUTING.md`**

````markdown
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
````

- [ ] **Step 2: Verify it is present and non-empty**

Run: `test -s CONTRIBUTING.md && echo OK`
Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING guide"
```

---

## Task 10: Add a Contributing pointer to the README

**Files:**
- Modify: `README.md` (insert a `## Contributing` section immediately before `## License`)

- [ ] **Step 1: Confirm the current tail of the README**

Run: `tail -6 README.md`
Expected to include:
```
## License

MIT. See [LICENSE](LICENSE).
```

- [ ] **Step 2: Edit `README.md` — insert the Contributing section before License**

Replace this exact text:

```
## License

MIT. See [LICENSE](LICENSE).
```

with:

```
## Contributing

Found a chip Norbert doesn't know yet, or want to add a feature? See
[CONTRIBUTING.md](CONTRIBUTING.md). Adding a chip to the table is the easiest
place to start — there's a short PR template just for it.

## License

MIT. See [LICENSE](LICENSE).
```

- [ ] **Step 3: Verify the section landed and the file still ends with the license**

Run: `grep -n "^## Contributing" README.md && tail -3 README.md`
Expected: a line number for `## Contributing`, and the last lines still show `MIT. See [LICENSE](LICENSE).`

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs(readme): add a Contributing pointer"
```

---

## Task 11: Local CI dry-run (verify the workflow commands are green)

This runs the exact commands the new `ci.yml` will run, against the real repo,
to confirm the workflow won't be red on first push. It changes no files.

**Files:** none (verification only).

- [ ] **Step 1: fmt**

Run: `cargo fmt --check`
Expected: no output, exit 0.

- [ ] **Step 2: clippy (as CI runs it)**

Run: `cargo clippy --all-targets --locked -- -D warnings`
Expected: finishes with no warnings/errors, exit 0.

- [ ] **Step 3: tests**

Run: `cargo test --locked`
Expected: all tests pass (includes the catalog sortedness + erase-menu guards).

- [ ] **Step 4: docs**

Run: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked`
Expected: builds, exit 0.

- [ ] **Step 5: lockfile / MSRV check**

Run: `cargo check --locked`
Expected: exit 0. (Local stable is 1.97.1, so this also stands in for the MSRV
floor of 1.97.)

- [ ] **Step 6: Record the outcome — no commit**

If every command above passed, the CI workflow will be green on first push;
there is nothing to commit for this task.

If any command **fails**, it indicates a pre-existing issue in the Rust source
(out of scope for this plan, which only adds `.github/` config). Do NOT fix code
here. Stop and report exactly which command failed and its output, so the
maintainer can decide whether to fix the code first or adjust the CI command.

---

## Self-Review

**Spec coverage** (against `docs/superpowers/specs/2026-07-27-...-design.md`):
- §4 file inventory → Tasks 1–9 create every listed file; Task 10 wires the README pointer promised in §5.2/§10.
- §5.1 default PR template → Task 2. §5.2 catalog template → Task 3 (+ advertised link in Tasks 2, 9, 10).
- §6.1 bug form → Task 4. §6.2 feature form → Task 5. §6.3 config → Task 6 (Discussions link intentionally omitted, noted).
- §7 label taxonomy (all 16 labels, colors, descriptions) → Task 1.
- §8 CI (fmt/clippy stable+beta/doc/test cross-OS/msrv 1.97/lockfile/actionlint, no cargo-deny) → Task 7; validated in Task 11.
- §9 dependabot (cargo + github-actions, weekly, labels, commit prefixes) → Task 8.
- §10 CONTRIBUTING (scopes, golden path, dev setup, hardware norm, MSRV 1.97, template links) → Task 9.
- §11 deferred items → correctly NOT implemented (no labeler, no chip-request form, no label-sync, no component labels, no cargo-deny, no AI trailers, no CHANGELOG).
- §12 verification → Tasks' Step 2/3 checks + Task 11.

**Placeholder scan:** No TBD/TODO. The `<!-- … -->` and `0x__` markers inside the templates are intentional author-facing fill-ins, not plan placeholders. Every file step contains complete final content.

**Type/consistency:** Label names/colors in Task 1 match spec §7 exactly. Area option lists are identical across Tasks 2/4/5. The advertised template URL (`?template=add-a-chip.md&labels=new-chip`) is identical in Tasks 2, 9, and 10, and the filename matches the file created in Task 3. Commit scopes match the repo's Conventional-Commits norm. MSRV "1.97" is consistent across Tasks 7, 9, 11.
