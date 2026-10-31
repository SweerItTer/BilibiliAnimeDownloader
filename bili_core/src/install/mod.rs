//! Installation transactions for packaged builds.
//!
//! An installation is a small, fixed layout:
//!
//! ```text
//! <root>/.bili-install.json   marker written once, never rewritten
//! <root>/CURRENT              single payload id, replaced atomically
//! <root>/versions/<id>/       one self-contained payload per directory
//! <root>/.update/<uuid>/      private staging for one transaction
//! ```
//!
//! The running payload is never modified, renamed or deleted: a transaction
//! extracts and validates a complete payload beside the running one, and only
//! then replaces `CURRENT`, which is the single pointer a shell reads. Any
//! failure before that replacement leaves both the previous pointer and the
//! previous payload untouched. This module owns extraction, hashes, metadata,
//! locking and the pointer write; it never spawns a process, so a shell decides
//! how the staged payload is exercised.

mod archive;
#[cfg(test)]
mod tests;

use crate::update::{Identity, Product, Target, UpdateCandidate};
use fs2::FileExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Marker file naming an installation.
pub const MARKER_FILE: &str = ".bili-install.json";
/// Pointer file naming the payload an installation runs.
pub const CURRENT_FILE: &str = "CURRENT";
/// Directory holding one complete payload per version.
pub const VERSIONS_DIR: &str = "versions";
/// Directory holding one transaction's private staging.
pub const UPDATE_DIR: &str = ".update";

/// Installation marker schema this build writes and accepts.
pub const SCHEMA_VERSION: u32 = 1;
/// Installation layout this build understands.
pub const LAYOUT_VERSION: u32 = 1;
/// Startup-entry protocol compiled into the shipped bootstrap.
pub const BOOTSTRAP_PROTOCOL: u32 = 1;
/// Update protocol compiled into this build.
pub const UPDATE_PROTOCOL: u32 = 1;

/// Largest archive this module accepts, matching the release descriptor bound.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// Largest total size an archive may expand to.
pub(crate) const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Deepest install root discovery tries above the running executable.
const MAX_DISCOVERY_DEPTH: usize = 5;

/// Installation transaction failure. Every variant is local and static.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstallError {
    /// The running executable is not inside a packaged installation.
    NotPackaged,
    /// The installation marker is missing, unreadable or malformed.
    MarkerInvalid,
    /// Installation metadata disagrees with this shell or with the archive.
    MarkerMismatch,
    /// `CURRENT` is missing or is not a single valid payload id.
    CurrentInvalid,
    /// Another installation transaction holds the lock.
    Busy,
    /// The installation directory cannot be written by this user.
    NotWritable,
    /// A local filesystem operation failed.
    Filesystem,
    /// The archive does not match its advertised size or digest.
    ArchiveDigest,
    /// The archive is implausible for its descriptor.
    ArchiveSize,
    /// The archive layout is unsafe or does not match the installation format.
    ArchiveLayout,
    /// A payload inventory does not match the tree it describes.
    InventoryMismatch,
    /// Payload metadata disagrees with the expected product, target or version.
    PayloadIdentity,
    /// The candidate is not newer than the running or currently selected version.
    Downgrade,
    /// The archive needs a newer layout or entry protocol than this build has.
    ProtocolIncompatible,
    /// The shell's staged-payload validation rejected the payload.
    Validator,
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::NotPackaged => "executable is not inside a packaged installation",
            Self::MarkerInvalid => "installation marker is missing or malformed",
            Self::MarkerMismatch => "installation belongs to another product, target or layout",
            Self::CurrentInvalid => "installation pointer is missing or invalid",
            Self::Busy => "another update is using this installation",
            Self::NotWritable => "installation directory is not writable by this user",
            Self::Filesystem => "installation filesystem failure",
            Self::ArchiveDigest => "update archive does not match its published digest",
            Self::ArchiveSize => "update archive size is implausible or disagrees",
            Self::ArchiveLayout => "update archive layout is unsafe or unexpected",
            Self::InventoryMismatch => "update archive contents do not match their inventory",
            Self::PayloadIdentity => "update payload identity disagrees with this installation",
            Self::Downgrade => "update candidate is not newer than the installed version",
            Self::ProtocolIncompatible => "update needs a newer installation or entry protocol",
            Self::Validator => "staged payload failed validation",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for InstallError {}

impl From<io::Error> for InstallError {
    fn from(_: io::Error) -> Self {
        Self::Filesystem
    }
}

/// Installation marker (`<root>/.bili-install.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub schema_version: u32,
    pub layout_version: u32,
    pub bootstrap_protocol: u32,
    pub product: Product,
    pub target: Target,
}

