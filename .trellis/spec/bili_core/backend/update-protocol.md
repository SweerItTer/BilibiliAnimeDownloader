# Shared update protocol

## 1. Scope / trigger

Every shell (CLI, GUI, TUI) updates through this one protocol. `bili_core::update` owns release discovery, descriptor validation, archive verification and exit codes; `bili_core::install` owns the installation transaction. A shell owns only scheduling, presentation, channel precedence and the runtime check it runs against a staged payload.

Read this before adding an update command, a release asset, a package layout or a GUI updater.

**Status: the implementation exists but has not passed acceptance.** Four known defects are listed in §8. Do not use product self-update to replace a real installation, and do not describe it as verified.

## 2. Signatures

```rust
// bili_core::update
pub const PROTOCOL: u32 = 1;              // descriptor + build_identity schema
pub const CHECK_INTERVAL_SECS: u64 = 86_400;
pub enum Product { Cli, Gui, Tui }        // serde: "cli" | "gui" | "tui"
pub enum Target { LinuxX86_64Gnu, WindowsX86_64Msvc }
pub enum Channel { Stable, PreRelease }   // serde: "stable" | "pre-release"
pub struct UpdateSettings { allow_pre_release: bool, check_updates: bool, auto_update: bool } // defaults: false, true, false
pub struct Identity;                      // Identity::new(Product, Target, &str) parses the version once
pub struct Descriptor;                    // Descriptor { schema_version, product, target, version, archive }
pub struct ArchiveRef;                    // ArchiveRef { name, format, sha256, size }
pub struct UpdateCandidate;               // UpdateCandidate { tag, version, channel, descriptor }
pub enum CheckOutcome {                   // Available carries the candidate; the other two carry the running version
    Available(UpdateCandidate),
    UpToDate { current: Version },
    NoCompatibleRelease { current: Version },
}
impl CheckOutcome {                      // the stable way for a shell to consume the result
    pub fn candidate(&self) -> Option<&UpdateCandidate>;
    pub fn is_available(&self) -> bool;
}
pub struct Updater;                       // Updater::new() is pinned to the official repository
impl Updater {
    pub async fn check(&self, identity: &Identity, channel: Channel) -> Result<CheckOutcome, UpdateError>;
    pub async fn download(&self, candidate: &UpdateCandidate, destination: &Path) -> Result<u64, UpdateError>;
    pub fn archive_url(&self, candidate: &UpdateCandidate) -> Result<Url, UpdateError>;
}
pub struct ReminderStore;                 // ReminderStore::new(dir)
impl ReminderStore {
    pub fn begin_check(&self, now_unix: u64, interval_secs: u64) -> Result<CheckPermit, UpdateError>;
    pub fn record_notification(&self, channel: Channel, version: &Version) -> Result<bool, UpdateError>;
}

// bili_core::install
pub const MARKER_FILE: &str = ".bili-install.json";
pub const CURRENT_FILE: &str = "CURRENT";
pub const VERSIONS_DIR: &str = "versions";
pub const UPDATE_DIR: &str = ".update";
impl InstallRoot { pub fn discover(current_exe: &Path) -> Result<Self, InstallError>; }
pub fn install_payload<F>(current_exe: &Path, identity: &Identity, candidate: &UpdateCandidate,
                          archive: &Path, validate: F) -> Result<Installed, InstallError>
    where F: FnOnce(&Staged<'_>) -> Result<(), InstallError>;
pub struct Installed { pub id: VersionId, pub version: Version }
```

`Identity::descriptor_name()` is `bili-update-<product>-<target>.json`. `Target::archive_kind()` is the only archive format a target may ship.

The block above is the surface a shell consumes, not an exhaustive listing of the crate's public items; constructors, field types and internal helpers are deliberately omitted. Take the real signatures from `bili_core/src/update/mod.rs` and `bili_core/src/install/mod.rs` before writing a shell.

## 3. Contracts

### Release assets

One GitHub Release, tagged `v<semver>`, may carry descriptors for several products and targets:

| Asset | Meaning |
|---|---|
| `<package>.tar.gz` / `.zip` | Product package; existing names are preserved |
| `bili-update-<product>-<target>.json` | Descriptor for exactly that product and target |
| `bili-native-source-<target>.tar.gz` | Matching FFmpeg source, recipe and notices |
| `SHA256SUMS` | Checksums over the published assets |

`bili-<target>-release.tar.gz` (Linux) and `bili-<target>-release.zip` (Windows) are the current CLI package names. Every product publishes its own descriptor asset, so a GUI release never overwrites the CLI descriptor and one tag can serve several product/target lines. A release without a descriptor for this product and target is skipped — the check continues with other qualifying newer releases rather than concluding that no update exists.

### Descriptor

