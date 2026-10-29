//! `bili update` and the throttled reminder: a thin shell over core update APIs.
//!
//! Discovery, archive verification and the installation transaction live in
//! `bili_core`; this module only decides channel precedence, isolation of the
//! staged-payload runtime check, and how a failure is presented. It never
//! writes the installation marker or `CURRENT` itself.

use crate::args::{Cli, Command};
use crate::output;
use bili_core::install::{self, InstallError, InstallRoot, Installed, Staged};
use bili_core::update::{
    Channel, CheckPermit, Identity, ReminderStore, UpdateError, UpdateSettings, Updater,
    CHECK_INTERVAL_SECS, PROTOCOL,
};
use bili_core::{CoreError, ErrorCode, Result};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Budget for the background reminder; it never delays the user's command.
const NOTIFY_BUDGET: Duration = Duration::from_secs(1);
/// Budget for the staged payload's own `doctor` before it is rejected.
const DOCTOR_BUDGET: Duration = Duration::from_secs(15);
/// Largest `doctor` report accepted from a staged payload.
const MAX_DOCTOR_BYTES: u64 = 256 * 1024;

/// Commands whose completion may trigger an automatic reminder. Everything
/// else is offline, diagnostic, credential-related or already an update.
fn notification_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Search { .. }
            | Command::Info { .. }
            | Command::Qualities { .. }
            | Command::Download(_)
            | Command::Resume { .. }
            | Command::Merge { .. }
    )
}

fn notify_eligible(cli: &Cli) -> bool {
    !cli.json
        && notification_command(&cli.command)
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
}

/// Channel for this invocation: the flag wins, otherwise configuration decides.
fn effective_channel(pre_release: Option<bool>, settings: UpdateSettings) -> Channel {
    match pre_release.unwrap_or(settings.allow_pre_release) {
        true => Channel::PreRelease,
        false => Channel::Stable,
    }
}

fn channel_key(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::PreRelease => "pre-release",
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Cache directory for one product, target and *selected* channel. The channel
/// is the one requested here, not the channel of whichever release is found.
fn reminder_dir(
    project: &directories::ProjectDirs,
    target: bili_core::update::Target,
    channel: Channel,
) -> PathBuf {
    project
        .cache_dir()
        .join("update")
        .join(bili_core::update::Product::Cli.as_str())
        .join(target.triple())
        .join(channel_key(channel))
}

fn map_update(error: UpdateError) -> CoreError {
    let (code, operation) = match error {
        UpdateError::InvalidInput => (ErrorCode::InvalidInput, "update request is invalid"),
        UpdateError::Transport => (ErrorCode::Transport, "reach the update service"),
        UpdateError::HttpStatus(_) => (
            ErrorCode::Transport,
            "update service returned an error status",
        ),
        UpdateError::RateLimited => (ErrorCode::Transport, "update service quota is exhausted"),
        UpdateError::TooManyReleases => (
            ErrorCode::Transport,
            "release listing exceeds the supported page limit",
        ),
        UpdateError::ResponseTooLarge => (
            ErrorCode::Transport,
            "update response exceeds its size bound",
        ),
        UpdateError::Timeout => (ErrorCode::Transport, "update transfer exceeded its budget"),
        UpdateError::Busy => (ErrorCode::Busy, "another update holds the update state"),
        UpdateError::Filesystem => (ErrorCode::Filesystem, "update local filesystem operation"),
        _ => (
            ErrorCode::Corruption,
            "update metadata is malformed or untrusted",
        ),
    };
    CoreError::new(code, operation)
}

fn map_install(error: InstallError) -> CoreError {
    let (code, operation) = match error {
        InstallError::NotPackaged => (
            ErrorCode::InvalidInput,
            "this build is not a packaged installation; only update checks are available",
        ),
        InstallError::Busy => (ErrorCode::Busy, "another update is using this installation"),
        InstallError::NotWritable | InstallError::Filesystem => (
            ErrorCode::Filesystem,
            "installation directory is not writable",
        ),
        InstallError::Downgrade => (
            ErrorCode::Conflict,
            "update candidate is not newer than the running version",
        ),
        _ => (
            ErrorCode::Corruption,
            "installation payload is inconsistent or unsupported",
        ),
    };
    CoreError::new(code, operation)
}

/// Run `bili update` without constructing the media core, credentials or jobs.
pub async fn run(
    cli: &Cli,
    settings: UpdateSettings,
    check_only: bool,
    pre_release: Option<bool>,
) -> u8 {
    match run_inner(cli, settings, check_only, pre_release).await {
        Ok(code) => code,
        Err(error) => {
            output::error(&error, cli.json);
            output::exit_code(&error)
        }
    }
}

async fn run_inner(
    cli: &Cli,
    settings: UpdateSettings,
    check_only: bool,
    pre_release: Option<bool>,
) -> Result<u8> {
    if settings.auto_update {
        return Err(invalid(
            "auto_update is a GUI-only setting; the CLI only installs on an explicit update command",
        ));
    }
    let identity = crate::compiled_identity()?;
    let channel = effective_channel(pre_release, settings);
    let updater = Updater::new().map_err(map_update)?;
    let outcome = updater
        .check(&identity, channel)
        .await
        .map_err(map_update)?;
    let mut report = serde_json::json!({
        "current": identity.version().to_string(),
        "channel": channel_key(channel),
        "check": serde_json::to_value(&outcome)
            .map_err(|_| CoreError::new(ErrorCode::Corruption, "format update result"))?,
        "installed": serde_json::Value::Null,
    });
    if check_only || !outcome.is_available() {
        output::result("update", &report, cli.json)?;
        return Ok(0);
    }
    let candidate = outcome
        .candidate()
        .cloned()
        .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "format update candidate"))?;
    let current_exe = std::env::current_exe().map_err(CoreError::io)?;
    let installed = install_candidate(&updater, &current_exe, &identity, &candidate).await?;
    report["installed"] = serde_json::json!({
        "version": installed.version.to_string(),
        "id": installed.id.as_str(),
    });
    output::result("update", &report, cli.json)?;
    Ok(0)
}

