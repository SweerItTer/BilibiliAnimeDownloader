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
./bin/bili download VIDEO_ID --list-resolutions --parts all --auth anonymous
./bin/bili download VIDEO_ID --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

Use `--list-resolutions` to view each part's quality IDs, actual dimensions, frame rate, codec and access status without starting a download. Select a listed Quality ID with `download VIDEO_ID --quality QUALITY_ID`; dimensions and Stream IDs are different values. Missing metadata or access reasons appear as unknown. Use `--parts 1,3-5` to choose parts, or `qualities VIDEO_ID --part 3` for one part. Add `--json` for machine-readable output. On Windows, use `.\bili.exe` in PowerShell with the same arguments.

The CLI prints completed file paths. It does not install or launch a video player. Use `./bin/bili --help` for other commands and options.

## Checking and installing updates

A CLI **built from this branch** supports checking and manually installing updates from a compatible versioned package. Use isolated installations for this unreleased updater. The published `2.0.0-alpha.6` package has no `update` subcommand:

```sh
./bin/bili update --check
./bin/bili update
```

Checks read a fixed HTTPS static index; archives download directly from the matching tagged GitHub Release. The client does not call the Releases API or require an API key. `--pre-release` also considers pre-release versions for that one command (`--pre-release=false` forces the stable channel even when the settings file opts in).

The updater uses the same versioned installation protocol on Linux and native Windows. In Windows PowerShell, run:

```powershell
.\bili.exe update --check
.\bili.exe update
```

For the first compatible package, extract the complete tree into a writable installation directory. Later updates locate that directory from the running executable, prepare and validate a separate version, then switch `CURRENT`; they do not overwrite the running payload or delete the previous version. Windows retains the canonical executable path when checking and starting deeply installed payloads, including paths with spaces or non-ASCII names. Filesystem path and component limits still apply.

`2.0.0-alpha.6` predates this protocol and cannot update itself: extract the first compatible package into a new directory by hand.

The shared protocol, descriptor format, configuration keys, verification results and known limits are documented in [the update protocol spec](.trellis/spec/bili_core/backend/update-protocol.md). `auto_update` there is a GUI-only preference; the CLI only ever checks unless you run `bili update` yourself.

## Login and privacy

Login is not saved by default. For a one-command QR login, use `--auth qr` on the command that needs it, such as `download`; the login ends when that command ends. To use a QR login in later commands, explicitly save it with `./bin/bili login qr --remember`, then use `--auth stored`. Secure OS storage is preferred; if unavailable, the CLI warns that saved login may be stored as plaintext in the local user configuration directory. Never save test-only credentials or put cookies in command text, logs, or reports.

Only download content you are authorized to save. The application does not bypass login, entitlements, regional restrictions, or DRM. If the website asks for additional verification, stop; the CLI reports an error.

## Limits and build

This branch adds the native desktop GUI; the Alpha release still targets Linux x86_64, and Windows release assets are not published yet. Aria2, verification WebView, search sorting, and corrupted-segment repair are not included. Stability, performance, and recovery have not been fully tested. FFmpeg is supplied as replaceable shared libraries; no FFmpeg executable or player is shipped or launched. The application is under the [MIT License with Commons Clause](LICENSE); FFmpeg and other third-party components have separate terms in the package notices.

See [BUILDING.md](BUILDING.md) for developer build requirements and commands.

## Community

See [Contributing](CONTRIBUTING.md) to report bugs, suggest features, or contribute changes. Please read the [Code of Conduct](CODE_OF_CONDUCT.md), [Security Policy](SECURITY.md), and [Accessibility statement](ACCESSIBILITY.md).
