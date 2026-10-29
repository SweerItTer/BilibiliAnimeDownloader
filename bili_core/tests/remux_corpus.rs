use bili_core::{remux, StreamTopology};
use ffmpeg_next as ffmpeg;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const TIMING_TOLERANCE_US: i64 = 100_000;

fn corpus() -> PathBuf {
    std::env::var_os("BILI_MEDIA_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/media-fixtures"))
}

fn assert_codecs(probe: &remux::MediaProbe, expected: &[&str]) {
    let mut actual: Vec<_> = probe
        .streams
        .iter()
        .map(|stream| stream.codec.to_ascii_lowercase())
        .collect();
    let mut expected: Vec<_> = expected.iter().map(|codec| codec.to_string()).collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

fn video_stream(probe: &remux::MediaProbe) -> &remux::ProbedStream {
    probe
        .streams
        .iter()
        .find(|stream| stream.media_type == "Video")
        .expect("video stream")
}

fn audio_stream(probe: &remux::MediaProbe) -> &remux::ProbedStream {
    probe
        .streams
        .iter()
        .find(|stream| stream.media_type == "Audio")
        .expect("audio stream")
}

fn assert_video_profile<'a>(probe: &'a remux::MediaProbe, codec: &str) -> &'a remux::ProbedStream {
    let stream = video_stream(probe);
    assert_eq!(stream.codec.to_ascii_lowercase(), codec);
    assert_eq!((stream.width, stream.height), (Some(64), Some(64)));
    stream
}

fn assert_video(probe: &remux::MediaProbe, codec: &str) {
    let stream = assert_video_profile(probe, codec);
    assert_eq!(
        (stream.frame_rate_num, stream.frame_rate_den),
        (Some(12), Some(1))
    );
}

fn assert_audio(probe: &remux::MediaProbe, codec: &str) {
    let stream = audio_stream(probe);
    assert_eq!(stream.codec.to_ascii_lowercase(), codec);
    assert_eq!(
        (stream.sample_rate, stream.channels),
        (Some(48_000), Some(1))
    );
    assert_eq!((stream.width, stream.height), (None, None));
}

fn assert_duration(actual: Option<i64>, expected: i64) {
    let actual = actual.expect("media duration");
    assert!(
        (actual - expected).abs() <= TIMING_TOLERANCE_US,
        "duration {actual}us differs from {expected}us"
    );
}

fn assert_stream_properties_preserved(
    source: &remux::MediaProbe,
    output: &remux::MediaProbe,
    check_video_frame_rate: bool,
) {
    for original in &source.streams {
        let copy = output
            .streams
            .iter()
            .find(|stream| {
                stream.media_type == original.media_type && stream.codec == original.codec
            })
            .expect("matching output stream");
        assert_eq!((copy.width, copy.height), (original.width, original.height));
        assert_eq!(
            (copy.sample_rate, copy.channels),
            (original.sample_rate, original.channels)
        );
        if check_video_frame_rate && original.media_type.eq_ignore_ascii_case("Video") {
            assert_eq!(
                (copy.frame_rate_num, copy.frame_rate_den),
                (original.frame_rate_num, original.frame_rate_den)
            );
        }
        assert_eq!(
            (
                copy.color_range,
                copy.color_primaries,
                copy.color_transfer,
                copy.color_matrix
            ),
            (
                original.color_range,
                original.color_primaries,
                original.color_transfer,
                original.color_matrix
            )
        );
        assert!(
            original
                .side_data
                .iter()
                .all(|kind| copy.side_data.contains(kind)),
            "remux dropped source stream side data"
        );
        for tag in &original.metadata {
            assert!(
                copy.metadata.contains(tag),
                "remux dropped stream metadata {tag:?}"
            );
        }
    }
}

#[derive(Debug)]
struct PacketTrace {
    sha256: [u8; 32],
    pts_us: Option<i64>,
    dts_us: Option<i64>,
}

#[derive(Debug)]
struct StreamTrace {
    index: usize,
    key: String,
    packets: Vec<PacketTrace>,
}

fn timestamp_us(timestamp: Option<i64>, time_base: ffmpeg::Rational) -> Option<i64> {
    let denominator = time_base.denominator();
    if denominator <= 0 {
        return None;
    }
    let timestamp = timestamp?;
    Some(
        ((i128::from(timestamp) * i128::from(time_base.numerator()) * 1_000_000)
            / i128::from(denominator)) as i64,
    )
}

