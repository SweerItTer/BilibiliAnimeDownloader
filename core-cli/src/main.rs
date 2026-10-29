mod args;
mod output;
mod secret_input;
mod update;

use args::{AuthMode, AuthOptions, Category, Cli, Command, Danmaku, Login, MergeMode};
use bili_core::*;
use clap::Parser;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    io::{IsTerminal, Read},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    state_dir: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    jobs: Option<usize>,
    connections: Option<usize>,
    #[serde(default = "allow_fallback")]
    allow_plaintext_fallback: bool,
    /// Update preferences; absent in older configuration files.
    #[serde(default)]
    update: bili_core::update::UpdateSettings,
}
fn allow_fallback() -> bool {
    true
}
fn invalid(operation: &'static str) -> CoreError {
    CoreError::new(ErrorCode::InvalidInput, operation)
}

fn project_dirs() -> Result<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "bili")
        .ok_or_else(|| CoreError::new(ErrorCode::Filesystem, "locate private platform directories"))
}

/// Read configuration exactly once per invocation. Only an explicit `--config`
/// is consulted; there is no implicit settings file lookup.
fn settings(cli: &Cli) -> Result<Settings> {
    let Some(path) = &cli.config else {
        return Ok(Settings {
            allow_plaintext_fallback: true,
            ..Settings::default()
        });
    };
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(CoreError::io)?
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(CoreError::io)?;
    if bytes.len() > 65_536 {
        return Err(invalid("settings file exceeds 64 KiB"));
    }
    serde_json::from_slice::<Settings>(&bytes)
        .map_err(|_| invalid("invalid settings JSON or unsupported field"))
}

fn configuration(cli: &Cli, settings: &Settings) -> Result<CoreConfig> {
    let project = project_dirs()?;
    let output = settings
        .output_dir
        .clone()
        .unwrap_or_else(|| project.data_local_dir().join("downloads"));
    let state = cli
        .state_dir
        .clone()
        .or_else(|| settings.state_dir.clone())
        .unwrap_or_else(|| project.data_local_dir().join("state"));
    let mut config = CoreConfig::new(state, output);
    config.state_mode = match cli.command {
        Command::Download(_)
        | Command::Resume { .. }
        | Command::Merge { .. }
        | Command::Clean { .. } => StateMode::ReadWrite,
        _ => StateMode::ReadOnly,
    };
    config.max_jobs = settings.jobs.unwrap_or(3);
    config.max_connections = settings.connections.unwrap_or(6);
    if let Command::Download(options) = &cli.command {
        if let Some(path) = &options.output {
            config.output_dir = path.clone();
        }
        if let Some(n) = options.jobs {
            config.max_jobs = usize::from(n);
        }
        if let Some(n) = options.connections {
            config.max_connections = usize::from(n);
        }
    }
    config.state_dir = std::path::absolute(&config.state_dir).map_err(CoreError::io)?;
    config.output_dir = std::path::absolute(&config.output_dir).map_err(CoreError::io)?;
    // Anonymous/diagnostic invocations do not even construct a production store
    // policy; HOME isolation alone would not isolate the OS keyring.
    if invocation_needs_store(&cli.command) {
        config.credential_store = bili_core::cookie::StorePolicy::Remember {
            directory: project.config_dir().join("auth"),
            allow_plaintext_fallback: settings.allow_plaintext_fallback,
        };
    }
    Ok(config)
}

fn invocation_needs_store(command: &Command) -> bool {
    let auth = match command {
        Command::Search { auth, .. }
        | Command::Info { auth, .. }
        | Command::Qualities { auth, .. }
        | Command::Resume { auth, .. } => auth,
        Command::Download(options) => &options.auth,
        Command::Login {
            method: Login::Qr { remember } | Login::Cookie { remember, .. },
        } => return *remember,
        Command::Login {
            method: Login::Status,
        }
        | Command::Logout => return true,
        _ => return false,
    };
    matches!(auth.auth, AuthMode::Auto | AuthMode::Stored)
        || (auth.remember
            && matches!(
                auth.auth,
                AuthMode::Qr | AuthMode::CookieStdin | AuthMode::CookiePrompt
            ))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("Unable to create async runtime");
            return ExitCode::from(4);
        }
    };
    let code = runtime.block_on(run(cli));
    // Core workers are joined before runtime teardown. Only explicit redirected
    // stdin can leave an uninterruptible blocking reader until process exit.
    runtime.shutdown_timeout(Duration::from_secs(1));
    ExitCode::from(code)
}

