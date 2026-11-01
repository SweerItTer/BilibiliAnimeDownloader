# CLI 使用指南

[English](CLI.md) | 简体中文 | [项目首页](README.zh-CN.md)

本指南对应当前源码中的 `bili`。已发布的 [v2.0.0-alpha.6](https://github.com/SweerItTer/BilibiliAnimeDownloader/releases/tag/v2.0.0-alpha.6) 是 Linux x86_64 CLI 软件包，**没有** `update`、`download --list-resolutions` 和 `search --sort`，可用 `qualities` 查询单个分 P。Windows 和原生 GUI 可从源码或 CI 构建获取，不是该版本的发布资产。开发构建方法见 [BUILDING.md](BUILDING.md)。

跳转到：[运行方式](#invocation)、[首次下载](#first-download)、[命令表](#commands)、[搜索](#search)、[清晰度与分 P](#quality)、[登录](#login)、[下载参数](#download-options)、[恢复](#resume)、[合并](#merge)、[清理](#cleanup)、[配置](#configuration)、[更新](#updates)、[输出与错误](#output)。

<a id="invocation"></a>
## 运行 CLI

示例使用 Linux 软件包入口 `./bin/bili`。在 Windows PowerShell 中，把它换成 `.\bili.exe`；已将软件包入口加入 PATH 时，可直接用 `bili`。请保留完整解压目录，不要只复制一个可执行文件。当前版本目录布局包含固定入口、`CURRENT` 和 `versions/`；Alpha 6 使用较早的 `bin/`、`lib/` 布局。

```sh
./bin/bili --version
./bin/bili --help
./bin/bili download --help
./bin/bili login qr --help
./bin/bili help download
```

运行前替换 `VIDEO_ID`、`QUALITY_ID`、`JOB_ID` 等大写占位符。`INPUT` 接受合法的 `BV...`、`av` 加正整数视频 ID、`ss` 加季 ID、`ep` 加单集 ID，以及受支持的 HTTPS Bilibili 链接或短链接。不接受没有前缀的纯数字 ID。链接、搜索词和带空格的路径应加引号。

全局参数可放在子命令前后：

| 参数 | 作用 |
|---|---|
| `--config PATH` | 本次读取指定的普通 JSON 配置文件；不会自动寻找配置文件 |
| `--state-dir DIR` | 使用指定的任务与断点目录；查看和恢复时要使用同一目录 |
| `--json` | 输出逐行 JSON，供脚本读取 |
| `-h`、`--help` | 查看当前命令帮助；`--help` 包含较长说明 |
| `-V`、`--version` | 在顶层查看程序版本 |

<a id="first-download"></a>
## 搜索、查看信息，再下载

以下示例使用匿名访问，不读取已保存账号。搜索不会直接开始下载：

```sh
./bin/bili search "KEYWORD" --type video --auth anonymous
./bin/bili info VIDEO_ID --auth anonymous
./bin/bili qualities VIDEO_ID --part 1 --auth anonymous
./bin/bili download VIDEO_ID --parts 1 --output "$HOME/Videos/Bilibili" --auth anonymous
```

最后一条命令选择当前账号实际可获得的最高分辨率，保存字幕和当前弹幕，并将音视频合并为可播放文件。命令会等待自动合并完成，再报告完成文件。这里用 `--parts 1` 明确只下载第一个分 P；省略时会选择解析得到的**全部**分 P。单集链接或 `ep` ID 可能只解析到一集，并保留它在整季中的原始序号。

PowerShell 中对应的下载命令：

```powershell
.\bili.exe download VIDEO_ID --parts 1 --output "$HOME\Videos\Bilibili" --auth anonymous
```

需要账号权限时，先阅读[登录](#login)。只下载你有权保存的内容。登录不会绕过大会员、购买、地区、额外验证或 DRM 限制。

<a id="commands"></a>
## 完整命令表

下表的方括号表示可选项，`INPUT...` 表示一个或多个输入。不要把方括号或省略号原样输入。

| 命令 | 用途与专用参数 |
|---|---|
| `search QUERY` | 按关键词搜索；`--type all\|video\|bangumi\|film`、`--page N`、`--sort ORDER`、登录参数 |
| `info INPUT` | 解析内容，查看分 P 或分集；登录参数 |
| `qualities INPUT` | 查询单个分 P 的清晰度；`--part N`、登录参数 |
| `download INPUT...` | 下载选中的内容；见下方[下载参数](#download-options) |
| `login qr` | 在交互终端扫码登录；`--remember` |
| `login cookie` | 隐藏输入 Cookie，或加 `--stdin` 从重定向输入读取；`--remember` |
| `login status` | 读取已保存账号并显示状态 |
| `logout` | 通过账号存储移除记忆的登录 |
| `jobs` | 只读查看任务，不占用下载写锁 |
| `resume JOB_ID` 或 `resume --all` | 继续或重试已保存任务；登录参数 |
| `merge JOB_ID` | 合并之前选择手动合并的受管下载任务 |
| `merge --video PATH [--audio PATH] --output PATH` | 将本地媒体重新封装到一个新文件 |
| `clean JOB_ID` 或 `clean --abandoned` | 清理程序拥有的临时文件；`--dry-run`、`--include-resumable` |
| `doctor` | 检查本地路径、权限、构建身份和原生媒体库能力；不发起网络请求 |
| `update` | 安装兼容的新版本；`--check`、`--pre-release[=true\|false]` |
| `help [COMMAND...]` | 查看程序或子命令帮助，与其帮助开关作用相同 |

登录参数指 `--auth MODE` 和显式的 `--remember`，适用于 `search`、`info`、`qualities`、`download`、`resume`，并非所有命令都接受。

<a id="search"></a>
## 搜索与排序

`--type` 默认 `all`，`--page` 默认 `1`，`--sort` 默认 `relevance`。页码范围为 1–1000；搜索词不能为空，且不超过 256 个 UTF-8 字节。`all` 分别返回视频、番剧和影视的分类页，不把三类拼成一个全局排名。

当前源码支持以下视频排序：

| `--sort` | 顺序 |
|---|---|
| `relevance` | 保持原有服务端默认排序，所有类型均可使用 |
| `views` | 最多播放 |
| `newest` | 最新发布 |
| `danmaku` | 最多弹幕 |
| `favorites` | 最多收藏 |
| `comments` | 最多评论 |

```sh
./bin/bili search "KEYWORD" --type video --sort newest --page 1 --auth anonymous
./bin/bili search "KEYWORD" --type video --sort newest --page 2 --auth anonymous
```

非默认排序必须配合 `--type video`。与 `all`、`bangumi` 或 `film` 组合时会明确报错，不会悄悄改回默认排序。程序保留服务端顺序与分页，不在本地只对当前一页重新排序。公开服务的实际排序效果尚未在线验收。

<a id="quality"></a>
## 选择分 P 与清晰度

先用 `info INPUT` 查看原始分 P 或分集序号。`qualities INPUT` 默认查询第一个或唯一解析结果；加 `--part N` 选择原始序号。链接选中的 P3，或整季中第 13 集的 `ep` ID，不会因此重新编号为 1。

当前源码可以只查询多个分 P，不创建下载任务：

```sh
./bin/bili download VIDEO_ID --list-resolutions --parts 1,3-5 --auth anonymous
./bin/bili download VIDEO_ID --parts 1,3-5 --quality QUALITY_ID --auth anonymous
```

清晰度表中的 **Quality ID** 是服务端档位 ID，尺寸（如 1920×1080）和 Stream ID 是另外的值。`--quality` 要填写 Quality ID，不是像素高度或 Stream ID。缺少的尺寸、帧率和权限信息会显示未知；列出了某档位或返回了媒体地址，不等于已经成功下载。

默认 `highest` 按实际可获得的空间分辨率选择最高档。明确指定的清晰度、音视频流、编码或 HDR 条件必须匹配可用流，程序不会静默换成别的。匿名、普通登录、大会员和已购买内容的可用范围可能不同。

需要精确选流时，可查询 `qualities INPUT --json`，使用 `--video-repr ID`、`--audio-repr ID`、`--codec CODEC`、`--hdr MODE`。ID 和编码、动态范围值应取自对应分 P 的清晰度结果。不同编码或元数据的流可能共用 ID，必要时要补齐约束；这些参数选择已有流，不会转码。

<a id="login"></a>
## 本次登录与保持登录

想让后续命令继续使用账号，只需明确保存一次：

```sh
./bin/bili login qr --remember
./bin/bili login status
./bin/bili download VIDEO_ID --parts 1
```

默认的 `--auth auto` 会读取你之前选择记忆的账号，后续无需重复 `--auth stored`。若必须使用已保存账号、没有就应报错，可显式选择 `--auth stored`。保存记录出错时会报告错误，不会静默降为匿名。

只为一次下载登录，可运行 `download VIDEO_ID --auth qr`，登录持续到这条命令结束。单独运行**不带** `--remember` 的 `login qr` 或 `login cookie` 只验证本次登录；进程结束后，该登录不会传给下一条命令。

| `--auth` 模式 | 行为 |
|---|---|
| `auto` | 默认：读取主动保存的登录，否则匿名；不会自动弹出输入 |
| `stored` | 必须读取到已保存登录 |
| `anonymous` | 不读取已保存登录 |
| `qr` | 为本次命令扫码登录 |
| `cookie-prompt` | 在终端隐藏输入 Cookie 请求头 |
| `cookie-stdin` | 从明确重定向的标准输入读取 Cookie 请求头，直到输入结束 |

`--remember` 默认关闭，只能配合 `qr`、`cookie-prompt`、`cookie-stdin`，不能配合 `auto`、`stored`、`anonymous`。单独的 Cookie 登录使用 `login cookie` 隐藏输入，或 `login cookie --stdin` 读取重定向输入。扫码和隐藏输入需要交互终端；`cookie-stdin`/`login cookie --stdin` 会拒绝普通终端，以免直接回显秘密。Cookie 输入上限为 64 KiB。不要把 Cookie 写进命令参数、Shell 历史、配置 JSON 或输入来源程序的参数。

记忆登录优先使用操作系统安全存储。安全存储不可用时，默认允许在提示后退回本机私有账号目录中的明文存储。要禁止这种后备方式，请在明确加载的[配置文件](#configuration)中设置 `"allow_plaintext_fallback": false`。`--state-dir` 不会移动或隔离账号存储。新输入的登录默认只在内存中，除非你明确选择 `--remember`；移除已记忆登录使用 `logout`。

<a id="download-options"></a>
## 下载参数与生成文件

`download` 可以一次接收多个输入，同一组选项应用于每个输入选中的分 P：

```sh
./bin/bili download VIDEO_ID OTHER_ID --parts all --subtitles none --danmaku none
```

| 参数 | 默认值与作用 |
|---|---|
| `--parts all` 或 `--parts 1,3-5` | 默认 `all`；正整数原始序号或范围，不加空格；不存在的序号会报错 |
| `--output DIR` | 新下载的目标目录，覆盖配置中的 `output_dir` |
| `--quality highest` 或 `--quality QUALITY_ID` | 默认 `highest`；也可指定正整数服务端清晰度档位 ID |
| `--video-repr ID`、`--audio-repr ID` | 指定音视频流的精确约束 |
| `--codec CODEC`、`--hdr MODE` | 指定清晰度结果中的编码或动态范围 |
| `--subtitles all\|none\|LANGS\|track:ID,...` | 默认 `all`；逗号分隔的语言标签或轨道 ID，不能混用 |
| `--danmaku current\|none` | 默认 `current`；保存当前可获得弹幕，不下载历史快照 |
| `--merge auto\|manual` | 默认 `auto`；`manual` 保留下载资源，之后使用受管任务 `merge` |
| `--jobs N` | 并发任务数，默认 3 或配置值；支持 1–128 |
| `--connections N` | 共享连接上限，默认 6 或配置值；支持 1–256 |
| `--keep-temp` | 成功完成后仍保留程序拥有的临时资源；默认关闭 |
| `--list-resolutions` | 仅查询清晰度，不创建任务或输出目录 |
| `--auth MODE`、`--remember` | 使用哪个账号、是否明确保存；见[登录](#login) |

`--list-resolutions` 可以配合 `--parts`、登录参数和 `--connections`，但不能显式配合 `--output`、`--quality`、精确选流/编码/HDR、字幕/弹幕/合并参数、`--jobs`、`--keep-temp`。即使显式填写的值恰好等于普通下载默认值，也会冲突。

字幕语言标签或轨道 ID 必须与来源元数据匹配。选 `all` 时，来源确实没有字幕不算失败；明确请求了不存在的语言或轨道则会报告。JSON 字幕会保存原始 JSON 和 SRT，来源提供的 ASS 则保存为 ASS。弹幕保存为媒体旁的 JSONL、XML 和 ASS 文件。这些都是单独文件，不会烧录进画面。并非每个来源都有全部附件；请求的附件下载失败时，即使视频可用，命令也会返回非零结果。

<a id="resume"></a>
## 查看、中断与恢复任务

**按一次 Ctrl+C** 会请求安全暂停，保存可恢复状态，并等待工作停止。再按一次则强制退出，需要走崩溃恢复。CLI 没有独立的跨进程 `pause`、`cancel` 命令，也不会在退出后留下下载 daemon。

```sh
./bin/bili --state-dir "./bili-state" download VIDEO_ID --parts 1
./bin/bili --state-dir "./bili-state" jobs
./bin/bili --state-dir "./bili-state" resume JOB_ID
./bin/bili --state-dir "./bili-state" resume --all
```

`JOB_ID` 取自进度或最终结果，且要使用下载时的**同一状态目录**；未覆盖时，各命令都使用平台默认目录。恢复使用本次选择的账号，任务记录不会保存原 Cookie。需要账号的内容，可能仍需已记忆登录或再次 `--auth qr`。

`jobs` 是只读查看，可与下载进程并行。同一状态目录只能有一个写入会话；第二个下载、恢复、合并或清理进程会报告 `Busy`。强制退出后，`jobs` 可能仍显示上次记录的运行中状态；写入会话会先核对中断任务，再执行恢复，显示的字节数本身不作为可靠断点。异常关机和断电恢复尚未充分验收，不要通过删除锁文件、断点文件或手改 JSON 强迫恢复。

`resume --all` 包括未完成任务，以及视频已完成但请求附件失败的任务。附件重试会复用已验证的视频，不会把所有成功任务重新下载。普通续传或重试不等于精准损坏段修复；媒体身份改变、损坏和输出冲突仍会明确报错。

<a id="merge"></a>
## 自动合并与手动合并

普通下载默认 `--merge auto`，成功完成意味着已经在进程内重新封装音视频并校验。想稍后再合并：

```sh
./bin/bili download VIDEO_ID --parts 1 --merge manual
./bin/bili merge JOB_ID
```

合并前保留该任务的状态和资源。手动下载的 `Downloaded` 表示资源已保存，不等于最终可播放文件已经发布到目标路径。

合并已有本地媒体时，使用路径而非任务 ID：

```sh
./bin/bili merge --video "./video.media" --audio "./audio.media" --output "./combined.mp4"
```

来源已包含所需流时可以不加 `--audio`。`--video` 必须配合 `--output`，本地路径模式与 `JOB_ID` 二选一。此操作只重新封装，不转码、不启动播放器，也不调用 FFmpeg 可执行程序。不会覆盖已有目标文件，也不会删除非受管输入文件。

<a id="cleanup"></a>
## 清理临时文件

先预览程序将删除的具体路径和字节数：

```sh
./bin/bili clean JOB_ID --dry-run
./bin/bili clean JOB_ID
```

默认保留可恢复的未完成数据，跳过运行中的任务。确实要丢弃一个已停止任务的下载资源时，先预览，再明确允许清理可恢复文件：

```sh
./bin/bili clean JOB_ID --include-resumable --dry-run
./bin/bili clean JOB_ID --include-resumable
```

`--abandoned` 按未完成、非 `Downloaded` 状态选任务，与指定一个 `JOB_ID` 二选一；它仍需 `--include-resumable` 才会删除那些任务的可恢复文件。例如 `clean --abandoned --include-resumable --dry-run` 只预览。不要在合并前清理手动下载的 `Downloaded` 任务。清理只移除确切归属本任务的临时资源或合并暂存文件，保留最终媒体和其他用户文件，不移除任务历史，也不退出登录。移除断点资源后，可能需要重新下载相应资源。

<a id="configuration"></a>
## 配置文件与目录

可创建普通 UTF-8 JSON 文件，例如 `bili-config.json`：

```json
{
  "state_dir": "./bili-state",
  "output_dir": "./downloads",
  "jobs": 3,
  "connections": 6,
  "allow_plaintext_fallback": false,
  "update": {
    "allow_pre_release": false,
    "check_updates": true,
    "auto_update": false
  }
}
```

```sh
./bin/bili --config "./bili-config.json" download VIDEO_ID --parts 1
./bin/bili --config "./bili-config.json" jobs
./bin/bili --config "./bili-config.json" resume JOB_ID
```

想使用该配置的**每次调用**都要传 `--config`。程序不会自动创建或寻找配置文件，也不要在其中存凭据。文件上限 64 KiB，未知顶层字段会报错。Alpha 6 不接受 `update` 配置块，使用该发布版时需省略此块。

显式 `--state-dir` 和下载的 `--output`、`--jobs`、`--connections` 优先于配置，配置优先于内置默认值。相对路径按运行命令时的工作目录解析，不按配置文件所在目录。默认状态与输出位于平台的 `bili` 本地数据目录，`doctor` 会显示生效路径；账号存储仍是单独的位置。Windows JSON 路径可用正斜杠或转义反斜杠，例如 `"C:/Users/NAME/Videos/Bilibili"`。

<a id="updates"></a>
## 检查与安装更新

当前源码将检查与安装分开：

```sh
./bin/bili update --check
./bin/bili update
./bin/bili update --check --pre-release
./bin/bili update --pre-release
./bin/bili update --check --pre-release=false
```

PowerShell 使用 `.\bili.exe update ...`。默认稳定渠道只考虑稳定版本。`--pre-release` 使本次命令也考虑预发布版；即使配置允许预发布，`--pre-release=false` 仍可强制稳定渠道。要长期允许预发布，在明确加载的配置中设置 `update.allow_pre_release`；命令开关不会改写配置。

检查读取官方固定 HTTPS 清单，再使用对应 Tag 的描述文件和软件包，不需要 Releases API key。没有匹配 CLI 产品及平台描述文件的版本会跳过。仅有更高版本号不代表有兼容更新，`NoCompatibleRelease` 与下载失败含义不同。

安装需要位于可写目录中的兼容版本目录软件包；保留标记文件、固定入口、`CURRENT` 和 `versions/`。更新会先校验独立候选版本及其原生运行能力，再切换 `CURRENT`，保留旧版本和用户文件。未打包的开发可执行程序可以检查，但不能安装。Alpha 6 不能自我更新，首次兼容包要手动解压到新目录；只有实际发布了匹配资产，才有 GUI 或 Windows 更新可供安装。

`update.check_updates` 默认开启。交互终端中的搜索、信息、清晰度、下载、恢复和合并命令可能在所选渠道每 24 小时至多检查一次，并提示新版本。检查有短时限，失败不打扰用户，不安装、不改变原命令退出码。JSON、非交互调用、帮助、版本、任务查询、doctor 和账号命令不触发此提示。关闭 `check_updates` 只关闭自动提示，不阻止显式 `update --check`。`auto_update` 仅供 GUI 使用；CLI 应保持 false，显式更新命令会拒绝 true。

<a id="output"></a>
## JSON、退出码与常见问题

```sh
./bin/bili search "KEYWORD" --type video --json --auth anonymous
./bin/bili download VIDEO_ID --parts 1 --json --auth anonymous
./bin/bili jobs --json
./bin/bili doctor --json
```

`--json` 在标准输出逐行写入 `{"schema_version":1,"type":"...","data":...}`。一次下载可产生 `plan`、`job`、`result`、`error` 等多条记录，应逐行读取，而不是把全部输出当成一个 JSON 文档。结果包含任务 ID、各组件结果和每任务 `exit_code`。其他类型包括 `search`、`info`、`qualities`、`resolutions`、`auth`、`jobs`、`cleanup`、`doctor`、`update`。人工可读进度与提示使用标准错误。即使加了 `--json`，Clap 帮助和参数解析错误仍是普通命令行文本。

| 退出码 | 含义 |
|---|---|
| `0` | 请求操作成功或已验证完成；更新检查成功不代表一定有新版本 |
| `2` | 命令、选择、配置无效，或参数组合不支持 |
| `3` | 登录、权益、地区、额外验证、可用性或已选媒体身份限制 |
| `4` | 网络、存储、Busy、损坏、原生库或合并等运行错误 |
| `130` | 被中断、暂停或取消 |

批量命令保留各项结果，并返回相关结果中的最高退出码。视频成功但请求的字幕或弹幕失败，仍返回非零。解决权限、网络、存储问题后，可用 `resume JOB_ID` 重试。遇到 `Busy`，先停止使用同一状态目录的其他写入进程；任务找不到时，检查 `--config`、`--state-dir` 与工作目录。遇到 `unknown argument`，先看 `--version` 和该可执行程序的 `--help`，确认是否使用了新版源码专有参数。原生库加载或启动问题应检查完整软件包，不是安装一个 `ffmpeg` 命令；构建排查见 [BUILDING.md](BUILDING.md)。

恢复、吞吐表现和公开服务排序尚未在全部支持系统上充分验收。Aria2、精准损坏段修复和验证 WebView 不是当前 CLI 能力。应用程序采用[附加 Commons Clause 的 MIT 许可证](LICENSE)；FFmpeg 独立适用 LGPL-2.1-or-later，保留随包对应的声明和源码材料。
