use bili_core::*;
fn part() -> PartRef {
    PartRef {
        content: ContentRef::Video(Aid::new(1).unwrap()),
        aid: Aid::new(1).unwrap(),
        cid: Cid::new(2).unwrap(),
        episode: None,
    }
}
fn representation(
    id: u64,
    quality: u32,
    width: u32,
    height: u32,
    codec: &str,
    hdr: &str,
) -> RepresentationKey {
    RepresentationKey {
        part: part(),
        id,
        quality,
        codec: codec.into(),
        width: Some(width),
        height: Some(height),
        fps: Some(Rational {
            numerator: 30,
            denominator: 1,
        }),
        dynamic_range: Some(hdr.into()),
        color: None,
        bandwidth: 1_000_000,
        audio: false,
        sample_rate: None,
        channels: None,
        language: None,
    }
}
fn selection() -> DownloadSelection {
    DownloadSelection {
        part: part(),
        title: "sample".into(),
        quality: None,
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
fn catalog(representations: Vec<RepresentationKey>) -> QualityCatalog {
    QualityCatalog {
        part: part(),
        advertised: vec![
            QualityEntry {
                id: 126,
                label: "Dolby 1080".into(),
                rank: 100,
                availability: Availability::Obtainable,
                requires_login: None,
                requires_vip: None,
                has_video_url: true,
                advertised_codecs: Vec::new(),
            },
            QualityEntry {
                id: 120,
                label: "4K".into(),
                rank: 2,
                availability: Availability::Obtainable,
                requires_login: None,
                requires_vip: None,
                has_video_url: true,
                advertised_codecs: Vec::new(),
            },
            QualityEntry {
                id: 127,
                label: "advertised only".into(),
                rank: 101,
                availability: Availability::Unknown,
                requires_login: None,
                requires_vip: None,
                has_video_url: false,
                advertised_codecs: Vec::new(),
            },
        ],
        representations,
        revision: 1,
        preview: false,
    }
}
#[test]
fn actual_obtainable_4k_beats_high_qn_dolby_1080_default() {
    let catalog = catalog(vec![
        representation(126, 126, 1920, 1080, "hev1", "Dolby Vision"),
        representation(120, 120, 3840, 2160, "avc1", "SDR"),
    ]);
    let (video, _) = catalog.select(&selection()).unwrap();
    assert_eq!((video.width, video.height), (Some(3840), Some(2160)));
    assert_eq!(video.id, 120);
    let mut explicit = selection();
    explicit.quality = Some(126);
    let (video, _) = catalog.select(&explicit).unwrap();
    assert_eq!(video.height, Some(1080));
    explicit.quality = Some(127);
    assert!(catalog.select(&explicit).is_err());
}
#[test]
fn same_numeric_representation_requires_qualified_codec_selection() {
    let catalog = catalog(vec![
        representation(80, 80, 1920, 1080, "avc1", "SDR"),
        representation(80, 80, 1920, 1080, "hev1", "SDR"),
        representation(80, 80, 1920, 1080, "av01", "SDR"),
    ]);
    let mut requested = selection();
    requested.video_representation = Some(80);
    assert!(
        catalog.select(&requested).is_err(),
        "numeric DASH qn is not unique across codecs"
    );
    for codec in ["avc1", "hev1", "av01"] {
        requested.codec = Some(codec.into());
        let (video, _) = catalog.select(&requested).unwrap();
        assert_eq!(video.codec, codec);
        assert_eq!(video.id, 80);
    }
}
#[test]
fn strict_hdr_filter_does_not_substitute_sdr_or_unknown_dimensions() {
    let mut unknown = representation(125, 125, 1, 1, "hev1", "HDR10");
    unknown.width = None;
    unknown.height = None;
    let catalog = catalog(vec![
        representation(120, 120, 3840, 2160, "avc1", "SDR"),
        unknown,
    ]);
    let mut requested = selection();
    requested.dynamic_range = Some("HDR10".into());
    let (video, _) = catalog.select(&requested).unwrap();
    assert_eq!(video.width, None);
    assert_eq!(video.height, None);
    assert_eq!(video.dynamic_range.as_deref(), Some("HDR10"));
    requested.dynamic_range = Some("Dolby Vision".into());
    assert!(catalog.select(&requested).is_err());
}