async fn run(cli: Cli) -> u8 {
    let settings = match settings(&cli) {
        Ok(settings) => settings,
        Err(error) => {
            output::error(&error, cli.json);
            return output::exit_code(&error);
        }
    };
    if let Command::Update { check, pre_release } = &cli.command {
        return update::run(&cli, settings.update, *check, *pre_release).await;
    }
    let config = match configuration(&cli, &settings) {
        Ok(config) => config,
        Err(error) => {
            output::error(&error, cli.json);
            return output::exit_code(&error);
        }
    };
    let core = match Core::new(config.clone(), tokio::runtime::Handle::current()) {
        Ok(core) => core,
        Err(error) => {
            output::error(&error, cli.json);
            return output::exit_code(&error);
        }
    };
    let mut updates = core.jobs();
    let mut revisions = std::collections::HashMap::new();
    let monitor = matches!(
        cli.command,
        Command::Download(_) | Command::Resume { .. } | Command::Merge { .. }
    );
    let mut operation = Box::pin(execute(&core, &cli, &config));
    let signal = interrupt();
    tokio::pin!(signal);
    let mut interrupted = false;
    let result = loop {
        tokio::select! {
            result = &mut operation => break result,
            received = &mut signal => {
                if received.is_err() { break Err(CoreError::new(ErrorCode::Filesystem, "install interrupt handler")); }
                interrupted = true;
                break Err(CoreError::new(ErrorCode::Cancelled, "user interruption"));
            }
            changed = updates.changed(), if monitor => {
                if changed.is_err() { break Err(CoreError::new(ErrorCode::Shutdown, "job snapshot stream closed")); }
                let snapshot = updates.borrow_and_update().clone();
                let mut failed = None;
                for job in snapshot.jobs.iter().filter(|job| job.phase.is_active()) {
                    if revisions.insert(job.id, job.revision) != Some(job.revision) {
                        if let Err(error) = output::progress(job, cli.json) { failed = Some(error); break; }
                    }
                }
                if let Some(error) = failed { break Err(error); }
            }
        }
    };
    if interrupted {
        drop(operation);
        eprintln!("Interrupted: stopping workers and saving resumable state. Press Ctrl+C again to force exit (crash recovery required).");
        tokio::select! {
            report = core.shutdown(StopMode::Pause) => {
                if let Err(error) = report { output::error(&error, cli.json); }
            },
            _ = interrupt() => { std::process::exit(130); }
        }
        if cli.json {
            let _ = output::json("jobs", &**core.jobs().borrow());
            output::error(
                &CoreError::new(ErrorCode::Cancelled, "user interruption"),
                true,
            );
        }
        return 130;
    }
    let shutdown = core.shutdown(StopMode::Pause).await;
    let code = match (result, shutdown) {
        (Ok(code), Ok(_)) => code,
        (Err(error), _) | (Ok(_), Err(error)) => {
            output::error(&error, cli.json);
            output::exit_code(&error)
        }
    };
    update::maybe_notify(&cli, settings.update).await;
    code
}

async fn interrupt() -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        tokio::signal::ctrl_c().await
    }
    #[cfg(windows)]
    {
        let mut console_break = tokio::signal::windows::ctrl_break()?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            event = console_break.recv() => event.ok_or_else(||
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "console interrupt stream closed")),
        }
    }
}

async fn read_cookie(stdin: bool) -> Result<String> {
    if stdin {
        tokio::task::spawn_blocking(read_cookie_stdin_blocking)
            .await
            .map_err(|_| CoreError::new(ErrorCode::Corruption, "secret input worker failed"))?
    } else {
        if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Err(invalid(
                "hidden Cookie prompt requires a terminal; use explicit --auth cookie-stdin",
            ));
        }
        secret_input::prompt_cookie("Cookie (hidden, memory only unless --remember): ").await
    }
}

