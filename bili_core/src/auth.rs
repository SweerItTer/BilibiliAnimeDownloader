//! Session-scoped Bilibili authentication. Secrets never implement `Debug` or `Serialize`.
use crate::{
    cookie::{CredentialJar, SharedCredentialJar, StorePolicy},
    domain::{CoreError, ErrorCode, Result},
};
use reqwest::{header::SET_COOKIE, Client, Url};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::task::JoinSet;

pub enum AuthAction {
    ImportCookie { header: String, remember: bool },
    LoadStored { required: bool },
    Anonymous,
    StartQr { remember: bool },
    PollQr,
    CancelQr,
    Status,
    Logout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QrStatus {
    WaitingForScan,
    WaitingForConfirmation,
    Expired,
    Complete,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthAccount {
    pub mid: u64,
    pub name: String,
    pub face: Option<String>,
}

#[derive(Clone)]
pub struct QrChallenge {
    pub login_url: String,
    pub expires_in_secs: u64,
}
impl std::fmt::Debug for QrChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrChallenge")
            .field("login_url", &"[REDACTED]")
            .field("expires_in_secs", &self.expires_in_secs)
            .finish()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSnapshot {
    pub logged_in: bool,
    pub account: Option<AuthAccount>,
    pub notice: Option<String>,
    #[serde(skip)]
    pub qr: Option<QrChallenge>,
    pub qr_status: Option<QrStatus>,
}
impl AuthSnapshot {
    fn new() -> Self {
        Self {
            logged_in: false,
            account: None,
            notice: None,
            qr: None,
            qr_status: None,
        }
    }
}

struct PendingQr {
    token: String,
    remember: bool,
}
impl std::fmt::Debug for PendingQr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingQr([REDACTED])")
    }
}

