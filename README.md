# AM Light

Native Windows desktop Apple Music client built with Rust, Direct2D / DirectComposition, DirectWrite, WASAPI, and Apple's native `AMPLibraryAgent` COM service.

## Features
- **Native Direct2D & DirectComposition UI**: Frameless window with adjustable corner radius (`0..28 px`), optional Liquid Glass controls, smooth spring sidebar & scroll physics, and live window resize/maximize/restore adaptation.
- **Word- & Character-Synced Lyrics**: Supports TTML, YRC, and LRC lyrics with sub-pixel smooth vertical float-up (`DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC` + `D2D1_DRAW_TEXT_OPTIONS_NO_SNAP`), per-character CJK / optional per-word Latin timing, emphasis glow/zoom, instrumental break centering, and dual-pass GPU Gaussian depth-of-field blur.
- **Audio Visualizers & Signal Analysis**: Real-time WASAPI loopback FFT spectrum, lossless/Dolby Atmos format & cutoff inspection, soundstage scatter with phosphor persistence, RBMK/matrix 3D core views, and beat-locked tempo rings.
- **Per-Page Monitor-Aware Refresh Rate Controls**: Configure Visualizer and Lyrics refresh rates independently (`60`, `72`, `Adaptive [60/72]` synced automatically to the active monitor's refresh rate such as `60 Hz @ 240Hz` or `72 Hz @ 144Hz`, `VSync`, or `Custom 30..360 Hz`), plus an optional unfocused Lyrics frame-rate limiter (disabled by default so Lyrics keep full frame rate in the background).

## Requirements
- Windows 10 / 11 (x64)
- Apple Music for Windows (installed from Microsoft Store for `AMPLibraryAgent` playback & lossless catalog streaming)

## Installation
Download `AM-Light-Setup-v0.2.0-x64.exe` from the [GitHub Releases](../../releases) page and run it.

- Silent install: `AM-Light-Setup-v0.2.0-x64.exe --silent [--dir="C:\Path\To\AM-Light"]`
- Uninstall: Run `Uninstall-AM-Light.exe` from the install folder or Windows Installed Apps settings.

## Building from Source
1. Install Rust (GNU or MSVC toolchain) and MinGW-w64 `windres` (if using `x86_64-pc-windows-gnu`).
2. Build the main application:
   ```powershell
   cargo build --release
   ```
3. Build the Windows installer (`dist\AM-Light-Setup-v0.2.0-x64.exe`):
   ```powershell
   $env:AM_LIGHT_BIN_PATH = "target\release\AM-Light.exe"
   cargo build --release --manifest-path installer\Cargo.toml
   ```
