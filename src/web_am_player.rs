#![cfg(target_os = "windows")]

use crate::am_agent::{format_artwork_template, AgentTrackInput};
use crate::audio_viz::WM_APP_AUDIO_WAKE;
use crate::backends::WebAmStatus;
use crate::smtc::SmtcState;
use base64::{engine::general_purpose, Engine as _};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
    SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE,
};

static WEB_AM_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .no_proxy()
        .build()
        .expect("web am local cdp http client")
});

static WAKE_HWND: AtomicUsize = AtomicUsize::new(0);
static WEB_AM_ACTIVE: AtomicBool = AtomicBool::new(false);
static WEB_AM_SEQ: AtomicU64 = AtomicU64::new(0);
static CDP_CMD_ID: AtomicU64 = AtomicU64::new(1);

struct WebAmRuntime {
    child: Option<Child>,
    child_pid: Option<u32>,
    cdp_port: u16,
    window_visible: bool,
    storefront: String,
    authorized: bool,
    ready: bool,
    captured_user_token: Option<String>,
    last_state: SmtcState,
    queue: Vec<AgentTrackInput>,
    queue_index: usize,
    last_error: Option<String>,
    last_poll: Instant,
}

impl Default for WebAmRuntime {
    fn default() -> Self {
        Self {
            child: None,
            child_pid: None,
            cdp_port: 0,
            window_visible: true,
            storefront: "cn".to_string(),
            authorized: false,
            ready: false,
            captured_user_token: None,
            last_state: SmtcState::default(),
            queue: Vec::new(),
            queue_index: 0,
            last_error: None,
            last_poll: Instant::now() - Duration::from_secs(10),
        }
    }
}

static RUNTIME: Lazy<Mutex<WebAmRuntime>> = Lazy::new(|| Mutex::new(WebAmRuntime::default()));

pub fn set_wake_hwnd(hwnd: usize) {
    WAKE_HWND.store(hwnd, Ordering::Release);
}

#[inline]
pub fn is_active() -> bool {
    WEB_AM_ACTIVE.load(Ordering::Acquire)
}

#[inline]
pub fn state_seq() -> u64 {
    WEB_AM_SEQ.load(Ordering::Acquire)
}

fn wake_ui() {
    WEB_AM_SEQ.fetch_add(1, Ordering::AcqRel);
    let h = WAKE_HWND.load(Ordering::Acquire);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(h as *mut std::ffi::c_void)),
                WM_APP_AUDIO_WAKE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

pub fn get_queue_tracks() -> Vec<AgentTrackInput> {
    RUNTIME.lock().map(|g| g.queue.clone()).unwrap_or_default()
}

pub fn get_playback_state() -> SmtcState {
    RUNTIME
        .lock()
        .map(|g| g.last_state.clone())
        .unwrap_or_default()
}

pub fn take_captured_user_token() -> Option<String> {
    RUNTIME.lock().ok().and_then(|mut g| g.captured_user_token.take())
}

pub fn get_status() -> WebAmStatus {
    let Ok(g) = RUNTIME.lock() else {
        return WebAmStatus::default();
    };
    let running = g.cdp_port != 0;
    let status_text = if !running {
        "未启动（点击「启动 / 网页登录」或在曲库点击播放自动拉起 Edge 引擎）".to_string()
    } else if let Some(ref e) = g.last_error {
        format!("连接异常：{e}")
    } else if !g.ready {
        "正在加载 music.apple.com MusicKit JS 引擎...".to_string()
    } else if g.authorized {
        format!(
            "已就绪并已登录 Apple ID ({} 区 · 完整曲库可播)",
            g.storefront.to_uppercase()
        )
    } else {
        "引擎已就绪（未登录 Apple ID：可播放官方试听，点击「打开登录窗口」解锁完整版）"
            .to_string()
    };
    WebAmStatus {
        running,
        ready: g.ready,
        authorized: g.authorized,
        window_visible: g.window_visible,
        storefront: g.storefront.clone(),
        status_text,
    }
}