pub(crate) struct AuthManager {
    client: Client,
    jar: SharedCredentialJar,
    policy: StorePolicy,
    connections: std::sync::Arc<tokio::sync::Semaphore>,
    pending_qr: Option<PendingQr>,
    store_tasks: JoinSet<Result<()>>,
}
impl AuthManager {
    pub(crate) fn new(
        client: Client,
        jar: SharedCredentialJar,
        policy: StorePolicy,
        connections: std::sync::Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            client,
            jar,
            policy,
            connections,
            pending_qr: None,
            store_tasks: JoinSet::new(),
        }
    }

    pub(crate) async fn apply(&mut self, action: AuthAction) -> Result<AuthSnapshot> {
        self.join_store_tasks().await?;
        match action {
            AuthAction::ImportCookie { header, remember } => {
                if remember && matches!(self.policy, StorePolicy::MemoryOnly) {
                    return Err(store_error());
                }
                let jar = CredentialJar::import_header(&header)?;
                *self.jar.write().map_err(|_| store_error())? = jar;
                let mut snap = match self.status().await {
                    Ok(snap) => snap,
                    Err(error) => {
                        self.clear_jar();
                        return Err(error);
                    }
                };
                if !snap.logged_in {
                    self.clear_jar();
                    return Err(login_error());
                }
                self.pending_qr = None;
                if let Some(fallback) = self.remember_if_requested(remember).await? {
                    snap.notice = Some(store_notice(fallback).into());
                }
                Ok(snap)
            }
            AuthAction::LoadStored { required } => self.load_stored(required).await,
            AuthAction::Anonymous => {
                self.pending_qr = None;
                self.clear_jar();
                Ok(AuthSnapshot::new())
            }
            AuthAction::StartQr { remember } => {
                if remember && matches!(self.policy, StorePolicy::MemoryOnly) {
                    return Err(store_error());
                }
                self.pending_qr = None;
                let url = Url::parse(
                    "https://passport.bilibili.com/x/passport-login/web/qrcode/generate",
                )
                .map_err(|_| transport_error())?;
                let _permit = self
                    .connections
                    .acquire()
                    .await
                    .map_err(|_| transport_error())?;
                let response = self
                    .client
                    .get(url)
                    .send()
                    .await
                    .map_err(|_| transport_error())?;
                if !response.status().is_success() {
                    return Err(transport_error());
                }
                let data: serde_json::Value =
                    response.json().await.map_err(|_| transport_error())?;
                drop(_permit);
                let (login_url, token) = parse_qr_challenge(&data)?;
                self.pending_qr = Some(PendingQr { token, remember });
                let mut snap = AuthSnapshot::new();
                snap.qr = Some(QrChallenge {
                    login_url: login_url.to_string(),
                    expires_in_secs: 180,
                });
                snap.qr_status = Some(QrStatus::WaitingForScan);
                Ok(snap)
            }
            AuthAction::PollQr => self.poll_qr().await,
            AuthAction::CancelQr => {
                self.pending_qr = None;
                let mut snap = AuthSnapshot::new();
                snap.qr_status = Some(QrStatus::Cancelled);
                snap.notice = Some("QR login cancelled.".into());
                Ok(snap)
            }
            AuthAction::Status => self.status().await,
            AuthAction::Logout => {
                self.pending_qr = None;
                self.clear_jar();
                self.delete_stored().await?;
                let mut snap = AuthSnapshot::new();
                snap.notice = Some("Logged out; stored credentials removed.".into());
                Ok(snap)
            }
        }
    }

    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        let tasks = self.join_store_tasks().await;
        self.pending_qr = None;
        let cleared = self
            .jar
            .write()
            .map(|mut jar| jar.clear())
            .map_err(|_| store_error());
        tasks?;
        cleared
    }

    async fn poll_qr(&mut self) -> Result<AuthSnapshot> {
        let pending = self.pending_qr.as_ref().ok_or_else(login_error)?;
        let token = pending.token.clone();
        let remember = pending.remember;
        let url = Url::parse_with_params(
            "https://passport.bilibili.com/x/passport-login/web/qrcode/poll",
            &[("qrcode_key", token.as_str())],
        )
        .map_err(|_| transport_error())?;
        let _permit = self
            .connections
            .acquire()
            .await
            .map_err(|_| transport_error())?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| transport_error())?;
        if !response.status().is_success() {
            return Err(transport_error());
        }
        let set_cookies: Vec<String> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|v| {
                v.to_str()
                    .map(str::to_owned)
                    .map_err(|_| CoreError::new(ErrorCode::InvalidInput, "invalid response cookie"))
            })
            .collect::<Result<_>>()?;
        let body: serde_json::Value = response.json().await.map_err(|_| transport_error())?;
        drop(_permit);
        if body.get("code").and_then(|v| v.as_i64()) != Some(0) {
            return Err(login_error());
        }
        let code = body
            .pointer("/data/code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        let mut snap = AuthSnapshot::new();
        match code {
            86101 => {
                snap.qr_status = Some(QrStatus::WaitingForScan);
            }
            86090 => {
                snap.qr_status = Some(QrStatus::WaitingForConfirmation);
            }
            86038 => {
                self.pending_qr = None;
                snap.qr_status = Some(QrStatus::Expired);
                snap.notice = Some("QR login expired; start a new QR login.".into());
            }
            0 => {
                let origin =
                    Url::parse("https://passport.bilibili.com/").map_err(|_| transport_error())?;
                let mut next = CredentialJar::default();
                if !set_cookies.is_empty() {
                    next.import_set_cookies(&origin, &set_cookies)?;
                } else if let Some(cookies) =
                    body.pointer("/data/cookies").and_then(|v| v.as_object())
                {
                    let mut pairs = Vec::new();
                    for (name, value) in cookies {
                        if let Some(v) = value.as_str() {
                            pairs.push(format!("{name}={v}"));
                        }
                    }
                    if !pairs.is_empty() {
                        next = CredentialJar::import_header(&pairs.join("; "))?;
                    }
                }
                *self.jar.write().map_err(|_| store_error())? = next;
                snap = match self.status().await {
                    Ok(snap) => snap,
                    Err(error) => {
                        self.clear_jar();
                        self.pending_qr = None;
                        return Err(error);
                    }
                };
                if !snap.logged_in {
                    self.clear_jar();
                    self.pending_qr = None;
                    return Err(login_error());
                }
                self.pending_qr = None;
                if let Some(fallback) = self.remember_if_requested(remember).await? {
                    snap.notice = Some(store_notice(fallback).into());
                }
                snap.qr_status = Some(QrStatus::Complete);
            }
            _ => {
                self.pending_qr = None;
                snap.qr_status = Some(QrStatus::Expired);
                snap.notice = Some("QR login could not be completed; start a new QR login.".into());
            }
        }
        Ok(snap)
    }

    async fn status(&self) -> Result<AuthSnapshot> {
        if self.jar.read().map_err(|_| store_error())?.is_empty() {
            return Ok(AuthSnapshot::new());
        }
        let url = Url::parse("https://api.bilibili.com/x/web-interface/nav")
            .map_err(|_| transport_error())?;
        let cookie = self.jar.read().map_err(|_| store_error())?.header_for(&url);
        let mut req = self.client.get(url);
        if let Some(value) = cookie {
            req = req.header(reqwest::header::COOKIE, value);
        }
        let _permit = self
            .connections
            .acquire()
            .await
            .map_err(|_| transport_error())?;
        let response = req.send().await.map_err(|_| transport_error())?;
        if !response.status().is_success() {
            return Err(transport_error());
        }
        let body: serde_json::Value = response.json().await.map_err(|_| transport_error())?;
        drop(_permit);
        if body.pointer("/data/v_voucher").is_some() {
            return Err(verification_error());
        }
        match body.get("code").and_then(|value| value.as_i64()) {
            Some(0) => {}
            Some(-101) => return Ok(AuthSnapshot::new()),
            _ => return Err(transport_error()),
        }
        let data = body.get("data").ok_or_else(transport_error)?;
        let logged_in = data
            .get("isLogin")
            .and_then(|x| x.as_bool())
            .ok_or_else(transport_error)?;
        if !logged_in {
            return Ok(AuthSnapshot::new());
        }
        let mid = data
            .get("mid")
            .and_then(|x| x.as_u64())
            .ok_or_else(login_error)?;
        let name = data
            .get("uname")
            .and_then(|x| x.as_str())
            .ok_or_else(transport_error)?
            .to_owned();
        let face = data.get("face").and_then(|x| x.as_str()).map(str::to_owned);
        let mut snap = AuthSnapshot::new();
        snap.logged_in = true;
        snap.account = Some(AuthAccount { mid, name, face });
        Ok(snap)
    }

    async fn load_stored(&mut self, required: bool) -> Result<AuthSnapshot> {
        let StorePolicy::Remember {
            directory,
            allow_plaintext_fallback,
        } = self.policy.clone()
        else {
            return if required {
                Err(store_error())
            } else {
                Ok(AuthSnapshot::new())
            };
        };
        let directory_clone = directory.clone();
        let allow = allow_plaintext_fallback;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.store_tasks.spawn_blocking(move || {
            let result = load_session(&directory_clone, allow).and_then(|loaded| {
                loaded
                    .map(|(jar, fallback)| jar.to_private_json().map(|json| (json, fallback)))
                    .transpose()
            });
            let task_result = result.as_ref().map(|_| ()).map_err(Clone::clone);
            let _ = tx.send(result);
            task_result
        });
        self.join_store_tasks().await?;
        let result = rx.await.map_err(|_| store_error())?;
        match result {
            Ok(Some((serialized, fallback))) => {
                *self.jar.write().map_err(|_| store_error())? =
                    CredentialJar::from_private_json(&serialized)?;
                let mut snap = match self.status().await {
                    Ok(snap) => snap,
                    Err(error) => {
                        self.clear_jar();
                        return Err(error);
                    }
                };
                if !snap.logged_in {
                    self.clear_jar();
                    return Err(login_error());
                }
                if fallback {
                    snap.notice = Some(store_notice(true).into());
                }
                Ok(snap)
            }
            Ok(None) if required => Err(login_error()),
            Ok(None) => Ok(AuthSnapshot::new()),
            Err(error) => Err(error),
        }
    }

    async fn store_current(&mut self) -> Result<bool> {
        let StorePolicy::Remember {
            directory,
            allow_plaintext_fallback,
        } = self.policy.clone()
        else {
            return Err(store_error());
        };
        let serialized = self
            .jar
            .read()
            .map_err(|_| store_error())?
            .to_private_json()?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.store_tasks.spawn_blocking(move || {
            let result = save_session(&directory, allow_plaintext_fallback, &serialized);
            let task_result = result.as_ref().map(|_| ()).map_err(Clone::clone);
            let _ = tx.send(result);
            task_result
        });
        self.join_store_tasks().await?;
        rx.await.map_err(|_| store_error())?
    }
    async fn remember_if_requested(&mut self, remember: bool) -> Result<Option<bool>> {
        if remember {
            Ok(Some(self.store_current().await?))
        } else {
            Ok(None)
        }
    }
    async fn delete_stored(&mut self) -> Result<()> {
        let StorePolicy::Remember {
            directory,
            allow_plaintext_fallback,
        } = self.policy.clone()
        else {
            return Ok(());
        };
        self.store_tasks
            .spawn_blocking(move || delete_session(&directory, allow_plaintext_fallback));
        self.join_store_tasks().await
    }
    async fn join_store_tasks(&mut self) -> Result<()> {
        let mut failure = None;
        while let Some(result) = self.store_tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    if failure.is_none() {
                        failure = Some(error)
                    }
                }
                Err(_) => {
                    if failure.is_none() {
                        failure = Some(store_error())
                    }
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
    fn clear_jar(&self) {
        if let Ok(mut jar) = self.jar.write() {
            jar.clear();
        }
    }
}
fn parse_qr_challenge(data: &serde_json::Value) -> Result<(Url, String)> {
    if data.get("code").and_then(|value| value.as_i64()) != Some(0) {
        return Err(CoreError::new(
            ErrorCode::LoginRequired,
            "QR generation rejected",
        ));
    }
    let login_url = data
        .pointer("/data/url")
        .and_then(|value| value.as_str())
        .ok_or_else(qr_challenge_error)?;
    let token = data
        .pointer("/data/qrcode_key")
        .and_then(|value| value.as_str())
        .ok_or_else(qr_challenge_error)?;
    if login_url.len() > 4096 || token.is_empty() || token.len() > 1024 {
        return Err(qr_challenge_error());
    }
    let login_url = Url::parse(login_url).map_err(|_| qr_challenge_error())?;
    if login_url.scheme() != "https"
        || !matches!(
            login_url.host_str(),
            Some("passport.bilibili.com" | "account.bilibili.com")
        )
        || login_url.port_or_known_default() != Some(443)
        || login_url.fragment().is_some()
        || !login_url.username().is_empty()
        || login_url.password().is_some()
    {
        return Err(qr_challenge_error());
    }
    Ok((login_url, token.to_owned()))
}
fn qr_challenge_error() -> CoreError {
    CoreError::new(ErrorCode::LoginRequired, "invalid QR challenge")
}

