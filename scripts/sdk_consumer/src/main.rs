use bili_core::{Core, CoreConfig, StateMode, StopMode};
use std::{env, path::PathBuf, process};

fn main() {
    if let Err(error) = run() {
        eprintln!("SDK consumer failed: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1).peekable();
    let mut probe = None;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--probe" {
            probe = Some(PathBuf::from(args.next().ok_or("--probe requires a path")?));
        } else if arg == "--help" || arg == "-h" {
            println!("Usage: bili-core-sdk-consumer [STATE_DIR OUTPUT_DIR] [--probe PATH]\n       bili-core-sdk-consumer --probe PATH");
            return Ok(());
        } else {
            positional.push(PathBuf::from(arg));
        }
    }

    if positional.is_empty() {
        let path = probe.ok_or("expected STATE_DIR OUTPUT_DIR or --probe PATH")?;
        let result = bili_core::remux::probe(&path)?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    if positional.len() != 2 {
        return Err("expected exactly STATE_DIR and OUTPUT_DIR".into());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let mut config = CoreConfig::new(positional[0].clone(), positional[1].clone());
        config.state_mode = StateMode::ReadOnly;
        let core = Core::new(config, tokio::runtime::Handle::current())?;
        let snapshot = core.jobs();
        let _initial_revision = snapshot.borrow().revision;
        core.shutdown(StopMode::Pause).await?;
        Ok::<(), bili_core::CoreError>(())
    })?;
    if let Some(path) = probe {
        let result = bili_core::remux::probe(&path)?;
        println!("{}", serde_json::to_string(&result)?);
    }
    Ok(())
}
