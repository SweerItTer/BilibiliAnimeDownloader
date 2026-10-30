//! GUI update flow. Follows the shared protocol: a product descriptor pins the
//! archive name, size and SHA-256, semver precedence picks the candidate, and a
//! staged copy replaces the installation only after this process exits.
use crate::prefs::UpdateChannel;
use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub const REPO_URL: &str = "https://github.com/SweerItTer/BilibiliAnimeDownloader";
pub const ISSUES_URL: &str = "https://github.com/SweerItTer/BilibiliAnimeDownloader/issues/new";
const API: &str = "https://api.github.com/repos/SweerItTer/BilibiliAnimeDownloader";
const MAX_PAGES: usize = 10;
const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
const MAX_DESCRIPTOR: u64 = 64 * 1024;
const MAX_EXTRACTED: u64 = 1024 * 1024 * 1024;
const MARKER_FILE: &str = "staged-update.json";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PRODUCT: &str = "gui";
pub const TARGET: &str = if cfg!(all(
    target_arch = "x86_64",
    target_os = "windows",
    target_env = "msvc"
)) {
    "x86_64-pc-windows-msvc"
} else if cfg!(all(
    target_arch = "x86_64",
    target_os = "linux",
    target_env = "gnu"
)) {
    "x86_64-unknown-linux-gnu"
} else {
    "unsupported"
};

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub archive_name: String,
    pub archive_url: String,
    pub sha256: String,
    pub size: u64,
    pub page_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    NoCompatibleRelease,
    Available(Release),
    Downloading(Release),
    Ready { version: String },
    Error(String),
}

impl Status {
    pub fn is_checking(&self) -> bool {
        matches!(self, Status::Checking | Status::Downloading(_))
    }
}

#[derive(Clone)]
pub struct StagedUpdate {
    pub version: String,
    pub root: PathBuf,
    pub install_root: PathBuf,
}

enum Outcome {
    Checked(Status),
    Staged(StagedUpdate),
}

pub struct UpdateState {
    status: Status,
    pending: Option<Receiver<Result<Outcome, String>>>,
    staged: Option<StagedUpdate>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            pending: None,
            staged: None,
        }
    }
}

impl UpdateState {
    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn staged(&self) -> Option<&StagedUpdate> {
        self.staged.as_ref()
    }

    pub fn start(&mut self, channel: UpdateChannel) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check(channel).map(Outcome::Checked));
        });
        self.pending = Some(rx);
        self.status = Status::Checking;
    }

    pub fn install(&mut self) {
        let Status::Available(release) = self.status.clone() else {
            return;
        };
        if self.pending.is_some() {
            return;
        }
        let worker_release = release.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(stage(&worker_release).map(Outcome::Staged));
        });
        self.pending = Some(rx);
        self.status = Status::Downloading(release);
    }

    /// Returns true once a check, download, or failure was applied.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok(Outcome::Checked(status))) => {
                self.status = status;
                self.pending = None;
                true
            }
            Ok(Ok(Outcome::Staged(staged))) => {
                self.status = Status::Ready {
                    version: staged.version.clone(),
                };
                self.staged = Some(staged);
                self.pending = None;
                true
            }
            Ok(Err(error)) => {
                self.status = Status::Error(error);
                self.pending = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.status = Status::Error("更新操作中断，请稍后重试。".into());
                self.pending = None;
                true
            }
        }
    }
}

struct Asset {
    name: String,
    url: String,
    size: u64,
}

struct Candidate {
    version: Version,
    tag: String,
    page_url: String,
    assets: Vec<Asset>,
}

struct ArchiveRef {
    name: String,
    sha256: String,
    size: u64,
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("Bili-GUI/{VERSION}"))
        .build()
        .map_err(|_| "无法创建更新请求。".to_owned())
}

fn checked(response: reqwest::blocking::Response) -> Result<reqwest::blocking::Response, String> {
    match response.status().as_u16() {
        200..=299 => Ok(response),
        403 | 429 => Err("更新服务暂时限流，请稍后再试。".into()),
        _ => Err("无法获取更新信息，请检查网络。".into()),
    }
}

