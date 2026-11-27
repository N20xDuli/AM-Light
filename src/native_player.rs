#![cfg(target_os = "windows")]

use crate::am_agent::AgentTrackInput;
use crate::audio_viz::{StreamAudioSpecs, WM_APP_AUDIO_WAKE};
use crate::backends::ResolvedAudioStream;
use crate::smtc::SmtcState;
use once_cell::sync::Lazy;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::MediaFoundation::{
    MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFShutdown, MFStartup,
    MFAudioFormat_Float, MFSTARTUP_NOSOCKET, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_MEDIASOURCE, MF_VERSION,
};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_I8;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const WAVE_FORMAT_EXTENSIBLE_TAG: u32 = 0xFFFE;
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const GUID_NULL: GUID = GUID::zeroed();
const TICKS_PER_SEC: f64 = 10_000_000.0;

static STREAM_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(8)
        .tcp_nodelay(true)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
        .expect("native stream http client")
});

static WAKE_HWND: AtomicUsize = AtomicUsize::new(0);
static NATIVE_STATE_SEQ: AtomicU64 = AtomicU64::new(0);
static NATIVE_ACTIVE: AtomicBool = AtomicBool::new(false);
static PLAY_SESSION_GEN: AtomicU64 = AtomicU64::new(0);

type AutoAdvanceCb = Arc<dyn Fn(String, usize, AgentTrackInput) + Send + Sync>;

struct PlayerShared {
    active: bool,
    paused: bool,
    switching_track: bool,
    repeat: bool,
    backend_tag: String,
    queue: Vec<AgentTrackInput>,
    queue_index: usize,
    volume: f64,
    position_sec: f64,
    duration_sec: f64,
    audio_quality: Option<String>,
    stream_specs: Option<StreamAudioSpecs>,
    pending_seek_sec: Option<f64>,
    last_updated: Instant,
    auto_advance_cb: Option<AutoAdvanceCb>,
    worker_stop: Option<Arc<AtomicBool>>,
    worker_join: Option<JoinHandle<()>>,
}

impl Default for PlayerShared {
    fn default() -> Self {
        Self {
            active: false,
            paused: false,
            switching_track: false,
            repeat: false,
            backend_tag: "netease".to_string(),
            queue: Vec::new(),
            queue_index: 0,
            volume: 0.85,
            position_sec: 0.0,
            duration_sec: 0.0,
            audio_quality: None,
            stream_specs: None,
            pending_seek_sec: None,
            last_updated: Instant::now(),
            auto_advance_cb: None,
            worker_stop: None,
            worker_join: None,
        }
    }
}

static PLAYER: Lazy<Mutex<PlayerShared>> = Lazy::new(|| Mutex::new(PlayerShared::default()));
static PLAYER_WAKE: Lazy<(Mutex<(u64, usize)>, Condvar)> =
    Lazy::new(|| (Mutex::new((0, 0)), Condvar::new()));
static ACTIVE_DOWNLOAD_ABORT: Lazy<Mutex<Option<(u64, tokio::task::AbortHandle)>>> =
    Lazy::new(|| Mutex::new(None));
static ACTIVE_DOWNLOAD_GEN: AtomicU64 = AtomicU64::new(0);
static LAST_ERROR: Lazy<RwLock<Option<String>>> = Lazy::new(|| RwLock::new(None));

fn notify_player_worker() {
    let (lock, cvar) = &*PLAYER_WAKE;
    if let Ok(mut g) = lock.lock() {
        g.0 = g.0.wrapping_add(1);
        let evt = g.1;
        if evt != 0 {
            unsafe {
                let _ = SetEvent(HANDLE(evt as *mut std::ffi::c_void));
            }
        }
        cvar.notify_all();
    }
}

fn cancel_active_stream_download_inner() -> u64 {
    let next_gen = ACTIVE_DOWNLOAD_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    if let Ok(mut slot) = ACTIVE_DOWNLOAD_ABORT.lock() {
        if let Some((_, handle)) = slot.take() {
            handle.abort();
        }
    }
    next_gen
}

/// Immediately aborts any superseded in-flight streaming download so rapid track switches
/// never waste bandwidth or spike RAM with stale Hi-Res FLAC downloads.
pub fn cancel_active_stream_download() {
    let _ = cancel_active_stream_download_inner();
}

fn retire_worker_join_async(join_handle: Option<JoinHandle<()>>) {
    if let Some(j) = join_handle {
        let _ = thread::Builder::new()
            .name("am-native-reap".to_string())
            .stack_size(64 * 1024)
            .spawn(move || {
                let _ = j.join();
            });
    }
}

#[inline]
pub fn state_seq() -> u64 {
    NATIVE_STATE_SEQ.load(Ordering::Acquire)
}

#[inline]
pub fn is_active() -> bool {
    NATIVE_ACTIVE.load(Ordering::Acquire)
}

pub fn set_wake_hwnd(hwnd: usize) {
    WAKE_HWND.store(hwnd, Ordering::Release);
}

/// Seeds the persistent user volume into the native MF+WASAPI player (NetEase / QQ Music)
/// so startup and backend switches preserve the exact volume chosen by the user.
pub fn set_saved_volume(vol: f64) {
    let v = if vol.is_finite() {
        vol.clamp(0.0, 1.0)
    } else {
        0.75
    };
    if let Ok(mut g) = PLAYER.lock() {
        g.volume = v;
    }
}

pub fn get_saved_volume() -> f64 {
    PLAYER.lock().map(|g| g.volume).unwrap_or(0.75)
}

