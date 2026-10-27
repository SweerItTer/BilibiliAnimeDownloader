//! Private, atomic, credential-free state and artifact ownership.
use crate::domain::*;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) const SCHEMA: u32 = 1;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResourceRecord {
    pub name: String,
    pub identity: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Completion {
    pub sha256: String,
    pub size: u64,
    pub fingerprint: String,
    pub promoted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LocalMerge {
    pub video: PathBuf,
    pub audio: Option<PathBuf>,
    pub output: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JobRecord {
    pub schema_version: u32,
    pub checkpoint_revision: u64,
    pub components_pending: bool,
    pub snapshot: JobSnapshot,
    pub resources: Vec<ResourceRecord>,
    pub completion: Option<Completion>,
    pub local: Option<LocalMerge>,
}
impl JobRecord {
    pub fn planned(id: JobId, plan: StreamPlan) -> Self {
        Self {
            schema_version: SCHEMA,
            checkpoint_revision: 0,
            components_pending: true,
            snapshot: JobSnapshot {
                id,
                revision: 0,
                plan: Some(std::sync::Arc::new(plan)),
                phase: JobPhase::Queued,
                downloaded: 0,
                total: None,
                output: None,
                skipped: false,
                error: None,
                components: Vec::new(),
            },
            resources: Vec::new(),
            completion: None,
            local: None,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct IndexedJob {
    id: JobId,
    revision: u64,
}
#[derive(Serialize, Deserialize)]
struct Index {
    schema_version: u32,
    revision: u64,
    jobs: Vec<IndexedJob>,
}

/// Lifetime of this object is lifetime of the OS lock, not a PID marker.
pub(crate) struct Journal {
    root: PathBuf,
    lock: Option<File>,
    readonly: bool,
}
impl Journal {
    pub fn open(root: &Path, mode: StateMode) -> Result<Self> {
        if !root.is_absolute() {
            return Err(CoreError::new(
                ErrorCode::InvalidInput,
                "state directory must be absolute",
            ));
        }
        let readonly = mode == StateMode::ReadOnly;
        if !readonly {
            private_dir(root)?;
        }
        if root.try_exists().map_err(CoreError::io)? {
            ensure_real_dir(root)?;
        }
        let lock = if readonly {
            None
        } else {
            let path = root.join("writer.lock");
            reject_symlink(&path)?;
            let file = private_options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(CoreError::io)?;
            match file.try_lock_exclusive() {
                Ok(()) => Some(file),
                Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                    return Err(CoreError::new(
                        ErrorCode::Busy,
                        "state directory already has a writer",
                    ))
                }
                Err(e) => return Err(CoreError::io(e)),
            }
        };
        let journal = Self {
            root: root.to_owned(),
            lock,
            readonly,
        };
        if !readonly {
            private_dir(&journal.root.join("jobs"))?;
        }
        Ok(journal)
    }
    pub fn job_dir(&self, id: JobId) -> Result<PathBuf> {
        let path = self.root.join("jobs").join(id.to_string());
        if !self.readonly {
            private_dir(&path)?;
        } else {
            ensure_real_dir(&path)?;
        }
        Ok(path)
    }
    pub fn load(&self) -> Result<(u64, Vec<JobRecord>)> {
        let index_path = self.root.join("queue.json");
        if !index_path.try_exists().map_err(CoreError::io)? {
            return Ok((0, Vec::new()));
        }
        // Atomic index swaps are safe for readers; retry a concurrent transition.
        'capture: for _ in 0..4 {
            let index: Index = read_json(&index_path)?;
            if index.schema_version != SCHEMA {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    "unsupported queue schema",
                ));
            }
            if index.jobs.len() > 1024
                || index
                    .jobs
                    .iter()
                    .map(|j| j.id)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != index.jobs.len()
            {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    "invalid queue history size or duplicate job",
                ));
            }
            let mut jobs = Vec::with_capacity(index.jobs.len());
            for entry in &index.jobs {
                let path = self
                    .root
                    .join("jobs")
                    .join(entry.id.to_string())
                    .join(format!("job-{}.json", entry.revision));
                let record: JobRecord = match read_json(&path) {
                    Ok(value) => value,
                    Err(error) => {
                        if !path.try_exists().map_err(CoreError::io)? {
                            continue 'capture;
                        }
                        return Err(error);
                    }
                };
                if record.schema_version != SCHEMA
                    || record.snapshot.id != entry.id
                    || record.checkpoint_revision != entry.revision
                {
                    return Err(CoreError::new(
                        ErrorCode::Corruption,
                        "invalid job checkpoint identity",
                    ));
                }
                jobs.push(record);
            }
            let after: Index = read_json(&index_path)?;
            if after.revision == index.revision
                && jobs.iter().all(|j| j.snapshot.revision <= index.revision)
            {
                return Ok((index.revision, jobs));
            }
        }
        Err(CoreError::new(
            ErrorCode::Busy,
            "queue changed during inspection",
        ))
    }
    pub fn save(
        &self,
        revision: u64,
        jobs: &mut [JobRecord],
        dirty: Option<&std::collections::HashSet<JobId>>,
    ) -> Result<()> {
        if self.readonly {
            return Err(CoreError::new(
                ErrorCode::ReadOnly,
                "read-only state inspection",
            ));
        }
        let mut obsolete = Vec::new();
        for record in jobs.iter_mut() {
            if dirty.is_some_and(|ids| !ids.contains(&record.snapshot.id)) {
                continue;
            }
            if record.checkpoint_revision >= revision {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    "non-monotonic job checkpoint revision",
                ));
            }
            let directory = self.job_dir(record.snapshot.id)?;
            if record.checkpoint_revision != 0 {
                obsolete.push(directory.join(format!("job-{}.json", record.checkpoint_revision)));
            }
            record.checkpoint_revision = revision;
            atomic_json(&directory.join(format!("job-{revision}.json")), record)?;
        }
        // Index references immutable per-job revisions. A failed index replace
        // leaves every previously acknowledged checkpoint intact and readable.
        atomic_json(
            &self.root.join("queue.json"),
            &Index {
                schema_version: SCHEMA,
                revision,
                jobs: jobs
                    .iter()
                    .map(|j| IndexedJob {
                        id: j.snapshot.id,
                        revision: j.checkpoint_revision,
                    })
                    .collect(),
            },
        )?;
        for path in obsolete {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }
    pub fn owned_file(&self, id: JobId, name: &str) -> Result<PathBuf> {
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                "invalid managed artifact name",
            ));
        }
        let path = self.job_dir(id)?.join(name);
        reject_symlink(&path)?;
        Ok(path)
    }
    pub fn clean(&self, record: &JobRecord, request: &CleanupRequest) -> CleanupReport {
        let mut report = CleanupReport::default();
        let phase = record.snapshot.phase;
        if phase.is_active() || (!request.include_resumable && phase != JobPhase::Completed) {
            return report;
        }
        let dir = match self.job_dir(record.snapshot.id) {
            Ok(d) => d,
            Err(e) => {
                report.errors.push(e);
                return report;
            }
        };
        // Only exact artifacts generated by this job; never recurse or touch outputs.
        let mut names: Vec<String> = record
            .resources
            .iter()
            .flat_map(|r| [r.name.clone(), format!("{}.checkpoint.json", r.name)])
            .collect();
        names.extend(["remux.partial.mp4".into(), "concat.ffconcat".into()]);
        for name in names {
            let path = match self.owned_file(record.snapshot.id, &name) {
                Ok(p) => p,
                Err(e) => {
                    report.errors.push(e);
                    continue;
                }
            };
            match fs::symlink_metadata(&path) {
                Ok(meta)
                    if meta.is_file()
                        && !meta.file_type().is_symlink()
                        && path.parent() == Some(dir.as_path()) =>
                {
                    report.bytes += meta.len();
                    report.paths.push(path.clone());
                    if !request.dry_run {
                        if let Err(e) = fs::remove_file(path) {
                            report.errors.push(CoreError::io(e));
                        }
                    }
                }
                Ok(_) => report.errors.push(CoreError::new(
                    ErrorCode::Conflict,
                    "refusing non-file managed artifact",
                )),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => report.errors.push(CoreError::io(e)),
            }
        }
        match clean_staging(record, request.dry_run) {
            Ok((paths, bytes)) => {
                report.paths.extend(paths);
                report.bytes += bytes;
            }
            Err(error) => report.errors.push(error),
        }
        report
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        if let Some(lock) = &self.lock {
            let _ = FileExt::unlock(lock);
        }
    }
}

