# ScreenHarbor

ScreenHarbor 是一款轻量、离线优先的 Windows 区域录屏工具。它专注于清晰的录制控制、可靠的本地文件输出，以及无需账户即可使用的简单体验。

ScreenHarbor is a lightweight, local-first screen recorder for Windows. It focuses on precise region capture, dependable local output, and a simple experience without accounts.

## 主要功能

- 智能识别窗口，也可以拖动自由框选录制区域。
- 同时录制麦克风和系统声音，并可在录制过程中静音。
- 输出 H.264/AAC MP4 高清母版。
- 可同时生成 MP4、WebM 或动态 WebP 轻量预览及双尺寸 WebP 封面。
- 支持开始倒计时、暂停/继续、多个显示器和高 DPI 缩放。
- 录制内容、设置和索引均保存在本地，不要求登录。
- 提供简体中文和英文界面。

## 系统要求

- 64 位 Windows 10 版本 2004 或更高版本，推荐 Windows 10 22H2 或受支持的 Windows 11 版本。
- 可用的 Windows Media Foundation H.264 编码器。
- 足够的本地磁盘空间；高清录制可能快速占用存储空间。

## 安装 Beta

1. 从正式发布页面下载名称类似 `ScreenHarbor_0.2.0-beta.1_x64-setup.exe` 的安装包。
2. 对照发布页面提供的 SHA-256 校验安装包。
3. 运行安装程序。当前 Beta 尚未代码签名，因此 Windows SmartScreen 可能显示“未知发布者”。请只从 ScreenHarbor 的正式发布页面获取安装包。
4. 启动 ScreenHarbor，点击“开始录屏”，或使用全局快捷键 `Alt + Shift + R`。

当前 Beta 尚未提供应用内自动更新。安装新版本前请退出正在进行的录制，然后运行新版安装程序覆盖升级。

## 数据与隐私

ScreenHarbor 不要求账户，且当前版本不会将录制、音频、设备名称、使用统计或崩溃报告上传到服务器。详细说明见 [PRIVACY.md](PRIVACY.md)。

使用本软件录制他人、会议或受版权保护的内容前，请自行确认已取得必要许可并遵守所在地法律和平台规则。

## Beta 反馈

反馈时建议附上 Windows 版本、ScreenHarbor 版本、显示器数量与缩放比例、录制设置以及复现步骤。不要提交包含私密内容的录制文件。

## 构建

需要 Node.js 22、Rust stable、Windows 构建工具和 PowerShell。

```powershell
npm install
npm test
npm run package:windows
```

维护者可使用下面的命令生成带校验文件的 Beta 发布目录：

```powershell
npm run release:beta
```

## 第三方组件

ScreenHarbor 以独立进程方式分发 LGPL 构建的 FFmpeg。安装包内附相应许可证、源码归档和构建配方；详情见 [third_party/ffmpeg/NOTICE.md](third_party/ffmpeg/NOTICE.md)。

ScreenHarbor 本身是专有软件，使用条件见 [LICENSE.txt](LICENSE.txt)。
