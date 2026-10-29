# Changelog

## Unreleased

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
