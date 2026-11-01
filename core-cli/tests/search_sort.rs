//! Exercise the actual CLI's offline validation and JSON contract.
use std::{
    fs,
    process::{Command, Stdio},
};

fn command(root: &std::path::Path) -> Command {
    let home = root.join("home");
    let roaming = home.join("AppData/Roaming");
    let local = home.join("AppData/Local");
    fs::create_dir_all(&roaming).unwrap();
    fs::create_dir_all(&local).unwrap();
    let config = root.join("config.json");
    fs::write(&config, r#"{"update":{"check_updates":false}}"#).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_bili"));
    command
        .current_dir(root)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", roaming)
        .env("LOCALAPPDATA", local)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "--state-dir",
            root.join("state").to_str().unwrap(),
        ])
        .stdin(Stdio::null());
    command
}

#[test]
fn unsupported_sort_combinations_are_typed_readonly_errors() {
    let root = tempfile::tempdir().unwrap();
    for category in ["all", "bangumi", "film"] {
        for sort in ["views", "newest", "danmaku", "favorites", "comments"] {
            let output = command(root.path())
                .args([
                    "--json",
                    "search",
                    "sample",
                    "--type",
                    category,
                    "--sort",
                    sort,
                    "--auth",
                    "anonymous",
                ])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            let rows = String::from_utf8(output.stdout).unwrap();
            assert_eq!(rows.lines().count(), 1);
            let row: serde_json::Value = serde_json::from_str(&rows).unwrap();
            assert_eq!(row["schema_version"], 1);
            assert_eq!(row["type"], "error");
            assert_eq!(row["data"]["error"]["code"], "InvalidInput");
            assert_eq!(row["data"]["error"]["retryable"], false);
            assert_eq!(row["data"]["exit_code"], 2);
            assert!(!root.path().join("state").exists());
        }
    }
}

#[test]
fn search_help_lists_sorting_and_unknown_choice_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let help = command(root.path())
        .args(["search", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--sort"));
    assert!(help.contains("favorites"));
    let invalid = command(root.path())
        .args(["search", "sample", "--sort", "random"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(!root.path().join("state").exists());
}