fn read_cookie_stdin_blocking() -> Result<String> {
    if std::io::stdin().is_terminal() {
        return Err(invalid(
            "Cookie stdin requires redirected input; use cookie-prompt for a terminal",
        ));
    }
    eprintln!("Reading Cookie from stdin into memory only. Do not embed secrets in shell command text or producer arguments.");
    let mut input = Zeroizing::new(String::new());
    std::io::stdin()
        .take(65_537)
        .read_to_string(&mut input)
        .map_err(|_| invalid("read Cookie stdin"))?;
    if input.len() > 65_536 {
        return Err(invalid("Cookie input exceeds 64 KiB"));
    }
    Ok(input.trim_end_matches(['\r', '\n']).to_owned())
}

async fn authenticate(core: &Core, options: &AuthOptions) -> Result<()> {
    if options.remember
        && matches!(
            options.auth,
            AuthMode::Auto | AuthMode::Stored | AuthMode::Anonymous
        )
    {
        return Err(invalid(
            "--remember requires qr, cookie-stdin or cookie-prompt authentication",
        ));
    }
    let snapshot = match options.auth {
        AuthMode::Auto => {
            core.auth(AuthAction::LoadStored { required: false })
                .await?
        }
        AuthMode::Stored => core.auth(AuthAction::LoadStored { required: true }).await?,
        AuthMode::Anonymous => core.auth(AuthAction::Anonymous).await?,
        AuthMode::Qr => {
            qr_login(core, options.remember).await?;
            return Ok(());
        }
        AuthMode::CookieStdin | AuthMode::CookiePrompt => {
            let header = read_cookie(matches!(options.auth, AuthMode::CookieStdin)).await?;
            core.auth(AuthAction::ImportCookie {
                header,
                remember: options.remember,
            })
            .await?
        }
    };
    if let Some(notice) = snapshot.notice {
        eprintln!("{notice}");
    }
    Ok(())
}

async fn qr_login(core: &Core, remember: bool) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(invalid("QR login requires an interactive terminal"));
    }
    let start = core.auth(AuthAction::StartQr { remember }).await?;
    let challenge = start
        .qr
        .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "QR challenge absent"))?;
    let code = qrcode::QrCode::new(challenge.login_url.as_bytes())
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "render QR challenge"))?;
    eprintln!(
        "Scan with Bilibili; this transient QR is not stored or printed as a URL.\n{}",
        code.render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(challenge.expires_in_secs);
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let snapshot = core.auth(AuthAction::PollQr).await?;
        if snapshot.logged_in {
            if let Some(notice) = snapshot.notice {
                eprintln!("{notice}");
            }
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline
            || matches!(
                snapshot.qr_status,
                Some(bili_core::auth::QrStatus::Expired | bili_core::auth::QrStatus::Cancelled)
            )
        {
            let _ = core.auth(AuthAction::CancelQr).await;
            return Err(CoreError::new(
                ErrorCode::LoginRequired,
                "QR session expired or cancelled; start a new login",
            ));
        }
    }
}

fn quality_part(parts: Vec<Part>, ordinal: Option<u32>) -> Result<Part> {
    match ordinal {
        Some(ordinal) => parts.into_iter().find(|p| p.ordinal == ordinal),
        None => parts.into_iter().next(),
    }
    .ok_or_else(|| invalid("part ordinal unavailable"))
}

fn selected_parts<'a>(parts: &'a [Part], selection: &str) -> Result<Vec<&'a Part>> {
    if selection == "all" {
        return Ok(parts.iter().collect());
    }
    let mut selected = BTreeSet::new();
    for item in selection.split(',') {
        let (start, end) = match item.split_once('-') {
            Some((a, b)) => (a.parse::<u32>(), b.parse::<u32>()),
            None => (item.parse::<u32>(), item.parse::<u32>()),
        };
        let (start, end) = (
            start.map_err(|_| invalid("invalid part selection"))?,
            end.map_err(|_| invalid("invalid part selection"))?,
        );
        if start == 0 || start > end {
            return Err(invalid("selected part ordinal is unavailable"));
        }
        // Iterate actual parts, not an attacker-sized numeric range; gaps are
        // errors, not silently omitted requested ordinals.
        let mut found = 0_u64;
        for part in parts.iter().filter(|p| (start..=end).contains(&p.ordinal)) {
            selected.insert(part.ordinal);
            found += 1;
        }
        if found != u64::from(end) - u64::from(start) + 1 {
            return Err(invalid("selected part ordinal is unavailable"));
        }
    }
    Ok(parts
        .iter()
        .filter(|p| selected.contains(&p.ordinal))
        .collect())
}

