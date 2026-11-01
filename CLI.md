# CLI guide

English | [简体中文](CLI.zh-CN.md) | [Project overview](README.md)

This guide describes the current source version of `bili`. The published [v2.0.0-alpha.6](https://github.com/SweerItTer/BilibiliAnimeDownloader/releases/tag/v2.0.0-alpha.6) is a Linux x86_64 CLI package and does **not** have `update`, `download --list-resolutions`, or `search --sort`. Use its `qualities` command to inspect one part. Windows and native GUI builds are available from source/CI, not as assets of that release. Build instructions belong in [BUILDING.md](BUILDING.md).

Go to: [invocation](#invocation), [first download](#first-download), [commands](#commands), [search](#search), [quality and parts](#quality), [login](#login), [download options](#download-options), [resume](#resume), [merge](#merge), [cleanup](#cleanup), [configuration](#configuration), [updates](#updates), [output and errors](#output).

<a id="invocation"></a>
## Run the CLI

Examples use the Linux package entry `./bin/bili`. In Windows PowerShell, replace it with `.\bili.exe`. If you have put the package entry on PATH, use `bili`. Keep the complete extracted package together; do not copy out only the executable. Current versioned packages contain a stable entry, `CURRENT`, and `versions/`; Alpha 6 uses the older `bin/` and `lib/` layout.

```sh
./bin/bili --version
./bin/bili --help
./bin/bili download --help
./bin/bili login qr --help
./bin/bili help download
```

Replace uppercase placeholders such as `VIDEO_ID`, `QUALITY_ID` and `JOB_ID` before running an example. `INPUT` accepts a valid `BV...`, `av` plus a positive numeric ID, `ss` plus a season ID, `ep` plus an episode ID, or a supported HTTPS Bilibili URL/short link. Quote URLs, search terms and paths containing spaces. A plain numeric ID without its prefix is not accepted.

Global options can appear before or after the command:

| Option | Meaning |
|---|---|
| `--config PATH` | Read this ordinary JSON configuration file for this invocation; there is no automatic config-file lookup |
| `--state-dir DIR` | Use this job/checkpoint directory; reuse it when inspecting or resuming those jobs |
| `--json` | Emit machine-readable JSON lines instead of human results |
| `-h`, `--help` | Show command help; `--help` includes longer descriptions |
| `-V`, `--version` | Show the program version at the top level |

<a id="first-download"></a>
## Search, inspect, then download

These examples use anonymous access and do not load a saved account. Search never starts a download:

```sh
./bin/bili search "KEYWORD" --type video --auth anonymous
./bin/bili info VIDEO_ID --auth anonymous
./bin/bili qualities VIDEO_ID --part 1 --auth anonymous
./bin/bili download VIDEO_ID --parts 1 --output "$HOME/Videos/Bilibili" --auth anonymous
```

The last command chooses the highest resolution actually available to that account, saves subtitles and current danmaku, and merges audio/video into a playable file. It waits for automatic merging and reports completed paths. `--parts 1` deliberately downloads only the first part; omitting it selects **all** resolved parts. An episode URL/ID can resolve to one episode with its original season ordinal.

Equivalent PowerShell download:

```powershell
.\bili.exe download VIDEO_ID --parts 1 --output "$HOME\Videos\Bilibili" --auth anonymous
```

To download content requiring your account, follow [login](#login). Only download content you are authorized to save. Logging in does not bypass VIP/purchase requirements, regional restrictions, verification challenges or DRM.

<a id="commands"></a>
## Command reference

Square brackets below mean optional arguments, and `INPUT...` means one or more inputs. Do not type the brackets or ellipsis.

| Command | Purpose and command-specific options |
|---|---|
| `search QUERY` | Search by keyword; `--type all\|video\|bangumi\|film`, `--page N`, `--sort ORDER`, authentication options |
| `info INPUT` | Resolve content and list its parts/episodes; authentication options |
| `qualities INPUT` | Inspect one part's quality catalog; `--part N`, authentication options |
| `download INPUT...` | Download selected parts; all [download options](#download-options) below |
| `login qr` | Scan a QR code in an interactive terminal; `--remember` |
| `login cookie` | Hidden Cookie prompt, or redirected input with `--stdin`; `--remember` |
| `login status` | Load any saved account and report its status |
| `logout` | Remove remembered login through the account store |
| `jobs` | Inspect durable jobs without acquiring the download writer lock |
| `resume JOB_ID` or `resume --all` | Continue/retry saved work; authentication options |
| `merge JOB_ID` | Merge a managed download saved for manual merging |
| `merge --video PATH [--audio PATH] --output PATH` | Remux local media to a new output file |
| `clean JOB_ID` or `clean --abandoned` | Clean owned temporary files; `--dry-run`, `--include-resumable` |
| `doctor` | Report local paths/permissions, build identity and linked native media capabilities; no network request |
| `update` | Install a compatible newer version; `--check`, `--pre-release[=true\|false]` |
| `help [COMMAND...]` | Show help for the program or a subcommand; equivalent to its help flag |

Authentication options are `--auth MODE` and explicit `--remember`; they apply to `search`, `info`, `qualities`, `download` and `resume`, not to every command.

<a id="search"></a>
## Search and sorting

`--type` defaults to `all`, `--page` to `1`, and `--sort` to `relevance`. Pages are numbered 1–1000. Queries must be nonempty and at most 256 UTF-8 bytes. The `all` type returns separate video, bangumi and film category pages; it is not one globally ranked list.

Current source supports these video sort orders:

| `--sort` | Order |
|---|---|
| `relevance` | Existing server default; valid with every type |
| `views` | Most views |
| `newest` | Newest publication |
| `danmaku` | Most danmaku |
| `favorites` | Most favorites |
| `comments` | Most comments |

```sh
./bin/bili search "KEYWORD" --type video --sort newest --page 1 --auth anonymous
./bin/bili search "KEYWORD" --type video --sort newest --page 2 --auth anonymous
```

Non-default sorting requires `--type video`. Combining it with `all`, `bangumi` or `film` is an error, rather than silently falling back. Results retain server order and pagination; the application does not sort only the current page. Live public-service sorting behavior has not been verified.

<a id="quality"></a>
## Choose parts and quality

Run `info INPUT` to see original part/episode ordinals. `qualities INPUT` inspects the first or sole resolved part; `--part N` chooses a specific original ordinal. For a URL selecting P3 or an `ep` ID for episode 13, do not assume its ordinal becomes 1.

Current source can inspect several parts without creating download jobs:

```sh
./bin/bili download VIDEO_ID --list-resolutions --parts 1,3-5 --auth anonymous
./bin/bili download VIDEO_ID --parts 1,3-5 --quality QUALITY_ID --auth anonymous
```

The catalog separates **Quality ID** (the server's quality tier), dimensions (such as 1920×1080), and Stream ID (a representation). Pass the Quality ID to `--quality`, not a pixel height or Stream ID. Missing dimensions/FPS or access requirements remain unknown. An advertised tier or returned media address is not proof that a download has succeeded.

`highest`, the default, selects the highest obtainable actual spatial resolution. An explicit quality/representation/codec/HDR constraint must match an available stream; the program does not silently pick another one. Account access may differ between anonymous users, logged-in users, VIP members and purchased content.

For advanced exact selection, inspect `qualities INPUT --json` and use `--video-repr ID`, `--audio-repr ID`, `--codec CODEC`, or `--hdr MODE`. Copy IDs and exact codec/dynamic-range values from the relevant part's catalog. IDs alone do not necessarily distinguish streams with different codecs or metadata; add the matching constraints when needed. These options select existing streams and do not transcode.

<a id="login"></a>
## Login once or keep an account

To use an account across commands, explicitly save it once:

```sh
./bin/bili login qr --remember
./bin/bili login status
./bin/bili download VIDEO_ID --parts 1
```

The default `--auth auto` loads an account you previously chose to remember. You do not need `--auth stored` on each later command. To require a saved account and fail if none exists, choose `--auth stored`. A bad saved record is reported, not silently treated as anonymous.

For one download only, run `download VIDEO_ID --auth qr`. The QR login lives until that command ends. Standalone `login qr` or `login cookie` **without** `--remember` validates a session, but that session ends with the process and is unavailable to the next command.

| `--auth` mode | Behavior |
|---|---|
| `auto` | Default: load explicitly saved login, otherwise anonymous; never silently ask for input |
| `stored` | Require and load saved login |
| `anonymous` | Do not load saved login |
| `qr` | Interactive QR login for this command |
| `cookie-prompt` | Read a Cookie header through a hidden terminal prompt |
| `cookie-stdin` | Read a Cookie header from explicitly redirected stdin, until EOF |

`--remember` is off by default and can be combined with `qr`, `cookie-prompt` or `cookie-stdin`, not `auto`, `stored` or `anonymous`. For standalone Cookie login, use `login cookie` for a hidden prompt or `login cookie --stdin` for redirected input. QR and hidden prompts need an interactive terminal; `cookie-stdin`/`login cookie --stdin` reject an ordinary terminal to avoid echoing secrets. Cookie input is limited to 64 KiB. Do not put the Cookie in argv, shell history, config JSON or a producer command's arguments.

Remembered login prefers the OS secure store. The default permits a warned plaintext fallback in the private local account directory if secure storage is unavailable. Set `"allow_plaintext_fallback": false` in an explicitly loaded [configuration file](#configuration) to reject that fallback. `--state-dir` does not move or isolate the account store. New login input is memory-only unless you explicitly choose `--remember`. To remove remembered login, run `logout`.

<a id="download-options"></a>
## Download options and saved files

`download` accepts multiple inputs in one invocation. The same options apply to each input's selected parts:

```sh
./bin/bili download VIDEO_ID OTHER_ID --parts all --subtitles none --danmaku none
```

| Option | Default / meaning |
|---|---|
| `--parts all` or `--parts 1,3-5` | Default `all`; positive original ordinals/ranges, no spaces; unavailable ordinals fail |
| `--output DIR` | Destination directory for new downloads; overrides config `output_dir` |
| `--quality highest` or `--quality QUALITY_ID` | Default `highest`; otherwise an exact positive server quality-tier ID |
| `--video-repr ID`, `--audio-repr ID` | Exact video/audio representation constraint |
| `--codec CODEC`, `--hdr MODE` | Exact catalog codec/dynamic-range constraint |
| `--subtitles all\|none\|LANGS\|track:ID,...` | Default `all`; comma-separated language labels or track IDs, never mixed |
| `--danmaku current\|none` | Default `current`; save currently available comments, not historical snapshots |
| `--merge auto\|manual` | Default `auto`; `manual` leaves downloaded resources for a later managed `merge` |
| `--jobs N` | Concurrent jobs; default 3 or config value, supported range 1–128 |
| `--connections N` | Shared connection budget; default 6 or config value, supported range 1–256 |
| `--keep-temp` | Retain owned temporary resources after successful completion; off by default |
| `--list-resolutions` | Catalog-only query; creates no jobs or output directories |
| `--auth MODE`, `--remember` | Account choice and explicit persistence; see [login](#login) |

`--list-resolutions` may use `--parts`, authentication and `--connections`. It conflicts with explicitly supplied `--output`, `--quality`, representation/codec/HDR constraints, subtitle/danmaku/merge options, `--jobs` and `--keep-temp`, including explicit values equal to their normal download defaults.

Subtitle language labels and track IDs must match the source metadata. `all` succeeds if the source has no subtitle tracks; an explicitly requested missing track/language is reported. JSON subtitle tracks produce raw JSON and SRT; supplied ASS tracks are saved as ASS. Danmaku produces JSONL, XML and ASS files beside the media. These are separate files, not burned into the picture. Not every source has every attachment. Requested attachment failures are reported and make the command fail even when the video itself is usable.

<a id="resume"></a>
## Inspect, interrupt and resume jobs

Press **Ctrl+C once** to request a safe pause, save recoverable state and wait for workers to stop. A second Ctrl+C forces exit and requires crash recovery. The CLI has no separate cross-process `pause` or `cancel` command, and does not keep a download daemon running after it exits.

```sh
./bin/bili --state-dir "./bili-state" download VIDEO_ID --parts 1
./bin/bili --state-dir "./bili-state" jobs
./bin/bili --state-dir "./bili-state" resume JOB_ID
./bin/bili --state-dir "./bili-state" resume --all
```

Use the job ID printed in progress/results, and the **same state directory** used for the download. Without an override, every invocation uses the platform default. `resume` uses your current chosen authentication; it does not store the original Cookie in the job. Account-only content may require saved login or `--auth qr` again.

`jobs` is read-only and may run while another process downloads. Only one mutating Core session may own a state directory; a second downloader/resume/merge/clean reports `Busy`. After a forced exit, `jobs` may still show the last recorded active phase. A writable session reconciles interrupted active jobs before resuming; displayed bytes alone are not the durable offset. Unexpected-shutdown/power-loss behavior has not been fully qualified. Do not delete lock/checkpoint files or edit JSON to force a state.

`resume --all` includes unfinished jobs and completed videos with failed requested attachments. Retrying attachments reuses a verified completed video. It does not redownload all successfully completed jobs. Ordinary retry/continuation is not precise damaged-segment repair; identity changes, corruption or output conflicts remain explicit failures.

<a id="merge"></a>
## Automatic and manual merging

Normal downloads use `--merge auto`, so successful completion includes in-process audio/video remuxing and validation. To defer merging:

```sh
./bin/bili download VIDEO_ID --parts 1 --merge manual
./bin/bili merge JOB_ID
```

Keep that job's state and resources until merging finishes. A manual download's `Downloaded` phase means resources are saved; it does not mean the final playable file has been published.

For existing local media, supply paths instead of a job ID:

```sh
./bin/bili merge --video "./video.media" --audio "./audio.media" --output "./combined.mp4"
```

`--audio` is optional for a source that already contains the intended streams. `--video` requires `--output`; local path mode and `JOB_ID` are alternatives. Remuxing does not transcode, launch a player or invoke an FFmpeg executable. Existing output files are not overwritten; unmanaged input files are retained.

<a id="cleanup"></a>
## Clean temporary files

Preview the exact owned paths and byte count before deleting:

```sh
./bin/bili clean JOB_ID --dry-run
./bin/bili clean JOB_ID
```

Default cleanup preserves recoverable partials and skips active jobs. If you intentionally want to discard a stopped job's partial download, preview and then explicitly permit it:

```sh
./bin/bili clean JOB_ID --include-resumable --dry-run
./bin/bili clean JOB_ID --include-resumable
```

`--abandoned` selects non-completed/non-Downloaded jobs rather than one `JOB_ID`; it still needs `--include-resumable` to remove their recoverable files. For example, `clean --abandoned --include-resumable --dry-run` previews them. Do not clean a manual `Downloaded` job before merging. Cleanup removes exact owned temporary resources/staging, preserves final media and unrelated user files, and does not remove job history or log you out. Removing resumable files can require downloading those resources again.

<a id="configuration"></a>
## Configuration and paths

Create an ordinary UTF-8 JSON file, for example `bili-config.json`:

```json
{
  "state_dir": "./bili-state",
  "output_dir": "./downloads",
  "jobs": 3,
  "connections": 6,
  "allow_plaintext_fallback": false,
  "update": {
    "allow_pre_release": false,
    "check_updates": true,
    "auto_update": false
  }
}
```

```sh
./bin/bili --config "./bili-config.json" download VIDEO_ID --parts 1
./bin/bili --config "./bili-config.json" jobs
./bin/bili --config "./bili-config.json" resume JOB_ID
```

Pass `--config` on **each invocation** that should use it. Config files are not created or loaded implicitly, and are not a place for credentials. The limit is 64 KiB; unknown top-level fields fail. Alpha 6 does not accept the `update` block: omit it when using that release.

Explicit `--state-dir` and download `--output`/`--jobs`/`--connections` override their config values; config values override built-in defaults. Relative paths are resolved from the command's working directory, not the config file's directory. Default state/output are the platform's `bili` local-data directories; `doctor` reports the effective paths. Account storage remains separate. For Windows paths in JSON, use forward slashes or escape backslashes, for example `"C:/Users/NAME/Videos/Bilibili"`.

<a id="updates"></a>
## Check and install updates

Current source separates checking from installation:

```sh
./bin/bili update --check
./bin/bili update
./bin/bili update --check --pre-release
./bin/bili update --pre-release
./bin/bili update --check --pre-release=false
```

Use `.\bili.exe update ...` in PowerShell. The default stable channel considers only stable versions. `--pre-release` also considers prereleases for that invocation, while `--pre-release=false` forces stable even if the config enables prereleases. To keep allowing prereleases, set `update.allow_pre_release` in the explicitly loaded config; the command flag does not rewrite it.

Checks use the official static HTTPS catalog, then matching tagged-release descriptors/packages, without a Releases API key. Releases missing this CLI product/platform's descriptor are skipped. A newer version number alone does not guarantee a compatible update; `NoCompatibleRelease` is different from a download failure.

Installing requires a compatible versioned package in a writable installation directory. Keep its marker, stable entry, `CURRENT` and `versions/` together. The updater verifies a separate candidate and its native runtime before switching `CURRENT`; it preserves the old version and user files. An unpackaged development executable can check but cannot install. Alpha 6 cannot self-update; manually extract the first compatible package into a new directory. No GUI/Windows update is available until a release actually supplies its matching assets.

With `update.check_updates` enabled (default), interactive search/info/qualities/download/resume/merge commands may check at most once per 24 hours for the selected channel and print a reminder. Checks have a short budget, failures stay silent, and they never install an update or change the command's exit status. JSON/noninteractive invocations, help/version, jobs, doctor and account commands do not trigger reminders. Setting `check_updates` false disables reminders, not explicit `update --check`. `auto_update` is GUI-only; keep it false for CLI, whose explicit update command rejects true.

<a id="output"></a>
## JSON, exit codes and troubleshooting

```sh
./bin/bili search "KEYWORD" --type video --json --auth anonymous
./bin/bili download VIDEO_ID --parts 1 --json --auth anonymous
./bin/bili jobs --json
./bin/bili doctor --json
```

`--json` writes one envelope per line to stdout, shaped as `{"schema_version":1,"type":"...","data":...}`. A download can emit `plan`, `job`, `result` and `error` records; parse lines as a stream, not one JSON document. Results include job IDs, component outcomes and per-job `exit_code`. Other types include `search`, `info`, `qualities`, `resolutions`, `auth`, `jobs`, `cleanup`, `doctor` and `update`. Human progress/notices use stderr. Clap help and argument errors remain normal command-line text even with `--json`.

| Exit | Meaning |
|---|---|
| `0` | Requested work succeeded or was verified already complete; a successful update check need not find an update |
| `2` | Invalid command/selection/configuration or an unsupported combination |
| `3` | Login, entitlement, region, verification, availability or selected-media-identity restriction |
| `4` | Operational failure: network, storage, Busy, corruption, native library or remux |
| `130` | Interrupted/paused/cancelled work |

A batch reports each result and returns the highest relevant exit code. Video success with a failed subtitle/danmaku component is still nonzero. Use `resume JOB_ID` to retry after correcting access/network/storage problems. For `Busy`, stop the other writer using that state directory; for missing jobs, check `--config`, `--state-dir` and your working directory. For `unknown argument`, inspect `--version` and that binary's `--help` before using a newer source-only option. Native loader/startup problems need the complete package, not an `ffmpeg` installation; build troubleshooting is in [BUILDING.md](BUILDING.md).

Recovery, throughput and public-service sorting are not fully qualified across all supported systems. Aria2, precise damaged-segment repair and challenge WebView are not current CLI capabilities. The app uses the [MIT License with Commons Clause](LICENSE); FFmpeg remains separately LGPL-2.1-or-later with the package's matching notices and source materials.
