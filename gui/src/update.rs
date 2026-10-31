//! GUI scheduling and headless validation over the shared core update protocol.
use crate::prefs::UpdateChannel;
use bili_core::install::{self, InstallError, InstallRoot, Staged};
use bili_core::update::{
    Channel, CheckOutcome, CheckPermit, Identity, Product, ReminderStore, Target, UpdateCandidate,
    Updater, CHECK_INTERVAL_SECS, PROTOCOL,
};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc as async_mpsc, watch};

pub const REPO_URL: &str = "https://github.com/SweerItTer/BilibiliAnimeDownloader";
pub const ISSUES_URL: &str = "https://github.com/SweerItTer/BilibiliAnimeDownloader/issues/new";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_DOCTOR_BYTES: u64 = 256 * 1024;
const DOCTOR_BUDGET: Duration = Duration::from_secs(15);

pub fn identity() -> Result<Identity, String> {
    let target = if cfg!(all(
        target_arch = "x86_64",
        target_os = "windows",
        target_env = "msvc"
    )) {
        Target::WindowsX86_64Msvc
    } else if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        Target::LinuxX86_64Gnu
    } else {
        return Err("当前平台不支持更新。".into());
    };
    Identity::new(Product::Gui, target, VERSION).map_err(|_| "当前版本号无效。".into())
}

/// Runs before preferences, authentication, a runtime or a native window exist.
pub fn doctor() -> bili_core::Result<Value> {
    let identity = identity().map_err(|_| {
        bili_core::CoreError::new(bili_core::ErrorCode::InvalidInput, "GUI build identity")
    })?;
    let native = bili_core::remux::diagnostics()?;
    Ok(json!({
        "schema_version": 1, "type": "doctor", "data": {
            "version": VERSION,
            "build_identity": {
                "product": identity.product().as_str(), "target": identity.target().triple(),
                "version": VERSION, "update_protocol": PROTOCOL
            },
            "native": native
        }
    }))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    candidate: UpdateCandidate,
}

impl From<UpdateCandidate> for Release {
    fn from(candidate: UpdateCandidate) -> Self {
        Self {
            tag: candidate.tag.clone(),
            version: candidate.version.to_string(),
            candidate,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    NoCompatibleRelease,
    Available(Release),
    Downloading(Release),
    Ready {
        version: String,
    },
    Error(String),
}

impl Status {
    pub fn is_checking(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading(_))
    }
}

enum Request {
    Check {
        channel: UpdateChannel,
        automatic: bool,
    },
    Install(Box<Release>),
}

struct Outcome {
    status: Status,
    notify: bool,
}

/// The host retains and joins this owner before dropping its one runtime.
pub struct Owner {
    worker: tokio::task::JoinHandle<()>,
    stop: watch::Sender<bool>,
}

impl Owner {
    pub async fn finish(self) {
        self.stop.send_replace(true);
        if self.worker.await.is_err() {
            eprintln!("GUI update worker failed.");
        }
    }
}

#[derive(Default)]
pub struct UpdateState {
    status: Status,
    commands: Option<async_mpsc::Sender<Request>>,
    results: Option<Receiver<Result<Outcome, String>>>,
    notify: bool,
    last_automatic: Option<Instant>,
}

impl UpdateState {
    pub fn connected(runtime: &tokio::runtime::Handle) -> (Self, Owner) {
        let (commands, receiver) = async_mpsc::channel(1);
        let (results_tx, results) = mpsc::channel();
        let (stop, stopped) = watch::channel(false);
        let worker = runtime.spawn(serve(receiver, results_tx, stopped));
        (
            Self {
                commands: Some(commands),
                results: Some(results),
                ..Self::default()
            },
            Owner { worker, stop },
        )
    }

    pub fn status(&self) -> &Status {
        &self.status
    }
    pub fn should_notify(&self) -> bool {
        self.notify
    }