fn check(channel: UpdateChannel) -> Result<Status, String> {
    let current = Version::parse(VERSION).map_err(|_| "当前版本号无效。".to_owned())?;
    let client = client()?;
    let mut candidates = Vec::new();
    for page in 1..=MAX_PAGES {
        let response = client
            .get(format!("{API}/releases?per_page=100&page={page}"))
            .send()
            .map_err(|_| "无法获取更新信息，请检查网络。".to_owned())?;
        let response = checked(response)?;
        let rows: Value = response
            .json()
            .map_err(|_| "更新服务返回了无法解析的内容。".to_owned())?;
        let rows = rows
            .as_array()
            .ok_or_else(|| "更新服务返回了无法解析的内容。".to_owned())?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            if row.get("draft").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let Some(tag) = row.get("tag_name").and_then(Value::as_str) else {
                continue;
            };
            let Some(version) = canonical_version(tag) else {
                continue;
            };
            let flagged = row
                .get("prerelease")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if flagged != !version.pre.is_empty() {
                continue;
            }
            if channel == UpdateChannel::Stable && !version.pre.is_empty() {
                continue;
            }
            if version.cmp_precedence(&current) != std::cmp::Ordering::Greater {
                continue;
            }
            let page_url = row
                .get("html_url")
                .and_then(Value::as_str)
                .unwrap_or(REPO_URL)
                .to_owned();
            let assets = row
                .get("assets")
                .and_then(Value::as_array)
                .map(|assets| {
                    assets
                        .iter()
                        .filter_map(|asset| {
                            Some(Asset {
                                name: asset.get("name")?.as_str()?.to_owned(),
                                url: asset.get("browser_download_url")?.as_str()?.to_owned(),
                                size: asset.get("size").and_then(Value::as_u64)?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            candidates.push(Candidate {
                version,
                tag: tag.to_owned(),
                page_url,
                assets,
            });
        }
        if rows.len() < 100 {
            break;
        }
        if page == MAX_PAGES {
            return Err("更新列表过长，无法完成检查。".into());
        }
    }

    candidates.sort_by(|a, b| b.version.cmp_precedence(&a.version));
    let mut skipped = false;
    for candidate in candidates {
        let Some(descriptor) = target_descriptor(&candidate.assets, TARGET)? else {
            skipped = true;
            continue;
        };
        let text = text_body(&client, &descriptor.url, MAX_DESCRIPTOR)?;
        let archive = parse_descriptor(&text, &candidate.tag)?;
        let mut matches = candidate
            .assets
            .iter()
            .filter(|asset| asset.name == archive.name);
        let Some(asset) = matches.next() else {
            return Err("发布清单缺少对应的安装包。".into());
        };
        if matches.next().is_some() {
            return Err("发布清单中的安装包不唯一。".into());
        }
        if asset.size != archive.size {
            return Err("安装包大小与发布清单不一致。".into());
        }
        return Ok(Status::Available(Release {
            tag: candidate.tag,
            version: candidate.version.to_string(),
            archive_name: archive.name,
            archive_url: asset.url.clone(),
            sha256: archive.sha256,
            size: archive.size,
            page_url: candidate.page_url,
        }));
    }
    if skipped {
        return Ok(Status::NoCompatibleRelease);
    }
    Ok(Status::UpToDate)
}

fn target_descriptor<'a>(assets: &'a [Asset], target: &str) -> Result<Option<&'a Asset>, String> {
    let name = format!("bili-update-{PRODUCT}-{target}.json");
    let mut matches = assets.iter().filter(|asset| asset.name == name);
    let descriptor = matches.next();
    if matches.next().is_some() {
        return Err("发布清单中的更新描述不唯一。".into());
    }
    Ok(descriptor)
}

fn text_body(client: &reqwest::blocking::Client, url: &str, limit: u64) -> Result<String, String> {
    let response = client
        .get(url)
        .send()
        .map_err(|_| "无法下载更新描述。".to_owned())?;
    let response = checked(response)?;
    if response.content_length().unwrap_or(0) > limit {
        return Err("更新描述超出大小限制。".into());
    }
    let mut text = String::new();
    response
        .take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|_| "无法读取更新描述。".to_owned())?;
    if text.len() as u64 > limit {
        return Err("更新描述超出大小限制。".into());
    }
    Ok(text)
}

fn canonical_version(tag: &str) -> Option<Version> {
    let rest = tag.strip_prefix('v')?;
    let version = Version::parse(rest).ok()?;
    (format!("v{version}") == tag).then_some(version)
}

fn parse_descriptor(text: &str, expected_tag: &str) -> Result<ArchiveRef, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| "更新描述不是有效的 JSON。".to_owned())?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("更新描述版本不受支持。".into());
    }
    if value.get("product").and_then(Value::as_str) != Some(PRODUCT) {
        return Err("更新描述属于其他产品。".into());
    }
    if value.get("target").and_then(Value::as_str) != Some(TARGET) {
        return Err("更新描述属于其他平台。".into());
    }
    let Some(version) = value.get("version").and_then(Value::as_str) else {
        return Err("更新描述缺少版本号。".into());
    };
    let Some(tag_version) = canonical_version(expected_tag) else {
        return Err("发布标签不是规范版本号。".into());
    };
    if version != tag_version.to_string() {
        return Err("更新描述与发布标签不一致。".into());
    }
    let archive = value
        .get("archive")
        .and_then(Value::as_object)
        .ok_or_else(|| "更新描述缺少安装包信息。".to_owned())?;
    let name = archive
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "更新描述缺少安装包名称。".to_owned())?;
    if !safe_archive_name(name) {
        return Err("更新描述的安装包名称不安全。".into());
    }
    let expected_suffix = if TARGET.contains("windows") {
        ".zip"
    } else {
        ".tar.gz"
    };
    if !name.starts_with(&format!("bili-gui-{TARGET}-")) || !name.ends_with(expected_suffix) {
        return Err("更新描述的安装包不是当前平台的 GUI 包。".into());
    }
    let format = archive.get("format").and_then(Value::as_str);
    let expected_format = if TARGET.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    if format != Some(expected_format) {
        return Err("更新描述的安装包格式不受支持。".into());
    }
    let sha256 = archive
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| "更新描述缺少 SHA-256。".to_owned())?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err("更新描述的 SHA-256 无效。".into());
    }
    let size = archive
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| "更新描述缺少安装包大小。".to_owned())?;
    if size == 0 || size > MAX_ARCHIVE {
        return Err("更新描述的安装包大小无效。".into());
    }
    Ok(ArchiveRef {
        name: name.to_owned(),
        sha256: sha256.to_owned(),
        size,
    })
}

