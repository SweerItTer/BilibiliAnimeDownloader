//! Scoped in-memory Bilibili credentials and opt-in remember policy.
use crate::domain::{CoreError, ErrorCode, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const IMPORT_HOSTS: &[&str] = &[
    "api.bilibili.com",
    "www.bilibili.com",
    "passport.bilibili.com",
    "account.bilibili.com",
    "member.bilibili.com",
    "message.bilibili.com",
];

#[derive(Clone, Default)]
pub struct CredentialJar(Vec<CredentialCookie>);
pub type SharedCredentialJar = Arc<RwLock<CredentialJar>>;

impl std::fmt::Debug for CredentialJar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialJar")
            .field("cookie_count", &self.0.len())
            .finish()
    }
}

#[derive(Clone)]
struct CredentialCookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: Option<String>,
    expires: Option<SystemTime>,
}

impl std::fmt::Debug for CredentialCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialCookie")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("secure", &self.secure)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum StorePolicy {
    MemoryOnly,
    Remember {
        directory: PathBuf,
        allow_plaintext_fallback: bool,
    },
}
/// Restrict a private credential/state path to its owner and SYSTEM.
pub(crate) fn restrict_private(path: &Path, is_directory: bool) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(CoreError::io)?;
    if metadata.file_type().is_symlink()
        || metadata.is_dir() != is_directory
        || (!is_directory && !metadata.is_file())
    {
        return Err(CoreError::new(ErrorCode::Filesystem, "private path type"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if is_directory { 0o700 } else { 0o600 }),
        )
        .map_err(CoreError::io)?;
    }
    #[cfg(windows)]
    windows_private_acl::restrict(path, is_directory)?;
    #[cfg(not(any(unix, windows)))]
    return Err(CoreError::new(
        ErrorCode::Filesystem,
        "private permissions unavailable",
    ));
    Ok(())
}

#[cfg(windows)]
mod windows_private_acl {
    use super::*;
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE},
        Security::{
            Authorization::{
                SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE,
                SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
                TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_TYPE, TRUSTEE_W,
            },
            GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION, NO_INHERITANCE,
            PROTECTED_DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
            TOKEN_USER,
        },
        Storage::FileSystem::FILE_ALL_ACCESS,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    pub(super) fn restrict(path: &Path, is_directory: bool) -> Result<()> {
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(failure());
            }
            let mut needed = 0;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                CloseHandle(token);
                return Err(failure());
            }
            let unit_size = std::mem::size_of::<usize>();
            let mut buffer = vec![0usize; (needed as usize).div_ceil(unit_size)];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            );
            CloseHandle(token);
            if ok == 0 {
                return Err(failure());
            }
            let user = (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid;
            let mut system_sid = vec![0u32; 17];
            let mut sid_len = (system_sid.len() * std::mem::size_of::<u32>()) as u32;
            if windows_sys::Win32::Security::CreateWellKnownSid(
                windows_sys::Win32::Security::WinLocalSystemSid,
                ptr::null_mut(),
                system_sid.as_mut_ptr().cast(),
                &mut sid_len,
            ) == 0
            {
                return Err(failure());
            }

            let entries = [
                access(user, TRUSTEE_IS_USER, is_directory),
                access(
                    system_sid.as_mut_ptr().cast(),
                    TRUSTEE_IS_WELL_KNOWN_GROUP,
                    is_directory,
                ),
            ];
            let mut acl = ptr::null_mut();
            if SetEntriesInAclW(2, entries.as_ptr(), ptr::null_mut(), &mut acl) != 0 {
                return Err(failure());
            }
            let mut name: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
            let status = SetNamedSecurityInfoW(
                name.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null_mut(),
            );
            LocalFree(acl.cast());
            if status != 0 {
                return Err(failure());
            }
        }
        Ok(())
    }

    unsafe fn access(
        sid: *mut core::ffi::c_void,
        trustee_type: TRUSTEE_TYPE,
        is_directory: bool,
    ) -> EXPLICIT_ACCESS_W {
        EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: SET_ACCESS,
            grfInheritance: if is_directory {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                NO_INHERITANCE
            },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: trustee_type,
                ptstrName: sid.cast(),
            },
        }
    }
    fn failure() -> CoreError {
        CoreError::new(ErrorCode::Filesystem, "private permissions")
    }
}