impl Marker {
    fn load(path: &Path) -> Result<Self, InstallError> {
        let bytes = fs::read(path).map_err(|_| InstallError::MarkerInvalid)?;
        let marker: Self =
            serde_json::from_slice(&bytes).map_err(|_| InstallError::MarkerInvalid)?;
        marker.validate()?;
        Ok(marker)
    }

    fn validate(&self) -> Result<(), InstallError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(InstallError::MarkerInvalid);
        }
        if self.layout_version != LAYOUT_VERSION || self.bootstrap_protocol != BOOTSTRAP_PROTOCOL {
            return Err(InstallError::ProtocolIncompatible);
        }
        Ok(())
    }
}

/// Payload directory name: `v-` followed by exactly 32 lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionId(String);

impl VersionId {
    /// Id a package build derives from its payload digest.
    pub fn from_app_digest(digest: &str) -> Option<Self> {
        let digest = digest.get(..32)?;
        if digest.len() != 32 || !is_lower_hex(digest) {
            return None;
        }
        Self::parse(&format!("v-{digest}")).ok()
    }

    /// Fresh id for a payload whose derived directory already exists.
    pub fn random() -> Self {
        Self(format!("v-{}", uuid::Uuid::new_v4().simple()))
    }

    /// Parse the only id spelling this layout accepts.
    pub fn parse(raw: &str) -> Result<Self, InstallError> {
        let hex = raw.strip_prefix("v-").ok_or(InstallError::CurrentInvalid)?;
        if hex.len() != 32 || !is_lower_hex(hex) {
            return Err(InstallError::CurrentInvalid);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VersionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Payload entry a target ships, relative to its version directory.
pub fn payload_entry(target: Target) -> &'static str {
    match target {
        Target::LinuxX86_64Gnu => "bin/bili",
        Target::WindowsX86_64Msvc => "bili.exe",
    }
}

/// A packaged installation discovered from a running executable.
#[derive(Debug, Clone)]
pub struct InstallRoot {
    root: PathBuf,
    marker: Marker,
}

impl InstallRoot {
    /// Find the installation containing `current_exe`; nothing else is consulted.
    pub fn discover(current_exe: &Path) -> Result<Self, InstallError> {
        for ancestor in current_exe.ancestors().skip(1).take(MAX_DISCOVERY_DEPTH) {
            let marker_path = ancestor.join(MARKER_FILE);
            match fs::symlink_metadata(&marker_path) {
                Ok(metadata) if metadata.is_file() => {
                    let marker = Marker::load(&marker_path)?;
                    return Ok(Self {
                        root: ancestor.to_path_buf(),
                        marker,
                    });
                }
                Ok(_) => return Err(InstallError::MarkerInvalid),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err(InstallError::Filesystem),
            }
        }
        Err(InstallError::NotPackaged)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn marker(&self) -> &Marker {
        &self.marker
    }

    fn versions(&self) -> PathBuf {
        self.root.join(VERSIONS_DIR)
    }

    fn current_path(&self) -> PathBuf {
        self.root.join(CURRENT_FILE)
    }

    /// The payload id this installation currently starts.
    pub fn current(&self) -> Result<VersionId, InstallError> {
        read_current(&self.current_path())
    }

    fn check_identity(&self, identity: &Identity) -> Result<(), InstallError> {
        let marker = &self.marker;
        if marker.product != identity.product()
            || marker.target != identity.target()
            || marker.layout_version != LAYOUT_VERSION
        {
            return Err(InstallError::MarkerMismatch);
        }
        match fs::symlink_metadata(self.versions()) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(InstallError::NotPackaged),
            Err(_) => Err(InstallError::NotPackaged),
        }
    }
}

/// Staged payload handed to a shell's validator before the pointer moves.
#[derive(Debug)]
pub struct Staged<'a> {
    payload: PathBuf,
    id: &'a VersionId,
    target: Target,
    identity: &'a Identity,
}

impl Staged<'_> {
    /// Version directory of the staged payload.
    pub fn payload(&self) -> &Path {
        &self.payload
    }

    /// The staged payload's CLI executable.
    pub fn entry(&self) -> PathBuf {
        self.payload.join(payload_entry(self.target))
    }

    pub fn id(&self) -> &VersionId {
        self.id
    }

    /// Product, target and version the archive claims to carry.
    pub fn identity(&self) -> &Identity {
        self.identity
    }
}

/// Successful installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub id: VersionId,
    pub version: Version,
}