pub(crate) fn private_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = options;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        options
    };
    options
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    output_dir(path)?;
    crate::cookie::restrict_private(path, true)
}
/// Create output directories without changing permissions of user-owned parents.
pub(crate) fn output_dir(path: &Path) -> Result<()> {
    let mut existing = path;
    while !existing.try_exists().map_err(CoreError::io)? {
        existing = existing
            .parent()
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "directory ancestor missing"))?;
    }
    ensure_real_dir(existing)?;
    fs::create_dir_all(path).map_err(CoreError::io)?;
    ensure_real_dir(path)
}
pub(crate) fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if is_link(&m) => Err(CoreError::new(
            ErrorCode::Conflict,
            "symlink artifact rejected",
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::io(e)),
    }
}
fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
fn ensure_real_dir(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let m = fs::symlink_metadata(&current).map_err(CoreError::io)?;
        if !m.is_dir() || is_link(&m) {
            return Err(CoreError::new(
                ErrorCode::Conflict,
                "directory symlink or non-directory rejected",
            ));
        }
    }
    Ok(())
}
pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    reject_symlink(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "checkpoint parent missing"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(CoreError::io)?;
    crate::cookie::restrict_private(tmp.path(), false)?;
    serde_json::to_writer(&mut tmp, value)
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "checkpoint encoding"))?;
    tmp.flush().map_err(CoreError::io)?;
    tmp.as_file().sync_all().map_err(CoreError::io)?;
    tmp.persist(path).map_err(|e| CoreError::io(e.error))?;
    sync_directory(parent)
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    reject_symlink(path)?;
    let file = File::open(path).map_err(CoreError::io)?;
    if file.metadata().map_err(CoreError::io)?.len() > 16 * 1024 * 1024 {
        return Err(CoreError::new(
            ErrorCode::Corruption,
            "checkpoint too large",
        ));
    }
    serde_json::from_reader(file)
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "checkpoint decoding"))
}
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(CoreError::io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
pub(crate) fn file_digest_cancel(
    path: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(u64, String)> {
    let metadata = fs::symlink_metadata(path).map_err(CoreError::io)?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(CoreError::new(
            ErrorCode::Conflict,
            "non-file verification artifact rejected",
        ));
    }
    let mut file = private_options()
        .read(true)
        .open(path)
        .map_err(CoreError::io)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(CoreError::new(
                ErrorCode::Cancelled,
                "file verification cancelled",
            ));
        }
        let count = file.read(&mut buffer).map_err(CoreError::io)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok((bytes, format!("{:x}", hasher.finalize())))
}
pub(crate) fn verify_completion_cancel(
    record: &JobRecord,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    let Some(completion) = &record.completion else {
        return Ok(false);
    };
    let output = record
        .snapshot
        .plan
        .as_ref()
        .map(|p| &p.destination)
        .or_else(|| record.local.as_ref().map(|l| &l.output));
    let Some(output) = output else {
        return Ok(false);
    };
    if !output.try_exists().map_err(CoreError::io)? {
        return Ok(false);
    }
    if let Some(plan) = &record.snapshot.plan {
        let identity = crate::api::plan_fingerprint(plan)?;
        if identity != plan.fingerprint {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                "stored media plan identity mismatch",
            ));
        }
        if completion.fingerprint != identity {
            return Ok(false);
        }
    }
    if !completion.promoted {
        let staging = staging_path(record)?;
        if !staging.try_exists().map_err(CoreError::io)?
            || !same_file::is_same_file(&staging, output).map_err(CoreError::io)?
        {
            return Ok(false);
        }
    }
    let (size, digest) = file_digest_cancel(output, cancel)?;
    Ok(size == completion.size && digest == completion.sha256)
}
/// Hard-link promotion is atomic and fails if the destination already exists.
/// Source and destination are required to share a filesystem.
pub(crate) fn promote(source: &Path, destination: &Path) -> Result<()> {
    reject_symlink(source)?;
    reject_symlink(destination)?;
    OpenOptions::new()
        .read(true)
        .write(cfg!(windows))
        .open(source)
        .and_then(|f| f.sync_all())
        .map_err(CoreError::io)?;
    fs::hard_link(source, destination).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            CoreError::new(ErrorCode::Conflict, "output already exists")
        } else {
            CoreError::io(e)
        }
    })?;
    sync_directory(
        destination
            .parent()
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "output parent missing"))?,
    )?;
    // Keep the hard-link ownership proof until the promoted manifest is durable.
    Ok(())
}

