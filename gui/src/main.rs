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
use std::path::PathBuf;

fn configuration(prefs: &GuiPrefs) -> bili_core::Result<CoreConfig> {
    let project = directories::ProjectDirs::from("", "", "bili").ok_or_else(|| {
        CoreError::new(ErrorCode::Filesystem, "locate private platform directories")
    })?;
    let mut state = project.data_local_dir().join("state");
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--state-dir" {
            state = PathBuf::from(arguments.next().ok_or_else(|| {
                CoreError::new(ErrorCode::InvalidInput, "--state-dir requires a directory")
            })?);
        } else {
            return Err(CoreError::new(
                ErrorCode::InvalidInput,
                "supported GUI option: --state-dir PATH",
            ));
        }
    }
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
    if std::env::args_os()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        println!("Bili native desktop\nUsage: gui [--state-dir PATH]\nThe state directory defaults to the platform bili data directory. Login is session-only unless you choose to keep it in the OS credential store.");
        return;
    }
    let prefs = GuiPrefs::load();
    let config = configuration(&prefs);
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
            Ok(Box::new(app::App::new(
                bridge,
                output,
                state,
                font_missing,
                startup_error,
                prefs,
            )))
        }),
    );
    if let Some(runtime) = runtime {
        if let Ok(owner) = owner_rx.recv() {
            // The native event loop has ended; all application-owned work is
            // cancelled/paused and joined before the one runtime is dropped.
            runtime.block_on(owner.finish());
        }
        drop(runtime);
    }
    if let Err(error) = result {
        eprintln!("Unable to open native Bili window: {error}");
    }
}