fn packet_traces(path: &Path) -> Vec<StreamTrace> {
    ffmpeg::init().expect("FFmpeg runtime");
    let mut input = ffmpeg::format::input(path).expect("open packet-trace input");
    let mut streams: Vec<_> = input
        .streams()
        .map(|stream| StreamTrace {
            index: stream.index(),
            key: format!(
                "{:?}:{:?}",
                stream.parameters().medium(),
                stream.parameters().id()
            )
            .to_ascii_lowercase(),
            packets: Vec::new(),
        })
        .collect();
    for (stream, packet) in input.packets() {
        let trace = streams
            .iter_mut()
            .find(|trace| trace.index == stream.index())
            .expect("packet stream");
        trace.packets.push(PacketTrace {
            sha256: Sha256::digest(packet.data().unwrap_or(&[])).into(),
            pts_us: timestamp_us(packet.pts(), stream.time_base()),
            dts_us: timestamp_us(packet.dts(), stream.time_base()),
        });
    }
    streams
}

fn packet_hashes(path: &Path) -> BTreeMap<String, Vec<[u8; 32]>> {
    packet_traces(path)
        .into_iter()
        .map(|stream| {
            (
                stream.key,
                stream
                    .packets
                    .into_iter()
                    .map(|packet| packet.sha256)
                    .collect(),
            )
        })
        .collect()
}

fn assert_packet_copy(sources: &[&Path], output: &Path) {
    let mut expected = BTreeMap::<String, Vec<[u8; 32]>>::new();
    for source in sources {
        for (key, hashes) in packet_hashes(source) {
            expected.entry(key).or_default().extend(hashes);
        }
    }
    assert_eq!(
        packet_hashes(output),
        expected,
        "packet-copy output must retain every source packet in source order"
    );
}

fn assert_monotonic_timestamps(stream: &StreamTrace) {
    let pts: Vec<_> = stream
        .packets
        .iter()
        .map(|packet| packet.pts_us.expect("packet PTS"))
        .collect();
    let dts: Vec<_> = stream
        .packets
        .iter()
        .map(|packet| packet.dts_us.expect("packet DTS"))
        .collect();
    assert!(
        pts.windows(2).all(|pair| pair[0] <= pair[1]),
        "{} PTS must be monotonic",
        stream.key
    );
    assert!(
        dts.windows(2).all(|pair| pair[0] <= pair[1]),
        "{} DTS must be monotonic",
        stream.key
    );
}
fn assert_timestamp_shift(
    source: Option<i64>,
    output: Option<i64>,
    offset_us: i64,
    stream: &str,
    field: &str,
    source_duration_us: i64,
    output_duration_us: i64,
) {
    match (source, output) {
        (Some(source), Some(output)) => assert!(
            (output - source - offset_us).abs() <= TIMING_TOLERANCE_US,
            "{stream} {field} source timestamp={source}us, output timestamp={output}us, expected offset={offset_us}us, source container duration={source_duration_us}us, output container duration={output_duration_us}us"
        ),
        (None, None) => {}
        _ => panic!(
            "{stream} {field} timestamp presence changed; source container duration={source_duration_us}us, output container duration={output_duration_us}us"
        ),
    }
}

fn assert_retimed_timestamps(
    source: &StreamTrace,
    output: &[PacketTrace],
    offset_us: i64,
    source_duration_us: i64,
    output_duration_us: i64,
) {
    assert_eq!(
        output.len(),
        source.packets.len(),
        "{} packet count",
        source.key
    );
    for (source_packet, output_packet) in source.packets.iter().zip(output) {
        assert_timestamp_shift(
            source_packet.pts_us,
            output_packet.pts_us,
            offset_us,
            &source.key,
            "PTS",
            source_duration_us,
            output_duration_us,
        );
        assert_timestamp_shift(
            source_packet.dts_us,
            output_packet.dts_us,
            offset_us,
            &source.key,
            "DTS",
            source_duration_us,
            output_duration_us,
        );
    }
}

#[derive(Debug)]
struct DecodedVideo {
    pixel_format: ffmpeg::util::format::Pixel,
    mastering_display_metadata: Option<Vec<u8>>,
    content_light_metadata: Option<Vec<u8>>,
}

fn capture_hdr_side_data(
    frame: &ffmpeg::util::frame::video::Video,
    pixel_format: &mut Option<ffmpeg::util::format::Pixel>,
    mastering: &mut Option<Vec<u8>>,
    light: &mut Option<Vec<u8>>,
) {
    pixel_format.get_or_insert_with(|| frame.format());
    if mastering.is_none() {
        *mastering = frame
            .side_data(ffmpeg::util::frame::side_data::Type::MasteringDisplayMetadata)
            .map(|side| side.data().to_vec());
    }
    if light.is_none() {
        *light = frame
            .side_data(ffmpeg::util::frame::side_data::Type::ContentLightLevel)
            .map(|side| side.data().to_vec());
    }
}

