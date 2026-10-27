use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "bili",
    version,
    about = "Bilibili downloads with local library remux",
    long_about = "A CLI shell over bili_core. Download only media you are authorized to save. No player or FFmpeg executable is launched."
)]
pub struct Cli {
    /// Ordinary settings JSON, never authentication material.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub state_dir: Option<PathBuf>,
    /// Schema-versioned JSON results or newline-delimited job updates.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Search {
        query: String,
        #[arg(long = "type", value_enum, default_value = "all")]
        category: Category,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
        #[command(flatten)]
        auth: AuthOptions,
    },
    Info {
        input: String,
        #[command(flatten)]
        auth: AuthOptions,
    },
    Qualities {
        input: String,
        /// Original ordinal; omitted means the first/sole part resolved from the URL.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        part: Option<u32>,
        #[command(flatten)]
        auth: AuthOptions,
    },
    Login {
        #[command(subcommand)]
        method: Login,
    },
    Logout,
    Download(Download),
    /// Inspect durable jobs without taking the state-directory writer lock.
    Jobs,
    Resume {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        job: Option<String>,
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        auth: AuthOptions,
    },
    Merge {
        #[arg(required_unless_present = "video", conflicts_with = "video")]
        job: Option<String>,
        #[arg(long, requires = "output")]
        video: Option<PathBuf>,
        #[arg(long, requires = "video")]
        audio: Option<PathBuf>,
        #[arg(long, requires = "video")]
        output: Option<PathBuf>,
    },
    Clean {
        #[arg(required_unless_present = "abandoned", conflicts_with = "abandoned")]
        job: Option<String>,
        #[arg(long)]
        abandoned: bool,
        #[arg(long)]
        dry_run: bool,
        /// Explicitly permit removal of owned recoverable partials.
        #[arg(long)]
        include_resumable: bool,
    },
    /// Report local permissions and linked native capabilities; no network request.
    Doctor,
}

#[derive(Subcommand)]
pub enum Login {
    Qr {
        #[arg(long)]
        remember: bool,
    },
    Cookie {
        /// Explicit secret input from stdin; never pass Cookies as command arguments.
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        remember: bool,
    },
    Status,
}

#[derive(Args)]
pub struct AuthOptions {
    /// auto loads opt-in stored login, otherwise anonymous; never silently prompts.
    #[arg(long, value_enum, default_value = "auto")]
    pub auth: AuthMode,
    /// Persist this invocation's production login explicitly (off by default).
    #[arg(long)]
    pub remember: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum AuthMode {
    Auto,
    Stored,
    Anonymous,
    Qr,
    CookieStdin,
    CookiePrompt,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Category {
    All,
    Video,
    Bangumi,
    Film,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum MergeMode {
    Auto,
    Manual,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Danmaku {
    Current,
    None,
}

#[derive(Args)]
pub struct Download {
    #[arg(required = true)]
    pub inputs: Vec<String>,
    /// all or comma-separated 1-based ordinals/ranges, for example 1,3-5.
    #[arg(long, default_value = "all")]
    pub parts: String,
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[command(flatten)]
    pub auth: AuthOptions,
    /// highest chooses the highest obtainable actual resolution, or exact server quality ID.
    #[arg(long, default_value = "highest")]
    pub quality: String,
    #[arg(long)]
    pub video_repr: Option<u64>,
    #[arg(long)]
    pub audio_repr: Option<u64>,
    #[arg(long)]
    pub codec: Option<String>,
    #[arg(long)]
    pub hdr: Option<String>,
    /// all, none, comma-separated language labels, or track:ID,track:ID.
    #[arg(long, default_value = "all")]
    pub subtitles: String,
    #[arg(long, value_enum, default_value = "current")]
    pub danmaku: Danmaku,
    #[arg(long, value_enum, default_value = "auto")]
    pub merge: MergeMode,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub jobs: Option<u16>,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub connections: Option<u16>,
    #[arg(long)]
    pub keep_temp: bool,
}
