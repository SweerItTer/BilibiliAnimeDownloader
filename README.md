# Bilibili Anime Downloader

English | [简体中文](README.zh-CN.md)

A Rust CLI for searching and downloading Bilibili videos. It can combine downloaded audio and video into a playable file and save subtitle and danmaku files. `bili_core` is reusable by Rust applications; `bili` is the command-line client.

## Download and status

Version `2.0.0-alpha.6` is a Linux x86_64 CLI prerelease. Its package and checksums are available on the [version's release page](https://github.com/SweerItTer/BilibiliAnimeDownloader/releases/tag/v2.0.0-alpha.6). Release CI validates packages offline; online checks are separate, explicitly authorized manual acceptance, and access to external services is not guaranteed. Older releases contain the former Python application. Windows packages are not included in this Alpha; macOS is out of scope.

Download the Linux package from that page, verify it using the published checksums, extract it, and keep its `bin/` and `lib/` directories together. Then run `./bin/bili --help` from the extracted package directory.

## Quick start

Replace `KEYWORD` with a search term and `VIDEO_ID` with an ID from the results (for example, `BV...` or `av` followed by its numeric ID). Search does not start a download automatically. These examples use anonymous access and do not read a saved login:

```sh
./bin/bili search "KEYWORD" --type video --page 1 --auth anonymous
./bin/bili download VIDEO_ID --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

The CLI prints completed file paths. It does not install or launch a video player. Use `./bin/bili --help` for other commands and options.

## Login and privacy

Login is not saved by default. For a one-command QR login, use `--auth qr` on the command that needs it, such as `download`; the login ends when that command ends. To use a QR login in later commands, explicitly save it with `./bin/bili login qr --remember`, then use `--auth stored`. Secure OS storage is preferred; if unavailable, the CLI warns that saved login may be stored as plaintext in the local user configuration directory. Never save test-only credentials or put cookies in command text, logs, or reports.

Only download content you are authorized to save. The application does not bypass login, entitlements, regional restrictions, or DRM. If the website asks for additional verification, stop; the CLI reports an error.

## Limits and build

This branch adds the native desktop GUI; the Alpha release still targets Linux x86_64, and Windows release assets are not published yet. Aria2, verification WebView, search sorting, and corrupted-segment repair are not included. Stability, performance, and recovery have not been fully tested. FFmpeg is supplied as replaceable shared libraries; no FFmpeg executable or player is shipped or launched. The application is under the [MIT License with Commons Clause](LICENSE); FFmpeg and other third-party components have separate terms in the package notices.

See [BUILDING.md](BUILDING.md) for developer build requirements and commands.

## Community

See [Contributing](CONTRIBUTING.md) to report bugs, suggest features, or contribute changes. Please read the [Code of Conduct](CODE_OF_CONDUCT.md), [Security Policy](SECURITY.md), and [Accessibility statement](ACCESSIBILITY.md).