fn wake_ui() {
    NATIVE_STATE_SEQ.fetch_add(1, Ordering::AcqRel);
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

pub fn set_repeat(repeat: bool) {
    if let Ok(mut g) = PLAYER.lock() {
        g.repeat = repeat;
    }
}

pub fn set_auto_advance_callback(cb: AutoAdvanceCb) {
    if let Ok(mut g) = PLAYER.lock() {
        g.auto_advance_cb = Some(cb);
    }
}

pub fn get_queue_tracks() -> Vec<AgentTrackInput> {
    PLAYER
        .lock()
        .map(|g| g.queue.clone())
        .unwrap_or_default()
}

/// Inserts a track immediately after the currently playing track in the native playback queue
/// ("下一首播放"). Returns `true` if inserted into an active queue; `false` if queue was empty.
pub fn insert_next_in_queue(track: AgentTrackInput) -> bool {
    let Ok(mut g) = PLAYER.lock() else {
        return false;
    };
    if g.queue.is_empty() {
        return false;
    }
    let insert_pos = (g.queue_index + 1).min(g.queue.len());
    if g.queue.get(insert_pos).is_some_and(|t| t.id == track.id && !t.id.is_empty()) {
        return true;
    }
    g.queue.insert(insert_pos, track);
    wake_ui();
    true
}

/// Immediately silences any outgoing track and freezes playhead at `0.0s` while the incoming
/// NetEase / QQ track resolves & downloads, preventing old `40s` playhead bleed into the next song's lyrics!
pub fn begin_track_transition() {
    PLAY_SESSION_GEN.fetch_add(1, Ordering::AcqRel);
    cancel_active_stream_download();
    if let Ok(mut g) = PLAYER.lock() {
        if let Some(ref flag) = g.worker_stop {
            flag.store(true, Ordering::Release);
        }
        g.switching_track = true;
        g.paused = true;
        g.position_sec = 0.0;
        g.pending_seek_sec = None;
        g.last_updated = Instant::now();
    }
    notify_player_worker();
    wake_ui();
}

pub fn get_queue_index() -> usize {
    PLAYER.lock().map(|g| g.queue_index).unwrap_or(0)
}

pub fn last_error() -> Option<String> {
    LAST_ERROR.read().ok().and_then(|g| g.clone())
}

pub fn get_playback_state() -> SmtcState {
    let Ok(g) = PLAYER.lock() else {
        return SmtcState::default();
    };
    if !g.active || g.queue.is_empty() {
        return SmtcState::default();
    }
    let idx = g.queue_index.min(g.queue.len() - 1);
    let tr = &g.queue[idx];
    let dur = if g.duration_sec > 0.5 {
        g.duration_sec
    } else {
        tr.duration_sec
    };
    let pos = if g.switching_track {
        0.0
    } else if dur > 0.0 {
        g.position_sec.clamp(0.0, dur)
    } else {
        g.position_sec.max(0.0)
    };
    SmtcState {
        active: true,
        app_id: format!("AMLight.{}", g.backend_tag),
        title: tr.name.clone(),
        artist: tr.artist_name.clone(),
        album: tr.album_name.clone(),
        status: if g.paused || g.switching_track {
            "Paused".to_string()
        } else {
            "Playing".to_string()
        },
        position_sec: pos,
        duration_sec: dur,
        thumbnail_base64: None,
        artwork_url: tr.artwork_url.clone(),
        backend_mode: g.backend_tag.clone(),
        audio_quality: g.audio_quality.clone(),
        volume: Some(g.volume),
        agent_pid: None,
        adam_id: Some(tr.id.clone()),
        stream_specs: g.stream_specs.clone(),
        sampled_at: Some(g.last_updated),
        timeline_updated_univ: 0,
        position_is_calibrated: !g.switching_track,
    }
}

fn stream_cache_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("AMLight").join("stream_cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub const STREAM_CACHE_EXTENSIONS: &[&str] = &["flac", "m4a", "mp3", "wav", "ogg", "aac"];
pub const STREAM_DOWNLOAD_CHUNK_BYTES: usize = 256 * 1024;
const STREAM_DOWNLOAD_CANCELLED_PREFIX: &str = "cancelled: ";

pub fn is_download_cancelled_err(err: &str) -> bool {
    err.starts_with(STREAM_DOWNLOAD_CANCELLED_PREFIX)
}

fn fnv1a_64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn normalize_cache_track_id(track_id: &str) -> String {
    let t = track_id.trim();
    let stripped = t
        .strip_prefix("ne-song:")
        .or_else(|| t.strip_prefix("ne:"))
        .or_else(|| t.strip_prefix("qq-song:"))
        .or_else(|| t.strip_prefix("qq:"))
        .unwrap_or(t);
    stripped.trim().to_ascii_lowercase()
}

/// Computes a deterministic semantic cache key from `(backend, track_id, quality)` that stays
/// stable across ephemeral signed CDN URL rotations.
pub fn stable_stream_cache_key(backend_tag: &str, track_id: &str, quality_tag: &str) -> u64 {
    let b = backend_tag.trim().to_ascii_lowercase();
    let tid = normalize_cache_track_id(track_id);
    let q = quality_tag.trim().to_ascii_lowercase();
    fnv1a_64(&format!("v2|{b}|{tid}|{q}"))
}

/// Strips ephemeral query strings, fragments, rotating NetEase CDN timestamp/MD5 path prefixes,
/// and rotating QQ CDN edge hostnames so URL-only callers achieve ~100% cache reuse.
pub fn strip_ephemeral_url_query(url: &str) -> String {
    let base = url
        .trim()
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    let Some(scheme_pos) = base.find("://") else {
        return base.to_ascii_lowercase();
    };
    let after_scheme = &base[scheme_pos + 3..];
    let Some(slash_pos) = after_scheme.find('/') else {
        return base.to_ascii_lowercase();
    };
    let host = after_scheme[..slash_pos].to_ascii_lowercase();
    let path = &after_scheme[slash_pos..];

    if host.ends_with(".music.126.net") {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segs.len() >= 3
            && segs[0].len() == 14
            && segs[0].bytes().all(|b| b.is_ascii_digit())
            && segs[1].len() == 32
            && segs[1].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return format!("ne-cdn/{}", segs[2..].join("/"));
        }
        return format!("ne-cdn{path}");
    }
    if host.ends_with(".qqmusic.qq.com") || host.ends_with(".tc.qq.com") {
        return format!("qq-cdn{path}");
    }
    format!("{host}{path}")
}

pub fn stable_stream_cache_key_from_url(url: &str, quality_label: &str, is_trial: bool) -> u64 {
    let norm = strip_ephemeral_url_query(url);
    let q = quality_label.trim().to_ascii_lowercase();
    let trial = if is_trial { "trial" } else { "full" };
    fnv1a_64(&format!("v2-url|{norm}|{q}|{trial}"))
}

/// Probes all supported audio extensions for `trk_{key:016x}.*` so `safe_ext` vs magic `actual_ext`
/// mismatches never cause a false cache miss.
pub fn find_cached_stream_file(dir: &Path, key: u64) -> Option<PathBuf> {
    for &ext in STREAM_CACHE_EXTENSIONS {
        let candidate = dir.join(format!("trk_{key:016x}.{ext}"));
        if let Ok(meta) = fs::metadata(&candidate) {
            if meta.is_file() && meta.len() > 8192 {
                return Some(candidate);
            }
        }
    }
    None
}

fn prune_stream_cache(dir: &Path) {
    const MAX_STREAM_CACHE_FILES: usize = 14;
    const TARGET_STREAM_CACHE_FILES: usize = 10;
    const MAX_STREAM_CACHE_BYTES: u64 = 256 * 1024 * 1024;
    const TARGET_STREAM_CACHE_BYTES: u64 = 192 * 1024 * 1024;

    let now = std::time::SystemTime::now();
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut total_bytes: u64 = 0;
    let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let m = e.metadata().ok()?;
            if !m.is_file() {
                return None;
            }
            let t = m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("part")) {
                if now.duration_since(t).unwrap_or_default() >= Duration::from_secs(300) {
                    let _ = fs::remove_file(&p);
                }
                return None;
            }
            let len = m.len();
            total_bytes = total_bytes.saturating_add(len);
            Some((p, len, t))
        })
        .collect();
    if entries.len() <= MAX_STREAM_CACHE_FILES && total_bytes <= MAX_STREAM_CACHE_BYTES {
        return;
    }
    entries.sort_by_key(|(_, _, t)| *t);
    let mut remaining_files = entries.len();
    for (p, len, _) in entries {
        if remaining_files <= TARGET_STREAM_CACHE_FILES && total_bytes <= TARGET_STREAM_CACHE_BYTES
        {
            break;
        }
        if fs::remove_file(&p).is_ok() {
            remaining_files = remaining_files.saturating_sub(1);
            total_bytes = total_bytes.saturating_sub(len);
        }
    }
}

