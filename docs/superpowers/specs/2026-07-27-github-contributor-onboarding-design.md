# GitHub contributor onboarding (templates, labels, CI) — design

- Date: 2026-07-27
- Status: approved in brainstorm; pending spec review before planning
- Branch: `github-contributor-onboarding`

## 1. Goal & scope

Norbert has **no `.github/` directory** today: no CI, no PR/issue templates, no
dependabot, no contributor guide. This effort adds a complete, self-consistent
contributor-onboarding surface, modeled on the Pico de Gallo repo's conventions
but retargeted to Norbert (a single, unpublished host binary crate).

Two contribution shapes must feel different:

1. **Adding a chip / manufacturer to the catalog** — a small, low-risk,
   datasheet-driven change. The contributor should be guided to a short template
   and a maintainer applies a `new-chip` / `new-manufacturer` label during triage.
2. **A real feature / fix / refactor** — needs a description, an affected-area
   map, and self-reported hardware testing before it can merge.

Deliverables:

- Two PR templates (a default "full" one + one combined catalog one).
- Two issue forms (bug, feature) + an issue `config.yml`.
- A lean, documented label taxonomy as an at-rest source-of-truth file.
- A single CI workflow (`ci.yml`) covering fmt / clippy / doc / test / MSRV /
  lockfile / actionlint, with cross-OS test coverage.
- `dependabot.yml` for cargo + GitHub Actions.
- `CONTRIBUTING.md` documenting the whole flow, with a golden path for chips.

### Non-goals (explicitly deferred — see §11)

- No auto-labeler workflow. Labels are applied **manually by maintainers**.
- No `cargo-deny` / supply-chain policy tracking.
- No `add-chip-support.yml` issue form (chip *requests* without a patch).
- No label-sync workflow. `labels.yml` is a documented file only, not applied by CI.
- No AI-assist commit trailers (`Co-authored-by: Copilot`, `Assisted-by:`).
- No component labels (`sfdp`, `cli`, `flash-core`, `transport`) yet.
- No `CHANGELOG.md`.
- No change to any Rust source, the catalog, or command behavior.

## 2. Background (current state)

- **Project**: `norbert` — a Rust (edition 2024) CLI that programs SPI NOR flash
  over a Pico de Gallo v1.1 USB bridge. Single binary crate; `Cargo.lock` is
  committed. Depends on `pico-de-gallo-lib` / `pico-de-gallo-hal` `0.6` from
  crates.io, plus tokio/clap/anyhow/indicatif/embedded-hal(-async).
- **Remote**: `git@github.com:felipebalbi/norbert.git` → owner `felipebalbi`,
  repo `norbert`.
- **Commit style**: Conventional Commits with scopes already in use
  (`feat(catalog):`, `refactor(voice):`, `docs(readme):`, `test(cli):`, …).
- **The catalog** (`src/catalog.rs`) has three distinct contribution surfaces:
  1. `manufacturer(id: u8) -> Option<&str>` — JEDEC vendor ID → name (**new-manufacturer**).
  2. `CHIP_NAMES: &[NamedChip]` — pretty name by full 3-byte JEDEC ID, for
     SFDP-capable parts (**new-chip**, trivial).
  3. `FALLBACK_TABLE: &[KnownChip]` — parts with **no SFDP**, described from the
     datasheet: `page_size`, `address_bytes`, `capacity`, `erase_types`
     (**new-chip**, needs datasheet values).
  Both tables must stay **sorted ascending by JEDEC id** — a `#[cfg(test)]` guard
  (`tables_are_sorted_by_jedec_for_binary_search`) and an erase-menu guard
  (`every_fallback_row_has_a_usable_erase_menu`) already enforce this, so a bad
  row fails `cargo test` rather than panicking in the field.
- **Voice**: the README establishes a patient, dry, verification-obsessed
  persona, with the principle *"The code stays professional; only I get to have a
  personality."* Contributor docs get a **light** in-character touch.
- Existing tests (`catalog`, `cli`) are pure unit/integration tests that need no
  hardware, so `cargo test` is safe to run in CI on any OS.

## 3. Design decisions (resolved in brainstorm)

