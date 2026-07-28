# Release Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a workflow-based release manager (release-plz + cross-OS binary builds) that maintains a release PR, bumps `Cargo.toml`, tags `vX.Y.Z`, creates a GitHub Release with binaries for 5 targets, and publishes to crates.io.

**Architecture:** Two GitHub Actions workflows. `release-plz.yml` (on push to `main`) owns the release-PR lifecycle, version bump, tag, Release, and crates.io publish via OIDC Trusted Publishing. `release-binaries.yml` (on tag `v*`) builds and uploads binaries for 5 targets using `taiki-e/upload-rust-binary-action`. A `release-plz.toml` pins the tag/release shape and a seed `CHANGELOG.md` gives release-plz a file to append to.

**Tech Stack:** GitHub Actions, release-plz, taiki-e/upload-rust-binary-action, rust-lang/crates-io-auth-action, actionlint.

**Verification note:** This plan configures CI/YAML, not application code, so there are no unit tests. Verification is `actionlint` (already a CI job) plus manual YAML/TOML inspection. The true end-to-end test is the first real release.

---

### Task 1: Seed CHANGELOG.md

**Files:**
- Create: `CHANGELOG.md`

- [ ] **Step 1: Create the seed changelog**

```markdown
# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Norbert is pre-1.0: breaking changes bump the minor version, fixes bump the patch.

## [Unreleased]
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): seed CHANGELOG for release-plz"
```

---

### Task 2: release-plz configuration

**Files:**
- Create: `release-plz.toml`

- [ ] **Step 1: Create the config**

```toml
# release-plz configuration.
#
# Norbert is a single-crate, pre-1.0 project. release-plz's default semver
# behavior already implements the pre-1.0 rule: on a 0.x crate a breaking change
# bumps the minor and any other change bumps the patch. We only pin the tag and
# release naming here so tags always look like "v0.1.0".

[workspace]
git_tag_name = "v{{ version }}"
git_release_name = "v{{ version }}"
git_release_body = "{{ changelog }}"
```

- [ ] **Step 2: Commit**

```bash
git add release-plz.toml
git commit -m "ci: add release-plz configuration"
```

---

### Task 3: release-plz workflow

**Files:**
- Create: `.github/workflows/release-plz.yml`

- [ ] **Step 1: Create the workflow**

```yaml
# Release manager for Norbert.
#
# On every push to main:
#   release-plz-release — if a Release PR was just merged, tag vX.Y.Z, create the
#                         GitHub Release, and publish to crates.io (OIDC token).
#   release-plz-pr      — open or update the "Release PR" that bumps Cargo.toml,
#                         Cargo.lock, and CHANGELOG.md based on Conventional Commits.
#
# Pre-1.0 semver is release-plz's default for 0.x crates: breaking -> minor,
# everything else -> patch.
#
# crates.io auth uses OIDC Trusted Publishing — no long-lived token in secrets.
# One-time setup: enable Trusted Publishing for felipebalbi/norbert on crates.io,
# and allow GitHub Actions to create PRs (Settings -> Actions -> General).
name: release-plz

on:
  push:
    branches: [main]

permissions:
  contents: write
  pull-requests: write
  id-token: write

concurrency:
  group: release-plz-${{ github.ref }}
  cancel-in-progress: false

jobs:
  release-plz-release:
    name: release-plz / release
    runs-on: ubuntu-latest
    if: ${{ github.repository == 'felipebalbi/norbert' }}
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      - name: Install stable
        uses: dtolnay/rust-toolchain@stable
      - name: Authenticate to crates.io
        uses: rust-lang/crates-io-auth-action@v1
        id: auth
      - name: Run release-plz release
        uses: release-plz/action@v0.5
        with:
          command: release
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}

  release-plz-pr:
    name: release-plz / pr
    runs-on: ubuntu-latest
    if: ${{ github.repository == 'felipebalbi/norbert' }}
    concurrency:
      group: release-plz-pr-${{ github.ref }}
      cancel-in-progress: false
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      - name: Install stable
        uses: dtolnay/rust-toolchain@stable
      - name: Run release-plz release-pr
        uses: release-plz/action@v0.5
        with:
          command: release-pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **Step 2: Validate with actionlint**

Run: `docker run --rm -v "${PWD}:/repo" -w /repo rhysd/actionlint:1.7.7 -color`
Expected: no errors reported for `release-plz.yml`. (If Docker is unavailable, run `actionlint` directly if installed, otherwise rely on the CI actionlint job.)

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release-plz.yml
git commit -m "ci: add release-plz workflow"
```