fn find_chromium_browser() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
        PathBuf::from(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
        PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(local);
        candidates.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(base.join(r"Google\Chrome\Application\chrome.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn web_profile_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("AMLight").join("web_am_profile");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn allocate_local_port() -> Result<u16, String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("无法分配本地调试端口: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取端口失败: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// Ensures isolated Edge/Chrome MusicKit web engine is running and responsive on CDP.
pub async fn ensure_started(storefront: &str, show_window: bool) -> Result<u16, String> {
    let sf = if storefront.trim().is_empty() {
        "cn"
    } else {
        storefront.trim()
    };

    let existing_port = {
        let mut g = RUNTIME.lock().map_err(|_| "mutex".to_string())?;
        if let Some(ref mut ch) = g.child {
            if ch.try_wait().ok().flatten().is_some() {
                g.child = None;
                g.child_pid = None;
                g.cdp_port = 0;
                g.ready = false;
            }
        }
        g.cdp_port
    };

    if existing_port != 0 {
        if find_am_tab_ws_url(existing_port).await.is_ok() {
            if show_window {
                set_window_visible(true);
            }
            return Ok(existing_port);
        }
    }

    let browser = find_chromium_browser().ok_or_else(|| {
        "未在本机找到 Microsoft Edge 或 Chrome，无法启动网页 Apple Music 引擎".to_string()
    })?;
    let port = allocate_local_port()?;
    let profile = web_profile_dir();
    let start_url = format!("https://music.apple.com/{sf}/home");

    let mut cmd = Command::new(&browser);
    cmd.arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--app={start_url}"))
        .arg("--autoplay-policy=no-user-gesture-required")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-timer-throttling")
        .arg("--disable-renderer-backgrounding")
        .arg("--window-size=980,700")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if !show_window {
        cmd.arg("--window-position=-2400,-2400");
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("启动 Edge 网页播放引擎失败: {e}"))?;
    let pid = child.id();

    {
        let mut g = RUNTIME.lock().map_err(|_| "mutex".to_string())?;
        g.child = Some(child);
        g.child_pid = Some(pid);
        g.cdp_port = port;
        g.window_visible = show_window;
        g.storefront = sf.to_string();
        g.ready = false;
        g.last_error = None;
    }
    WEB_AM_ACTIVE.store(true, Ordering::Release);

    // Wait up to 9 seconds for music.apple.com tab + MusicKit.getInstance()
    for _ in 0..36 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(ws) = find_am_tab_ws_url(port).await {
            if let Ok(val) = cdp_eval(&ws, "Boolean(window.MusicKit && window.MusicKit.getInstance && window.MusicKit.getInstance())", false).await {
                if val.as_bool() == Some(true) {
                    if let Ok(mut g) = RUNTIME.lock() {
                        g.ready = true;
                        g.last_error = None;
                    }
                    if !show_window {
                        set_window_visible(false);
                    }
                    let _ = poll_playback_state().await;
                    wake_ui();
                    return Ok(port);
                }
            }
        }
    }

    if !show_window {
        set_window_visible(false);
    }
    let _ = poll_playback_state().await;
    wake_ui();
    Ok(port)
}

/// Shows or hides the spawned Edge Apple Music app window cleanly.
pub fn set_window_visible(visible: bool) {
    let target_pid = {
        let Ok(mut g) = RUNTIME.lock() else { return };
        g.window_visible = visible;
        g.child_pid
    };
    struct EnumCtx {
        pid: Option<u32>,
        visible: bool,
    }
    let mut ctx = EnumCtx {
        pid: target_pid,
        visible,
    };
    unsafe extern "system" fn enum_proc(
        hwnd: HWND,
        lparam: LPARAM,
    ) -> windows::core::BOOL {
        let ctx = &*(lparam.0 as *const EnumCtx);
        let mut wpid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut wpid));
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        let title = if len > 0 {
            String::from_utf16_lossy(&buf[..len as usize])
        } else {
            String::new()
        };
        let is_am_edge = title.contains("Apple Music")
            && (ctx.pid.is_some_and(|p| p == wpid) || title.contains("music.apple.com") || title.ends_with("Apple Music"));
        if is_am_edge {
            if ctx.visible {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            } else if IsWindowVisible(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut EnumCtx as isize));
    }
    wake_ui();
}

