//! Reminder bookkeeping: 24-hour throttle, one announcement per channel and
//! version, and a short advisory OS lock.
//!
//! Two tiny critical sections exist and neither spans network I/O: the caller
//! records the attempt *before* checking (so a failed check is throttled too)
//! and records the announcement after. A busy lock is reported, never waited
//! for, so a background check can never delay the user's command.

use super::{Channel, UpdateError};
use fs2::FileExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const STATE_SCHEMA: u32 = 1;
const MAX_STATE_BYTES: u64 = 64 * 1024;

/// Durable reminder bookkeeping inside one caller-chosen directory.
pub struct ReminderStore {
    dir: PathBuf,
}

/// Outcome of asking whether an automatic check may run now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckPermit {
    /// Interval elapsed (or clock moved backwards); the attempt was recorded.
    Due,
    /// Nothing to do before this Unix second.
    TooSoon { next_check_unix: u64 },
    /// Another process holds the state lock; the caller must not wait.
    Busy,
}

/// Persisted reminder state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReminderState {
    pub schema_version: u32,
    pub last_checked_unix: Option<u64>,
    pub notified: Notified,
}

impl Default for ReminderState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA,
            last_checked_unix: None,
            notified: Notified::default(),
        }
    }
}

/// Last version announced per channel. Separate slots keep channels isolated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notified {
    #[serde(rename = "stable")]
    pub stable: Option<String>,
    #[serde(rename = "pre-release")]
    pub pre_release: Option<String>,
}

impl ReminderStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn directory(&self) -> &Path {
        &self.dir
    }

    pub fn state_path(&self) -> PathBuf {
        self.dir.join("update-reminders.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join("update-reminders.lock")
    }

    /// Ask whether an automatic check may run, recording the attempt so a
    /// failed check is throttled exactly like a successful one.
    pub fn begin_check(
        &self,
        now_unix: u64,
        interval_secs: u64,
    ) -> Result<CheckPermit, UpdateError> {
        let guard = match self.acquire() {
            Ok(guard) => guard,
            Err(UpdateError::Busy) => return Ok(CheckPermit::Busy),
            Err(error) => return Err(error),
        };
        let mut state = self.load();
        if let Some(last) = state.last_checked_unix {
            if last <= now_unix {
                let next_check_unix = last.saturating_add(interval_secs);
                if now_unix < next_check_unix {
                    drop(guard);
                    return Ok(CheckPermit::TooSoon { next_check_unix });
                }
            }
        }
        state.last_checked_unix = Some(now_unix);
        self.save(&state)?;
        drop(guard);
        Ok(CheckPermit::Due)
    }

    /// Record an announcement. Returns whether the caller should show it: the
    /// same version on the same channel is announced once.
    pub fn record_notification(
        &self,
        channel: Channel,
        version: &Version,
    ) -> Result<bool, UpdateError> {
        let guard = self.acquire()?;
        let mut state = self.load();
        let text = version.to_string();
        let slot = match channel {
            Channel::Stable => &mut state.notified.stable,
            Channel::PreRelease => &mut state.notified.pre_release,
        };
        if slot.as_deref() == Some(text.as_str()) {
            drop(guard);
            return Ok(false);
        }
        *slot = Some(text);
        self.save(&state)?;
        drop(guard);
        Ok(true)
    }

    /// Read-only snapshot for diagnostics and shell tests.
    pub fn state(&self) -> Result<ReminderState, UpdateError> {
        let _guard = self.acquire()?;
        Ok(self.load())
    }

    /// Brief advisory lock. Released by RAII, so no lock file is ever left
    /// behind in a "locked" state and no stale sentinel needs creating.
    pub(crate) fn acquire(&self) -> Result<LockGuard, UpdateError> {
        fs::create_dir_all(&self.dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.lock_path())?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(LockGuard { file }),
            Err(error) if would_block(&error) => Err(UpdateError::Busy),
            Err(_) => Err(UpdateError::Filesystem),
        }
    }

    /// Never fabricate an announcement: unreadable, oversized, unparsable or
    /// future-schema state is treated as "nothing recorded yet".
    fn load(&self) -> ReminderState {
        let Ok(metadata) = fs::metadata(self.state_path()) else {
            return ReminderState::default();
        };
        if metadata.len() > MAX_STATE_BYTES {
            return ReminderState::default();
        }
        let Ok(bytes) = fs::read(self.state_path()) else {
            return ReminderState::default();
        };
        match serde_json::from_slice::<ReminderState>(&bytes) {
            Ok(state) if state.schema_version == STATE_SCHEMA => state,
            _ => ReminderState::default(),
        }
    }

    fn save(&self, state: &ReminderState) -> Result<(), UpdateError> {
        let bytes = serde_json::to_vec(state).map_err(|_| UpdateError::Filesystem)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.dir)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.state_path())
            .map_err(|_| UpdateError::Filesystem)?;
        Ok(())
    }
}

