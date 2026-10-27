# Changelog

## Unreleased

## [2.0.0-alpha.1] - 2026-10-27

### ✨ Features

- Deliver the `bili` Linux x86_64 CLI over the reusable `bili_core` library, with anonymous or explicit remembered-login workflows, video search, downloads, automatic local remux, subtitle and danmaku sidecars.

### 🐞 Fixes

- Accept `account.bilibili.com` for QR challenges while retaining HTTPS/443 and URL safety checks.
- Try the primary stream URL, then the first API-provided backup that passes the existing HTTPS/CDN/default-port/no-credentials policy.
- Validate search and media responses as typed payloads; access and verification challenges remain explicit failures.
- Preserve managed output integrity and report requested subtitle/danmaku component failures instead of treating them as successful empty output.

### 📦 Build/Packaging

- Build the trimmed FFmpeg 8.0.3 shared libraries from pinned source; package the Linux CLI with package-relative libraries, matching source, checksums and notices. No FFmpeg executable is shipped or invoked.

### 👷 CI/CD

- Tagged `v2.*-alpha.*` candidates build on Ubuntu 22.04, pass offline and fixed anonymous-media verification on a separate Ubuntu 22.04 runner, then publish the same checksummed runtime/source assets as a prerelease only after draft-asset readback succeeds.

### 📝 Documentation

- Document CLI use, authentication/privacy, output sidecars, package licensing and native source-build requirements.

### Verified / Known Limitations

- Alpha assets are released only after the tagged build and independent VM qualification pipeline passes; this workflow has not yet run for a merged tag.
- Local WSL/glibc 2.43 results are not Ubuntu 22.04/glibc 2.35 evidence. Resume/recovery stability and performance remain unverified.
- Resume/jobs and recovery behavior are implemented, but cross-process recovery and stability/performance have not been verified for this Alpha.
- GUI, Aria2, WebView verification UI, search sorting and corrupted-segment repair are not delivered in this Alpha.
- The application remains under the MIT License with Commons Clause. FFmpeg remains separately licensed LGPL-2.1-or-later; preserve its source, replacement rights and notices. The application's restriction does not apply to FFmpeg.

### 💥 Breaking Changes

- Replace the legacy Python/Qt GUI application and scripts with a Rust CLI-first workspace. The GUI is not delivered in this Alpha.