| # | Decision |
|---|---|
| Scope | Full contributor onboarding: templates + labels + CI + issue forms + dependabot + CONTRIBUTING + labels file. |
| Labeling | **Manual by maintainers.** No labeler workflow. Template links may carry `&labels=` as a convenience only. |
| CI rigor | Cross-OS everything **except** `cargo-deny`. |
| PR templates | Default "full" + one **combined** catalog template. |
| Voice | Light Norbert flavor (in-character intro lines; professional checklists). |
| MSRV | Pinned to **1.97**. |
| Chip-request issue form | Dropped for now. |
| Label sync workflow | Dropped; `labels.yml` is file-only. |
| AI-assist trailers | Not adopted. |

## 4. File inventory

```
.github/
├── pull_request_template.md              # default = full (features/fixes/refactors)
├── PULL_REQUEST_TEMPLATE/
│   └── add-a-chip.md                     # combined catalog template (link-selected)
├── ISSUE_TEMPLATE/
│   ├── bug_report.yml                    # form → labels: bug, triage
│   ├── feature_request.yml               # form → labels: enhancement, triage
│   └── config.yml                        # blank issues off; contact links
├── workflows/
│   └── ci.yml                            # fmt / clippy / doc / test / msrv / lockfile / actionlint
├── dependabot.yml                        # cargo + github-actions, weekly
└── labels.yml                            # labels-as-code (documented source of truth)
CONTRIBUTING.md
```

## 5. PR templates

### 5.1 Default — `.github/pull_request_template.md`

For features, fixes, refactors. Rendered for every PR that doesn't opt into the
catalog template via URL. HTML-comment guidance; a single light Norbert intro line.

Sections:

- **Summary** — what and why, one or two sentences.
- **Linked issues** — `Closes #…` / `Refs #…`.
- **Affected areas** (checkboxes): detect/SFDP · catalog · erase · program ·
  verify · read · protect/reset · doctor/test · cli · transport · docs · ci.
- **Testing performed** (checkboxes):
  - `cargo fmt --check`
  - `cargo clippy --all-targets --locked -- -D warnings`
  - `cargo test --locked`
  - **Tested on real hardware** — which Pico de Gallo + which flash chip; paste
    `detect` / `info` / `verify` output.
  - *or* "Not hardware-tested" with a one-line justification of why that's safe.
- **Docs** — README / `HARDWARE-DEBUG.md` / rustdoc updated, or N/A.
- **Checklist**:
  - Commits follow Conventional Commits with a correct scope.
  - `Cargo.lock` committed alongside any `Cargo.toml` change; ran `--locked`.
  - New public items have rustdoc.

### 5.2 Catalog — `.github/PULL_REQUEST_TEMPLATE/add-a-chip.md`

The simple path. Reached via
`https://github.com/felipebalbi/norbert/compare/main...<branch>?template=add-a-chip.md&labels=new-chip`
(advertised in CONTRIBUTING and README). The `&labels=` is a convenience for
collaborators with triage rights; maintainers remain the source of truth.

Light Norbert intro ("a chip that keeps notes"). Sections:

- **Which chip** — manufacturer + part number · JEDEC id as `mfr / type / cap`
  (e.g. `0xEF 0x40 0x18`) · **datasheet URL** (required).
- **What this adds** (tick all that apply):
  - New vendor ID in `manufacturer()` → *maintainer applies `new-manufacturer`*.
  - New pretty name in `CHIP_NAMES` (SFDP-capable part).
  - New `FALLBACK_TABLE` row (**part has no SFDP**).
- **Fallback-row datasheet values** (only if adding a `FALLBACK_TABLE` row):
  page size · address width (3- or 4-byte) · capacity · erase types as
  `size:opcode` (e.g. `65536:D8`).
- **Confirmations** (checkboxes):
  - Kept the tables **sorted ascending by JEDEC id** (a test enforces this).
  - `cargo test` passes (sortedness + erase-menu guards).
  - Tested on real hardware — paste `detect` / `info` output — *or* explicitly
    "datasheet values only, no chip in hand."

## 6. Issue templates

GitHub issue **forms** (YAML). Both auto-apply a type label + `triage`.

### 6.1 `bug_report.yml` — labels `bug`, `triage`; title `bug: <short description>`

- Markdown preamble: thanks + "do not report security issues here" pointer.
- **Preflight** (required checkboxes): searched for duplicates; on the latest
  released Norbert + Pico de Gallo firmware (or explain why not).
