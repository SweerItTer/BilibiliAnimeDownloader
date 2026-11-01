//! Explicit, transient risk verification. No challenge or grant is journaled.
use crate::{cookie::SharedCredentialJar, CoreError, ErrorCode, Result};
use reqwest::{header, Client, Url};
use serde::Deserialize;
use serde_json::Value;
use std::{fmt, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

const MAX_RESPONSE: usize = 64 * 1024;
const LOCAL_LIFETIME: Duration = Duration::from_secs(180);

#[derive(Clone)]
pub struct VerificationRequest {
    voucher: String,
}

impl VerificationRequest {
    pub(crate) fn from_response(value: &Value) -> Option<Self> {
        fn find(value: &Value, depth: usize) -> Option<&str> {
            if depth > 8 {
                return None;
            }
            match value {
                Value::Object(fields) => fields
                    .get("v_voucher")
                    .and_then(Value::as_str)
                    .filter(|v| safe_field(v))
                    .or_else(|| fields.values().find_map(|v| find(v, depth + 1))),
                Value::Array(items) => items.iter().find_map(|v| find(v, depth + 1)),
                _ => None,
            }
        }
        find(value, 0).map(|voucher| Self {
            voucher: voucher.to_owned(),
        })
    }
}

impl fmt::Debug for VerificationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerificationRequest([REDACTED])")
    }
}

/// The public presentation fields are for the explicitly opened CAPTCHA only.
/// Dropping this value cancels the local verification session.
pub struct VerificationChallenge {
    pub gt: String,
    pub challenge: String,
    pub expires_in_secs: u64,
    token: String,
    profile_id: Option<String>,
    credentials: SharedCredentialJar,
    deadline: Instant,
}
impl fmt::Debug for VerificationChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerificationChallenge([REDACTED])")
    }
}

#[derive(Deserialize)]
pub struct VerificationProof {
    pub challenge: String,
    pub validate: String,
    pub seccode: String,
}
impl fmt::Debug for VerificationProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerificationProof([REDACTED])")
    }
}

pub(crate) struct VerificationGrant {
    pub profile_id: Option<String>,
    pub vtoken: String,
}

pub(crate) async fn begin(
    client: &Client,
    credentials: SharedCredentialJar,
    connections: Arc<Semaphore>,
    request: VerificationRequest,
    profile_id: Option<String>,
) -> Result<VerificationChallenge> {
    let value = post(
        client,
        &credentials,
        &connections,
        "register",
        &[("v_voucher", request.voucher.as_str())],
    )
    .await?;
    parse_challenge(&value, credentials, profile_id)
}

fn parse_challenge(
    value: &Value,
    credentials: SharedCredentialJar,
    profile_id: Option<String>,
) -> Result<VerificationChallenge> {
    let data = result(value)?;
    let field = |pointer| {
        data.pointer(pointer)
            .and_then(Value::as_str)
            .filter(|v| safe_field(v))
            .map(str::to_owned)
            .ok_or_else(invalid)
    };
    Ok(VerificationChallenge {
        gt: field("/geetest/gt")?,
        challenge: field("/geetest/challenge")?,
        token: field("/token")?,
        profile_id,
        credentials,
        expires_in_secs: LOCAL_LIFETIME.as_secs(),
        deadline: Instant::now() + LOCAL_LIFETIME,
    })
}

pub(crate) async fn complete(
    client: &Client,
    connections: Arc<Semaphore>,
    challenge: VerificationChallenge,
    proof: VerificationProof,
) -> Result<VerificationGrant> {
    validate_proof(&challenge, &proof)?;
    let value = post(
        client,
        &challenge.credentials,
        &connections,
        "validate",
        &[
            ("token", challenge.token.as_str()),
            ("challenge", proof.challenge.as_str()),
            ("validate", proof.validate.as_str()),
            ("seccode", proof.seccode.as_str()),
        ],
    )
    .await?;
    if Instant::now() >= challenge.deadline {
        return Err(expired());
    }
    parse_grant(&value, challenge.profile_id)
}

fn validate_proof(challenge: &VerificationChallenge, proof: &VerificationProof) -> Result<()> {
    if Instant::now() >= challenge.deadline {
        return Err(expired());
    }
    if !safe_field(&proof.challenge) || !safe_field(&proof.validate) || !safe_field(&proof.seccode)
    {
        return Err(invalid());
    }
    Ok(())
}

fn parse_grant(value: &Value, profile_id: Option<String>) -> Result<VerificationGrant> {
    let data = result(value)?;
    if data.get("is_valid").and_then(Value::as_u64) != Some(1) {
        return Err(expired());
    }
    let token = data
        .get("grisk_id")
        .and_then(Value::as_str)
        .filter(|v| safe_field(v))
        .ok_or_else(invalid)?;
    Ok(VerificationGrant {
        profile_id,
        vtoken: token.to_owned(),
    })
}

