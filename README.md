# AM Light

基于 Rust 与 Windows 原生 Direct2D / DirectComposition 构建的轻量级 Apple Music 桌面客户端，提供亚像素逐字/逐词滚动歌词、实时音频频谱可视化与自适应高刷渲染体验。

> [!WARNING]
> **重要免责与稳定性声明（使用前请仔细阅读）**
> - 本项目当前处于 **不稳定版本（Unstable / Experimental）**，可能存在各类界面、兼容性或播放同步问题（Bug）。
> - 本项目由个人维护，**随时有可能停止更新（断更）或直接删除、下架消失**，不承诺长期维护、功能可用性或问题响应。
> - 本软件及内置工具仅供个人学习与技术研究使用，**作者及贡献者不对因下载、安装或使用本项目所造成的任何直接、间接后果（包括但不限于系统异常、账号风险或数据损失）承担任何责任**。使用即代表您已了解并自行承担全部风险。

---

## 使用流程（必读）

### 第一步：安装并登录原版 Apple Music（前置条件）
AM Light 通过对接微软商店版官方 Apple Music 的本地后台播放服务实现无损流媒体播放与队列控制，**使用前必须完成以下步骤**：
1. 前往 **Microsoft Store** 安装官方 **Apple Music（Windows 版）**。
2. 打开原版 Apple Music，登录拥有有效订阅的 **Apple ID**（确认地区与订阅状态正常）。
3. 在原版客户端中**随便播放一首歌曲**，确认能够正常加载并出声（用于完成首次账号凭据与 FairPlay / ADI 设备授权初始化）。
4. 确认可正常播放后，即可关闭或切出原版窗口，启动 **AM Light** 进行日常播放。

### 若原版 Apple Music 无法播放 / 点歌秒切
如果您的电脑曾使用过部分第三方 iOS 签名工具（如 Sideloadly、爱思助手等），或者因系统目录权限异常导致原版 Apple Music 出现**点歌后直接秒切下一首、进度停在 `0:00` 且无声音**的 DRM 故障，可以通过以下任一方式解决：
- **方式一（内置工具）**：打开 AM Light 的 **「设置 -> 播放故障诊断与 DRM 修复」**，可直接无权限检测异常状态，并点击 **「一键修复」**（自动备份旧文件与 ACL/注册表、修复 `%ProgramData%\Apple Computer` 写入权限并清理第三方 DRM 重定向）。
- **方式二（独立脚本仓库）**：前往作者的专门开源修复项目按照指引排查修复：  
  https://github.com/N20xDuli/apple-music-windows-drm-fix

### 第二步：安装与启动 AM Light
1. 前往 [GitHub Releases](https://github.com/N20xDuli/AM-Light/releases) 下载最新的 Windows 安装包（如 `AM-Light-Setup-v0.2.6-x64.exe`）并运行安装。
   - 支持静默安装：`AM-Light-Setup-v0.2.6-x64.exe --silent [--dir="C:\Path\To\AM-Light"]`
   - 卸载方式：通过系统「已安装的应用」或安装目录下的 `Uninstall-AM-Light.exe` 卸载。
2. 启动 AM Light，在首页浏览或搜索歌曲，软件会自动连接本地 Apple Music 播放服务（亦可在设置中切换控制原版客户端）。

### 第三步：同步个人云端歌单与资料库（可选）
若需浏览个人资料库、同步自建歌单与最近添加：
1. 在浏览器打开 https://music.apple.com 并登录您的 Apple ID（若已在本机原版 Apple Music 登录且资料库可见，可先在设置尝试同步地区）。
2. 从浏览器开发者工具复制您的 `media-user-token`（或包含它的 Cookie 字符串）。
3. 打开 AM Light **「设置 -> 账号与地区」**，点击 **「粘贴保存」** 或在输入框按 `Ctrl+V`，再点击 **「验证」** 即可。
4. 您的凭据与歌单缓存**仅保存在本机本地目录**，本仓库与安装包中**不包含任何个人登录 Session、Cookie 或 Token**。

---

## 核心特性

- **原生 Direct2D / DirectComposition UI**：无边框可调圆角窗口（`0..28 px`）、平滑窗口拖拽缩放/最大化与还原自适应、平滑弹簧侧栏与惯性滚动、可选液态玻璃控件质感（默认关闭）。
- **逐字/逐词亚像素歌词系统**：支持 TTML / YRC / LRC 格式，支持 `DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC` + 亚像素定位平滑上浮、象形文字逐字与西文按词（可在设置切换）精准动效、强调段落高亮放大、间奏纯音乐自动居中与双通道 GPU 高斯景深模糊。
- **音频可视化与信号分析**：基于 WASAPI Loopback 的实时频谱、音质鉴真检测、声场散点图（带荧光视觉暂留）、高密度反应堆芯视图与多频段 Onset 节拍锁定圆环。
- **分页面显示器自适应帧率调度**：可分别为「可视化页面」与「歌词页面」设置 `60`、`72`、`自适应 [60/72]`（自动识别当前窗口所在显示器刷新率切换，如 240Hz 显示器自适应 60Hz、144Hz 显示器自适应 72Hz）、`垂直同步` 与 `自定义 (30..360 Hz)`；歌词页窗口失焦时默认保持满帧率渲染，也可按需在设置中开启失焦限帧。
- **低资源占用设计**：面向静态内存 `<= 100 MB`、静默无操作时 CPU/GPU 占用 `<= 1%` 进行节流与休眠调度。

---

## 环境要求

- Windows 10 / Windows 11（x64）
- 已通过 Microsoft Store 安装并登录的 **Apple Music（Windows 版）**

---

## 从源码构建

需要 Rust 工具链（`x86_64-pc-windows-gnu` 或 MSVC）；若使用 GNU 工具链需安装 MinGW-w64 并确保 `windres` 在 `PATH` 中。

```powershell
# 运行单元测试（Release）
cargo test --release --bin AM-Light

# 构建主程序
cargo build --release

# 打包 Windows 安装器（输出至 dist\AM-Light-Setup-v0.2.6-x64.exe）
$env:AM_LIGHT_BIN_PATH = "target\release\AM-Light.exe"
cargo build --release --manifest-path installer\Cargo.toml
```

---

## 致谢与参考说明

- 本项目的歌词解析、时间轴校准、逐字动效曲线与双通道模糊歌词界面**参考了 LyricsBlossom 的实现及其作者公开分享的技术资料**，在此致以诚挚感谢。
- Apple Music 无法播放/秒切排查与内置 DRM 修复逻辑集成自：[N20xDuli/apple-music-windows-drm-fix](https://github.com/N20xDuli/apple-music-windows-drm-fix)。

---

## 开源协议

本项目基于 **MIT License** 开源，完整协议文本请参阅 [LICENSE](LICENSE)。
