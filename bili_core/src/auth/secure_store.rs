//! Bounded immutable generations over the OS keyring's small value slots.
use super::{keyring_delete, keyring_get, keyring_set, storage_account, KeyringLoad};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, path::Path};

const CHUNK_BYTES: usize = 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_CHUNKS: u32 = 16_384;

#[derive(Debug)]
pub(super) enum SaveError {
    Unavailable,
    Failed,
}

#[derive(Clone, Serialize, Deserialize)]
struct Generation {
    id: String,
    count: u32,
}
#[derive(Serialize, Deserialize)]
struct Pointer {
    bili_secure_chunks: u32,
    active: Option<Generation>,
    sha256: Option<String>,
    #[serde(default)]
    retired: Option<Generation>,
}
enum Root {
    Missing,
    Inline(String),
    Chunked(Pointer),
}

fn writer(dir: &Path) -> std::result::Result<File, ()> {
    crate::state::reject_symlink(dir).map_err(|_| ())?;
    super::private_dir(dir).map_err(|_| ())?;
    let path = dir.join(".credential-store.lock");
    crate::state::reject_symlink(&path).map_err(|_| ())?;
    let lock = crate::state::private_options()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .map_err(|_| ())?;
    crate::cookie::restrict_private(&path, false).map_err(|_| ())?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|_| ())?;
    Ok(lock)
}
fn valid(generation: &Generation) -> bool {
    generation.count > 0
        && generation.count <= MAX_CHUNKS
        && uuid::Uuid::parse_str(&generation.id).is_ok_and(|id| id.to_string() == generation.id)
}
fn chunk_account(account: &str, generation: &Generation, index: u32) -> String {
    format!("{account}/chunks/{}/{index}", generation.id)
}
fn pending_account(account: &str) -> String {
    format!("{account}/chunks/pending")
}
fn pointer(value: &str) -> std::result::Result<Option<Pointer>, KeyringLoad> {
    if value.len() > MAX_BYTES {
        return Err(KeyringLoad::Invalid);
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(value) else {
        return Ok(None);
    };
    if json.get("bili_secure_chunks").is_none() {
        return Ok(None);
    }
    if value.len() > CHUNK_BYTES {
        return Err(KeyringLoad::Invalid);
    }
    let pointer: Pointer = serde_json::from_value(json).map_err(|_| KeyringLoad::Invalid)?;
    if pointer.bili_secure_chunks != 1
        || pointer.active.as_ref().is_some_and(|g| !valid(g))
        || pointer.retired.as_ref().is_some_and(|g| !valid(g))
        || pointer
            .active
            .as_ref()
            .zip(pointer.retired.as_ref())
            .is_some_and(|(a, b)| a.id == b.id)
        || match (&pointer.active, &pointer.sha256) {
            (Some(_), Some(hash)) => {
                hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            }
            (None, None) => false,
            _ => true,
        }
    {
        return Err(KeyringLoad::Invalid);
    }
    Ok(Some(pointer))
}
fn read_root(account: &str) -> std::result::Result<Root, KeyringLoad> {
    match keyring_get(account) {
        Ok(value) => Ok(match pointer(&value)? {
            Some(pointer) => Root::Chunked(pointer),
            None => Root::Inline(value),
        }),
        Err(KeyringLoad::Missing) => Ok(Root::Missing),
        Err(error) => Err(error),
    }
}
fn write_pointer(account: &str, pointer: &Pointer) -> std::result::Result<(), ()> {
    let value = serde_json::to_string(pointer).map_err(|_| ())?;
    if value.len() > CHUNK_BYTES {
        return Err(());
    }
    keyring_set(account, &value)
}
fn remove_generation(account: &str, generation: &Generation) -> std::result::Result<(), ()> {
    if !valid(generation) {
        return Err(());
    }
    for index in 0..generation.count {
        keyring_delete(&chunk_account(account, generation, index))?;
    }
    Ok(())
}
fn cleanup_pending(
    account: &str,
    active: Option<&Generation>,
) -> std::result::Result<(), KeyringLoad> {
    let key = pending_account(account);
    let value = match keyring_get(&key) {
        Ok(value) => value,
        Err(KeyringLoad::Missing) => return Ok(()),
        Err(error) => return Err(error),
    };
    if value.len() > CHUNK_BYTES {
        return Err(KeyringLoad::Invalid);
    }
    let generation: Generation = serde_json::from_str(&value).map_err(|_| KeyringLoad::Invalid)?;
    if !valid(&generation) {
        return Err(KeyringLoad::Invalid);
    }
    if active.is_none_or(|current| current.id != generation.id) {
        remove_generation(account, &generation).map_err(|_| KeyringLoad::Unavailable)?;
    }
    keyring_delete(&key).map_err(|_| KeyringLoad::Unavailable)
}
fn cleanup(account: &str, root: &mut Root) -> std::result::Result<(), KeyringLoad> {
    let active = match root {
        Root::Chunked(pointer) => pointer.active.as_ref(),
        _ => None,
    };
    cleanup_pending(account, active)?;
    if let Root::Chunked(pointer) = root {
        if let Some(retired) = &pointer.retired {
            remove_generation(account, retired).map_err(|_| KeyringLoad::Unavailable)?;
            pointer.retired = None;
            write_pointer(account, pointer).map_err(|_| KeyringLoad::Unavailable)?;
        }
    }
    Ok(())
}

pub(super) fn set(dir: &Path, value: &str) -> std::result::Result<(), SaveError> {
    if value.len() > MAX_BYTES {
        return Err(SaveError::Failed);
    }
    let _writer = writer(dir).map_err(|_| SaveError::Failed)?;
    let account = storage_account(dir);
    let mut old = read_root(&account).map_err(|error| match error {
        KeyringLoad::Unavailable => SaveError::Unavailable,
        _ => SaveError::Failed,
    })?;
    cleanup(&account, &mut old).map_err(|_| SaveError::Failed)?;
    let retired = match &old {
        Root::Chunked(pointer) => pointer.active.clone(),
        _ => None,
    };
    if value.len() <= CHUNK_BYTES && retired.is_none() {
        return keyring_set(&account, value).map_err(|_| SaveError::Failed);
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let mut end = (start + CHUNK_BYTES).min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&value[start..end]);
        start = end;
    }
    let generation = Generation {
        id: uuid::Uuid::new_v4().to_string(),
        count: u32::try_from(chunks.len()).map_err(|_| SaveError::Failed)?,
    };
    if !valid(&generation) {
        return Err(SaveError::Failed);
    }
    let pending = pending_account(&account);
    let marker = serde_json::to_string(&generation).map_err(|_| SaveError::Failed)?;
    keyring_set(&pending, &marker).map_err(|_| SaveError::Failed)?;
    for (index, chunk) in chunks.iter().enumerate() {
        if keyring_set(&chunk_account(&account, &generation, index as u32), chunk).is_err() {
            let _ = cleanup_pending(&account, retired.as_ref());
            return Err(SaveError::Failed);
        }
    }
    let pointer = Pointer {
        bili_secure_chunks: 1,
        active: Some(generation),
        sha256: Some(format!("{:x}", Sha256::digest(value.as_bytes()))),
        retired,
    };
    if write_pointer(&account, &pointer).is_err() {
        let active = match &old {
            Root::Chunked(old) => old.active.as_ref(),
            _ => None,
        };
        let _ = cleanup_pending(&account, active);
        return Err(SaveError::Failed);
    }
    let mut root = Root::Chunked(pointer);
    cleanup(&account, &mut root).map_err(|_| SaveError::Failed)
}