pub(crate) fn staging_path(record: &JobRecord) -> Result<PathBuf> {
    let destination = record
        .snapshot
        .plan
        .as_ref()
        .map(|p| &p.destination)
        .or_else(|| record.local.as_ref().map(|l| &l.output))
        .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "job has no output path"))?;
    let parent = destination
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "output parent missing"))?;
    Ok(parent
        .join(format!(".bili-{}", record.snapshot.id))
        .join("remux.partial.mp4"))
}
pub(crate) fn prepare_staging(record: &JobRecord) -> Result<PathBuf> {
    let staging = staging_path(record)?;
    let directory = staging
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "staging parent missing"))?;
    output_dir(
        directory
            .parent()
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "output parent missing"))?,
    )?;
    if directory.try_exists().map_err(CoreError::io)? {
        ensure_real_dir(directory)?;
        let owner: JobId = read_json(&directory.join("owner.json"))?;
        if owner != record.snapshot.id {
            return Err(CoreError::new(
                ErrorCode::Conflict,
                "staging directory ownership mismatch",
            ));
        }
    } else {
        fs::create_dir(directory).map_err(CoreError::io)?;
        private_dir(directory)?;
        atomic_json(&directory.join("owner.json"), &record.snapshot.id)?;
    }
    reject_symlink(&staging)?;
    Ok(staging)
}
pub(crate) fn clean_staging(record: &JobRecord, dry_run: bool) -> Result<(Vec<PathBuf>, u64)> {
    let staging = staging_path(record)?;
    let directory = staging
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "staging parent missing"))?
        .to_owned();
    if !directory.try_exists().map_err(CoreError::io)? {
        return Ok((Vec::new(), 0));
    }
    ensure_real_dir(&directory)?;
    let marker = directory.join("owner.json");
    let owner: JobId = read_json(&marker)?;
    if owner != record.snapshot.id {
        return Err(CoreError::new(
            ErrorCode::Conflict,
            "staging directory ownership mismatch",
        ));
    }
    let mut paths = Vec::new();
    let mut bytes = 0;
    for path in [staging, marker] {
        reject_symlink(&path)?;
        if let Ok(metadata) = fs::metadata(&path) {
            if !metadata.is_file() {
                return Err(CoreError::new(
                    ErrorCode::Conflict,
                    "non-file staging artifact",
                ));
            }
            bytes += metadata.len();
            paths.push(path.clone());
            if !dry_run {
                fs::remove_file(path).map_err(CoreError::io)?;
            }
        }
    }
    if !dry_run {
        fs::remove_dir(directory).map_err(CoreError::io)?;
    }
    Ok((paths, bytes))
}

