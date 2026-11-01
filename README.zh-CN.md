# Bilibili Anime Downloader

[English](README.md) | 简体中文

这是一个用于搜索和下载 Bilibili 视频的 Rust 命令行工具。下载后可将音视频合成为可播放文件，也可保存字幕和弹幕文件。`bili_core` 可供 Rust 程序复用；`bili` 是命令行客户端。

- [CLI 使用指南与完整命令表](CLI.zh-CN.md)：登录、搜索、清晰度选择、下载、恢复、清理、配置和更新。
- [原生 GUI 构建与更新说明](gui/README.md)。
- [开发构建环境](BUILDING.md)。

## 下载与当前状态

`2.0.0-alpha.6` 是面向 Linux x86_64 的命令行试用版。软件包和校验文件现已在[该版本的发布页面](https://github.com/SweerItTer/BilibiliAnimeDownloader/releases/tag/v2.0.0-alpha.6)提供。发布 CI 进行离线验证；在线检查是单独进行且须明确授权的人工验收，外部服务可能无法访问。更早的版本是旧 Python 程序。本次不提供 Windows 包；macOS 不在范围内。

从该页面下载 Linux 软件包，并用提供的校验文件核对下载是否完整。解压后保持 `bin/` 与 `lib/` 目录在一起，在解压目录中运行 `./bin/bili --help`。

## 快速开始

将 `KEYWORD` 替换为搜索词，将 `VIDEO_ID` 替换为搜索结果中的 ID（例如 `BV...` 或 `av` 加数字 ID）。搜索不会自动开始下载。以下命令使用匿名访问，不会读取已保存的登录：

```sh
./bin/bili search "KEYWORD" --type video --page 1 --auth anonymous
./bin/bili qualities VIDEO_ID --auth anonymous
./bin/bili download VIDEO_ID --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

`qualities` 查询单个分 P，不开始下载；加 `--part 3` 可查询原始序号为 3 的分 P。选好后使用 `download VIDEO_ID --quality QUALITY_ID`，像素尺寸和 Stream ID 不是清晰度档位 ID。用 `--parts 1,3-5` 选择下载分 P。当前源码还提供 `download VIDEO_ID --list-resolutions` 批量列出各分 P 清晰度，Alpha 6 没有此参数。加 `--json` 可获得机器可读输出。Windows 源码构建在 PowerShell 中使用 `.\bili.exe`，参数相同。

CLI 会显示已完成文件的路径；不会安装或启动播放器。接下来可阅读[完整 CLI 指南](CLI.zh-CN.md)，或运行 `./bin/bili download --help` 查看某条命令的参数。

## 检查和安装更新

**由当前源码构建**的 CLI 支持在 Linux 和原生 Windows 上检查并手动安装兼容更新。已发布的 `2.0.0-alpha.6` 软件包没有 `update` 子命令，首次使用兼容新布局的软件包需要手动安装：

```sh
./bin/bili update --check
./bin/bili update
```

默认只考虑稳定版。加 `--pre-release` 可在本次命令中同时考虑预发布版，`--pre-release=false` 则强制使用稳定渠道。检查使用固定 HTTPS 清单，并要求版本提供匹配产品和平台的软件包。安装会先校验独立的新版本，再切换到它，并保留旧版本。CLI 只有在你明确运行 `update` 时才安装。软件包布局、Windows 命令、长期偏好和自动检查行为见[更新与兼容性](CLI.zh-CN.md#updates)。

## 登录与隐私

想在后续命令中保持登录，只需明确记忆一次：

```sh
./bin/bili login qr --remember
./bin/bili download VIDEO_ID --parts 1
```

后续命令会通过默认的 `--auth auto` 自动使用已保存账号，不用重新登录，也不必反复填写 `--auth stored`。只想为本次下载登录，可用 `download VIDEO_ID --auth qr`。单独运行不带 `--remember` 的 `login qr` 只验证本次登录，不会让下一个进程继续使用它。

优先使用操作系统安全存储；不可用时会提示可能使用明文存储。[登录说明](CLI.zh-CN.md#login)包含关闭明文后备、Cookie 输入、查看状态和退出登录的方法。不要将 Cookie 写进命令文本，也不要保存测试专用凭据。

只下载你有权保存的内容。本项目不会绕过登录、权益、地区限制或 DRM。网站要求额外验证时，请停止；程序会报错。

## 限制与构建

当前源码包含原生桌面 GUI；已发布的 Alpha 仍为 Linux x86_64 CLI，没有 Windows 或 GUI 发布包。Aria2、验证 WebView 和损坏分段修复尚未包含；稳定性、性能和异常退出恢复还没有充分验收。FFmpeg 以可替换共享库形式提供；不会随包提供或启动 FFmpeg 可执行文件，也不会启动播放器。应用程序采用[附加 Commons Clause 的 MIT 许可证](LICENSE)；FFmpeg 和其他第三方组件适用软件包声明中的独立条款。

开发构建要求和命令见 [BUILDING.md](BUILDING.md)。

## 社区

请参阅[贡献指南](CONTRIBUTING.md)来报告问题、提出功能建议或贡献代码，并阅读[行为准则](CODE_OF_CONDUCT.md)、[安全政策](SECURITY.md)和[无障碍说明](ACCESSIBILITY.md)。
