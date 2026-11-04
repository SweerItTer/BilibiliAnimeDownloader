use bili_core::*;
use serde_json::{json, Value};
use std::path::Path;

fn entry(path: &str, query: Value, body: Value) -> Value {
    json!({"path":path,"query":query,"body":body})
}

fn write_replay(root: &Path, responses: Vec<Value>) -> OfflineApiReplay {
    let path = root.join("responses.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"schema_version":1,"responses":responses})).unwrap(),
    )
    .unwrap();
    OfflineApiReplay::load(&path).unwrap()
}

fn playurl(full_hd: bool) -> Value {
    let mut video = vec![json!({
        "id":64,"codecs":"avc1.64001f","width":1280,"height":720,
        "bandwidth":1_000_000,"baseUrl":"https://v.bilivideo.com/offline/720.m4s"
    })];
    if full_hd {
        video.push(json!({
            "id":80,"codecs":"avc1.640028","width":1920,"height":1080,
            "bandwidth":2_000_000,"baseUrl":"https://v.bilivideo.com/offline/1080.m4s"
        }));
    }
    json!({"code":0,"result":{
        "quality":if full_hd {80} else {64},
        "support_formats":[{"quality":120,"new_description":"4K"},{"quality":80,"new_description":"1080P"},{"quality":64,"new_description":"720P"}],
        "dash":{"video":video,"audio":[{
            "id":30280,"codecs":"mp4a.40.2","bandwidth":128_000,
            "audio_sampling_rate":"48000","channels":2,
            "baseUrl":"https://a.bilivideo.com/offline/audio.m4s"
        }]}
    }})
}

fn selection(part: PartRef, quality: Option<u32>) -> DownloadSelection {
    DownloadSelection {
        part,
        title: "Offline selection".into(),
        quality,
        video_representation: None,
        audio_representation: None,
        codec: None,
        dynamic_range: None,
        output: None,
        merge: MergePolicy::Automatic,
        sidecars: SidecarPolicy {
            subtitles: SubtitleSelection::None,
            danmaku: false,
        },
        keep_temp: false,
    }
}

#[tokio::test]
async fn real_core_replay_retains_parts_and_strict_obtainable_quality() {
    // Core replay also forces its HTTP clients to the closed-loopback proxy;
    // tests do not depend on or mutate process-wide proxy settings.
    let root = tempfile::tempdir().unwrap();
    let season = json!({"code":0,"result":{
        "season_id":9001,"title":"Offline season","type":1,
        "episodes":[
            {"id":9101,"aid":9201,"cid":9301,"title":"1","long_title":"First","duration":60_000,"is_preview":false},
            {"id":9102,"aid":9202,"cid":9302,"title":"2","long_title":"Second","duration":60_000,"is_preview":false}
        ]
    }});
    let mut responses = vec![entry(
        "/pgc/view/web/season",
        json!({"season_id":"9001"}),
        season.clone(),
    )];
    for (episode, full_hd) in [(9101, true), (9102, false)] {
        responses.push(entry(
            "/pgc/view/web/season",
            json!({"ep_id":episode.to_string()}),
            season.clone(),
        ));
        for quality in [127, 120, 80, 64] {
            responses.push(entry(
                "/pgc/player/web/v2/playurl",
                json!({"ep_id":episode.to_string(),"qn":quality.to_string(),"fnval":"12240","fourk":"1"}),
                playurl(full_hd),
            ));
        }
    }
    let mut config = CoreConfig::new(root.path().join("state"), root.path().join("output"));
    config.state_mode = StateMode::ReadOnly;
    config.api_replay = Some(write_replay(root.path(), responses));
    let core = Core::new(config, tokio::runtime::Handle::current()).unwrap();

    let item = core.resolve_for("ss9001".into(), None).await.unwrap();
    assert_eq!(item.parts.len(), 2);
    assert_eq!(item.parts[0].ordinal, 1);
    assert_eq!(item.parts[1].reference.episode.unwrap().get(), 9102);
    let explicit = core.resolve_for("ep9102".into(), None).await.unwrap();
    assert_eq!(explicit.parts.len(), 1);
    assert_eq!(explicit.parts[0].reference, item.parts[1].reference);
    assert_eq!(explicit.parts[0].ordinal, 2);

    let first = core
        .quality_catalog_for(item.parts[0].reference.clone(), None)
        .await
        .unwrap();
    assert_eq!(
        first
            .select(&selection(item.parts[0].reference.clone(), None))
            .unwrap()
            .0
            .quality,
        80
    );
    assert!(first
        .select(&selection(item.parts[0].reference.clone(), Some(120)))
        .is_err());
    let second = core
        .quality_catalog_for(item.parts[1].reference.clone(), None)
        .await
        .unwrap();
    assert_eq!(
        second
            .select(&selection(item.parts[1].reference.clone(), None))
            .unwrap()
            .0
            .quality,
        64
    );
    assert!(second
        .select(&selection(item.parts[1].reference.clone(), Some(80)))
        .is_err());
    let plan = core
        .plan(selection(item.parts[0].reference.clone(), Some(80)))
        .await
        .unwrap();
    assert_eq!(plan.video.quality, 80);
    assert_eq!(
        (plan.video.width, plan.video.height),
        (Some(1920), Some(1080))
    );
    assert_eq!(plan.part, item.parts[0].reference);
    assert_eq!(
        core.plan(selection(item.parts[1].reference.clone(), Some(80)))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    let missing = core.resolve_for("av9402".into(), None).await.unwrap_err();
    assert_eq!(missing.operation, "offline API replay response missing");
    assert!(core.jobs().borrow().jobs.is_empty());
    core.shutdown(StopMode::Pause).await.unwrap();
    assert!(!root.path().join("state").exists());
}

#[tokio::test]
async fn replay_rejects_persistent_auth_before_opening_any_state() {
    let root = tempfile::tempdir().unwrap();
    let replay = write_replay(
        root.path(),
        vec![entry("/x/web-interface/nav", json!({}), json!({"code":0}))],
    );
    let mut config = CoreConfig::new(root.path().join("state"), root.path().join("output"));
    config.api_replay = Some(replay);
    config.credential_store = cookie::StorePolicy::Remember {
        directory: root.path().join("auth"),
        allow_plaintext_fallback: false,
    };
    assert!(
        matches!(Core::new(config, tokio::runtime::Handle::current()), Err(error) if error.code == ErrorCode::InvalidInput)
    );
    assert!(!root.path().join("state").exists());
    assert!(!root.path().join("auth").exists());
}