fn subtitle_selection(value: &str) -> Result<SubtitleSelection> {
    match value {
        "all" => Ok(SubtitleSelection::All),
        "none" => Ok(SubtitleSelection::None),
        _ => {
            let values: Vec<_> = value.split(',').collect();
            if values.iter().any(|v| v.is_empty()) {
                return Err(invalid("empty subtitle selector"));
            }
            if values
                .iter()
                .all(|v| v.starts_with("track:") && v.len() > 6)
            {
                Ok(SubtitleSelection::Tracks(
                    values.iter().map(|v| v[6..].to_owned()).collect(),
                ))
            } else if values.iter().any(|v| v.starts_with("track:")) {
                Err(invalid("do not mix subtitle track IDs and languages"))
            } else {
                Ok(SubtitleSelection::Languages(
                    values.iter().map(|v| (*v).to_owned()).collect(),
                ))
            }
        }
    }
}

async fn execute(core: &Core, cli: &Cli, config: &CoreConfig) -> Result<u8> {
    match &cli.command {
        Command::Search {
            query,
            category,
            page,
            auth,
        } => {
            authenticate(core, auth).await?;
            let category = match category {
                Category::All => SearchCategory::All,
                Category::Video => SearchCategory::Video,
                Category::Bangumi => SearchCategory::Bangumi,
                Category::Film => SearchCategory::Film,
            };
            let results = core
                .search(SearchQuery {
                    keyword: query.clone(),
                    category,
                    page: *page,
                })
                .await?;
            output::result("search", &results, cli.json)?;
        }
        Command::Info { input, auth } => {
            authenticate(core, auth).await?;
            output::result(
                "info",
                &core.resolve(input.as_str().into()).await?,
                cli.json,
            )?;
        }
        Command::Qualities { input, part, auth } => {
            authenticate(core, auth).await?;
            let item = core.resolve(input.as_str().into()).await?;
            let selected = quality_part(item.parts, *part)?;
            output::result(
                "qualities",
                &core.quality_catalog(selected.reference).await?,
                cli.json,
            )?;
        }
        Command::Login { method } => {
            match method {
                Login::Qr { remember } => {
                    qr_login(core, *remember).await?;
                    if !remember {
                        eprintln!("Session validated only: without --remember, later CLI processes remain unauthenticated.");
                    }
                }
                Login::Cookie { stdin, remember } => {
                    let header = read_cookie(*stdin).await?;
                    let auth = core
                        .auth(AuthAction::ImportCookie {
                            header,
                            remember: *remember,
                        })
                        .await?;
                    if let Some(notice) = &auth.notice {
                        eprintln!("{notice}");
                    }
                    if !remember {
                        eprintln!("Session validated only: without --remember, later CLI processes remain unauthenticated.");
                    }
                }
                Login::Status => {
                    let loaded = core
                        .auth(AuthAction::LoadStored { required: false })
                        .await?;
                    if let Some(notice) = loaded.notice {
                        eprintln!("{notice}");
                    }
                }
            }
            output::result("auth", &core.auth(AuthAction::Status).await?, cli.json)?;
        }
        Command::Logout => {
            output::result("auth", &core.auth(AuthAction::Logout).await?, cli.json)?;
        }
        Command::Jobs => {
            output::result("jobs", &**core.jobs().borrow(), cli.json)?;
        }
        Command::Download(options) => {
            authenticate(core, &options.auth).await?;
            let quality = if options.quality == "highest" {
                None
            } else {
                Some(
                    options
                        .quality
                        .parse::<u32>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| {
                            invalid("quality must be highest or a positive server ID")
                        })?,
                )
            };
            let subtitles = subtitle_selection(&options.subtitles)?;
            let mut jobs = Vec::new();
            let mut failures = 0;
            for input in &options.inputs {
                let item = match core.resolve(input.as_str().into()).await {
                    Ok(item) => item,
                    Err(error) => {
                        failures = failures.max(output::exit_code(&error));
                        output::error(&error, cli.json);
                        continue;
                    }
                };
                let parts = match selected_parts(&item.parts, &options.parts) {
                    Ok(parts) => parts,
                    Err(error) => {
                        failures = failures.max(output::exit_code(&error));
                        output::error(&error, cli.json);
                        continue;
                    }
                };
                for part in parts {
                    let selection = DownloadSelection {
                        part: part.reference.clone(),
                        title: format!("{} - {}", item.title, part.title),
                        quality,
                        video_representation: options.video_repr,
                        audio_representation: options.audio_repr,
                        codec: options.codec.clone(),
                        dynamic_range: options.hdr.clone(),
                        output: None,
                        merge: match options.merge {
                            MergeMode::Auto => MergePolicy::Automatic,
                            MergeMode::Manual => MergePolicy::Manual,
                        },
                        sidecars: SidecarPolicy {
                            subtitles: subtitles.clone(),
                            danmaku: matches!(options.danmaku, Danmaku::Current),
                        },
                        keep_temp: options.keep_temp,
                    };
                    match core.plan(selection).await {
                        Ok(plan) => {
                            if cli.json {
                                output::json("plan", &plan)?;
                            } else {
                                eprintln!("Selected {}", output::representation(&plan.video));
                                if let Some(audio) = &plan.audio {
                                    eprintln!("Audio: {}", output::representation(audio));
                                }
                            }
                            match core.enqueue(plan).await {
                                Ok(id) => jobs.push(id),
                                Err(error) => {
                                    failures = failures.max(output::exit_code(&error));
                                    output::error(&error, cli.json);
                                }
                            }
                        }
                        Err(error) => {
                            failures = failures.max(output::exit_code(&error));
                            output::error(&error, cli.json);
                        }
                    }
                }
            }
            return Ok(wait_jobs(core, &jobs, cli.json).await?.max(failures));
        }
        Command::Resume { job, all, auth } => {
            authenticate(core, auth).await?;
            let jobs: Vec<JobId> = if *all {
                core.jobs()
                    .borrow()
                    .jobs
                    .iter()
                    .filter(|j| {
                        j.phase != JobPhase::Completed
                            || j.components.iter().any(|c| c.error.is_some())
                    })
                    .map(|j| j.id)
                    .collect()
            } else {
                vec![job
                    .as_ref()
                    .ok_or_else(|| invalid("resume requires job or --all"))?
                    .parse()?]
            };
            for id in &jobs {
                core.control(*id, JobAction::Resume).await?;
            }
            return wait_jobs(core, &jobs, cli.json).await;
        }
        Command::Merge {
            job,
            video,
            audio,
            output,
        } => {
            let request = if let Some(id) = job {
                MergeRequest::Managed(id.parse()?)
            } else {
                MergeRequest::Local {
                    video: std::path::absolute(
                        video
                            .as_ref()
                            .ok_or_else(|| invalid("manual merge requires --video"))?,
                    )
                    .map_err(CoreError::io)?,
                    audio: audio
                        .as_ref()
                        .map(std::path::absolute)
                        .transpose()
                        .map_err(CoreError::io)?,
                    output: std::path::absolute(
                        output
                            .as_ref()
                            .ok_or_else(|| invalid("manual merge requires --output"))?,
                    )
                    .map_err(CoreError::io)?,
                }
            };
            let id = core.merge(request).await?;
            return wait_jobs(core, &[id], cli.json).await;
        }
        Command::Clean {
            job,
            abandoned,
            dry_run,
            include_resumable,
        } => {
            let report = core
                .clean(CleanupRequest {
                    job: job.as_ref().map(|id| id.parse()).transpose()?,
                    abandoned: *abandoned,
                    include_resumable: *include_resumable,
                    dry_run: *dry_run,
                })
                .await?;
            output::result("cleanup", &report, cli.json)?;
            if !report.errors.is_empty() {
                return Ok(4);
            }
        }
        // `run` intercepts this command before the media core is constructed.
        Command::Update { .. } => {
            return Err(invalid("update runs before the media core starts"));
        }
        Command::Doctor => {
            let native = bili_core::remux::diagnostics()?;
            output::result(
                "doctor",
                &serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
                    "state_dir": config.state_dir, "output_dir": config.output_dir,
                    "permissions": { "state": path_status(&config.state_dir), "output": path_status(&config.output_dir),
                        "settings": cli.config.as_ref().map(|p| path_status(p)) }, "native": native,
                    "build_identity": build_identity(),
                    "native_integration": "replaceable shared libraries; no FFmpeg executable", "license": "MIT WITH COMMONS CLAUSE (app); FFmpeg LGPL-2.1-or-later",
                }),
                cli.json,
            )?;
        }
    }
    Ok(0)
}