- **Affected area** (dropdown, multi): detect/SFDP · catalog · erase · program ·
  verify · read · protect/reset · doctor/test · cli · transport · docs · CI ·
  other/not sure.
- **Chip under test** (input): JEDEC id + part (e.g. `EF 40 18 — Winbond W25Q128JV`).
- **Pico de Gallo firmware version + hw-rev** (input).
- **Norbert version** (input) — `norbert --version`.
- **Host OS** (dropdown): Linux · macOS · Windows · other.
- **Wiring / setup** (input) — clip vs breakout, `--reset`, `--freq`, shared bus.
- **Describe the bug** (textarea, required) — include expected behavior.
- **Steps to reproduce** (textarea, required, `render: shell`) — exact `norbert` command.
- **Actual output / error** (textarea, required, `render: shell`) — prefer
  `--quiet` or verbose output; include panics/backtraces.
- **Logs / captures** (textarea) — logic-analyzer traces, photos, `RUST_LOG`.
- **Regression info** (textarea) — last known-good version; `git bisect` gold.

### 6.2 `feature_request.yml` — labels `enhancement`, `triage`; title `feat: <short description>`

- Markdown preamble: skim README/HARDWARE-DEBUG first.
- **Preflight** (required): searched issues/discussions; confirmed not already supported.
- **Affected area** (dropdown, multi) — same option set as bug.
- **Problem / scenario** (textarea, required) — concrete usage story.
- **Proposed solution** (textarea, required) — sketch the CLI shape / flag / output.
- **Alternatives considered** (textarea).
- **Additional context** (textarea) — datasheets, similar tools, links.
- **Willing to contribute?** (checkboxes) — implement it / help test on hardware.

### 6.3 `config.yml`

- `blank_issues_enabled: false`.
- Contact links:
  - 📖 README / `HARDWARE-DEBUG.md` — read before filing.
  - 🔒 Security advisory — private report; do not open a public issue.
  - (GitHub Discussions link included **only if** the repo enables Discussions;
    otherwise omitted. Confirm at implementation time.)

## 7. Label taxonomy

Lean by design. `.github/labels.yml` is the documented source of truth
(name / color / description), not applied by any workflow.

| Group | Label | Color | Description |
|---|---|---|---|
| Type | `bug` | `d73a4a` | Something is broken. |
| Type | `enhancement` | `a2eeef` | New capability or improvement. |
| Type | `documentation` | `0075ca` | README, HARDWARE-DEBUG, rustdoc. |
| Type | `refactor` | `fbca04` | Internal change, no behavior change. |
| Type | `dependencies` | `0366d6` | Dependency / toolchain bumps (dependabot). |
| Catalog | `new-chip` | `0e8a16` | Adds a chip to `CHIP_NAMES` or `FALLBACK_TABLE`. |
| Catalog | `new-manufacturer` | `1d76db` | Adds a JEDEC vendor ID to `manufacturer()`. |
| Workflow | `triage` | `ededed` | New issue, awaiting maintainer. |
| Workflow | `needs-hardware-test` | `b60205` | Unverified on real silicon; blocks merge. |
| Workflow | `needs-datasheet` | `fef2c0` | Catalog entry lacks a datasheet citation. |
| Workflow | `good first issue` | `7057ff` | Approachable — e.g. adding a known chip. |
| Workflow | `help wanted` | `008672` | Maintainer would welcome help here. |
| Resolution | `duplicate` | `cfd3d7` | Already tracked elsewhere. |
| Resolution | `invalid` | `e4e669` | Not actionable as filed. |
| Resolution | `wontfix` | `ffffff` | Deliberately out of scope. |
| Resolution | `question` | `d876e3` | A question, not tracked work. |

`needs-hardware-test` is the mechanism for the "features require further testing"
intent: a maintainer applies it to anything unverified on silicon and treats it
as a merge blocker. Adding a *known* chip is the canonical `good first issue`.

## 8. CI — `.github/workflows/ci.yml`

Single workflow, multiple jobs. Header:

- `name: ci`
- `on: { push: { branches: [main] }, pull_request: {} }`
- `permissions: { contents: read }`
- Concurrency: `group: ${{ github.workflow }}-${{ github.head_ref || github.run_id }}`,
  `cancel-in-progress: true`.