fn decoded_video(path: &Path) -> DecodedVideo {
    ffmpeg::init().expect("FFmpeg runtime");
    let mut input = ffmpeg::format::input(path).expect("open decoded video input");
    let (index, parameters) = {
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .expect("video stream");
        (stream.index(), stream.parameters())
    };
    let context =
        ffmpeg::codec::context::Context::from_parameters(parameters).expect("video parameters");
    let mut decoder = context.decoder().video().expect("enabled video decoder");
    let mut pixel_format = None;
    let mut mastering_display_metadata = None;
    let mut content_light_metadata = None;
    let mut frame = ffmpeg::util::frame::video::Video::empty();
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet).expect("send video packet");
        while decoder.receive_frame(&mut frame).is_ok() {
            capture_hdr_side_data(
                &frame,
                &mut pixel_format,
                &mut mastering_display_metadata,
                &mut content_light_metadata,
            );
            if mastering_display_metadata.is_some() && content_light_metadata.is_some() {
                return DecodedVideo {
                    pixel_format: pixel_format.expect("decoded pixel format"),
                    mastering_display_metadata,
                    content_light_metadata,
                };
            }
        }
    }
    decoder.send_eof().expect("flush video decoder");
    while decoder.receive_frame(&mut frame).is_ok() {
        capture_hdr_side_data(
            &frame,
            &mut pixel_format,
            &mut mastering_display_metadata,
            &mut content_light_metadata,
        );
    }
    DecodedVideo {
        pixel_format: pixel_format.expect("video fixture produced no decoded frame"),
        mastering_display_metadata,
        content_light_metadata,
    }
}

fn stream_side_data(path: &Path, media_type: ffmpeg::media::Type) -> Vec<(String, Vec<u8>)> {
    ffmpeg::init().expect("FFmpeg runtime");
    let input = ffmpeg::format::input(path).expect("open stream side data");
    let stream = input.streams().best(media_type).expect("media stream");
    let mut side_data: Vec<_> = stream
        .side_data()
        .map(|item| (format!("{:?}", item.kind()), item.data().to_vec()))
        .collect();
    side_data.sort();
    side_data
}
fn side_data_payloads_preserved(
    source: &[(String, Vec<u8>)],
    output: &[(String, Vec<u8>)],
) -> bool {
    let mut output = output.iter();
    for required in source {
        let Some(mut observed) = output.next() else {
            return false;
        };
        while observed < required {
            let Some(next) = output.next() else {
                return false;
            };
            observed = next;
        }
        if observed != required {
            return false;
        }
    }
    true
}

// ffmpeg-sys-next 8.1.0 omits these payload types; mirror the public FFmpeg 8.0.3 target header layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct MasteringDisplayMetadata {
    display_primaries: [[ffmpeg::ffi::AVRational; 2]; 3],
    white_point: [ffmpeg::ffi::AVRational; 2],
    min_luminance: ffmpeg::ffi::AVRational,
    max_luminance: ffmpeg::ffi::AVRational,
    has_primaries: std::ffi::c_int,
    has_luminance: std::ffi::c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ContentLightMetadata {
    max_cll: std::ffi::c_uint,
    max_fall: std::ffi::c_uint,
}

fn mastering_side_data(data: &[u8]) -> MasteringDisplayMetadata {
    assert_eq!(
        data.len(),
        std::mem::size_of::<MasteringDisplayMetadata>(),
        "mastering metadata size must match the pinned FFmpeg payload type"
    );
    unsafe {
        data.as_ptr()
            .cast::<MasteringDisplayMetadata>()
            .read_unaligned()
    }
}

fn content_light_side_data(data: &[u8]) -> ContentLightMetadata {
    assert_eq!(
        data.len(),
        std::mem::size_of::<ContentLightMetadata>(),
        "content-light metadata size must match the pinned FFmpeg payload type"
    );
    unsafe {
        data.as_ptr()
            .cast::<ContentLightMetadata>()
            .read_unaligned()
    }
}

fn rational_equals(value: ffmpeg::ffi::AVRational, numerator: i32, denominator: i32) -> bool {
    value.den != 0
        && i64::from(value.num) * i64::from(denominator)
            == i64::from(numerator) * i64::from(value.den)
}

