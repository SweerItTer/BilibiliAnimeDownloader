# Changelog

## Unreleased

### ✨ Features

- Add a shared update protocol in `bili_core` and the `bili update [--check] [--pre-release[=true|false]]` command on top of it: per-product release descriptors, SemVer-precedence selection across stable and pre-release channels, verified archive download, a versioned install layout with an atomic `CURRENT` pointer, and a throttled reminder that announces each available version at most once per channel.
- Discover updates through one bounded static HTTPS catalog shared by CLI, GUI and TUI; retain product/target isolation and stable/pre-release SemVer precedence without client Releases API calls or API keys.

### 🐞 Fixes

- Recheck the selected version under the installation lock to prevent stale updaters from activating an older payload; validate staged payloads against the candidate identity.
- Isolate staged doctor checks from the developer working directory and DLL search path.
- Restore ordinary Windows workspace checks while retaining static CRT enforcement for shipping bootstraps; correct cross-platform bootstrap tests and updater Clippy failures.
- Accept Linux update archives whose file symlinks precede their targets; retain final checks for missing, cyclic, directory and out-of-tree link targets.

### 💥 Breaking Changes

- The stable package entry remains unchanged, but the CLI and native runtime now live under `versions/<id>/`, selected by `CURRENT`. `2.0.0-alpha.6` cannot update to this layout; manually extract the first compatible package into a new directory rather than overwriting an Alpha 6 installation. No automatic migration is provided.

### 📝 Documentation

- Add community conduct, contribution, security and accessibility guidance, plus issue forms and a pull request template.
- Document the shared update protocol, the GUI validation boundary and installation verification limits.

### 📦 Build/Packaging

- Narrow Tokio features while keeping `rt-multi-thread` enabled for the `bili_core` SDK consumer; move only test-only `net` to dev-dependencies. Remove unused GUI `image` and Tokio dependencies while retaining JPEG, PNG, WebP, GIF, and eframe backends. This removes 21 lockfile packages with no dependency upgrades.
- Stage Linux and Windows CLI downloads with their matching FFmpeg source and product-only checksums; leave the separately developed Windows GUI unmodified and unpublished.
- Emit a `bili-update-cli-<target>.json` descriptor beside each CLI package, check it against the packaged CLI's own `--json doctor` identity, and cover it in the product checksums.

### 👷 CI/CD

- Route pull requests through Docs, CLI (`core-cli`, `bili_core`, and `core-bootstrap`), GUI, or full-workspace checks with a stable `CI result` gate; keep tag releases on the full offline qualification path.
- Reuse validated native FFmpeg, synthetic-corpus/tool, and Rust build caches without skipping current Rust builds or checks.
- Fix the truncated Linux build-container digest so the matrix job can pull the published CI image.
- Retry the pinned FFmpeg source download on transient connection failures; the SHA-256 pin still rejects any bad bytes.
- Archive successful CI runs' non-product handoffs in GHCR with digest-pinned readback before deleting Actions copies. Use a trusted default-branch workflow; preserve failed-run inputs and both SDK verification modes without uploading the unused rlib SDK archive.
- Rebuild and deploy the complete static update catalog after qualified release publication, with an index-only manual entry in the same workflow; preserve ordinary tag/main release gates and the six-asset release bundle.

## [2.0.0-alpha.6] - 2026-10-29

### ✨ Features

- Add the Linux x86_64 `bili` CLI over reusable `bili_core`: search, downloads that merge video and audio, saved subtitle and danmaku files, and anonymous or explicitly remembered login.

### 🐞 Fixes

- Validate search and video responses, and preserve explicit access, verification, and subtitle/comment file failures.
- Include HTTP status codes and distinguish connection failures from timeouts without logging private request data.
- Apply URL safety checks to QR challenges and stream backups; preserve managed output integrity.
- Fix option validation and download-size checks in the basic package test.
- Trust only the checked-out repository inside the release container so Git can validate the tag and branch.
- Include the synthetic test video's checksum manifest in the bundle sent to the independent package checker.

### 📦 Build/Packaging

- Build trimmed FFmpeg 8.0.3 shared libraries from pinned source and package them with the CLI, matching source, checksums, and notices. No FFmpeg executable is shipped or invoked.

### 👷 CI/CD

- Use the same build, offline qualification, and publish workflow for every version tag; the tag controls only GitHub prerelease/latest metadata.
- Require offline package, dependency, native-library, license, isolated CLI, and synthetic managed-remux checks before publication. Online media checks are separate manual acceptance and do not gate publication; external-service access is not guaranteed.
- Configure Python 3.13 explicitly in the publisher because its receipt verifier requires `tomllib`; do not rely on the Ubuntu runner's default Python.
- Reuse a pinned Ubuntu 22.04 image containing Python, Rust, and build tools for Linux builds. Rebuild the image when its inputs change and check it on a fresh runner before use.
- Check packages on separate machines without the build environment. Rust builds reject a toolchain version that differs from the checked-out source.
- Check the repository's current default branch before publishing, so renaming it does not break releases.
- Upload verification reports from the VM that creates them, including failed checks; failed offline checks still block publication.

### 📝 Documentation

- Add concise CLI usage and separate native build instructions.

### Verified / Known Limitations

- Local alpha.6 package validation and offline merge passed. The actual offline gate and publisher verification/metadata scripts passed locally; hosted CI repeats the required checks before publication.
- The diagnostics changes passed Rust formatting, workspace all-target Clippy, and 105 Rust tests (1 ignored). Packaging scripts passed 12 Python tests.
- Recovery stability and performance have not been fully tested.
- GUI, Aria2, verification WebView, search sorting, and corrupted-segment repair are not delivered.
- The application uses the MIT License with Commons Clause. FFmpeg is separately licensed LGPL-2.1-or-later; preserve its source, replacement rights, and notices. The application's restriction does not apply to FFmpeg.

### 💥 Breaking Changes

- Replace the legacy Python/Qt application with a Rust CLI-first workspace. The GUI is not delivered in this Alpha.

## [2.0.0-alpha.5] - 2026-10-29

This attempt was canceled during build after local preflight reproduced the publisher's missing `tomllib` under Ubuntu 22.04's default Python 3.10. No draft or public files were created. The tag is retained.

## [2.0.0-alpha.4] - 2026-10-29

The package built and passed offline checks, but anonymous API access from the GitHub-hosted verifier returned HTTP 412 and blocked publication. The identical candidate passed anonymous discovery, download and merge locally. The tag is retained; no public files were published.

## [2.0.0-alpha.3] - 2026-10-29

The CLI package was built, but the independent anonymous `info` check failed with a generic `Transport` error. No public files were published; the precise remote cause was not established. Its tag is retained.

## [2.0.0-alpha.2] - 2026-10-29

The CLI package built successfully, but the independent offline check stopped because its test video's manifest was missing. No application files were published. Its tag is retained; alpha.3 includes the manifest.

## [2.0.0-alpha.1] - 2026-10-29

The first release attempt stopped at Git's repository-ownership check before building a package. No application files were published. Its tag is retained; alpha.2 contains the fix.
