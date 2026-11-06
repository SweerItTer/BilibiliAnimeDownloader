//! Explicit, disposable isolation for debugging the normal native GUI.
use crate::prefs::GuiPrefs;
use bili_core::{CoreConfig, CoreError, ErrorCode, OfflineApiReplay, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const ROOT_ENV: &str = "BILI_GUI_ISOLATED_DEBUG_DIR";
const LIVE_ENV: &str = "BILI_GUI_ANONYMOUS_LIVE_DIR";
const REPLAY_ENV: &str = "BILI_GUI_OFFLINE_REPLAY";
const DEAD_PROXY: &str = "http://127.0.0.1:0";

pub struct DebugIsolation {
    root: PathBuf,
    api_replay: Option<OfflineApiReplay>,
}

impl DebugIsolation {
    pub fn from_environment(state_dir: Option<&Path>) -> Result<Option<Self>> {
        Self::from_values(state_dir, |key| std::env::var_os(key))
    }

    fn from_values(
        state_dir: Option<&Path>,
        environment: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Option<Self>> {
        if let Some(root) = environment(LIVE_ENV) {
            if environment(ROOT_ENV).is_some() || environment(REPLAY_ENV).is_some() {
                return Err(invalid(
                    "anonymous live GUI cannot use offline isolation or replay",
                ));
            }
            let root = PathBuf::from(root);
            Self::validate_root(&root, state_dir)?;
            return Self::create(root, None).map(Some);
        }
        let Some(root) = environment(ROOT_ENV) else {
            if environment(REPLAY_ENV).is_some() {
                return Err(invalid(
                    "offline API replay requires isolated GUI debugging",
                ));
            }
            return Ok(None);
        };
        Self::prepare(PathBuf::from(root), state_dir, environment).map(Some)
    }

    fn prepare(
        root: PathBuf,
        state_dir: Option<&Path>,
        environment: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self> {
        Self::validate_root(&root, state_dir)?;
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            if environment(key).as_deref() != Some(OsStr::new(DEAD_PROXY)) {
                return Err(invalid(
                    "isolated GUI debugging requires the closed-loopback proxy",
                ));
            }
        }
        if environment("NO_PROXY").as_deref() != Some(OsStr::new("")) {
            return Err(invalid("isolated GUI debugging requires an empty NO_PROXY"));
        }
        // Lowercase proxy variables take precedence in some HTTP stacks. Never
        // permit an inherited alternate proxy or direct-connect exception.
        for key in ["http_proxy", "https_proxy", "all_proxy"] {
            if environment(key).is_some_and(|value| value.as_os_str() != OsStr::new(DEAD_PROXY)) {
                return Err(invalid("isolated GUI debugging has a conflicting proxy"));
            }
        }
        if environment("no_proxy").is_some_and(|value| !value.is_empty()) {
            return Err(invalid("isolated GUI debugging has a proxy bypass"));
        }

        let api_replay = environment(REPLAY_ENV)
            .map(|path| {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(invalid("offline API replay path must be absolute"));
                }
                OfflineApiReplay::load(&path)
            })
            .transpose()?;

        Self::create(root, api_replay)
    }

    fn validate_root(root: &Path, state_dir: Option<&Path>) -> Result<()> {
        if !root.is_absolute() {
            return Err(invalid("isolated GUI debug directory must be absolute"));
        }
        if state_dir.is_some() {
            return Err(invalid("isolated GUI debugging cannot use --state-dir"));
        }
        Ok(())
    }

    fn create(root: PathBuf, api_replay: Option<OfflineApiReplay>) -> Result<Self> {
        // An atomic create rejects existing tasks, accounts, preferences and
        // symlinks at the chosen root instead of silently reusing their data.
        std::fs::create_dir(&root).map_err(CoreError::io)?;
        for name in ["state", "output", "preferences"] {
            std::fs::create_dir(root.join(name)).map_err(CoreError::io)?;
        }
        Ok(Self { root, api_replay })
    }

    pub fn preferences(&self) -> GuiPrefs {
        GuiPrefs::load_isolated(
            self.root.join("preferences/gui-preferences.json"),
            self.root.join("output"),
        )
    }

    pub fn configuration(&self) -> CoreConfig {
        // CoreConfig defaults to memory-only authentication. In particular,
        // never locate the normal GUI auth store or initialize its OS keyring.
        let mut config = CoreConfig::new(self.root.join("state"), self.root.join("output"));
        config.api_replay = self.api_replay.clone();
        config
    }
}

fn invalid(message: &'static str) -> CoreError {
    CoreError::new(ErrorCode::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn environment() -> HashMap<&'static str, OsString> {
        HashMap::from([
            ("HTTP_PROXY", DEAD_PROXY.into()),
            ("HTTPS_PROXY", DEAD_PROXY.into()),
            ("ALL_PROXY", DEAD_PROXY.into()),
            ("NO_PROXY", OsString::new()),
        ])
    }

    #[test]
    fn isolated_start_has_empty_tasks_output_and_memory_only_accounts() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("session");
        let values = environment();
        let isolation =
            DebugIsolation::prepare(root.clone(), None, |key| values.get(key).cloned()).unwrap();
        let config = isolation.configuration();
        assert!(config.api_replay.is_none());
        assert_eq!(config.state_dir, root.join("state"));
        assert_eq!(config.output_dir, root.join("output"));
        assert!(matches!(
            config.credential_store,
            bili_core::cookie::StorePolicy::MemoryOnly
        ));
        assert_eq!(std::fs::read_dir(&config.state_dir).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&config.output_dir).unwrap().count(), 0);
        let prefs = isolation.preferences();
        assert!(!prefs.remember_login);
        assert!(!prefs.update.check_updates);
        assert!(!prefs.update.auto_update);
        assert_eq!(prefs.output, Some(config.output_dir));
        assert!(DebugIsolation::prepare(root, None, |key| values.get(key).cloned()).is_err());
    }

    #[test]
    fn incomplete_proxy_isolation_fails_before_creating_any_state() {
        let parent = tempfile::tempdir().unwrap();
        for (index, (key, value)) in [
            ("HTTP_PROXY", None),
            ("HTTPS_PROXY", Some("http://127.0.0.1:8080")),
            ("ALL_PROXY", None),
            ("NO_PROXY", None),
            ("NO_PROXY", Some("*")),
            ("http_proxy", Some("http://127.0.0.1:8080")),
            ("no_proxy", Some("localhost")),
        ]
        .into_iter()
        .enumerate()
        {
            let mut values = environment();
            match value {
                Some(value) => {
                    values.insert(key, value.into());
                }
                None => {
                    values.remove(key);
                }
            }
            let root = parent.path().join(format!("session-{index}"));
            assert!(DebugIsolation::prepare(root.clone(), None, |key| {
                values.get(key).cloned()
            })
            .is_err());
            assert!(!root.exists());
        }
    }

    #[test]
    fn relative_root_and_state_override_fail_before_creating_any_state() {
        let parent = tempfile::tempdir().unwrap();
        let values = environment();
        assert!(
            DebugIsolation::prepare(PathBuf::from("relative"), None, |key| {
                values.get(key).cloned()
            })
            .is_err()
        );
        let root = parent.path().join("session");
        assert!(
            DebugIsolation::prepare(root.clone(), Some(parent.path()), |key| {
                values.get(key).cloned()
            })
            .is_err()
        );
        assert!(!root.exists());
    }

    #[test]
    fn replay_requires_explicit_isolation_instead_of_normal_startup() {
        let mut values = environment();
        assert!(
            DebugIsolation::from_values(None, |key| values.get(key).cloned())
                .unwrap()
                .is_none()
        );
        values.insert(REPLAY_ENV, OsString::from("replay.json"));
        assert!(DebugIsolation::from_values(None, |key| values.get(key).cloned()).is_err());
    }

    #[test]
    fn anonymous_live_uses_real_transport_without_saved_accounts_or_updates() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("live");
        let values = HashMap::from([(LIVE_ENV, root.as_os_str().to_owned())]);
        let isolation = DebugIsolation::from_values(None, |key| values.get(key).cloned())
            .unwrap()
            .unwrap();
        let config = isolation.configuration();
        assert!(config.api_replay.is_none());
        assert!(matches!(
            config.credential_store,
            bili_core::cookie::StorePolicy::MemoryOnly
        ));
        assert_eq!(config.state_dir, root.join("state"));
        let prefs = isolation.preferences();
        assert!(!prefs.remember_login);
        assert!(!prefs.update.check_updates);
        assert!(!prefs.update.auto_update);
        assert!(DebugIsolation::from_values(None, |key| values.get(key).cloned()).is_err());
    }

    #[test]
    fn anonymous_live_rejects_replay_and_conflicting_paths_before_creating_state() {
        let parent = tempfile::tempdir().unwrap();
        for conflict in [ROOT_ENV, REPLAY_ENV] {
            let root = parent.path().join(conflict);
            let values = HashMap::from([
                (LIVE_ENV, root.as_os_str().to_owned()),
                (conflict, "other".into()),
            ]);
            assert!(DebugIsolation::from_values(None, |key| values.get(key).cloned()).is_err());
            assert!(!root.exists());
        }
        let root = parent.path().join("override");
        let values = HashMap::from([(LIVE_ENV, root.as_os_str().to_owned())]);
        assert!(
            DebugIsolation::from_values(Some(parent.path()), |key| values.get(key).cloned())
                .is_err()
        );
        assert!(!root.exists());
        let values = HashMap::from([(LIVE_ENV, "relative".into())]);
        assert!(DebugIsolation::from_values(None, |key| values.get(key).cloned()).is_err());
    }

    #[test]
    fn invalid_replay_fails_before_creating_the_private_root() {
        let parent = tempfile::tempdir().unwrap();
        let malformed = parent.path().join("malformed.json");
        std::fs::write(&malformed, b"{").unwrap();
        for (index, path) in [
            PathBuf::from("relative.json"),
            parent.path().join("missing.json"),
            malformed,
        ]
        .into_iter()
        .enumerate()
        {
            let mut values = environment();
            values.insert(REPLAY_ENV, path.into_os_string());
            let root = parent.path().join(format!("session-{index}"));
            assert!(DebugIsolation::prepare(root.clone(), None, |key| {
                values.get(key).cloned()
            })
            .is_err());
            assert!(!root.exists());
        }
    }

    #[test]
    fn validated_replay_is_attached_to_the_memory_only_core_configuration() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("replay.json");
        std::fs::write(
            &path,
            br#"{"schema_version":1,"responses":[{"path":"/x/web-interface/nav","query":{},"body":{"code":0,"data":{}},"delay_ms":0}]}"#,
        )
        .unwrap();
        let mut values = environment();
        values.insert(REPLAY_ENV, path.into_os_string());
        let root = parent.path().join("session");
        values.insert(ROOT_ENV, root.clone().into_os_string());
        let isolation = DebugIsolation::from_values(None, |key| values.get(key).cloned())
            .unwrap()
            .unwrap();
        let config = isolation.configuration();
        assert!(config.api_replay.is_some());
        assert!(matches!(
            config.credential_store,
            bili_core::cookie::StorePolicy::MemoryOnly
        ));
        assert_eq!(config.state_dir, root.join("state"));
        assert_eq!(config.output_dir, root.join("output"));
    }
}