#[derive(Debug)]
struct DecodedFrameTrace {
    width: u32,
    height: u32,
    pixel_format: ffmpeg::util::format::Pixel,
    active_pixels: Vec<u8>,
}

fn decoded_frame_trace(frame: &ffmpeg::util::frame::video::Video) -> DecodedFrameTrace {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let pixel_format = frame.format();
    assert_eq!((width, height), (64, 64), "decoded corpus frame dimensions");
    assert_eq!(
        pixel_format,
        ffmpeg::util::format::Pixel::YUV420P,
        "decoded corpus frame pixel format"
    );
    assert_eq!(frame.planes(), 3, "YUV420P frame plane count");

    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let mut active_pixels = Vec::with_capacity(width * height + 2 * chroma_width * chroma_height);
    for (plane_index, row_width, row_count) in [
        (0, width, height),
        (1, chroma_width, chroma_height),
        (2, chroma_width, chroma_height),
    ] {
        let plane = frame.data(plane_index);
        let stride = frame.stride(plane_index);
        for row in 0..row_count {
            let start = row * stride;
            active_pixels.extend_from_slice(&plane[start..start + row_width]);
        }
    }
    DecodedFrameTrace {
        width: width as u32,
        height: height as u32,
        pixel_format,
        active_pixels,
    }
}

fn receive_decoded_frames(
    decoder: &mut ffmpeg::decoder::Video,
    frames: &mut Vec<DecodedFrameTrace>,
) {
    let mut frame = ffmpeg::util::frame::video::Video::empty();
    while decoder.receive_frame(&mut frame).is_ok() {
        frames.push(decoded_frame_trace(&frame));
    }
}

fn decoded_video_frames(path: &Path) -> Vec<DecodedFrameTrace> {
    ffmpeg::init().expect("FFmpeg runtime");
    let mut input = ffmpeg::format::input(path).expect("open decoded video source");
    let (stream_index, parameters) = {
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .expect("video stream");
        (stream.index(), stream.parameters())
    };
    let context = ffmpeg::codec::context::Context::from_parameters(parameters)
        .expect("video codec parameters");
    let mut decoder = context.decoder().video().expect("video decoder");
    let mut frames = Vec::new();
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        decoder.send_packet(&packet).expect("send video packet");
        receive_decoded_frames(&mut decoder, &mut frames);
    }
    decoder.send_eof().expect("flush video decoder");
    receive_decoded_frames(&mut decoder, &mut frames);
    frames
}

#[test]
fn dash_packet_copy_preserves_concrete_stream_profiles_timing_and_payloads() {
    let root = corpus();
    let video = root.join("dash-video-1.mp4");
    let audio = root.join("dash-audio.m4a");
    assert!(
        video.is_file() && audio.is_file(),
        "native media fixtures are required"
    );
    let video_probe = remux::probe(&video).expect("probe DASH video input");
    let audio_probe = remux::probe(&audio).expect("probe DASH audio input");
    assert_codecs(&video_probe, &["h264"]);
    assert_codecs(&audio_probe, &["aac"]);
    assert_video(&video_probe, "h264");
    assert_audio(&audio_probe, "aac");

    let out_dir = tempfile::tempdir().unwrap();
    let output = out_dir.path().join("dash.mp4");
    let result = remux::remux(
        &[video.clone(), audio.clone()],
        StreamTopology::Dash,
        &output,
        &CancellationToken::new(),
    )
    .expect("DASH packet-copy remux");
    assert_eq!(result.format.as_deref(), Some("mov"));
    assert_codecs(&result, &["h264", "aac"]);
    assert_video(&result, "h264");
    assert_audio(&result, "aac");
    assert_stream_properties_preserved(&video_probe, &result, true);
    assert_stream_properties_preserved(&audio_probe, &result, true);
    assert_duration(
        result.duration_us,
        video_probe
            .duration_us
            .unwrap()
            .max(audio_probe.duration_us.unwrap()),
    );
    assert_packet_copy(&[&video, &audio], &output);

    let unused_video =
        remux::probe(&root.join("dash-video-2.mp4")).expect("probe second DASH video fixture");
    assert_codecs(&unused_video, &["h264"]);
    assert_video(&unused_video, "h264");
}

