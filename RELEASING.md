# Releasing

Releases are built by GitHub Actions on macOS, Linux (x86_64 and arm64) and Windows. Nobody needs a Mac, a Windows machine or a Rust toolchain to ship one.

## Cut a release

`main` is protected: changes arrive by pull request, so the version bump does too. `scripts/release.sh` splits the work around that.

1. Make sure `main` is green (the **CI** workflow).
2. `scripts/release.sh prepare 0.2.0` branches `release/v0.2.0` from `origin/main`, bumps the version in the root `Cargo.toml` (the only place it lives), commits `Release v0.2.0`, pushes the branch and opens a pull request.
3. Let CI pass on the pull request and merge it.
4. `scripts/release.sh tag 0.2.0` checks that `origin/main` now has that version and tags it. Review with `git show v0.2.0`, then start the release with `git push origin v0.2.0`.
5. Pushing the tag starts the **Release** workflow:
   1. It runs the whole **CI** workflow (format, clippy, tests on Linux, macOS and Windows) on the tagged commit. **Nothing is built unless every one of those is green.**
   2. It builds the installers on four runners and attaches them to a **draft** GitHub release with generated notes.
   3. It records a build provenance attestation for every installer.
6. Open the draft on the Releases page, check the notes and installers, and publish it.

A pre-release uses a numeric suffix: `scripts/release.sh prepare 0.2.0-1`. Any version with a hyphen is marked as a pre-release on GitHub. The suffix must be numeric (and at most 65535) because the Windows MSI installer cannot represent anything else.

## Dry run

*Actions → Release → Run workflow* runs the same gate and builds, but creates no release. The installers are uploaded as workflow artifacts instead. Use it to check the pipeline after changing it.

## What gets built

| Platform | Installers |
| --- | --- |
| macOS (universal: Apple Silicon and Intel) | `.dmg` |
| Linux x86_64 and arm64 | `.deb`, `.rpm`, `.AppImage` |
| Windows x86_64 | `.msi`, `.exe` (NSIS, per-user install) |

## Signing

Builds are **not notarised or Authenticode-signed**, so users see an "unverified developer" prompt on first launch (see the README for what to click).

- **macOS** gets an ad-hoc signature (`signingIdentity: "-"` in `tauri.conf.json`). That is what lets a downloaded Apple Silicon build launch at all, but it is not notarisation. To get rid of the prompt you need a paid Apple Developer account (99 USD a year). Then add these repository secrets and the workflow signs and notarises automatically: `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`. The workflow exports them only when they exist (GitHub hands a missing secret over as an empty string, which the Tauri bundler would otherwise try to import as a certificate). See [Tauri's macOS signing guide](https://v2.tauri.app/distribute/sign/macos/).
- **Windows** has no certificate configured. See [Tauri's Windows signing guide](https://v2.tauri.app/distribute/sign/windows/).

## Verifying a download

Every installer has a provenance attestation tying it to this repository's workflow and the tagged commit:

```bash
gh attestation verify <downloaded-installer> --repo Graloth/claude-usage
```

## Upkeep

- **Actions are pinned to commit SHAs**, with the version in a comment. Dependabot proposes updates weekly for actions, Cargo and npm dependencies.
- **The Rust version is pinned** in `rust-toolchain.toml`. Dependabot does not touch it: bump it by hand, and CI follows because it reads that file.
- **`Audit`** runs weekly and when a lockfile changes, scanning Rust and npm dependencies for known vulnerabilities.
- **Recommended repository settings** (Settings → Branches): protect `main` and require the **Check** jobs from CI to pass before merging. The release gate already re-runs them on the tag; branch protection keeps broken commits out of `main` in the first place.

## Repository settings

These live on GitHub, not in the repository, so they are written down here.

**Branch ruleset `main`** (Settings → Rules → Rulesets): target the default branch, require a pull request (0 approvals for now), require the three **Check** jobs from CI (`Check (ubuntu-24.04)`, `Check (macos-latest)`, `Check (windows-latest)`), restrict deletions, block force pushes, require linear history with squash merges. The bypass list holds **Repository admin, for pull requests only**: an admin can merge a pull request before its checks finish, but nobody can push to `main` directly.

**Tag ruleset `release tags`** (optional, same place, target type *tag*, pattern `v*`): restrict updates, restrict deletions and block force pushes, so a tag that has been released cannot be moved or removed. Bypass for **Repository admin, always**, so a mistyped tag can still be fixed. Adding *restrict creations* makes tags (and with them the release pipeline) something only admins can start, which matters once signing secrets exist.

**Immutable releases** (Settings → General → Releases): once a release is published, its tag and assets can no longer be changed or deleted. It only affects releases published after it is switched on, and it fits the draft-then-publish flow above.

When signing secrets are added, it may be worth putting them in a GitHub *environment* with required reviewers instead of repository secrets, so a workflow run cannot read them without approval.
