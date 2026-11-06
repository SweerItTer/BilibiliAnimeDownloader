#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod backend;
mod background;
mod covers;
mod detail;
mod download_config;
mod download_panel;
mod isolation;
mod login;
mod prefs;
mod render_diagnostics;
mod search;
mod theme;
mod update;
mod verification;
mod window;

use bili_core::{CoreConfig, CoreError, ErrorCode};
use prefs::GuiPrefs;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
enum Launch {
    Window {
        state_dir: Option<PathBuf>,
        profile_dir: Option<PathBuf>,
    },
    Version,
    Help,
    Doctor,
}

/// Classify options before reading preferences or creating any GUI resources.
/// Values belonging to an option are paths, even when spelled `doctor`.
fn launch_request(arguments: &[OsString]) -> bili_core::Result<Launch> {
    let invalid = || {
        CoreError::new(
            ErrorCode::InvalidInput,
            "supported GUI options: --state-dir PATH, --profile-dir PATH, --version, --help, --json doctor",
        )
    };
    let mut state_dir = None;
    let mut profile_dir = None;
    let mut json = false;
    let mut doctor = false;
    let mut version = false;
    let mut help = false;
    let mut args = arguments.iter();
    while let Some(argument) = args.next() {
        if argument == "--state-dir" {
            state_dir = Some(PathBuf::from(args.next().ok_or_else(invalid)?));
        } else if argument == "--profile-dir" && profile_dir.is_none() {
            profile_dir = Some(PathBuf::from(args.next().ok_or_else(invalid)?));
        } else if argument == "--json" && !json {
            json = true;
        } else if argument == "doctor" && !doctor {
            doctor = true;
        } else if argument == "--version" && !version {
            version = true;
        } else if (argument == "--help" || argument == "-h") && !help {
            help = true;
        } else {
            return Err(invalid());
        }
    }
    if state_dir.is_some() && profile_dir.is_some() {
        return Err(invalid());
    }
    if u8::from(doctor) + u8::from(version) + u8::from(help) > 1 || json != doctor {
        return Err(invalid());
    }
    Ok(if doctor {
        Launch::Doctor
    } else if version {
        Launch::Version
    } else if help {
        Launch::Help
    } else {
        Launch::Window {
            state_dir,
            profile_dir,
        }
    })
}

fn configuration(
    prefs: &GuiPrefs,
    state_dir: Option<PathBuf>,
    profile_dir: Option<&std::path::Path>,
) -> bili_core::Result<CoreConfig> {
    let (state, output, auth) = if let Some(root) = profile_dir {
        (root.join("state"), root.join("output"), root.join("auth"))
    } else {
        let project = directories::ProjectDirs::from("", "", "bili").ok_or_else(|| {
            CoreError::new(ErrorCode::Filesystem, "locate private platform directories")
        })?;
        (
            project.data_local_dir().join("state"),
            project.data_local_dir().join("downloads"),
            project.config_dir().join("auth-gui"),
        )
    };
    let state = std::path::absolute(state_dir.unwrap_or(state)).map_err(CoreError::io)?;
    let output =
        std::path::absolute(prefs.output.clone().unwrap_or(output)).map_err(CoreError::io)?;
    let mut config = CoreConfig::new(state, output);
    #[cfg(windows)]
    {
        config.recycle = Some(bili_core::RecycleBin {
            move_files: backend::recycle_files,
        });
    }
    // A store policy alone persists nothing: core writes only when the user
    // asks to keep the login. Nothing is loaded until the GUI explicitly sends
    // LoadStored on startup (only when the preference is enabled).
    config.credential_store = bili_core::cookie::StorePolicy::Remember {
        // A GUI-specific store keeps "keep login" independent of the CLI account.
        directory: auth,
        allow_plaintext_fallback: false,
    };
    Ok(config)
}