fn safe_archive_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.contains("..")
}

fn stage(release: &Release) -> Result<StagedUpdate, String> {
    let updates = updates_dir()?;
    let work = updates.join(&release.version);
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|_| "无法清理旧的更新缓存。".to_owned())?;
    }
    fs::create_dir_all(&work).map_err(|_| "无法创建更新目录。".to_owned())?;
    let archive = work.join(&release.archive_name);
    download(release, &archive)?;
    let extracted = work.join("package");
    extract(&archive, &extracted)?;
    let Some(root) = package_root(&extracted) else {
        return Err("更新包缺少可执行文件。".into());
    };
    let executable = root.join(executable_rel());
    let reported = std::process::Command::new(&executable)
        .arg("--version")
        .output()
        .map_err(|_| "更新包无法运行。".to_owned())?;
    if !reported.status.success() {
        return Err("更新包自检失败。".into());
    }
    let reported = String::from_utf8_lossy(&reported.stdout);
    let Some(version) = reported.trim().strip_prefix("Bili ") else {
        return Err("更新包自检失败。".into());
    };
    if version != release.version {
        return Err("更新包版本与发布清单不一致。".into());
    }
    let install_root = install_root()?;
    let marker = serde_json::json!({
        "version": release.version,
        "root": root,
        "install_root": install_root,
    });
    fs::write(
        work.join(MARKER_FILE),
        serde_json::to_vec_pretty(&marker).map_err(|_| "无法记录待安装更新。".to_owned())?,
    )
    .map_err(|_| "无法记录待安装更新。".to_owned())?;
    Ok(StagedUpdate {
        version: release.version.clone(),
        root,
        install_root,
    })
}