impl CredentialJar {
    /// Import a request Cookie header with session-only, exact-host scope.
    pub fn import_header(header: &str) -> Result<Self> {
        if header.is_empty() || header.len() > 64 * 1024 || header.contains(['\r', '\n']) {
            return Err(invalid_cookie());
        }
        let mut parsed = Vec::new();
        for pair in header.split(';') {
            let (name, value) = pair.trim().split_once('=').ok_or_else(invalid_cookie)?;
            if !valid_name(name) || !valid_value(value) {
                return Err(invalid_cookie());
            }
            for host in IMPORT_HOSTS {
                parsed.push(CredentialCookie {
                    name: name.to_owned(),
                    value: value.to_owned(),
                    domain: (*host).to_owned(),
                    host_only: true,
                    path: "/".to_owned(),
                    secure: true,
                    http_only: false,
                    same_site: None,
                    expires: None,
                });
            }
        }
        if parsed.is_empty() {
            return Err(invalid_cookie());
        }
        Ok(Self(parsed))
    }

    /// Add Set-Cookie values received from the trusted HTTPS passport flow.
    pub fn import_set_cookies(&mut self, origin: &Url, values: &[String]) -> Result<()> {
        if origin.scheme() != "https"
            || !origin.host_str().is_some_and(is_import_host)
            || values.len() > 512
            || values.iter().map(String::len).sum::<usize>() > 512 * 1024
        {
            return Err(invalid_cookie());
        }
        for value in values {
            if value.contains(['\r', '\n']) || value.len() > 16 * 1024 {
                return Err(invalid_cookie());
            }
            let mut parts = value.split(';');
            let (name, value) = parts
                .next()
                .and_then(|s| s.trim().split_once('='))
                .ok_or_else(invalid_cookie)?;
            if !valid_name(name) || !valid_value(value) {
                return Err(invalid_cookie());
            }
            let host = origin
                .host_str()
                .ok_or_else(invalid_cookie)?
                .to_ascii_lowercase();
            let mut cookie = CredentialCookie {
                name: name.to_owned(),
                value: value.to_owned(),
                domain: host.clone(),
                host_only: true,
                path: default_path(origin.path()),
                secure: false,
                http_only: false,
                same_site: None,
                expires: None,
            };
            let mut max_age = None;
            for attr in parts {
                let (key, val) = attr.trim().split_once('=').unwrap_or((attr.trim(), ""));
                match key.trim().to_ascii_lowercase().as_str() {
                    "domain" => {
                        let domain = val.trim().trim_start_matches('.').to_ascii_lowercase();
                        if domain.is_empty()
                            || !(domain == host
                                || (domain == "bilibili.com" && host.ends_with(".bilibili.com")))
                        {
                            return Err(invalid_cookie());
                        }
                        cookie.domain = domain;
                        cookie.host_only = false;
                    }
                    "path" if val.starts_with('/') => cookie.path = val.to_owned(),
                    "secure" => cookie.secure = true,
                    "httponly" => cookie.http_only = true,
                    "samesite" => cookie.same_site = Some(val.trim().to_owned()),
                    "max-age" => max_age = val.trim().parse::<i64>().ok(),
                    "expires" => cookie.expires = parse_http_date(val.trim()),
                    _ => {}
                }
            }
            if let Some(age) = max_age {
                cookie.expires = if age <= 0 {
                    Some(UNIX_EPOCH)
                } else {
                    SystemTime::now().checked_add(Duration::from_secs(age as u64))
                };
            }
            self.0.retain(|old| {
                !(old.name == cookie.name && old.domain == cookie.domain && old.path == cookie.path)
            });
            if !cookie.expires.is_some_and(|at| at <= SystemTime::now()) {
                self.0.push(cookie);
            }
        }
        Ok(())
    }