---

### Task 4: Cross-OS binary release workflow

**Files:**
- Create: `.github/workflows/release-binaries.yml`

- [ ] **Step 1: Create the workflow**

```yaml
# Build and attach norbert binaries to the GitHub Release.
#
# Triggered by the vX.Y.Z tag that release-plz pushes. Builds the `norbert`
# binary for five targets and uploads a per-target archive (.tar.gz on Unix,
# .zip on Windows) with SHA256 checksums to the matching Release.
#
# aarch64 Linux is cross-compiled; aarch64 Windows and aarch64 Linux legs are
# build-only since the runners can't execute those binaries natively.
name: release-binaries

on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  upload:
    name: ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    if: ${{ github.repository == 'felipebalbi/norbert' }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
          - target: x86_64-pc-windows-msvc
            os: windows-latest
          - target: aarch64-pc-windows-msvc
            os: windows-latest
          - target: aarch64-apple-darwin
            os: macos-latest
    steps:
      - uses: actions/checkout@v7
      - name: Install stable
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Build and upload norbert
        uses: taiki-e/upload-rust-binary-action@v1
        with:
          bin: norbert
          target: ${{ matrix.target }}
          archive: norbert-$tag-$target
          tar: unix
          zip: windows
          checksum: sha256
          token: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **Step 2: Validate with actionlint**

Run: `docker run --rm -v "${PWD}:/repo" -w /repo rhysd/actionlint:1.7.7 -color`
Expected: no errors reported for `release-binaries.yml`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release-binaries.yml
git commit -m "ci: add cross-OS binary release workflow"
```

---

### Task 5: Document the one-time manual setup

**Files:**
- Modify: `CONTRIBUTING.md` (append a "Releasing" section)

- [ ] **Step 1: Read the end of CONTRIBUTING.md to find the insertion point**

Run: `Read CONTRIBUTING.md` and note the last section so the new section is appended cleanly.

- [ ] **Step 2: Append a Releasing section**

Append this section to the end of `CONTRIBUTING.md`:

```markdown
## Releasing

Releases are automated by [release-plz](https://release-plz.dev). You don't cut
releases by hand.

**How it works**

1. Merges to `main` update an open **Release PR** that bumps `Cargo.toml`,
   `Cargo.lock`, and `CHANGELOG.md`. Norbert is pre-1.0, so breaking changes bump
   the **minor** version and fixes bump the **patch** version.
2. Merging the Release PR tags `vX.Y.Z`, creates a GitHub Release, and publishes
   to crates.io.
3. The tag triggers `release-binaries.yml`, which attaches `norbert` binaries for
   Linux (x86_64, aarch64), Windows (x86_64, aarch64), and macOS (Apple Silicon).

**One-time setup (maintainers)**

- **crates.io Trusted Publishing:** on crates.io → norbert → Settings → Trusted
  Publishing, add a GitHub Actions publisher for `felipebalbi/norbert` with
  workflow `release-plz.yml`. This lets CI publish without a stored token.
- **Allow Actions to create PRs:** repo Settings → Actions → General → Workflow
  permissions → enable "Allow GitHub Actions to create and approve pull
  requests".
```

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs(contributing): document the automated release process"
```

---

### Task 6: Final validation

- [ ] **Step 1: Confirm all files exist and lint clean**

Run: `docker run --rm -v "${PWD}:/repo" -w /repo rhysd/actionlint:1.7.7 -color`
Expected: exit 0, no errors across all workflow files.

- [ ] **Step 2: Confirm the file set**

Run: `git status --short && git log --oneline -6`
Expected: working tree clean; six commits present (changelog, config, two workflows, contributing docs, and this plan).
