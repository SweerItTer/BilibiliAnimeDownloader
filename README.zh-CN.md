# Bilibili Anime Downloader

[English](README.md) | 简体中文

一个仅面向 Bilibili 的 Rust 下载器：`bili_core` 是可复用的业务核心库，`bili` 是其 CLI 外壳；原生 GUI 是后续工作。首个 Alpha 面向 Linux x86_64 CLI，目前仍在准备中，尚无发布下载地址。不集成播放器、不启动 FFmpeg 可执行文件子进程。目前未实现 WebView；未来原生 GUI 仅可为人工处理验证挑战而按需延迟加载 WebView。

**许可证：**应用程序继续采用[附加 Commons Clause 的 MIT 许可证](LICENSE)，并非纯 MIT。随包提供的 FFmpeg 库分别受 LGPL-2.1-or-later 许可；应用程序的限制不适用于这些库。只下载你有权保存的内容；本项目不会绕过登录、权益、地区限制、DRM 或验证挑战。

## Linux CLI 快速开始

在仓库根目录，本节使用此检出中 `target/packages` 下已有的归档；它不是下载 URL，单独克隆源码的用户无法取得该归档。请解压到一个全新的 `target/cli` 目录。首次解压后，之后打开终端只需重新设置 `BILI`；不要再次解压。此归档是在当前 WSL 主机 glibc 2.43 环境下构建的；它不能证明 Ubuntu 22.04 发布兼容性。请保持包内 `bin/` 和 `lib/` 布局完整。运行已打包的 CLI 无需 Rust 工具链、FFmpeg 可执行文件、编译器设置或构建专用环境。

```sh
printf '%s  %s\n' 'd79a79e8d873db837b36f64adadebdbbacaa4d7e9a58c9a8a598b3fea1d3311b' 'target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz' | sha256sum -c - &&
mkdir target/cli &&
tar -xzf target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz -C target/cli &&
BILI="$PWD/target/cli/bili-x86_64-unknown-linux-gnu-release/bin/bili" &&
"$BILI" --help &&
"$BILI" search --help &&
"$BILI" download --help
```

搜索关键词并查看结果，再选择 ID 下载；搜索不会自动将结果加入下载队列。页码参数用于选择页；没有 page-size 参数。用户已确认登录/记住登录、视频搜索、下载/合并、字幕和弹幕功能。此确认不代表 Alpha 软件包已通过发布验收，也不代表稳定性或性能已验证。遇到访问或风险验证挑战时请停止。

```sh
"$BILI" search "keyword" --type video --page 1 --auth anonymous
"$BILI" search "keyword" --type video --page 2 --auth anonymous
"$BILI" info BV... --auth anonymous
"$BILI" qualities BV... --part 1 --auth anonymous
"$BILI" download av123456789 --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
"$BILI" download av123456789 ss12345 ep12345 --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

请将命令中的数字替换为搜索结果中的实际 ID（视频结果使用 `av` 加数字 aid；季结果使用 `ss` 加数字季 ID）。`BV...`、`ep...` 和受支持的 Bilibili URL 也可作为下载输入。所示 ID 是占位符，运行命令前必须替换。最后一条命令展示多个位置参数。示例默认匿名访问；仅在存在时才会包含字幕和弹幕。相对输出路径基于当前工作目录解析，而不是 CLI 的安装目录。CLI 会报告已完成文件的路径；请自行用已有播放器打开视频。程序不会安装或启动播放器。

按一次 Ctrl+C 会请求协作式暂停。查看任务并用相同任务 ID 续传；再次下载时，只有匹配且已验证完成的文件才会跳过。

```sh
"$BILI" jobs
"$BILI" resume JOB-ID --auth anonymous
"$BILI" download BV... --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

若只需一次性且不持久化的 QR 登录，请在实际的 `search`、`info`、`qualities`、`download` 或 `resume` 命令上使用 `--auth qr`。在交互式终端中，CLI 会显示可扫描的二维码；请用 Bilibili App 扫描。stdin 和 stderr 都必须连接到终端，因此不要管道传输或重定向登录命令。独立的 `login qr` 也会显示此终端二维码。不带 `--remember` 的独立登录只属于当前进程；之后启动的命令仍是未登录状态。若要记住生产环境 QR 登录供之后进程使用，请运行：

```sh
"$BILI" login qr --remember
"$BILI" download av123456789 --parts all --output "$HOME/Videos/Bilibili" --auth stored
```

