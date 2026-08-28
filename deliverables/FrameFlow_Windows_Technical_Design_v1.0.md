# FrameFlow Windows 技术设计文档

> 版本：v1.0  
> 平台：Windows 首发版  
> 状态：技术基线  
> 更新日期：2026-08-27

## 1. 文档目标

本文定义 FrameFlow Windows 首发版本的技术架构、模块边界和关键实现策略，作为工程拆分、技术验证和版本验收的共同基线。

首发目标不是构建一套“所有底层代码完全跨平台”的录屏引擎，而是：

- Windows 录制稳定、清晰、低资源占用。
- 主窗口、框选、悬浮条、托盘和状态流具备完整产品体验。
- 一次录制可生成主视频、视频预览或动态图片预览以及封面。
- 主视频与视频预览可以分别决定是否包含音频。
- 平台相关能力被隔离，为后续 macOS 适配保留清晰边界。

## 2. 首发范围

### 2.1 支持环境

- 推荐最低系统：Windows 10 版本 2004。
- 重点支持：Windows 10 22H2、Windows 11 当前受支持版本。
- 架构：x64 首发，ARM64 后续评估。
- 图形环境：DWM 开启，Direct3D 11 可用。
- DPI：Per-Monitor V2，支持多显示器不同缩放比例。

选择 Windows 10 2004 作为建议下限，是因为录制悬浮窗口可使用 `WDA_EXCLUDEFROMCAPTURE` 从捕获内容中排除；该值从 Windows 10 2004 开始正式支持。

### 2.2 MVP 功能

- 区域、显示器和窗口录制。
- 自定义框选、比例预设、实际像素尺寸显示。
- 麦克风与系统声音独立开关。
- 主视频：MP4、MOV、WebM，最大分辨率、帧率、包含音频。
- 预览：MP4、WebM、动态 WebP、动态 AVIF。
- 视频预览：最大分辨率、帧率、包含音频。
- 动态图片预览：短循环、固定无音频。
- 主视频和视频预览分别生成 JPG 封面。
- 录制中、暂停、处理、完成、异常状态。
- 主窗口最小化到托盘，托盘快速控制。
- 最近录制与保存目录管理。

### 2.3 暂不进入首发

- 摄像头画中画。
- 标注、鼠标点击效果和按键显示。
- 云上传、团队空间和在线转码。
- HDR 成片输出。
- ARM64 正式安装包。
- 高级码率、CRF、GOP 等专业参数。

## 3. 技术选型

| 层级 | 推荐技术 | 用途 |
| --- | --- | --- |
| 桌面外壳 | Tauri 2 | 窗口、托盘、安装包、更新、前后端 IPC |
| UI | React + TypeScript + Vite | 主窗口、展开设置、结果页、状态展示 |
| UI 状态 | Zustand | 仅保存前端展示状态和临时表单状态 |
| UI 基础能力 | Radix UI + 自定义 CSS | 可访问控件、下拉框、开关和轻盈通透视觉 |
| 应用核心 | Rust stable | 状态机、任务调度、文件管理、平台适配 |
| Windows API | `windows` crate | Win32、WinRT、Direct3D、WASAPI、Media Foundation |
| 异步任务 | Tokio | IPC、文件任务、处理队列和取消信号 |
| 序列化 | Serde | 设置、任务清单、前后端数据 |
| 错误模型 | thiserror + anyhow | 分层错误与顶层诊断 |
| 日志 | tracing | 本地结构化日志与问题诊断 |
| 主视频编码 | Media Foundation | H.264/AAC 和硬件编码优先 |
| 视频预览 | Media Foundation / libvpx | MP4/H.264 或 WebM/VP9 |
| 动态 WebP | libwebp | 短循环动画编码 |
| 动态 AVIF | libavif + AV1 编码后端 | 高压缩短循环动画编码 |
| 媒体封装与兼容处理 | 受控的 LGPL FFmpeg 构建 | 缩放、重封装、格式补偿和失败恢复 |
| 设置存储 | JSON / Tauri Store | 用户配置，支持原子写入 |
| 录制索引 | SQLite（rusqlite） | 最近录制、文件组和处理任务 |

