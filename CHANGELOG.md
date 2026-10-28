# Changelog

## Unreleased

### 🐞 Fixes

- Check basic package-test options before use and read its recorded download size correctly.

### 👷 CI/CD

- Run every `v*` tag through the same build, test, and publish steps. The tag decides whether the release is a prerelease and the latest release; pull-request checks never publish.

## [2.0.0-alpha.1] - 2026-10-27

### ✨ Features

- Add the Linux x86_64 `bili` CLI over reusable `bili_core`: search, downloads that merge video and audio, saved subtitle and danmaku files, and anonymous or explicitly remembered login.

### 🐞 Fixes

- Validate search and video responses, and preserve explicit access, verification, and subtitle/comment file failures.
- Apply URL safety checks to QR challenges and stream backups; preserve managed output integrity.

### 📦 Build/Packaging

- Build trimmed FFmpeg 8.0.3 shared libraries from pinned source and package them with the CLI, matching source, checksums, and notices. No FFmpeg executable is shipped or invoked.

### 👷 CI/CD

- Prepare the Linux CLI release pipeline; no Rust CLI package has been published as of this changelog entry.

### 📝 Documentation

- Add concise CLI usage and separate native build instructions.

### Verified / Known Limitations

- Local WSL results have not been tested on the Ubuntu release system.
- Recovery stability and performance have not been fully tested.
- GUI, Aria2, verification WebView, search sorting, and corrupted-segment repair are not delivered.
- The application uses the MIT License with Commons Clause. FFmpeg is separately licensed LGPL-2.1-or-later; preserve its source, replacement rights, and notices. The application's restriction does not apply to FFmpeg.

### 💥 Breaking Changes

- Replace the legacy Python/Qt application with a Rust CLI-first workspace. The GUI is not delivered in this Alpha.