/// Download-free installation transaction for one already verified archive.
///
/// The archive is expanded inside the installation, every payload identity is
/// checked, the caller's `validate` runs against the staged payload, and only
/// then is `CURRENT` replaced in a single rename. `validate` is the shell's own
/// runtime check; a shell is free to exercise the payload however it likes.
pub fn install_payload<F>(
    current_exe: &Path,
    identity: &Identity,
    candidate: &UpdateCandidate,
    archive: &Path,
    validate: F,
) -> Result<Installed, InstallError>
where
    F: FnOnce(&Staged<'_>) -> Result<(), InstallError>,
{
    let root = InstallRoot::discover(current_exe)?;
    root.check_identity(identity)?;
    let reference = &candidate.descriptor.archive;
    if candidate.descriptor.product != identity.product()
        || candidate.descriptor.target != identity.target()
    {
        return Err(InstallError::MarkerMismatch);
    }
    if candidate.descriptor.version != candidate.version {
        return Err(InstallError::PayloadIdentity);
    }
    if reference.format != identity.target().archive_kind() {
        return Err(InstallError::ArchiveLayout);
    }
    if !candidate.version.cmp_precedence(identity.version()).is_gt() {
        return Err(InstallError::Downgrade);
    }
    let (size, digest) = hash_file(archive)?;
    if size == 0 || size > MAX_ARCHIVE_BYTES || size != reference.size {
        return Err(InstallError::ArchiveSize);
    }
    if digest != reference.sha256 {
        return Err(InstallError::ArchiveDigest);
    }

    let lock = InstallLock::acquire(&root)?;
    // Another updater may have committed while this process downloaded the
    // archive. The running executable is immutable and can now be stale.
    let current_id = root.current()?;
    let current_info = read_build_info(&root.versions().join(current_id.as_str()), identity)?;
    let current_version =
        Version::parse(&current_info.app_version).map_err(|_| InstallError::PayloadIdentity)?;
    if !candidate.version.cmp_precedence(&current_version).is_gt() {
        return Err(InstallError::Downgrade);
    }
    let work = begin_work(&root)?;
    let outcome = expand(&root, identity, candidate, archive, &work, validate);
    drop(lock);
    let _ = fs::remove_dir_all(&work);
    outcome
}

fn expand<F>(
    root: &InstallRoot,
    identity: &Identity,
    candidate: &UpdateCandidate,
    archive: &Path,
    work: &Path,
    validate: F,
) -> Result<Installed, InstallError>
where
    F: FnOnce(&Staged<'_>) -> Result<(), InstallError>,
{
    let reference = &candidate.descriptor.archive;
    let extract = work.join("extract");
    let top = archive::extract(archive, reference.format, &extract, identity.target())?;
    archive::check_links(&top)?;
    archive::verify_inventory(&top, &top.join("SHA256SUMS"))?;

    let archive_marker = Marker::load(&top.join(MARKER_FILE))?;
    if archive_marker != root.marker {
        return Err(InstallError::MarkerMismatch);
    }
    let archive_id = read_current(&top.join(CURRENT_FILE))?;
    let versions = top.join(VERSIONS_DIR);
    let mut names = fs::read_dir(&versions).map_err(|_| InstallError::ArchiveLayout)?;
    let only = names
        .next()
        .transpose()
        .map_err(|_| InstallError::Filesystem)?
        .ok_or(InstallError::ArchiveLayout)?;
    if names.next().is_some() || only.file_name() != archive_id.as_str() {
        return Err(InstallError::ArchiveLayout);
    }
    let payload = versions.join(archive_id.as_str());
    archive::verify_inventory(&payload, &payload.join("SHA256SUMS"))?;
    archive::check_links(&payload)?;

    let info = read_build_info(&payload, identity)?;
    if info.app_version != candidate.version.to_string() {
        return Err(InstallError::PayloadIdentity);
    }
    let entry = payload.join(payload_entry(identity.target()));
    let entry_meta = fs::symlink_metadata(&entry).map_err(|_| InstallError::PayloadIdentity)?;
    if !entry_meta.is_file() {
        return Err(InstallError::PayloadIdentity);
    }
    let (_, entry_digest) = hash_file(&entry)?;
    if entry_digest != info.app_sha256 {
        return Err(InstallError::PayloadIdentity);
    }
    // The packaging contract derives a payload's directory from its own digest.
    if VersionId::from_app_digest(&entry_digest).as_ref() != Some(&archive_id) {
        return Err(InstallError::PayloadIdentity);
    }

    let candidate_identity =
        Identity::new(identity.product(), identity.target(), &info.app_version)
            .map_err(|_| InstallError::PayloadIdentity)?;
    let staged = Staged {
        payload: payload.clone(),
        id: &archive_id,
        target: identity.target(),
        identity: &candidate_identity,
    };
    validate(&staged)?;

    let destination = match root.versions().join(archive_id.as_str()) {
        path if path.exists() => root.versions().join(VersionId::random().as_str()),
        path => path,
    };
    fs::rename(&payload, &destination).map_err(|_| InstallError::Filesystem)?;
    let id = destination
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| VersionId::parse(name).ok())
        .ok_or(InstallError::Filesystem)?;
    if let Err(error) = commit_current(root, &id) {
        let _ = fs::remove_dir_all(&destination);
        return Err(error);
    }
    Ok(Installed {
        id,
        version: candidate.version.clone(),
    })
}

#[derive(Deserialize)]
struct PayloadBuildInfo {
    schema_version: u32,
    target: String,
    app_version: String,
    app_sha256: String,
    product: Product,
    update_protocol: u32,
    layout_version: u32,
    bootstrap_protocol: u32,
}

fn read_build_info(payload: &Path, identity: &Identity) -> Result<PayloadBuildInfo, InstallError> {
    let bytes =
        fs::read(payload.join("BUILD_INFO.json")).map_err(|_| InstallError::PayloadIdentity)?;
    let info: PayloadBuildInfo =
        serde_json::from_slice(&bytes).map_err(|_| InstallError::PayloadIdentity)?;
    if info.schema_version != SCHEMA_VERSION
        || info.layout_version != LAYOUT_VERSION
        || info.bootstrap_protocol != BOOTSTRAP_PROTOCOL
        || info.update_protocol != UPDATE_PROTOCOL
    {
        return Err(InstallError::ProtocolIncompatible);
    }
    if info.product != identity.product() || info.target != identity.target().triple() {
        return Err(InstallError::PayloadIdentity);
    }
    if !is_lower_hex(&info.app_sha256) || info.app_sha256.len() != 64 {
        return Err(InstallError::PayloadIdentity);
    }
    Ok(info)
}

/// Read a pointer file: one payload id, with at most one trailing newline.
fn read_current(path: &Path) -> Result<VersionId, InstallError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| InstallError::CurrentInvalid)?;
    if !metadata.is_file() || metadata.len() > 128 {
        return Err(InstallError::CurrentInvalid);
    }
    let text = fs::read_to_string(path).map_err(|_| InstallError::CurrentInvalid)?;
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    VersionId::parse(text)
}