### 3.1 为什么选择 Tauri

- UI 可以继续使用成熟的 Web 技术完成高质量桌面界面。
- Rust 后端可以直接管理 Windows 原生 API 和录制状态。
- 主窗口、框选窗口、悬浮条和托盘可以共享同一产品层。
- 后续 macOS 可以复用 React UI、Rust 状态机、配置、任务和文件模型。

Electron 可快速开发普通桌面 UI，但屏幕捕获、系统声音、窗口排除和硬件编码仍需要原生模块。FrameFlow 的主要技术难度位于原生媒体管线，因此不把 Electron 作为首选。

## 4. 总体架构

```text
┌─────────────────────────────────────────────────────────┐
│ React / TypeScript                                      │
│ 主窗口 · 框选层 · 悬浮控制条 · 结果状态 · 托盘菜单       │
└──────────────────────┬──────────────────────────────────┘
                       │ Tauri Commands / Events
┌──────────────────────▼──────────────────────────────────┐
│ Rust Application Core                                  │
│ Recording State Machine · Settings · Jobs · Library    │
└───────────────┬──────────────────────┬──────────────────┘
                │                      │
┌───────────────▼────────────┐  ┌──────▼──────────────────┐
│ Windows Platform Adapter   │  │ Media Output Pipeline   │
│ WGC · D3D11 · WASAPI · HWND│  │ Main · Preview · Cover  │
└───────────────┬────────────┘  └──────┬──────────────────┘
                │                      │
                └──────────┬───────────┘
                           ▼
                 Recording Group Files
```

### 4.1 进程模型

首版采用两个进程边界：

1. `frameflow.exe`
   - Tauri 应用主进程。
   - 承载 UI、状态机、屏幕与音频采集、主视频硬件编码。
   - 录制路径必须尽量短，不依赖网络或外部服务。

2. `frameflow-media-worker.exe`
   - 受控媒体工作进程。
   - 承载 WebM、动态 WebP、动态 AVIF、补偿转码和恢复任务。
   - 崩溃不会带走正在录制的主进程。
   - 只接受结构化任务清单和白名单参数，不向 UI 暴露任意命令执行。

主视频稳定性优先于预览生成。任何预览任务积压、崩溃或编码过慢，都不能阻塞屏幕采集和主视频编码。

## 5. 工程结构建议

```text
frameflow/
├─ apps/
│  └─ desktop-ui/                 React / TypeScript
├─ src-tauri/
│  ├─ src/
│  │  ├─ commands/                前后端命令
│  │  ├─ events/                  状态事件
│  │  ├─ app_state/               应用共享状态
│  │  └─ windows/                 Tauri 窗口管理
│  └─ capabilities/               Tauri 最小权限配置
├─ crates/
│  ├─ frameflow-core/             状态机、领域模型、设置
│  ├─ frameflow-capture/          平台无关采集接口
│  ├─ frameflow-windows/          Windows 原生实现
│  ├─ frameflow-media/            帧流、缩放、编码接口
│  ├─ frameflow-storage/          SQLite、文件组、恢复
│  └─ frameflow-ipc/              主进程与媒体工作进程协议
├─ workers/
│  └─ media-worker/               WebM/WebP/AVIF/恢复任务
├─ third-party/
│  ├─ licenses/
│  └─ manifests/                  第三方版本与构建配置
├─ tests/
│  ├─ integration/
│  ├─ media-fixtures/
│  └─ recovery/
└─ docs/
```

## 6. 核心模块

### 6.1 App Shell

职责：

- 创建和恢复主窗口。
- 创建每个显示器的框选窗口。
- 创建录制区域上方的悬浮控制条。
- 管理托盘图标和右键菜单。
- 处理最小化、关闭到托盘、单实例和系统启动。

窗口清单：

| 窗口 | 技术形态 | 说明 |
| --- | --- | --- |
| `main` | Tauri WebView | 紧凑主窗口，包含所有设置 |
| `selection-{monitor}` | 透明无边框窗口 | 每个显示器一个，负责区域绘制 |
| `recording-toolbar` | 透明置顶窗口 | 录制状态、暂停、结束 |
| `notification` | 轻量窗口 | 完成、异常和权限提示 |