pub fn shutdown() {
    let (port, child_opt) = {
        let Ok(mut g) = RUNTIME.lock() else { return };
        g.ready = false;
        g.authorized = false;
        g.last_state = SmtcState::default();
        let p = g.cdp_port;
        g.cdp_port = 0;
        g.child_pid = None;
        WEB_AM_ACTIVE.store(false, Ordering::Release);
        (p, g.child.take())
    };
    let _ = port;
    if let Some(mut ch) = child_opt {
        let _ = ch.kill();
        let _ = ch.wait();
    }
    wake_ui();
}

async fn find_am_tab_ws_url(port: u16) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{port}/json");
    let resp = WEB_AM_HTTP
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("CDP 端口不可达: {e}"))?;
    let tabs: Vec<Value> = resp
        .json()
        .await
        .map_err(|e| format!("解析 CDP 页面列表失败: {e}"))?;
    for tab in &tabs {
        if tab.get("type").and_then(Value::as_str) == Some("page") {
            let turl = tab.get("url").and_then(Value::as_str).unwrap_or_default();
            if turl.contains("music.apple.com") {
                if let Some(ws) = tab.get("webSocketDebuggerUrl").and_then(Value::as_str) {
                    return Ok(ws.to_string());
                }
            }
        }
    }
    for tab in &tabs {
        if tab.get("type").and_then(Value::as_str) == Some("page") {
            if let Some(ws) = tab.get("webSocketDebuggerUrl").and_then(Value::as_str) {
                return Ok(ws.to_string());
            }
        }
    }
    Err("未找到已打开的 Apple Music 网页标签".to_string())
}

pub async fn play_tracks(
    tracks: Vec<AgentTrackInput>,
    start_index: usize,
    storefront: &str,
    hide_window: bool,
) -> Result<SmtcState, String> {
    if tracks.is_empty() {
        return Err("播放队列为空".to_string());
    }
    let idx = start_index.min(tracks.len() - 1);
    let port = ensure_started(storefront, !hide_window).await?;
    let ws = find_am_tab_ws_url(port).await?;

    let ids: Vec<String> = tracks
        .iter()
        .map(|t| t.effective_adam_id())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err("未找到有效的 Apple Music 曲目 ID".to_string());
    }
    let ids_json = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string());
    let js = format!(
        r#"(async () => {{
            const mk = window.MusicKit && window.MusicKit.getInstance ? window.MusicKit.getInstance() : null;
            if (!mk) return "NO_MK";
            await mk.setQueue({{ songs: {ids_json}, startWith: {idx}, startPlaying: true }});
            if (mk.playbackState !== 2) {{
                try {{ await mk.play(); }} catch (e) {{}}
            }}
            return "OK";
        }})()"#
    );
    let res = cdp_eval(&ws, &js, true).await?;
    if res.as_str() == Some("NO_MK") {
        return Err("网页版 MusicKit 仍在加载中，请稍候 2 秒重试".to_string());
    }

    if let Ok(mut g) = RUNTIME.lock() {
        g.queue = tracks;
        g.queue_index = idx;
    }
    tokio::time::sleep(Duration::from_millis(260)).await;
    poll_playback_state().await
}

pub async fn play_container(
    kind: &str,
    id: &str,
    storefront: &str,
    hide_window: bool,
) -> Result<SmtcState, String> {
    let port = ensure_started(storefront, !hide_window).await?;
    let ws = find_am_tab_ws_url(port).await?;
    let id_json = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
    let js = if kind.contains("station") {
        format!(
            r#"(async () => {{
                const mk = window.MusicKit && window.MusicKit.getInstance ? window.MusicKit.getInstance() : null;
                if (!mk) return "NO_MK";
                if (typeof mk.setStationQueue === 'function') {{
                    await mk.setStationQueue({{ station: {id_json} }});
                }} else {{
                    await mk.setQueue({{ station: {id_json}, startPlaying: true }});
                }}
                await mk.play();
                return "OK";
            }})()"#
        )
    } else if kind.contains("playlist") {
        format!(
            r#"(async () => {{
                const mk = window.MusicKit && window.MusicKit.getInstance ? window.MusicKit.getInstance() : null;
                if (!mk) return "NO_MK";
                await mk.setQueue({{ playlist: {id_json}, startPlaying: true }});
                try {{ await mk.play(); }} catch (e) {{}}
                return "OK";
            }})()"#
        )
    } else {
        format!(
            r#"(async () => {{
                const mk = window.MusicKit && window.MusicKit.getInstance ? window.MusicKit.getInstance() : null;
                if (!mk) return "NO_MK";
                await mk.setQueue({{ album: {id_json}, startPlaying: true }});
                try {{ await mk.play(); }} catch (e) {{}}
                return "OK";
            }})()"#
        )
    };
    let res = cdp_eval(&ws, &js, true).await?;
    if res.as_str() == Some("NO_MK") {
        return Err("网页版 MusicKit 仍在加载中，请稍候重试".to_string());
    }
    tokio::time::sleep(Duration::from_millis(320)).await;
    poll_playback_state().await
}