async fn install_candidate(
    updater: &Updater,
    current_exe: &Path,
    identity: &Identity,
    candidate: &bili_core::update::UpdateCandidate,
) -> Result<Installed> {
    // Refuse unpackaged development builds before any download happens.
    InstallRoot::discover(current_exe).map_err(map_install)?;
    let scratch = tempfile::Builder::new()
        .prefix("bili-update-")
        .tempdir()
        .map_err(CoreError::io)?;
    let archive = scratch.path().join("archive");
    updater
        .download(candidate, &archive)
        .await
        .map_err(map_update)?;
    let expected = Identity::new(
        candidate.descriptor.product,
        candidate.descriptor.target,
        &candidate.version.to_string(),
    )
    .map_err(map_update)?;
    let executable = current_exe.to_path_buf();
    let running = identity.clone();
    let staged_candidate = candidate.clone();
    let scratch_path = scratch.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        install::install_payload(
            &executable,
            &running,
            &staged_candidate,
            &archive,
            |staged| validate_staged(staged, &expected, &scratch_path),
        )
    })
    .await
    .map_err(|_| CoreError::new(ErrorCode::Corruption, "installation worker failed"))?
    .map_err(map_install)
}

/// Runtime check of the staged payload: its declared identity, then its own
/// isolated `--json doctor` report. The shell never rewrites the expected
/// identity to match a payload.
fn validate_staged(
    staged: &Staged<'_>,
    expected: &Identity,
    scratch: &Path,
) -> std::result::Result<(), InstallError> {
    if staged.identity() != expected {
        return Err(InstallError::Validator);
    }
    let report = staged_doctor(&staged.entry(), scratch)?;
    let build = report
        .pointer("/data/build_identity")
        .ok_or(InstallError::Validator)?;
    let version = expected.version().to_string();
    let matches = build.get("product").and_then(|value| value.as_str())
        == Some(expected.product().as_str())
        && build.get("target").and_then(|value| value.as_str()) == Some(expected.target().triple())
        && build.get("version").and_then(|value| value.as_str()) == Some(version.as_str())
        && build
            .get("update_protocol")
            .and_then(|value| value.as_u64())
            == Some(u64::from(PROTOCOL));
    if !matches
        || !report
            .pointer("/data/native")
            .is_some_and(|value| value.is_object())
    {
        return Err(InstallError::Validator);
    }
    Ok(())
}