async fn post(
    client: &Client,
    credentials: &SharedCredentialJar,
    connections: &Semaphore,
    operation: &str,
    fields: &[(&str, &str)],
) -> Result<Value> {
    // Operation names are internal constants; never accept a remote destination.
    let endpoint = match operation {
        "register" => "https://api.bilibili.com/x/gaia-vgate/v1/register",
        "validate" => "https://api.bilibili.com/x/gaia-vgate/v1/validate",
        _ => return Err(invalid()),
    };
    let mut url = Url::parse(endpoint).map_err(|_| invalid())?;
    let cookie = credentials.read().map_err(|_| invalid())?.header_for(&url);
    if let Some(csrf) = cookie.as_deref().and_then(|header| {
        header
            .split(';')
            .find_map(|pair| pair.trim().strip_prefix("bili_jct="))
    }) {
        url.query_pairs_mut().append_pair("csrf", csrf);
    }
    let _permit = connections
        .acquire()
        .await
        .map_err(|_| CoreError::new(ErrorCode::Shutdown, "verification connection pool closed"))?;
    let mut request = client
        .post(url)
        .header(header::REFERER, "https://www.bilibili.com/")
        .header(header::USER_AGENT, crate::api::USER_AGENT)
        .form(fields);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let mut response = request.send().await.map_err(|_| transport())?;
    if !response.status().is_success() {
        return Err(transport());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE as u64)
    {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| transport())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

fn result(value: &Value) -> Result<&Value> {
    if value.get("code").and_then(Value::as_i64) != Some(0) {
        return Err(expired());
    }
    value
        .get("data")
        .filter(|v| v.is_object())
        .ok_or_else(invalid)
}
fn safe_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.|+/=".contains(&b))
}
fn invalid() -> CoreError {
    CoreError::new(ErrorCode::InvalidInput, "invalid verification response")
}
fn expired() -> CoreError {
    CoreError::new(
        ErrorCode::VerificationRequired,
        "verification expired or rejected",
    )
}
fn transport() -> CoreError {
    CoreError::new(ErrorCode::Transport, "submit verification")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cookie::CredentialJar;
    use std::sync::RwLock;
    fn jar() -> SharedCredentialJar {
        Arc::new(RwLock::new(CredentialJar::default()))
    }
    #[test]
    fn challenge_requires_real_fields_and_redacts_all_private_values() {
        let canary = "canary_voucher_123";
        let request =
            VerificationRequest::from_response(&serde_json::json!({"data":{"v_voucher":canary}}))
                .unwrap();
        assert!(!format!("{request:?}").contains(canary));
        assert!(
            VerificationRequest::from_response(&serde_json::json!({"v_voucher":"</script>"}))
                .is_none()
        );
        assert!(parse_challenge(&serde_json::json!({"code":0,"data":{}}), jar(), None).is_err());
        let challenge = parse_challenge(&serde_json::json!({"code":0,"data":{"token":canary,"geetest":{"gt":canary,"challenge":canary}}}),jar(),Some("profile-a".into())).unwrap();
        assert!(!format!("{challenge:?}").contains(canary));
        let mut error = CoreError::new(ErrorCode::VerificationRequired, "test challenge");
        error.verification = Some(request);
        assert!(!serde_json::to_string(&error).unwrap().contains(canary));
    }
    #[tokio::test]
    async fn expired_proof_is_rejected_without_network_and_server_grant_is_required() {
        let mut challenge = parse_challenge(&serde_json::json!({"code":0,"data":{"token":"token","geetest":{"gt":"gt","challenge":"challenge"}}}),jar(),None).unwrap();
        challenge.deadline = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            validate_proof(
                &challenge,
                &VerificationProof {
                    challenge: "challenge".into(),
                    validate: "proof".into(),
                    seccode: "proof|jordan".into()
                }
            )
            .unwrap_err()
            .code,
            ErrorCode::VerificationRequired
        );
        assert!(parse_grant(
            &serde_json::json!({"code":0,"data":{"is_valid":0,"grisk_id":"token"}}),
            None
        )
        .is_err());
        let grant = parse_grant(
            &serde_json::json!({"code":0,"data":{"is_valid":1,"grisk_id":"verified"}}),
            Some("profile-a".into()),
        )
        .unwrap();
        assert_eq!(grant.profile_id.as_deref(), Some("profile-a"));
        assert_eq!(grant.vtoken, "verified");
    }
}
