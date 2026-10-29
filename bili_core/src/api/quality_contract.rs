use super::*;
use serde_json::json;

fn part() -> PartRef {
    let aid = Aid::new(7).unwrap();
    PartRef {
        content: ContentRef::Video(aid),
        aid,
        cid: Cid::new(8).unwrap(),
        episode: None,
    }
}

fn selection(part: PartRef) -> DownloadSelection {
    DownloadSelection {
        part,
        title: "fixture".into(),
        quality: None,
        video_representation: None,
        audio_representation: None,
        codec: None,
        dynamic_range: None,
        output: None,
        merge: MergePolicy::Automatic,
        sidecars: SidecarPolicy::default(),
        keep_temp: false,
    }
}

#[test]
fn obtainable_resolution_beats_a_higher_numeric_quality() {
    let payload = json!({"code":0,"data":{"dash":{"video":[{"id":120,"codecs":"avc","width":1920,"height":1080,"bandwidth":8000},{"id":80,"codecs":"avc","width":3840,"height":2160,"bandwidth":4000}],"audio":[]},"support_formats":[{"quality":120,"new_description":"1080p"},{"quality":80,"new_description":"4K"}]}});
    let catalog = parse_catalog(part(), &payload).unwrap();
    let selected = catalog.select(&selection(part())).unwrap().0;
    assert_eq!(
        (selected.width, selected.height, selected.quality),
        (Some(3840), Some(2160), 80)
    );
}