fn download(release: &Release, destination: &Path) -> Result<(), String> {
    let client = client()?;
    let response = client
        .get(&release.archive_url)
        .send()
        .map_err(|_| "无法下载更新包。".to_owned())?;
    let mut response = checked(response)?;
    if let Some(length) = response.content_length() {
        if length != release.size {
            return Err("更新包大小与发布清单不一致。".into());
        }
    }
    let mut file = File::create(destination).map_err(|_| "无法写入更新包。".to_owned())?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|_| "下载更新包时连接中断。".to_owned())?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > release.size {
            return Err("更新包超过发布清单记录的大小。".into());
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .map_err(|_| "无法写入更新包。".to_owned())?;
    }
    if total != release.size {
        return Err("更新包下载不完整。".into());
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if digest != release.sha256 {
        return Err("更新包校验失败，已停止安装。".into());
    }
    Ok(())
}

fn updates_dir() -> Result<PathBuf, String> {
    directories::ProjectDirs::from("", "", "bili")
        .map(|dirs| dirs.data_local_dir().join("updates"))
        .ok_or_else(|| "无法定位更新目录。".to_owned())
}

fn executable_rel() -> &'static str {
    if cfg!(windows) {
        "gui.exe"
    } else {
        "bin/gui"
    }
}

fn package_root(root: &Path) -> Option<PathBuf> {
    if root.join(executable_rel()).is_file() {
        return Some(root.to_path_buf());
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join(executable_rel()).is_file() {
            return Some(path);
        }
    }
    None
}

fn install_root() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|_| "无法定位当前程序。".to_owned())?;
    let directory = executable
        .parent()
        .ok_or_else(|| "无法定位安装目录。".to_owned())?;
    if directory.file_name().is_some_and(|name| name == "bin") {
        directory
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "无法定位安装目录。".to_owned())
    } else {
        Ok(directory.to_path_buf())
    }
}

#[cfg(windows)]
fn extract(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|_| "无法打开更新包。".to_owned())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "更新包格式无效。".to_owned())?;
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|_| "更新包内容损坏。".to_owned())?;
        let Some(relative) = entry.enclosed_name() else {
            return Err("更新包包含不安全的路径。".into());
        };
        let target = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|_| "无法展开更新包。".to_owned())?;
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED {
            return Err("更新包展开后超出大小限制。".into());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| "无法展开更新包。".to_owned())?;
        }
        let mut output = File::create(&target).map_err(|_| "无法展开更新包。".to_owned())?;
        std::io::copy(&mut entry, &mut output).map_err(|_| "无法展开更新包。".to_owned())?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn extract(archive: &Path, destination: &Path) -> Result<(), String> {
    use std::path::Component;
    let file = File::open(archive).map_err(|_| "无法打开更新包。".to_owned())?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar.entries().map_err(|_| "更新包格式无效。".to_owned())?;
    let mut total = 0_u64;
    for entry in entries {
        let mut entry = entry.map_err(|_| "更新包内容损坏。".to_owned())?;
        let path = entry
            .path()
            .map_err(|_| "更新包包含不安全的路径。".to_owned())?;
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("更新包包含不安全的路径。".into());
        }
        let target = destination.join(&path);
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&target).map_err(|_| "无法展开更新包。".to_owned())?;
            continue;
        }
        if entry.header().entry_type().is_symlink() {
            let link = entry
                .link_name()
                .map_err(|_| "更新包包含不安全的链接。".to_owned())?
                .ok_or_else(|| "更新包包含无效的链接。".to_owned())?;
            if link.is_absolute()
                || link.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                return Err("更新包包含不安全的链接。".into());
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|_| "无法展开更新包。".to_owned())?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &target)
                .map_err(|_| "无法展开更新包。".to_owned())?;
            continue;
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED {
            return Err("更新包展开后超出大小限制。".into());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| "无法展开更新包。".to_owned())?;
        }
        let mut output = File::create(&target).map_err(|_| "无法展开更新包。".to_owned())?;
        std::io::copy(&mut entry, &mut output).map_err(|_| "无法展开更新包。".to_owned())?;
    }
    Ok(())
}