`recording-toolbar` 和录制期间可能出现的 FrameFlow 窗口需要调用 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`。该 API 适合用于让录制控制窗口不出现在捕获结果中，但不是 DRM 或安全防护机制。

### 6.2 Recording State Machine

Rust 核心是唯一可信状态源：

```text
Idle
  → Selecting
  → Ready
  → Countdown
  → Recording ⇄ Paused
  → Finalizing
  → Processing
  → Completed
  ↘ Failed
```

规则：

- React、悬浮条和托盘不自行推断录制状态。
- 所有操作先发送命令，由 Rust 校验当前状态是否允许，再广播新状态。
- 输出参数在 `Recording`、`Paused`、`Finalizing` 和 `Processing` 期间锁定。
- 最小化只是窗口状态，不改变录制任务状态。

核心事件：

- `recording://state-changed`
- `recording://timer`
- `recording://audio-level`
- `recording://output-progress`
- `recording://completed`
- `recording://failed`
- `devices://changed`

### 6.3 Capture Adapter

定义平台无关接口：

```text
CaptureAdapter
├─ list_displays()
├─ list_windows()
├─ start(source, region, fps)
├─ pause()
├─ resume()
└─ stop()
```

Windows 首版实现：

- `Windows.Graphics.Capture` 获取显示器或窗口帧。
- Direct3D 11 纹理作为内部视频帧载体。
- 区域录制采用“捕获所在显示器 + GPU 裁剪”，不使用逐帧 CPU 截图。
- 显示器尺寸或旋转变化时重建帧池和裁剪参数。
- 首版输出 SDR，检测 HDR 显示器但统一转换到 SDR。

### 6.4 Region Selection

框选需要同时处理物理像素、逻辑坐标和不同显示器 DPI：

- 进程清单声明 `PerMonitorV2`。
- UI 显示的是用户可理解的区域尺寸。
- 内核保存物理像素矩形：`display_id + x + y + width + height`。
- 跨显示器拖拽时，不允许生成一个横跨不同 GPU/DPI 空间的单区域；首版将选区限制在一个显示器内。
- 比例预设只改变框选矩形，不改变最终最大分辨率规则。
- 输出分辨率只缩小，不放大，并始终保持原始宽高比。

### 6.5 Audio Adapter

Windows 音频来源：

- 系统声音：WASAPI Loopback。
- 麦克风：WASAPI Capture。
- 设备变化：监听默认设备和设备列表变化。

音频处理：

```text
System Audio ─┐
              ├─ Resample / Clock Sync ─ Audio Bus ─ Output Sinks
Microphone ───┘
```

- 内部统一为固定采样率的 PCM 流。
- 使用高精度时钟与视频时间戳对齐。
- 主视频和视频预览分别订阅音频总线。
- 每个输出的“包含音频”开关只控制对应输出 Sink。
- 动态 WebP 和动态 AVIF 不订阅音频总线。
- 用户在主窗口关闭“包含音频”不等于关闭麦克风输入；前者决定文件是否带声音，后者决定声音是否被采集。

### 6.6 Frame Hub

屏幕只采集一次，统一帧流按消费者分发：

```text
Windows Graphics Capture
          │
          ▼
   D3D11 Frame Hub
    │      │       │
    │      │       └─ Cover Sampler
    │      └─ Preview Downscale / Frame Sampling
    └─ Main Video Encoder
```

原则：

- 主视频是高优先级消费者。
- 预览使用独立有界队列，不能对主视频施加反压。
- 视频预览可降低到 15/24/30 fps。
- 预览队列落后时允许有节奏地跳帧，但保持时间轴正确。
- 如果实时预览编码跟不上，停止录制后从主视频补做，不中断主视频。
- 封面采样只保留少量候选帧，不缓存完整录制。

## 7. 输出管线

### 7.1 输出配置模型

