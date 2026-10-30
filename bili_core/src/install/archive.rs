//! Archive extraction and inventory checks for packaged payloads.
//!
//! Every path an archive supplies is validated before it can touch the
//! filesystem: only normal relative components are accepted, parents are never
//! written through a symlink, hard links and special files are refused, and a
//! symlink must stay inside the tree it belongs to. The archive is never
//! buffered in memory: entries stream into the destination.

use super::{InstallError, MAX_EXTRACTED_BYTES};
use crate::update::{ArchiveKind, Target};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

/// Maximum number of entries one archive may carry.
const MAX_ENTRIES: usize = 20_000;
/// Maximum depth of a single entry path.
const MAX_DEPTH: usize = 16;
/// Maximum size of an inventory file.
const MAX_INVENTORY_BYTES: u64 = 8 * 1024 * 1024;

/// Extract `archive` into `destination` and return its single top-level directory.
pub(super) fn extract(
    archive: &Path,
    kind: ArchiveKind,
    destination: &Path,
    target: Target,
) -> Result<PathBuf, InstallError> {
    fs::create_dir_all(destination).map_err(|_| InstallError::Filesystem)?;
    let mut sink = Sink::new(destination.to_path_buf(), target);
    match kind {
        ArchiveKind::TarGz => extract_tar(archive, &mut sink)?,
        ArchiveKind::Zip => extract_zip(archive, &mut sink)?,
    }
    let mut entries = fs::read_dir(destination).map_err(|_| InstallError::Filesystem)?;
    let first = entries
        .next()
        .transpose()
        .map_err(|_| InstallError::Filesystem)?
        .ok_or(InstallError::ArchiveLayout)?;
    if entries.next().is_some() {
        return Err(InstallError::ArchiveLayout);
    }
    let metadata = fs::symlink_metadata(first.path()).map_err(|_| InstallError::Filesystem)?;
    if !metadata.is_dir() {
        return Err(InstallError::ArchiveLayout);
    }
    Ok(first.path())
}

fn extract_tar(archive: &Path, sink: &mut Sink) -> Result<(), InstallError> {
    let file = File::open(archive).map_err(|_| InstallError::Filesystem)?;
    let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
    let mut tar = tar::Archive::new(decoder);
    let entries = tar.entries().map_err(|_| InstallError::ArchiveLayout)?;
    for entry in entries {
        let mut entry = entry.map_err(|_| InstallError::ArchiveLayout)?;
        let path = entry
            .path()
            .map_err(|_| InstallError::ArchiveLayout)?
            .to_path_buf();
        let outer = entry.header().entry_type();
        let mode = entry.header().mode().unwrap_or(0o644);
        if outer.is_dir() {
            sink.create_dir(&path)?;
            continue;
        }
        if outer.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|_| InstallError::ArchiveLayout)?
                .ok_or(InstallError::ArchiveLayout)?;
            sink.create_symlink(&path, &target)?;
            continue;
        }
        if !outer.is_file() {
            // Hard links, character/block devices, FIFOs and unknown types.
            return Err(InstallError::ArchiveLayout);
        }
        let size = entry
            .header()
            .size()
            .map_err(|_| InstallError::ArchiveLayout)?;
        sink.write_file(&path, &mut entry, size, Some(mode))?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, sink: &mut Sink) -> Result<(), InstallError> {
    let file = File::open(archive).map_err(|_| InstallError::Filesystem)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| InstallError::ArchiveLayout)?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|_| InstallError::ArchiveLayout)?;
        let enclosed = entry.enclosed_name().ok_or(InstallError::ArchiveLayout)?;
        if entry.is_dir() {
            sink.create_dir(&enclosed)?;
            continue;
        }
        let size = entry.size();
        let mode = entry.unix_mode();
        if is_zip_symlink(mode) {
            let mut target = String::new();
            entry
                .take(size.min(4096))
                .read_to_string(&mut target)
                .map_err(|_| InstallError::ArchiveLayout)?;
            sink.create_symlink(&enclosed, Path::new(target.trim_end_matches('\0')))?;
            continue;
        }
        sink.write_file(&enclosed, &mut entry, size, mode)?;
    }
    Ok(())
}