    pub fn automatic_due(&self) -> bool {
        self.automatic_due_at(Instant::now())
    }

    fn automatic_due_at(&self, now: Instant) -> bool {
        self.commands.is_some()
            && matches!(
                self.status,
                Status::Idle
                    | Status::UpToDate
                    | Status::NoCompatibleRelease
                    | Status::Available(_)
                    | Status::Error(_)
            )
            && self.last_automatic.is_none_or(|time| {
                now.saturating_duration_since(time).as_secs() >= CHECK_INTERVAL_SECS
            })
    }

    pub fn start(&mut self, channel: UpdateChannel) {
        self.check(channel, false);
    }
    pub fn automatic(&mut self, channel: UpdateChannel) {
        self.check(channel, true);
    }

    fn check(&mut self, channel: UpdateChannel, automatic: bool) {
        if self.status.is_checking() {
            return;
        }
        self.last_automatic = Some(Instant::now());
        if self.send(Request::Check { channel, automatic }) {
            self.status = Status::Checking;
        }
    }

    pub fn install(&mut self) {
        let Status::Available(release) = self.status.clone() else {
            return;
        };
        if self.send(Request::Install(Box::new(release.clone()))) {
            self.status = Status::Downloading(release);
        }
    }

    fn send(&mut self, request: Request) -> bool {
        if self
            .commands
            .as_ref()
            .is_some_and(|commands| commands.try_send(request).is_ok())
        {
            true
        } else {
            self.status = Status::Error("更新后台不可用，请关闭并重新打开应用。".into());
            false
        }
    }

    pub fn poll(&mut self) -> bool {
        let Some(results) = &self.results else {
            return false;
        };
        match results.try_recv() {
            Ok(Ok(outcome)) => {
                self.status = outcome.status;
                self.notify = outcome.notify;
                true
            }
            Ok(Err(error)) => {
                self.status = Status::Error(error);
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) if self.status.is_checking() => {
                self.status = Status::Error("更新操作中断，请稍后重试。".into());
                true
            }
            Err(TryRecvError::Disconnected) => false,
        }
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    if !*stop.borrow() {
        let _ = stop.changed().await;
    }
}

async fn serve(
    mut commands: async_mpsc::Receiver<Request>,
    results: mpsc::Sender<Result<Outcome, String>>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let request = tokio::select! {
            biased;
            _ = stopped(&mut stop) => break,
            request = commands.recv() => match request { Some(request) => request, None => break },
        };
        let result = match request {
            Request::Check { channel, automatic } => tokio::select! {
                biased;
                _ = stopped(&mut stop) => break,
                result = check(channel, automatic) => match result {
                    Err(_) if automatic => Ok(Outcome { status: Status::Idle, notify: false }),
                    result => result,
                },
            },
            Request::Install(release) => install(&release, &mut stop).await,
        };
        if results.send(result).is_err() || *stop.borrow() {
            break;
        }
    }
}

fn selected_channel(channel: UpdateChannel) -> Channel {
    match channel {
        UpdateChannel::Stable => Channel::Stable,
        UpdateChannel::Beta => Channel::PreRelease,
    }
}

fn reminders(identity: &Identity, channel: Channel) -> Result<ReminderStore, String> {
    let project = directories::ProjectDirs::from("", "", "bili")
        .ok_or_else(|| "无法定位更新设置目录。".to_owned())?;
    Ok(ReminderStore::new(
        project
            .cache_dir()
            .join("update")
            .join("gui")
            .join(identity.target().triple())
            .join(match channel {
                Channel::Stable => "stable",
                Channel::PreRelease => "pre-release",
            }),
    ))
}

async fn check(channel: UpdateChannel, automatic: bool) -> Result<Outcome, String> {
    let identity = identity()?;
    let channel = selected_channel(channel);
    let store = reminders(&identity, channel)?;
    if automatic {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|t| t.as_secs())
            .unwrap_or(0);
        if store
            .begin_check(now, CHECK_INTERVAL_SECS)
            .map_err(|_| "无法读取更新检查记录。")?
            != CheckPermit::Due
        {
            return Ok(Outcome {
                status: Status::Idle,
                notify: false,
            });
        }
    }
    let outcome = Updater::new()
        .map_err(|_| "无法创建更新请求。")?
        .check(&identity, channel)
        .await
        .map_err(|_| "无法获取更新信息，请稍后重试。")?;
    let notify = match &outcome {
        CheckOutcome::Available(candidate) if automatic => store
            .record_notification(channel, &candidate.version)
            .map_err(|_| "无法保存更新通知记录。")?,
        _ => true,
    };
    let status = match outcome {
        CheckOutcome::Available(candidate) => Status::Available(candidate.into()),
        CheckOutcome::UpToDate { .. } => Status::UpToDate,
        CheckOutcome::NoCompatibleRelease { .. } => Status::NoCompatibleRelease,
    };
    Ok(Outcome { status, notify })
}