```text
OutputConfig
├─ main
│  ├─ format: mp4 | mov | webm
│  ├─ max_resolution
│  ├─ fps
│  └─ include_audio
├─ preview
│  ├─ enabled
│  ├─ format: mp4 | webm | animated_webp | animated_avif
│  ├─ max_resolution
│  ├─ fps
│  └─ include_audio
└─ cover_enabled
```

约束：

- `animated_webp`、`animated_avif` 强制 `include_audio = false`。
- 预览帧率不能高于主视频帧率。
- 主视频和预览最大分辨率都是上限，不是强制输出尺寸。
- 预览关闭时不创建任何预览任务。
- 封面开启且存在预览输出时，主视频和预览各生成一张 JPG；预览为动态图片时规则不变。

### 7.2 主视频

MP4 优先路径：

- D3D11 帧直接进入 Media Foundation H.264 硬件编码器。
- 音频开启时使用 AAC。
- 使用临时文件写入，成功完成后原子重命名。
- 硬件编码器不可用时回退到系统软件编码器，并提示性能模式变化。
- 主视频选择 MOV 或 WebM 时，采集与高质量编码仍保持高优先级，最终封装或兼容编码交给媒体工作进程；失败不得删除可恢复的高质量中间文件。

### 7.3 MP4 视频预览

- GPU 缩放到最大 720p/480p/360p/240p。
- 默认 480p、15 fps。
- 使用独立 Media Foundation H.264 编码实例。
- 音频开启时从音频总线生成低码率 AAC。

### 7.4 WebM 视频预览

- 默认 VP9 + Opus。
- 由媒体工作进程处理。
- 实时队列性能足够时边录边生成；落后时自动切换为停止后补做。
- 工作进程失败不影响已完成的主视频。

### 7.5 动态 WebP

- 使用 libwebp 动画编码能力。
- 固定无音频。
- 默认最大 360p、10 fps、短循环。
- 不将完整长视频全部编码为动图，避免产生比视频更大的文件。
- UI 只显示“动态 WebP”，不解释内部片段选择机制。

### 7.6 动态 AVIF

- 使用 libavif 和 AV1 编码后端。
- 固定无音频。
- 默认最大 360p、8–10 fps、短循环。
- 编码开销高于 WebP，始终在低优先级媒体工作进程中执行。
- 超过处理时间或编码失败时保留主视频，并允许用户重试或改用动态 WebP。

### 7.7 封面

- 格式固定 JPG，界面只提供一个“生成封面”开关。
- 主视频封面跟随主视频实际尺寸。
- 视频预览封面跟随视频预览实际尺寸。
- 文件与对应视频保存在同一录制组目录。
- 不向用户展示选帧方式、质量值或内部规则。

## 8. 文件组织

每次录制是一个 Recording Group：

```text
Recordings/
└─ 2026-08-27/
   └─ 产品演示_10-24-16/
      ├─ 产品演示_main.mp4
      ├─ 产品演示_main.jpg
      ├─ 产品演示_preview.webm
      ├─ 产品演示_preview.jpg
      └─ recording.json
```

动态图片预览示例：

```text
产品演示_preview.webp
产品演示_preview.avif
```

写入规则：

- 处理中使用 `.part` 临时后缀。
- 完成后刷新文件并原子重命名。
- `recording.json` 记录任务 ID、时间、区域、配置、实际输出尺寸和每个文件状态。
- SQLite 只保存索引，文件清单以录制目录内的 manifest 为恢复依据。

## 9. 失败与恢复

### 9.1 失败隔离

- 主视频失败：停止相关预览任务并保留可恢复临时文件。
- 预览失败：主视频继续并可正常交付。
- 封面失败：视频保持完成状态，封面可单独重试。
- 媒体工作进程崩溃：自动重启一次；仍失败则记录为可重试任务。

### 9.2 启动恢复

应用启动时扫描：

- SQLite 中未完成任务。
- 保存目录中的 `.part` 文件。
- 未完成的 `recording.json`。

恢复结果只呈现用户可理解的三类：

- 可以继续处理。
- 主视频可用，预览需要重试。
- 文件损坏，保留临时文件或丢弃。