默认不会记住登录。优先使用安全的操作系统存储；若不可用，应用可能会回退到私有本地存储中的明文，并显示警告。绝不要记住仅供测试的凭据。

## Alpha 状态与支持目标

首个计划发布版本为 Linux x86_64 CLI `v2.0.0-alpha.1`。目前仍在准备，尚未发布，也没有发布下载地址。本机现有归档由 WSL/glibc 2.43 构建，不能证明 Ubuntu 22.04/glibc 2.35 兼容性。发布前须在 Ubuntu 22.04 基线上构建，并由独立干净 hosted VM 对同一发布制品执行离线包检查和匿名基础媒体冒烟。

首个 Alpha 不面向 Windows；macOS 不在范围内。GUI、Aria2、验证 WebView、搜索排序及损坏分段恢复均未交付。稳定性、性能和本版本跨进程续传/恢复尚未验证。

应用程序继续采用[附加 Commons Clause 的 MIT 许可证](LICENSE)，并非纯 MIT。FFmpeg 从固定源码构建为可替换共享库，另受 LGPL-2.1-or-later 条款约束；发布时提供匹配源码和声明。应用程序限制不适用于 FFmpeg；不随包提供或启动 `ffmpeg`/`ffprobe` 可执行文件。

## 维护者离线软件包检查（Linux）

以下步骤使用历史 gate33 代与当前 local-standard 代共有的归档布局。默认 `target/packages` 归档现已重新生成；请使用当前应用程序/native-source 校验和清单（其中不包含保留的 SDK 归档）识别其文件，而不要使用历史 gate33–35 哈希值。这不会下载发布包，也不会为其他操作系统提供资格证明。在仓库根目录提取 CLI 归档，并保持 `bin/` 与 `lib/` 放在一起：

```sh
(cd target/packages/x86_64-unknown-linux-gnu && sha256sum -c SHA256SUMS)
tar -xzf target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz
APP="$PWD/bili-x86_64-unknown-linux-gnu-release"
WORK="$PWD/candidate-local"
mkdir -p "$WORK/home"
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --help
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --version
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --state-dir "$WORK/state" --json doctor
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --state-dir "$WORK/state" --json merge --video "$PWD/target/media-fixtures/h264-aac.mp4" --output "$WORK/manual-merge.mp4"
```

合并示例需要已有且经过验证的合成媒体样本；该命令不会创建样本。记录的 gate35 公开样本使用了完全相同的命令形式，并使用全新的隔离路径、未改变的默认值和显式匿名认证：

```sh
env -i PATH="$APP/bin" HOME="$WORK/home" "$APP/bin/bili" --state-dir "$WORK/state" --json download BV1gD4y1o72p --parts 1 --output "$WORK/output" --auth anonymous --merge auto
```

这是文档示例，不代表授权重新运行网络验收。首次按 Ctrl+C 会协作式暂停；`resume JOB-ID --auth anonymous` 使用同一状态。验证/访问拒绝仍然是错误，绝不是绕过限制或切换身份的指示。记录的验收没有启用代理，代理也不是主线要求。

## 命令

```text
bili --help
bili --version
bili doctor --json
bili search "keyword" --type all --page 1 --auth anonymous --json
bili info BV... --auth anonymous --json
bili qualities BV... --part 1 --auth anonymous --json
bili download BV... ss... ep... --parts all --output ./downloads --auth anonymous
bili jobs --json
bili resume --all --auth anonymous
bili merge JOB-ID
bili merge --video ./video.m4s --audio ./audio.m4s --output ./merged.mp4
bili clean JOB-ID --dry-run
bili clean --abandoned --include-resumable --dry-run
```

上面的 `BV...`、`ss...`、`ep...` 和 `JOB-ID` 表示参数形式，并非实际执行的示例。规范的 BV/AV/season/episode ID 及受支持的 Bilibili URL 由同一核心解析；普通多 P 视频、番剧季和电影会列出所有分 P。搜索是分页发现功能，不是下载队列。

下载默认选择**全部分 P**、实际可获取的最高空间分辨率、所有可用的对白字幕轨、当前可用的弹幕池，并由本地库自动重封装。选中的表示形式、实际宽/高、FPS、动态范围和编解码器会分别显示。较大的 HDR/Dolby 数值质量标签不代表分辨率更高。排序由核心负责；不会悄悄臆造访问权限或媒体表示形式的回退。