#[derive(Serialize, Deserialize)]
struct ComponentArtifact {
    schema_version: u32,
    destination: PathBuf,
    identity: String,
    size: u64,
    sha256: String,
    staging: String,
    promoted: bool,
}

fn component_manifest(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "component filename missing"))?;
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "component parent missing"))?;
    Ok(parent.join(format!(
        ".bili-sidecar-{:x}.json",
        Sha256::digest(name.as_encoded_bytes())
    )))
}
fn component_guard(path: &Path) -> Result<(PathBuf, File)> {
    if !path.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidInput,
            "component path must be absolute",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "component parent missing"))?;
    output_dir(parent)?;
    reject_symlink(path)?;
    let manifest = component_manifest(path)?;
    let lock_path = manifest.with_extension("lock");
    reject_symlink(&lock_path)?;
    let lock = private_options()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(CoreError::io)?;
    crate::cookie::restrict_private(&lock_path, false)?;
    FileExt::try_lock_exclusive(&lock).map_err(|error| {
        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            CoreError::new(ErrorCode::Busy, "component writer is active")
        } else {
            CoreError::io(error)
        }
    })?;
    Ok((manifest, lock))
}
fn load_component(
    path: &Path,
    manifest: &Path,
    identity: Option<&str>,
) -> Result<Option<ComponentArtifact>> {
    if !manifest.try_exists().map_err(CoreError::io)? {
        return Ok(None);
    }
    let artifact: ComponentArtifact = read_json(manifest)?;
    let valid_stage = artifact
        .staging
        .strip_prefix(".bili-sidecar-")
        .and_then(|name| name.strip_suffix(".partial"))
        .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok());
    if artifact.schema_version != SCHEMA
        || artifact.destination.as_os_str()
            != path.file_name().ok_or_else(|| {
                CoreError::new(ErrorCode::InvalidInput, "component filename missing")
            })?
        || !valid_stage
        || artifact.sha256.len() != 64
        || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CoreError::new(
            ErrorCode::Corruption,
            "component ownership manifest is invalid",
        ));
    }
    if identity.is_some_and(|identity| artifact.identity != identity) {
        return Err(CoreError::new(
            ErrorCode::Conflict,
            "component identity differs",
        ));
    }
    Ok(Some(artifact))
}
fn finish_component(
    path: &Path,
    manifest: &Path,
    artifact: &mut ComponentArtifact,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "component parent missing"))?;
    let staging = parent.join(&artifact.staging);
    reject_symlink(&staging)?;
    reject_symlink(path)?;
    let output_exists = path.try_exists().map_err(CoreError::io)?;
    if !output_exists && !staging.try_exists().map_err(CoreError::io)? {
        return Ok(false);
    }
    if output_exists
        && !artifact.promoted
        && (!staging.try_exists().map_err(CoreError::io)?
            || !same_file::is_same_file(&staging, path).map_err(CoreError::io)?)
    {
        return Err(CoreError::new(
            ErrorCode::Conflict,
            "component lacks owned promotion proof",
        ));
    }
    let (size, sha256) = file_digest_cancel(if output_exists { path } else { &staging }, cancel)?;
    if size != artifact.size || sha256 != artifact.sha256 {
        return Err(CoreError::new(
            ErrorCode::Corruption,
            "owned component content changed",
        ));
    }
    if !output_exists {
        if cancel.is_cancelled() {
            return Err(CoreError::new(
                ErrorCode::Cancelled,
                "component publication cancelled",
            ));
        }
        promote(&staging, path)?;
    }
    if !artifact.promoted || !output_exists {
        artifact.promoted = true;
        atomic_json(manifest, artifact)?;
    }
    if staging.try_exists().map_err(CoreError::io)? {
        if !same_file::is_same_file(&staging, path).map_err(CoreError::io)? {
            return Err(CoreError::new(
                ErrorCode::Conflict,
                "component staging ownership mismatch",
            ));
        }
        fs::remove_file(staging).map_err(CoreError::io)?;
        sync_directory(parent)?;
    }
    Ok(true)
}