async fn download_stream_to_cache_with_key(
    stream: &ResolvedAudioStream,
    key: u64,
) -> Result<PathBuf, String> {
    let dir = stream_cache_dir();
    let probe_dir = dir.clone();
    if let Ok(Some(hit)) =
        tokio::task::spawn_blocking(move || find_cached_stream_file(&probe_dir, key)).await
    {
        return Ok(hit);
    }

    let ext = stream
        .file_ext
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let safe_ext: &'static str = match ext.as_str() {
        "flac" => "flac",
        "mp3" => "mp3",
        "m4a" => "m4a",
        "aac" => "aac",
        "wav" => "wav",
        "ogg" => "ogg",
        _ => "mp3",
    };

    let my_gen = cancel_active_stream_download_inner();
    let url = stream.url.clone();
    let referer = stream.referer.clone();
    let cookie = stream.cookie.clone();
    let dir_task = dir.clone();

    let join_handle = tokio::spawn(async move {
        struct PartFileCleanupGuard {
            path: PathBuf,
            committed: bool,
        }
        impl Drop for PartFileCleanupGuard {
            fn drop(&mut self) {
                if !self.committed {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }

        let tmp = dir_task.join(format!("trk_{key:016x}_{my_gen}.part"));
        // Declare cleanup guard BEFORE `file_opt` so Rust drops `file_opt` (closing the Windows
        // file handle) first on cancellation/error, allowing `remove_file` to succeed cleanly!
        let mut part_guard = PartFileCleanupGuard {
            path: tmp.clone(),
            committed: false,
        };

        let mut req = STREAM_HTTP.get(&url).header("Referer", &referer);
        if let Some(ref ck) = cookie {
            if !ck.trim().is_empty() {
                req = req.header("Cookie", ck.trim());
            }
        }
        let mut resp = req
            .send()
            .await
            .map_err(|e| format!("拉取音频流失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("音频流返回 HTTP {}", resp.status().as_u16()));
        }
        if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) != my_gen {
            return Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}superseded"));
        }

        let tmp_create = tmp.clone();
        let file = tokio::task::spawn_blocking(move || File::create(&tmp_create))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("创建音频缓存文件失败: {e}"))?;

        let mut file_opt = Some(file);
        let mut chunk_buf: Vec<u8> = Vec::with_capacity(STREAM_DOWNLOAD_CHUNK_BYTES);
        let mut magic_head = [0u8; 64];
        let mut magic_len = 0usize;
        let mut total_written: u64 = 0;

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("读取音频流分块失败: {e}"))?
        {
            if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) != my_gen {
                drop(file_opt.take());
                return Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}superseded"));
            }
            if magic_len < magic_head.len() {
                let copy_n = (magic_head.len() - magic_len).min(chunk.len());
                magic_head[magic_len..magic_len + copy_n].copy_from_slice(&chunk[..copy_n]);
                magic_len += copy_n;
            }
            chunk_buf.extend_from_slice(&chunk);
            total_written = total_written.saturating_add(chunk.len() as u64);

            if chunk_buf.len() >= STREAM_DOWNLOAD_CHUNK_BYTES {
                let mut f = file_opt
                    .take()
                    .ok_or_else(|| "音频缓存文件句柄丢失".to_string())?;
                let mut write_buf = std::mem::take(&mut chunk_buf);
                let chunk_tmp = tmp.clone();
                let (f_back, buf_back) =
                    tokio::task::spawn_blocking(move || -> Result<(File, Vec<u8>), String> {
                        if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) != my_gen {
                            drop(f);
                            let _ = fs::remove_file(&chunk_tmp);
                            return Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}superseded"));
                        }
                        f.write_all(&write_buf)
                            .map_err(|e| format!("写入音频缓存分块失败: {e}"))?;
                        if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) != my_gen {
                            drop(f);
                            let _ = fs::remove_file(&chunk_tmp);
                            return Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}superseded"));
                        }
                        write_buf.clear();
                        Ok((f, write_buf))
                    })
                    .await
                    .map_err(|e| e.to_string())??;
                file_opt = Some(f_back);
                chunk_buf = buf_back;
            }
        }

        if let Some(mut f) = file_opt.take() {
            let tail = std::mem::take(&mut chunk_buf);
            let tail_tmp = tmp.clone();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) != my_gen {
                    drop(f);
                    let _ = fs::remove_file(&tail_tmp);
                    return Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}superseded"));
                }
                if !tail.is_empty() {
                    f.write_all(&tail)
                        .map_err(|e| format!("写入音频缓存尾块失败: {e}"))?;
                }
                f.flush().map_err(|e| format!("刷新音频缓存失败: {e}"))?;
                if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) != my_gen {
                    drop(f);
                    let _ = fs::remove_file(&tail_tmp);
                    return Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}superseded"));
                }
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())??;
        }

        if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) != my_gen {
            return Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}superseded"));
        }
        if total_written < 4096 {
            return Err("音频流内容过小或已失效，请检查版权/登录状态".to_string());
        }

        let actual_ext = detect_audio_magic_ext(&magic_head[..magic_len]).unwrap_or(safe_ext);
        let final_dst = dir_task.join(format!("trk_{key:016x}.{actual_ext}"));
        let commit_tmp = tmp.clone();
        let commit_dst = final_dst.clone();
        let commit_dir = dir_task.clone();
        tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
            for &old_ext in STREAM_CACHE_EXTENSIONS {
                let old_p = commit_dir.join(format!("trk_{key:016x}.{old_ext}"));
                if old_p != commit_dst {
                    let _ = fs::remove_file(&old_p);
                }
            }
            let _ = fs::remove_file(&commit_dst);
            fs::rename(&commit_tmp, &commit_dst)
                .map_err(|e| format!("提交音频缓存失败: {e}"))?;
            prune_stream_cache(&commit_dir);
            Ok(commit_dst)
        })
        .await
        .map_err(|e| e.to_string())??;

        part_guard.committed = true;
        Ok(final_dst)
    });

    let abort_handle = join_handle.abort_handle();
    if let Ok(mut slot) = ACTIVE_DOWNLOAD_ABORT.lock() {
        if ACTIVE_DOWNLOAD_GEN.load(Ordering::Acquire) == my_gen {
            *slot = Some((my_gen, abort_handle));
        } else {
            abort_handle.abort();
        }
    }

    let res = match join_handle.await {
        Ok(inner) => inner,
        Err(e) if e.is_cancelled() => Err(format!("{STREAM_DOWNLOAD_CANCELLED_PREFIX}aborted")),
        Err(e) => Err(format!("音频流下载任务异常: {e}")),
    };

    if let Ok(mut slot) = ACTIVE_DOWNLOAD_ABORT.lock() {
        if slot.as_ref().is_some_and(|(g, _)| *g == my_gen) {
            *slot = None;
        }
    }
    res
}

