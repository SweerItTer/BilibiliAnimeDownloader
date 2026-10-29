# Changelog

## Unreleased

## [2.0.0-alpha.1] - 2026-10-29

### ✨ Features

- Add the Linux x86_64 `bili` CLI over reusable `bili_core`: search, downloads that merge video and audio, saved subtitle and danmaku files, and anonymous or explicitly remembered login.

### 🐞 Fixes

- Validate search and video responses, and preserve explicit access, verification, and subtitle/comment file failures.
- Apply URL safety checks to QR challenges and stream backups; preserve managed output integrity.
- Fix option validation and download-size checks in the basic package test.

### 📦 Build/Packaging

- Build trimmed FFmpeg 8.0.3 shared libraries from pinned source and package them with the CLI, matching source, checksums, and notices. No FFmpeg executable is shipped or invoked.

### 👷 CI/CD

- Use the same build, check, and publish steps for every version tag; the tag determines whether GitHub marks it as a prerelease.
- Reuse a pinned Ubuntu 22.04 image containing Python, Rust, and build tools for Linux builds. Rebuild the image when its inputs change and check it on a fresh runner before use.
- Check packages on separate machines without the build environment. Rust builds reject a toolchain version that differs from the checked-out source.
- Check the repository's current default branch before publishing, so renaming it does not break releases.

### 📝 Documentation

- Add concise CLI usage and separate native build instructions.

### Verified / Known Limitations

- Linux and Windows PR builds and extracted-package checks passed on Ubuntu 22.04, Ubuntu 24.04, and Windows 2022. This release ships only the Linux x86_64 CLI; its separate release checks must pass before files become public.
- Recovery stability and performance have not been fully tested.
- GUI, Aria2, verification WebView, search sorting, and corrupted-segment repair are not delivered.
- The application uses the MIT License with Commons Clause. FFmpeg is separately licensed LGPL-2.1-or-later; preserve its source, replacement rights, and notices. The application's restriction does not apply to FFmpeg.

### 💥 Breaking Changes

- Replace the legacy Python/Qt application with a Rust CLI-first workspace. The GUI is not delivered in this Alpha.