```json
{
  "schema_version": 1,
  "product": "cli",
  "target": "x86_64-unknown-linux-gnu",
  "version": "2.0.0",
  "archive": {
    "name": "bili-x86_64-unknown-linux-gnu-release.tar.gz",
    "format": "tar.gz",
    "sha256": "<64 lowercase hex characters>",
    "size": 12345678
  }
}
```

- `schema_version` must equal `PROTOCOL`; unknown extra fields are ignored for forward compatibility.
- `product` and `target` must equal the compiled identity; `version` must equal the release tag, the descriptor and the identity the new payload reports, and must be strictly greater than the running version. None of these fields decides what the running shell is; they are checked against it.
- `archive.name` is a plain basename (no separators, no `..`, no leading dot, ≤128 characters), `format` must be the target's only legal format, `sha256` is 64 lowercase hex characters, `size` is between 1 and 512 MiB.
- The release must advertise the archive exactly once and with exactly `size` bytes. Zero, a different size, or a missing asset is rejected.

### Selection

1. Enumerate `GET /repos/{owner}/{repo}/releases?per_page=100`, at most 10 pages. Page 10 still full is an error ("cannot complete the check"), never "up to date".
2. Skip drafts. Accept only tags that parse as canonical `v<semver>`, and only when GitHub's prerelease flag agrees with the presence of a version pre-release segment.
3. Compare with `semver` **precedence**: build metadata never makes a version newer. Only a strictly higher version is a candidate, so a downgrade is not expressible.
4. `Stable` accepts versions without a pre-release segment; `PreRelease` accepts both and takes the highest of the two. It never steps down to a pre-release when a stable version is higher.
5. The highest remaining release that ships a valid descriptor for this product and target wins. A release with no descriptor for this product is skipped and the search continues with the next qualifying release; `NoCompatibleRelease` is reported only when every newer release in the selected channel lacks one. A newer release whose descriptor is present but fails validation is an error, never a silent skip.

### Configuration

`update` is an optional block in the ordinary settings file (explicit `--config` only):

```json
{ "update": { "allow_pre_release": false, "check_updates": true, "auto_update": false } }
```

Absent keys keep these defaults, so older settings files stay valid. The root object still rejects unknown keys. A shell's flag wins for one invocation: `allow_pre_release = true` plus `--pre-release=false` checks the stable channel.

`auto_update` is a GUI-only preference. The CLI refuses an explicit `update` command while it is true and explains the setting; it never installs anything on its own. Its automatic reminder is check-only.

### Installation layout

```text
<root>/.bili-install.json     marker: schema_version, layout_version, bootstrap_protocol, product, target
<root>/CURRENT                one payload id, "v-" + 32 lowercase hex
<root>/versions/<id>/         one complete payload per version
<root>/.update/               private staging for one transaction
```

- The root is found by walking up at most five directories from the running executable. There is no install-prefix override and no marker search elsewhere.
- `bin/bili` (Linux) or `bili.exe` (Windows) is the payload entry; the native library closure lives with it in the same version directory.
- The transaction verifies the archive's size, digest and layout, expands it into the private staging directory, checks every payload identity, runs the caller's `validate` callback, and only then replaces `CURRENT` with a single rename. A failure before that rename leaves the previous pointer untouched.
- Files outside the layout are never touched. No automatic rollback, no background daemon and no startup repair process is part of this protocol.

### Identity and the shell's validation callback

The identity is compiled into the shell (product, target, version) and is the only authority for what may be installed. Package metadata is checked against it and can never widen it. The CLI's callback runs the staged payload as `--json doctor` with a private profile and an empty inherited environment, then requires the `schema_version: 1` / `type: "doctor"` envelope, `data.build_identity.{product,target,version,update_protocol}` equal to the candidate identity, and a successful `data.native` object. A GUI passes its own headless equivalent; it must not accept a payload it could not run.

Automatic reminders are shell policy: human terminal only, never with machine-readable output, never for offline diagnostics, at most once per 24 hours, at most one announcement per channel and version, silent on failure, and never changing the command's exit code. Offline diagnostics means commands that must not reach the network, such as `doctor`, `update` itself and login/logout/job inspection. A local business command still qualifies: `merge` on local files is a tracked business command, so it can trigger the automatic check even though it needs no Bilibili request. The reminder directory is per product, target and *selected* channel — not the channel of whichever release was found.

## 4. Validation / error matrix