pub async fn control(action: &str, value: Option<f64>) -> Result<SmtcState, String> {
    let port = {
        let g = RUNTIME.lock().map_err(|_| "mutex".to_string())?;
        g.cdp_port
    };
    if port == 0 {
        return Err("网页 Apple Music 引擎尚未启动".to_string());
    }
    let ws = find_am_tab_ws_url(port).await?;
    let act = action.trim().to_ascii_lowercase();
    let js = match act.as_str() {
        "play" => "(async () => { const mk = MusicKit.getInstance(); await mk.play(); return 'ok'; })()".to_string(),
        "pause" => "(async () => { const mk = MusicKit.getInstance(); await mk.pause(); return 'ok'; })()".to_string(),
        "toggle" => "(async () => { const mk = MusicKit.getInstance(); if (mk.playbackState === 2) await mk.pause(); else await mk.play(); return 'ok'; })()".to_string(),
        "next" => "(async () => { const mk = MusicKit.getInstance(); await mk.skipToNextItem(); return 'ok'; })()".to_string(),
        "prev" => "(async () => { const mk = MusicKit.getInstance(); await mk.skipToPreviousItem(); return 'ok'; })()".to_string(),
        "seek" => {
            let s = value.unwrap_or(0.0).max(0.0);
            format!("(async () => {{ const mk = MusicKit.getInstance(); await mk.seekToTime({s:.3}); return 'ok'; }})()")
        }
        "volume" => {
            let v = value.unwrap_or(0.8).clamp(0.0, 1.0);
            format!("(() => {{ const mk = MusicKit.getInstance(); mk.volume = {v:.3}; return 'ok'; }})()")
        }
        _ => "(() => 'ok')()".to_string(),
    };
    let _ = cdp_eval(&ws, &js, act != "volume").await?;
    poll_playback_state().await
}