pub(crate) struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn would_block(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        // Windows reports ERROR_LOCK_VIOLATION instead of a would-block kind.
        || error.raw_os_error() == Some(33)
        || error.raw_os_error() == Some(11)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const DAY: u64 = 24 * 60 * 60;

    fn store() -> (TempDir, ReminderStore) {
        let dir = TempDir::new().expect("temporary directory");
        let store = ReminderStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn first_check_is_due_then_the_interval_applies() {
        let (_dir, store) = store();
        assert_eq!(store.begin_check(1_000, DAY), Ok(CheckPermit::Due));
        assert_eq!(
            store.begin_check(1_000 + DAY - 1, DAY),
            Ok(CheckPermit::TooSoon {
                next_check_unix: 1_000 + DAY
            })
        );
        assert_eq!(store.begin_check(1_000 + DAY, DAY), Ok(CheckPermit::Due));
    }

    #[test]
    fn a_failed_or_empty_check_still_throttles() {
        let (_dir, store) = store();
        assert_eq!(store.begin_check(5, DAY), Ok(CheckPermit::Due));
        // Nothing was announced, yet the attempt alone must rate-limit.
        assert!(matches!(
            store.begin_check(6, DAY),
            Ok(CheckPermit::TooSoon { .. })
        ));
        assert_eq!(store.state().expect("state").notified, Notified::default());
    }

    #[test]
    fn one_announcement_per_version_and_channel() {
        let (_dir, store) = store();
        let stable = Version::parse("2.0.0").expect("version");
        let newer = Version::parse("2.1.0").expect("version");
        assert_eq!(
            store.record_notification(Channel::Stable, &stable),
            Ok(true)
        );
        assert_eq!(
            store.record_notification(Channel::Stable, &stable),
            Ok(false)
        );
        assert_eq!(
            store.record_notification(Channel::PreRelease, &stable),
            Ok(true)
        );
        assert_eq!(store.record_notification(Channel::Stable, &newer), Ok(true));
        assert_eq!(
            store.record_notification(Channel::Stable, &newer),
            Ok(false)
        );
    }

    #[test]
    fn a_busy_lock_never_blocks_the_caller() {
        let (_dir, store) = store();
        let guard = store.acquire().expect("lock");
        assert_eq!(store.begin_check(7, DAY), Ok(CheckPermit::Busy));
        assert_eq!(
            store.record_notification(Channel::Stable, &Version::parse("2.0.0").expect("version")),
            Err(UpdateError::Busy)
        );
        assert_eq!(store.state(), Err(UpdateError::Busy));
        drop(guard);
        assert_eq!(store.begin_check(7, DAY), Ok(CheckPermit::Due));
    }

    #[test]
    fn clock_rollback_does_not_disable_reminders() {
        let (_dir, store) = store();
        assert_eq!(store.begin_check(10_000_000_000, DAY), Ok(CheckPermit::Due));
        assert_eq!(store.begin_check(100, DAY), Ok(CheckPermit::Due));
        assert_eq!(store.state().expect("state").last_checked_unix, Some(100));
        assert_eq!(
            store.begin_check(200, DAY),
            Ok(CheckPermit::TooSoon {
                next_check_unix: 100 + DAY
            })
        );
    }

    #[test]
    fn corrupt_state_is_never_treated_as_already_announced() {
        let (_dir, store) = store();
        store.begin_check(1_000, DAY).expect("due");
        store
            .record_notification(Channel::Stable, &Version::parse("2.0.0").expect("version"))
            .expect("announce");
        fs::write(store.state_path(), b"{not json").expect("corrupt state");
        assert_eq!(store.state().expect("state").notified, Notified::default());
        assert_eq!(store.begin_check(2_000, DAY), Ok(CheckPermit::Due));
        assert_eq!(
            store.record_notification(Channel::Stable, &Version::parse("2.0.0").expect("version")),
            Ok(true)
        );
    }

    #[test]
    fn oversized_state_is_ignored() {
        let (_dir, store) = store();
        fs::write(
            store.state_path(),
            vec![b'x'; (MAX_STATE_BYTES + 1) as usize],
        )
        .expect("oversized state");
        assert_eq!(store.state().expect("state"), ReminderState::default());
    }

    #[test]
    fn persisted_state_keeps_the_published_shape() {
        let parsed = serde_json::from_str::<ReminderState>(
            r#"{"schema_version":1,"last_checked_unix":42,"notified":{"stable":"2.0.0","pre-release":"2.1.0-alpha.1"}}"#,
        )
        .expect("state parse");
        assert_eq!(parsed.last_checked_unix, Some(42));
        assert_eq!(parsed.notified.stable.as_deref(), Some("2.0.0"));
        assert_eq!(
            parsed.notified.pre_release.as_deref(),
            Some("2.1.0-alpha.1")
        );

        let minimal = serde_json::from_str::<ReminderState>("{}").expect("minimal state parse");
        assert_eq!(minimal, ReminderState::default());
        assert_eq!(minimal.schema_version, STATE_SCHEMA);
    }
}