/// Replace `CURRENT` in one rename; the pointer never disappears.
///
/// The rename is the activation commit point: once it succeeds the new payload
/// is installed, so nothing after it may report a rollback. Everything that can
/// fail (validation, payload promotion, the rename itself) happens before it.
fn commit_current(root: &InstallRoot, id: &VersionId) -> Result<(), InstallError> {
    let temporary = root
        .root
        .join(format!(".CURRENT-{}", uuid::Uuid::new_v4().simple()));
    let written = (|| -> io::Result<()> {
        let mut file = File::create(&temporary)?;
        file.write_all(id.as_str().as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, root.current_path())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(InstallError::Filesystem);
    }
    // Best-effort durability only; a failure here cannot undo activation, so it
    // is never reported as a failed or rolled-back installation.
    sync_dir(&root.root);
    Ok(())
}

fn begin_work(root: &InstallRoot) -> Result<PathBuf, InstallError> {
    let work = root
        .root
        .join(UPDATE_DIR)
        .join(uuid::Uuid::new_v4().simple().to_string());
    fs::create_dir_all(&work).map_err(|error| match error.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            InstallError::NotWritable
        }
        _ => InstallError::Filesystem,
    })?;
    Ok(work)
}

struct InstallLock {
    file: File,
}

impl InstallLock {
    fn acquire(root: &InstallRoot) -> Result<Self, InstallError> {
        let path = root.versions().join(".update.lock");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| match error.kind() {
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                    InstallError::NotWritable
                }
                _ => InstallError::Filesystem,
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { file }),
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                Err(InstallError::Busy)
            }
            Err(_) => Err(InstallError::Filesystem),
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Stream a file through SHA-256 without buffering it.
pub(super) fn hash_file(path: &Path) -> Result<(u64, String), InstallError> {
    let mut file = File::open(path).map_err(|_| InstallError::Filesystem)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| InstallError::Filesystem)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

#[cfg(unix)]
fn sync_dir(path: &Path) {
    // Durable directory metadata is a POSIX guarantee; Windows has no equivalent
    // through std, so the pointer rename there is atomic but not power-loss durable.
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) {}