pub async fn poll_playback_state() -> Result<SmtcState, String> {
    let port = {
        let Ok(g) = RUNTIME.lock() else {
            return Ok(SmtcState::default());
        };
        g.cdp_port
    };
    if port == 0 {
        return Ok(SmtcState::default());
    }
    let ws = match find_am_tab_ws_url(port).await {
        Ok(u) => u,
        Err(e) => {
            if let Ok(mut g) = RUNTIME.lock() {
                g.last_error = Some(e.clone());
            }
            return Err(e);
        }
    };

    let js = r#"(() => {
        const mk = window.MusicKit && window.MusicKit.getInstance ? window.MusicKit.getInstance() : null;
        if (!mk) return JSON.stringify({ ready: false });
        const item = mk.nowPlayingItem || null;
        const attrs = item && item.attributes ? item.attributes : {};
        const art = attrs.artwork && attrs.artwork.url ? attrs.artwork.url : (item && item.artworkURL ? item.artworkURL : "");
        const dur = Number(mk.currentPlaybackDuration || 0) || (Number(attrs.durationInMillis || 0) / 1000);
        return JSON.stringify({
            ready: true,
            authorized: !!mk.isAuthorized,
            storefront: String(mk.storefrontId || ""),
            userToken: String(mk.musicUserToken || ""),
            state: Number(mk.playbackState || 0),
            pos: Number(mk.currentPlaybackTime || 0),
            dur: Number(dur || 0),
            vol: typeof mk.volume === 'number' ? mk.volume : 1.0,
            id: item ? String(item.id || (attrs.playParams && attrs.playParams.catalogId) || "") : "",
            title: String(attrs.name || (item && item.title) || ""),
            artist: String(attrs.artistName || (item && item.artistName) || ""),
            album: String(attrs.albumName || (item && item.albumName) || ""),
            artwork: String(art || "")
        });
    })()"#;

    let val = cdp_eval(&ws, js, false).await?;
    let raw_str = val.as_str().unwrap_or("{}");
    let parsed: Value = serde_json::from_str(raw_str).unwrap_or(Value::Null);
    let ready = parsed.get("ready").and_then(Value::as_bool).unwrap_or(false);
    if !ready {
        return Ok(SmtcState::default());
    }

    let authorized = parsed.get("authorized").and_then(Value::as_bool).unwrap_or(false);
    let sf = parsed.get("storefront").and_then(Value::as_str).unwrap_or_default().to_string();
    let utok = parsed.get("userToken").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let state_num = parsed.get("state").and_then(Value::as_i64).unwrap_or(0);
    let status = match state_num {
        2 => "Playing",
        3 => "Paused",
        1 | 8 => "Paused",
        _ => "Stopped",
    }
    .to_string();
    let pos = parsed.get("pos").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
    let dur = parsed.get("dur").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
    let vol = parsed.get("vol").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0);
    let id = parsed.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let title = parsed.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
    let artist = parsed.get("artist").and_then(Value::as_str).unwrap_or_default().to_string();
    let album = parsed.get("album").and_then(Value::as_str).unwrap_or_default().to_string();
    let art_raw = parsed.get("artwork").and_then(Value::as_str).unwrap_or_default();
    let artwork_url = if art_raw.is_empty() {
        None
    } else {
        Some(format_artwork_template(art_raw, 600))
    };

    let st = SmtcState {
        active: !title.is_empty() || state_num == 2 || state_num == 3,
        app_id: "AMLight.WebAppleMusic".to_string(),
        title,
        artist,
        album,
        status,
        position_sec: pos,
        duration_sec: dur,
        thumbnail_base64: None,
        artwork_url,
        backend_mode: "web-am".to_string(),
        audio_quality: Some(if authorized {
            "AAC 256kbps (网页 MusicKit)".to_string()
        } else {
            "网页试听 (未登录)".to_string()
        }),
        volume: Some(vol),
        agent_pid: None,
        adam_id: if id.is_empty() { None } else { Some(id) },
        stream_specs: None,
        sampled_at: Some(Instant::now()),
        timeline_updated_univ: 0,
        position_is_calibrated: true,
    };

    if let Ok(mut g) = RUNTIME.lock() {
        g.ready = true;
        g.authorized = authorized;
        if !sf.is_empty() {
            g.storefront = sf;
        }
        if !utok.is_empty() && utok.len() > 40 {
            g.captured_user_token = Some(utok);
        }
        g.last_state = st.clone();
        g.last_error = None;
        g.last_poll = Instant::now();
    }
    Ok(st)
}

// ============================================================================
// Zero-Dependency Pure-Rust RFC 6455 Local WebSocket CDP Client
// ============================================================================