#[test]
fn single_file_and_legacy_flv_remux_retain_codec_profiles_and_packets() {
    let root = corpus();
    let out_dir = tempfile::tempdir().unwrap();
    for name in ["h264-aac.mp4", "legacy.flv"] {
        let input = root.join(name);
        assert!(input.is_file(), "native media fixtures are required");
        let source = remux::probe(&input).expect("probe local source");
        assert_codecs(&source, &["h264", "aac"]);
        assert_video(&source, "h264");
        assert_audio(&source, "aac");
        let output = out_dir.path().join(format!("{name}.mp4"));
        let result = remux::remux(
            std::slice::from_ref(&input),
            StreamTopology::Direct,
            &output,
            &CancellationToken::new(),
        )
        .expect("direct packet-copy remux");
        assert_eq!(result.format.as_deref(), Some("mov"));
        assert_codecs(&result, &["h264", "aac"]);
        assert_stream_properties_preserved(&source, &result, true);
        assert_duration(result.duration_us, source.duration_us.unwrap());
        assert_packet_copy(&[&input], &output);
    }
}

#[test]
fn hevc_ten_bit_hdr_remux_preserves_color_and_mastering_metadata() {
    let root = corpus();
    let input = root.join("hevc-hdr.mp4");
    assert!(input.is_file(), "native media fixtures are required");
    let source = remux::probe(&input).expect("probe HEVC HDR source");
    assert_codecs(&source, &["hevc"]);
    assert_video(&source, "hevc");
    let video = video_stream(&source);
    assert_eq!(
        (
            video.color_primaries,
            video.color_transfer,
            video.color_matrix
        ),
        (Some(9), Some(16), Some(9))
    );
    let source_video = decoded_video(&input);
    assert_eq!(
        source_video.pixel_format,
        ffmpeg::util::format::Pixel::YUV420P10LE
    );
    let source_mastering = source_video
        .mastering_display_metadata
        .as_deref()
        .expect("decoded-frame mastering display metadata");
    let source_light = source_video
        .content_light_metadata
        .as_deref()
        .expect("decoded-frame MaxCLL/MaxFALL metadata");

    let out_dir = tempfile::tempdir().unwrap();
    let output = out_dir.path().join("hevc-hdr.mp4");
    let result = remux::remux(
        std::slice::from_ref(&input),
        StreamTopology::Direct,
        &output,
        &CancellationToken::new(),
    )
    .expect("HEVC HDR packet-copy remux");
    assert_eq!(result.format.as_deref(), Some("mov"));
    assert_codecs(&result, &["hevc"]);
    assert_stream_properties_preserved(&source, &result, true);
    let output_video = decoded_video(&output);
    assert_eq!(
        output_video.pixel_format,
        ffmpeg::util::format::Pixel::YUV420P10LE
    );
    let output_mastering = output_video
        .mastering_display_metadata
        .as_deref()
        .expect("remuxed decoded-frame mastering display metadata");
    let output_light = output_video
        .content_light_metadata
        .as_deref()
        .expect("remuxed decoded-frame MaxCLL/MaxFALL metadata");
    assert_eq!(
        output_mastering, source_mastering,
        "remux must preserve decoded-frame mastering metadata byte-for-byte"
    );
    assert_eq!(
        output_light, source_light,
        "remux must preserve decoded-frame content-light metadata byte-for-byte"
    );
    assert_duration(result.duration_us, source.duration_us.unwrap());
    assert_packet_copy(&[&input], &output);

    let source_side_data = stream_side_data(&input, ffmpeg::media::Type::Video);
    let output_side_data = stream_side_data(&output, ffmpeg::media::Type::Video);
    assert_eq!(
        output_side_data, source_side_data,
        "remux must preserve any stream-level HDR side data byte-for-byte"
    );
    let source_mastering = mastering_side_data(source_mastering);
    let output_mastering = mastering_side_data(output_mastering);
    let source_light = content_light_side_data(source_light);
    let output_light = content_light_side_data(output_light);
    for metadata in [&source_mastering, &output_mastering] {
        assert_eq!(
            metadata.has_primaries, 1,
            "mastering chromaticities must be present"
        );
        assert_eq!(
            metadata.has_luminance, 1,
            "mastering luminance must be present"
        );
        assert!(rational_equals(
            metadata.display_primaries[0][0],
            34_000,
            50_000
        ));
        assert!(rational_equals(
            metadata.display_primaries[0][1],
            16_000,
            50_000
        ));
        assert!(rational_equals(
            metadata.display_primaries[1][0],
            13_250,
            50_000
        ));
        assert!(rational_equals(
            metadata.display_primaries[1][1],
            34_500,
            50_000
        ));
        assert!(rational_equals(
            metadata.display_primaries[2][0],
            7_500,
            50_000
        ));
        assert!(rational_equals(
            metadata.display_primaries[2][1],
            3_000,
            50_000
        ));
        assert!(rational_equals(metadata.white_point[0], 15_635, 50_000));
        assert!(rational_equals(metadata.white_point[1], 16_450, 50_000));
        assert!(rational_equals(metadata.max_luminance, 1_000, 1));
        assert!(rational_equals(metadata.min_luminance, 1, 10_000));
    }
    for metadata in [&source_light, &output_light] {
        assert_eq!((metadata.max_cll, metadata.max_fall), (1_000, 400));
    }
}

