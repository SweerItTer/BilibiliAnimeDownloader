use bili_core::{Core, CoreConfig, ErrorCode, JobPhase, MergeRequest, StateMode, StopMode};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

fn config(root: &std::path::Path) -> CoreConfig {
    CoreConfig::new(root.join("state"), root.join("output"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_directory_writer_is_exclusive_and_readers_do_not_mutate() {
    let root = tempfile::tempdir().unwrap();
    let writer = Core::new(config(root.path()), tokio::runtime::Handle::current()).unwrap();
    assert!(
        matches!(Core::new(config(root.path()),tokio::runtime::Handle::current()),Err(e) if e.code==ErrorCode::Busy)
    );
    let mut readonly = config(root.path());
    readonly.state_mode = StateMode::ReadOnly;
    let reader = Core::new(readonly, tokio::runtime::Handle::current()).unwrap();
    assert!(reader.jobs().borrow().jobs.is_empty());
    let error = reader
        .merge(MergeRequest::Local {
            video: root.path().join("v"),
            audio: None,
            output: root.path().join("out"),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ReadOnly);
    let other = tempfile::tempdir().unwrap();
    let independent = Core::new(config(other.path()), tokio::runtime::Handle::current()).unwrap();
    independent.shutdown(StopMode::Pause).await.unwrap();
    reader.shutdown(StopMode::Pause).await.unwrap();
    writer.shutdown(StopMode::Pause).await.unwrap();
    let replacement = Core::new(config(root.path()), tokio::runtime::Handle::current()).unwrap();
    replacement.shutdown(StopMode::Pause).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_readonly_inspection_creates_no_state() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = config(root.path());
    settings.state_mode = StateMode::ReadOnly;
    let core = Core::new(settings, tokio::runtime::Handle::current()).unwrap();
    assert!(core.jobs().borrow().jobs.is_empty());
    core.shutdown(StopMode::Pause).await.unwrap();
    assert!(!root.path().join("state").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_merge_never_overwrites_or_deletes_unmanaged_inputs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.media");
    let output = root.path().join("chosen.mp4");
    std::fs::write(&source, b"not a media container").unwrap();
    std::fs::write(&output, b"unrelated output").unwrap();
    let core = Core::new(config(root.path()), tokio::runtime::Handle::current()).unwrap();
    let error = core
        .merge(MergeRequest::Local {
            video: source.clone(),
            audio: None,
            output: output.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(std::fs::read(&output).unwrap(), b"unrelated output");
    std::fs::remove_file(&output).unwrap();
    let id = core
        .merge(MergeRequest::Local {
            video: source.clone(),
            audio: None,
            output: output.clone(),
        })
        .await
        .unwrap();
    let mut jobs = core.jobs();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let terminal = jobs.borrow().jobs.iter().any(|j| {
                j.id == id && matches!(j.phase, JobPhase::Failed | JobPhase::NeedsAttention)
            });
            if terminal {
                break;
            }
            jobs.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(source).unwrap(), b"not a media container");
    assert!(!output.exists());
    core.shutdown(StopMode::Pause).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_schema_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(
        state.join("queue.json"),
        br#"{"schema_version":900,"revision":0,"jobs":[]}"#,
    )
    .unwrap();
    assert!(
        matches!(Core::new(config(root.path()),tokio::runtime::Handle::current()),Err(e) if e.code==ErrorCode::Corruption)
    );
    assert!(std::fs::read_to_string(state.join("queue.json"))
        .unwrap()
        .contains("900"));
}

#[test]
#[ignore = "subprocess helper invoked by crash_releases_os_writer_lock"]
fn lock_child() {
    let Some(root) = std::env::var_os("BILI_LOCK_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _core = Core::new(config(&root), runtime.handle().clone()).unwrap();
    std::fs::write(root.join("ready"), b"locked").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_releases_os_writer_lock() {
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_child", "--ignored", "--nocapture"])
        .env("BILI_LOCK_TEST_ROOT", root.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if root.path().join("ready").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !root.path().join("ready").exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("lock child did not become ready");
    }
    assert!(
        matches!(Core::new(config(root.path()),tokio::runtime::Handle::current()),Err(e) if e.code==ErrorCode::Busy)
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let core = Core::new(config(root.path()), tokio::runtime::Handle::current()).unwrap();
    core.shutdown(StopMode::Pause).await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlink_state_directory_is_not_followed() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("state")).unwrap();
    assert!(
        matches!(Core::new(config(root.path()),tokio::runtime::Handle::current()),Err(e) if e.code==ErrorCode::Conflict)
    );
    assert!(!outside.path().join("jobs").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_durable_admission_poison_stops_later_unacknowledged_work() {
    let root = tempfile::tempdir().unwrap();
    let settings = config(root.path());
    let core = Core::new(settings.clone(), tokio::runtime::Handle::current()).unwrap();
    let source = root.path().join("original.media");
    std::fs::write(&source, b"retained original").unwrap();
    let index = settings.state_dir.join("queue.json");
    let saved = settings.state_dir.join("saved-index.json");
    std::fs::rename(&index, &saved).unwrap();
    std::fs::create_dir(&index).unwrap();
    let output = root.path().join("first.mp4");
    let error = core
        .merge(MergeRequest::Local {
            video: source.clone(),
            audio: None,
            output: output.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Filesystem);
    assert!(!output.exists());
    assert!(core
        .jobs()
        .borrow()
        .jobs
        .iter()
        .all(|j| j.phase == JobPhase::Failed));
    // Repairing the disk externally cannot silently resume the poisoned writer.
    std::fs::remove_dir(&index).unwrap();
    std::fs::rename(&saved, &index).unwrap();
    let second = root.path().join("second.mp4");
    assert_eq!(
        core.merge(MergeRequest::Local {
            video: source.clone(),
            audio: None,
            output: second.clone()
        })
        .await
        .unwrap_err()
        .code,
        ErrorCode::Filesystem
    );
    assert!(!second.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"retained original");
    assert_eq!(
        core.shutdown(StopMode::Pause).await.unwrap_err().code,
        ErrorCode::Filesystem
    );
    drop(core);
    let restarted = Core::new(settings, tokio::runtime::Handle::current()).unwrap();
    assert!(restarted.jobs().borrow().jobs.is_empty());
    restarted.shutdown(StopMode::Pause).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_acknowledgement_has_no_active_native_worker_or_promotion() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("original.media");
    std::fs::write(&source, b"invalid but retained").unwrap();
    let output = root.path().join("paused.mp4");
    let core = Core::new(config(root.path()), tokio::runtime::Handle::current()).unwrap();
    let id = core
        .merge(MergeRequest::Local {
            video: source.clone(),
            audio: None,
            output: output.clone(),
        })
        .await
        .unwrap();
    core.control(id, bili_core::JobAction::Pause).await.unwrap();
    let snapshot = core.jobs().borrow().clone();
    assert_eq!(
        snapshot.jobs.iter().find(|j| j.id == id).unwrap().phase,
        JobPhase::Paused
    );
    assert!(!output.exists());
    assert_eq!(std::fs::read(source).unwrap(), b"invalid but retained");
    core.shutdown(StopMode::Pause).await.unwrap();
}
