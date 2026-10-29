//! Local, packet-copy media remux through the pinned shared FFmpeg libraries.
use crate::domain::{CoreError, ErrorCode, RepresentationKey, Result, StreamPlan, StreamTopology};
use ffmpeg_next as ffmpeg;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbedStream {
    pub index: usize,
    pub codec: String,
    pub media_type: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub color_range: Option<i32>,
    pub color_primaries: Option<i32>,
    pub color_transfer: Option<i32>,
    pub color_matrix: Option<i32>,
    pub frame_rate_num: Option<i32>,
    pub frame_rate_den: Option<i32>,
    pub time_base_num: i32,
    pub time_base_den: i32,
    pub start_time: Option<i64>,
    pub duration: Option<i64>,
    pub metadata: Vec<(String, String)>,
    pub side_data: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaProbe {
    pub format: Option<String>,
    pub duration_us: Option<i64>,
    pub streams: Vec<ProbedStream>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeDiagnostics {
    pub ffmpeg_version: String,
    pub libraries: Vec<(String, u32)>,
    pub configurations: Vec<(String, String)>,
    pub licenses: Vec<(String, String)>,
    pub formats: Vec<String>,
    pub codecs: Vec<String>,
}

fn native_error() -> CoreError {
    CoreError::new(ErrorCode::NativeCapability, "local media operation")
}
fn remux_error() -> CoreError {
    CoreError::new(ErrorCode::Remux, "packet-copy remux")
}
fn ffmpeg_error(code: ErrorCode, operation: &'static str, error: ffmpeg::Error) -> CoreError {
    let mut core = CoreError::new(code, operation);
    core.native_code = Some(error.into());
    core
}
fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(CoreError::new(ErrorCode::Cancelled, "remux cancelled"))
    } else {
        Ok(())
    }
}