async fn install(release: &Release, stop: &mut watch::Receiver<bool>) -> Result<Outcome, String> {
    let identity = identity()?;
    let executable = std::env::current_exe().map_err(|_| "无法定位当前安装。")?;
    InstallRoot::discover(&executable).map_err(install_error)?;
    let scratch = tempfile::Builder::new()
        .prefix("bili-gui-update-")
        .tempdir()
        .map_err(|_| "无法创建更新临时目录。")?;
    let archive = scratch
        .path()
        .join(&release.candidate.descriptor.archive.name);
    let updater = Updater::new().map_err(|_| "无法创建更新请求。")?;
    tokio::select! {
        biased;
        _ = stopped(stop) => return Err("更新已取消。".into()),
        result = updater.download(&release.candidate, &archive) => result.map_err(|_| "下载或校验更新失败，请稍后重试。")?,
    };
    let candidate = release.candidate.clone();
    let expected = Identity::new(Product::Gui, identity.target(), &release.version)
        .map_err(|_| "更新版本号无效。")?;
    // Once the transaction starts, shutdown joins it through this worker. Never
    // abort a blocking installer and let it outlive the host runtime.
    let installed = tokio::task::spawn_blocking(move || {
        install::install_payload(&executable, &identity, &candidate, &archive, |staged| {
            validate_staged(staged, &expected, scratch.path())
        })
    })
    .await
    .map_err(|_| "更新安装任务中断。")?
    .map_err(install_error)?;
    Ok(Outcome {
        status: Status::Ready {
            version: installed.version.to_string(),
        },
        notify: true,
    })
}

fn install_error(error: InstallError) -> String {
    match error {
        InstallError::NotPackaged => "当前为开发构建，只能检查更新；请先安装完整 GUI 包。".into(),
        InstallError::Busy => "其他更新正在使用此安装，请稍后重试。".into(),
        InstallError::Validator => "新版本未通过 GUI 身份或运行库检查，旧版本保持不变。".into(),
        InstallError::Downgrade => "安装中已有相同或更新的版本，请重新检查更新。".into(),
        _ => "更新安装或校验失败，旧版本保持不变。".into(),
    }
}

pub fn restart_entry() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|_| "无法定位当前安装。")?;
    let root = InstallRoot::discover(&executable).map_err(install_error)?;
    Ok(root.root().join(if cfg!(windows) {
        "bili.exe"
    } else {
        "bin/bili"
    }))
}

fn validate_staged(
    staged: &Staged<'_>,
    expected: &Identity,
    scratch: &Path,
) -> Result<(), InstallError> {
    if staged.identity() != expected {
        return Err(InstallError::Validator);
    }
    let bytes = staged_doctor(&staged.entry(), scratch)?;
    validate_report(&bytes, expected)
}