/// Execute the staged payload's `doctor` with a private profile and an empty
/// inherited environment, then accept only its strict JSON envelope.
fn staged_doctor(
    entry: &Path,
    scratch: &Path,
) -> std::result::Result<serde_json::Value, InstallError> {
    let home = scratch.join("home");
    let roaming = home.join("AppData").join("Roaming");
    let local = home.join("AppData").join("Local");
    for directory in [
        home.clone(),
        home.join("config"),
        home.join("data"),
        home.join("cache"),
        scratch.join("tmp"),
        roaming.clone(),
        local.clone(),
    ] {
        std::fs::create_dir_all(directory).map_err(|_| InstallError::Filesystem)?;
    }
    let mut command = std::process::Command::new(entry);
    command
        .arg("--json")
        .arg("doctor")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // Nothing from the parent environment reaches the staged payload: no
        // tokens, no loader overrides, no agent sockets.
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("APPDATA", &roaming)
        .env("LOCALAPPDATA", &local)
        .env("TMPDIR", scratch.join("tmp"))
        .env("TEMP", scratch.join("tmp"))
        .env("TMP", scratch.join("tmp"));
    // Windows cannot start a process without its system directories.
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR", "PATH"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().map_err(|_| InstallError::Validator)?;
    let stdout = child.stdout.take().ok_or(InstallError::Validator)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // One byte past the bound is enough to reject instead of truncating.
        let _ = stdout.take(MAX_DOCTOR_BYTES + 1).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + DOCTOR_BUDGET;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => {
                break None;
            }
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(InstallError::Validator);
    }
    let bytes = reader.join().map_err(|_| InstallError::Validator)?;
    let status = status.expect("status checked above");
    if timed_out || !status.success() {
        return Err(InstallError::Validator);
    }
    doctor_report(bytes)
}

/// Accept only a complete `doctor` envelope within the size bound.
fn doctor_report(bytes: Vec<u8>) -> std::result::Result<serde_json::Value, InstallError> {
    if bytes.len() as u64 > MAX_DOCTOR_BYTES {
        return Err(InstallError::Validator);
    }
    let report: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| InstallError::Validator)?;
    let object = report.as_object().ok_or(InstallError::Validator)?;
    let schema = object
        .get("schema_version")
        .and_then(|value| value.as_u64());
    if schema != Some(1)
        || object.get("type").and_then(|value| value.as_str()) != Some("doctor")
        || !object.get("data").is_some_and(|value| value.is_object())
    {
        return Err(InstallError::Validator);
    }
    Ok(report)
}

/// After a successful business command, offer at most one announcement per
/// channel and version. Silent on every failure; never changes the exit code.
pub async fn maybe_notify(cli: &Cli, settings: UpdateSettings) {
    if !settings.check_updates || !notify_eligible(cli) {
        return;
    }
    let Ok(identity) = crate::compiled_identity() else {
        return;
    };
    let Ok(project) = crate::project_dirs() else {
        return;
    };
    let channel = settings.channel();
    let store = ReminderStore::new(reminder_dir(&project, identity.target(), channel));
    match store.begin_check(now_unix(), CHECK_INTERVAL_SECS) {
        Ok(CheckPermit::Due) => {}
        _ => return,
    }
    let Ok(updater) = Updater::new() else {
        return;
    };
    let Ok(Ok(outcome)) =
        tokio::time::timeout(NOTIFY_BUDGET, updater.check(&identity, channel)).await
    else {
        return;
    };
    let Some(candidate) = outcome.candidate() else {
        return;
    };
    if let Ok(true) = store.record_notification(channel, &candidate.version) {
        eprintln!(
            "A newer bili {} is available ({}). Run `bili update` to install it.",
            candidate.version,
            channel_key(channel)
        );
    }
}

