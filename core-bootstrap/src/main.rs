#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

//! Installation entry that starts the payload named by `CURRENT`.
//!
//! This binary is the stable, dependency-free door of a packaged installation.
//! It reads exactly one local file (`<root>/CURRENT`), validates that the value
//! is a single payload directory name, and hands control to the payload inside
//! `<root>/versions/<id>/`. It never parses JSON, never compares versions, never
//! scans for a fallback, never touches the network and never replaces itself;
//! everything else belongs to the installer in `bili_core::install`.
//!
//! Linux replaces this process image with the payload, so the pid, exit code and
//! standard streams are the payload's own. Windows starts the payload as a child
//! with inherited standard streams and returns the child's exit code unchanged.
//!
//! The Windows build must be static over the C runtime so this executable has no
//! local DLL dependency: it is the only file the installer may replace at the
//! installation root, and a missing runtime DLL there would break every start.

#[cfg(all(windows, feature = "packaged", not(target_feature = "crt-static")))]
compile_error!(
    "core-bootstrap must be built with `-C target-feature=+crt-static` on Windows; \
     a dynamically linked C runtime would leave the installation entry depending \
     on DLLs beside itself"
);

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Marker file that names a packaged installation.
const MARKER: &str = ".bili-install.json";
/// Pointer file that names the payload to start.
const CURRENT: &str = "CURRENT";
/// Directory holding one payload per version.
const VERSIONS: &str = "versions";

/// Payload entry relative to its version directory.
#[cfg(windows)]
const PAYLOAD: &str = "bili.exe";
#[cfg(not(windows))]
const PAYLOAD: &str = "bin/bili";

fn main() {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(_) => refuse("cannot locate the installation entry"),
    };
    let payload = match start(&executable) {
        Ok(payload) => payload,
        Err(message) => refuse(message),
    };
    launch(&payload, &arguments);
}

/// Resolve the payload this installation must start.
fn start(executable: &Path) -> Result<PathBuf, &'static str> {
    let directory = executable
        .parent()
        .ok_or("installation entry has no directory")?;
    // Linux ships the entry as `<root>/bin/bili`; Windows ships `<root>/bili.exe`.
    let root = if cfg!(windows) {
        directory.to_path_buf()
    } else {
        directory
            .parent()
            .ok_or("installation entry is not inside an installation")?
            .to_path_buf()
    };
    if !is_regular_file(&root.join(MARKER)) {
        return Err("not a packaged installation");
    }
    let current = fs::read(root.join(CURRENT)).map_err(|_| "installation pointer is missing")?;
    let text = std::str::from_utf8(&current).map_err(|_| "installation pointer is not text")?;
    let id = parse_pointer(text)?;
    let payload = resolve(&root, id)?;
    Ok(payload)
}

/// Validate the pointer text: one payload id, with at most one trailing newline.
fn parse_pointer(text: &str) -> Result<&str, &'static str> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    if !valid_id(text) {
        return Err("installation pointer is invalid");
    }
    Ok(text)
}