#[test]
fn av1_stream_is_probed_and_packet_copied_without_a_decoder() {
    let root = corpus();
    let input = root.join("av1.mp4");
    assert!(input.is_file(), "native media fixtures are required");
    let source = remux::probe(&input).expect("probe AV1 container and sequence header");
    assert_codecs(&source, &["av1"]);
    assert_video(&source, "av1");
    let out_dir = tempfile::tempdir().unwrap();
    let output = out_dir.path().join("av1.mp4");
    let result = remux::remux(
        std::slice::from_ref(&input),
        StreamTopology::Direct,
        &output,
        &CancellationToken::new(),
    )
    .expect("AV1 packet-copy remux without decoder");
    assert_eq!(result.format.as_deref(), Some("mov"));
    assert_codecs(&result, &["av1"]);
    assert_stream_properties_preserved(&source, &result, true);
    assert_duration(result.duration_us, source.duration_us.unwrap());
    assert_packet_copy(&[&input], &output);
}

#[test]
fn flac_ac3_and_eac3_audio_streams_are_probed_and_packet_copied() {
    let root = corpus();
    let out_dir = tempfile::tempdir().unwrap();
    for (name, codec) in [
        ("audio-flac.flac", "flac"),
        ("audio-ac3.ac3", "ac3"),
        ("audio-eac3.eac3", "eac3"),
    ] {
        let input = root.join(name);
        assert!(input.is_file(), "native media fixtures are required");
        let source = remux::probe(&input).expect("probe standalone audio");
        assert_codecs(&source, &[codec]);
        assert_audio(&source, codec);
        let output = out_dir.path().join(format!("{name}.mp4"));
        let remuxed = remux::remux(
            std::slice::from_ref(&input),
            StreamTopology::Direct,
            &output,
            &CancellationToken::new(),
        );
        if let Err(error) = &remuxed {
            if error.operation.contains("side-data") && output.exists() {
                let target = remux::probe(&output).expect("probe output for side-data diagnostics");
                let source_side = stream_side_data(&input, ffmpeg::media::Type::Audio);
                let target_side = stream_side_data(&output, ffmpeg::media::Type::Audio);
                let source_types: Vec<_> =
                    source_side.iter().map(|(kind, _)| kind.as_str()).collect();
                let target_types: Vec<_> =
                    target_side.iter().map(|(kind, _)| kind.as_str()).collect();
                let source_lengths: Vec<_> =
                    source_side.iter().map(|(_, data)| data.len()).collect();
                let target_lengths: Vec<_> =
                    target_side.iter().map(|(_, data)| data.len()).collect();
                let source_audio = audio_stream(&source);
                let target_audio = audio_stream(&target);
                panic!(
                    "packet-copy verification codec={codec}, operation={}, source-side-data-count={}, source-side-data={source_types:?}/{source_lengths:?}, output-side-data-count={}, output-side-data={target_types:?}/{target_lengths:?}, all-source-payload-tuples-preserved={}, source-avg-frame-rate={:?}/{:?}, output-avg-frame-rate={:?}/{:?}",
                    error.operation, source_side.len(), target_side.len(), side_data_payloads_preserved(&source_side, &target_side),
                    source_audio.frame_rate_num, source_audio.frame_rate_den,
                    target_audio.frame_rate_num, target_audio.frame_rate_den,
                );
            }
        }
        let result = remuxed.expect("audio packet-copy remux");
        assert_eq!(result.format.as_deref(), Some("mov"));
        assert_codecs(&result, &[codec]);
        assert_stream_properties_preserved(&source, &result, true);
        assert_duration(result.duration_us, source.duration_us.unwrap());
        assert_packet_copy(&[&input], &output);
    }
}