/// Verify/recover a prior successful component without trusting its filename.
pub(crate) fn component_exists(
    path: &Path,
    identity: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    if cancel.is_cancelled() {
        return Err(CoreError::new(
            ErrorCode::Cancelled,
            "component verification cancelled",
        ));
    }
    let (manifest, _guard) = component_guard(path)?;
    match load_component(path, &manifest, Some(identity))? {
        Some(mut artifact) => finish_component(path, &manifest, &mut artifact, cancel),
        None if path.try_exists().map_err(CoreError::io)? => Err(CoreError::new(
            ErrorCode::Conflict,
            "unowned component output exists",
        )),
        None => Ok(false),
    }
}

/// Check a path already acknowledged in the durable component result list.
pub(crate) fn verify_component(
    path: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool> {
    if cancel.is_cancelled() {
        return Err(CoreError::new(
            ErrorCode::Cancelled,
            "component verification cancelled",
        ));
    }
    let (manifest, _guard) = component_guard(path)?;
    match load_component(path, &manifest, None)? {
        Some(mut artifact) => finish_component(path, &manifest, &mut artifact, cancel),
        None if path.try_exists().map_err(CoreError::io)? => Err(CoreError::new(
            ErrorCode::Conflict,
            "unowned component output exists",
        )),
        None => Ok(false),
    }
}

/// Persist ownership before no-clobber publication; retry reuses verified bytes.
pub(crate) fn write_component(
    path: &Path,
    bytes: &[u8],
    identity: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(CoreError::new(
            ErrorCode::Cancelled,
            "component write cancelled",
        ));
    }
    let (manifest, _guard) = component_guard(path)?;
    if let Some(mut artifact) = load_component(path, &manifest, Some(identity))? {
        if finish_component(path, &manifest, &mut artifact, cancel)? {
            return Ok(());
        }
    } else if path.try_exists().map_err(CoreError::io)? {
        return Err(CoreError::new(
            ErrorCode::Conflict,
            "unowned component output exists",
        ));
    }
    let staging_name = format!(".bili-sidecar-{}.partial", uuid::Uuid::new_v4());
    let staging = path
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "component parent missing"))?
        .join(&staging_name);
    let mut file = private_options()
        .write(true)
        .create_new(true)
        .open(&staging)
        .map_err(CoreError::io)?;
    let prepared = (|| {
        crate::cookie::restrict_private(&staging, false)?;
        let mut hash = Sha256::new();
        for chunk in bytes.chunks(1024 * 1024) {
            if cancel.is_cancelled() {
                return Err(CoreError::new(
                    ErrorCode::Cancelled,
                    "component write cancelled",
                ));
            }
            file.write_all(chunk).map_err(CoreError::io)?;
            hash.update(chunk);
        }
        file.sync_all().map_err(CoreError::io)?;
        drop(file);
        let mut artifact = ComponentArtifact {
            schema_version: SCHEMA,
            destination: PathBuf::from(path.file_name().ok_or_else(|| {
                CoreError::new(ErrorCode::InvalidInput, "component filename missing")
            })?),
            identity: identity.into(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", hash.finalize()),
            staging: staging_name,
            promoted: false,
        };
        atomic_json(&manifest, &artifact)?;
        finish_component(path, &manifest, &mut artifact, cancel).map(|_| ())
    })();
    // A committed manifest owns its stage even if promotion was interrupted.
    if prepared.is_err() {
        let stage_is_owned = match read_json::<ComponentArtifact>(&manifest) {
            Ok(artifact) => staging.file_name() == Some(std::ffi::OsStr::new(&artifact.staging)),
            Err(_) => manifest.try_exists().map_err(CoreError::io)?,
        };
        if !stage_is_owned {
            let _ = fs::remove_file(staging);
        }
    }
    prepared
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local_record(root: &Path) -> JobRecord {
        JobRecord {
            schema_version: 1,
            checkpoint_revision: 0,
            components_pending: false,
            snapshot: JobSnapshot {
                id: JobId::new(),
                revision: 1,
                plan: None,
                phase: JobPhase::Committing,
                downloaded: 0,
                total: None,
                output: None,
                skipped: false,
                error: None,
                components: Vec::new(),
            },
            resources: Vec::new(),
            completion: None,
            local: Some(LocalMerge {
                video: root.join("original"),
                audio: None,
                output: root.join("final.mp4"),
            }),
        }
    }
    #[test]
    fn promotion_recovery_requires_owned_hard_link_not_merely_matching_bytes() {
        let root = tempfile::tempdir().unwrap();
        let mut job = local_record(root.path());
        let cancel = tokio_util::sync::CancellationToken::new();
        let source = prepare_staging(&job).unwrap();
        fs::write(&source, b"verified-media").unwrap();
        let (size, sha256) = file_digest_cancel(&source, &cancel).unwrap();
        job.completion = Some(Completion {
            size,
            sha256,
            fingerprint: format!("manual:{}", job.snapshot.id),
            promoted: false,
        });
        let output = &job.local.as_ref().unwrap().output;
        fs::write(output, b"verified-media").unwrap();
        assert!(!verify_completion_cancel(&job, &cancel).unwrap());
        assert_eq!(
            promote(&source, output).unwrap_err().code,
            ErrorCode::Conflict
        );
        assert_eq!(fs::read(&source).unwrap(), b"verified-media");
        fs::remove_file(output).unwrap();
        promote(&source, output).unwrap();
        assert!(source.exists());
        assert!(verify_completion_cancel(&job, &cancel).unwrap());
        job.completion.as_mut().unwrap().promoted = true;
        clean_staging(&job, false).unwrap();
        assert!(verify_completion_cancel(&job, &cancel).unwrap());
        fs::write(output, b"same-length-BAD").unwrap();
        assert!(!verify_completion_cancel(&job, &cancel).unwrap());
    }
    #[test]
    fn cleanup_is_exact_owned_and_preserves_resumable_and_unmanaged_data() {
        let root = tempfile::tempdir().unwrap();
        let journal = Journal::open(&root.path().join("state"), StateMode::ReadWrite).unwrap();
        let mut job = local_record(root.path());
        job.snapshot.phase = JobPhase::Paused;
        job.resources.push(ResourceRecord {
            name: "resource-0.media".into(),
            identity: "selected-stream".into(),
        });
        let owned = journal
            .owned_file(job.snapshot.id, "resource-0.media")
            .unwrap();
        fs::write(&owned, b"partial").unwrap();
        let unrelated = owned.parent().unwrap().join("user-notes");
        fs::write(&unrelated, b"leave").unwrap();
        let original = &job.local.as_ref().unwrap().video;
        fs::write(original, b"source").unwrap();
        let mut request = CleanupRequest {
            job: Some(job.snapshot.id),
            abandoned: false,
            include_resumable: false,
            dry_run: false,
        };
        assert!(journal.clean(&job, &request).paths.is_empty());
        assert!(owned.exists());
        request.include_resumable = true;
        request.dry_run = true;
        let dry = journal.clean(&job, &request);
        assert!(dry.paths.contains(&owned));
        assert!(owned.exists());
        request.dry_run = false;
        let removed = journal.clean(&job, &request);
        assert!(removed.errors.is_empty());
        assert!(!owned.exists());
        assert_eq!(fs::read(unrelated).unwrap(), b"leave");
        assert_eq!(fs::read(original).unwrap(), b"source");
    }
}