### 9.3 边界情况

- 磁盘空间不足：录制前检查，录制中持续监控。
- 保存磁盘断开：立即停止并尝试将可恢复数据迁移到本地临时目录。
- 休眠/锁屏：记录时间轴中断，恢复后提示继续或结束。
- 显示器拔出或分辨率变化：停止录制并保存当前可用内容，不静默切换显示器。
- 音频设备断开：视频继续，音频轨标记中断并提示。
- 受保护内容：遵循 Windows 系统捕获结果，不尝试绕过保护。

## 10. IPC 与权限边界

前端只可调用领域命令：

```text
get_app_snapshot
update_next_recording_config
begin_selection
start_recording
pause_recording
resume_recording
stop_recording
open_recording
open_recording_folder
retry_output
```

不向 WebView 开放：

- 任意文件系统路径写入。
- 任意 shell 命令。
- 任意 FFmpeg 参数。
- 原始 HWND、D3D 指针或设备句柄。

媒体工作进程使用版本化任务协议，字段经过 Rust 反序列化校验；输出路径必须位于当前录制组或受控临时目录。

## 11. 本地数据与隐私

- 录制、封面和转码全部本地完成。
- 首版不上传文件、帧、声音或文件名。
- 日志默认不记录完整用户路径，只记录匿名任务 ID 和错误码。
- 崩溃报告默认关闭或明确征得同意。
- 卸载应用不自动删除用户录制文件。
- “退出 FrameFlow”在录制或处理期间必须二次确认。

## 12. FFmpeg 与第三方许可证策略

FrameFlow 只分发经过审计的 LGPL 构建：

- 禁止 `--enable-gpl`。
- 禁止 `--enable-nonfree`。
- 不包含 libx264、libx265 等会改变整体许可证条件的模块。
- 优先使用动态库或独立媒体工作进程。
- 保存并发布准确的 `ffmpeg -L`、`ffmpeg -buildconf`、源码版本和构建脚本。
- 安装包包含第三方许可证，应用“关于”中提供声明。

libwebp、libavif、libvpx、AV1 编码后端也必须锁定版本、保存许可证并纳入依赖清单。正式商业发行前进行一次二进制级许可证审计。

## 13. 性能目标

| 指标 | 目标 |
| --- | --- |
| 开始录制响应 | 点击后 1 秒内进入倒计时或录制 |
| 录制帧稳定性 | 主视频优先，预览拥塞不得阻塞主流 |
| 1080p 30 fps | 主流 Windows 设备稳定录制 |
| 1080p 60 fps | 支持硬件编码设备，动态降级可提示 |
| 内存 | 不缓存完整录制；使用有界帧队列 |
| 停止响应 | 立即停止采集并进入文件生成状态 |
| 预览可用性 | 低清输出尽可能先完成，失败可重试 |
| 长时录制 | 首发验收至少覆盖连续 2 小时 |

## 14. 测试策略

### 14.1 自动化测试

- 状态机转移和非法操作测试。
- 输出配置约束测试。
- 分辨率按比例缩小测试。
- 音频开关组合测试。
- WebP/AVIF 自动无音频测试。
- 文件命名、原子写入和恢复清单测试。
- IPC 参数白名单和路径越界测试。

### 14.2 集成测试

- WGC → D3D11 → Media Foundation 完整录制。
- WASAPI 系统声音与麦克风混合。
- 主视频与预览音频四种组合。
- MP4、WebM、WebP、AVIF 输出。
- 暂停/继续后的时间轴同步。
- 媒体工作进程崩溃与重试。

### 14.3 设备矩阵

- Intel、AMD、NVIDIA GPU。
- 单显示器、多显示器、横竖屏组合。
- 100%、125%、150%、200% 混合 DPI。
- 内置声卡、USB 麦克风、蓝牙耳机。
- Windows 10 22H2、Windows 11 主流版本。
- HDR 显示器以 SDR 录制。
- 低磁盘空间、设备断开、锁屏和睡眠恢复。

## 15. 可观测性

每次录制分配 `recording_id`，贯穿以下日志：