#[test]
fn sequential_flv_remux_retains_audio_and_video_order_and_retimes_both_streams() {
    let root = corpus();
    let first = root.join("segment-01.flv");
    let second = root.join("segment-02.flv");
    assert!(
        first.is_file() && second.is_file(),
        "native media fixtures are required"
    );
    let first_probe = remux::probe(&first).expect("probe first legacy segment");
    let second_probe = remux::probe(&second).expect("probe second legacy segment");
    // Use the observed input-container duration as the concat boundary instead of a nominal fixture timestamp.
    let first_boundary_offset_us = first_probe
        .duration_us
        .expect("first source container duration");
    let second_source_duration_us = second_probe
        .duration_us
        .expect("second source container duration");
    assert_codecs(&first_probe, &["h264", "aac"]);
    assert_codecs(&second_probe, &["h264", "aac"]);
    assert_video(&first_probe, "h264");
    assert_audio(&first_probe, "aac");
    assert_video(&second_probe, "h264");
    assert_audio(&second_probe, "aac");

    let first_frames = decoded_video_frames(&first);
    let second_frames = decoded_video_frames(&second);
    assert!(!first_frames.is_empty() && !second_frames.is_empty());
    assert!(
        first_frames[0].active_pixels != second_frames[0].active_pixels,
        "the red and blue source segments must have distinct decoded pixels"
    );
    let out_dir = tempfile::tempdir().unwrap();
    let source_bytes = [
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
    ];
    let input_dir = out_dir.path().join("concat inputs' directory");
    std::fs::create_dir(&input_dir).unwrap();
    let first_input = input_dir.join("segment '01.flv");
    #[cfg(unix)]
    let second_input = input_dir.join("segment \\ '02.flv");
    #[cfg(not(unix))]
    let second_input = input_dir.join("segment '02.flv");
    std::fs::copy(&first, &first_input).unwrap();
    std::fs::copy(&second, &second_input).unwrap();
    let inputs = [first_input, second_input];
    let output = out_dir.path().join("legacy.mp4");
    let remuxed = remux::remux(
        &inputs,
        StreamTopology::Sequential,
        &output,
        &CancellationToken::new(),
    );
    if let Err(error) = &remuxed {
        if error.operation.contains("frame-rate") && output.exists() {
            let target = remux::probe(&output).expect("probe output for frame-rate diagnostics");
            let first_video = video_stream(&first_probe);
            let second_video = video_stream(&second_probe);
            let target_video = video_stream(&target);
            let source_side = stream_side_data(&first, ffmpeg::media::Type::Video);
            let target_side = stream_side_data(&output, ffmpeg::media::Type::Video);
            let source_types: Vec<_> = source_side.iter().map(|(kind, _)| kind.as_str()).collect();
            let target_types: Vec<_> = target_side.iter().map(|(kind, _)| kind.as_str()).collect();
            let source_lengths: Vec<_> = source_side.iter().map(|(_, data)| data.len()).collect();
            let target_lengths: Vec<_> = target_side.iter().map(|(_, data)| data.len()).collect();
            panic!(
                "sequential remux verification operation={}, source avg-frame-rates={:?}/{:?},{:?}/{:?}, output avg-frame-rate={:?}/{:?}, source-side-data-count={}, source-side-data={source_types:?}/{source_lengths:?}, output-side-data-count={}, output-side-data={target_types:?}/{target_lengths:?}, all-source-payload-tuples-preserved={}",
                error.operation, first_video.frame_rate_num, first_video.frame_rate_den,
                second_video.frame_rate_num, second_video.frame_rate_den,
                target_video.frame_rate_num, target_video.frame_rate_den,
                source_side.len(), target_side.len(), side_data_payloads_preserved(&source_side, &target_side),
            );
        }
    }
    let result = remuxed.expect("ordered local concat remux");
    assert_eq!(result.format.as_deref(), Some("mov"));
    assert_eq!(
        result.streams.len(),
        2,
        "each segment must continue the same audio/video streams"
    );
    assert_codecs(&result, &["h264", "aac"]);
    assert_video_profile(&result, "h264");
    assert_audio(&result, "aac");
    let actual_us = result.duration_us.expect("output duration");
    assert!(
        (actual_us - 1_100_000).abs() <= TIMING_TOLERANCE_US,
        "output container duration={actual_us}us differs from expected 1100000us; source container durations: first={first_boundary_offset_us}us, second={second_source_duration_us}us"
    );
    assert!(
        actual_us >= first_boundary_offset_us + second_source_duration_us - Duration::from_millis(100).as_micros() as i64,
        "output container duration={actual_us}us is shorter than source durations first={first_boundary_offset_us}us plus second={second_source_duration_us}us within 100ms"
    );
    assert_stream_properties_preserved(&first_probe, &result, false);

    let output_frames = decoded_video_frames(&output);
    assert_eq!(
        output_frames.len(),
        first_frames.len() + second_frames.len(),
        "output must decode exactly the source frames"
    );
    for (index, (output_frame, source_frame)) in output_frames
        .iter()
        .zip(first_frames.iter().chain(second_frames.iter()))
        .enumerate()
    {
        assert_eq!(
            (
                output_frame.width,
                output_frame.height,
                output_frame.pixel_format
            ),
            (
                source_frame.width,
                source_frame.height,
                source_frame.pixel_format
            ),
            "decoded frame {index} profile and sequence",
        );
        assert!(
            output_frame.active_pixels == source_frame.active_pixels,
            "decoded frame {index} active pixels must match first-source-then-second-source order"
        );
    }

    let first_packets = packet_traces(&first);
    let second_packets = packet_traces(&second);
    let output_packets = packet_traces(&output);
    let first_audio: Vec<_> = first_packets
        .iter()
        .find(|stream| stream.key == "audio:aac")
        .unwrap()
        .packets
        .iter()
        .map(|packet| packet.sha256)
        .collect();
    let second_audio: Vec<_> = second_packets
        .iter()
        .find(|stream| stream.key == "audio:aac")
        .unwrap()
        .packets
        .iter()
        .map(|packet| packet.sha256)
        .collect();
    assert!(
        first_audio != second_audio,
        "the 440 Hz and 880 Hz segments must contain distinct audio payloads"
    );
    let expected_audio: Vec<_> = first_audio.iter().chain(&second_audio).copied().collect();
    let output_audio: Vec<_> = output_packets
        .iter()
        .find(|stream| stream.key == "audio:aac")
        .expect("output audio packet stream")
        .packets
        .iter()
        .map(|packet| packet.sha256)
        .collect();
    assert!(
        output_audio == expected_audio,
        "AAC output must preserve every source packet payload and its exact order"
    );
    for output_stream in &output_packets {
        assert_monotonic_timestamps(output_stream);
        let first_stream = first_packets
            .iter()
            .find(|stream| stream.key == output_stream.key)
            .expect("first segment stream");
        let second_stream = second_packets
            .iter()
            .find(|stream| stream.key == output_stream.key)
            .expect("second segment stream");
        let split = first_stream.packets.len();
        assert!(split > 0 && split < output_stream.packets.len());
        assert_retimed_timestamps(
            first_stream,
            &output_stream.packets[..split],
            0,
            first_boundary_offset_us,
            actual_us,
        );
        assert_retimed_timestamps(
            second_stream,
            &output_stream.packets[split..],
            first_boundary_offset_us,
            second_source_duration_us,
            actual_us,
        );
        assert!(
            output_stream.packets[split].pts_us.unwrap()
                > output_stream.packets[split - 1].pts_us.unwrap()
        );
    }
    assert_eq!(std::fs::read(&inputs[0]).unwrap(), source_bytes[0]);
    assert_eq!(std::fs::read(&inputs[1]).unwrap(), source_bytes[1]);
}