#[cfg(test)]
mod revision_recovery_tests {
    use super::*;
    #[test]
    fn half_written_new_record_cannot_replace_acknowledged_index_capture() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state");
        let journal = Journal::open(&path, StateMode::ReadWrite).unwrap();
        let id = JobId::new();
        let mut jobs = vec![JobRecord {
            schema_version: 1,
            checkpoint_revision: 0,
            components_pending: false,
            snapshot: JobSnapshot {
                id,
                revision: 1,
                plan: None,
                phase: JobPhase::Paused,
                downloaded: 0,
                total: None,
                output: None,
                skipped: false,
                error: None,
                components: Vec::new(),
            },
            resources: Vec::new(),
            completion: None,
            local: Some(LocalMerge {
                video: root.path().join("original"),
                audio: None,
                output: root.path().join("final"),
            }),
        }];
        journal.save(1, &mut jobs, None).unwrap();
        let mut interrupted = jobs[0].clone();
        interrupted.checkpoint_revision = 2;
        interrupted.snapshot.revision = 2;
        interrupted.snapshot.phase = JobPhase::Queued;
        // Exact crash window: new record durable; atomic index replacement has
        // not happened. Old mutable job.json protocol lost its old version here.
        atomic_json(
            &journal.job_dir(id).unwrap().join("job-2.json"),
            &interrupted,
        )
        .unwrap();
        let reader = Journal::open(&path, StateMode::ReadOnly).unwrap();
        let (revision, capture) = reader.load().unwrap();
        assert_eq!(revision, 1);
        assert_eq!(capture[0].snapshot.phase, JobPhase::Paused);
        drop(reader);
        drop(journal);
        let recovered = Journal::open(&path, StateMode::ReadWrite).unwrap();
        let (revision, mut capture) = recovered.load().unwrap();
        assert_eq!(revision, 1);
        assert_eq!(capture[0].snapshot.phase, JobPhase::Paused);
        capture[0].snapshot.revision = 2;
        recovered.save(2, &mut capture, None).unwrap();
        assert_eq!(
            recovered.load().unwrap().1[0].snapshot.phase,
            JobPhase::Paused
        );
    }
}