fn validate_report(bytes: &[u8], expected: &Identity) -> Result<(), InstallError> {
    if bytes.len() as u64 > MAX_DOCTOR_BYTES {
        return Err(InstallError::Validator);
    }
    let report: Value = serde_json::from_slice(bytes).map_err(|_| InstallError::Validator)?;
    let expected_build = json!({"product": expected.product().as_str(), "target": expected.target().triple(),
        "version": expected.version().to_string(), "update_protocol": PROTOCOL});
    if report.get("schema_version") != Some(&json!(1))
        || report.get("type") != Some(&json!("doctor"))
        || report.pointer("/data/build_identity") != Some(&expected_build)
        || report.pointer("/data/version") != Some(&json!(expected.version().to_string()))
        || !report.pointer("/data/native").is_some_and(Value::is_object)
    {
        return Err(InstallError::Validator);
    }
    Ok(())
}

fn staged_doctor(entry: &Path, scratch: &Path) -> Result<Vec<u8>, InstallError> {
    let home = scratch.join("home");
    let roaming = home.join("AppData/Roaming");
    let local = home.join("AppData/Local");
    for directory in [
        &roaming,
        &local,
        &home.join("config"),
        &home.join("data"),
        &home.join("cache"),
        &scratch.join("tmp"),
    ] {
        std::fs::create_dir_all(directory).map_err(|_| InstallError::Filesystem)?;
    }
    #[cfg(windows)]
    let entry = std::fs::canonicalize(entry).map_err(|_| InstallError::Validator)?;
    let mut command = std::process::Command::new(entry);
    command
        .args(["--json", "doctor"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env_clear()
        .current_dir(scratch)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", &roaming)
        .env("LOCALAPPDATA", &local)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("TEMP", scratch.join("tmp"))
        .env("TMP", scratch.join("tmp"))
        .env("TMPDIR", scratch.join("tmp"));
    #[cfg(windows)]
    {
        let (windows, system) = windows_directories()?;
        let path =
            std::env::join_paths([system, windows.clone()]).map_err(|_| InstallError::Validator)?;
        command
            .env("SystemRoot", &windows)
            .env("WINDIR", &windows)
            .env("PATH", path);
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: validation never opens a console/window.
    }
    let mut child = command.spawn().map_err(|_| InstallError::Validator)?;
    let stdout = child.stdout.take().ok_or(InstallError::Validator)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(MAX_DOCTOR_BYTES + 1).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + DOCTOR_BUDGET;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => break None,
        }
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let bytes = reader.join().map_err(|_| InstallError::Validator)?;
    if !status.is_some_and(|status| status.success()) || bytes.len() as u64 > MAX_DOCTOR_BYTES {
        return Err(InstallError::Validator);
    }
    Ok(bytes)
}

#[cfg(windows)]
fn windows_directories() -> Result<(PathBuf, PathBuf), InstallError> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let read = |get: unsafe extern "system" fn(*mut u16, u32) -> u32| {
        let mut buffer = vec![0u16; 32768];
        // SAFETY: the live writable buffer has exactly the supplied capacity.
        let length = unsafe { get(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if length == 0 || length >= buffer.len() {
            return Err(InstallError::Validator);
        }
        let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
        if !path.is_absolute() {
            return Err(InstallError::Validator);
        }
        Ok(path)
    };
    Ok((
        read(GetSystemWindowsDirectoryW)?,
        read(GetSystemDirectoryW)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_report_requires_gui_candidate_identity_and_complete_envelope() {
        let identity = Identity::new(Product::Gui, Target::WindowsX86_64Msvc, "2.0.0").unwrap();
        let report = json!({"schema_version": 1, "type": "doctor", "data": {
            "version": "2.0.0", "build_identity": {"product": "gui", "target": "x86_64-pc-windows-msvc",
            "version": "2.0.0", "update_protocol": 1}, "native": {"ffmpeg_version": "8.0.3"}}});
        let valid = serde_json::to_vec(&report).unwrap();
        assert_eq!(validate_report(&valid, &identity), Ok(()));
        for pointer in [
            "/schema_version",
            "/type",
            "/data/version",
            "/data/build_identity/product",
            "/data/build_identity/target",
            "/data/build_identity/version",
            "/data/build_identity/update_protocol",
            "/data/native",
        ] {
            let mut wrong = report.clone();
            *wrong.pointer_mut(pointer).unwrap() = Value::Null;
            assert_eq!(
                validate_report(&serde_json::to_vec(&wrong).unwrap(), &identity),
                Err(InstallError::Validator)
            );
        }
        let mut oversized = valid.clone();
        oversized.resize(MAX_DOCTOR_BYTES as usize + 1, b' ');
        assert_eq!(
            validate_report(&oversized, &identity),
            Err(InstallError::Validator)
        );
        assert_eq!(
            validate_report(&valid[..valid.len() - 1], &identity),
            Err(InstallError::Validator)
        );
    }

    #[test]
    fn gui_identity_and_channel_are_shell_owned() {
        assert_eq!(identity().unwrap().product(), Product::Gui);
        assert_eq!(selected_channel(UpdateChannel::Stable), Channel::Stable);
        assert_eq!(selected_channel(UpdateChannel::Beta), Channel::PreRelease);
    }

    #[tokio::test]
    async fn host_joins_the_idle_update_worker() {
        let (mut state, owner) = UpdateState::connected(&tokio::runtime::Handle::current());
        owner.finish().await;
        assert!(!state.poll());
        state.start(UpdateChannel::Stable);
        assert!(matches!(state.status(), Status::Error(_)));
    }

    #[tokio::test]
    async fn available_candidate_is_checked_again_after_the_interval() {
        let candidate: UpdateCandidate = serde_json::from_value(json!({
            "tag": "v2.0.0", "version": "2.0.0", "channel": "stable", "descriptor": {
                "schema_version": 1, "product": "gui", "target": "x86_64-pc-windows-msvc", "version": "2.0.0",
                "archive": {"name": "gui.zip", "format": "zip", "sha256": "0".repeat(64), "size": 1}
            }
        })).unwrap();
        let release = Release::from(candidate);
        let (mut state, owner) = UpdateState::connected(&tokio::runtime::Handle::current());
        let now = Instant::now();
        state.status = Status::Available(release.clone());
        state.last_automatic = Some(now);
        assert!(!state.automatic_due_at(now));
        let due = now + Duration::from_secs(CHECK_INTERVAL_SECS);
        assert!(state.automatic_due_at(due));
        for status in [
            Status::Checking,
            Status::Downloading(release),
            Status::Ready {
                version: "2.0.0".into(),
            },
        ] {
            state.status = status;
            assert!(!state.automatic_due_at(due));
        }
        owner.finish().await;
    }

    /// Native package acceptance. Inputs are locally source-built version A/B
    /// packages, never a mocked report or a published test release.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires BILI_GUI_BASE_INSTALL, BILI_GUI_UPDATE_ARCHIVE and BILI_GUI_UPDATE_DESCRIPTOR"]
    fn native_gui_packages_update_at_multiple_depths_and_preserve_failures() {
        use bili_core::update::Descriptor;
        use std::collections::BTreeMap;
        use std::os::windows::ffi::OsStrExt;

        fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
            fn visit(root: &Path, directory: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
                for entry in std::fs::read_dir(directory).unwrap() {
                    let entry = entry.unwrap();
                    if entry.file_type().unwrap().is_dir() {
                        visit(root, &entry.path(), output);
                    } else {
                        output.insert(
                            entry.path().strip_prefix(root).unwrap().to_path_buf(),
                            std::fs::read(entry.path()).unwrap(),
                        );
                    }
                }
            }
            let mut output = BTreeMap::new();
            visit(root, root, &mut output);
            output
        }

        let source = PathBuf::from(
            std::env::var_os("BILI_GUI_BASE_INSTALL").expect("base GUI installation"),
        );
        let archive =
            PathBuf::from(std::env::var_os("BILI_GUI_UPDATE_ARCHIVE").expect("candidate archive"));
        let descriptor =
            std::env::var_os("BILI_GUI_UPDATE_DESCRIPTOR").expect("candidate descriptor");
        let descriptor: Descriptor =
            serde_json::from_slice(&std::fs::read(descriptor).unwrap()).unwrap();
        let candidate = UpdateCandidate {
            tag: format!("v{}", descriptor.version),
            version: descriptor.version.clone(),
            channel: Channel::of(&descriptor.version),
            descriptor,
        };
        let expected = Identity::new(
            Product::Gui,
            Target::WindowsX86_64Msvc,
            &candidate.version.to_string(),
        )
        .unwrap();
        let running = identity().unwrap();
        let original = snapshot(&source);
        let temporary = tempfile::tempdir().unwrap();
        for depth in [0, 180, 300] {
            let mut root = temporary.path().join(format!("安装 空格-{depth}"));
            while root.as_os_str().encode_wide().count() < depth {
                root = root.join("nested-installation-depth");
            }
            std::fs::create_dir_all(&root).unwrap();
            for (relative, bytes) in &original {
                let path = root.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
            let sentinel = root.join("user-file.txt");
            std::fs::write(&sentinel, b"preserve user data").unwrap();
            let before = snapshot(&root);
            let selected = InstallRoot::discover(&root.join("bili.exe"))
                .unwrap()
                .current()
                .unwrap();
            let executable = root
                .join("versions")
                .join(selected.as_str())
                .join("bili.exe");
            let scratch = temporary.path().join(format!("scratch-{depth}"));
            std::fs::create_dir_all(&scratch).unwrap();
            let baseline = staged_doctor(&root.join("bili.exe"), &scratch).unwrap();
            validate_report(&baseline, &running).unwrap();

            let rejected =
                install::install_payload(&executable, &running, &candidate, &archive, |staged| {
                    // Fault injection touches the inactive candidate only; the real
                    // headless GUI validator must reject the missing runtime DLL.
                    let library = std::fs::read_dir(staged.payload())
                        .unwrap()
                        .map(|e| e.unwrap().path())
                        .find(|p| {
                            p.file_name()
                                .unwrap()
                                .to_string_lossy()
                                .to_ascii_lowercase()
                                .contains("avutil")
                                && p.extension().is_some_and(|e| e == "dll")
                        })
                        .expect("packaged avutil DLL");
                    std::fs::remove_file(library).unwrap();
                    validate_staged(staged, &expected, &scratch)
                });
            assert_eq!(rejected, Err(InstallError::Validator));
            for (relative, bytes) in &before {
                assert_eq!(
                    &std::fs::read(root.join(relative)).unwrap(),
                    bytes,
                    "failure changed {}",
                    relative.display()
                );
            }
            let installed =
                install::install_payload(&executable, &running, &candidate, &archive, |staged| {
                    validate_staged(staged, &expected, &scratch)
                })
                .expect("real GUI transaction");
            assert_eq!(installed.version, candidate.version);
            let report = staged_doctor(&root.join("bili.exe"), &scratch).unwrap();
            validate_report(&report, &expected).unwrap();
            for (relative, bytes) in &before {
                if relative != Path::new("CURRENT") {
                    assert_eq!(
                        &std::fs::read(root.join(relative)).unwrap(),
                        bytes,
                        "update changed {}",
                        relative.display()
                    );
                }
            }
            assert_eq!(
                install::install_payload(&executable, &running, &candidate, &archive, |_| panic!(
                    "repeat candidate must be rejected before validation"
                )),
                Err(InstallError::Downgrade)
            );
            eprintln!(
                "GUI package activation and preservation passed at root length {} UTF-16 units",
                root.as_os_str().encode_wide().count()
            );
        }
    }
}