/// Applies a staged update left by a previous session before the window opens.
/// Returns true when a helper was started and this process should exit.
pub fn apply_pending_update() -> bool {
    let Ok(updates) = updates_dir() else {
        return false;
    };
    let Ok(entries) = fs::read_dir(&updates) else {
        return false;
    };
    let current = Version::parse(VERSION).ok();
    for entry in entries.flatten() {
        let marker_path = entry.path().join(MARKER_FILE);
        let Ok(bytes) = fs::read(&marker_path) else {
            continue;
        };
        let Ok(marker) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let (Some(version), Some(root), Some(install_root)) = (
            marker.get("version").and_then(Value::as_str),
            marker.get("root").and_then(Value::as_str),
            marker.get("install_root").and_then(Value::as_str),
        ) else {
            continue;
        };
        let Some(version) = Version::parse(version).ok() else {
            continue;
        };
        if current.as_ref().is_some_and(|current| version <= *current) {
            continue;
        }
        let executable = Path::new(root).join(executable_rel());
        if !executable.is_file() {
            continue;
        }
        let spawned = std::process::Command::new(&executable)
            .arg("--apply-update")
            .arg(install_root)
            .arg(std::process::id().to_string())
            .spawn();
        return spawned.is_ok();
    }
    false
}

/// Spawns the staged build, which replaces the installation after this process exits.
pub fn spawn_helper(staged: &StagedUpdate) -> Result<(), String> {
    let executable = staged.root.join(executable_rel());
    std::process::Command::new(&executable)
        .arg("--apply-update")
        .arg(&staged.install_root)
        .arg(std::process::id().to_string())
        .spawn()
        .map_err(|_| "无法启动更新程序。".to_owned())?;
    Ok(())
}

pub struct HelperRequest {
    pub install_root: PathBuf,
    pub parent_pid: u32,
}

pub fn helper_request() -> Option<HelperRequest> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != "--apply-update" {
        return None;
    }
    let install_root = PathBuf::from(args.next()?);
    let parent_pid = args.next()?.to_string_lossy().parse().ok()?;
    Some(HelperRequest {
        install_root,
        parent_pid,
    })
}

/// Runs inside the staged build after the old process exits.
pub fn run_helper(request: HelperRequest) -> i32 {
    wait_for_exit(request.parent_pid);
    let Some(staged_root) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(|dir| {
            if cfg!(windows) {
                Some(dir)
            } else {
                dir.parent().map(Path::to_path_buf)
            }
        })
    else {
        return 1;
    };
    if copy_tree(&staged_root, &request.install_root).is_err() {
        return 1;
    }
    if let Some(staging) = staged_root.parent() {
        let _ = fs::remove_file(staging.join(MARKER_FILE));
    }
    let executable = request.install_root.join(executable_rel());
    let _ = std::process::Command::new(executable).spawn();
    0
}