/// Downloads a resolved audio stream into a bounded local cache using a deterministic semantic
/// `(backend, track_id, quality)` cache key and 256 KB streaming chunks with `AbortHandle` cancellation.
pub async fn download_stream_to_cache_keyed(
    stream: &ResolvedAudioStream,
    backend_tag: &str,
    track_id: &str,
    quality_tag: &str,
) -> Result<PathBuf, String> {
    let eff_q = if stream.is_trial {
        format!("{}|trial", quality_tag.trim())
    } else {
        quality_tag.trim().to_string()
    };
    let key = stable_stream_cache_key(backend_tag, track_id, &eff_q);
    download_stream_to_cache_with_key(stream, key).await
}

/// Downloads a resolved audio stream with platform headers into a bounded local cache file
/// (stripping ephemeral query/CDN tokens & probing all audio extensions for ~100% cache reuse).
pub async fn download_stream_to_cache(stream: &ResolvedAudioStream) -> Result<PathBuf, String> {
    let key = stable_stream_cache_key_from_url(&stream.url, &stream.quality_label, stream.is_trial);
    download_stream_to_cache_with_key(stream, key).await
}

pub fn detect_audio_magic_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 4 && &bytes[0..4] == b"fLaC" {
        return Some("flac");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some("m4a");
    }
    if bytes.len() >= 4 && &bytes[0..4] == b"RIFF" {
        return Some("wav");
    }
    if bytes.len() >= 4 && &bytes[0..4] == b"OggS" {
        return Some("ogg");
    }
    if bytes.len() >= 3 && (&bytes[0..3] == b"ID3" || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0)) {
        return Some("mp3");
    }
    None
}

/// Parses FLAC STREAMINFO block (`fLaC` + metadata block 0) to extract authentic sample rate, channels & bit depth.
pub fn parse_flac_streaminfo(bytes: &[u8]) -> Option<(u32, u16, u16)> {
    if bytes.len() < 42 || &bytes[0..4] != b"fLaC" {
        return None;
    }
    let block_type = bytes[4] & 0x7F;
    if block_type != 0 {
        return None;
    }
    // STREAMINFO starts at byte 8; bytes 10..18 inside STREAMINFO hold sr(20b), ch-1(3b), bps-1(5b)
    let b = &bytes[18..26];
    let sr = ((b[0] as u32) << 12) | ((b[1] as u32) << 4) | ((b[2] as u32) >> 4);
    let ch = (((b[2] >> 1) & 0x07) as u16) + 1;
    let bps = ((((b[2] & 0x01) as u16) << 4) | ((b[3] >> 4) as u16)) + 1;
    if sr >= 8000 && ch >= 1 && bps >= 8 {
        Some((sr, ch, bps))
    } else {
        None
    }
}