fn store_error() -> CoreError {
    CoreError::new(ErrorCode::CredentialStore, "credential store operation")
}
fn login_error() -> CoreError {
    CoreError::new(ErrorCode::LoginRequired, "authentication failed")
}
fn transport_error() -> CoreError {
    CoreError::new(ErrorCode::Transport, "authentication request")
}
fn verification_error() -> CoreError {
    CoreError::new(
        ErrorCode::VerificationRequired,
        "authentication verification required",
    )
}
fn store_notice(plaintext: bool) -> &'static str {
    if plaintext {
        "Credentials saved in a private local file because the secure store was unavailable."
    } else {
        "Credentials saved in the operating-system secure store."
    }
}
fn store_file(dir: &Path) -> PathBuf {
    dir.join("credentials.json")
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(CoreError::io)?;
    crate::cookie::restrict_private(dir, true)
}
#[cfg(windows)]
fn private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(CoreError::io)?;
    crate::cookie::restrict_private(dir, true)
}

fn save_session(dir: &Path, allow_plaintext: bool, serialized: &str) -> Result<bool> {
    let service_account = storage_account(dir);
    if keyring_set(&service_account, serialized).is_ok() {
        remove_store_file(&store_file(dir))?;
        return Ok(false);
    }
    if !allow_plaintext {
        return Err(store_error());
    }
    private_dir(dir)?;
    let path = store_file(dir);
    let tmp = dir.join("credentials.json.new");
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(CoreError::io)?;
        crate::cookie::restrict_private(&tmp, false)?;
        file.write_all(serialized.as_bytes())
            .map_err(CoreError::io)?;
        file.sync_all().map_err(CoreError::io)?;
    }
    replace_store_file(&tmp, &path)?;
    Ok(true)
}
fn load_session(dir: &Path, allow_plaintext: bool) -> Result<Option<(CredentialJar, bool)>> {
    let account = storage_account(dir);
    match keyring_get(&account) {
        Ok(value) => {
            let jar = CredentialJar::from_private_json(&value)?;
            remove_store_file(&store_file(dir))?;
            return Ok(Some((jar, false)));
        }
        Err(KeyringLoad::Missing) => {}
        Err(KeyringLoad::Unavailable) if !allow_plaintext => return Err(store_error()),
        Err(KeyringLoad::Unavailable) => {}
    }
    let path = store_file(dir);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CoreError::io(error)),
    };
    if !allow_plaintext {
        return Err(store_error());
    }
    crate::cookie::restrict_private(dir, true)?;
    if metadata.file_type().is_symlink() {
        return Err(store_error());
    }
    crate::cookie::restrict_private(&path, false)?;
    let value = std::fs::read_to_string(path).map_err(CoreError::io)?;
    CredentialJar::from_private_json(&value).map(|jar| Some((jar, true)))
}
fn delete_session(dir: &Path, _allow_plaintext: bool) -> Result<()> {
    let account = storage_account(dir);
    let key_result = keyring_delete(&account).map_err(|_| store_error());
    let file_result = remove_store_file(&store_file(dir));
    key_result.and(file_result)
}
fn remove_store_file(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            std::fs::remove_file(path).map_err(CoreError::io)
        }
        Ok(_) => Err(store_error()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CoreError::io(error)),
    }
}
#[cfg(unix)]
fn replace_store_file(tmp: &Path, path: &Path) -> Result<()> {
    std::fs::rename(tmp, path).map_err(CoreError::io)?;
    std::fs::File::open(path.parent().ok_or_else(store_error)?)
        .and_then(|directory| directory.sync_all())
        .map_err(CoreError::io)
}
#[cfg(windows)]
fn replace_store_file(tmp: &Path, path: &Path) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    let from: Vec<u16> = OsStr::new(tmp).encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    let replaced = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        return Err(CoreError::new(
            ErrorCode::Filesystem,
            "replace credential file",
        ));
    }
    Ok(())
}
#[cfg(not(any(unix, windows)))]
fn replace_store_file(_: &Path, _: &Path) -> Result<()> {
    Err(CoreError::new(
        ErrorCode::Filesystem,
        "replace credential file",
    ))
}
fn storage_account(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(dir.to_string_lossy().as_bytes()))
}