#[test]
fn unsupported_matroska_remux_fails_without_modifying_or_removing_its_input() {
    let root = corpus();
    let input = root.join("unsupported.mkv");
    assert!(input.is_file(), "native media fixtures are required");
    let original_hash: [u8; 32] =
        Sha256::digest(std::fs::read(&input).expect("read fixture")).into();
    let out_dir = tempfile::tempdir().unwrap();
    let output = out_dir.path().join("unsupported.mp4");
    let error = remux::remux(
        std::slice::from_ref(&input),
        StreamTopology::Direct,
        &output,
        &CancellationToken::new(),
    )
    .expect_err("unsupported container must fail");
    assert_eq!(error.code, bili_core::ErrorCode::Remux);
    assert_eq!(error.operation, "packet-copy input open");
    let native_code = error.native_code.expect("native FFmpeg failure code");
    assert!(error
        .to_string()
        .contains(&format!("FFmpeg code {native_code}")));
    let serialized = serde_json::to_value(&error).expect("serialize native failure");
    assert_eq!(
        serialized["native_code"].as_i64(),
        Some(i64::from(native_code))
    );
    assert_eq!(serialized["operation"], "packet-copy input open");
    let restored: bili_core::CoreError =
        serde_json::from_value(serialized).expect("deserialize native failure");
    assert_eq!(restored.native_code, Some(native_code));
    assert!(
        !output.exists(),
        "unsupported container must not leave a partial output"
    );
    assert!(input.is_file(), "unsupported source must be retained");
    let retained_hash: [u8; 32] =
        Sha256::digest(std::fs::read(&input).expect("read retained source")).into();
    assert_eq!(retained_hash, original_hash);
}