fn read_audio_file_header(path: &Path) -> (u64, [u8; 64], usize) {
    let mut head_buf = [0u8; 64];
    match File::open(path) {
        Ok(mut f) => {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let mut n = 0usize;
            while n < head_buf.len() {
                match f.read(&mut head_buf[n..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => n += k,
                }
            }
            (len, head_buf, n)
        }
        Err(_) => (0, head_buf, 0),
    }
}

pub fn inspect_audio_file_specs_from_header(
    file_len: u64,
    head: &[u8],
    stream: &ResolvedAudioStream,
    decoded_sr: u32,
    decoded_ch: u16,
    duration_sec: f64,
) -> StreamAudioSpecs {
    let ext = detect_audio_magic_ext(head)
        .unwrap_or(stream.file_ext.as_str())
        .to_ascii_lowercase();

    let (codec, codec_label, sample_rate_hz, bit_depth, is_lossless) = if ext == "flac" {
        if let Some((sr, _ch, bps)) = parse_flac_streaminfo(head) {
            ("FLAC", "FLAC 无损", sr, Some(bps), true)
        } else {
            ("FLAC", "FLAC 无损", decoded_sr.max(44100), Some(16), true)
        }
    } else if ext == "m4a" || ext == "aac" {
        ("AAC", "AAC / M4A", decoded_sr.max(44100), None, false)
    } else if ext == "wav" {
        ("PCM", "WAV 无损", decoded_sr.max(44100), Some(16), true)
    } else {
        ("MP3", "MP3", decoded_sr.max(44100), None, false)
    };

    let measured_kbps = if duration_sec > 1.0 && file_len > 0 {
        ((file_len as f64 * 8.0) / (duration_sec * 1000.0)).round() as u32
    } else {
        stream.nominal_bitrate_kbps
    };
    let bitrate_kbps = if measured_kbps >= 32 {
        measured_kbps
    } else {
        stream.nominal_bitrate_kbps.max(128)
    };
    let is_hires = is_lossless && (sample_rate_hz > 48_000 || bit_depth.unwrap_or(16) >= 24);
    let tier_label = if stream.is_trial {
        format!("试听片段 · {}", stream.quality_label)
    } else if is_hires {
        "高解析度无损".to_string()
    } else if is_lossless {
        "无损".to_string()
    } else if bitrate_kbps >= 280 {
        "极高音质".to_string()
    } else {
        "标准音质".to_string()
    };

    let uncompressed_pcm_kbps = bit_depth.map(|bps| {
        (sample_rate_hz as u64 * bps as u64 * decoded_ch.max(2) as u64 / 1000) as u32
    });
    let compression_ratio_pct = uncompressed_pcm_kbps.map(|pcm| {
        if pcm > 0 {
            (bitrate_kbps as f32 / pcm as f32) * 100.0
        } else {
            100.0
        }
    });

    StreamAudioSpecs {
        codec: codec.to_string(),
        codec_label: codec_label.to_string(),
        tier_label,
        sample_rate_hz,
        bit_depth,
        channels: if decoded_ch <= 1 {
            "1.0 单声道".to_string()
        } else {
            "2.0 立体声".to_string()
        },
        bitrate_kbps,
        peak_bitrate_kbps: bitrate_kbps,
        uncompressed_pcm_kbps,
        compression_ratio_pct,
        atmos_fallback_reason: None,
        hires_catalog_trait: is_hires,
        available_variants: Vec::new(),
    }
}

pub fn inspect_audio_file_specs(
    path: &Path,
    stream: &ResolvedAudioStream,
    decoded_sr: u32,
    decoded_ch: u16,
    duration_sec: f64,
) -> StreamAudioSpecs {
    let (file_len, head_buf, head_len) = read_audio_file_header(path);
    inspect_audio_file_specs_from_header(
        file_len,
        &head_buf[..head_len],
        stream,
        decoded_sr,
        decoded_ch,
        duration_sec,
    )
}

pub fn stop_and_clear() {
    PLAY_SESSION_GEN.fetch_add(1, Ordering::AcqRel);
    cancel_active_stream_download();
    let join_handle = {
        let Ok(mut g) = PLAYER.lock() else { return };
        if let Some(flag) = g.worker_stop.take() {
            flag.store(true, Ordering::Release);
        }
        g.active = false;
        g.paused = false;
        g.switching_track = false;
        g.position_sec = 0.0;
        g.duration_sec = 0.0;
        g.pending_seek_sec = None;
        g.audio_quality = None;
        g.stream_specs = None;
        NATIVE_ACTIVE.store(false, Ordering::Release);
        g.worker_join.take()
    };
    notify_player_worker();
    retire_worker_join_async(join_handle);
    wake_ui();
}

pub fn start_playback_file(
    backend_tag: &str,
    tracks: Option<(Vec<AgentTrackInput>, usize)>,
    jump_index: Option<usize>,
    stream: ResolvedAudioStream,
    local_file_path: PathBuf,
) -> Result<SmtcState, String> {
    let prev_join = {
        let mut g = PLAYER.lock().map_err(|_| "player mutex poisoned".to_string())?;
        if let Some(flag) = g.worker_stop.take() {
            flag.store(true, Ordering::Release);
        }
        g.worker_join.take()
    };
    let gen = PLAY_SESSION_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    notify_player_worker();
    retire_worker_join_async(prev_join);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);
    let path_clone = local_file_path.clone();
    let stream_clone = stream.clone();
    let tag_owned = backend_tag.to_string();

    // Inspect 64-byte audio header ONCE outside `PLAYER` lock and pass cached bytes to render worker!
    let preview_dur_sec = if let Some((ref q, idx)) = tracks {
        q.get(idx.min(q.len().saturating_sub(1)))
            .map(|t| t.duration_sec)
            .unwrap_or(0.0)
    } else if let Ok(g) = PLAYER.lock() {
        let idx = jump_index
            .map(|i| i.min(g.queue.len().saturating_sub(1)))
            .unwrap_or(g.queue_index);
        g.queue.get(idx).map(|t| t.duration_sec).unwrap_or(0.0)
    } else {
        0.0
    };
    let (file_len, head_buf, head_len) = read_audio_file_header(&local_file_path);
    let initial_specs = inspect_audio_file_specs_from_header(
        file_len,
        &head_buf[..head_len],
        &stream,
        44100,
        2,
        preview_dur_sec,
    );

    {
        let mut g = PLAYER.lock().map_err(|_| "player mutex poisoned".to_string())?;
        if let Some((q, idx)) = tracks {
            if !q.is_empty() {
                g.queue_index = idx.min(q.len() - 1);
                g.queue = q;
            }
        } else if let Some(idx) = jump_index {
            if !g.queue.is_empty() {
                g.queue_index = idx.min(g.queue.len() - 1);
            }
        }
        g.backend_tag = tag_owned.clone();
        g.active = true;
        g.paused = false;
        g.switching_track = false;
        g.position_sec = 0.0;
        g.duration_sec = g
            .queue
            .get(g.queue_index)
            .map(|t| t.duration_sec)
            .unwrap_or(0.0);
        g.pending_seek_sec = None;
        g.audio_quality = Some(stream.quality_label.clone());
        g.stream_specs = Some(initial_specs);
        g.last_updated = Instant::now();
        g.worker_stop = Some(stop_flag);
        NATIVE_ACTIVE.store(true, Ordering::Release);
    }

    if let Ok(mut e) = LAST_ERROR.write() {
        *e = None;
    }

    let join = thread::Builder::new()
        .name(format!("am-native-audio-{gen}"))
        .spawn(move || {
            if let Err(err) = render_audio_file_loop(
                &path_clone,
                &stream_clone,
                (file_len, head_buf, head_len),
                gen,
                &stop_clone,
            ) {
                if let Ok(mut e) = LAST_ERROR.write() {
                    *e = Some(err);
                }
            }
        })
        .map_err(|e| format!("启动音频播放线程失败: {e}"))?;

    if let Ok(mut g) = PLAYER.lock() {
        g.worker_join = Some(join);
    }
    wake_ui();
    Ok(get_playback_state())
}