/// Report loaded shared-library versions, effective configurations, licenses and capabilities.
pub fn diagnostics() -> Result<NativeDiagnostics> {
    use std::{collections::BTreeSet, ffi::CStr, ptr};
    ffmpeg::init().map_err(|error| {
        ffmpeg_error(ErrorCode::NativeCapability, "FFmpeg initialization", error)
    })?;
    let raw_version = unsafe { CStr::from_ptr(ffmpeg::ffi::av_version_info()) };
    let ffmpeg_version = raw_version.to_str().map_err(|_| native_error())?.to_owned();
    let versions = [
        ("avutil", ffmpeg::util::version()),
        ("avcodec", ffmpeg::codec::version()),
        ("avformat", ffmpeg::format::version()),
    ];
    let mut formats = BTreeSet::new();
    let mut opaque = ptr::null_mut();
    loop {
        let format = unsafe { ffmpeg::ffi::av_demuxer_iterate(&mut opaque) };
        if format.is_null() {
            break;
        }
        formats.insert(
            unsafe { CStr::from_ptr((*format).name) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    let mut opaque = ptr::null_mut();
    loop {
        let format = unsafe { ffmpeg::ffi::av_muxer_iterate(&mut opaque) };
        if format.is_null() {
            break;
        }
        formats.insert(
            unsafe { CStr::from_ptr((*format).name) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    let mut codecs = BTreeSet::new();
    let mut opaque = ptr::null_mut();
    loop {
        let codec = unsafe { ffmpeg::ffi::av_codec_iterate(&mut opaque) };
        if codec.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*codec).name) }
            .to_string_lossy()
            .into_owned();
        codecs.insert(name);
    }
    Ok(NativeDiagnostics {
        ffmpeg_version,
        libraries: versions
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
        configurations: vec![
            ("avutil".into(), ffmpeg::util::configuration().into()),
            ("avcodec".into(), ffmpeg::codec::configuration().into()),
            ("avformat".into(), ffmpeg::format::configuration().into()),
        ],
        licenses: vec![
            ("avutil".into(), ffmpeg::util::license().into()),
            ("avcodec".into(), ffmpeg::codec::license().into()),
            ("avformat".into(), ffmpeg::format::license().into()),
        ],
        formats: formats.into_iter().collect(),
        codecs: codecs.into_iter().collect(),
    })
}

pub fn probe(path: &Path) -> Result<MediaProbe> {
    ffmpeg::init().map_err(|error| {
        ffmpeg_error(ErrorCode::NativeCapability, "FFmpeg initialization", error)
    })?;
    let input = ffmpeg::format::input(path)
        .map_err(|error| ffmpeg_error(ErrorCode::Remux, "packet-copy input probe", error))?;
    probe_input(&input)
}

fn probe_input(input: &ffmpeg::format::context::Input) -> Result<MediaProbe> {
    let format = input.format().name().split(',').next().map(str::to_owned);
    let duration_us = (input.duration() > 0).then_some(input.duration());
    let streams = input
        .streams()
        .map(|stream| probe_stream(&stream))
        .collect::<Vec<_>>();
    if streams.is_empty() {
        return Err(remux_error());
    }
    Ok(MediaProbe {
        format,
        duration_us,
        streams,
    })
}

fn probe_stream(stream: &ffmpeg::Stream<'_>) -> ProbedStream {
    let parameters = stream.parameters();
    let (width, height, sample_rate, channels, range, primaries, transfer, matrix) = unsafe {
        let parameters = &*parameters.as_ptr();
        (
            (parameters.width > 0).then_some(parameters.width),
            (parameters.height > 0).then_some(parameters.height),
            (parameters.sample_rate > 0).then_some(parameters.sample_rate),
            (parameters.ch_layout.nb_channels > 0).then_some(parameters.ch_layout.nb_channels),
            parameters.color_range as i32,
            parameters.color_primaries as i32,
            parameters.color_trc as i32,
            parameters.color_space as i32,
        )
    };
    let rate = stream.avg_frame_rate();
    let mut side_data: Vec<_> = stream
        .side_data()
        .map(|side_data| format!("{:?}", side_data.kind()))
        .collect();
    side_data.sort();
    ProbedStream {
        index: stream.index(),
        codec: format!("{:?}", parameters.id()),
        media_type: format!("{:?}", parameters.medium()),
        width,
        height,
        sample_rate,
        channels,
        color_range: (range != 0).then_some(range),
        color_primaries: (primaries != 0).then_some(primaries),
        color_transfer: (transfer != 0).then_some(transfer),
        color_matrix: (matrix != 0).then_some(matrix),
        frame_rate_num: (rate.numerator() > 0).then_some(rate.numerator()),
        frame_rate_den: (rate.denominator() > 0).then_some(rate.denominator()),
        time_base_num: stream.time_base().numerator(),
        time_base_den: stream.time_base().denominator(),
        start_time: (stream.start_time() != ffmpeg::ffi::AV_NOPTS_VALUE)
            .then_some(stream.start_time()),
        duration: (stream.duration() >= 0).then_some(stream.duration()),
        metadata: stream
            .metadata()
            .iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        side_data,
    }
}

/// Packet-copy local media into an MP4 container. Inputs are never modified.
/// Sequential legacy segments use FFmpeg's local concat demuxer, which enforces
/// compatible stream layouts and advances timestamps across segments.
pub fn remux(
    inputs: &[PathBuf],
    topology: StreamTopology,
    output: &Path,
    cancel: &CancellationToken,
) -> Result<MediaProbe> {
    use std::io::Write;
    check_cancel(cancel)?;
    ffmpeg::init().map_err(|error| {
        ffmpeg_error(ErrorCode::NativeCapability, "FFmpeg initialization", error)
    })?;
    if inputs.is_empty() || (topology == StreamTopology::Dash && inputs.len() != 2) {
        return Err(remux_error());
    }
    if output.to_string_lossy().contains("://")
        || output.exists()
        || !output.parent().unwrap_or_else(|| Path::new(".")).is_dir()
    {
        return Err(CoreError::new(
            ErrorCode::Conflict,
            "local remux destination",
        ));
    }
    let mut concat_file = None;
    let mut sources = Vec::new();
    if topology == StreamTopology::Sequential {
        let mut list =
            tempfile::NamedTempFile::new_in(output.parent().unwrap_or_else(|| Path::new(".")))
                .map_err(CoreError::io)?;
        for path in inputs {
            check_cancel(cancel)?;
            let local = path.canonicalize().map_err(CoreError::io)?;
            if !local.is_file() {
                return Err(remux_error());
            }
            let item = local.to_str().ok_or_else(remux_error)?;
            if item.bytes().any(|byte| byte == b'\n' || byte == b'\r') {
                return Err(remux_error());
            }
            list.write_all(b"file '").map_err(CoreError::io)?;
            let mut chunks = item.split('\'');
            if let Some(chunk) = chunks.next() {
                list.write_all(chunk.as_bytes()).map_err(CoreError::io)?;
            }
            for chunk in chunks {
                list.write_all(b"'\\''").map_err(CoreError::io)?;
                list.write_all(chunk.as_bytes()).map_err(CoreError::io)?;
            }
            list.write_all(b"'\n").map_err(CoreError::io)?;
        }
        list.flush().map_err(CoreError::io)?;
        let mut options = ffmpeg::Dictionary::new();
        options.set("safe", "0");
        options.set("protocol_whitelist", "file");
        let name = std::ffi::CString::new("concat").expect("static demuxer name");
        let ptr = unsafe { ffmpeg::ffi::av_find_input_format(name.as_ptr()) };
        if ptr.is_null() {
            return Err(native_error());
        }
        let format =
            ffmpeg::Format::Input(unsafe { ffmpeg::format::format::Input::wrap(ptr as *mut _) });
        let context = ffmpeg::format::open_with(list.path(), &format, options)
            .map_err(|error| ffmpeg_error(ErrorCode::Remux, "packet-copy concat open", error))?;
        sources.push(context.input());
        concat_file = Some(list);
    } else {
        for path in inputs {
            check_cancel(cancel)?;
            let local = path.canonicalize().map_err(CoreError::io)?;
            if !local.is_file() {
                return Err(remux_error());
            }
            sources.push(ffmpeg::format::input(&local).map_err(|error| {
                ffmpeg_error(ErrorCode::Remux, "packet-copy input open", error)
            })?);
        }
    }
    let mut output_ctx = ffmpeg::format::output(output)
        .map_err(|error| ffmpeg_error(ErrorCode::Remux, "packet-copy output open", error))?;
    let mut maps = Vec::new();
    let mut output_index = 0usize;
    for (source_index, source) in sources.iter().enumerate() {
        for stream in source.streams() {
            let medium = stream.parameters().medium();
            if topology == StreamTopology::Dash
                && ((source_index == 0 && medium != ffmpeg::media::Type::Video)
                    || (source_index == 1 && medium != ffmpeg::media::Type::Audio))
            {
                continue;
            }
            let parameters = stream.parameters();
            let profile = probe_stream(&stream);
            let mut target = output_ctx
                .add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))
                .map_err(|error| {
                    ffmpeg_error(ErrorCode::Remux, "packet-copy stream setup", error)
                })?;
            // FFmpeg 8 stores stream-global side data in codec parameters; this supported copy preserves it.
            target.set_parameters(parameters);
            unsafe {
                (*(*target.as_mut_ptr()).codecpar).codec_tag = 0;
            }
            target.set_time_base(stream.time_base());
            let mut metadata = ffmpeg::Dictionary::new();
            for (key, value) in stream.metadata().iter() {
                metadata.set(key, value);
            }
            target.set_metadata(metadata);
            // AVStream disposition is stream-level metadata not copied with codec parameters.
            unsafe {
                (*target.as_mut_ptr()).disposition = stream.disposition().bits();
            }
            maps.push((
                source_index,
                stream.index(),
                output_index,
                stream.time_base(),
                profile,
            ));
            output_index += 1;
        }
    }
    if maps.is_empty() {
        return Err(remux_error());
    }
    output_ctx
        .write_header()
        .map_err(|error| ffmpeg_error(ErrorCode::Remux, "packet-copy output header", error))?;
    let mut packets: Vec<Option<ffmpeg::Packet>> = (0..sources.len()).map(|_| None).collect();
    let mut ended = vec![false; sources.len()];
    loop {
        check_cancel(cancel)?;
        for i in 0..sources.len() {
            if ended[i] || packets[i].is_some() {
                continue;
            }
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut sources[i]) {
                Ok(()) => packets[i] = Some(packet),
                Err(ffmpeg::Error::Eof) => ended[i] = true,
                Err(error) => {
                    return Err(ffmpeg_error(
                        ErrorCode::Remux,
                        "packet-copy input read",
                        error,
                    ))
                }
            }
        }
        if ended.iter().all(|v| *v) && packets.iter().all(Option::is_none) {
            break;
        }
        let selected = packets
            .iter()
            .enumerate()
            .filter_map(|(i, packet)| {
                packet.as_ref().map(|p| {
                    let time_base = sources[i]
                        .stream(p.stream())
                        .map(|s| s.time_base())
                        .unwrap_or((1, 1).into());
                    let dts = p.dts().or_else(|| p.pts()).unwrap_or(i64::MAX);
                    (
                        i,
                        dts as i128 * i128::from(time_base.numerator()) * 1_000_000
                            / i128::from(time_base.denominator().max(1)),
                    )
                })
            })
            .min_by_key(|(_, timestamp)| *timestamp)
            .map(|(i, _)| i);
        let Some(i) = selected else {
            break;
        };
        let Some(mut packet) = packets[i].take() else {
            return Err(remux_error());
        };
        if let Some((_, _, target_index, source_time_base, _)) = maps
            .iter()
            .find(|(src, stream, _, _, _)| *src == i && *stream == packet.stream())
        {
            packet.set_stream(*target_index);
            let output_time_base = output_ctx
                .stream(*target_index)
                .ok_or_else(remux_error)?
                .time_base();
            packet.rescale_ts(*source_time_base, output_time_base);
            packet.write_interleaved(&mut output_ctx).map_err(|error| {
                ffmpeg_error(ErrorCode::Remux, "packet-copy output packet", error)
            })?;
        }
    }
    check_cancel(cancel)?;
    output_ctx
        .write_trailer()
        .map_err(|error| ffmpeg_error(ErrorCode::Remux, "packet-copy output trailer", error))?;
    drop(output_ctx);
    let output_input = ffmpeg::format::input(output)
        .map_err(|error| ffmpeg_error(ErrorCode::Remux, "packet-copy output probe", error))?;
    let result = probe_input(&output_input).map_err(|mut error| {
        error.operation = "packet-copy output probe".to_owned();
        error
    })?;
    verify_result(&maps, &sources, &output_input, &result, topology)?;
    drop(output_input);
    drop(sources);
    drop(concat_file);
    Ok(result)
}