fn window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/icon-256.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let (state_dir, profile_dir) = match launch_request(&arguments) {
        Ok(Launch::Window {
            state_dir,
            profile_dir,
        }) => (state_dir, profile_dir),
        Ok(Launch::Doctor) => {
            match update::doctor() {
                Ok(report) => println!("{report}"),
                Err(error) => {
                    println!(
                        "{}",
                        serde_json::json!({"schema_version": 1, "type": "error", "data": {"error": error, "exit_code": 4}})
                    );
                    std::process::exit(4);
                }
            }
            return;
        }
        Ok(Launch::Version) => {
            println!("bili {}", update::VERSION);
            return;
        }
        Ok(Launch::Help) => {
            println!("Bili native desktop\nUsage: bili [--state-dir PATH | --profile-dir PATH] [--version] [--json doctor]\nThe state directory defaults to the platform bili data directory. Login is session-only unless you choose to keep it in the OS credential store.");
            return;
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let profile_dir = profile_dir
        .map(|path| std::path::absolute(path).map_err(CoreError::io))
        .transpose()
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        });
    if profile_dir.is_some()
        && [
            "BILI_GUI_ISOLATED_DEBUG_DIR",
            "BILI_GUI_ANONYMOUS_LIVE_DIR",
            "BILI_GUI_OFFLINE_REPLAY",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        eprintln!("--profile-dir cannot be combined with debug isolation or replay");
        std::process::exit(2);
    }
    let isolation = match isolation::DebugIsolation::from_environment(state_dir.as_deref()) {
        Ok(isolation) => isolation,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let (prefs, config) = match &isolation {
        Some(isolation) => (isolation.preferences(), Ok(isolation.configuration())),
        None => {
            let prefs = profile_dir.as_ref().map_or_else(GuiPrefs::load, |root| {
                GuiPrefs::load_profile(
                    root.join("preferences/gui-preferences.json"),
                    root.join("output"),
                )
            });
            let config = configuration(&prefs, state_dir, profile_dir.as_deref());
            (prefs, config)
        }
    };
    let output = config
        .as_ref()
        .map(|c| c.output_dir.clone())
        .unwrap_or_default();
    let state = config
        .as_ref()
        .map(|c| c.state_dir.clone())
        .unwrap_or_default();
    let fonts = theme::system_fonts();
    let font_missing = fonts.is_none();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    let (runtime, startup_error) = match runtime {
        Ok(runtime) => (Some(runtime), None),
        Err(_) => (
            None,
            Some("无法创建异步运行环境，请关闭应用后重试。".to_owned()),
        ),
    };
    let handle = runtime.as_ref().map(|r| r.handle().clone());
    let (updates, update_owner) = match (&handle, isolation.is_none() && profile_dir.is_none()) {
        (Some(handle), true) => {
            let (updates, owner) = update::UpdateState::connected(handle);
            (updates, Some(owner))
        }
        _ => (update::UpdateState::default(), None),
    };
    let (restart_tx, restart_rx) = std::sync::mpsc::channel();
    // The single-slot handoff keeps ownership in main, including native window
    // creation errors. Neither paint nor on_exit waits on asynchronous work.
    let (owner_tx, owner_rx) = std::sync::mpsc::sync_channel(1);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 800.0])
        .with_min_inner_size([900.0, 640.0])
        .with_decorations(false)
        .with_title(window::WINDOW_TITLE);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Bili",
        options,
        Box::new(move |cc| {
            render_diagnostics::report_renderer(cc);
            theme::install(&cc.egui_ctx, fonts);
            prefs.apply(&cc.egui_ctx);
            let bridge = handle.map(|handle| {
                let (bridge, owner) = backend::Bridge::start(config, &handle, cc.egui_ctx.clone());
                let _ = owner_tx.send(owner);
                bridge
            });
            let mut app = app::App::new(bridge, output, state, font_missing, startup_error, prefs);
            app.attach_updates(updates, restart_tx);
            Ok(Box::new(app))
        }),
    );
    if let Some(runtime) = runtime {
        if let Ok(owner) = owner_rx.recv() {
            // The native event loop has ended; all application-owned work is
            // cancelled/paused and joined before the one runtime is dropped.
            runtime.block_on(owner.finish());
        }
        if let Some(owner) = update_owner {
            runtime.block_on(owner.finish());
        }
        drop(runtime);
    }
    if let Err(error) = result {
        eprintln!("Unable to open native Bili window: {error}");
    }
    if let Ok(entry) = restart_rx.try_recv() {
        let entry = std::fs::canonicalize(entry);
        match entry {
            Ok(entry) => {
                let mut command = std::process::Command::new(entry);
                command.args(&arguments);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x08000000);
                }
                if command.spawn().is_err() {
                    eprintln!("Unable to restart Bili; open the installation entry manually.");
                }
            }
            Err(_) => eprintln!("Unable to locate the updated installation entry."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> bili_core::Result<Launch> {
        launch_request(
            &arguments
                .iter()
                .map(|argument| OsString::from(*argument))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn option_values_are_never_interpreted_as_headless_commands() {
        for value in ["doctor", "--version", "--help", "--json"] {
            assert_eq!(
                parse(&["--state-dir", value]).unwrap(),
                Launch::Window {
                    state_dir: Some(PathBuf::from(value)),
                    profile_dir: None
                }
            );
        }
        assert_eq!(
            parse(&[]).unwrap(),
            Launch::Window {
                state_dir: None,
                profile_dir: None
            }
        );
    }

    #[test]
    fn headless_doctor_requires_json_and_skips_option_values() {
        for arguments in [
            vec!["--json", "doctor"],
            vec!["--state-dir", "doctor", "--json", "doctor"],
            vec!["doctor", "--state-dir", "--json", "--json"],
        ] {
            assert_eq!(parse(&arguments).unwrap(), Launch::Doctor);
        }
        for arguments in [
            vec!["doctor"],
            vec!["--json"],
            vec!["--json", "doctor", "--version"],
            vec!["--json", "doctor", "--state-dir"],
            vec!["--json", "doctor", "doctor"],
        ] {
            assert!(parse(&arguments).is_err(), "accepted {arguments:?}");
        }
    }

    #[test]
    fn explicit_profile_has_real_network_and_a_distinct_secure_store() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let prefs = GuiPrefs::load_profile(
            root.join("preferences/gui-preferences.json"),
            root.join("output"),
        );
        let config = configuration(&prefs, None, Some(root)).unwrap();
        assert!(config.api_replay.is_none());
        assert_eq!(config.state_dir, root.join("state"));
        assert_eq!(config.output_dir, root.join("output"));
        assert!(
            matches!(config.credential_store, bili_core::cookie::StorePolicy::Remember {directory, allow_plaintext_fallback: false} if directory == root.join("auth"))
        );
        assert!(
            !root.join("auth").exists(),
            "configuration must not access the OS credential store"
        );
        assert_eq!(
            parse(&["--profile-dir", "profile"]).unwrap(),
            Launch::Window {
                state_dir: None,
                profile_dir: Some("profile".into())
            }
        );
        assert!(parse(&["--state-dir", "state", "--profile-dir", "profile"]).is_err());
        assert!(parse(&["--profile-dir"]).is_err());
    }

    #[test]
    fn informational_modes_only_match_option_positions() {
        assert_eq!(parse(&["--version"]).unwrap(), Launch::Version);
        assert_eq!(parse(&["--help"]).unwrap(), Launch::Help);
        assert_eq!(parse(&["-h"]).unwrap(), Launch::Help);
        assert!(parse(&["--state-dir"]).is_err());
        assert!(parse(&["--unsupported"]).is_err());
    }
}
