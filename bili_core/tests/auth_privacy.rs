use bili_core::{
    auth::{AuthSnapshot, QrChallenge, QrStatus},
    cookie::CredentialJar,
};
use reqwest::Url;

#[test]
fn pasted_cookie_is_https_bilibili_only_and_redacted() {
    let jar = CredentialJar::import_header("SESSDATA=synthetic-session; bili_jct=synthetic-token")
        .unwrap();
    let api = Url::parse("https://api.bilibili.com/x/web-interface/nav").unwrap();
    assert_eq!(
        jar.header_for(&api).as_deref(),
        Some("SESSDATA=synthetic-session; bili_jct=synthetic-token")
    );
    assert!(jar
        .header_for(&Url::parse("http://api.bilibili.com/").unwrap())
        .is_none());
    assert!(jar
        .header_for(&Url::parse("https://upos-sz-mirror.example/").unwrap())
        .is_none());
    assert!(jar
        .header_for(&Url::parse("https://evilbilibili.com/").unwrap())
        .is_none());
    let debug = format!("{jar:?}");
    assert!(!debug.contains("synthetic-session"));
    assert!(!debug.contains("synthetic-token"));
    assert!(CredentialJar::import_header("SESSDATA=x\r\nHost:evil.test").is_err());
}

#[test]
fn structured_cookie_attributes_limit_path_domain_and_expiry() {
    let origin = Url::parse("https://passport.bilibili.com/login/start").unwrap();
    let mut jar = CredentialJar::default();
    jar.import_set_cookies(
        &origin,
        &[
            "session=synthetic; Domain=.bilibili.com; Path=/login; Secure; HttpOnly; SameSite=Lax"
                .into(),
            "expired=synthetic; Path=/; Max-Age=0".into(),
        ],
    )
    .unwrap();
    assert_eq!(
        jar.header_for(&Url::parse("https://api.bilibili.com/login/check").unwrap())
            .as_deref(),
        Some("session=synthetic")
    );
    assert!(jar
        .header_for(&Url::parse("https://api.bilibili.com/other").unwrap())
        .is_none());
    assert!(jar
        .header_for(&Url::parse("https://upos.example/login/check").unwrap())
        .is_none());
    assert!(jar
        .header_for(&Url::parse("https://api.bilibili.com/").unwrap())
        .is_none());
    assert!(jar
        .import_set_cookies(
            &Url::parse("http://passport.bilibili.com/").unwrap(),
            &["x=y".into()]
        )
        .is_err());
    assert!(jar
        .import_set_cookies(&origin, &["bad=synthetic; Domain=.com".into()])
        .is_err());
}

#[test]
fn qr_transient_payload_is_redacted_and_never_serialized() {
    let challenge = QrChallenge {
        login_url: "https://passport.bilibili.com/qr/test?token=synthetic-qr-secret".into(),
        expires_in_secs: 180,
    };
    let snapshot = AuthSnapshot {
        logged_in: false,
        account: None,
        notice: None,
        qr: Some(challenge),
        qr_status: Some(QrStatus::WaitingForScan),
    };
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(!serialized.contains("synthetic-qr-secret"));
    assert!(!serialized.contains("passport.bilibili.com"));
    let debug = format!("{snapshot:?}");
    assert!(!debug.contains("synthetic-qr-secret"));
    assert!(!debug.contains("passport.bilibili.com"));
}