#[cfg(test)]
mod component_tests {
    use super::*;
    use tokio_util::sync::CancellationToken;
    #[test]
    fn component_writer_contention_is_busy_and_other_file_errors_remain_filesystem() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("en.srt");
        let (manifest, guard) = component_guard(&path).unwrap();
        assert_eq!(component_guard(&path).unwrap_err().code, ErrorCode::Busy);
        drop(guard);
        let (_, replacement) = component_guard(&path).unwrap();
        drop(replacement);
        let lock_path = manifest.with_extension("lock");
        fs::remove_file(&lock_path).unwrap();
        fs::create_dir(&lock_path).unwrap();
        assert_eq!(
            component_guard(&path).unwrap_err().code,
            ErrorCode::Filesystem
        );
    }
    #[test]
    fn retry_reuses_verified_owned_component_but_not_unrelated_or_modified_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("en.srt");
        let cancel = CancellationToken::new();
        write_component(&path, b"original snapshot", "part:1/track:2/srt", &cancel).unwrap();
        write_component(&path, b"new live snapshot", "part:1/track:2/srt", &cancel).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"original snapshot");
        assert!(component_exists(&path, "part:1/track:2/srt", &cancel).unwrap());
        assert!(verify_component(&path, &cancel).unwrap());
        assert_eq!(
            write_component(&path, b"other track", "part:1/track:3/srt", &cancel)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        fs::write(&path, b"modified output").unwrap();
        assert_eq!(
            component_exists(&path, "part:1/track:2/srt", &cancel)
                .unwrap_err()
                .code,
            ErrorCode::Corruption
        );
        assert_eq!(
            verify_component(&path, &cancel).unwrap_err().code,
            ErrorCode::Corruption
        );
        let unrelated = root.path().join("unrelated.ass");
        fs::write(&unrelated, b"equal bytes").unwrap();
        assert_eq!(
            write_component(&unrelated, b"equal bytes", "part:1/ass", &cancel)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(fs::read(unrelated).unwrap(), b"equal bytes");
    }
    #[test]
    fn interrupted_component_promotion_requires_surviving_owned_hard_link_proof() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pool.xml");
        let cancel = CancellationToken::new();
        let stage_name = format!(".bili-sidecar-{}.partial", uuid::Uuid::new_v4());
        let stage = root.path().join(&stage_name);
        fs::write(&stage, b"complete pool").unwrap();
        let (size, sha256) = file_digest_cancel(&stage, &cancel).unwrap();
        let manifest = component_manifest(&path).unwrap();
        let artifact = ComponentArtifact {
            schema_version: SCHEMA,
            destination: PathBuf::from("pool.xml"),
            identity: "part:1/xml".into(),
            size,
            sha256,
            staging: stage_name,
            promoted: false,
        };
        atomic_json(&manifest, &artifact).unwrap();
        fs::write(&path, b"complete pool").unwrap();
        assert_eq!(
            component_exists(&path, "part:1/xml", &cancel)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        fs::remove_file(&path).unwrap();
        fs::hard_link(&stage, &path).unwrap();
        assert!(component_exists(&path, "part:1/xml", &cancel).unwrap());
        assert!(!stage.exists());
        assert!(read_json::<ComponentArtifact>(&manifest).unwrap().promoted);
    }
    #[test]
    fn cancelled_component_never_claims_unrelated_output() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("subtitle.srt");
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            write_component(&path, b"content", "part:1/srt", &cancel)
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert!(!path.exists());
        assert!(!component_manifest(&path).unwrap().exists());
    }
}