fn wait_for_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && process_alive(pid) {
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const SYNCHRONIZE: u32 = 0x0010_0000;
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let state = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        state != WAIT_OBJECT_0
    }
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            {
                let link = fs::read_link(entry.path())?;
                if link.is_relative()
                    && !link.components().any(|component| {
                        matches!(
                            component,
                            std::path::Component::ParentDir
                                | std::path::Component::RootDir
                                | std::path::Component::Prefix(_)
                        )
                    })
                {
                    let _ = fs::remove_file(&target);
                    std::os::unix::fs::symlink(link, &target)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gui_archive_name(target: &str) -> String {
        format!(
            "bili-gui-{target}-release.{}",
            if target.contains("windows") {
                "zip"
            } else {
                "tar.gz"
            }
        )
    }

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            url: String::new(),
            size: 1,
        }
    }

    fn descriptor(version: &str, target: &str, name: &str, size: u64) -> String {
        format!(
            r#"{{"schema_version":1,"product":"gui","target":"{target}","version":"{version}",
            "archive":{{"name":"{name}","format":"{}","sha256":"{}","size":{size}}}}}"#,
            if target.contains("windows") {
                "zip"
            } else {
                "tar.gz"
            },
            "a".repeat(64)
        )
    }

    #[test]
    fn canonical_tags_reject_prefixes_and_build_metadata() {
        assert_eq!(
            canonical_version("v2.0.0-alpha.6"),
            Some(Version::parse("2.0.0-alpha.6").unwrap())
        );
        assert_eq!(canonical_version("2.0.0"), None);
        assert_eq!(canonical_version("v02.0.0"), None);
        let build = canonical_version("v2.0.0+build").unwrap();
        assert_eq!(
            build.cmp_precedence(&Version::parse("2.0.0").unwrap()),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn descriptor_pins_product_target_and_archive() {
        let name = gui_archive_name(TARGET);
        let text = descriptor("2.0.0-alpha.7", TARGET, &name, 1024);
        let archive = parse_descriptor(&text, "v2.0.0-alpha.7").expect("valid descriptor");
        assert_eq!(archive.size, 1024);

        let wrong_product = text.replace(r#""product":"gui""#, r#""product":"cli""#);
        assert!(parse_descriptor(&wrong_product, "v2.0.0-alpha.7").is_err());
        let wrong_tag = parse_descriptor(&text, "v2.0.0-alpha.8");
        assert!(wrong_tag.is_err());
        let unsafe_name = text.replace(&name, "../gui.zip");
        assert!(parse_descriptor(&unsafe_name, "v2.0.0-alpha.7").is_err());
        let cli_package = text.replace(
            &name,
            &format!(
                "bili-{TARGET}-release.{}",
                if TARGET.contains("windows") {
                    "zip"
                } else {
                    "tar.gz"
                }
            ),
        );
        assert!(parse_descriptor(&cli_package, "v2.0.0-alpha.7").is_err());
    }

    #[test]
    fn releases_without_this_platforms_gui_package_are_skipped() {
        let linux_cli = [
            asset("bili-x86_64-unknown-linux-gnu-release.tar.gz"),
            asset("bili-native-source-x86_64-unknown-linux-gnu.tar.gz"),
            asset("bili-update-cli-x86_64-unknown-linux-gnu.json"),
            asset("SHA256SUMS"),
        ];
        assert!(target_descriptor(&linux_cli, "x86_64-pc-windows-msvc")
            .unwrap()
            .is_none());
        assert!(target_descriptor(&linux_cli, "x86_64-unknown-linux-gnu")
            .unwrap()
            .is_none());

        let windows_gui = [
            asset("bili-gui-x86_64-pc-windows-msvc-release.zip"),
            asset("bili-update-gui-x86_64-pc-windows-msvc.json"),
            asset("SHA256SUMS"),
        ];
        assert!(target_descriptor(&windows_gui, "x86_64-pc-windows-msvc")
            .unwrap()
            .is_some());
        assert!(target_descriptor(&windows_gui, "x86_64-unknown-linux-gnu")
            .unwrap()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn tar_extraction_keeps_relative_symlinks() {
        let base = std::env::temp_dir().join(format!("bili-update-extract-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let archive = base.join("package.tar.gz");
        {
            let encoder = flate2::write::GzEncoder::new(
                File::create(&archive).unwrap(),
                flate2::Compression::default(),
            );
            let mut builder = tar::Builder::new(encoder);
            let mut file = tar::Header::new_gnu();
            file.set_size(3);
            file.set_mode(0o644);
            file.set_cksum();
            builder
                .append_data(&mut file, "pkg/lib/libav.so.62.0", &b"abc"[..])
                .unwrap();
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            link.set_link_name("libav.so.62.0").unwrap();
            link.set_cksum();
            builder
                .append_data(&mut link, "pkg/lib/libav.so.62", std::io::empty())
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let extracted = base.join("out");
        extract(&archive, &extracted).unwrap();
        let link = extracted.join("pkg/lib/libav.so.62");
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("libav.so.62.0"));
        assert_eq!(fs::read(link).unwrap(), b"abc");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn archive_names_stay_flat_and_short() {
        assert!(safe_archive_name(
            "bili-gui-x86_64-pc-windows-msvc-release.zip"
        ));
        assert!(!safe_archive_name("../gui.zip"));
        assert!(!safe_archive_name("nested/gui.zip"));
        assert!(!safe_archive_name(".hidden.zip"));
        assert!(!safe_archive_name(&"a".repeat(129)));
    }
}