fn path_status(path: &std::path::Path) -> serde_json::Value {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let mut result = serde_json::json!({
                "exists": true, "symlink": metadata.file_type().is_symlink(),
                "readonly": metadata.permissions().readonly(),
            });
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode() & 0o777;
                result["unix_mode"] = format!("{mode:03o}").into();
                result["private_mode"] = (mode & 0o077 == 0).into();
            }
            #[cfg(windows)]
            {
                result["private_acl"] = "Core enforces private ACLs during storage operations; not inspected by this diagnostic".into();
            }
            result
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::json!({"exists": false})
        }
        Err(_) => serde_json::json!({"exists": null, "error": "metadata unavailable"}),
    }
}
fn build_identity() -> serde_json::Value {
    serde_json::json!({
        "product": bili_core::update::Product::Cli.as_str(),
        "target": env!("BILI_TARGET"),
        "version": env!("CARGO_PKG_VERSION"),
        "update_protocol": bili_core::update::PROTOCOL,
    })
}

/// The typed identity the updater acts on. It reads the same two Cargo-provided
/// values `build_identity` renders, so there is only one identity source.
fn compiled_identity() -> Result<bili_core::update::Identity> {
    let target = bili_core::update::Target::from_triple(env!("BILI_TARGET"))
        .ok_or_else(|| invalid("this build target has no update channel"))?;
    bili_core::update::Identity::new(
        bili_core::update::Product::Cli,
        target,
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|_| invalid("compiled version is not semantic versioning"))
}

