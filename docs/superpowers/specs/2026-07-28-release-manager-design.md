# Release Manager Design

**Date:** 2026-07-28
**Status:** Approved

## Problem

Norbert has no automated release process. Cutting a release today means manually
bumping `Cargo.toml`, tagging, drafting a GitHub Release, building binaries for
each OS by hand, and running `cargo publish`. This is error-prone and
inconsistent. We want a workflow-based release manager that:

1. Maintains a PR tracking the next version bump (major/minor/patch).
2. Uses pre-1.0 semver: breaking changes bump **minor**, fixes bump **patch**.
3. Automatically updates the `Cargo.toml` version.
4. Automatically creates a `vX.Y.Z`-format tag.
5. Automatically creates a GitHub Release with `norbert` binaries for the major OSes.
6. Automatically publishes to crates.io after the tag.

## Approach

Two GitHub Actions workflows, split by responsibility, plus a `release-plz`
configuration file and a seed `CHANGELOG.md`.

- **release-plz** (Rust-native, Cargo-aware) owns the release-PR lifecycle,
  version bumping, tagging, GitHub Release creation, and crates.io publish.
- **taiki-e/upload-rust-binary-action** builds and attaches cross-OS binaries to
  the Release, triggered by the tag release-plz pushes.

crates.io authentication uses OIDC **Trusted Publishing** (no long-lived token
stored in the repo) via `rust-lang/crates-io-auth-action`.

## Components

### 1. `.github/workflows/release-plz.yml` (trigger: push to `main`)

Two independent jobs, guarded so they never run on forks.

**Job `release-plz-release`** — runs `release-plz release`. On a merge that lands
a version bump, it:
- creates the `vX.Y.Z` git tag,
- creates the GitHub Release,
- publishes to crates.io using an OIDC-minted token.

**Job `release-plz-pr`** — runs `release-plz release-pr`. On every push to
`main`, it opens or updates the Release PR that bumps `Cargo.toml`, refreshes
`Cargo.lock`, and updates `CHANGELOG.md`.

Concurrency: a `release-plz-${{ github.ref }}` group so overlapping pushes don't
race.

**Permissions:**
- `contents: write` (tags, releases, commits to the PR branch)
- `pull-requests: write` (open/update the Release PR)
- `id-token: write` (OIDC for crates.io Trusted Publishing)

Auth wiring: the release job calls `rust-lang/crates-io-auth-action` to mint a
short-lived token, then passes it to release-plz via `CARGO_REGISTRY_TOKEN`.
`GITHUB_TOKEN` is provided to release-plz for tag/release/PR operations.

### 2. `.github/workflows/release-binaries.yml` (trigger: push tag `v*`)

A matrix build using `taiki-e/upload-rust-binary-action@v1` that compiles the
`norbert` binary and uploads an archive to the Release matching the pushed tag.

| OS label | Target triple | Runner | Archive |
|---|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `.tar.gz` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` | `ubuntu-latest` | `.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `windows-latest` | `.zip` |
| Windows aarch64 | `aarch64-pc-windows-msvc` | `windows-latest` | `.zip` |
| macOS Apple Silicon | `aarch64-apple-darwin` | `macos-latest` | `.tar.gz` |

The action handles cross-compilation (installing `cross` for the Linux aarch64
target), archive naming (`norbert-<tag>-<target>.<ext>`), and SHA256 checksums.

**Permissions:** `contents: write` (upload assets to the Release).

The Windows aarch64 and Linux aarch64 legs build-only (no test), since the
runners can't execute those binaries natively.

### 3. `release-plz.toml`

```toml
[workspace]
# Explicit pre-1.0 semver: on 0.x, cargo-semver rules already map breaking ->
# minor and everything else -> patch. We keep the default and pin the tag shape.
git_tag_name = "v{{version}}"
git_release_name = "v{{version}}"
git_release_body = "{{changelog}}"
```

release-plz's default semver behavior already implements the pre-1.0 rule
(item 2): for a `0.x.y` crate a breaking change bumps the minor and a fix bumps
the patch. No override is needed beyond documenting it here.

### 4. `CHANGELOG.md`

A seed changelog with a `## [Unreleased]` header so release-plz has a file to
append to on the first run.

## Data Flow

```
push/merge to main
   │
   ├─ release-plz-pr: compute next version from Conventional Commits
   │                  → open/update "Release PR" (Cargo.toml + Cargo.lock + CHANGELOG)
   │
   └─ (when Release PR merged)
      release-plz-release: tag vX.Y.Z → GitHub Release → cargo publish (OIDC)
                                   │
                                   └─ tag push (v*) triggers release-binaries.yml
                                          → matrix build → upload archives to Release
```

## Error Handling

- **Fork safety:** both release-plz jobs guard on
  `github.repository == 'felipebalbi/norbert'` so forks never attempt to tag or
  publish.
- **Idempotent publish:** release-plz skips crates.io publish if the version
  already exists on the registry, so re-runs are safe.
- **Binary build failure:** the matrix uses `fail-fast: false` so one target's
  failure doesn't cancel the others; a failed leg leaves the Release intact and
  can be retried by re-running the job.
- **PR-creation permission:** GitHub Actions must be allowed to create PRs
  (repo Settings → Actions → General). Without it, `release-plz-pr` fails
  loudly.

## Testing / Verification

- `actionlint` (already in CI) validates both new workflow files.
- Dry validation: confirm `release-plz` config parses via
  `release-plz release-pr --dry-run` locally if the tool is installed.
- First real release is the end-to-end test: merge the Release PR and confirm
  tag, Release, five binary archives, and the crates.io publish all appear.

## One-Time Manual Setup (documented, not automated)

1. **crates.io Trusted Publishing:** on crates.io → norbert → Settings →
   Trusted Publishing, add a GitHub Actions publisher for
   `felipebalbi/norbert`, workflow `release-plz.yml`.
2. **Allow Actions to create PRs:** repo Settings → Actions → General →
   Workflow permissions → check "Allow GitHub Actions to create and approve
   pull requests".

## Out of Scope (YAGNI)

- macOS Intel (`x86_64-apple-darwin`) binaries — not requested.
- Homebrew tap / package-manager distribution.
- Signed / notarized binaries.
- Multi-crate workspace release orchestration (single crate).