#[cfg(unix)]
fn is_zip_symlink(mode: Option<u32>) -> bool {
    mode.is_some_and(|mode| mode & 0o170000 == 0o120000)
}

#[cfg(not(unix))]
fn is_zip_symlink(_mode: Option<u32>) -> bool {
    false
}

/// Destination-side bookkeeping for one extraction.
struct Sink {
    destination: PathBuf,
    case_insensitive: bool,
    seen: HashSet<String>,
    bytes: u64,
}

impl Sink {
    fn new(destination: PathBuf, target: Target) -> Self {
        Self {
            destination,
            case_insensitive: target == Target::WindowsX86_64Msvc,
            seen: HashSet::new(),
            bytes: 0,
        }
    }

    /// Validate and register an entry path, rejecting duplicates.
    fn claim(&mut self, path: &Path) -> Result<PathBuf, InstallError> {
        let relative = safe_relative(path).ok_or(InstallError::ArchiveLayout)?;
        let key = relative.to_string_lossy().replace('\\', "/");
        let key = if self.case_insensitive {
            key.to_lowercase()
        } else {
            key
        };
        if !self.seen.insert(key) {
            return Err(InstallError::ArchiveLayout);
        }
        Ok(relative)
    }

    fn absolute(&self, relative: &Path) -> PathBuf {
        self.destination.join(relative)
    }

    fn create_dir(&mut self, path: &Path) -> Result<(), InstallError> {
        let relative = self.claim(path)?;
        self.ensure_parents(&relative)?;
        let absolute = self.absolute(&relative);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(InstallError::ArchiveLayout),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&absolute).map_err(|_| InstallError::Filesystem)
            }
            Err(_) => Err(InstallError::Filesystem),
        }
    }

    fn write_file(
        &mut self,
        path: &Path,
        reader: &mut impl Read,
        size: u64,
        mode: Option<u32>,
    ) -> Result<(), InstallError> {
        let relative = self.claim(path)?;
        self.ensure_parents(&relative)?;
        self.bytes = self
            .bytes
            .checked_add(size)
            .filter(|total| *total <= MAX_EXTRACTED_BYTES)
            .ok_or(InstallError::ArchiveLayout)?;
        let absolute = self.absolute(&relative);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&absolute)
            .map_err(|_| InstallError::ArchiveLayout)?;
        let copied =
            io::copy(&mut reader.take(size), &mut file).map_err(|_| InstallError::Filesystem)?;
        file.flush().map_err(|_| InstallError::Filesystem)?;
        if copied != size {
            return Err(InstallError::ArchiveLayout);
        }
        drop(file);
        set_mode(&absolute, mode)?;
        Ok(())
    }

    fn create_symlink(&mut self, path: &Path, target: &Path) -> Result<(), InstallError> {
        let relative = self.claim(path)?;
        self.ensure_parents(&relative)?;
        if !contained(&relative, target) {
            return Err(InstallError::ArchiveLayout);
        }
        let absolute = self.absolute(&relative);
        // A file target may appear later in the archive. The complete tree is
        // checked by check_links before inventory validation or activation.
        symlink(target, &absolute).map_err(|_| InstallError::ArchiveLayout)
    }

    /// Create missing parent directories, refusing to write through a symlink.
    fn ensure_parents(&mut self, relative: &Path) -> Result<(), InstallError> {
        let parent = match relative.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => return Ok(()),
        };
        let mut current = self.destination.clone();
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err(InstallError::ArchiveLayout);
            };
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Err(InstallError::ArchiveLayout),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(|_| InstallError::Filesystem)?;
                }
                Err(_) => return Err(InstallError::Filesystem),
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = mode.unwrap_or(0o644) & 0o777;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| InstallError::Filesystem)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<(), InstallError> {
    Ok(())
}

/// Whether `link_target` (seen from the directory holding `link`) stays inside the tree.
fn contained(link: &Path, link_target: &Path) -> bool {
    if link_target.is_absolute() {
        return false;
    }
    let mut depth = link
        .parent()
        .map_or(0, |parent| parent.components().count());
    for component in link_target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// Convert an archive path into a strictly relative path made of normal components.
pub(super) fn safe_relative(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                relative.push(name);
                depth += 1;
                if depth > MAX_DEPTH {
                    return None;
                }
            }
            _ => return None,
        }
    }
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}