#[cfg(all(any(windows, target_os = "linux"), not(test)))]
fn keyring_set(account: &str, password: &str) -> std::result::Result<(), ()> {
    keyring::Entry::new("bili_core", account)
        .map_err(|_| ())?
        .set_password(password)
        .map_err(|_| ())
}
#[cfg(all(any(windows, target_os = "linux"), not(test)))]
fn keyring_get(account: &str) -> std::result::Result<String, KeyringLoad> {
    match keyring::Entry::new("bili_core", account)
        .map_err(|_| KeyringLoad::Unavailable)?
        .get_password()
    {
        Ok(value) => Ok(value),
        Err(keyring::Error::NoEntry) => Err(KeyringLoad::Missing),
        Err(_) => Err(KeyringLoad::Unavailable),
    }
}
#[cfg(all(any(windows, target_os = "linux"), not(test)))]
fn keyring_delete(account: &str) -> std::result::Result<(), ()> {
    match keyring::Entry::new("bili_core", account)
        .map_err(|_| ())?
        .delete_credential()
    {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(()),
    }
}
#[cfg(all(not(test), not(any(windows, target_os = "linux"))))]
fn keyring_set(_: &str, _: &str) -> std::result::Result<(), ()> {
    Err(())
}
#[cfg(all(not(test), not(any(windows, target_os = "linux"))))]
fn keyring_get(_: &str) -> std::result::Result<String, KeyringLoad> {
    Err(KeyringLoad::Unavailable)
}
#[cfg(all(not(test), not(any(windows, target_os = "linux"))))]
fn keyring_delete(_: &str) -> std::result::Result<(), ()> {
    Ok(())
}
#[cfg(test)]
fn keyring_set(account: &str, password: &str) -> std::result::Result<(), ()> {
    test_keyring::set(account, password)
}
#[cfg(test)]
fn keyring_get(account: &str) -> std::result::Result<String, KeyringLoad> {
    test_keyring::get(account)
}
#[cfg(test)]
fn keyring_delete(account: &str) -> std::result::Result<(), ()> {
    test_keyring::delete(account)
}
enum KeyringLoad {
    Missing,
    Unavailable,
}
#[cfg(test)]
mod test_keyring {
    use super::KeyringLoad;
    use std::{
        collections::{HashMap, HashSet},
        sync::{Arc, Condvar, LazyLock, Mutex},
    };