// Keep every input coded-side-data tuple; a muxer may add container-native entries.
fn verify_error(operation: &'static str) -> CoreError {
    CoreError::new(ErrorCode::Remux, operation)
}
fn side_data_types_preserved(required: &[String], actual: &[String]) -> bool {
    let mut actual = actual.iter();
    for expected in required {
        let Some(mut observed) = actual.next() else {
            return false;
        };
        while observed < expected {
            let Some(next) = actual.next() else {
                return false;
            };
            observed = next;
        }
        if observed != expected {
            return false;
        }
    }
    true
}

fn coded_side_data_tuple_equal(
    left: &ffmpeg::packet::SideData<'_>,
    right: &ffmpeg::packet::SideData<'_>,
) -> bool {
    if left.kind() != right.kind() {
        return false;
    }
    let left = unsafe { &*left.as_ptr() };
    let right = unsafe { &*right.as_ptr() };
    let size = left.size;
    if size != right.size {
        return false;
    }
    if size == 0 {
        return true;
    }
    if left.data.is_null() || right.data.is_null() {
        return false;
    }
    unsafe {
        std::slice::from_raw_parts(left.data, size) == std::slice::from_raw_parts(right.data, size)
    }
}

fn coded_side_data_preserved(source: &ffmpeg::Stream<'_>, output: &ffmpeg::Stream<'_>) -> bool {
    for (index, required) in source.side_data().enumerate() {
        let duplicates_before = source
            .side_data()
            .take(index)
            .filter(|item| coded_side_data_tuple_equal(item, &required))
            .count();
        let output_matches = output
            .side_data()
            .filter(|item| coded_side_data_tuple_equal(item, &required))
            .count();
        if output_matches <= duplicates_before {
            return false;
        }
    }
    true
}

