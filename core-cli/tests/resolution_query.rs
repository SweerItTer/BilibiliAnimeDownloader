//! Real CLI checks that reject invalid inputs before any network operation.
use bili_core::{Core, CoreConfig, StopMode};
use std::{
    fs, io,
    path::Path,
    process::{Command, Stdio},
};

fn command(root: &Path) -> Command {
    let home = root.join("home");
    let roaming = home.join("AppData/Roaming");
    let local = home.join("AppData/Local");
    fs::create_dir_all(&roaming).unwrap();
    fs::create_dir_all(&local).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_bili"));
    command
        .current_dir(root)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", roaming)
        .env("LOCALAPPDATA", local)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .arg("--state-dir")
        .arg(root.join("state"))
        .stdin(Stdio::null());
    command
}

#[tokio::test]
async fn invalid_queries_are_readonly_with_or_without_a_writer_and_do_not_echo_url_queries() {
    let root = tempfile::tempdir().unwrap();
    let args = [
        "--json",
        "download",
        "not-a-media-id",
        "https://example.invalid/?private=QUERY_CANARY",
        "--list-resolutions",
        "--auth",
        "anonymous",
    ];
    let first = command(root.path()).args(args).output().unwrap();
    assert_eq!(first.status.code(), Some(2), "{first:?}");
    assert!(!root.path().join("state").exists());
    assert!(!String::from_utf8_lossy(&first.stdout).contains("QUERY_CANARY"));
    let rows: Vec<serde_json::Value> = String::from_utf8(first.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    for (position, row) in rows.iter().enumerate() {
        assert_eq!(row["type"], "error");
        assert_eq!(
            row["data"]["input"],
            serde_json::json!({"index":position+1,"content":null})
        );
        assert_eq!(row["data"]["exit_code"], 2);
    }
    let writer = Core::new(
        CoreConfig::new(root.path().join("state"), root.path().join("output")),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let before = fs::read(root.path().join("state/queue.json")).unwrap();
    let mut events = writer.subscribe();
    let second = command(root.path()).args(args).output().unwrap();
    assert_eq!(second.status.code(), Some(2), "{second:?}");
    assert_eq!(
        fs::read(root.path().join("state/queue.json")).unwrap(),
        before
    );
    assert!(!root.path().join("output").exists());
    assert!(writer.jobs().borrow().jobs.is_empty());
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    writer.shutdown(StopMode::Pause).await.unwrap();
    let human = command(root.path())
        .args([
            "download",
            "not-a-media-id",
            "--list-resolutions",
            "--auth",
            "anonymous",
        ])
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(2));
    assert!(human.stdout.is_empty());
    assert!(String::from_utf8(human.stderr)
        .unwrap()
        .starts_with("Input 1:"));
}

#[test]
fn help_conflicts_and_closed_query_stdout_are_offline() {
    let root = tempfile::tempdir().unwrap();
    let help = command(root.path())
        .args(["download", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("--list-resolutions"));
    let conflict = command(root.path())
        .args([
            "download",
            "av1",
            "--list-resolutions",
            "--quality",
            "highest",
        ])
        .output()
        .unwrap();
    assert_eq!(conflict.status.code(), Some(2));
    let (reader, writer) = io::pipe().unwrap();
    drop(reader);
    let failed = command(root.path())
        .args([
            "--json",
            "download",
            "not-a-media-id",
            "--list-resolutions",
            "--auth",
            "anonymous",
        ])
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(4));
    assert!(!String::from_utf8_lossy(&failed.stderr).contains("panicked"));
    assert!(!root.path().join("state").exists());
}
