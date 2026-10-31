//! Offline acceptance against actual source-built Windows packages.
//!
//! This ignored test requires a clean extracted baseline installation, a newer
//! descriptor and its exact archive. It copies the baseline before touching it;
//! never point the fixture variables at a user's live installation or profile.

use super::*;
use bili_core::update::{Descriptor, UpdateCandidate};
use std::fs;
use std::os::windows::ffi::OsStrExt;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name} fixture")))
}

/// Retain every original byte, including both published inventories. Exact
/// equality also preserves their recorded hashes without an external tool.
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        for entry in fs::read_dir(root.join(&relative)).expect("fixture directory") {
            let entry = entry.expect("fixture entry");
            let path = relative.join(entry.file_name());
            let metadata = entry.metadata().expect("fixture metadata");
            assert!(!entry.file_type().expect("fixture type").is_symlink());
            if metadata.is_dir() {
                stack.push(path);
            } else {
                assert!(metadata.is_file());
                files.push((path, fs::read(entry.path()).expect("fixture bytes")));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn assert_preserved(root: &Path, files: &[(PathBuf, Vec<u8>)], activated: bool) {
    for (relative, bytes) in files {
        if activated && relative == Path::new(install::CURRENT_FILE) {
            continue;
        }
        assert_eq!(
            fs::read(root.join(relative)).expect("preserved file"),
            *bytes
        );
    }
}

fn isolated_version(entry: &Path, canonical: bool) -> std::io::Result<std::process::Output> {
    let entry = if canonical {
        fs::canonicalize(entry)?
    } else {
        entry.to_path_buf()
    };
    let (windows, system) = windows_directories().expect("Windows system directories");
    std::process::Command::new(entry)
        .arg("--version")
        .env_clear()
        .env("SystemRoot", &windows)
        .env("WINDIR", &windows)
        .env(
            "PATH",
            std::env::join_paths([system, windows]).expect("system PATH"),
        )
        .output()
}

#[test]
#[ignore = "requires BILI_TEST_UPDATE_BASELINE, BILI_TEST_UPDATE_DESCRIPTOR and BILI_TEST_UPDATE_ARCHIVE"]
fn offline_windows_packages_update_at_multiple_depths() {
    let baseline = fixture("BILI_TEST_UPDATE_BASELINE");
    let archive = fixture("BILI_TEST_UPDATE_ARCHIVE");
    let descriptor: Descriptor = serde_json::from_slice(
        &fs::read(fixture("BILI_TEST_UPDATE_DESCRIPTOR")).expect("descriptor fixture"),
    )
    .expect("descriptor JSON");
    assert_eq!(descriptor.schema_version, PROTOCOL);
    let running = crate::compiled_identity().expect("compiled CLI identity");
    assert_eq!(descriptor.product, running.product());
    assert_eq!(descriptor.target, running.target());
    let candidate = UpdateCandidate {
        tag: format!("v{}", descriptor.version),
        version: descriptor.version.clone(),
        channel: Channel::of(&descriptor.version),
        descriptor,
    };
    let expected = Identity::new(
        running.product(),
        running.target(),
        &candidate.version.to_string(),
    )
    .expect("candidate identity");
    let original = snapshot(&baseline);

    for (case, minimum_length, name) in [
        (0, 0, "shallow"),
        (1, 180, "nested"),
        (2, 300, "space and 目录"),
    ] {
        let temporary = tempfile::TempDir::new().expect("acceptance tempdir");
        let mut root = temporary.path().join(name);
        while root.as_os_str().encode_wide().count() < minimum_length {
            root.push("nested-0123456789");
        }
        for (relative, bytes) in &original {
            let file = root.join(relative);
            fs::create_dir_all(file.parent().expect("fixture parent")).expect("copy directory");
            fs::write(file, bytes).expect("copy package file");
        }
        fs::write(root.join("user-sentinel.txt"), b"preserve user file").expect("user fixture");
        let before = snapshot(&root);
        let selected =
            fs::read_to_string(root.join(install::CURRENT_FILE)).expect("baseline CURRENT");
        let selected = install::VersionId::parse(selected.trim_end()).expect("baseline pointer");
        let executable = root
            .join(install::VERSIONS_DIR)
            .join(selected.as_str())
            .join(install::payload_entry(running.target()));
        let scratch = temporary.path().join("private-runtime");
        let mut validated_before_rejection = false;
        let rejected = install::install_payload(
            &executable,
            &running,
            &candidate,
            &archive,
            |staged| {
                let normal = isolated_version(&staged.entry(), false);
                let (normal_exit, normal_spawn_error) = match normal {
                    Ok(output) => (output.status.code(), None),
                    Err(error) => (None, error.raw_os_error()),
                };
                eprintln!(
                    "case={case} root_utf16={} stage=runtime_validation entry_utf16={} normal_exit={normal_exit:?} normal_spawn_error={normal_spawn_error:?}",
                    root.as_os_str().encode_wide().count(),
                    staged.entry().as_os_str().encode_wide().count(),
                );
                validate_staged(staged, &expected, &scratch)?;
                validated_before_rejection = true;
                Err(InstallError::Validator)
            },
        );
        assert_eq!(rejected, Err(InstallError::Validator));
        assert!(
            validated_before_rejection,
            "real staged doctor must pass before injected rejection"
        );
        assert_preserved(&root, &before, false);

        let installed =
            install::install_payload(&executable, &running, &candidate, &archive, |staged| {
                validate_staged(staged, &expected, &scratch)
            })
            .expect("activate validated real package");
        assert_eq!(
            fs::read_to_string(root.join(install::CURRENT_FILE)).expect("new pointer"),
            format!("{}\n", installed.id)
        );
        assert_preserved(&root, &before, true);
        let outer = root.join("bili.exe");
        let version = isolated_version(&outer, true).expect("outer entry version");
        assert!(version.status.success());
        assert!(String::from_utf8(version.stdout)
            .expect("version UTF-8")
            .contains(&candidate.version.to_string()));
        let report = staged_doctor(&outer, &scratch).expect("new outer entry native doctor");
        assert_eq!(
            report
                .pointer("/data/build_identity/version")
                .and_then(|v| v.as_str()),
            Some(candidate.version.to_string().as_str())
        );
        assert!(report
            .pointer("/data/native")
            .is_some_and(|v| v.is_object()));
        let activated = snapshot(&root);
        assert_eq!(
            install::install_payload(&executable, &running, &candidate, &archive, |_| {
                panic!("stale retry must be rejected before validation")
            }),
            Err(InstallError::Downgrade)
        );
        assert_preserved(&root, &activated, false);
        assert_eq!(
            snapshot(&baseline),
            original,
            "original baseline stays immutable"
        );
        eprintln!("case={case} activation=passed native_doctor=passed preserved_files={} stale_retry=passed", before.len());
    }
}