- 状态转换。
- 捕获源和实际像素尺寸。
- 目标与实际帧率。
- 丢帧、预览队列积压和编码器回退。
- 音频设备变化和时间轴漂移。
- 每个输出任务的耗时、结果和错误码。

日志分级：

- `info`：生命周期和用户可见状态。
- `warn`：降级、设备变化、预览落后。
- `error`：输出失败、文件不可写、编码器失败。
- `debug`：仅开发版本记录帧管线细节。

## 16. 构建与发布

- Rust、Node 和第三方媒体库全部锁定版本。
- CI 构建 x64 release，执行单元测试、集成测试和许可证扫描。
- 产出 NSIS 或 MSI 安装包。
- `frameflow.exe`、`frameflow-media-worker.exe` 和安装包统一代码签名。
- 更新包同样签名，并通过 Tauri Updater 校验。
- 发布前运行全格式烟雾测试并检查 `ffmpeg -L` 与 `-buildconf`。

## 17. 实施阶段

### 阶段 A：技术风险验证

- WGC 捕获区域。
- D3D11 裁剪与 Media Foundation H.264。
- WASAPI 系统声音和麦克风。
- 悬浮条排除捕获。
- 混合 DPI 坐标一致性。

完成标准：连续录制 30 分钟，画面、声音和区域均正确。

### 阶段 B：完整录制闭环

- 主窗口与状态机。
- 框选、倒计时、录制、暂停、结束。
- 主视频 MP4 输出。
- 托盘与最小化。
- 基础异常处理。

### 阶段 C：多输出

- MP4/WebM 视频预览。
- 主视频和预览独立音频开关。
- 动态 WebP、动态 AVIF。
- 封面和结果文件组。
- 处理进度、重试和失败隔离。

### 阶段 D：稳定性与发行

- 长时录制与设备矩阵。
- 恢复、磁盘和睡眠场景。
- 安装、签名、更新与许可证审计。
- 性能回归和发布验收。

## 18. 后续 macOS 迁移边界

可以直接复用：

- React UI 和视觉系统。
- Rust 状态机和领域模型。
- 输出配置、录制组、SQLite 和恢复逻辑。
- 媒体工作进程中的 WebM/WebP/AVIF 能力。
- 前后端 IPC 协议和测试用例。

需要替换：

| Windows | macOS |
| --- | --- |
| Windows.Graphics.Capture | ScreenCaptureKit |
| Direct3D 11 texture | IOSurface / CVPixelBuffer |
| WASAPI | ScreenCaptureKit Audio / Core Audio |
| Media Foundation | VideoToolbox / AVFoundation |
| HWND / Win32 overlay | NSWindow / NSPanel |
| WDA_EXCLUDEFROMCAPTURE | ScreenCaptureKit 内容过滤 |

因此工程层面的关键约束是：任何 Windows API 类型都不能穿透到 `frameflow-core`、React UI 或持久化模型中。

## 19. 官方技术参考

- [Tauri 2 Capability](https://v2.tauri.app/reference/acl/capability/)
- [Tauri 2 Sidecar](https://v2.tauri.app/develop/sidecar/)
- [Windows Graphics Capture](https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture)
- [Windows 图形开发入口](https://learn.microsoft.com/en-us/windows/apps/develop/graphics)
- [WASAPI Loopback Recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording)
- [Media Foundation H.264 Encoder](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder)
- [SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)
- [Windows Per-Monitor V2 DPI](https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process)
- [FFmpeg License and Legal Considerations](https://ffmpeg.org/legal.html)

## 20. 技术结论

Windows 首发版采用：

```text
Tauri 2 + React + TypeScript
Rust Application Core
Windows.Graphics.Capture + Direct3D 11
WASAPI
Media Foundation
受控媒体工作进程
libvpx + libwebp + libavif
LGPL FFmpeg 兼容层
```

这套架构将录制稳定性放在第一优先级：主视频由 Windows 原生硬件能力保证，预览和动态图片作为隔离的低优先级任务运行；同时保留清晰的平台适配边界，为后续 macOS 版本复用产品层和业务层。