/// Reject any symlink that escapes the tree it belongs to.
pub(super) fn check_links(tree: &Path) -> Result<(), InstallError> {
    let canonical_root = fs::canonicalize(tree).map_err(|_| InstallError::Filesystem)?;
    let mut stack = vec![PathBuf::new()];
    let mut visited = 0usize;
    while let Some(relative) = stack.pop() {
        let absolute = tree.join(&relative);
        let metadata = fs::symlink_metadata(&absolute).map_err(|_| InstallError::Filesystem)?;
        if metadata.is_symlink() {
            let target = fs::read_link(&absolute).map_err(|_| InstallError::Filesystem)?;
            if !contained(&relative, &target) {
                return Err(InstallError::ArchiveLayout);
            }
            let resolved = fs::canonicalize(&absolute).map_err(|_| InstallError::ArchiveLayout)?;
            if !resolved.starts_with(&canonical_root) || !resolved.is_file() {
                return Err(InstallError::ArchiveLayout);
            }
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&absolute).map_err(|_| InstallError::Filesystem)? {
                let entry = entry.map_err(|_| InstallError::Filesystem)?;
                visited += 1;
                if visited > MAX_ENTRIES {
                    return Err(InstallError::ArchiveLayout);
                }
                stack.push(relative.join(entry.file_name()));
            }
            continue;
        }
        if !metadata.is_file() {
            return Err(InstallError::ArchiveLayout);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(InstallError::ArchiveLayout);
            }
        }
    }
    Ok(())
}

/// Verify a SHA-256 inventory against the tree it describes.
///
/// Lines carry `sha256` of the *file contents*, so a symlink is hashed through
/// its target exactly like the packaging writer does. Every non-directory entry
/// in the tree must be listed, and the inventory file itself is excluded.
pub(super) fn verify_inventory(tree: &Path, sums: &Path) -> Result<(), InstallError> {
    let metadata = fs::metadata(sums).map_err(|_| InstallError::InventoryMismatch)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_INVENTORY_BYTES {
        return Err(InstallError::InventoryMismatch);
    }
    let text = fs::read_to_string(sums).map_err(|_| InstallError::InventoryMismatch)?;
    let mut listed: HashSet<String> = HashSet::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            return Err(InstallError::InventoryMismatch);
        }
        let (digest, name) = line
            .split_once("  ")
            .ok_or(InstallError::InventoryMismatch)?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InstallError::InventoryMismatch);
        }
        let relative = safe_relative(Path::new(name)).ok_or(InstallError::InventoryMismatch)?;
        let key = relative.to_string_lossy().replace('\\', "/");
        if !listed.insert(key) {
            return Err(InstallError::InventoryMismatch);
        }
        let actual =
            super::hash_file(&tree.join(&relative)).map_err(|_| InstallError::InventoryMismatch)?;
        if actual.1 != digest {
            return Err(InstallError::InventoryMismatch);
        }
        if listed.len() > MAX_ENTRIES {
            return Err(InstallError::InventoryMismatch);
        }
    }
    if listed.is_empty() {
        return Err(InstallError::InventoryMismatch);
    }
    let excluded = sums
        .strip_prefix(tree)
        .map_err(|_| InstallError::InventoryMismatch)?
        .to_string_lossy()
        .replace('\\', "/");
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let absolute = tree.join(&relative);
        let metadata = fs::symlink_metadata(&absolute).map_err(|_| InstallError::Filesystem)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(&absolute).map_err(|_| InstallError::Filesystem)? {
                let entry = entry.map_err(|_| InstallError::Filesystem)?;
                stack.push(relative.join(entry.file_name()));
            }
            continue;
        }
        let key = relative.to_string_lossy().replace('\\', "/");
        if key == excluded {
            continue;
        }
        if !listed.contains(&key) {
            return Err(InstallError::InventoryMismatch);
        }
    }
    Ok(())
}
