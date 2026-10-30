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
./bin/bili download VIDEO_ID --parts all --output "$HOME/Videos/Bilibili" --auth anonymous
```

CLI 会显示已完成文件的路径；不会安装或启动播放器。其他命令和选项请运行 `./bin/bili --help`。

## 登录与隐私

- 默认不保存登录。
- 单次命令可在需要登录的命令（例如 `download`）上使用 `--auth qr`。登录只在这条命令结束前有效。
- 若要在之后的命令中使用 QR 登录，请显式运行 `./bin/bili login qr --remember` 保存，再使用 `--auth stored`。
- 优先使用操作系统安全存储；若不可用，会提示，并可能以明文保存在本机个人配置目录。
- 不要保存测试专用凭据，也不要将 Cookie 放入命令文本、日志或报告。

只下载你有权保存的内容。本项目不会绕过登录、权益、地区限制或 DRM。网站要求额外验证时，请停止；程序会报错。

## 限制与构建

此分支新增原生桌面 GUI；Alpha 发布仍只针对 Linux x86_64，暂不发布 Windows 发布包。Aria2、验证 WebView、搜索排序或损坏分段修复尚未包含；稳定性、性能和恢复能力还没有充分测试。FFmpeg 以可替换共享库形式提供；不会随包提供或启动 FFmpeg 可执行文件，也不会启动播放器。应用程序采用[附加 Commons Clause 的 MIT 许可证](LICENSE)；FFmpeg 和其他第三方组件适用软件包声明中的独立条款。

开发构建要求和命令见 [BUILDING.md](BUILDING.md)。

## 社区

请参阅[贡献指南](CONTRIBUTING.md)来报告问题、提出功能建议或贡献代码，并阅读[行为准则](CODE_OF_CONDUCT.md)、[安全政策](SECURITY.md)和[无障碍说明](ACCESSIBILITY.md)。