fn invalid(operation: &'static str) -> CoreError {
    CoreError::new(ErrorCode::InvalidInput, operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Cli;
    use clap::Parser;

    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).expect("parse")
    }

    #[test]
    fn flag_overrides_configuration_for_the_selected_channel() {
        let configured = UpdateSettings {
            allow_pre_release: true,
            ..UpdateSettings::default()
        };
        assert_eq!(effective_channel(None, configured), Channel::PreRelease);
        assert_eq!(effective_channel(Some(false), configured), Channel::Stable);
        assert_eq!(
            effective_channel(Some(true), UpdateSettings::default()),
            Channel::PreRelease
        );
        assert_eq!(
            effective_channel(None, UpdateSettings::default()),
            Channel::Stable
        );
    }

    #[test]
    fn pre_release_flag_accepts_only_the_strict_spelling() {
        let bare = parse(&["bili", "update"]);
        match bare.command {
            Command::Update { check, pre_release } => {
                assert!(!check);
                assert_eq!(pre_release, None);
            }
            _ => panic!("update command"),
        }
        let enabled = parse(&["bili", "update", "--pre-release"]);
        let disabled = parse(&["bili", "update", "--pre-release=false"]);
        for (cli, expected) in [(enabled, Some(true)), (disabled, Some(false))] {
            match cli.command {
                Command::Update { pre_release, .. } => assert_eq!(pre_release, expected),
                _ => panic!("update command"),
            }
        }
        assert!(Cli::try_parse_from(["bili", "update", "--pre-release", "true"]).is_err());
        assert!(Cli::try_parse_from(["bili", "update", "--check"]).is_ok());
    }

    #[test]
    fn only_business_commands_may_remind() {
        for arguments in [
            vec!["bili", "search", "query"],
            vec!["bili", "info", "BV1xx411c7mD"],
            vec!["bili", "resume", "job-id"],
        ] {
            let cli = parse(&arguments);
            assert!(notification_command(&cli.command), "{arguments:?}");
        }
        for arguments in [
            vec!["bili", "doctor"],
            vec!["bili", "update"],
            vec!["bili", "jobs"],
            vec!["bili", "logout"],
            vec!["bili", "login", "status"],
            vec!["bili", "clean", "--abandoned"],
        ] {
            let cli = parse(&arguments);
            assert!(!notification_command(&cli.command), "{arguments:?}");
        }
    }

    #[test]
    fn failures_keep_the_existing_exit_code_classes() {
        assert_eq!(output::exit_code(&map_update(UpdateError::InvalidInput)), 2);
        assert_eq!(
            output::exit_code(&map_update(UpdateError::UnsafeAssetName)),
            4
        );
        assert_eq!(output::exit_code(&map_update(UpdateError::Transport)), 4);
        assert_eq!(output::exit_code(&map_update(UpdateError::Busy)), 4);
        assert_eq!(output::exit_code(&map_update(UpdateError::Filesystem)), 4);
        assert_eq!(
            output::exit_code(&map_install(InstallError::NotPackaged)),
            2
        );
        assert_eq!(output::exit_code(&map_install(InstallError::Downgrade)), 4);
        assert_eq!(output::exit_code(&map_install(InstallError::Busy)), 4);
        assert_eq!(
            output::exit_code(&map_install(InstallError::MarkerInvalid)),
            4
        );
    }

    #[test]
    fn doctor_envelope_must_be_the_strict_schema() {
        let valid =
            br#"{"schema_version":1,"type":"doctor","data":{"build_identity":{},"native":{}}}"#
                .to_vec();
        assert!(doctor_report(valid).is_ok());
        for rejected in [
            br#"{"schema_version":1,"type":"update","data":{}}"#.to_vec(),
            br#"{"type":"doctor","data":{}}"#.to_vec(),
            br#"{"schema_version":2,"type":"doctor","data":{}}"#.to_vec(),
            br#"{"schema_version":1,"type":"doctor","data":[]}"#.to_vec(),
            br#"{"schema_version":1,"type":"doctor"}"#.to_vec(),
            br#"not json"#.to_vec(),
        ] {
            assert_eq!(doctor_report(rejected), Err(InstallError::Validator));
        }
    }

    #[test]
    fn oversized_doctor_output_is_rejected_instead_of_truncated() {
        // Complete, this payload would be valid JSON with trailing whitespace,
        // so accepting a truncated prefix would wrongly trust it.
        let mut bytes = br#"{"schema_version":1,"type":"doctor","data":{}}"#.to_vec();
        bytes.resize(MAX_DOCTOR_BYTES as usize + 1, b' ');
        assert_eq!(doctor_report(bytes), Err(InstallError::Validator));
    }

    #[test]
    fn reminder_directories_separate_target_and_selected_channel() {
        let project = directories::ProjectDirs::from("", "", "bili").expect("platform directories");
        let stable = reminder_dir(
            &project,
            bili_core::update::Target::LinuxX86_64Gnu,
            Channel::Stable,
        );
        let preview = reminder_dir(
            &project,
            bili_core::update::Target::LinuxX86_64Gnu,
            Channel::PreRelease,
        );
        assert_ne!(stable, preview);
        assert!(stable.ends_with("update/cli/x86_64-unknown-linux-gnu/stable"));
        assert!(preview.ends_with("update/cli/x86_64-unknown-linux-gnu/pre-release"));
    }
}