pub fn control(action: &str, value: Option<f64>) -> Result<SmtcState, String> {
    let act = action.trim().to_ascii_lowercase();
    match act.as_str() {
        "play" => {
            if let Ok(mut g) = PLAYER.lock() {
                g.paused = false;
                g.last_updated = Instant::now();
            }
            notify_player_worker();
            wake_ui();
            Ok(get_playback_state())
        }
        "pause" => {
            if let Ok(mut g) = PLAYER.lock() {
                g.paused = true;
                g.last_updated = Instant::now();
            }
            notify_player_worker();
            wake_ui();
            Ok(get_playback_state())
        }
        "toggle" => {
            if let Ok(mut g) = PLAYER.lock() {
                g.paused = !g.paused;
                g.last_updated = Instant::now();
            }
            notify_player_worker();
            wake_ui();
            Ok(get_playback_state())
        }
        "seek" => {
            if let Some(sec) = value {
                let target = sec.max(0.0);
                if let Ok(mut g) = PLAYER.lock() {
                    let max_d = if g.duration_sec > 0.5 { g.duration_sec } else { target };
                    let clamped = target.min(max_d);
                    g.position_sec = clamped;
                    g.pending_seek_sec = Some(clamped);
                    g.last_updated = Instant::now();
                }
                notify_player_worker();
                wake_ui();
            }
            Ok(get_playback_state())
        }
        "volume" => {
            if let Some(v) = value {
                if let Ok(mut g) = PLAYER.lock() {
                    g.volume = v.clamp(0.0, 1.0);
                }
            }
            Ok(get_playback_state())
        }
        "stop" => {
            stop_and_clear();
            Ok(SmtcState::default())
        }
        "next" | "prev" => {
            let next_req: Option<(AutoAdvanceCb, String, usize, AgentTrackInput)> = {
                let mut g = PLAYER.lock().map_err(|_| "mutex".to_string())?;
                if g.queue.is_empty() {
                    return Ok(get_playback_state());
                }
                let len = g.queue.len();
                let next_idx = if act == "next" {
                    (g.queue_index + 1) % len
                } else if g.position_sec > 4.0 && !g.switching_track {
                    // Restart current track if >4s into track on Prev
                    g.position_sec = 0.0;
                    g.pending_seek_sec = Some(0.0);
                    g.last_updated = Instant::now();
                    drop(g);
                    notify_player_worker();
                    return Ok(get_playback_state());
                } else if g.queue_index == 0 {
                    len - 1
                } else {
                    g.queue_index - 1
                };
                // Immediately invalidate the outgoing track's WASAPI render thread so it stops
                // outputting audio AND cannot overwrite `position_sec` with the old song's playhead!
                PLAY_SESSION_GEN.fetch_add(1, Ordering::AcqRel);
                if let Some(ref flag) = g.worker_stop {
                    flag.store(true, Ordering::Release);
                }
                let tr = g.queue[next_idx].clone();
                g.queue_index = next_idx;
                g.position_sec = 0.0;
                g.duration_sec = tr.duration_sec;
                g.paused = true;
                g.switching_track = true;
                g.pending_seek_sec = None;
                g.last_updated = Instant::now();
                let tag = g.backend_tag.clone();
                g.auto_advance_cb.clone().map(|cb| (cb, tag, next_idx, tr))
            };
            cancel_active_stream_download();
            notify_player_worker();
            wake_ui();
            if let Some((cb, tag, idx, tr)) = next_req {
                cb(tag, idx, tr);
            }
            Ok(get_playback_state())
        }
        _ => Ok(get_playback_state()),
    }
}

fn make_i64_propvariant(val: i64) -> PROPVARIANT {
    let mut pv = PROPVARIANT::default();
    unsafe {
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = val;
    }
    pv
}

