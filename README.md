# Bilibili Anime Downloader

English | [简体中文](README.zh-CN.md)

A Rust CLI for searching and downloading Bilibili videos. It can combine downloaded audio and video into a playable file and save subtitle and danmaku files. `bili_core` is reusable by Rust applications; `bili` is the command-line client.

- [CLI guide and all commands](CLI.md): login, search, quality selection, downloads, recovery, cleanup, configuration and updates.
- [Native GUI build and update instructions](gui/README.md).
- [Developer build setup](BUILDING.md).

## Download and status

Version `2.0.0-alpha.6` is a Linux x86_64 CLI prerelease. Its package and checksums are available on the [version's release page](https://github.com/SweerItTer/BilibiliAnimeDownloader/releases/tag/v2.0.0-alpha.6). Release CI validates packages offline; online checks are separate, explicitly authorized manual acceptance, and access to external services is not guaranteed. Older releases contain the former Python application. Windows packages are not included in this Alpha; macOS is out of scope.

Download the Linux package from that page, verify it using the published checksums, extract it, and keep its `bin/` and `lib/` directories together. Then run `./bin/bili --help` from the extracted package directory.

## Quick start

Replace `KEYWORD` with a search term and `VIDEO_ID` with an ID from the results (for example, `BV...` or `av` followed by its numeric ID). Search does not start a download automatically. These examples use anonymous access and do not read a saved login:

```sh
./bin/bili search "KEYWORD" --type video --page 1 --auth anonymous
./bin/bili qualities VIDEO_ID --auth anonymous
./bin/bili download VIDEO_ID --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

`qualities` inspects one part without starting a download; use `--part 3` to inspect its original part number 3. Select a listed Quality ID with `download VIDEO_ID --quality QUALITY_ID`; dimensions and Stream IDs are different values. Use `--parts 1,3-5` to choose download parts. Current source also provides `download VIDEO_ID --list-resolutions` for a multi-part catalog; Alpha 6 does not. Add `--json` for machine-readable output. On Windows source builds, use `.\bili.exe` in PowerShell with the same arguments.

The CLI prints completed file paths. It does not install or launch a video player. Continue with the [full CLI guide](CLI.md), or run `./bin/bili download --help` for a command's options. Source builds also support [video search sorting](CLI.md#search); the published Alpha 6 package does not.

## Checking and installing updates

A CLI **built from current source** supports checking and manually installing compatible updates on Linux and native Windows. The published `2.0.0-alpha.6` package has no `update` subcommand and needs a manual first installation of a compatible package:

```sh
./bin/bili update --check
./bin/bili update
```

Stable releases are the default. Add `--pre-release` to consider prereleases for that command only; `--pre-release=false` forces stable. Checks use a fixed HTTPS catalog and require a matching product/platform package. Installation validates a separate new version before selecting it and retains the old one. The CLI installs only when you explicitly run `update`. See [updates and compatibility](CLI.md#updates) for package layout, Windows invocation, persistent preferences and automatic-check behavior.

## Login and privacy

To stay logged in across commands, explicitly remember the account once:

```sh
./bin/bili login qr --remember
./bin/bili download VIDEO_ID --parts 1
```

Later commands use the saved account automatically through the default `--auth auto`; you do not need to log in again or repeat `--auth stored`. For a login used only by one download, use `download VIDEO_ID --auth qr` instead. A standalone `login qr` without `--remember` validates the login but does not make it available to the next process.

Secure OS storage is preferred; if unavailable, a warning explains possible plaintext fallback. The [login guide](CLI.md#login) covers disabling that fallback, Cookie input, status and logout. Never put Cookies in command text or save test-only credentials.

Only download content you are authorized to save. The application does not bypass login, entitlements, regional restrictions, or DRM. If the website asks for additional verification, stop; the CLI reports an error.

## Limits and build

Current source includes a native desktop GUI and video search sorting; the published Alpha still targets Linux x86_64 CLI, without Windows or GUI release assets. Aria2, verification WebView and corrupted-segment repair are not included. Stability, performance, and abnormal-exit recovery have not been fully qualified. FFmpeg is supplied as replaceable shared libraries; no FFmpeg executable or player is shipped or launched. The application is under the [MIT License with Commons Clause](LICENSE); FFmpeg and other third-party components have separate terms in the package notices.

See [BUILDING.md](BUILDING.md) for developer build requirements and commands.

## Community

See [Contributing](CONTRIBUTING.md) to report bugs, suggest features, or contribute changes. Please read the [Code of Conduct](CODE_OF_CONDUCT.md), [Security Policy](SECURITY.md), and [Accessibility statement](ACCESSIBILITY.md).