| Condition | Classification |
|---|---|
| Draft, malformed tag, prerelease flag disagrees with the version | Release skipped |
| Every qualifying newer release lacks a descriptor for this product and target | `CheckOutcome::NoCompatibleRelease` |
| Descriptor schema, product, target or version disagrees | `UpdateError::{UnsupportedSchema, ProductMismatch, TargetMismatch, VersionMismatch}` |
| Unsafe archive name, impossible format, bad digest, bad or disagreeing size | `UpdateError::{UnsafeAssetName, ArchiveKindMismatch, DigestMismatch, SizeMismatch}` |
| Archive named once but absent, or advertised twice | `UpdateError::{MissingAsset, AmbiguousAsset}` |
| More releases than the page budget | `UpdateError::TooManyReleases` (never "up to date") |
| Transport, HTTP status, quota, oversized body, timeout | `UpdateError::{Transport, HttpStatus, RateLimited, ResponseTooLarge, Timeout}` |
| Reminder state locked by another process | `UpdateError::Busy`; the caller must not wait |
| Running executable not in a packaged installation | `InstallError::NotPackaged` |
| Marker or `CURRENT` missing or mismatched | `InstallError::{MarkerInvalid, CurrentInvalid, MarkerMismatch}` |
| Candidate not newer, payload identity disagrees, layout or inventory wrong, digest wrong | `InstallError::{Downgrade, PayloadIdentity, ArchiveLayout, InventoryMismatch, ArchiveDigest}` |
| Validation callback rejected the staged payload | `InstallError::Validator` |

Shells map these to their own exit codes; the CLI uses 2 only for local input and configuration errors, and 4 for remote, remote-metadata, lock, corruption and filesystem failures. Error text is static: no URL, header, body or credential ever reaches a message.

## 5. Good / base / bad

Good: a GUI calls `Updater::check` for `Product::Gui`, shows the candidate, downloads it into a private directory, and runs its own headless payload check before letting `install_payload` replace `CURRENT`.

Base: an offline diagnostic command prints its report and never performs a network check.

Bad: a shell parses release assets itself, compares versions with `Ord` (build metadata), treats an exhausted page budget as "up to date", or installs a payload whose `doctor` it never ran.

## 6. Tests

Covered by focused tests at the owning boundaries: channel union and precedence, build metadata, drafts and inconsistent flags, missing descriptors, descriptor defects, duplicate descriptors and archives, paging past a full first page, page-budget failure, rate limiting, redirect allow-list, reminder intervals, failed-check throttling, per-channel deduplication, busy locks, clock rollback, corrupt state, settings compatibility, flag strictness, exit-code classes, and doctor envelope/size rejection.

Observed results (2026-10-30): 50 focused `bili_core` tests, 3 `core-bootstrap` tests, 17 CLI tests, CLI-scoped Clippy, 18 Python tests, actionlint for both workflows, and final formatting passed. The workspace Clippy run exited 101 at `core-bootstrap/src/main.rs:76` on `resolve(&root, &id)` (`needless_borrow`); it does not establish Clippy results for other workspace crates.

The local Linux release package's descriptor matched archive hash and size; offline verification passed extraction, checksums, source/configuration, dependency closure, and clean-environment outer-entry `--help`, `--version`, and doctor (`FFmpeg 8.0.3`). A separate PTY smoke ran three local fixture merges through outer `bin/bili`; all exited 0. The first run created `update-reminders.json`, and a second merge with the same cache preserved its bytes and `last_checked_unix`. A fresh-profile `--json` TTY run emitted eight valid JSON envelopes and wrote no update cache. No compatible higher descriptor was available, so a visible new-version announcement or notification deduplication was not exercised.

Not covered: any complete update installation. No run has downloaded and installed an update, validated its staged payload, and switched `CURRENT` end to end. No Windows/MSVC package has been produced or checked, and this local Linux run is not Ubuntu 2.35 evidence. All four blockers in §8 remain unmodified.

## 7. Wrong versus correct

Wrong: shell copies candidate selection or re-derives the identity from `BUILD_INFO.json`; wrong: accepting a truncated `doctor` report; wrong: writing `CURRENT` directly from a shell.

Correct: one `Identity` from compile-time values, one `check`, one verified archive, one `validate` callback, and one `CURRENT` rename performed by `install_payload`.

## 8. Known blockers (not fixed)

These are code defects in the delivered implementation, recorded with their exact locations. None of them is an intended limitation, and none may be documented as approved behaviour:

| Blocker | Location | Effect |
|---|---|---|
| Candidate is compared only with the running identity; `CURRENT` is not re-read after the lock is taken | `bili_core/src/install/mod.rs:339-350` | Two concurrent installs can leave an older payload selected |
| `Staged::identity` reports the old running identity instead of the candidate | `bili_core/src/install/mod.rs:409-415` | The staged-payload check compares against the old version, so a valid new payload is rejected and the candidate identity is never verified |
| Default Windows workspace `check`/`test` is blocked by a compile guard | `core-bootstrap/src/main.rs:18` | The ordinary workspace check/test cannot run on Windows; an actual MSVC package has not been verified (the explicit `crt-static` shipping build is a separate path) |
| The staged-payload check forwards inherited `PATH` on Windows | `core-cli/src/update.rs:310-315` | A missing DLL could be resolved from a developer native prefix instead of the package closure |

Additionally, the published `2.0.0-alpha.6` package predates this protocol: it contains no installation marker and no release descriptor, so it cannot update itself. The first compatible installation has to be installed manually.