未指定 `--part` 时，`qualities` 使用输入解析后选中的第一个或唯一分 P/剧集，因此原季序号为 13 的剧集或带 `?p=3` 的视频 URL 仍然有效。显式 `--part` 指原始序号，不是从 1 开始重新编号的列表。

使用 `--parts 1,3-5`、`--quality ID`、`--video-repr ID`、`--audio-repr ID`、`--codec CODEC` 和 `--hdr MODE` 限定选择。精确指定但不可用或有歧义的选项会明确报错。`--subtitles all|none|en,ja` 选择对白字幕轨；`--subtitles track:ID,track:ID` 选择确切轨道 ID。不要混用轨道 ID 与语言选择器。`--danmaku current|none`、`--merge auto|manual`、`--jobs N`、`--connections N` 和 `--keep-temp` 控制相应核心策略。`--merge manual` 会保留经过校验的受管理输入，供 `bili merge JOB-ID` 使用；本地显式合并不会接管或删除你的输入文件。

## 身份验证与隐私

身份验证与下载/续传在**同一次调用和同一个核心生命周期**内进行：

```text
bili download BV... --auth qr
bili download BV... --auth cookie-prompt
bili download BV... --auth cookie-stdin
bili resume --all --auth stored
bili login qr --remember
bili login cookie --stdin --remember
bili login status
bili logout
```

- `auto` 会加载已明确选择记住的现有会话；否则以匿名方式运行。读取已存记录失败时会报错，不会回退为匿名。`anonymous` 从不加载已记住的登录；`stored` 则要求存在已存登录。
- QR 和隐藏 Cookie 提示要求交互式终端。非 TTY 模式不会悄悄提示输入。提交、拒绝或中断时，隐藏输入提示都会恢复终端设置。`cookie-stdin` / `login cookie --stdin` 要求 stdin 被显式重定向，并将有界长度的秘密输入读入内存。请安排可信的秘密输入提供程序；**绝不要把 Cookie 放入命令文本、提供程序参数、环境变量、临时文件、错误报告或测试样本**。
- 不带 `--remember` 的独立登录只验证当前进程的会话，**不会**让之后启动的 CLI 进程处于登录状态。对于仅驻留内存的身份验证，优先使用同一次调用的 `download/resume --auth ...` 形式。
- 默认不记住登录。只有新提供的 QR/Cookie 会话才接受 `--remember`。显式的生产环境持久化优先使用操作系统存储；安全存储不可用时，可以回退到带有明确提示、权限受平台保护的明文存储。在普通设置中将 `allow_plaintext_fallback` 设为 `false` 可拒绝该回退。此前提供的测试专用凭据绝不能被记住、恢复或放入本仓库。
- 导入 Cookie header 时会保留每个适用的名称/值，但无法重建原始信息中不存在的 domain/path/expiry 元数据。核心会应用安全的 HTTPS Bilibili 范围默认值。签名媒体 URL、请求头和 QR 轮询令牌不会成为持久化任务身份或机器输出。

## 任务、中断、输出与设置

一个执行变更的 CLI/GUI 核心会话会为整个状态目录持有 OS 锁。即使是不同任务，第二个写入者也会得到 `Busy`；`jobs` 是只读检查。独立会话应使用不同的显式状态目录，而不是删除陈旧锁。

首次按 Ctrl+C 会请求协作式停止，等待已跟踪工作和持久化检查点完成，并返回 130。第二次按 Ctrl+C 会强制退出，需要执行崩溃恢复。续传会为精确选中的流重新获取当前定位信息并验证区间连续性；重复下载只会跳过匹配且已校验的受管理完成项，绝不会仅因文件大小相同就跳过。失败的旁路文件会作为独立组件失败显示；可单独重试，而无需再次下载已校验完成的视频。

面向人的进度和提示写入 stderr；最终结果/路径写入 stdout。`--json` 输出 schema 版本 1 的 JSON 信封（`type` 加 `data`），任务执行期间每行一个。它绝不输出凭据、媒体定位信息、QR 内容或 ANSI 进度。每个任务结果记录都包含其退出码。退出状态：0 表示所有请求组件成功/经校验后跳过；2 表示用法/输入无效；3 表示登录/访问/验证/选择需要处理；4 表示传输/文件系统/损坏/原生依赖/重封装/部分失败；130 表示中断。混合批次会保留每项结果；优先级依次为中断、4、3、2。