- Toolchains via `dtolnay/rust-toolchain`; checkout via `actions/checkout` (no
  submodules — Norbert has none).

Jobs:

| Job | Command | Matrix / runner |
|---|---|---|
| `fmt` | `cargo fmt --check` | stable · ubuntu-latest |
| `clippy` | `cargo clippy --all-targets --locked -- -D warnings` | toolchain: [stable, beta] · ubuntu-latest · `fail-fast: false` |
| `doc` | `cargo doc --no-deps` with `RUSTDOCFLAGS: -D warnings` | stable · ubuntu-latest |
| `test` | `cargo test --locked` | os: [ubuntu-latest, macos-latest, windows-latest] · stable · `fail-fast: false` |
| `msrv` | `cargo check --locked` | toolchain `1.97` · ubuntu-latest |
| `lockfile` | `cargo check --locked` | stable · ubuntu-latest |
| `actionlint` | `rhysd/actionlint` (docker action) | ubuntu-latest |

Notes:

- No `cargo-deny`, no `semver-checks` (binary crate, not published), no
  `cargo-hack`/feature-powerset (no features defined), no firmware/no_std targets.
- The `beta` clippy lane is early-warning for new lints; keep `fail-fast: false`
  so a beta-only lint doesn't mask the stable result.
- MSRV `1.97` is the pinned floor. If `cargo check --locked` fails there because a
  dependency needs newer, bump the pin and note it in CONTRIBUTING.
- `lockfile` + the `--locked` flags across jobs catch a stale `Cargo.lock`.

## 9. dependabot — `.github/dependabot.yml`

- `version: 2`.
- `cargo`, directory `/`, weekly (Monday), labels `dependencies`, commit prefix
  `chore(deps)`, `open-pull-requests-limit: 5`.
- `github-actions`, directory `/`, weekly (Monday), labels `dependencies`, commit
  prefix `chore(ci)`, `open-pull-requests-limit: 5`.

## 10. CONTRIBUTING.md

Top-level file. Light Norbert intro line, then:

- **Conventional Commits** — format + the canonical scope list: `catalog`,
  `sfdp`, `flash`, `device`, `profile`, `cli`, `ui`/`voice`, `commands`, `docs`,
  `ci`, `deps`, `repo`.
- **Adding a chip (the golden path)**:
  1. Read the chip's JEDEC id — `norbert info` prints `mfr / type / cap`.
  2. Cite the datasheet.
  3. Decide: SFDP-capable → add a `CHIP_NAMES` name (+ vendor ID if new);
     no SFDP → add a `FALLBACK_TABLE` row with datasheet values.
  4. Keep both tables **sorted ascending by JEDEC id**.
  5. `cargo test` (the sortedness + erase-menu guards must pass).
  6. Open the PR via the `add-a-chip` template link.
- **Dev setup** — `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
  warnings`, `cargo fmt`.
- **Hardware testing** — what "tested on real hardware" means; the honesty norm
  ("Norbert doesn't guess" — say so when a change is datasheet-only / untested).
- **Templates** — links to the PR templates and issue forms.
- **MSRV** — note the pinned `1.97` floor.

## 11. Deferred / future work

Captured so nothing is lost; all out of scope for this pass per §1 non-goals:

- Diff-based **auto-labeler** workflow (path + hunk inspection) if manual triage
  becomes a burden.
- `add-chip-support.yml` issue form for chip requests without a patch.
- `label-sync` workflow to apply `labels.yml` to the repo automatically.
- Component labels (`sfdp`, `cli`, `flash-core`, `transport`).
- `cargo-deny` supply-chain policy (explicitly declined).
- AI-assist commit trailers.
- `CHANGELOG.md` (Keep a Changelog).

## 12. Verification

How we confirm the deliverable is correct before merging:

- `actionlint` passes on `ci.yml` (also runs as a CI job).
- YAML forms parse (valid `ISSUE_TEMPLATE/*.yml`, `dependabot.yml`, `labels.yml`);
  GitHub renders the issue forms without schema errors.
- The `?template=add-a-chip.md` URL resolves to the catalog template.
- A dry-run read-through: opening a PR shows the default template; the advertised
  link shows the catalog template.
- `ci.yml` jobs are green on a scratch PR (fmt/clippy/doc/test/msrv/lockfile/actionlint).
```