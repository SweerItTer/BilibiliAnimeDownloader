//! Session-scoped Bilibili authentication. Secrets never implement `Debug` or `Serialize`.
use crate::{
    cookie::{CredentialJar, SharedCredentialJar, StorePolicy},
    domain::{CoreError, ErrorCode, Result},
};
use reqwest::{header::SET_COOKIE, Client, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinSet;
mod secure_store;

pub enum AuthAction {
    ImportCookie { header: String, remember: bool },
    LoadStored { required: bool },
    Anonymous,
    StartQr { remember: bool, activate: bool },
    PollQr,
    CancelQr,
    Status,
    Logout,
    ListProfiles,
    SelectProfile { id: String },
    ForgetProfile { id: String },
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
    #[serde(default)]
    pub vip: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileValidity {
    Valid,
    Expired,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfile {
    pub id: String,
    pub account: AuthAccount,
    pub remembered: bool,
    pub validity: ProfileValidity,
    pub checked_at: Option<u64>,
    pub expires_at: Option<u64>,
}

#[derive(Clone)]
struct ProfileSession {
    metadata: AuthProfile,
    jar: CredentialJar,
}

// Only this private secure-store payload contains credentials. Never return it
// from an API, implement Debug, or put it in a job checkpoint.
#[derive(Serialize, Deserialize)]
struct StoredProfiles {
    profile_schema: u32,
    selected: Option<String>,
    profiles: Vec<StoredProfile>,
}
#[derive(Serialize, Deserialize)]
struct StoredProfile {
    metadata: AuthProfile,
    credentials: serde_json::Value,
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
    #[serde(default)]
    pub profiles: Vec<AuthProfile>,
    #[serde(default)]
    pub selected_profile: Option<String>,
    /// Receipt for a newly completed QR login; distinct from global selection.
    #[serde(default)]
    pub login_profile: Option<String>,
}
impl AuthSnapshot {
    fn new() -> Self {
        Self {
            logged_in: false,
            account: None,
            notice: None,
            qr: None,
            qr_status: None,
            profiles: Vec::new(),
            selected_profile: None,
            login_profile: None,
        }
    }
}

struct PendingQr {
    token: String,
    remember: bool,
    activate: bool,
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
    profiles: BTreeMap<String, ProfileSession>,
    selected: Option<String>,
    profiles_loaded: bool,
    stored_selected: Option<String>,
    profile_registry: bool,
    store_fallback: bool,
    anonymous_jar: CredentialJar,
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
            profiles: BTreeMap::new(),
            selected: None,
            profiles_loaded: false,
            stored_selected: None,
            profile_registry: false,
            store_fallback: false,
            anonymous_jar: CredentialJar::default(),
        }
    }

    pub(crate) async fn apply(&mut self, action: AuthAction) -> Result<AuthSnapshot> {
        self.join_store_tasks().await?;
        let result = match action {
            AuthAction::ImportCookie { header, remember } => {
                if remember && matches!(self.policy, StorePolicy::MemoryOnly) {
                    return Err(store_error());
                }
                if remember {
                    self.load_profiles().await?;
                }
                let jar = CredentialJar::import_header(&header)?;
                let candidate = Arc::new(RwLock::new(jar.clone()));
                let mut snap = self.status_with(&candidate).await?;
                if !snap.logged_in {
                    return Err(login_error());
                }
                if let (_, Some(fallback)) = self.accept_login(&snap, jar, remember, true).await? {
                    snap.notice = Some(store_notice(fallback).into());
                }
                self.pending_qr = None;
                Ok(snap)
            }
            AuthAction::LoadStored { required } => self.load_stored(required).await,
            AuthAction::Anonymous => {
                self.pending_qr = None;
                self.clear_jar();
                self.selected = None;
                *self.jar.write().map_err(|_| store_error())? = self.anonymous_jar.clone();
                Ok(AuthSnapshot::new())
            }
            AuthAction::StartQr { remember, activate } => {
                if remember && matches!(self.policy, StorePolicy::MemoryOnly) {
                    return Err(store_error());
                }
                if remember {
                    self.load_profiles().await?;
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
                self.pending_qr = Some(PendingQr {
                    token,
                    remember,
                    activate,
                });
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
            AuthAction::Status => {
                let snap = self.status().await?;
                self.refresh_profile(&snap);
                Ok(snap)
            }
            AuthAction::Logout => {
                self.load_profiles().await?;
                let selected = self
                    .selected
                    .clone()
                    .or_else(|| self.stored_selected.clone());
                if let Some(id) = selected {
                    self.forget_profile(&id).await?;
                } else {
                    self.delete_stored().await?;
                }
                self.pending_qr = None;
                self.clear_jar();
                let mut snap = AuthSnapshot::new();
                snap.notice = Some("Logged out; selected credentials removed.".into());
                Ok(snap)
            }
            AuthAction::ListProfiles => {
                self.load_profiles().await?;
                Ok(self.cached_snapshot())
            }
            AuthAction::SelectProfile { id } => {
                self.load_profiles().await?;
                let profile = self.profiles.get(&id).ok_or_else(missing_profile)?;
                let jar = profile.jar.clone();
                let persisted = if self.profiles.values().any(|p| p.metadata.remembered) {
                    let serialized = serialize_profiles(&self.profiles, Some(&id))?;
                    Some(self.save_serialized(serialized).await?)
                } else {
                    None
                };
                *self.jar.write().map_err(|_| store_error())? = jar;
                self.selected = Some(id.clone());
                if let Some(fallback) = persisted {
                    self.profile_registry = true;
                    self.stored_selected = self
                        .profiles
                        .get(&id)
                        .filter(|p| p.metadata.remembered)
                        .map(|_| id);
                    self.store_fallback = fallback;
                }
                Ok(self.cached_snapshot())
            }
            AuthAction::ForgetProfile { id } => {
                self.load_profiles().await?;
                self.forget_profile(&id).await?;
                Ok(self.cached_snapshot())
            }
        };
        result.map(|mut snap| {
            snap.profiles = self
                .profiles
                .values()
                .map(|p| {
                    let mut metadata = p.metadata.clone();
                    if metadata
                        .expires_at
                        .is_some_and(|expires| expires <= now_ms())
                    {
                        metadata.validity = ProfileValidity::Expired;
                    }
                    metadata
                })
                .collect();
            snap.selected_profile = self.selected.clone();
            snap
        })
    }

    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        let tasks = self.join_store_tasks().await;
        self.pending_qr = None;
        self.profiles.clear();
        self.selected = None;
        self.anonymous_jar.clear();
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
        let activate = pending.activate;
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
                let candidate = Arc::new(RwLock::new(next.clone()));
                snap = self.status_with(&candidate).await?;
                if !snap.logged_in {
                    return Err(login_error());
                }
                snap = self.complete_qr(snap, next, remember, activate).await?;
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
        self.status_with(&self.jar).await
    }

    async fn complete_qr(
        &mut self,
        mut snapshot: AuthSnapshot,
        jar: CredentialJar,
        remembered: bool,
        activate: bool,
    ) -> Result<AuthSnapshot> {
        let (profile, fallback) = self
            .accept_login(&snapshot, jar, remembered, activate)
            .await?;
        if let Some(fallback) = fallback {
            snapshot.notice = Some(store_notice(fallback).into());
        }
        snapshot.login_profile = Some(profile);
        snapshot.qr_status = Some(QrStatus::Complete);
        self.pending_qr = None;
        Ok(snapshot)
    }

    async fn status_with(&self, credentials: &SharedCredentialJar) -> Result<AuthSnapshot> {
        if credentials.read().map_err(|_| store_error())?.is_empty() {
            return Ok(AuthSnapshot::new());
        }
        let url = Url::parse("https://api.bilibili.com/x/web-interface/nav")
            .map_err(|_| transport_error())?;
        let cookie = credentials
            .read()
            .map_err(|_| store_error())?
            .header_for(&url);
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
        let vip = data
            .pointer("/vip/status")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("vipStatus").and_then(|v| v.as_u64()))
            .map(|v| v == 1);
        snap.account = Some(AuthAccount {
            mid,
            name,
            face,
            vip,
        });
        Ok(snap)
    }

    async fn load_stored(&mut self, required: bool) -> Result<AuthSnapshot> {
        self.load_profiles().await?;
        if let Some(id) = self.stored_selected.clone() {
            let jar = self
                .profiles
                .get(&id)
                .ok_or_else(missing_profile)?
                .jar
                .clone();
            let candidate = Arc::new(RwLock::new(jar.clone()));
            let mut snap = self.status_with(&candidate).await?;
            if !snap.logged_in {
                if let Some(profile) = self.profiles.get_mut(&id) {
                    profile.metadata.checked_at = Some(now_ms());
                    profile.metadata.validity = ProfileValidity::Expired;
                }
                return Err(login_error());
            }
            *self.jar.write().map_err(|_| store_error())? = jar;
            self.selected = Some(id);
            self.refresh_profile(&snap);
            if self.store_fallback {
                snap.notice = Some(store_notice(true).into());
            }
            return Ok(snap);
        }
        if self.profile_registry {
            return if required {
                Err(login_error())
            } else {
                Ok(AuthSnapshot::new())
            };
        }
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
                let jar = CredentialJar::from_private_json(&serialized)?;
                let candidate = Arc::new(RwLock::new(jar.clone()));
                let mut snap = self.status_with(&candidate).await?;
                if !snap.logged_in {
                    return Err(login_error());
                }
                // The old single-session record was already explicitly saved.
                // Capture a stable opaque profile ID in that same secure store
                // before a caller can admit jobs referring to it.
                self.accept_login(&snap, jar, true, true).await?;
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

    async fn save_serialized(&mut self, serialized: String) -> Result<bool> {
        let StorePolicy::Remember {
            directory,
            allow_plaintext_fallback,
        } = self.policy.clone()
        else {
            return Err(store_error());
        };
        if serialized.len() > 8 * 1024 * 1024 {
            return Err(store_error());
        }
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
    #[cfg(test)]
    async fn store_current(&mut self) -> Result<bool> {
        let serialized = if self.profiles.is_empty()
            && self.selected.is_none()
            && !self.jar.read().map_err(|_| store_error())?.is_empty()
        {
            self.jar
                .read()
                .map_err(|_| store_error())?
                .to_private_json()?
        } else {
            serialize_profiles(&self.profiles, self.selected.as_deref())?
        };
        let fallback = self.save_serialized(serialized).await?;
        self.profile_registry = true;
        self.stored_selected = self
            .selected
            .as_ref()
            .filter(|id| {
                self.profiles
                    .get(*id)
                    .is_some_and(|p| p.metadata.remembered)
            })
            .cloned();
        Ok(fallback)
    }
    #[cfg(test)]
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
    fn clear_jar(&mut self) {
        self.selected = None;
        if let Ok(mut jar) = self.jar.write() {
            jar.clear();
        }
    }

    pub(crate) fn selected_profile_id(&self) -> Option<String> {
        self.selected.clone()
    }

    pub(crate) fn session(&self, profile_id: Option<&str>) -> Result<SharedCredentialJar> {
        let jar = match profile_id {
            None => self.anonymous_jar.clone(),
            Some(id) => {
                let profile = self.profiles.get(id).ok_or_else(missing_profile)?;
                if profile.metadata.validity == ProfileValidity::Expired || profile.jar.is_empty() {
                    return Err(login_error());
                }
                if profile
                    .metadata
                    .expires_at
                    .is_some_and(|expires| expires <= now_ms())
                {
                    return Err(login_error());
                }
                profile.jar.clone()
            }
        };
        Ok(Arc::new(RwLock::new(jar)))
    }

    pub(crate) fn apply_verification(
        &mut self,
        profile_id: Option<&str>,
        vtoken: &str,
    ) -> Result<()> {
        if vtoken.is_empty()
            || vtoken.len() > 4096
            || !vtoken
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.|+/=".contains(&b))
        {
            return Err(CoreError::new(
                ErrorCode::InvalidInput,
                "invalid verification result",
            ));
        }
        let jar = match profile_id {
            Some(id) => &mut self.profiles.get_mut(id).ok_or_else(missing_profile)?.jar,
            None => &mut self.anonymous_jar,
        };
        let origin = Url::parse("https://api.bilibili.com/").map_err(|_| transport_error())?;
        jar.import_set_cookies(
            &origin,
            &[format!(
                "x-bili-gaia-vtoken={vtoken}; Path=/; Secure; Max-Age=600"
            )],
        )?;
        if self.selected.as_deref() == profile_id {
            *self.jar.write().map_err(|_| store_error())? = jar.clone();
        }
        Ok(())
    }

    fn cached_snapshot(&self) -> AuthSnapshot {
        let mut snapshot = AuthSnapshot::new();
        if self.store_fallback {
            snapshot.notice = Some(store_notice(true).into());
        }
        if let Some(profile) = self.selected.as_ref().and_then(|id| self.profiles.get(id)) {
            snapshot.logged_in = profile.metadata.validity == ProfileValidity::Valid
                && !profile
                    .metadata
                    .expires_at
                    .is_some_and(|expires| expires <= now_ms());
            snapshot.account = Some(profile.metadata.account.clone());
        }
        snapshot
    }

    fn prepare_profile(
        &self,
        snapshot: &AuthSnapshot,
        remembered: bool,
        jar: CredentialJar,
        activate: bool,
    ) -> Result<(String, ProfileSession)> {
        let account = snapshot.account.clone().ok_or_else(login_error)?;
        let id = if remembered && activate {
            self.selected
                .as_ref()
                .and_then(|id| self.profiles.get(id))
                .filter(|p| p.metadata.account.mid == account.mid)
                .map(|p| p.metadata.id.clone())
                .or_else(|| {
                    self.profiles
                        .values()
                        .find(|p| p.metadata.remembered && p.metadata.account.mid == account.mid)
                        .map(|p| p.metadata.id.clone())
                })
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        if self.profiles.len() >= 64 && !self.profiles.contains_key(&id) {
            return Err(CoreError::new(
                ErrorCode::Busy,
                "credential profile limit reached",
            ));
        }
        let metadata = AuthProfile {
            id: id.clone(),
            account,
            remembered,
            validity: ProfileValidity::Valid,
            checked_at: Some(now_ms()),
            expires_at: jar.session_expiry_unix_ms(),
        };
        Ok((id, ProfileSession { metadata, jar }))
    }

    async fn accept_login(
        &mut self,
        snapshot: &AuthSnapshot,
        jar: CredentialJar,
        remembered: bool,
        activate: bool,
    ) -> Result<(String, Option<bool>)> {
        let (id, profile) = self.prepare_profile(snapshot, remembered, jar.clone(), activate)?;
        let mut profiles = self.profiles.clone();
        profiles.insert(id.clone(), profile);
        // Validation and opt-in storage finish before changing the active
        // identity. Dropping this future cannot install half of an account.
        let persisted = if remembered {
            let selected = if activate {
                Some(id.as_str())
            } else {
                self.stored_selected.as_deref()
            };
            Some(
                self.save_serialized(serialize_profiles(&profiles, selected)?)
                    .await?,
            )
        } else {
            None
        };
        if activate {
            *self.jar.write().map_err(|_| store_error())? = jar;
            self.selected = Some(id.clone());
        }
        self.profiles = profiles;
        if let Some(fallback) = persisted {
            self.profile_registry = true;
            if activate {
                self.stored_selected = Some(id.clone());
            }
            self.store_fallback = fallback;
        }
        Ok((id, persisted))
    }

    async fn forget_profile(&mut self, id: &str) -> Result<()> {
        let mut profiles = self.profiles.clone();
        let removed = profiles.remove(id).ok_or_else(missing_profile)?;
        let selected = self.selected.clone().filter(|selected| selected != id);
        let persisted = if removed.metadata.remembered {
            Some(
                self.save_serialized(serialize_profiles(&profiles, selected.as_deref())?)
                    .await?,
            )
        } else {
            None
        };
        if self.selected.as_deref() == Some(id) {
            *self.jar.write().map_err(|_| store_error())? = self.anonymous_jar.clone();
        }
        self.profiles = profiles;
        self.selected = selected;
        if let Some(fallback) = persisted {
            self.profile_registry = true;
            self.stored_selected = self
                .selected
                .as_ref()
                .filter(|id| {
                    self.profiles
                        .get(*id)
                        .is_some_and(|p| p.metadata.remembered)
                })
                .cloned();
            self.store_fallback = fallback;
        }
        Ok(())
    }

    #[cfg(test)]
    fn install_profile(&mut self, snapshot: &AuthSnapshot, remembered: bool) -> Result<()> {
        let jar = self.jar.read().map_err(|_| store_error())?.clone();
        let (id, profile) = self.prepare_profile(snapshot, remembered, jar, true)?;
        self.profiles.insert(id.clone(), profile);
        self.selected = Some(id);
        Ok(())
    }

    fn refresh_profile(&mut self, snapshot: &AuthSnapshot) {
        if let Some(profile) = self
            .selected
            .as_ref()
            .and_then(|id| self.profiles.get_mut(id))
        {
            profile.metadata.checked_at = Some(now_ms());
            profile.metadata.validity = if snapshot.logged_in {
                ProfileValidity::Valid
            } else {
                ProfileValidity::Expired
            };
            if let Some(account) = &snapshot.account {
                profile.metadata.account = account.clone();
            }
        }
    }

    async fn load_profiles(&mut self) -> Result<()> {
        if self.profiles_loaded {
            return Ok(());
        }
        let StorePolicy::Remember {
            directory,
            allow_plaintext_fallback,
        } = self.policy.clone()
        else {
            self.profiles_loaded = true;
            return Ok(());
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.store_tasks.spawn_blocking(move || {
            let result = load_secret(&directory, allow_plaintext_fallback);
            let task_result = result.as_ref().map(|_| ()).map_err(Clone::clone);
            let _ = tx.send(result);
            task_result
        });
        self.join_store_tasks().await?;
        if let Some((payload, fallback)) = rx.await.map_err(|_| store_error())?? {
            self.store_fallback = fallback;
            let value: serde_json::Value =
                serde_json::from_str(&payload).map_err(|_| store_error())?;
            if value.get("profile_schema").is_some() {
                self.profile_registry = true;
                let stored: StoredProfiles =
                    serde_json::from_value(value).map_err(|_| store_error())?;
                if stored.profile_schema != 1 || stored.profiles.len() > 64 {
                    return Err(store_error());
                }
                self.stored_selected = stored.selected;
                for entry in stored.profiles {
                    if !entry.metadata.remembered
                        || uuid::Uuid::parse_str(&entry.metadata.id).is_err()
                    {
                        return Err(store_error());
                    }
                    let jar = CredentialJar::from_private_json(&entry.credentials.to_string())?;
                    self.profiles
                        .entry(entry.metadata.id.clone())
                        .or_insert(ProfileSession {
                            metadata: entry.metadata,
                            jar,
                        });
                }
            }
        }
        self.profiles_loaded = true;
        Ok(())
    }
}
fn serialize_profiles(
    profiles: &BTreeMap<String, ProfileSession>,
    selected: Option<&str>,
) -> Result<String> {
    let profiles = profiles
        .values()
        .filter(|p| p.metadata.remembered)
        .map(|p| {
            Ok(StoredProfile {
                metadata: p.metadata.clone(),
                credentials: serde_json::from_str(&p.jar.to_private_json()?)
                    .map_err(|_| store_error())?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let selected = selected
        .filter(|id| profiles.iter().any(|p| p.metadata.id == *id))
        .map(str::to_owned);
    serde_json::to_string(&StoredProfiles {
        profile_schema: 1,
        selected,
        profiles,
    })
    .map_err(|_| store_error())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
fn missing_profile() -> CoreError {
    CoreError::new(
        ErrorCode::LoginRequired,
        "selected credential profile unavailable",
    )
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
    match secure_store::set(dir, serialized) {
        Ok(()) => {
            remove_store_file(&store_file(dir))?;
            return Ok(false);
        }
        Err(secure_store::SaveError::Failed) => return Err(store_error()),
        Err(secure_store::SaveError::Unavailable) => {}
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
    load_secret(dir, allow_plaintext)?
        .map(|(value, fallback)| {
            CredentialJar::from_private_json(&value).map(|jar| (jar, fallback))
        })
        .transpose()
}
fn load_secret(dir: &Path, allow_plaintext: bool) -> Result<Option<(String, bool)>> {
    match secure_store::get(dir) {
        Ok(value) => {
            if value.len() > 8 * 1024 * 1024 {
                return Err(store_error());
            }
            remove_store_file(&store_file(dir))?;
            return Ok(Some((value, false)));
        }
        Err(KeyringLoad::Missing) => {}
        Err(KeyringLoad::Invalid) => return Err(store_error()),
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
    if value.len() > 8 * 1024 * 1024 {
        return Err(store_error());
    }
    Ok(Some((value, true)))
}
fn delete_session(dir: &Path, _allow_plaintext: bool) -> Result<()> {
    let key_result = secure_store::delete(dir).map_err(|_| store_error());
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
#[derive(Debug)]
enum KeyringLoad {
    Missing,
    Unavailable,
    Invalid,
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
        chunk_failures: HashMap<String, u32>,
        delete_failures: HashSet<String>,
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
    pub(super) struct ReleaseGate(Arc<Gate>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    impl Gate {
        pub(super) fn release_on_drop(self: &Arc<Self>) -> ReleaseGate {
            ReleaseGate(self.clone())
        }
        pub(super) fn wait_started(&self) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut state = self.state.lock().unwrap();
            while !state.0 {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                state = self.changed.wait_timeout(state, remaining).unwrap().0;
            }
            true
        }
        pub(super) fn release(&self) {
            let mut state = self.state.lock().unwrap();
            state.1 = true;
            self.changed.notify_all();
        }
        fn block(&self) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut state = self.state.lock().unwrap();
            state.0 = true;
            self.changed.notify_all();
            while !state.1 {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                state = self.changed.wait_timeout(state, remaining).unwrap().0;
            }
            true
        }
    }
    pub(super) fn set(account: &str, password: &str) -> std::result::Result<(), ()> {
        // Match the pinned native Windows password blob bound even on Linux.
        if password.encode_utf16().count() * 2 > 2560 {
            return Err(());
        }
        let gate = {
            let mut backend = backend().lock().map_err(|_| ())?;
            *backend.writes.entry(account.to_owned()).or_default() += 1;
            if let Some((prefix, _)) = account.split_once("/chunks/") {
                if !account.ends_with("/pending") {
                    if let Some(remaining) = backend.chunk_failures.get_mut(prefix) {
                        if *remaining == 0 {
                            return Err(());
                        }
                        *remaining -= 1;
                    }
                }
            }
            backend.gates.get(account).cloned()
        };
        if let Some(gate) = gate {
            if !gate.block() {
                return Err(());
            }
        }
        let mut backend = backend().lock().map_err(|_| ())?;
        if backend.unavailable.contains(account) {
            return Err(());
        }
        backend
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
        if backend.delete_failures.contains(account) {
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
    pub(super) fn fail_chunks_after(account: &str, remaining: Option<u32>) {
        let mut backend = backend().lock().unwrap();
        if let Some(remaining) = remaining {
            backend.chunk_failures.insert(account.to_owned(), remaining);
        } else {
            backend.chunk_failures.remove(account);
        }
    }
    pub(super) fn fail_delete(account: &str, failed: bool) {
        let mut backend = backend().lock().unwrap();
        if failed {
            backend.delete_failures.insert(account.to_owned());
        } else {
            backend.delete_failures.remove(account);
        }
    }
    pub(super) fn owned_entries(account: &str) -> Vec<(String, String)> {
        let prefix = format!("{account}/chunks/");
        backend()
            .lock()
            .unwrap()
            .entries
            .iter()
            .filter(|(key, _)| key.as_str() == account || key.starts_with(&prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
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
    fn account_snapshot(mid: u64) -> AuthSnapshot {
        let mut snap = AuthSnapshot::new();
        snap.logged_in = true;
        snap.account = Some(AuthAccount {
            mid,
            name: format!("Fixture {mid}"),
            face: None,
            vip: None,
        });
        snap
    }

    #[tokio::test]
    async fn profile_capacity_failure_preserves_the_previous_active_identity() {
        let original = CredentialJar::import_header("SESSDATA=synthetic-original-session").unwrap();
        let (mut manager, active) = manager(StorePolicy::MemoryOnly, original);
        for mid in 1..=64 {
            manager
                .install_profile(&account_snapshot(mid), false)
                .unwrap();
        }
        let selected = manager.selected_profile_id();
        let origin = Url::parse("https://api.bilibili.com/").unwrap();
        let before = active.read().unwrap().header_for(&origin);
        let candidate =
            CredentialJar::import_header("SESSDATA=synthetic-rejected-session").unwrap();
        let error = manager
            .accept_login(&account_snapshot(999), candidate, false, true)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Busy);
        assert_eq!(manager.selected_profile_id(), selected);
        assert_eq!(manager.profiles.len(), 64);
        assert_eq!(active.read().unwrap().header_for(&origin), before);
        assert_eq!(
            manager
                .session(selected.as_deref())
                .unwrap()
                .read()
                .unwrap()
                .header_for(&origin),
            before
        );
    }

    #[tokio::test]
    async fn failed_remember_does_not_install_a_candidate_or_change_active_credentials() {
        let root = tempfile::tempdir().unwrap();
        let account = storage_account(root.path());
        test_keyring::unavailable(&account, true);
        let original = CredentialJar::import_header("SESSDATA=synthetic-original-session").unwrap();
        let (mut manager, active) = manager(
            StorePolicy::Remember {
                directory: root.path().to_owned(),
                allow_plaintext_fallback: false,
            },
            original,
        );
        manager
            .install_profile(&account_snapshot(1), false)
            .unwrap();
        let selected = manager.selected_profile_id();
        let origin = Url::parse("https://api.bilibili.com/").unwrap();
        let before = active.read().unwrap().header_for(&origin);
        let candidate =
            CredentialJar::import_header("SESSDATA=synthetic-uncommitted-session").unwrap();
        let error = manager
            .accept_login(&account_snapshot(2), candidate, true, true)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::CredentialStore);
        assert_eq!(manager.selected_profile_id(), selected);
        assert_eq!(manager.profiles.len(), 1);
        assert_eq!(active.read().unwrap().header_for(&origin), before);
        assert!(!test_keyring::contains(&account));
        assert!(!store_file(root.path()).exists());
        test_keyring::unavailable(&account, false);
    }

    #[tokio::test]
    async fn task_only_qr_addition_preserves_global_identity_and_stored_selection() {
        for remembered in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let policy = StorePolicy::Remember {
                directory: dir.path().to_owned(),
                allow_plaintext_fallback: false,
            };
            let original =
                CredentialJar::import_header("SESSDATA=synthetic-global-session").unwrap();
            let (mut auth, shared) = manager(policy.clone(), CredentialJar::default());
            let (global, _) = auth
                .accept_login(&account_snapshot(1), original, true, true)
                .await
                .unwrap();
            let mut candidate = account_snapshot(1);
            candidate.account.as_mut().unwrap().name = "Task-only credentials".into();
            let jar = CredentialJar::import_header("SESSDATA=synthetic-task-session").unwrap();
            let receipt = auth
                .complete_qr(candidate, jar, remembered, false)
                .await
                .unwrap();
            let added = receipt.login_profile.unwrap();
            assert_ne!(added,global,"adding credentials for the same MID must not overwrite the globally selected profile");
            assert!(receipt.logged_in);
            assert_eq!(receipt.qr_status, Some(QrStatus::Complete));
            assert_eq!(receipt.account.unwrap().name, "Task-only credentials");
            let snapshot = auth.apply(AuthAction::ListProfiles).await.unwrap();
            assert_eq!(snapshot.selected_profile.as_deref(), Some(global.as_str()));
            assert!(snapshot.login_profile.is_none());
            assert_eq!(snapshot.account.unwrap().name, "Fixture 1");
            assert_eq!(auth.stored_selected.as_deref(), Some(global.as_str()));
            let origin = Url::parse("https://api.bilibili.com/").unwrap();
            assert_eq!(
                shared.read().unwrap().header_for(&origin).as_deref(),
                Some("SESSDATA=synthetic-global-session")
            );
            assert_eq!(
                auth.session(Some(&global))
                    .unwrap()
                    .read()
                    .unwrap()
                    .header_for(&origin)
                    .as_deref(),
                Some("SESSDATA=synthetic-global-session")
            );
            assert_eq!(
                auth.session(Some(&added))
                    .unwrap()
                    .read()
                    .unwrap()
                    .header_for(&origin)
                    .as_deref(),
                Some("SESSDATA=synthetic-task-session")
            );
            let stored: StoredProfiles =
                serde_json::from_str(&secure_store::get(dir.path()).unwrap()).unwrap();
            assert_eq!(stored.selected.as_deref(), Some(global.as_str()));
            assert_eq!(stored.profiles.len(), if remembered { 2 } else { 1 });
            auth.pending_qr = Some(PendingQr {
                token: "synthetic-pending-qr".into(),
                remember: remembered,
                activate: false,
            });
            let cancelled = auth.apply(AuthAction::CancelQr).await.unwrap();
            assert_eq!(cancelled.qr_status, Some(QrStatus::Cancelled));
            assert!(cancelled.login_profile.is_none());
            assert_eq!(auth.selected_profile_id().as_deref(), Some(global.as_str()));
            assert_eq!(
                shared.read().unwrap().header_for(&origin).as_deref(),
                Some("SESSDATA=synthetic-global-session")
            );
            secure_store::delete(dir.path()).unwrap();
        }
    }

    #[tokio::test]
    async fn task_only_remembered_addition_does_not_auto_select_an_account_on_restore() {
        let dir = tempfile::tempdir().unwrap();
        let policy = StorePolicy::Remember {
            directory: dir.path().to_owned(),
            allow_plaintext_fallback: false,
        };
        let (mut auth, shared) = manager(policy.clone(), CredentialJar::default());
        let receipt = auth
            .complete_qr(
                account_snapshot(9),
                CredentialJar::import_header("SESSDATA=synthetic-task-only").unwrap(),
                true,
                false,
            )
            .await
            .unwrap();
        assert!(receipt.login_profile.is_some());
        assert!(auth.selected_profile_id().is_none());
        assert!(auth.stored_selected.is_none());
        assert!(shared.read().unwrap().is_empty());
        let (mut reopened, _) = manager(policy, CredentialJar::default());
        let restored = reopened
            .apply(AuthAction::LoadStored { required: false })
            .await
            .unwrap();
        assert!(!restored.logged_in);
        assert!(restored.selected_profile.is_none());
        assert_eq!(restored.profiles.len(), 1);
        secure_store::delete(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn profile_sessions_are_frozen_and_public_metadata_has_no_credentials() {
        let first = CredentialJar::import_header("SESSDATA=synthetic-first-profile").unwrap();
        let (mut manager, shared) = manager(StorePolicy::MemoryOnly, first);
        manager
            .install_profile(&account_snapshot(1), false)
            .unwrap();
        let first_id = manager.selected_profile_id().unwrap();
        let captured = manager.session(Some(&first_id)).unwrap();
        *shared.write().unwrap() =
            CredentialJar::import_header("SESSDATA=synthetic-second-profile").unwrap();
        manager
            .install_profile(&account_snapshot(2), false)
            .unwrap();
        let second_id = manager.selected_profile_id().unwrap();
        manager.apply(AuthAction::Anonymous).await.unwrap();
        let snapshot = manager.apply(AuthAction::ListProfiles).await.unwrap();
        assert_eq!(snapshot.profiles.len(), 2);
        assert!(snapshot.selected_profile.is_none());
        let url = Url::parse("https://api.bilibili.com/").unwrap();
        assert_eq!(
            captured.read().unwrap().header_for(&url).as_deref(),
            Some("SESSDATA=synthetic-first-profile")
        );
        assert_eq!(
            manager
                .session(Some(&second_id))
                .unwrap()
                .read()
                .unwrap()
                .header_for(&url)
                .as_deref(),
            Some("SESSDATA=synthetic-second-profile")
        );
        assert!(manager.session(None).unwrap().read().unwrap().is_empty());
        let public = serde_json::to_string(&snapshot).unwrap();
        assert!(!public.contains("synthetic-first-profile"));
        assert!(!public.contains("synthetic-second-profile"));
        assert!(snapshot
            .profiles
            .iter()
            .all(|p| p.expires_at.is_none() && !p.remembered));
        assert!(manager.session(Some("missing")).is_err());
    }

    #[tokio::test]
    async fn remembering_the_active_account_preserves_task_profile_identity_across_restart() {
        let root = tempfile::tempdir().unwrap();
        let policy = StorePolicy::Remember {
            directory: root.path().to_owned(),
            allow_plaintext_fallback: false,
        };
        let initial = CredentialJar::import_header("SESSDATA=synthetic-session-only").unwrap();
        let (mut auth, _) = manager(policy.clone(), initial);
        auth.install_profile(&account_snapshot(42), false).unwrap();
        let id = auth.selected_profile_id().unwrap();
        let captured = auth.session(Some(&id)).unwrap();
        let next = CredentialJar::import_header("SESSDATA=synthetic-remembered-upgrade").unwrap();
        let (remembered_id, _) = auth
            .accept_login(&account_snapshot(42), next, true, true)
            .await
            .unwrap();
        assert_eq!(remembered_id, id);
        assert_eq!(auth.profiles.len(), 1);
        let (mut reopened, _) = manager(policy, CredentialJar::default());
        reopened.load_profiles().await.unwrap();
        let origin = Url::parse("https://api.bilibili.com/").unwrap();
        assert_eq!(reopened.stored_selected.as_deref(), Some(id.as_str()));
        assert!(reopened.profiles[&id].metadata.remembered);
        assert_eq!(
            reopened
                .session(Some(&id))
                .unwrap()
                .read()
                .unwrap()
                .cookie_value_for("SESSDATA", &origin)
                .as_deref(),
            Some("synthetic-remembered-upgrade")
        );
        assert_eq!(
            captured
                .read()
                .unwrap()
                .cookie_value_for("SESSDATA", &origin)
                .as_deref(),
            Some("synthetic-session-only")
        );
        secure_store::delete(root.path()).unwrap();
    }

    #[tokio::test]
    async fn remembered_profile_collection_roundtrips_and_forget_preserves_other_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let policy = StorePolicy::Remember {
            directory: dir.path().to_owned(),
            allow_plaintext_fallback: false,
        };
        let (mut original, shared) = manager(
            policy.clone(),
            CredentialJar::import_header("SESSDATA=synthetic-remembered-a").unwrap(),
        );
        original
            .install_profile(&account_snapshot(10), true)
            .unwrap();
        let first = original.selected_profile_id().unwrap();
        *shared.write().unwrap() =
            CredentialJar::import_header("SESSDATA=synthetic-remembered-b").unwrap();
        original
            .install_profile(&account_snapshot(20), true)
            .unwrap();
        let second = original.selected_profile_id().unwrap();
        original.store_current().await.unwrap();
        // No source service is called by listing: validity and check time are
        // cached metadata, not a new claim of an authenticated response.
        let (mut reopened, _) = manager(policy.clone(), CredentialJar::default());
        let listed = reopened.apply(AuthAction::ListProfiles).await.unwrap();
        assert_eq!(listed.profiles.len(), 2);
        assert!(listed.selected_profile.is_none());
        reopened
            .apply(AuthAction::ForgetProfile { id: first.clone() })
            .await
            .unwrap();
        let (mut again, _) = manager(policy, CredentialJar::default());
        let listed = again.apply(AuthAction::ListProfiles).await.unwrap();
        assert_eq!(listed.profiles.len(), 1);
        assert_eq!(listed.profiles[0].id, second);
        assert!(again.session(Some(&first)).is_err());
        assert!(!again
            .session(Some(&second))
            .unwrap()
            .read()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn realistic_unicode_profile_collections_fit_native_secure_blob_limits() {
        let dir = tempfile::tempdir().unwrap();
        let policy = StorePolicy::Remember {
            directory: dir.path().to_owned(),
            allow_plaintext_fallback: false,
        };
        let (mut auth, _) = manager(policy.clone(), CredentialJar::default());
        for mid in 1..=3 {
            let header = format!(
                "SESSDATA={}; bili_jct={}; DedeUserID={mid}",
                "synthetic-token".repeat(24),
                "c".repeat(32)
            );
            let jar = CredentialJar::import_header(&header).unwrap();
            let mut snapshot = account_snapshot(mid);
            snapshot.account.as_mut().unwrap().name = format!("测试账号 {mid} · 中文资料");
            auth.accept_login(&snapshot, jar, true, true).await.unwrap();
        }
        let expected = auth
            .profiles
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let account = storage_account(dir.path());
        let entries = test_keyring::owned_entries(&account);
        assert!(entries.len() > 3);
        assert!(entries
            .iter()
            .all(|(_, value)| value.encode_utf16().count() * 2 <= 2560));
        assert!(!store_file(dir.path()).exists());
        let (mut loaded, _) = manager(policy, CredentialJar::default());
        let snapshot = loaded.apply(AuthAction::ListProfiles).await.unwrap();
        assert_eq!(
            snapshot
                .profiles
                .iter()
                .map(|p| p.id.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        assert!(snapshot
            .profiles
            .iter()
            .all(|p| p.account.name.contains("中文资料") && p.remembered));
        for id in expected {
            loaded
                .apply(AuthAction::ForgetProfile { id })
                .await
                .unwrap();
        }
        secure_store::delete(dir.path()).unwrap();
        assert!(test_keyring::owned_entries(&account).is_empty());
    }

    #[tokio::test]
    async fn memory_login_never_replaces_a_remembered_secret() {
        let dir = tempfile::tempdir().unwrap();
        let policy = StorePolicy::Remember {
            directory: dir.path().to_owned(),
            allow_plaintext_fallback: false,
        };
        let (mut manager, shared) = manager(
            policy,
            CredentialJar::import_header("SESSDATA=synthetic-durable").unwrap(),
        );
        manager
            .install_profile(&account_snapshot(10), true)
            .unwrap();
        manager.store_current().await.unwrap();
        let writes = test_keyring::writes(&storage_account(dir.path()));
        *shared.write().unwrap() =
            CredentialJar::import_header("SESSDATA=synthetic-memory").unwrap();
        manager
            .install_profile(&account_snapshot(10), false)
            .unwrap();
        manager.remember_if_requested(false).await.unwrap();
        assert_eq!(test_keyring::writes(&storage_account(dir.path())), writes);
        manager.store_current().await.unwrap();
        let stored = secure_store::get(dir.path()).ok().unwrap();
        assert!(stored.contains("synthetic-durable"));
        assert!(!stored.contains("synthetic-memory"));
    }

    #[tokio::test]
    async fn verification_is_profile_scoped_and_excluded_from_storage() {
        let (mut manager, shared) = manager(
            StorePolicy::MemoryOnly,
            CredentialJar::import_header("SESSDATA=synthetic-a").unwrap(),
        );
        manager
            .install_profile(&account_snapshot(1), false)
            .unwrap();
        let first = manager.selected_profile_id().unwrap();
        *shared.write().unwrap() = CredentialJar::import_header("SESSDATA=synthetic-b").unwrap();
        manager
            .install_profile(&account_snapshot(2), false)
            .unwrap();
        let second = manager.selected_profile_id().unwrap();
        manager
            .apply_verification(Some(&first), "synthetic-proof-a")
            .unwrap();
        let url = Url::parse("https://api.bilibili.com/").unwrap();
        let first_jar = manager.session(Some(&first)).unwrap();
        assert_eq!(
            first_jar
                .read()
                .unwrap()
                .cookie_value_for("x-bili-gaia-vtoken", &url)
                .as_deref(),
            Some("synthetic-proof-a")
        );
        assert!(!first_jar
            .read()
            .unwrap()
            .to_private_json()
            .unwrap()
            .contains("synthetic-proof-a"));
        assert!(manager
            .session(Some(&second))
            .unwrap()
            .read()
            .unwrap()
            .cookie_value_for("x-bili-gaia-vtoken", &url)
            .is_none());
        assert!(shared
            .read()
            .unwrap()
            .cookie_value_for("x-bili-gaia-vtoken", &url)
            .is_none());
        manager
            .apply_verification(None, "synthetic-anon-proof")
            .unwrap();
        manager.apply(AuthAction::Anonymous).await.unwrap();
        assert_eq!(
            manager
                .session(None)
                .unwrap()
                .read()
                .unwrap()
                .cookie_value_for("x-bili-gaia-vtoken", &url)
                .as_deref(),
            Some("synthetic-anon-proof")
        );
        assert!(!shared
            .read()
            .unwrap()
            .to_private_json()
            .unwrap()
            .contains("synthetic-anon-proof"));
        assert!(manager.apply_verification(None, "invalid; cookie").is_err());
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
        let _release = gate.release_on_drop();
        test_keyring::gate(&account, gate.clone());
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
            result=&mut waiter => assert!(result.unwrap(),"store fixture never reached its bounded write gate"),
        }
        // Permit the initial read, then fail the gated write at completion.
        test_keyring::unavailable(&account, true);
        drop(store);
        let mut shutdown = Box::pin(manager.shutdown());
        let first = std::future::poll_fn(|cx| Poll::Ready(shutdown.as_mut().poll(cx))).await;
        assert!(first.is_pending());
        assert!(!shared.read().unwrap().is_empty());
        gate.release();
        assert!(shutdown.await.is_err());
        assert!(shared.read().unwrap().is_empty());
        assert!(!test_keyring::contains(&account));
        assert!(test_keyring::owned_entries(&account).is_empty());
        test_keyring::unavailable(&account, false);
    }
}