async fn wait_jobs(core: &Core, ids: &[JobId], machine: bool) -> Result<u8> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut updates = core.jobs();
    loop {
        let snapshot = updates.borrow_and_update().clone();
        let mut terminal = 0;
        let mut code = 0;
        for id in ids {
            let Some(job) = snapshot.jobs.iter().find(|j| j.id == *id) else {
                continue;
            };
            let downloaded_only = job.phase == JobPhase::Downloaded
                && job
                    .plan
                    .as_ref()
                    .is_some_and(|p| p.merge == MergePolicy::Manual);
            if (job.phase.is_terminal() && job.phase != JobPhase::Downloaded) || downloaded_only {
                terminal += 1;
                code = code.max(job_exit_code(job));
            }
        }
        if terminal == ids.len() {
            for id in ids {
                if let Some(job) = snapshot.jobs.iter().find(|j| j.id == *id) {
                    output::result(
                        "result",
                        &serde_json::json!({"job": job, "exit_code": job_exit_code(job)}),
                        machine,
                    )?;
                }
            }
            return Ok(code);
        }
        updates
            .changed()
            .await
            .map_err(|_| CoreError::new(ErrorCode::Shutdown, "job snapshot stream closed"))?;
    }
}

fn job_exit_code(job: &JobSnapshot) -> u8 {
    if matches!(job.phase, JobPhase::Paused | JobPhase::Cancelled) {
        return 130;
    }
    let code = job
        .error
        .iter()
        .chain(job.components.iter().filter_map(|c| c.error.as_ref()))
        .map(output::exit_code)
        .max()
        .unwrap_or(0);
    if code == 0 && matches!(job.phase, JobPhase::Failed | JobPhase::NeedsAttention) {
        4
    } else {
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_settings_keep_conservative_update_defaults() {
        let settings: Settings =
            serde_json::from_str(r#"{"jobs":2}"#).expect("legacy settings parse");
        assert_eq!(settings.jobs, Some(2));
        assert!(settings.update.check_updates);
        assert!(!settings.update.allow_pre_release);
        assert!(!settings.update.auto_update);
        // Older files keep the documented fallback permission.
        assert!(settings.allow_plaintext_fallback);
    }

    #[test]
    fn update_preferences_are_read_from_configuration() {
        let settings: Settings =
            serde_json::from_str(r#"{"update":{"allow_pre_release":true,"check_updates":false}}"#)
                .expect("update settings parse");
        assert!(settings.update.allow_pre_release);
        assert!(!settings.update.check_updates);
        assert!(!settings.update.auto_update);
        assert!(serde_json::from_str::<Settings>(r#"{"unknown_field":1}"#).is_err());
    }

    fn part(ordinal: u32) -> Part {
        Part {
            reference: PartRef {
                content: ContentRef::Video(Aid::new(1).unwrap()),
                aid: Aid::new(1).unwrap(),
                cid: Cid::new(ordinal.into()).unwrap(),
                episode: None,
            },
            ordinal,
            title: String::new(),
            duration_ms: None,
            preview: false,
        }
    }

    #[test]
    fn anonymous_and_diagnostics_cannot_construct_a_production_store() {
        for argv in [
            vec!["bili", "doctor"],
            vec!["bili", "jobs"],
            vec!["bili", "info", "BV17x411w7KC", "--auth", "anonymous"],
            vec![
                "bili",
                "download",
                "BV17x411w7KC",
                "--auth",
                "anonymous",
                "--remember",
            ],
        ] {
            let cli = Cli::try_parse_from(argv).unwrap();
            assert!(matches!(
                configuration(&cli, &Settings::default())
                    .unwrap()
                    .credential_store,
                bili_core::cookie::StorePolicy::MemoryOnly
            ));
        }
    }

    #[test]
    fn qualities_keep_resolved_original_ordinals() {
        assert_eq!(quality_part(vec![part(13)], None).unwrap().ordinal, 13);
        assert_eq!(quality_part(vec![part(3)], None).unwrap().ordinal, 3);
        assert!(quality_part(vec![part(13)], Some(1)).is_err());
        assert_eq!(quality_part(vec![part(13)], Some(13)).unwrap().ordinal, 13);
    }

    #[test]
    fn ranges_are_bounded_by_real_parts() {
        let parts = vec![part(1), part(2), part(3)];
        assert_eq!(selected_parts(&parts, "1,2-3,2").unwrap().len(), 3);
        assert!(selected_parts(&parts, "1-4294967295").is_err());
        assert!(selected_parts(&parts, "3-1").is_err());
        assert!(selected_parts(&parts, "0").is_err());
        assert!(selected_parts(&[part(1), part(3)], "1-3").is_err());
    }
    #[test]
    fn selectors_do_not_silently_mix_tracks_and_languages() {
        assert!(matches!(
            subtitle_selection("track:12,track:13").unwrap(),
            SubtitleSelection::Tracks(_)
        ));
        assert!(subtitle_selection("track:12,en").is_err());
        assert!(subtitle_selection("en,").is_err());
    }
    #[test]
    fn completed_video_does_not_hide_failed_requested_components() {
        let mut job = JobSnapshot {
            id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
            revision: 1,
            plan: None,
            phase: JobPhase::Completed,
            downloaded: 0,
            total: None,
            output: None,
            skipped: false,
            error: None,
            components: vec![ComponentResult {
                name: "subtitles".into(),
                path: None,
                omissions: 0,
                error: Some(CoreError::new(ErrorCode::LoginRequired, "subtitle access")),
            }],
        };
        assert_eq!(job_exit_code(&job), 3);
        job.components.push(ComponentResult {
            name: "danmaku".into(),
            path: None,
            omissions: 0,
            error: Some(CoreError::new(ErrorCode::Filesystem, "sidecar output")),
        });
        assert_eq!(job_exit_code(&job), 4);
        job.phase = JobPhase::Paused;
        assert_eq!(job_exit_code(&job), 130);
    }
}