`--state-dir PATH` 选择日志目录；`--config PATH` 读取普通 JSON 设置。CLI 相对文件路径按当前目录解析，不会悄悄覆盖目标。默认状态/输出目录是平台 `bili` 本地数据目录中的 `state`/`downloads`，不会隐式回退到当前工作目录。支持的设置（未知字段会报错）：

```json
{
  "state_dir": "/absolute/private/state",
  "output_dir": "/absolute/downloads",
  "jobs": 3,
  "connections": 6,
  "allow_plaintext_fallback": false
}
```

在 Windows 上使用适合该平台的路径。设置不包含 Cookies。清理操作以任务为单位；请先检查 `--dry-run`。删除仍可续传且由程序管理的部分文件需要 `--include-resumable`；清理操作绝不会递归删除任意用户目录或已完成的用户输出。

## 字幕与弹幕兼容性

对白轨道是相互独立的 UTF-8 SRT 或保留原样的 ASS 文件，并标记语言/类型/轨道。当前弹幕池会分别输出原始/XML 文件和滚动评论 ASS，不会强行合并成一个对白/弹幕 ASS。缺失分段、不支持的模式和请求组件失败都会报告；不保证已删除、已审核或历史评论。

当前弹幕池确实为空时，可以产生零字节 JSONL，同时 XML/ASS 结构有效且评论/事件数为零。接收器校验会解析全部三种格式，检查数量/遗漏的一致性，并保留安全路径/文件/哈希检查；它不会把传输、protobuf、格式错误输出或访问错误转成空结果成功。早期开发流水线弹幕损坏的原因仍未知；之后发现的空 JSONL 接收器缺陷是另一问题。

普通视频不需要特殊播放器。同时显示对白字幕和滚动 ASS 弹幕需要另外安装兼容播放器及适用字体。`bili` 不会安装或启动播放器。

以下手动配方曾在**隔离 Linux 环境的 mpv 0.41.0 / libass 0.17.4**上，使用核心实际生成的 SRT 和滚动评论 ASS 进行验证：

```sh
mpv --sub-file="dialogue.srt" --sub-file="danmaku.ass" --sub-ass-override=no --secondary-sub-ass-override=no "video.mp4"
```

检查播放器实际分配的字幕轨道 ID，再将对白轨 ID 设为 `sid`、弹幕轨 ID 设为 `secondary-sid`；不要假设 ID 固定。设置 `sid=no` 或 `secondary-sid=no` 只隐藏对应轨道；恢复其对应 ID 即可重新显示。**弹幕 ASS 作为第二轨时，`secondary-sub-ass-override=no` 很重要。**该 Linux 运行中观察到了同时渲染、独立开关、像素恢复和定时弹幕移动。

这不构成 Windows、字体可用性、布局、性能或高级模式的资格验证。XML 弹幕及其他播放器需要各自支持对应格式；这些组合未经过测试。

## 构建与原生库替换

Rust 版本由 `rust-toolchain.toml` 固定；使用普通 Cargo 依赖/缓存。按照精确的源码固定版本和配方构建 FFmpeg：

```sh
python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu
FFMPEG_DIR="$PWD/target/native/x86_64-unknown-linux-gnu" \
  cargo build -p core-cli --release --locked --target x86_64-unknown-linux-gnu
```

构建脚本要求 Python 3.11.9+（CI 使用 Python 3.13）；Ubuntu 22.04 默认的 Python 3.10 不足。Linux 还需要 C 编译器、make、NASM、pkg-config、用于绑定的 libclang 和用于打包的 patchelf。Windows 使用相应源码构建工作流，并需要已初始化的 MSVC 开发环境及 MSYS2 构建工具/NASM；MinGW 二进制不能替代 MSVC 目标。`FFMPEG_DIR` 用于构建时发现，**不是**运行时库选择器。未启用 `ffmpeg-next` 中会暗示静态链接的 `build` 功能。

### 当前主机离线构建与打包配方

当前 local-first 运行记录在 `.trellis/tasks/10-24-core-cli-delivery/research/local-standard-current.json`，提取运行时细节记录在旁边的 `local-standard-extracted-package.json`。该运行确实构建了当前源码版本的 FFmpeg 和 CLI，启动了 help/version/doctor，完成了合成的本地库合并，生成了两种 SDK 模式和软件包，并通过离线提取包接收器检查。这台 WSL 主机使用 **glibc 2.43**：这些字节仅是当前主机的证据，并非 Ubuntu 22.04 发布字节或 Windows 源码构建证据。执行该流程不需要新提交、推送或 CI 运行。

