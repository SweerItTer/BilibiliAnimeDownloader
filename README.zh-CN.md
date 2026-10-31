# Bilibili Anime Downloader

[English](README.md) | 简体中文

这是一个用于搜索和下载 Bilibili 视频的 Rust 命令行工具。下载后可将音视频合成为可播放文件，也可保存字幕和弹幕文件。`bili_core` 可供 Rust 程序复用；`bili` 是命令行客户端。

## 下载与当前状态

`2.0.0-alpha.6` 是面向 Linux x86_64 的命令行试用版。软件包和校验文件现已在[该版本的发布页面](https://github.com/SweerItTer/BilibiliAnimeDownloader/releases/tag/v2.0.0-alpha.6)提供。发布 CI 进行离线验证；在线检查是单独进行且须明确授权的人工验收，外部服务可能无法访问。更早的版本是旧 Python 程序。本次不提供 Windows 包；macOS 不在范围内。

从该页面下载 Linux 软件包，并用提供的校验文件核对下载是否完整。解压后保持 `bin/` 与 `lib/` 目录在一起，在解压目录中运行 `./bin/bili --help`。

## 快速开始

将 `KEYWORD` 替换为搜索词，将 `VIDEO_ID` 替换为搜索结果中的 ID（例如 `BV...` 或 `av` 加数字 ID）。搜索不会自动开始下载。以下命令使用匿名访问，不会读取已保存的登录：

```sh
./bin/bili search "KEYWORD" --type video --page 1 --auth anonymous
./bin/bili download VIDEO_ID --list-resolutions --parts all --auth anonymous
./bin/bili download VIDEO_ID --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

`--list-resolutions` 会按分 P 列出清晰度档位 ID、实际尺寸、帧率、编码和可用状态，不开始下载。选好后使用 `download VIDEO_ID --quality QUALITY_ID`；像素尺寸和 Stream ID 不是清晰度档位 ID。缺少元数据或权限原因时显示未知。用 `--parts 1,3-5` 选择分 P，或用 `qualities VIDEO_ID --part 3` 查询单个分 P。加 `--json` 可获得机器可读输出。在 Windows PowerShell 中将程序路径替换为 `.\bili.exe`，参数相同。

CLI 会显示已完成文件的路径；不会安装或启动播放器。其他命令和选项请运行 `./bin/bili --help`。

## 检查和安装更新

**由本分支构建**的 CLI 支持检查更新，并从兼容的新布局软件包中手动安装更新。这项功能尚未正式发布，请在隔离的安装目录中试用。已发布的 `2.0.0-alpha.6` 软件包没有 `update` 子命令：

```sh
./bin/bili update --check
./bin/bili update
```

检查更新时读取固定的 HTTPS 静态清单，安装包仍直接从对应 Tag 的 GitHub Release 下载。客户端不调用 Releases API，也不需要 API key。`--pre-release` 仅对这一次命令同时考虑预发布版本（配置文件中已允许预发布时，`--pre-release=false` 仍可强制切回稳定渠道）。

本阶段交付 Linux CLI 更新。公开静态清单、实际安装包下载、新版本及原生库校验、版本切换和旧安装保留均已验证通过。Windows 自更新方案与验收后置；已记录的问题不阻塞本阶段 Linux 交付。

首次安装兼容的 Linux 软件包时，将完整目录解压到可写的安装位置。后续更新从正在运行的程序定位安装目录，准备并校验独立的新版本，再切换 `CURRENT`；不覆盖正在运行的版本，也不删除旧版本。

`2.0.0-alpha.6` 早于本协议，无法自我更新；首个兼容版本需要手工解压到新目录中安装。

共享协议、descriptor 格式、配置项、验证结果与已知限制记录在[升级协议说明](.trellis/spec/bili_core/backend/update-protocol.md)。其中的 `auto_update` 仅供 GUI 使用；CLI 只在你亲自执行 `bili update` 时才会安装，其余情况最多做一次检查。

## 登录与隐私

- 默认不保存登录。
- 单次命令可在需要登录的命令（例如 `download`）上使用 `--auth qr`。登录只在这条命令结束前有效。
- 若要在之后的命令中使用 QR 登录，请显式运行 `./bin/bili login qr --remember` 保存，再使用 `--auth stored`。
- 优先使用操作系统安全存储；若不可用，会提示，并可能以明文保存在本机个人配置目录。
- 不要保存测试专用凭据，也不要将 Cookie 放入命令文本、日志或报告。

只下载你有权保存的内容。本项目不会绕过登录、权益、地区限制或 DRM。网站要求额外验证时，请停止；程序会报错。

## 限制与构建

此 Alpha 不包含 GUI、Aria2、验证 WebView、搜索排序或损坏分段修复；稳定性、性能和恢复能力还没有充分测试。FFmpeg 以可替换共享库形式提供；不会随包提供或启动 FFmpeg 可执行文件，也不会启动播放器。应用程序采用[附加 Commons Clause 的 MIT 许可证](LICENSE)；FFmpeg 和其他第三方组件适用软件包声明中的独立条款。

开发构建要求和命令见 [BUILDING.md](BUILDING.md)。

## 社区

请参阅[贡献指南](CONTRIBUTING.md)来报告问题、提出功能建议或贡献代码，并阅读[行为准则](CODE_OF_CONDUCT.md)、[安全政策](SECURITY.md)和[无障碍说明](ACCESSIBILITY.md)。