#[test]
fn repeated_numeric_id_across_codecs_requires_an_explicit_filter() {
    let payload = json!({"code":0,"data":{"dash":{"video":[{"id":80,"codecs":"avc","width":1920,"height":1080},{"id":80,"codecs":"hev","width":3840,"height":2160}],"audio":[]},"support_formats":[{"quality":80,"new_description":"quality"}]}});
    let catalog = parse_catalog(part(), &payload).unwrap();
    let mut choice = selection(part());
    choice.quality = Some(80);
    choice.video_representation = Some(80);
    assert_eq!(
        catalog.select(&choice).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    choice.codec = Some("hev".into());
    assert_eq!(catalog.select(&choice).unwrap().0.codec, "hev");
}
#[test]
fn catalog_keeps_distinct_representation_metadata_with_same_id_and_codec() {
    let payload = json!({"code":0,"data":{"dash":{"video":[
        {"id":80,"codecs":"avc","width":1920,"height":1080,"frameRate":"30/1","color":"BT.709"},
        {"id":80,"codecs":"avc","width":1920,"height":1080,"frameRate":"60/1","color":"BT.2020"}
    ]}}});
    let catalog = parse_catalog(part(), &payload).unwrap();
    let variants: Vec<_> = catalog
        .representations
        .iter()
        .filter(|rep| !rep.audio)
        .collect();
    assert_eq!(variants.len(), 2);
    assert_ne!(variants[0].fps, variants[1].fps);
    assert_ne!(variants[0].color, variants[1].color);
}
#[test]
fn automatic_selection_does_not_rank_unknown_dimensions_by_numeric_id() {
    let payload = json!({"code":0,"data":{"dash":{"video":[
        {"id":80,"codecs":"avc","bandwidth":100},
        {"id":64,"codecs":"hev","bandwidth":1000}
    ]}}});
    let catalog = parse_catalog(part(), &payload).unwrap();
    assert_eq!(
        catalog.select(&selection(part())).unwrap_err().code,
        ErrorCode::Unavailable
    );
    let mut choice = selection(part());
    choice.quality = Some(64);
    assert_eq!(catalog.select(&choice).unwrap().0.quality, 64);
}

#[test]
fn catalog_preserves_independent_fps_hdr_codec_and_unknown_access() {
    let payload = json!({"code":0,"data":{"quality":120,"support_formats":[{"quality":120,"new_description":"4K"},{"quality":125,"new_description":"unreturned"}],"dash":{"video":[{"id":120,"codecs":"av01","width":3840,"height":2160,"frameRate":"59.94","hdr_type":2,"color":"BT.2020","bandwidth":9000}],"audio":[{"id":30280,"codecs":"mp4a.40.2","bandwidth":256000,"audio_sampling_rate":"48000","channels":2}]}}});
    let catalog = parse_catalog(part(), &payload).unwrap();
    let video = catalog.representations.iter().find(|r| !r.audio).unwrap();
    let audio = catalog.representations.iter().find(|r| r.audio).unwrap();
    assert_eq!(video.codec, "av01");
    assert_eq!(
        video.fps,
        Some(Rational {
            numerator: 5994,
            denominator: 100
        })
    );
    assert_eq!(video.dynamic_range.as_deref(), Some("2"));
    assert_eq!(video.color.as_deref(), Some("BT.2020"));
    assert_eq!(audio.sample_rate, Some(48000));
    assert_eq!(audio.channels, Some(2));
    assert_eq!(
        catalog
            .advertised
            .iter()
            .find(|q| q.id == 125)
            .unwrap()
            .availability,
        Availability::Unknown
    );
}

#[test]
fn all_direct_segments_keep_order_and_locators_redact_signed_urls() {
    let payload = json!({"code":0,"data":{"quality":80,"durl":[{"url":"https://upos-sz-mirror.bilivideo.com/first?token=secret","backupUrl":["https://upos-sz-mirror.bilivideo.com/alternate"],"size":11,"length":5000,"format":"mp4"},{"url":"https://upos-sz-mirror.bilivideo.com/second?token=secret","size":22,"length":3000,"format":"mp4"}]}});
    let catalog = parse_catalog(part(), &payload).unwrap();
    let video = catalog.representations.iter().find(|r| !r.audio).unwrap();
    assert_eq!(video.codec, "unknown");
    let locators = parse_locators(&part(), video, None, &payload).unwrap();
    assert_eq!(locators.len(), 2);
    assert!(locators[0].url.path().ends_with("first"));
    assert!(locators[1].url.path().ends_with("second"));
    assert_eq!(locators[0].expected_size, Some(11));
    assert_eq!(locators[0].duration_ms, Some(5000));
    assert_eq!(locators[1].duration_ms, Some(3000));
    assert!(!format!("{:?}", locators[0]).contains("secret"));
    assert!(!locators[0]
        .allows(&Url::parse("https://user@upos-sz-mirror.bilivideo.com/third").unwrap()));
    assert!(
        !locators[0].allows(&Url::parse("https://upos-sz-mirror.bilivideo.com:444/third").unwrap())
    );
}
#[test]
fn selected_dash_representations_use_allowed_same_stream_backups() {
    let payload = json!({
        "code": 0,
        "data": {
            "support_formats": [
                {"quality": 127, "new_description": "4K"},
                {"quality": 80, "new_description": "1080p"}
            ],
            "dash": {
                "video": [
                    {
                        "id": 127,
                        "codecs": "avc",
                        "width": 3840,
                        "height": 2160,
                        "baseUrl": "https://xy.mcdn.bilivideo.cn:8082/video",
                        "backupUrl": [
                            "https://upos-sz-mirrorhwo1.bilivideo.com.attacker.invalid/video",
                            "https://upos-sz-mirrorhwo1.bilivideo.com/video?token=first",
                            "https://upos-sz-mirrorhw.bilivideo.com/video?token=second"
                        ]
                    },
                    {
                        "id": 80,
                        "codecs": "avc",
                        "width": 1920,
                        "height": 1080,
                        "baseUrl": "https://upos-sz-mirror.bilivideo.com/lower"
                    }
                ],
                "audio": [
                    {
                        "id": 30280,
                        "codecs": "aac",
                        "bandwidth": 192000,
                        "baseUrl": "https://w5u929t.edge.mountaintoys.cn:4483/audio",
                        "backupUrl": [
                            "https://upos-sz-mirrorbd.bilivideo.com.attacker.invalid/audio",
                            "https://upos-sz-mirrorbd.bilivideo.com/audio?token=first",
                            "https://upos-sz-mirrorbd2.bilivideo.com/audio?token=second"
                        ]
                    },
                    {
                        "id": 30232,
                        "codecs": "aac",
                        "bandwidth": 128000,
                        "baseUrl": "https://upos-sz-mirror.bilivideo.com/lower-audio"
                    }
                ]
            }
        }
    });
    let catalog = parse_catalog(part(), &payload).unwrap();
    let (video, audio) = catalog.select(&selection(part())).unwrap();
    assert_eq!(video.id, 127);
    assert_eq!(audio.as_ref().unwrap().id, 30280);

    let locators = parse_locators(&part(), &video, audio.as_ref(), &payload).unwrap();
    assert_eq!(locators.len(), 2);
    assert_eq!(
        locators
            .iter()
            .map(|locator| locator.url.host_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "upos-sz-mirrorhwo1.bilivideo.com",
            "upos-sz-mirrorbd.bilivideo.com"
        ]
    );
    assert!(locators.iter().all(|locator| locator.url.port().is_none()));
}

#[test]
fn every_direct_segment_uses_an_allowed_backup_without_widening_hosts() {
    let payload = json!({
        "code": 0,
        "data": {
            "quality": 80,
            "durl": [
                {
                    "url": "https://untrusted.invalid:4483/first",
                    "backup_url": [
                        "https://upos-sz-mirror.bilivideo.com/first?token=first",
                        "https://another.invalid/first"
                    ],
                    "size": 11,
                    "length": 5000
                },
                {
                    "url": "https://upos-sz-mirror.bilivideo.cn:8082/second",
                    "backupUrl": [
                        "https://upos-sz-mirror.bilivideo.com/second?token=second"
                    ],
                    "size": 22,
                    "length": 3000
                }
            ]
        }
    });
    let catalog = parse_catalog(part(), &payload).unwrap();
    let video = catalog.representations.iter().find(|r| !r.audio).unwrap();
    let locators = parse_locators(&part(), video, None, &payload).unwrap();
    assert_eq!(
        locators
            .iter()
            .map(|locator| locator.url.path())
            .collect::<Vec<_>>(),
        vec!["/first", "/second"]
    );
    assert_eq!(locators[0].expected_size, Some(11));
    assert_eq!(locators[0].duration_ms, Some(5000));
    assert_eq!(locators[1].expected_size, Some(22));
    assert_eq!(locators[1].duration_ms, Some(3000));
}

#[test]
fn dash_segments_select_allowed_backups_independently() {
    let payload = json!({
        "code": 0,
        "data": {
            "dash": {
                "video": [{
                    "id": 80,
                    "codecs": "avc",
                    "width": 1920,
                    "height": 1080,
                    "segments": [
                        {
                            "baseUrl": "https://xy.mcdn.bilivideo.cn:8082/v0",
                            "backupUrl": ["https://upos-sz-mirror.bilivideo.com/v0?token=0"],
                            "size": 10,
                            "duration_ms": 4000
                        },
                        {
                            "baseUrl": "https://untrusted.invalid:4483/v1",
                            "backupUrl": ["https://upos-sz-mirror.bilivideo.com/v1?token=1"],
                            "size": 20,
                            "duration_ms": 6000
                        }
                    ]
                }],
                "audio": [{
                    "id": 30280,
                    "codecs": "aac",
                    "bandwidth": 128000,
                    "baseUrl": "https://a.bilivideo.com/audio"
                }]
            }
        }
    });
    let catalog = parse_catalog(part(), &payload).unwrap();
    let video = catalog.representations.iter().find(|r| !r.audio).unwrap();
    let audio = catalog.representations.iter().find(|r| r.audio).unwrap();
    let locators = parse_locators(&part(), video, Some(audio), &payload).unwrap();
    assert_eq!(
        locators
            .iter()
            .map(|locator| locator.url.path())
            .collect::<Vec<_>>(),
        vec!["/v0", "/v1", "/audio"]
    );
    assert_eq!(
        locators.iter().map(|l| l.expected_size).collect::<Vec<_>>(),
        vec![Some(10), Some(20), None]
    );
    assert_eq!(
        locators.iter().map(|l| l.duration_ms).collect::<Vec<_>>(),
        vec![Some(4000), Some(6000), None]
    );
}

#[test]
fn untrusted_backup_candidates_do_not_expand_the_stream_policy() {
    for backup in [
        "https://upos-sz-mirror.bilivideo.com.attacker.invalid/media",
        "https://attacker.invalid/media",
        "https://127.0.0.1/media",
        "http://upos-sz-mirror.bilivideo.com/media",
        "https://user@upos-sz-mirror.bilivideo.com/media",
        "https://upos-sz-mirror.bilivideo.com:444/media",
    ] {
        let payload = json!({
            "code": 0,
            "data": {
                "support_formats": [
                    {"quality": 127, "new_description": "4K"},
                    {"quality": 80, "new_description": "1080p"}
                ],
                "dash": {
                    "video": [
                        {
                            "id": 127,
                            "codecs": "avc",
                            "width": 3840,
                            "height": 2160,
                            "baseUrl": "https://bad.invalid:4483/high",
                            "backupUrl": [backup]
                        },
                        {
                            "id": 80,
                            "codecs": "avc",
                            "width": 1920,
                            "height": 1080,
                            "baseUrl": "https://upos-sz-mirror.bilivideo.com/lower"
                        }
                    ]
                }
            }
        });
        let catalog = parse_catalog(part(), &payload).unwrap();
        let (video, _) = catalog.select(&selection(part())).unwrap();
        assert_eq!(video.id, 127);
        assert!(parse_locators(&part(), &video, None, &payload).is_err());
    }
}

#[test]
fn dash_locators_are_video_then_audio_and_preserve_segments() {
    let payload = json!({"code":0,"data":{"dash":{"video":[{"id":80,"codecs":"avc","width":1280,"height":720,"segments":[{"baseUrl":"https://v.bilivideo.com/v0","size":1,"duration_ms":4000},{"baseUrl":"https://v.bilivideo.com/v1","size":2,"duration_ms":6000}]}],"audio":[{"id":30280,"codecs":"aac","bandwidth":100,"segments":[{"baseUrl":"https://a.bilivideo.com/a0","size":3}]}]}}});
    let catalog = parse_catalog(part(), &payload).unwrap();
    let video = catalog.representations.iter().find(|r| !r.audio).unwrap();
    let audio = catalog.representations.iter().find(|r| r.audio).unwrap();
    let locators = parse_locators(&part(), video, Some(audio), &payload).unwrap();
    assert_eq!(
        locators.iter().map(|l| l.url.path()).collect::<Vec<_>>(),
        vec!["/v0", "/v1", "/a0"]
    );
    assert_eq!(
        locators.iter().map(|l| l.duration_ms).collect::<Vec<_>>(),
        vec![Some(4000), Some(6000), None]
    );
}

#[test]
fn pgc_playurl_trial_is_not_obtainable_but_explicit_trailer_is() {
    let aid = Aid::new(71).unwrap();
    let episode = EpisodeId::new(72).unwrap();
    let part = PartRef {
        content: ContentRef::Episode(episode),
        aid,
        cid: Cid::new(73).unwrap(),
        episode: Some(episode),
    };
    let payload = json!({"code":0,"result":{"play_video_type":"preview","video_info":{"support_formats":[{"quality":80,"new_description":"1080p"}],"dash":{"video":[{"id":80,"codecs":"avc1","width":1920,"height":1080,"frame_rate":"30/1","baseUrl":"https://v.bilivideo.com/video"}],"audio":[{"id":30280,"codecs":"aac","bandwidth":128000,"baseUrl":"https://a.bilivideo.com/audio"}],"flac":{"audio":{"id":30251,"codecs":"flac","bandwidth":500000,"baseUrl":"https://a.bilivideo.com/flac"}}}}}});
    let mut catalog = parse_catalog(part.clone(), &payload).unwrap();
    assert!(catalog.preview);
    assert_eq!(
        catalog.advertised[0].availability,
        Availability::PreviewOnly
    );
    let mut choice = selection(part);
    assert_eq!(
        catalog.select(&choice).unwrap_err().code,
        ErrorCode::Unavailable
    );
    allow_explicit_trailer(&mut catalog, true);
    assert!(!catalog.preview);
    choice.audio_representation = Some(30251);
    let (video, audio) = catalog.select(&choice).unwrap();
    assert_eq!(
        video.fps,
        Some(Rational {
            numerator: 30,
            denominator: 1
        })
    );
    assert_eq!(audio.unwrap().codec, "flac");
}

#[test]
fn plan_fingerprint_tracks_media_not_presentation_or_delivery_policy() {
    let media = part();
    let video = RepresentationKey {
        part: media.clone(),
        id: 80,
        quality: 80,
        codec: "avc".into(),
        width: Some(1920),
        height: Some(1080),
        fps: None,
        dynamic_range: None,
        color: None,
        bandwidth: 100,
        audio: false,
        sample_rate: None,
        channels: None,
        language: None,
    };
    let mut plan = StreamPlan {
        schema_version: 1,
        fingerprint: String::new(),
        part: media.clone(),
        title: "first".into(),
        video: video.clone(),
        audio: None,
        topology: StreamTopology::Direct,
        segments: vec![SegmentIdentity {
            ordinal: 0,
            size: Some(100),
            duration_ms: Some(5000),
        }],
        destination: "/tmp/first.mp4".into(),
        merge: MergePolicy::Automatic,
        sidecars: SidecarPolicy::default(),
        keep_temp: false,
        catalog_revision: 1,
    };
    let original = plan_fingerprint(&plan).unwrap();
    plan.title = "renamed".into();
    plan.destination = "/tmp/elsewhere.MP4".into();
    plan.merge = MergePolicy::Manual;
    plan.sidecars = SidecarPolicy {
        subtitles: SubtitleSelection::None,
        danmaku: false,
    };
    plan.keep_temp = true;
    plan.catalog_revision = 999;
    assert_eq!(plan_fingerprint(&plan).unwrap(), original);
    plan.video.width = Some(3840);
    assert_ne!(plan_fingerprint(&plan).unwrap(), original);
    plan.video = video;
    plan.audio = Some(RepresentationKey {
        part: media,
        id: 30280,
        quality: 30280,
        codec: "aac".into(),
        width: None,
        height: None,
        fps: None,
        dynamic_range: None,
        color: None,
        bandwidth: 128000,
        audio: true,
        sample_rate: Some(48000),
        channels: Some(2),
        language: None,
    });
    assert_ne!(plan_fingerprint(&plan).unwrap(), original);
    plan.destination = "/tmp/no-container".into();
    assert_eq!(
        plan_fingerprint(&plan).unwrap_err().code,
        ErrorCode::InvalidInput
    );
}
#[test]
fn cover_validation_decodes_supported_bytes_and_reports_their_real_mime() {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    assert_eq!(validate_image(png.get_ref()).unwrap(), "image/png");
    assert!(validate_image(b"not an image").is_err());
}
#[test]
fn playurl_data_result_label_does_not_hide_top_level_dash() {
    let payload = json!({"code":0,"data":{
        "result":"suee",
        "quality":80,
        "support_formats":[{"quality":80,"new_description":"720p"}],
        "dash":{"video":[{"id":80,"codecs":"avc1","width":1280,"height":720,"bandwidth":1_000_000}]}
    }});
    let catalog = parse_catalog(part(), &payload).unwrap();
    assert_eq!(catalog.representations.len(), 1);
    assert_eq!(catalog.representations[0].quality, 80);
    assert_eq!(
        (
            catalog.representations[0].width,
            catalog.representations[0].height
        ),
        (Some(1280), Some(720))
    );
}