这台主机所需的原生工具已私下解压；不需要安装工具或修改全局环境。下面的子 shell 复现仅进程范围的配置。在其他机器上请使用该机已安装工具的路径。第一次本地 Cargo 尝试失败，是因为搬迁后的 libclang 找不到其内置 `limits.h`；未修改的日志位于上述任务下的 `research/logs/local-standard-cargo-current.txt`。下面的 `-resource-dir` 指向现有 Clang 21 内置头文件；它不会伪造或替换系统头文件。

```sh
(
  tools="$HOME/.cache/bili-native/tools/root/usr"
  export PATH="$tools/bin:$PATH"
  export LD_LIBRARY_PATH="$tools/lib/x86_64-linux-gnu"
  export PKG_CONFIG="$tools/bin/pkgconf"
  export LIBCLANG_PATH="$tools/lib/x86_64-linux-gnu"
  export BINDGEN_EXTRA_CLANG_ARGS="-resource-dir=$tools/lib/llvm-21/lib/clang/21"
  export FFMPEG_DIR="$PWD/target/native/x86_64-unknown-linux-gnu"
  export CARGO_NET_OFFLINE=true
  python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu \
    --source-cache "$HOME/.cache/bili-native/sources" --jobs 8
  cargo build -p core-cli --release --locked --offline --target x86_64-unknown-linux-gnu
  # Direct developer launch needs the freshly built shared libraries.
  LD_LIBRARY_PATH="$FFMPEG_DIR/lib" target/x86_64-unknown-linux-gnu/release/bili --help
  # SDK producers refuse overwrite: preserve any existing target/sdk/<target>
  # outside that path before generating fresh, matched outputs.
  python3 scripts/verify_core_linkage.py --mode rlib --target x86_64-unknown-linux-gnu --native-prefix "$FFMPEG_DIR"
  python3 scripts/verify_core_linkage.py --mode dylib --target x86_64-unknown-linux-gnu --native-prefix "$FFMPEG_DIR"
  python3 scripts/package.py --target x86_64-unknown-linux-gnu --profile release --native-prefix "$FFMPEG_DIR"
)
# Receiver is deliberately outside the build subshell; no private build loader,
# FFMPEG_DIR, libclang or binding-generation environment reaches the app.
env -u LD_LIBRARY_PATH -u FFMPEG_DIR -u LIBCLANG_PATH -u BINDGEN_EXTRA_CLANG_ARGS \
  -u PKG_CONFIG -u PKG_CONFIG_PATH -u PKG_CONFIG_LIBDIR \
  python3 scripts/verify_package.py \
    target/packages/x86_64-unknown-linux-gnu/bili-x86_64-unknown-linux-gnu-release.tar.gz \
    --target x86_64-unknown-linux-gnu \
    --native-source target/packages/x86_64-unknown-linux-gnu/bili-native-source-x86_64-unknown-linux-gnu.tar.gz \
    --sdk-artifact target/packages/x86_64-unknown-linux-gnu/bili-sdk-dylib-x86_64-unknown-linux-gnu.tar.gz
```

默认离线接收器会检查提取后的应用、隔离状态、help/version 和 doctor；若不启用可选替换/在线分支，它**不会**进行手动合成合并。因而，当前交付路径的合成合并是在一次全新提取后单独执行的，使用显式最小环境、不包含构建加载器/工具变量且 PATH 仅含包内容：详见[独立提取合并证明](.trellis/tasks/10-24-core-cli-delivery/research/local-standard-extracted-merge.json)。省略 `--public-media-id` 意味着不会请求 Bilibili。可选的不同库替换实验需要独立标记、由源码构建的替换前缀，不能使用普通原生前缀。直接开发者合并证据和独立提取合并证据分别保存在当前 local-standard 记录中。原生 Windows 编译仍因缺少 Rust/MSVC/SDK/Clang/libclang/MSYS2 依赖而受阻，等待安装授权；先前下载的 Windows 包运行记录仅是运行时证据。

对于直接开发者合并阶段，请使用全新的私有目录和已有合成样本（记录的运行使用 `target/media-fixtures/h264-aac.mp4`）。以下是开发者构建加载器设置，不是提取包运行时设置：