fn verify_result(
    expected: &[(usize, usize, usize, ffmpeg::Rational, ProbedStream)],
    sources: &[ffmpeg::format::context::Input],
    output_context: &ffmpeg::format::context::Input,
    probe: &MediaProbe,
    topology: StreamTopology,
) -> Result<()> {
    if expected.len() != probe.streams.len() {
        return Err(verify_error("packet-copy output stream count verification"));
    }
    for (source_index, source_stream_index, output_index, _, source) in expected {
        let Some(source_input) = sources.get(*source_index) else {
            return Err(verify_error("packet-copy source stream verification"));
        };
        let Some(source_stream) = source_input.stream(*source_stream_index) else {
            return Err(verify_error("packet-copy source stream verification"));
        };
        let Some(output_stream) = output_context.stream(*output_index) else {
            return Err(verify_error("packet-copy output stream verification"));
        };
        let Some(target) = probe
            .streams
            .iter()
            .find(|stream| stream.index == *output_index)
        else {
            return Err(verify_error("packet-copy output stream verification"));
        };
        if target.codec != source.codec
            || target.media_type != source.media_type
            || target.width != source.width
            || target.height != source.height
            || target.sample_rate != source.sample_rate
            || target.channels != source.channels
        {
            return Err(verify_error(
                "packet-copy output codec profile verification",
            ));
        }
        if target.color_range != source.color_range
            || target.color_primaries != source.color_primaries
            || target.color_transfer != source.color_transfer
            || target.color_matrix != source.color_matrix
        {
            return Err(verify_error("packet-copy output color verification"));
        }
        // The concat demuxer's aggregate average rate is a guess; packet timing is verified end-to-end in the corpus.
        if source.media_type.eq_ignore_ascii_case("Video")
            && topology != StreamTopology::Sequential
            && !same_rate(
                source.frame_rate_num,
                source.frame_rate_den,
                target.frame_rate_num,
                target.frame_rate_den,
            )
        {
            return Err(verify_error("packet-copy output frame-rate verification"));
        }
        if !side_data_types_preserved(&source.side_data, &target.side_data) {
            return Err(verify_error(
                "packet-copy output side-data type verification",
            ));
        }
        if !coded_side_data_preserved(&source_stream, &output_stream) {
            return Err(verify_error(
                "packet-copy output side-data payload verification",
            ));
        }
        if source
            .metadata
            .iter()
            .any(|pair| !target.metadata.contains(pair))
        {
            return Err(verify_error("packet-copy output metadata verification"));
        }
        let expected_duration =
            duration_us(source.duration, source.time_base_num, source.time_base_den);
        let actual_duration =
            duration_us(target.duration, target.time_base_num, target.time_base_den);
        if let (Some(expected), Some(actual)) = (expected_duration, actual_duration) {
            if expected > 0 && (expected - actual).abs() > 1_000_000 {
                return Err(verify_error("packet-copy output duration verification"));
            }
        }
    }
    Ok(())
}

