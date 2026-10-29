# Changelog

## Unreleased

## [2.0.0-alpha.4] - 2026-10-29

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

- Use the same build, check, and publish steps for every version tag; the tag determines whether GitHub marks it as a prerelease.
- Reuse a pinned Ubuntu 22.04 image containing Python, Rust, and build tools for Linux builds. Rebuild the image when its inputs change and check it on a fresh runner before use.
- Check packages on separate machines without the build environment. Rust builds reject a toolchain version that differs from the checked-out source.
- Check the repository's current default branch before publishing, so renaming it does not break releases.
- Upload verification reports from the VM that creates them, including failed checks; failed checks still block publication.

### 📝 Documentation

- Add concise CLI usage and separate native build instructions.

### Verified / Known Limitations

- Local checks passed: Rust formatting, workspace all-target Clippy with warnings denied, 105 Rust tests passed (1 ignored), 12 Python script tests, and actionlint for the release and CLI package workflows.
- The source-built CLI, independent `rlib` consumer, and extracted-package offline and anonymous basic download/merge checks passed locally.
- Hosted release checks run independently and must pass before files are published; local checks do not replace that gate.
- Recovery stability and performance have not been fully tested.
- GUI, Aria2, verification WebView, search sorting, and corrupted-segment repair are not delivered.
- The application uses the MIT License with Commons Clause. FFmpeg is separately licensed LGPL-2.1-or-later; preserve its source, replacement rights, and notices. The application's restriction does not apply to FFmpeg.

### 💥 Breaking Changes

- Replace the legacy Python/Qt application with a Rust CLI-first workspace. The GUI is not delivered in this Alpha.

## [2.0.0-alpha.3] - 2026-10-29

The CLI package was built, but the independent anonymous `info` check failed with a generic `Transport` error. No public files were published; the precise remote cause was not established. Its tag is retained.

## [2.0.0-alpha.2] - 2026-10-29

The CLI package built successfully, but the independent offline check stopped because its test video's manifest was missing. No application files were published. Its tag is retained; alpha.3 includes the manifest.

## [2.0.0-alpha.1] - 2026-10-29

The first release attempt stopped at Git's repository-ownership check before building a package. No application files were published. Its tag is retained; alpha.2 contains the fix.