async fn cdp_eval(ws_url: &str, expression: &str, await_promise: bool) -> Result<Value, String> {
    let stripped = ws_url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("不支持的 WebSocket 地址: {ws_url}"))?;
    let (host_port, path) = stripped
        .split_once('/')
        .ok_or_else(|| "无效的 CDP WebSocket 路径".to_string())?;

    let mut stream = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(host_port))
        .await
        .map_err(|_| "连接 Edge CDP 超时".to_string())?
        .map_err(|e| format!("连接 Edge CDP 失败: {e}"))?;

    let ws_key = general_purpose::STANDARD.encode(b"amlight-cdp-key!");
    let req = format!(
        "GET /{path} HTTP/1.1\r\nHost: {host_port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {ws_key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| format!("CDP 握手发送失败: {e}"))?;

    // Read HTTP 101 Switching Protocols header
    let mut hdr_buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut byte))
            .await
            .map_err(|_| "CDP 握手读取超时".to_string())?
            .map_err(|e| format!("CDP 握手读取失败: {e}"))?;
        hdr_buf.push(byte[0]);
        if hdr_buf.ends_with(b"\r\n\r\n") || hdr_buf.len() > 8192 {
            break;
        }
    }
    let hdr_str = String::from_utf8_lossy(&hdr_buf);
    if !hdr_str.contains("101") {
        return Err(format!("CDP 握手被拒绝: {}", hdr_str.lines().next().unwrap_or("")));
    }

    let id = CDP_CMD_ID.fetch_add(1, Ordering::Relaxed);
    let msg = json!({
        "id": id,
        "method": "Runtime.evaluate",
        "params": {
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": await_promise
        }
    })
    .to_string();

    ws_write_text(&mut stream, &msg).await?;

    let timeout_sec = if await_promise { 8 } else { 4 };
    let deadline = Instant::now() + Duration::from_secs(timeout_sec);
    loop {
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            return Err("MusicKit JS 执行超时".to_string());
        }
        let frame_txt = tokio::time::timeout(rem, ws_read_text(&mut stream))
            .await
            .map_err(|_| "MusicKit JS 响应超时".to_string())??;
        let Ok(resp) = serde_json::from_str::<Value>(&frame_txt) else {
            continue;
        };
        if resp.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(ex) = resp.pointer("/result/exceptionDetails/text").and_then(Value::as_str) {
            return Err(format!("MusicKit 异常: {ex}"));
        }
        return Ok(resp
            .pointer("/result/result/value")
            .cloned()
            .unwrap_or(Value::Null));
    }
}

async fn ws_write_text(stream: &mut TcpStream, text: &str) -> Result<(), String> {
    let payload = text.as_bytes();
    let mask = [0x37u8, 0xFA, 0x21, 0x9C];
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x81); // FIN + Text
    let n = payload.len();
    if n < 126 {
        frame.push(0x80 | (n as u8));
    } else if n < 65536 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(n as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    for (i, &b) in payload.iter().enumerate() {
        frame.push(b ^ mask[i & 3]);
    }
    stream
        .write_all(&frame)
        .await
        .map_err(|e| format!("CDP 发送失败: {e}"))
}

async fn ws_read_text(stream: &mut TcpStream) -> Result<String, String> {
    let mut acc = Vec::new();
    loop {
        let mut hdr = [0u8; 2];
        stream
            .read_exact(&mut hdr)
            .await
            .map_err(|e| format!("CDP 读取帧头失败: {e}"))?;
        let fin = (hdr[0] & 0x80) != 0;
        let opcode = hdr[0] & 0x0F;
        let masked = (hdr[1] & 0x80) != 0;
        let mut len = (hdr[1] & 0x7F) as u64;
        if len == 126 {
            let mut ext = [0u8; 2];
            stream
                .read_exact(&mut ext)
                .await
                .map_err(|e| format!("CDP 长度读取失败: {e}"))?;
            len = u16::from_be_bytes(ext) as u64;
        } else if len == 127 {
            let mut ext = [0u8; 8];
            stream
                .read_exact(&mut ext)
                .await
                .map_err(|e| format!("CDP 长度读取失败: {e}"))?;
            len = u64::from_be_bytes(ext);
        }
        let mut mask_key = [0u8; 4];
        if masked {
            stream
                .read_exact(&mut mask_key)
                .await
                .map_err(|e| format!("CDP 掩码读取失败: {e}"))?;
        }
        let mut buf = vec![0u8; (len as usize).min(4 * 1024 * 1024)];
        if !buf.is_empty() {
            stream
                .read_exact(&mut buf)
                .await
                .map_err(|e| format!("CDP 载荷读取失败: {e}"))?;
        }
        if masked {
            for (i, b) in buf.iter_mut().enumerate() {
                *b ^= mask_key[i & 3];
            }
        }
        match opcode {
            0x8 => return Err("CDP 连接已关闭".to_string()),
            0x9 => {
                // Ping -> ignore or continue
                continue;
            }
            0xA => continue, // Pong
            _ => {
                acc.extend_from_slice(&buf);
                if fin {
                    return Ok(String::from_utf8_lossy(&acc).to_string());
                }
            }
        }
    }
}