fn render_audio_file_loop(
    file_path: &Path,
    stream: &ResolvedAudioStream,
    cached_header: (u64, [u8; 64], usize),
    gen: u64,
    stop: &AtomicBool,
) -> Result<(), String> {
    unsafe {
        let com_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        struct ComGuard(bool);
        impl Drop for ComGuard {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { CoUninitialize() };
                }
            }
        }
        let _com = ComGuard(com_ok);

        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
            .map_err(|e| format!("MediaFoundation 初始化失败: {e}"))?;
        struct MfGuard;
        impl Drop for MfGuard {
            fn drop(&mut self) {
                unsafe {
                    let _ = MFShutdown();
                }
            }
        }
        let _mf = MfGuard;

        let hpath = HSTRING::from(file_path.as_os_str());
        let reader = MFCreateSourceReaderFromURL(&hpath, None)
            .map_err(|e| format!("无法解码音频文件: {e}"))?;

        let req_type = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
        let _ = req_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio);
        let _ = req_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float);
        reader
            .SetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, None, &req_type)
            .map_err(|e| format!("设置浮点解码格式失败: {e}"))?;

        let out_type = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
            .map_err(|e| format!("读取解码格式失败: {e}"))?;
        let src_ch = out_type.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).max(1) as usize;
        let src_sr = out_type
            .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
            .unwrap_or(44100)
            .max(8000);

        // Read accurate media duration if available
        let mut file_dur_sec = 0.0f64;
        if let Ok(pv) = reader.GetPresentationAttribute(
            MF_SOURCE_READER_MEDIASOURCE.0 as u32,
            &MF_PD_DURATION,
        ) {
            let h = pv.Anonymous.Anonymous.Anonymous.uhVal;
            if h > 0 {
                file_dur_sec = (h as f64) / TICKS_PER_SEC;
            }
        }

        // Update real decoded specs & duration from cached 64-byte header (zero second File::open!)
        {
            let fallback_dur = if file_dur_sec > 0.5 {
                file_dur_sec
            } else {
                PLAYER.lock().map(|g| g.duration_sec).unwrap_or(0.0)
            };
            let (file_len, ref head_buf, head_len) = cached_header;
            let specs = inspect_audio_file_specs_from_header(
                file_len,
                &head_buf[..head_len],
                stream,
                src_sr,
                src_ch as u16,
                fallback_dur,
            );
            if let Ok(mut g) = PLAYER.lock() {
                if PLAY_SESSION_GEN.load(Ordering::Acquire) == gen {
                    if file_dur_sec > 0.5 {
                        g.duration_sec = file_dur_sec;
                    }
                    g.audio_quality = Some(format!(
                        "{} · {}kbps",
                        specs.tier_label, specs.bitrate_kbps
                    ));
                    g.stream_specs = Some(specs);
                }
            }
            wake_ui();
        }

        // Open WASAPI shared audio render endpoint (prefer EVENTCALLBACK with safe timer fallback)
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("MMDeviceEnumerator: {e}"))?;
        let dev = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("找不到默认音频输出设备: {e}"))?;
        let mut client: IAudioClient = dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("激活 WASAPI 输出设备失败: {e}"))?;
        let wfx_ptr = client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat 失败: {e}"))?;
        struct WfxGuard(*mut windows::Win32::Media::Audio::WAVEFORMATEX);
        impl Drop for WfxGuard {
            fn drop(&mut self) {
                unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
            }
        }
        let _wfx_g = WfxGuard(wfx_ptr);
        let wfx = std::ptr::read_unaligned(wfx_ptr);
        let out_sr = wfx.nSamplesPerSec.max(8000);
        let out_ch = wfx.nChannels.max(1) as usize;
        let is_float = if wfx.wFormatTag as u32 == WAVE_FORMAT_IEEE_FLOAT {
            true
        } else if wfx.wFormatTag as u32 == WAVE_FORMAT_EXTENSIBLE_TAG {
            let ext = std::ptr::read_unaligned(wfx_ptr as *const WAVEFORMATEXTENSIBLE);
            let sf = ext.SubFormat;
            sf == SUBTYPE_IEEE_FLOAT
        } else {
            false
        };
        if !is_float || wfx.wBitsPerSample != 32 {
            return Err("默认音频设备混合格式非 32-bit Float，暂不支持输出".to_string());
        }

        struct EventHandleGuard {
            handle: HANDLE,
        }
        impl Drop for EventHandleGuard {
            fn drop(&mut self) {
                if let Ok(mut g) = PLAYER_WAKE.0.lock() {
                    if g.1 == self.handle.0 as usize {
                        g.1 = 0;
                    }
                }
                unsafe {
                    let _ = CloseHandle(self.handle);
                }
            }
        }

        let evt_guard = CreateEventW(None, false, false, PCWSTR::null())
            .ok()
            .map(|handle| EventHandleGuard { handle });
        let mut use_event_callback = false;
        if let Some(ref eg) = evt_guard {
            if client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    600_000,
                    0,
                    wfx_ptr,
                    None,
                )
                .is_ok()
                && client.SetEventHandle(eg.handle).is_ok()
            {
                if let Ok(mut g) = PLAYER_WAKE.0.lock() {
                    g.1 = eg.handle.0 as usize;
                }
                use_event_callback = true;
            }
        }
        if !use_event_callback {
            client = dev
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("激活 WASAPI 输出设备失败: {e}"))?;
            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 600_000, 0, wfx_ptr, None)
                .map_err(|e| format!("WASAPI Initialize 失败: {e}"))?;
        }

        let buf_frames = client
            .GetBufferSize()
            .map_err(|e| format!("GetBufferSize: {e}"))?;
        let render: IAudioRenderClient = client
            .GetService()
            .map_err(|e| format!("IAudioRenderClient: {e}"))?;
        let _ = client.Start();
        let mut client_running = true;
        let mut need_reset_on_resume = false;

        // Decoded stereo f32 ring buffer at `src_sr`
        let mut pcm_stereo: VecDeque<[f32; 2]> = VecDeque::with_capacity(src_sr as usize);
        let mut eof = false;
        let mut resamp_phase = 0.0f64;
        let mut prev_lr = [0.0f32; 2];
        let mut cur_lr = [0.0f32; 2];
        let step_src_per_out = (src_sr as f64) / (out_sr as f64);
        let mut played_src_frames: f64 = 0.0;
        let mut seek_base_sec: f64 = 0.0;
        let mut last_state_sync = Instant::now();
        let mut natural_finished = false;

        while !stop.load(Ordering::Acquire) && PLAY_SESSION_GEN.load(Ordering::Acquire) == gen {
            // 1. Check pause, volume, and pending seek
            let (paused, vol, seek_req) = {
                let Ok(mut g) = PLAYER.lock() else { break };
                let sk = g.pending_seek_sec.take();
                (g.paused, g.volume as f32, sk)
            };

            if let Some(target_sec) = seek_req {
                let ticks = (target_sec.max(0.0) * TICKS_PER_SEC).round() as i64;
                let pv = make_i64_propvariant(ticks);
                if reader.SetCurrentPosition(&GUID_NULL, &pv).is_ok() {
                    pcm_stereo.clear();
                    eof = false;
                    resamp_phase = 0.0;
                    prev_lr = [0.0; 2];
                    cur_lr = [0.0; 2];
                    played_src_frames = 0.0;
                    seek_base_sec = target_sec.max(0.0);
                    if client_running && !paused {
                        let _ = client.Stop();
                        let _ = client.Reset();
                        let _ = client.Start();
                        need_reset_on_resume = false;
                    } else {
                        need_reset_on_resume = true;
                    }
                }
            }

            // Zero-wakeup pause: stop IAudioClient and park on Condvar (0 wakeups/s instead of 83 Hz silence pumping!)
            if paused {
                if client_running {
                    let _ = client.Stop();
                    client_running = false;
                }
                let (lock, cvar) = &*PLAYER_WAKE;
                if let Ok(mut wg) = lock.lock() {
                    loop {
                        if stop.load(Ordering::Acquire)
                            || PLAY_SESSION_GEN.load(Ordering::Acquire) != gen
                        {
                            break;
                        }
                        let still_paused = PLAYER
                            .lock()
                            .map(|pg| pg.paused && pg.pending_seek_sec.is_none())
                            .unwrap_or(false);
                        if !still_paused {
                            break;
                        }
                        wg = match cvar.wait(wg) {
                            Ok(g) => g,
                            Err(e) => e.into_inner(),
                        };
                    }
                }
                continue;
            }

            if !client_running {
                if need_reset_on_resume {
                    let _ = client.Reset();
                    need_reset_on_resume = false;
                }
                let _ = client.Start();
                client_running = true;
            }

            // 2. Refill decoded PCM ring buffer if below ~0.25s
            while !eof && pcm_stereo.len() < (src_sr as usize / 4) {
                let mut flags = 0u32;
                let mut sample_opt = None;
                if reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut sample_opt),
                    )
                    .is_err()
                {
                    eof = true;
                    break;
                }
                if (flags & 2) != 0 {
                    eof = true;
                    break;
                }
                let Some(sample) = sample_opt else {
                    continue;
                };
                let Ok(buf) = sample.ConvertToContiguousBuffer() else {
                    continue;
                };
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut cur_len = 0u32;
                if buf.Lock(&mut ptr, None, Some(&mut cur_len)).is_ok() && !ptr.is_null() {
                    let floats = std::slice::from_raw_parts(ptr as *const f32, cur_len as usize / 4);
                    let frames = floats.len() / src_ch;
                    for f in 0..frames {
                        let base = f * src_ch;
                        let l = floats[base];
                        let r = if src_ch >= 2 { floats[base + 1] } else { l };
                        pcm_stereo.push_back([
                            if l.is_finite() { l } else { 0.0 },
                            if r.is_finite() { r } else { 0.0 },
                        ]);
                    }
                    let _ = buf.Unlock();
                }
            }

            // 3. Render into WASAPI buffer
            let Ok(padding) = client.GetCurrentPadding() else {
                break;
            };
            let avail = buf_frames.saturating_sub(padding);
            if avail < 64 {
                if use_event_callback {
                    if let Some(ref eg) = evt_guard {
                        let _ = WaitForSingleObject(eg.handle, 25);
                    }
                } else {
                    thread::sleep(Duration::from_millis(8));
                }
                continue;
            }
            let Ok(ptr) = render.GetBuffer(avail) else {
                break;
            };
            let out_slice =
                std::slice::from_raw_parts_mut(ptr as *mut f32, avail as usize * out_ch);

            // Perceptual loudness curve (`vol^1.35`) feels natural like Apple Music slider
            let gain = vol.clamp(0.0, 1.0).powf(1.35);
            for f in 0..avail as usize {
                resamp_phase += step_src_per_out;
                while resamp_phase >= 1.0 {
                    resamp_phase -= 1.0;
                    prev_lr = cur_lr;
                    if let Some(next_lr) = pcm_stereo.pop_front() {
                        cur_lr = next_lr;
                        played_src_frames += 1.0;
                    } else {
                        cur_lr = [0.0, 0.0];
                    }
                }
                let frac = resamp_phase as f32;
                let l = (prev_lr[0] + (cur_lr[0] - prev_lr[0]) * frac) * gain;
                let r = (prev_lr[1] + (cur_lr[1] - prev_lr[1]) * frac) * gain;
                let base = f * out_ch;
                if out_ch == 1 {
                    out_slice[base] = (l + r) * 0.5;
                } else {
                    out_slice[base] = l;
                    out_slice[base + 1] = r;
                    for c in 2..out_ch {
                        out_slice[base + c] = 0.0;
                    }
                }
            }
            if render.ReleaseBuffer(avail, 0).is_err() {
                break;
            }

            // 4. Sync exact playhead back to shared snapshot every ~45ms
            if last_state_sync.elapsed() >= Duration::from_millis(45) {
                last_state_sync = Instant::now();
                let pos_sec = seek_base_sec + (played_src_frames / src_sr as f64);
                if let Ok(mut g) = PLAYER.lock() {
                    if PLAY_SESSION_GEN.load(Ordering::Acquire) == gen {
                        g.position_sec = pos_sec;
                        g.last_updated = Instant::now();
                    }
                }
            }

            // 5. Check clean EOF once ring drained
            if eof && pcm_stereo.is_empty() {
                thread::sleep(Duration::from_millis(45));
                natural_finished = true;
                break;
            }
        }

        let _ = client.Stop();

        // Trigger seamless auto-advance or repeat on natural track completion
        if natural_finished
            && !stop.load(Ordering::Acquire)
            && PLAY_SESSION_GEN.load(Ordering::Acquire) == gen
        {
            let next_req: Option<(AutoAdvanceCb, String, usize, AgentTrackInput)> = {
                if let Ok(mut g) = PLAYER.lock() {
                    if g.queue.is_empty() {
                        g.paused = true;
                        None
                    } else if g.repeat && g.queue.len() == 1 {
                        g.position_sec = 0.0;
                        g.switching_track = true;
                        g.paused = true;
                        let tr = g.queue[g.queue_index].clone();
                        let tag = g.backend_tag.clone();
                        let idx = g.queue_index;
                        g.auto_advance_cb.clone().map(|cb| (cb, tag, idx, tr))
                    } else if g.queue_index + 1 < g.queue.len() {
                        g.queue_index += 1;
                        g.position_sec = 0.0;
                        g.switching_track = true;
                        g.paused = true;
                        let tr = g.queue[g.queue_index].clone();
                        let tag = g.backend_tag.clone();
                        let idx = g.queue_index;
                        g.auto_advance_cb.clone().map(|cb| (cb, tag, idx, tr))
                    } else if g.repeat {
                        g.queue_index = 0;
                        g.position_sec = 0.0;
                        g.switching_track = true;
                        g.paused = true;
                        let tr = g.queue[0].clone();
                        let tag = g.backend_tag.clone();
                        g.auto_advance_cb.clone().map(|cb| (cb, tag, 0, tr))
                    } else {
                        g.paused = true;
                        g.position_sec = g.duration_sec;
                        None
                    }
                } else {
                    None
                }
            };
            wake_ui();
            if let Some((cb, tag, idx, tr)) = next_req {
                cb(tag, idx, tr);
            }
        }
    }
    Ok(())
}