/// A payload id is `v-` followed by exactly 32 lowercase hex digits.
fn valid_id(text: &str) -> bool {
    match text.strip_prefix("v-") {
        Some(hex) => {
            hex.len() == 32
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        None => false,
    }
}

/// Payload entry of an id that already passed [`valid_id`].
fn resolve(root: &Path, id: &str) -> Result<PathBuf, &'static str> {
    let payload = root.join(VERSIONS).join(id).join(PAYLOAD);
    if !is_regular_file(&payload) {
        return Err("installation pointer names a payload that is not installed");
    }
    Ok(payload)
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn refuse(message: &str) -> ! {
    eprintln!("bili: {message}");
    std::process::exit(1);
}

#[cfg(unix)]
fn launch(payload: &Path, arguments: &[OsString]) -> ! {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(payload);
    if let Some(program) = arguments.first() {
        command.arg0(program);
    }
    command.args(arguments.iter().skip(1));
    // `exec` only returns on failure, and keeps this pid, streams and signals.
    let error = command.exec();
    eprintln!("bili: cannot start the installed payload: {error}");
    std::process::exit(1);
}

#[cfg(windows)]
fn launch(payload: &Path, arguments: &[OsString]) -> ! {
    // Retain Windows' verbatim path spelling when starting a deeply installed
    // payload. Filesystem lookup accepts longer paths than process creation
    // does with the ordinary drive-letter spelling.
    let status = fs::canonicalize(payload).and_then(|payload| {
        Command::new(payload)
            .args(arguments.iter().skip(1))
            .status()
    });
    match status {
        Ok(status) => std::process::exit(forwarded(status)),
        Err(error) => {
            eprintln!("bili: cannot start the installed payload: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(windows)]
fn forwarded(status: std::process::ExitStatus) -> i32 {
    // Windows reports the payload's raw 32-bit code; forward it unchanged so
    // loader failures such as 0xC0000135 stay visible to the caller.
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const ID: &str = "v-0123456789abcdef0123456789abcdef";

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "core-bootstrap-{}-{label}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn pointer_grammar_accepts_one_newline_only() {
        for accepted in [format!("{ID}\n"), format!("{ID}\r\n"), ID.to_owned()] {
            assert_eq!(parse_pointer(&accepted).expect("accepted"), ID);
        }
        for rejected in [
            format!(" {ID}\n"),
            format!("{ID}\n\n"),
            format!("{ID} "),
            ID.to_uppercase(),
            "v-0123".to_owned(),
            format!("versions/{ID}"),
            String::new(),
        ] {
            assert!(parse_pointer(&rejected).is_err(), "{rejected:?}");
        }
    }

    #[test]
    fn payload_must_be_an_installed_regular_file() {
        let root = scratch("resolve");
        let entry = root.join(VERSIONS).join(ID).join(PAYLOAD);
        fs::create_dir_all(entry.parent().expect("payload directory")).expect("payload directory");
        assert!(resolve(&root, ID).is_err(), "missing payload is refused");
        let mut file = fs::File::create(&entry).expect("payload");
        file.write_all(b"payload").expect("payload bytes");
        drop(file);
        assert_eq!(resolve(&root, ID).expect("payload"), entry);

        #[cfg(unix)]
        {
            let other = "v-ffffffffffffffffffffffffffffffff";
            let link = root.join(VERSIONS).join(other).join(PAYLOAD);
            fs::create_dir_all(link.parent().expect("payload directory"))
                .expect("payload directory");
            std::os::unix::fs::symlink(&entry, &link).expect("symlink payload");
            assert!(resolve(&root, other).is_err(), "a symlink is not a payload");
        }
    }

    #[test]
    fn start_requires_a_marker_in_the_installation_root() {
        let root = scratch("start");
        let entry = root.join(PAYLOAD);
        fs::create_dir_all(entry.parent().expect("entry parent")).expect("entry directory");
        fs::write(&entry, b"bootstrap").expect("entry");
        assert_eq!(start(&entry).err(), Some("not a packaged installation"));

        fs::write(root.join(MARKER), b"{}").expect("marker");
        assert_eq!(start(&entry).err(), Some("installation pointer is missing"));
        fs::write(root.join(CURRENT), format!("{ID}\n")).expect("pointer");
        assert_eq!(
            start(&entry).err(),
            Some("installation pointer names a payload that is not installed")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_launch_starts_a_deep_unicode_payload_and_forwards_its_exit() {
        let root = scratch("deep-launch");
        let mut directory = root.join("installed space-目录");
        while directory.as_os_str().to_string_lossy().len() < 320 {
            directory.push("nested-0123456789");
        }
        fs::create_dir_all(&directory).expect("deep payload directory");
        let payload = directory.join("payload.exe");
        fs::copy(std::env::current_exe().expect("test executable"), &payload)
            .expect("copy native payload");
        let result = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "tests::windows_launch_probe"])
            .env("BILI_TEST_BOOTSTRAP_PAYLOAD", &payload)
            .output()
            .expect("launch isolated bootstrap probe");
        assert_eq!(result.status.code(), Some(23), "{result:?}");
        fs::remove_dir_all(root).expect("remove owned test tree");
    }

    #[cfg(windows)]
    #[test]
    fn windows_launch_probe() {
        let Some(payload) = std::env::var_os("BILI_TEST_BOOTSTRAP_PAYLOAD") else {
            return;
        };
        launch(
            Path::new(&payload),
            &[
                OsString::from("bili.exe"),
                OsString::from("--exact"),
                OsString::from("tests::windows_payload_probe"),
            ],
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_payload_probe() {
        if std::env::var_os("BILI_TEST_BOOTSTRAP_PAYLOAD").is_some() {
            std::process::exit(23);
        }
    }
}