    /// Produce a Cookie header only for a matching HTTPS origin.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        if url.scheme() != "https" {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        if !is_import_host(&host) {
            return None;
        }
        let path = url.path();
        let now = SystemTime::now();
        let mut out = String::new();
        for c in &self.0 {
            let domain_match = if c.host_only {
                host == c.domain
            } else {
                host == c.domain || host.ends_with(&format!(".{}", c.domain))
            };
            if !domain_match
                || (c.secure && url.scheme() != "https")
                || !path_matches(path, &c.path)
                || c.expires.is_some_and(|e| e <= now)
            {
                continue;
            }
            if !out.is_empty() {
                out.push_str("; ");
            }
            out.push_str(&c.name);
            out.push('=');
            out.push_str(&c.value);
        }
        (!out.is_empty()).then_some(out)
    }

    pub(crate) fn to_private_json(&self) -> Result<String> {
        if self.0.len() > 512 {
            return Err(CoreError::new(
                ErrorCode::CredentialStore,
                "credential storage limit",
            ));
        }
        let cookies: Vec<StoredCookie> = self.0.iter().map(StoredCookie::from).collect();
        let value = serde_json::to_string(&cookies).map_err(|_| {
            CoreError::new(ErrorCode::CredentialStore, "credential storage encoding")
        })?;
        if value.len() > 1024 * 1024 {
            return Err(CoreError::new(
                ErrorCode::CredentialStore,
                "credential storage limit",
            ));
        }
        Ok(value)
    }

    pub(crate) fn from_private_json(value: &str) -> Result<Self> {
        if value.len() > 1024 * 1024 {
            return Err(CoreError::new(
                ErrorCode::CredentialStore,
                "credential storage limit",
            ));
        }
        let stored: Vec<StoredCookie> = serde_json::from_str(value).map_err(|_| {
            CoreError::new(ErrorCode::CredentialStore, "credential storage decoding")
        })?;
        if stored.len() > 512 {
            return Err(CoreError::new(
                ErrorCode::CredentialStore,
                "credential storage limit",
            ));
        }
        let mut jar = Self::default();
        for c in stored {
            if c.expires
                .is_some_and(|e| UNIX_EPOCH.checked_add(Duration::from_secs(e)).is_none())
            {
                return Err(CoreError::new(
                    ErrorCode::CredentialStore,
                    "credential storage validation",
                ));
            }
            let cookie: CredentialCookie = c.into();
            if !valid_name(&cookie.name)
                || !valid_value(&cookie.value)
                || !cookie.path.starts_with('/')
                || cookie.path.len() > 4096
                || cookie.name.len() > 256
                || cookie.value.len() > 16 * 1024
                || !(is_import_host(&cookie.domain) || cookie.domain == "bilibili.com")
            {
                return Err(CoreError::new(
                    ErrorCode::CredentialStore,
                    "credential storage validation",
                ));
            }
            jar.0.push(cookie);
        }
        Ok(jar)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

fn is_import_host(host: &str) -> bool {
    IMPORT_HOSTS.contains(&host)
}
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
fn valid_value(s: &str) -> bool {
    !s.bytes()
        .any(|b| b <= 0x20 || b == 0x7f || b == b'"' || b == b',' || b == b';' || b == b'\\')
}
fn invalid_cookie() -> CoreError {
    CoreError::new(ErrorCode::InvalidInput, "invalid credential input")
}
fn default_path(path: &str) -> String {
    if !path.starts_with('/') || path == "/" {
        return "/".into();
    }
    path.rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
        .unwrap_or("/")
        .to_owned()
}
fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie
        || (request.starts_with(cookie)
            && (cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/')))
}

fn parse_http_date(value: &str) -> Option<SystemTime> {
    httpdate::parse_http_date(value).ok()
}
#[derive(Serialize, Deserialize)]
struct StoredCookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: Option<String>,
    expires: Option<u64>,
}
impl From<&CredentialCookie> for StoredCookie {
    fn from(c: &CredentialCookie) -> Self {
        Self {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            host_only: c.host_only,
            path: c.path.clone(),
            secure: c.secure,
            http_only: c.http_only,
            same_site: c.same_site.clone(),
            expires: c
                .expires
                .and_then(|e| e.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        }
    }
}
impl From<StoredCookie> for CredentialCookie {
    fn from(c: StoredCookie) -> Self {
        Self {
            name: c.name,
            value: c.value,
            domain: c.domain,
            host_only: c.host_only,
            path: c.path,
            secure: c.secure,
            http_only: c.http_only,
            same_site: c.same_site,
            expires: c.expires.map(|e| UNIX_EPOCH + Duration::from_secs(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_scope_and_crlf_are_enforced() {
        let jar = CredentialJar::import_header("SESSDATA=synthetic").unwrap();
        assert_eq!(
            jar.header_for(&Url::parse("https://api.bilibili.com/x").unwrap())
                .as_deref(),
            Some("SESSDATA=synthetic")
        );
        assert!(jar
            .header_for(&Url::parse("https://upos-sz-mirror.example/x").unwrap())
            .is_none());
        assert!(CredentialJar::import_header("x=y\r\nHost: evil").is_err());
    }
    #[test]
    fn set_cookie_attrs_are_applied() {
        let mut jar = CredentialJar::default();
        let origin = Url::parse("https://passport.bilibili.com/foo/bar").unwrap();
        jar.import_set_cookies(
            &origin,
            &["token=v; Path=/foo; Secure; Domain=.bilibili.com".into()],
        )
        .unwrap();
        assert_eq!(
            jar.header_for(&Url::parse("https://api.bilibili.com/foo/x").unwrap())
                .as_deref(),
            Some("token=v")
        );
        assert!(jar
            .header_for(&Url::parse("https://upos.example/foo/x").unwrap())
            .is_none());
        assert!(jar
            .header_for(&Url::parse("https://api.bilibili.com/other").unwrap())
            .is_none());
    }
}