fn duration_us(duration: Option<i64>, numerator: i32, denominator: i32) -> Option<i128> {
    let duration = duration? as i128;
    if denominator <= 0 {
        return None;
    }
    Some(duration * numerator as i128 * 1_000_000 / denominator as i128)
}
fn same_rate(
    left_num: Option<i32>,
    left_den: Option<i32>,
    right_num: Option<i32>,
    right_den: Option<i32>,
) -> bool {
    match (left_num, left_den, right_num, right_den) {
        (None, None, None, None) => true,
        (Some(ln), Some(ld), Some(rn), Some(rd)) if ld > 0 && rd > 0 => {
            let left = ln as i128 * rd as i128;
            let right = rn as i128 * ld as i128;
            (left - right).abs() * 100 <= left.abs().max(1)
        }
        _ => false,
    }
}

pub(crate) fn validate_plan(probe: &MediaProbe, plan: &StreamPlan) -> Result<()> {
    let invalid = || {
        CoreError::new(
            ErrorCode::RepresentationChanged,
            "selected representation verification",
        )
    };
    let stream_for = |key: &RepresentationKey| {
        let required_type = if key.audio { "Audio" } else { "Video" };
        probe
            .streams
            .iter()
            .find(|stream| stream.media_type.eq_ignore_ascii_case(required_type))
    };
    let video = stream_for(&plan.video).ok_or_else(invalid)?;
    validate_representation(&plan.video, video).map_err(|_| invalid())?;
    if let Some(audio) = &plan.audio {
        let stream = stream_for(audio).ok_or_else(invalid)?;
        validate_representation(audio, stream).map_err(|_| invalid())?;
    } else if plan.topology == StreamTopology::Dash
        && probe
            .streams
            .iter()
            .any(|stream| stream.media_type.eq_ignore_ascii_case("Audio"))
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_representation(expected: &RepresentationKey, actual: &ProbedStream) -> Result<()> {
    let invalid = || {
        CoreError::new(
            ErrorCode::RepresentationChanged,
            "selected representation verification",
        )
    };
    if !codec_matches(&expected.codec, &actual.codec) {
        return Err(invalid());
    }
    if !expected.audio {
        for (wanted, observed) in [
            (expected.width, actual.width),
            (expected.height, actual.height),
        ] {
            if let Some(wanted) = wanted {
                if observed != Some(wanted as i32) {
                    return Err(invalid());
                }
            }
        }
        if let Some(fps) = &expected.fps {
            let (Some(num), Some(den)) = (actual.frame_rate_num, actual.frame_rate_den) else {
                return Err(invalid());
            };
            let expected_num = fps.numerator as i128;
            let expected_den = fps.denominator as i128;
            let actual_num = num as i128;
            let actual_den = den as i128;
            if expected_den == 0
                || actual_den <= 0
                || (actual_num * expected_den - expected_num * actual_den).abs() * 100
                    > (expected_num * actual_den).abs().max(1)
            {
                return Err(invalid());
            }
        }
    }
    if expected.audio {
        if let Some(rate) = expected.sample_rate {
            if actual.sample_rate != Some(rate as i32) {
                return Err(invalid());
            }
        }
        if let Some(channels) = expected.channels {
            if actual.channels != Some(channels as i32) {
                return Err(invalid());
            }
        }
        if let Some(language) = expected.language.as_deref() {
            if !actual.metadata.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("language") && value.eq_ignore_ascii_case(language)
            }) {
                return Err(invalid());
            }
        }
    }
    if !expected.audio {
        if let Some(range) = expected.dynamic_range.as_deref() {
            let range = range.to_ascii_lowercase().replace(['-', '_', ' '], "");
            let hdr =
                range.contains("hdr") || range.contains("dolby") || range == "pq" || range == "hlg";
            if hdr && !matches!(actual.color_transfer, Some(16 | 18)) {
                return Err(invalid());
            }
            if range.contains("dolby") && !actual.side_data.iter().any(|kind| kind.contains("DOVI"))
            {
                return Err(invalid());
            }
            if range == "sdr" && matches!(actual.color_transfer, Some(16 | 18)) {
                return Err(invalid());
            }
            if !hdr && range != "sdr" {
                return Err(invalid());
            }
        }
        if let Some(color) = expected.color.as_deref() {
            let color = color.to_ascii_lowercase().replace(['-', '_', '.'], "");
            let primaries = if color.contains("bt2020") {
                9
            } else if color.contains("bt709") {
                1
            } else if color.contains("smpte170m") {
                6
            } else {
                return Err(invalid());
            };
            if actual.color_primaries != Some(primaries) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn codec_matches(advertised: &str, native: &str) -> bool {
    let advertised = advertised.to_ascii_lowercase();
    let native = native.to_ascii_lowercase();
    let expected = if advertised.starts_with("avc") || advertised.contains("h264") {
        "h264"
    } else if advertised.starts_with("hev")
        || advertised.starts_with("hvc")
        || advertised.contains("hevc")
    {
        "hevc"
    } else if advertised.starts_with("av01") || advertised == "av1" {
        "av1"
    } else if advertised.starts_with("mp4a") || advertised.contains("aac") {
        "aac"
    } else if advertised.contains("flac") {
        "flac"
    } else if advertised.contains("eac3") || advertised.contains("ec-3") {
        "eac3"
    } else if advertised.contains("ac3") || advertised.contains("ac-3") {
        "ac3"
    } else if advertised.contains("opus") {
        "opus"
    } else if advertised.contains("mp3") {
        "mp3"
    } else {
        return advertised == native;
    };
    expected == native
}
#[cfg(test)]
mod representation_validation_tests {
    use super::*;
    use crate::domain::{Aid, Cid, ContentRef, MergePolicy, PartRef, Rational, SidecarPolicy};

    fn plan() -> StreamPlan {
        let aid = Aid::new(1).unwrap();
        let part = PartRef {
            content: ContentRef::Video(aid),
            aid,
            cid: Cid::new(2).unwrap(),
            episode: None,
        };
        StreamPlan {
            schema_version: 1,
            fingerprint: "fixture".into(),
            part: part.clone(),
            title: "fixture".into(),
            video: RepresentationKey {
                part: part.clone(),
                id: 32,
                quality: 80,
                codec: "avc1.64001F".into(),
                width: Some(852),
                height: Some(480),
                fps: Some(Rational {
                    numerator: 30_303,
                    denominator: 1_000,
                }),
                dynamic_range: None,
                color: None,
                bandwidth: 791_947,
                audio: false,
                sample_rate: None,
                channels: None,
                language: None,
            },
            audio: Some(RepresentationKey {
                part: part.clone(),
                id: 30,
                quality: 80,
                codec: "mp4a.40.2".into(),
                width: Some(0),
                height: Some(0),
                fps: None,
                dynamic_range: None,
                color: None,
                bandwidth: 329_381,
                audio: true,
                sample_rate: None,
                channels: None,
                language: None,
            }),
            topology: StreamTopology::Dash,
            segments: Vec::new(),
            destination: PathBuf::from("fixture.mp4"),
            merge: MergePolicy::Automatic,
            sidecars: SidecarPolicy::default(),
            keep_temp: false,
            catalog_revision: 1,
        }
    }

    fn native_probe() -> MediaProbe {
        MediaProbe {
            format: Some("mov".into()),
            duration_us: Some(1_000_000),
            streams: vec![
                ProbedStream {
                    index: 0,
                    codec: "H264".into(),
                    media_type: "Video".into(),
                    width: Some(852),
                    height: Some(480),
                    sample_rate: None,
                    channels: None,
                    color_range: None,
                    color_primaries: None,
                    color_transfer: None,
                    color_matrix: None,
                    frame_rate_num: Some(30),
                    frame_rate_den: Some(1),
                    time_base_num: 1,
                    time_base_den: 30,
                    start_time: Some(0),
                    duration: None,
                    metadata: Vec::new(),
                    side_data: Vec::new(),
                },
                ProbedStream {
                    index: 1,
                    codec: "AAC".into(),
                    media_type: "Audio".into(),
                    width: None,
                    height: None,
                    sample_rate: None,
                    channels: None,
                    color_range: None,
                    color_primaries: None,
                    color_transfer: None,
                    color_matrix: None,
                    frame_rate_num: None,
                    frame_rate_den: None,
                    time_base_num: 1,
                    time_base_den: 48_000,
                    start_time: Some(0),
                    duration: None,
                    metadata: Vec::new(),
                    side_data: Vec::new(),
                },
            ],
        }
    }

    fn assert_representation_changed(probe: &MediaProbe, plan: &StreamPlan) {
        let error =
            validate_plan(probe, plan).expect_err("changed representation must be rejected");
        assert_eq!(error.code, ErrorCode::RepresentationChanged);
    }

    #[test]
    fn validates_selected_keys_against_native_mixed_stream_probe() {
        let probe = native_probe();
        let selected = plan();
        assert!(
            validate_representation(&selected.video, &probe.streams[0]).is_ok(),
            "30/1 native video must satisfy the selected 30303/1000 FPS under the existing tolerance"
        );
        let result = validate_plan(&probe, &selected);
        assert!(
            result.is_ok(),
            "native audio with no dimensions matches selected zero dimensions: {result:?}"
        );

        let mut wrong_video_dimensions = selected.clone();
        wrong_video_dimensions.video.width = Some(851);
        assert_representation_changed(&probe, &wrong_video_dimensions);

        let mut wrong_audio_codec = selected.clone();
        wrong_audio_codec.audio.as_mut().unwrap().codec = "ac-3".into();
        assert_representation_changed(&probe, &wrong_audio_codec);

        let mut audio_probe = probe.clone();
        audio_probe.streams[1].sample_rate = Some(48_000);
        audio_probe.streams[1].channels = Some(2);
        let mut matching_audio = selected.clone();
        matching_audio.audio.as_mut().unwrap().sample_rate = Some(48_000);
        matching_audio.audio.as_mut().unwrap().channels = Some(2);
        assert!(validate_plan(&audio_probe, &matching_audio).is_ok());

        let mut wrong_audio_rate = matching_audio.clone();
        wrong_audio_rate.audio.as_mut().unwrap().sample_rate = Some(44_100);
        assert_representation_changed(&audio_probe, &wrong_audio_rate);

        let mut wrong_audio_channels = matching_audio;
        wrong_audio_channels.audio.as_mut().unwrap().channels = Some(1);
        assert_representation_changed(&audio_probe, &wrong_audio_channels);
    }
}