    #[derive(Default)]
    struct Backend {
        entries: HashMap<String, String>,
        unavailable: HashSet<String>,
        writes: HashMap<String, u32>,
        gates: HashMap<String, Arc<Gate>>,
    }
    static BACKEND: LazyLock<Mutex<Backend>> = LazyLock::new(|| Mutex::new(Backend::default()));
    fn backend() -> &'static Mutex<Backend> {
        &BACKEND
    }
    #[derive(Default)]
    pub(super) struct Gate {
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }
    impl Gate {
        pub(super) fn wait_started(&self) {
            let mut state = self.state.lock().unwrap();
            while !state.0 {
                state = self.changed.wait(state).unwrap();
            }
        }
        pub(super) fn release(&self) {
            let mut state = self.state.lock().unwrap();
            state.1 = true;
            self.changed.notify_all();
        }
        fn block(&self) {
            let mut state = self.state.lock().unwrap();
            state.0 = true;
            self.changed.notify_all();
            while !state.1 {
                state = self.changed.wait(state).unwrap();
            }
        }
    }
    pub(super) fn set(account: &str, password: &str) -> std::result::Result<(), ()> {
        let (unavailable, gate) = {
            let mut backend = backend().lock().map_err(|_| ())?;
            *backend.writes.entry(account.to_owned()).or_default() += 1;
            (
                backend.unavailable.contains(account),
                backend.gates.get(account).cloned(),
            )
        };
        if let Some(gate) = gate {
            gate.block();
        }
        if unavailable {
            return Err(());
        }
        backend()
            .lock()
            .map_err(|_| ())?
            .entries
            .insert(account.to_owned(), password.to_owned());
        Ok(())
    }
    pub(super) fn get(account: &str) -> std::result::Result<String, KeyringLoad> {
        let backend = backend().lock().map_err(|_| KeyringLoad::Unavailable)?;
        if backend.unavailable.contains(account) {
            return Err(KeyringLoad::Unavailable);
        }
        backend
            .entries
            .get(account)
            .cloned()
            .ok_or(KeyringLoad::Missing)
    }
    pub(super) fn delete(account: &str) -> std::result::Result<(), ()> {
        let mut backend = backend().lock().map_err(|_| ())?;
        if backend.unavailable.contains(account) {
            return Err(());
        }
        backend.entries.remove(account);
        Ok(())
    }
    pub(super) fn unavailable(account: &str, value: bool) {
        let mut backend = backend().lock().unwrap();
        if value {
            backend.unavailable.insert(account.to_owned());
        } else {
            backend.unavailable.remove(account);
        }
    }
    pub(super) fn gate(account: &str, gate: Arc<Gate>) {
        backend()
            .lock()
            .unwrap()
            .gates
            .insert(account.to_owned(), gate);
    }
    pub(super) fn writes(account: &str) -> u32 {
        backend()
            .lock()
            .unwrap()
            .writes
            .get(account)
            .copied()
            .unwrap_or_default()
    }
    pub(super) fn contains(account: &str) -> bool {
        backend().lock().unwrap().entries.contains_key(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, sync::Arc, task::Poll};

    fn manager(policy: StorePolicy, jar: CredentialJar) -> (AuthManager, SharedCredentialJar) {
        let shared = std::sync::Arc::new(std::sync::RwLock::new(jar));
        (
            AuthManager::new(
                Client::new(),
                shared.clone(),
                policy,
                Arc::new(tokio::sync::Semaphore::new(1)),
            ),
            shared,
        )
    }
    #[test]
    fn qr_challenge_parser_accepts_official_hosts_and_rejects_unsafe_urls() {
        let mut response = serde_json::json!({
            "code": 0,
            "data": {
                "url": "https://account.bilibili.com/pc/passport-login/scan?synthetic=1",
                "qrcode_key": "synthetic-qr-key"
            }
        });
        for host in ["account.bilibili.com", "passport.bilibili.com"] {
            response["data"]["url"] =
                format!("https://{host}/pc/passport-login/scan?synthetic=1").into();
            let (url, token) = parse_qr_challenge(&response).unwrap();
            assert_eq!(url.host_str(), Some(host));
            assert_eq!(token, "synthetic-qr-key");
        }

        for url in [
            "https://evilaccount.bilibili.com/scan?synthetic=1",
            "http://account.bilibili.com/scan?synthetic=1",
            "https://user@account.bilibili.com/scan?synthetic=1",
            "https://account.bilibili.com/scan?synthetic=1#fragment",
            "https://account.bilibili.com:444/scan?synthetic=1",
        ] {
            response["data"]["url"] = url.into();
            let error = parse_qr_challenge(&response).unwrap_err();
            assert_eq!(error.code, ErrorCode::LoginRequired);
            let serialized = serde_json::to_string(&error).unwrap();
            assert!(!serialized.contains("synthetic-qr-key"));
            assert!(!serialized.contains("account.bilibili.com"));
            assert!(!serialized.contains("synthetic=1"));
        }
    }

    #[tokio::test]
    async fn remember_off_does_not_call_secure_store() {
        let dir = tempfile::tempdir().unwrap();
        let account = storage_account(dir.path());
        let (mut manager, _) = manager(
            StorePolicy::MemoryOnly,
            CredentialJar::import_header("SESSDATA=synthetic").unwrap(),
        );
        assert_eq!(manager.remember_if_requested(false).await.unwrap(), None);
        assert_eq!(test_keyring::writes(&account), 0);
        assert!(!dir.path().join("credentials.json").exists());
    }

    #[test]
    fn unavailable_secure_store_requires_explicit_noticed_private_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let account = storage_account(dir.path());
        test_keyring::unavailable(&account, true);
        let serialized = CredentialJar::import_header("SESSDATA=synthetic-session")
            .unwrap()
            .to_private_json()
            .unwrap();
        assert!(save_session(dir.path(), false, &serialized).is_err());
        assert!(!store_file(dir.path()).exists());
        assert!(save_session(dir.path(), true, &serialized).unwrap());
        assert!(store_notice(true).contains("private local file"));
        let (loaded, fallback) = load_session(dir.path(), true).unwrap().unwrap();
        assert!(fallback);
        assert_eq!(
            loaded
                .header_for(&Url::parse("https://api.bilibili.com/").unwrap())
                .as_deref(),
            Some("SESSDATA=synthetic-session")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(store_file(dir.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(delete_session(dir.path(), true).is_err());
        assert!(!store_file(dir.path()).exists());
        test_keyring::unavailable(&account, false);
    }

    #[tokio::test]
    async fn logout_clears_memory_and_isolated_secure_store_entry() {
        let dir = tempfile::tempdir().unwrap();
        let account = storage_account(dir.path());
        let jar = CredentialJar::import_header("SESSDATA=synthetic-logout").unwrap();
        assert!(!save_session(dir.path(), false, &jar.to_private_json().unwrap()).unwrap());
        let (mut manager, shared) = manager(
            StorePolicy::Remember {
                directory: dir.path().to_owned(),
                allow_plaintext_fallback: false,
            },
            jar,
        );
        manager.apply(AuthAction::Logout).await.unwrap();
        assert!(shared.read().unwrap().is_empty());
        assert!(!test_keyring::contains(&account));
    }

    #[tokio::test]
    async fn shutdown_joins_store_worker_after_caller_future_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let account = storage_account(dir.path());
        let gate = Arc::new(test_keyring::Gate::default());
        test_keyring::gate(&account, gate.clone());
        test_keyring::unavailable(&account, true);
        let jar = CredentialJar::import_header("SESSDATA=synthetic-join").unwrap();
        let (mut manager, shared) = manager(
            StorePolicy::Remember {
                directory: dir.path().to_owned(),
                allow_plaintext_fallback: false,
            },
            jar,
        );
        let mut store = Box::pin(manager.store_current());
        let mut waiter = tokio::task::spawn_blocking({
            let gate = gate.clone();
            move || gate.wait_started()
        });
        tokio::select! {
            result=&mut store => panic!("store returned before gate release: {result:?}"),
            result=&mut waiter => result.unwrap(),
        }
        drop(store);
        let mut shutdown = Box::pin(manager.shutdown());
        let first = std::future::poll_fn(|cx| Poll::Ready(shutdown.as_mut().poll(cx))).await;
        assert!(first.is_pending());
        gate.release();
        assert!(shutdown.await.is_err());
        assert!(shared.read().unwrap().is_empty());
        assert!(!test_keyring::contains(&account));
        test_keyring::unavailable(&account, false);
    }
}
