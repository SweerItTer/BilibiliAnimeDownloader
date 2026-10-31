#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod backend;
mod background;
mod covers;
mod detail;
mod download_panel;
mod login;
mod prefs;
mod search;
mod theme;
mod update;
mod window;

use bili_core::{CoreConfig, CoreError, ErrorCode};
use prefs::GuiPrefs;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
enum Launch {
    Window { state_dir: Option<PathBuf> },
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
            "supported GUI options: --state-dir PATH, --version, --help, --json doctor",
        )
    };
    let mut state_dir = None;
    let mut json = false;
    let mut doctor = false;
    let mut version = false;
    let mut help = false;
    let mut args = arguments.iter();
    while let Some(argument) = args.next() {
        if argument == "--state-dir" {
            state_dir = Some(PathBuf::from(args.next().ok_or_else(invalid)?));
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
        Launch::Window { state_dir }
    })
}

fn configuration(prefs: &GuiPrefs, state_dir: Option<PathBuf>) -> bili_core::Result<CoreConfig> {
    let project = directories::ProjectDirs::from("", "", "bili").ok_or_else(|| {
        CoreError::new(ErrorCode::Filesystem, "locate private platform directories")
    })?;
    let state = state_dir.unwrap_or_else(|| project.data_local_dir().join("state"));
    let state = std::path::absolute(state).map_err(CoreError::io)?;
    let output = prefs
        .output
        .clone()
        .unwrap_or_else(|| project.data_local_dir().join("downloads"));
    let output = std::path::absolute(output).map_err(CoreError::io)?;
    let mut config = CoreConfig::new(state, output);
    // A store policy alone persists nothing: core writes only when the user
    // asks to keep the login. Nothing is loaded until the GUI explicitly sends
    // LoadStored on startup (only when the preference is enabled).
    config.credential_store = bili_core::cookie::StorePolicy::Remember {
        // A GUI-specific store keeps "keep login" independent of the CLI account.
        directory: project.config_dir().join("auth-gui"),
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
    let state_dir = match launch_request(&arguments) {
        Ok(Launch::Window { state_dir }) => state_dir,
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
            println!("Bili native desktop\nUsage: bili [--state-dir PATH] [--version] [--json doctor]\nThe state directory defaults to the platform bili data directory. Login is session-only unless you choose to keep it in the OS credential store.");
            return;
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let prefs = GuiPrefs::load();
    let config = configuration(&prefs, state_dir);
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
    let (updates, update_owner) = match &handle {
        Some(handle) => {
            let (updates, owner) = update::UpdateState::connected(handle);
            (updates, Some(owner))
        }
        None => (update::UpdateState::default(), None),
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
                    state_dir: Some(PathBuf::from(value))
                }
            );
        }
        assert_eq!(parse(&[]).unwrap(), Launch::Window { state_dir: None });
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
    fn informational_modes_only_match_option_positions() {
        assert_eq!(parse(&["--version"]).unwrap(), Launch::Version);
        assert_eq!(parse(&["--help"]).unwrap(), Launch::Help);
        assert_eq!(parse(&["-h"]).unwrap(), Launch::Help);
        assert!(parse(&["--state-dir"]).is_err());
        assert!(parse(&["--unsupported"]).is_err());
    }
}