pub(super) fn get(dir: &Path) -> std::result::Result<String, KeyringLoad> {
    let _writer = writer(dir).map_err(|_| KeyringLoad::Unavailable)?;
    let account = storage_account(dir);
    let mut root = read_root(&account)?;
    cleanup(&account, &mut root)?;
    match root {
        Root::Missing => Err(KeyringLoad::Missing),
        Root::Inline(value) => Ok(value),
        Root::Chunked(pointer) => {
            let Some(generation) = pointer.active else {
                return Err(KeyringLoad::Missing);
            };
            let mut value = String::new();
            for index in 0..generation.count {
                let chunk = keyring_get(&chunk_account(&account, &generation, index))
                    .map_err(|_| KeyringLoad::Invalid)?;
                if chunk.len() > CHUNK_BYTES || value.len().saturating_add(chunk.len()) > MAX_BYTES
                {
                    return Err(KeyringLoad::Invalid);
                }
                value.push_str(&chunk);
            }
            if Some(format!("{:x}", Sha256::digest(value.as_bytes()))) != pointer.sha256 {
                return Err(KeyringLoad::Invalid);
            }
            Ok(value)
        }
    }
}

pub(super) fn delete(dir: &Path) -> std::result::Result<(), ()> {
    let _writer = writer(dir)?;
    let account = storage_account(dir);
    let mut root = read_root(&account).map_err(|_| ())?;
    cleanup(&account, &mut root).map_err(|_| ())?;
    if let Root::Chunked(pointer) = root {
        // A committed tombstone never points readers at partially deleted chunks.
        let mut tombstone = Root::Chunked(Pointer {
            bili_secure_chunks: 1,
            active: None,
            sha256: None,
            retired: pointer.active,
        });
        if let Root::Chunked(pointer) = &tombstone {
            write_pointer(&account, pointer)?;
        }
        cleanup(&account, &mut tombstone).map_err(|_| ())?;
    }
    keyring_delete(&account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_keyring;

    #[test]
    fn interrupted_chunk_write_preserves_the_old_collection_and_cleans_exact_new_entries() {
        let dir = tempfile::tempdir().unwrap();
        let account = storage_account(dir.path());
        let unrelated = format!("unrelated-{}", uuid::Uuid::new_v4());
        keyring_set(&unrelated, "unrelated fixture").unwrap();
        let original = "原始凭据".repeat(600);
        set(dir.path(), &original).unwrap();
        let old_keys = test_keyring::owned_entries(&account)
            .into_iter()
            .map(|(key, _)| key)
            .collect::<std::collections::HashSet<_>>();
        test_keyring::fail_chunks_after(&account, Some(1));
        assert!(matches!(
            set(dir.path(), &"replacement".repeat(1000)),
            Err(SaveError::Failed)
        ));
        test_keyring::fail_chunks_after(&account, None);
        assert_eq!(get(dir.path()).unwrap(), original);
        let keys = test_keyring::owned_entries(&account)
            .into_iter()
            .map(|(key, _)| key)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(keys, old_keys);
        assert_eq!(keyring_get(&unrelated).unwrap(), "unrelated fixture");
        delete(dir.path()).unwrap();
        assert!(test_keyring::owned_entries(&account).is_empty());
        keyring_delete(&unrelated).unwrap();
    }

    #[test]
    fn failed_retirement_or_delete_keeps_a_retryable_marker_and_never_acknowledges_forget() {
        let dir = tempfile::tempdir().unwrap();
        let account = storage_account(dir.path());
        let original = "a".repeat(3000);
        set(dir.path(), &original).unwrap();
        let root = keyring_get(&account).unwrap();
        let first = pointer(&root).unwrap().unwrap().active.unwrap();
        let protected = chunk_account(&account, &first, 0);
        test_keyring::fail_delete(&protected, true);
        let replacement = "新凭据".repeat(1200);
        assert!(matches!(
            set(dir.path(), &replacement),
            Err(SaveError::Failed)
        ));
        assert!(matches!(get(dir.path()), Err(KeyringLoad::Unavailable)));
        test_keyring::fail_delete(&protected, false);
        assert_eq!(get(dir.path()).unwrap(), replacement);
        assert!(!test_keyring::contains(&protected));
        let current = pointer(&keyring_get(&account).unwrap())
            .unwrap()
            .unwrap()
            .active
            .unwrap();
        let protected = chunk_account(&account, &current, 0);
        test_keyring::fail_delete(&protected, true);
        assert!(delete(dir.path()).is_err());
        let tombstone = pointer(&keyring_get(&account).unwrap()).unwrap().unwrap();
        assert!(tombstone.active.is_none());
        assert!(tombstone.retired.is_some());
        assert!(matches!(get(dir.path()), Err(KeyringLoad::Unavailable)));
        test_keyring::fail_delete(&protected, false);
        delete(dir.path()).unwrap();
        assert!(test_keyring::owned_entries(&account).is_empty());
    }

    #[test]
    fn chunk_integrity_and_generation_bounds_are_checked_without_plaintext_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let account = storage_account(dir.path());
        let value = "safe-data".repeat(1000);
        set(dir.path(), &value).unwrap();
        let root = pointer(&keyring_get(&account).unwrap()).unwrap().unwrap();
        let generation = root.active.unwrap();
        keyring_set(&chunk_account(&account, &generation, 0), "tampered").unwrap();
        assert!(matches!(get(dir.path()), Err(KeyringLoad::Invalid)));
        assert!(crate::auth::load_secret(dir.path(), true).is_err());
        delete(dir.path()).unwrap();
        assert!(test_keyring::owned_entries(&account).is_empty());
        let invalid=serde_json::json!({"bili_secure_chunks":1,"active":{"id":"../unrelated","count":1},"sha256":"0".repeat(64)}).to_string();
        keyring_set(&account, &invalid).unwrap();
        assert!(matches!(get(dir.path()), Err(KeyringLoad::Invalid)));
        keyring_delete(&account).unwrap();
    }
}