```sh
repo="$PWD"
work="$(mktemp -d "$repo/target/local-cli-XXXXXX")"
mkdir -m 700 "$work/home" "$work/config" "$work/data" "$work/state" "$work/cache"
(
  export HOME="$work/home" XDG_CONFIG_HOME="$work/config" XDG_DATA_HOME="$work/data"
  export XDG_STATE_HOME="$work/state" XDG_CACHE_HOME="$work/cache"
  export LD_LIBRARY_PATH="$repo/target/native/x86_64-unknown-linux-gnu/lib"
  cli="$repo/target/x86_64-unknown-linux-gnu/release/bili"
  "$cli" --version
  "$cli" --state-dir "$work/doctor-state" --json doctor
  "$cli" --state-dir "$work/merge-state" --json merge \
    --video "$repo/target/media-fixtures/h264-aac.mp4" --output "$work/merged.mp4"
)
```

精简后的共享库配置构建 avformat/avcodec/avutil、本地文件解复用/复用/解析器/BSF，以及严格限定的探测解码器。不会选用可执行程序、网络协议、编码器、播放功能、GPL、nonfree 或 version3 配置。支持范围由实际配置闭包和原生样本资格验证决定，而非声称任何解码器都不会被使用。不兼容的 MP4 组合会保留输入并报告失败；不会进行转码。

发布包包含构建来源信息、校验和、适用的完整 Rust 依赖/运行时/原生库许可证及 NOTICE/版权文本、替换说明，以及同等可获取的匹配原生源码/配方制品。Rust 静态链接不会免除运行时归属义务。第三方组件各自享有独立权利；应用程序的 Commons Clause 不适用于它们。许可证覆盖缺失或未知时会阻止发布，而不会用 SPDX 名称替代实际许可文本。

在保持包内布局不变的前提下，可将对应 FFmpeg 共享库替换为 ABI 主版本兼容的库；你可以修改/重新构建 LGPL 库，并按需要对应用程序进行逆向工程，以调试这些修改。例如，`python3 scripts/build_ffmpeg.py --target x86_64-unknown-linux-gnu --prefix ./target/native/replacement --extra-version replacement-smoke` 会构建可识别的兼容替换库；应使用相同的源码/配置固定版本及所需 ABI/能力。在 Linux 上，每个被替换的库都需要自己的 `$ORIGIN` RUNPATH（打包配方使用 patchelf）；Windows DLL 与可执行文件放在同一目录。替换后重新运行 `doctor` 和加载器/媒体冒烟检查。这并非对法律或专利问题的一揽子许可。

## 可复用 Rust 核心与 SDK

普通 Cargo/源码/rlib 使用方会静态集成 Rust 核心代码，同时仍动态使用 FFmpeg。单独打包的 Rust `dylib` SDK 只适用于**完全匹配的构建元组**，包括 rustc commit/target/flags/profile、锁文件/依赖图、panic 策略/分配器及原生编译器/头文件。它不是稳定 ABI、C API、`staticlib`、`cdylib`、`dlopen` 插件或热卸载接口。

使用方拥有一个 Tokio runtime，使用其句柄创建 `Core`，调用类型化 API/快照，然后等待 `shutdown` 并释放自有值；在此期间匹配的库必须保持加载。`scripts/verify_core_linkage.py` 会在 `rlib` 和 `dylib` 模式下构建独立使用方，并验证实际导入情况及移除核心 dylib 后的运行时失败。`scripts/package.py` 与 `scripts/verify_package.py` 负责打包并测试真实提取布局。每个 SDK 都附带构建元数据和使用方配方；仅凭 semver 绝不能认定 Rust 二进制兼容。

两种预构建模式都会执行一组相对于 SDK 根目录的编译器 argv，并将同一组 argv 序列化到 `consumer/build-command.txt`；`-o` 与其路径是分开的参数。编译器输入闭包包括普通依赖项所需的 proc-macro DSO 以及 rlib/rmeta 文件；这不是应用程序运行时闭包。rlib 验证器还会单独执行 `consumer/source-build-command.txt` 和其只读生命周期。原生路径会相对于接收方 SDK 根目录展开，pkg-config 的 prefix/libdir/includedir 字段会一并重定位。这些 SDK 构建要求安装完全匹配的 Rust 编译器；运行打包后的应用则不需要。

完整安装的发行版版权材料可能包含 GCC Runtime Library Exception 及 GPL 文本；应保留真实且完整的声明/来源信息，而不是只要求存在一个单独的例外文本文件。原生策略防护同样会读取生成配置的语义：取反的 `!CONFIG_GPL=yes` 表示禁用，而正向 Make 赋值或 C define 仍会拒绝该构建。这些是打包/声明检查，不是修改应用程序许可证或原生配置的许可。
