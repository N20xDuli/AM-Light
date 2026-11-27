use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmtcState {
    pub active: bool,
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub status: String,
    pub position_sec: f64,
    pub duration_sec: f64,
    pub thumbnail_base64: Option<String>,
    pub artwork_url: Option<String>,
    pub backend_mode: String,
    pub audio_quality: Option<String>,
    pub volume: Option<f64>,
    pub agent_pid: Option<u32>,
    pub adam_id: Option<String>,
    pub stream_specs: Option<crate::audio_viz::StreamAudioSpecs>,
    #[serde(skip)]
    pub sampled_at: Option<Instant>,
    #[serde(default)]
    pub timeline_updated_univ: i64,
    #[serde(default)]
    pub position_is_calibrated: bool,
}

impl PartialEq for SmtcState {
    fn eq(&self, other: &Self) -> bool {
        self.active == other.active
            && self.app_id == other.app_id
            && self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.status == other.status
            && self.position_sec == other.position_sec
            && self.duration_sec == other.duration_sec
            && self.thumbnail_base64 == other.thumbnail_base64
            && self.artwork_url == other.artwork_url
            && self.backend_mode == other.backend_mode
            && self.audio_quality == other.audio_quality
            && self.volume == other.volume
            && self.agent_pid == other.agent_pid
            && self.adam_id == other.adam_id
            && self.stream_specs == other.stream_specs
            && self.timeline_updated_univ == other.timeline_updated_univ
            && self.position_is_calibrated == other.position_is_calibrated
    }
}

static SMTC_EVENT_SEQ: AtomicU64 = AtomicU64::new(0);
static SMTC_MEDIA_DIRTY: AtomicBool = AtomicBool::new(true);
static SMTC_PUB_ENABLED: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn smtc_event_seq() -> u64 {
    SMTC_EVENT_SEQ.load(Ordering::Acquire)
}

#[inline]
pub fn is_publisher_enabled() -> bool {
    SMTC_PUB_ENABLED.load(Ordering::Acquire)
}

/// Incoming playback control command received from Windows System Media Transport Controls
/// (`Win + A` media card, Lock Screen, keyboard media keys, Bluetooth headsets, or Lyricify).
#[derive(Clone, Debug, PartialEq)]
pub enum SmtcPublishedCommand {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    SeekSec(f64),
}

/// Snapshot of AM-Light's built-in playback state published out to Windows SMTC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmtcPublishPayload {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub status: String,
    pub position_sec: f64,
    pub duration_sec: f64,
    pub track_key: String,
}

/// Defense-in-depth AUMID check so AM-Light's SMTC consumer never binds to its own published session.
#[inline]
pub fn is_self_aumid(app_id_lower: &str) -> bool {
    app_id_lower.contains("am-light") || app_id_lower.contains("amlight")
}

const SMTC_STALE_THUMB_WINDOW_MS: u64 = 2_500;

#[inline]
fn smtc_thumb_fnv1a_64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Computes the album-level artwork identity for SMTC cross-track stale-thumbnail detection.
/// Tracks sharing the same non-empty album legitimately share cover art (0ms instant display);
/// tracks on different albums (or lacking album metadata) must not inherit the outgoing track's
/// delayed SMTC thumbnail during the ~2.5s async flush window.
pub fn smtc_album_identity_key(title: &str, artist: &str, album: &str) -> String {
    let al = album.trim().to_lowercase();
    let ar = artist.trim().to_lowercase();
    if !al.is_empty() {
        format!("alb:{ar}\x1f{al}")
    } else {
        format!("trk:{}\x1f{ar}", title.trim().to_lowercase())
    }
}

/// Deterministic SMTC thumbnail cache + cross-track stale-bleed guard.
/// Prevents Apple Music / external SMTC's async two-phase `MediaPropertiesChanged`
/// (title/artist updates before `Thumbnail` finishes flushing) from permanently locking
/// the previous song's cover art onto the new song.
#[derive(Clone, Debug, Default)]
pub struct SmtcThumbCacheState {
    pub current_track_key: String,
    pub current_album_key: String,
    pub current_thumb_b64: Option<String>,
    pub current_thumb_hash: Option<u64>,
    pub prev_album_key: String,
    pub prev_thumb_hash: Option<u64>,
    pub track_switched_at_ms: u64,
    pub awaiting_fresh_thumb: bool,
}

impl SmtcThumbCacheState {
    pub fn should_read_thumbnail(&self, track_key: &str, media_dirty: bool) -> bool {
        media_dirty
            || self.current_track_key != track_key
            || self.current_thumb_b64.is_none()
            || self.awaiting_fresh_thumb
    }

    pub fn is_awaiting_fresh_thumb(&self) -> bool {
        self.awaiting_fresh_thumb
    }

    pub fn cached_thumbnail(&self, track_key: &str) -> Option<String> {
        if self.current_track_key == track_key {
            self.current_thumb_b64.clone()
        } else {
            None
        }
    }

    pub fn commit_read_thumbnail(
        &mut self,
        track_key: &str,
        title: &str,
        artist: &str,
        album: &str,
        fresh_b64: Option<String>,
        now_ms: u64,
    ) -> Option<String> {
        let new_album_key = smtc_album_identity_key(title, artist, album);
        if self.current_track_key != track_key {
            if self.current_thumb_hash.is_some() && !self.current_album_key.is_empty() {
                self.prev_album_key = self.current_album_key.clone();
                self.prev_thumb_hash = self.current_thumb_hash;
            }
            self.current_track_key = track_key.to_string();
            self.current_album_key = new_album_key.clone();
            self.track_switched_at_ms = now_ms;
            self.current_thumb_b64 = None;
            self.current_thumb_hash = None;
            self.awaiting_fresh_thumb = false;
        }

        let fresh_hash = fresh_b64
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(smtc_thumb_fnv1a_64);
        let different_album = !self.prev_album_key.is_empty()
            && !new_album_key.is_empty()
            && self.prev_album_key != new_album_key;
        let same_as_prev_bytes = fresh_hash.is_some() && fresh_hash == self.prev_thumb_hash;
        let within_bleed_window =
            now_ms.saturating_sub(self.track_switched_at_ms) < SMTC_STALE_THUMB_WINDOW_MS;

        if different_album && same_as_prev_bytes && within_bleed_window {
            self.current_thumb_b64 = None;
            self.current_thumb_hash = None;
            self.awaiting_fresh_thumb = true;
            return None;
        }

        if let Some(b64) = fresh_b64.filter(|s| !s.is_empty()) {
            self.current_thumb_b64 = Some(b64.clone());
            self.current_thumb_hash = fresh_hash;
            self.awaiting_fresh_thumb = false;
            Some(b64)
        } else {
            self.current_thumb_b64 = None;
            self.current_thumb_hash = None;
            self.awaiting_fresh_thumb = true;
            None
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Pure deterministic timeline update policy for SMTC publishing:
/// immediately updates on track switch, play/pause toggle, seek, duration change,
/// playback drift >0.35s, paused seek >0.20s, or gentle 5s heartbeat while playing.
#[inline]
pub fn compute_timeline_should_update(
    force: bool,
    track_changed: bool,
    status_changed: bool,
    is_playing: bool,
    cur_pos: f64,
    cur_dur: f64,
    last_pos: f64,
    last_dur: f64,
    elapsed_sec: f64,
) -> bool {
    if force || track_changed || status_changed || (cur_dur - last_dur).abs() > 0.25 {
        return true;
    }
    if is_playing {
        let predicted = last_pos + elapsed_sec.max(0.0);
        (cur_pos - predicted).abs() > 0.35 || elapsed_sec >= 5.0
    } else {
        (cur_pos - last_pos).abs() > 0.20
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};
    use windows::core::{Interface, BOOL, HSTRING, PWSTR};
    use windows::Foundation::{TimeSpan, TypedEventHandler};
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus, MediaPropertiesChangedEventArgs,
        PlaybackInfoChangedEventArgs, SessionsChangedEventArgs, TimelinePropertiesChangedEventArgs,
    };
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
        SystemMediaTransportControls, SystemMediaTransportControlsButton,
        SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    };
    use windows::Storage::Streams::{
        DataReader, DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference,
    };
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible, ShowWindow, SW_MINIMIZE,
    };

    const TICKS_PER_SEC: i64 = 10_000_000;
    /// Sub-second WinRT/SMTC event IPC lag compensation (`LyricsBlossom` `0x40210c`: `now - 295ms`).
    const ROLLOVER_IPC_COMP_SEC: f64 = 0.295;

    struct SmtcComGuard {
        should_uninit: bool,
    }

    impl Drop for SmtcComGuard {
        fn drop(&mut self) {
            if self.should_uninit {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    thread_local! {
        static SMTC_COM_GUARD: RefCell<Option<SmtcComGuard>> = const { RefCell::new(None) };
    }

    fn ensure_com() {
        SMTC_COM_GUARD.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                unsafe {
                    // Initialize MTA COM once per thread and pair it with CoUninitialize on thread exit.
                    // Only call CoUninitialize if CoInitializeEx succeeded (S_OK / S_FALSE); never on RPC_E_CHANGED_MODE.
                    let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
                    *slot = Some(SmtcComGuard {
                        should_uninit: hr.is_ok(),
                    });
                    let _ = crate::am_agent::init_process_com_security();
                }
            }
        });
    }

    #[inline]
    fn is_apple_music_aumid(app_id_lower: &str) -> bool {
        if is_self_aumid(app_id_lower) {
            return false;
        }
        app_id_lower.contains("applemusic")
            || app_id_lower.contains("apple.music")
            || app_id_lower.contains("music.ui")
            || app_id_lower.contains("itunes")
    }

    struct SmtcTracker {
        manager: Option<GlobalSystemMediaTransportControlsSessionManager>,
        sessions_token: Option<i64>,
        bound_session: Option<GlobalSystemMediaTransportControlsSession>,
        bound_raw_ptr: usize,
        tl_token: Option<i64>,
        pb_token: Option<i64>,
        mp_token: Option<i64>,
        // Cached media metadata (refreshed on `MediaPropertiesChanged` + 2.0s watchdog)
        cached_title: String,
        cached_artist: String,
        cached_album: String,
        cached_thumb: Option<String>,
        last_media_fetch: Option<Instant>,
        // Sub-second whole-second rollover phase calibrator (`#7` `0x401f0f..0x402383`)
        track_key: String,
        prev_raw_ticks: i64,
        was_playing: bool,
        has_cal: bool,
        cal_trusted: bool,
        cal_base_sec: f64,
        cal_anchor_inst: Option<Instant>,
        paused_pos_sec: Option<f64>,
        synthetic_timeline_id: i64,
        last_song_timeline_univ: i64,
        prev_song_timeline_univ: i64,
    }

    impl Default for SmtcTracker {
        fn default() -> Self {
            Self {
                manager: None,
                sessions_token: None,
                bound_session: None,
                bound_raw_ptr: 0,
                tl_token: None,
                pb_token: None,
                mp_token: None,
                cached_title: String::new(),
                cached_artist: String::new(),
                cached_album: String::new(),
                cached_thumb: None,
                last_media_fetch: None,
                track_key: String::new(),
                prev_raw_ticks: -1,
                was_playing: false,
                has_cal: false,
                cal_trusted: false,
                cal_base_sec: 0.0,
                cal_anchor_inst: None,
                paused_pos_sec: None,
                synthetic_timeline_id: 1_000_000,
                last_song_timeline_univ: 0,
                prev_song_timeline_univ: 0,
            }
        }
    }

    static SMTC_TRACKER: OnceLock<Mutex<SmtcTracker>> = OnceLock::new();
    static SMTC_THUMB_CACHE: OnceLock<Mutex<SmtcThumbCacheState>> = OnceLock::new();
    static SMTC_EPOCH_START: OnceLock<Instant> = OnceLock::new();

    #[inline]
    fn smtc_now_ms() -> u64 {
        SMTC_EPOCH_START
            .get_or_init(Instant::now)
            .elapsed()
            .as_millis() as u64
    }

    fn thumb_cache_mtx() -> &'static Mutex<SmtcThumbCacheState> {
        SMTC_THUMB_CACHE.get_or_init(|| Mutex::new(SmtcThumbCacheState::default()))
    }

    fn tracker_mtx() -> &'static Mutex<SmtcTracker> {
        SMTC_TRACKER.get_or_init(|| Mutex::new(SmtcTracker::default()))
    }

    fn handle_timeline_event(sess: &GlobalSystemMediaTransportControlsSession) {
        let now_inst = Instant::now();
        let is_playing = sess
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            .unwrap_or(false);
        let Ok(tl) = sess.GetTimelineProperties() else {
            return;
        };
        let start = tl.StartTime().map(|d| d.Duration).unwrap_or(0);
        let pos = tl.Position().map(|d| d.Duration).unwrap_or(0);
        let tl_univ = tl.LastUpdatedTime().map(|dt| dt.UniversalTime).unwrap_or(0);
        let raw_ticks = (pos - start).max(0);
        let is_whole_second = (raw_ticks % TICKS_PER_SEC) == 0;

        if let Ok(mut tr) = tracker_mtx().lock() {
            if is_whole_second {
                if is_playing
                    && tr.was_playing
                    && tr.prev_raw_ticks >= 0
                    && (raw_ticks - tr.prev_raw_ticks) == TICKS_PER_SEC
                {
                    let new_cal_sec = (raw_ticks as f64) / (TICKS_PER_SEC as f64) + ROLLOVER_IPC_COMP_SEC;
                    let drift_exceeds = if tr.has_cal && tr.cal_trusted {
                        tr.cal_anchor_inst
                            .map(|t0| {
                                let pred = tr.cal_base_sec + t0.elapsed().as_secs_f64();
                                (new_cal_sec - pred).abs() > 0.050
                            })
                            .unwrap_or(true)
                    } else {
                        true
                    };
                    tr.cal_base_sec = new_cal_sec;
                    tr.cal_anchor_inst = Some(now_inst);
                    tr.has_cal = true;
                    tr.cal_trusted = true;
                    tr.paused_pos_sec = None;
                    if drift_exceeds {
                        tr.synthetic_timeline_id = tr.synthetic_timeline_id.wrapping_add(1);
                    }
                } else if is_playing
                    && (!tr.was_playing
                        || tr.prev_raw_ticks < 0
                        || (raw_ticks - tr.prev_raw_ticks).abs() >= TICKS_PER_SEC)
                {
                    // Non-`+1s` jump or start/resume: clear phase lock until next clean `+1.000s` rollover
                    tr.has_cal = false;
                    tr.cal_trusted = false;
                    tr.cal_anchor_inst = None;
                    tr.paused_pos_sec = None;
                    tr.synthetic_timeline_id = tr.synthetic_timeline_id.wrapping_add(1);
                } else if !is_playing && tr.was_playing {
                    let frozen = if tr.has_cal && tr.cal_trusted {
                        tr.cal_anchor_inst
                            .map(|t0| tr.cal_base_sec + t0.elapsed().as_secs_f64())
                            .unwrap_or((raw_ticks as f64) / (TICKS_PER_SEC as f64))
                    } else {
                        (raw_ticks as f64) / (TICKS_PER_SEC as f64)
                    };
                    tr.paused_pos_sec = Some(frozen);
                    tr.cal_trusted = false;
                    tr.synthetic_timeline_id = tr.synthetic_timeline_id.wrapping_add(1);
                }
            } else {
                tr.has_cal = false;
                tr.cal_trusted = false;
                tr.paused_pos_sec = None;
            }
            tr.prev_raw_ticks = raw_ticks;
            tr.was_playing = is_playing;
            if tl_univ > 0 {
                tr.last_song_timeline_univ = tl_univ;
            }
        }

        SMTC_EVENT_SEQ.fetch_add(1, Ordering::Release);
        crate::audio_viz::wake_registered_ui_window();
    }

    fn handle_playback_info_event(sess: &GlobalSystemMediaTransportControlsSession) {
        let now_inst = Instant::now();
        let is_playing = sess
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            .unwrap_or(false);

        if let Ok(mut tr) = tracker_mtx().lock() {
            if !is_playing && tr.was_playing {
                if tr.has_cal {
                    if let Some(t0) = tr.cal_anchor_inst {
                        tr.paused_pos_sec = Some(tr.cal_base_sec + t0.elapsed().as_secs_f64());
                    }
                }
                tr.cal_trusted = false;
                tr.synthetic_timeline_id = tr.synthetic_timeline_id.wrapping_add(1);
            } else if is_playing && !tr.was_playing {
                if let Some(frozen) = tr.paused_pos_sec.take() {
                    tr.cal_base_sec = frozen;
                    tr.cal_anchor_inst = Some(now_inst);
                    tr.has_cal = true;
                    tr.cal_trusted = false;
                }
                tr.synthetic_timeline_id = tr.synthetic_timeline_id.wrapping_add(1);
            }
            tr.was_playing = is_playing;
        }

        SMTC_EVENT_SEQ.fetch_add(1, Ordering::Release);
        crate::audio_viz::wake_registered_ui_window();
    }

    fn pick_apple_session(
        manager: &GlobalSystemMediaTransportControlsSessionManager,
    ) -> Option<GlobalSystemMediaTransportControlsSession> {
        if let Ok(sessions) = manager.GetSessions() {
            let count = sessions.Size().unwrap_or(0);
            for i in 0..count {
                if let Ok(sess) = sessions.GetAt(i) {
                    let app_id = sess
                        .SourceAppUserModelId()
                        .map(|h| h.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default();
                    if is_apple_music_aumid(&app_id) {
                        return Some(sess);
                    }
                }
            }
        }

        if let Ok(curr) = manager.GetCurrentSession() {
            let app_id = curr
                .SourceAppUserModelId()
                .map(|h| h.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if is_apple_music_aumid(&app_id) {
                return Some(curr);
            }
        }

        None
    }

    fn find_apple_music_session() -> Option<GlobalSystemMediaTransportControlsSession> {
        ensure_com();

        // 1. Obtain cached SessionManager (creating & subscribing once outside lock)
        let mut manager_opt = tracker_mtx()
            .lock()
            .ok()
            .and_then(|tr| tr.manager.clone());

        if manager_opt.is_none() {
            let fresh_mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                .ok()?
                .join()
                .ok()?;
            let sessions_tok = fresh_mgr
                .SessionsChanged(&TypedEventHandler::<
                    GlobalSystemMediaTransportControlsSessionManager,
                    SessionsChangedEventArgs,
                >::new(|_mgr, _args| {
                    SMTC_MEDIA_DIRTY.store(true, Ordering::Release);
                    SMTC_EVENT_SEQ.fetch_add(1, Ordering::Release);
                    crate::audio_viz::wake_registered_ui_window();
                    Ok(())
                }))
                .ok();
            if let Ok(mut tr) = tracker_mtx().lock() {
                tr.manager = Some(fresh_mgr.clone());
                tr.sessions_token = sessions_tok;
            }
            manager_opt = Some(fresh_mgr);
        }

        let manager = manager_opt?;
        let sess_opt = match manager.GetSessions() {
            Ok(_) => pick_apple_session(&manager),
            Err(_) => {
                // Manager RPC invalidated (e.g. Explorer/SMTC service restarted): drop cached manager
                if let Ok(mut tr) = tracker_mtx().lock() {
                    tr.manager = None;
                    tr.sessions_token = None;
                    tr.bound_session = None;
                    tr.bound_raw_ptr = 0;
                }
                return None;
            }
        };

        let Some(sess) = sess_opt else {
            let prev_unbind = tracker_mtx().lock().ok().and_then(|mut tr| {
                tr.bound_raw_ptr = 0;
                tr.has_cal = false;
                tr.cal_trusted = false;
                tr.cached_title.clear();
                tr.cached_artist.clear();
                tr.cached_album.clear();
                tr.cached_thumb = None;
                tr.last_media_fetch = None;
                let old_s = tr.bound_session.take()?;
                Some((
                    old_s,
                    tr.tl_token.take(),
                    tr.pb_token.take(),
                    tr.mp_token.take(),
                ))
            });
            if let Ok(mut tc) = thumb_cache_mtx().lock() {
                tc.clear();
            }
            if let Some((old_s, tl_t, pb_t, mp_t)) = prev_unbind {
                if let Some(t) = tl_t {
                    let _ = old_s.RemoveTimelinePropertiesChanged(t);
                }
                if let Some(t) = pb_t {
                    let _ = old_s.RemovePlaybackInfoChanged(t);
                }
                if let Some(t) = mp_t {
                    let _ = old_s.RemoveMediaPropertiesChanged(t);
                }
            }
            return None;
        };

        // 2. Subscribe WinRT timeline/playback/media listeners if session instance changed
        let raw_ptr = sess.as_raw() as usize;
        let already_bound = tracker_mtx()
            .lock()
            .map(|tr| tr.bound_raw_ptr == raw_ptr && tr.bound_session.is_some())
            .unwrap_or(false);

        if !already_bound {
            let old_unbind = tracker_mtx().lock().ok().and_then(|mut tr| {
                let old_s = tr.bound_session.take()?;
                Some((
                    old_s,
                    tr.tl_token.take(),
                    tr.pb_token.take(),
                    tr.mp_token.take(),
                ))
            });
            if let Some((old_s, tl_t, pb_t, mp_t)) = old_unbind {
                if let Some(t) = tl_t {
                    let _ = old_s.RemoveTimelinePropertiesChanged(t);
                }
                if let Some(t) = pb_t {
                    let _ = old_s.RemovePlaybackInfoChanged(t);
                }
                if let Some(t) = mp_t {
                    let _ = old_s.RemoveMediaPropertiesChanged(t);
                }
            }

            let tl_tok = sess
                .TimelinePropertiesChanged(&TypedEventHandler::<
                    GlobalSystemMediaTransportControlsSession,
                    TimelinePropertiesChangedEventArgs,
                >::new(|sender, _args| {
                    if let Some(s) = sender.as_ref() {
                        handle_timeline_event(s);
                    }
                    Ok(())
                }))
                .ok();
            let pb_tok = sess
                .PlaybackInfoChanged(&TypedEventHandler::<
                    GlobalSystemMediaTransportControlsSession,
                    PlaybackInfoChangedEventArgs,
                >::new(|sender, _args| {
                    if let Some(s) = sender.as_ref() {
                        handle_playback_info_event(s);
                    }
                    Ok(())
                }))
                .ok();
            let mp_tok = sess
                .MediaPropertiesChanged(&TypedEventHandler::<
                    GlobalSystemMediaTransportControlsSession,
                    MediaPropertiesChangedEventArgs,
                >::new(|_sender, _args| {
                    SMTC_MEDIA_DIRTY.store(true, Ordering::Release);
                    SMTC_EVENT_SEQ.fetch_add(1, Ordering::Release);
                    crate::audio_viz::wake_registered_ui_window();
                    Ok(())
                }))
                .ok();

            SMTC_MEDIA_DIRTY.store(true, Ordering::Release);
            if let Ok(mut tr) = tracker_mtx().lock() {
                tr.bound_session = Some(sess.clone());
                tr.bound_raw_ptr = raw_ptr;
                tr.tl_token = tl_tok;
                tr.pb_token = pb_tok;
                tr.mp_token = mp_tok;
                tr.cached_title.clear();
                tr.cached_artist.clear();
                tr.cached_album.clear();
                tr.cached_thumb = None;
                tr.last_media_fetch = None;
                tr.prev_raw_ticks = -1;
                tr.has_cal = false;
                tr.cal_trusted = false;
                tr.cal_anchor_inst = None;
                tr.paused_pos_sec = None;
            }
            if let Ok(mut tc) = thumb_cache_mtx().lock() {
                tc.clear();
            }
        }

        Some(sess)
    }

    fn read_thumbnail_b64(
        session_props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
    ) -> Option<String> {
        let thumb_ref = session_props.Thumbnail().ok()?;
        let stream = thumb_ref.OpenReadAsync().ok()?.join().ok()?;
        let size = stream.Size().ok()?;
        if size == 0 || size > 4 * 1024 * 1024 {
            return None;
        }
        let size_u32 = size as u32;
        let reader = DataReader::CreateDataReader(&stream).ok()?;
        let loaded = reader.LoadAsync(size_u32).ok()?.join().ok()?;
        if loaded == 0 {
            return None;
        }
        let mut buf = vec![0u8; loaded as usize];
        reader.ReadBytes(&mut buf).ok()?;
        Some(general_purpose::STANDARD.encode(buf))
    }

    pub fn get_smtc_state() -> SmtcState {
        let Some(session) = find_apple_music_session() else {
            return SmtcState {
                status: "Stopped".to_string(),
                sampled_at: Some(Instant::now()),
                ..SmtcState::default()
            };
        };

        let app_id = session
            .SourceAppUserModelId()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();

        let status = match session.GetPlaybackInfo() {
            Ok(info) => match info.PlaybackStatus() {
                Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing) => "Playing",
                Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused) => "Paused",
                Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped) => "Stopped",
                _ => "Unknown",
            },
            Err(_) => "Unknown",
        }
        .to_string();

        // Refresh MediaProperties immediately on `MediaPropertiesChanged`, empty cache,
        // 250ms retry while awaiting a delayed fresh thumbnail, or 2.0s watchdog (`#7` `0x404ca5`).
        let media_dirty = SMTC_MEDIA_DIRTY.swap(false, Ordering::AcqRel);
        let awaiting_thumb = thumb_cache_mtx()
            .lock()
            .map(|g| g.is_awaiting_fresh_thumb())
            .unwrap_or(false);
        let watchdog_ms = if awaiting_thumb { 250u64 } else { 2000u64 };
        let need_media_fetch = media_dirty
            || tracker_mtx()
                .lock()
                .map(|tr| {
                    tr.cached_title.is_empty()
                        || tr
                            .last_media_fetch
                            .is_none_or(|t0| t0.elapsed() >= Duration::from_millis(watchdog_ms))
                })
                .unwrap_or(true);

        let (title, artist, album, thumbnail_base64) = if need_media_fetch {
            match session
                .TryGetMediaPropertiesAsync()
                .ok()
                .and_then(|op| op.join().ok())
            {
                Some(props) => {
                    let t = props.Title().map(|h| h.to_string_lossy()).unwrap_or_default();
                    let ar = props.Artist().map(|h| h.to_string_lossy()).unwrap_or_default();
                    let al = props
                        .AlbumTitle()
                        .map(|h| h.to_string_lossy())
                        .unwrap_or_default();
                    let thumb_key = format!("{t}\x1f{ar}\x1f{al}");
                    let thumb = if !t.is_empty() {
                        let should_read = thumb_cache_mtx()
                            .lock()
                            .map(|g| g.should_read_thumbnail(&thumb_key, media_dirty))
                            .unwrap_or(true);
                        if should_read {
                            let fresh = read_thumbnail_b64(&props);
                            let now_ms = smtc_now_ms();
                            thumb_cache_mtx()
                                .lock()
                                .map(|mut g| {
                                    g.commit_read_thumbnail(&thumb_key, &t, &ar, &al, fresh, now_ms)
                                })
                                .unwrap_or(None)
                        } else {
                            thumb_cache_mtx()
                                .lock()
                                .ok()
                                .and_then(|g| g.cached_thumbnail(&thumb_key))
                        }
                    } else {
                        None
                    };
                    if let Ok(mut tr) = tracker_mtx().lock() {
                        tr.cached_title = t.clone();
                        tr.cached_artist = ar.clone();
                        tr.cached_album = al.clone();
                        tr.cached_thumb = thumb.clone();
                        tr.last_media_fetch = Some(Instant::now());
                    }
                    (t, ar, al, thumb)
                }
                None => tracker_mtx()
                    .lock()
                    .map(|tr| {
                        (
                            tr.cached_title.clone(),
                            tr.cached_artist.clone(),
                            tr.cached_album.clone(),
                            tr.cached_thumb.clone(),
                        )
                    })
                    .unwrap_or_else(|_| (String::new(), String::new(), String::new(), None)),
            }
        } else {
            tracker_mtx()
                .lock()
                .map(|tr| {
                    (
                        tr.cached_title.clone(),
                        tr.cached_artist.clone(),
                        tr.cached_album.clone(),
                        tr.cached_thumb.clone(),
                    )
                })
                .unwrap_or_else(|_| (String::new(), String::new(), String::new(), None))
        };

        let sampled_now = Instant::now();
        let (position_sec, duration_sec, timeline_updated_univ, position_is_calibrated) =
            match session.GetTimelineProperties() {
                Ok(tl) => {
                    let start = tl.StartTime().map(|d| d.Duration).unwrap_or(0);
                    let end = tl.EndTime().map(|d| d.Duration).unwrap_or(0);
                    let pos = tl.Position().map(|d| d.Duration).unwrap_or(0);
                    let tl_univ = tl.LastUpdatedTime().map(|dt| dt.UniversalTime).unwrap_or(0);
                    let dur = ((end - start).max(0) as f64) / (TICKS_PER_SEC as f64);
                    let raw_ticks = (pos - start).max(0);
                    let is_whole_second = (raw_ticks % TICKS_PER_SEC) == 0;
                    let is_playing = status == "Playing";

                    let mut out_pos = (raw_ticks as f64) / (TICKS_PER_SEC as f64);
                    let mut out_univ = tl_univ;
                    let mut out_cal = !is_whole_second;

                    if let Ok(mut tr) = tracker_mtx().lock() {
                        let new_tkey = format!("{title}\x1f{artist}\x1f{album}");
                        if !title.is_empty() && tr.track_key != new_tkey {
                            if !tr.track_key.is_empty() {
                                tr.prev_song_timeline_univ = tr.last_song_timeline_univ;
                            }
                            tr.track_key = new_tkey;
                            tr.prev_raw_ticks = raw_ticks;
                            tr.has_cal = false;
                            tr.cal_trusted = false;
                            tr.cal_anchor_inst = None;
                            tr.paused_pos_sec = None;
                            tr.synthetic_timeline_id = tr.synthetic_timeline_id.wrapping_add(1);
                        }

                        if tl_univ > 0 && tl_univ == tr.prev_song_timeline_univ {
                            // MediaProperties updated to Song B before TimelineProperties updated from Song A!
                            out_pos = 0.0;
                            out_univ = tl_univ;
                            out_cal = false;
                        } else {
                            if tl_univ > 0 {
                                tr.last_song_timeline_univ = tl_univ;
                                tr.prev_song_timeline_univ = 0;
                            }
                            let raw_sec = (raw_ticks as f64) / (TICKS_PER_SEC as f64);
                            let mut cal_fresh = tr.has_cal
                                && tr.cal_trusted
                                && tr.cal_anchor_inst.is_some();
                            if cal_fresh && !is_playing {
                                cal_fresh = tr
                                    .cal_anchor_inst
                                    .is_some_and(|t0| t0.elapsed() < Duration::from_secs(5));
                            }
                            if cal_fresh {
                                let cand_cal = if is_playing {
                                    tr.cal_base_sec
                                        + tr.cal_anchor_inst
                                            .map(|t0| t0.elapsed().as_secs_f64())
                                            .unwrap_or(0.0)
                                } else {
                                    tr.paused_pos_sec.unwrap_or(tr.cal_base_sec)
                                };
                                // Invalidate calibration only if a newly updated `tl_univ` diverges >2.2s
                                if tl_univ != 0
                                    && tr.prev_raw_ticks != raw_ticks
                                    && (cand_cal - raw_sec).abs() > 2.2
                                {
                                    tr.has_cal = false;
                                    tr.cal_trusted = false;
                                    tr.cal_anchor_inst = None;
                                    cal_fresh = false;
                                } else {
                                    out_pos = cand_cal;
                                }
                            }

                            if cal_fresh {
                                out_univ = tr.synthetic_timeline_id;
                                out_cal = tr.cal_trusted;
                            } else {
                                if is_playing && tl_univ > 100_000_000_000_000_000 {
                                    let unix_100ns = (std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_nanos()
                                        / 100)
                                        as i64;
                                    let now_univ = unix_100ns + 116_444_736_000_000_000i64;
                                    let elapsed_sec = ((now_univ - tl_univ).max(0) as f64)
                                        / (TICKS_PER_SEC as f64);
                                    if elapsed_sec < 1800.0 {
                                        out_pos += elapsed_sec;
                                    }
                                }
                                out_univ = tl_univ;
                                out_cal = !is_whole_second;
                            }
                            tr.was_playing = is_playing;
                            tr.prev_raw_ticks = raw_ticks;
                        }
                    } else if is_playing && tl_univ > 100_000_000_000_000_000 {
                        let unix_100ns = (std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                            / 100) as i64;
                        let now_univ = unix_100ns + 116_444_736_000_000_000i64;
                        let elapsed_sec =
                            ((now_univ - tl_univ).max(0) as f64) / (TICKS_PER_SEC as f64);
                        if elapsed_sec < 1800.0 {
                            out_pos += elapsed_sec;
                        }
                    }

                    if dur > 0.0 && out_pos > dur {
                        out_pos = dur;
                    }
                    (out_pos.max(0.0), dur, out_univ, out_cal)
                }
                Err(_) => (0.0, 0.0, 0, false),
            };

        let active = !title.is_empty() || status == "Playing" || status == "Paused";

        SmtcState {
            active,
            app_id,
            title,
            artist,
            album,
            status,
            position_sec,
            duration_sec,
            thumbnail_base64,
            backend_mode: if active {
                "smtc".to_string()
            } else {
                String::new()
            },
            sampled_at: Some(sampled_now),
            timeline_updated_univ,
            position_is_calibrated,
            ..SmtcState::default()
        }
    }

    pub fn control_playback(action: &str, seek_sec: Option<f64>) -> Result<(), String> {
        let session = find_apple_music_session()
            .ok_or_else(|| "当前没有运行中的官方 Apple Music 媒体会话".to_string())?;

        match action {
            "toggle" => {
                session
                    .TryTogglePlayPauseAsync()
                    .map_err(|e| format!("切换播放失败: {e}"))?
                    .join()
                    .map_err(|e| format!("切换播放失败: {e}"))?;
            }
            "play" => {
                session
                    .TryPlayAsync()
                    .map_err(|e| format!("播放失败: {e}"))?
                    .join()
                    .map_err(|e| format!("播放失败: {e}"))?;
            }
            "pause" => {
                session
                    .TryPauseAsync()
                    .map_err(|e| format!("暂停失败: {e}"))?
                    .join()
                    .map_err(|e| format!("暂停失败: {e}"))?;
            }
            "next" => {
                SMTC_MEDIA_DIRTY.store(true, Ordering::Release);
                session
                    .TrySkipNextAsync()
                    .map_err(|e| format!("下一曲失败: {e}"))?
                    .join()
                    .map_err(|e| format!("下一曲失败: {e}"))?;
            }
            "prev" => {
                SMTC_MEDIA_DIRTY.store(true, Ordering::Release);
                session
                    .TrySkipPreviousAsync()
                    .map_err(|e| format!("上一曲失败: {e}"))?
                    .join()
                    .map_err(|e| format!("上一曲失败: {e}"))?;
            }
            "seek" => {
                let secs = seek_sec.unwrap_or(0.0).max(0.0);
                let ticks = (secs * (TICKS_PER_SEC as f64)) as i64;
                if let Ok(mut tr) = tracker_mtx().lock() {
                    tr.cal_base_sec = secs;
                    tr.cal_anchor_inst = Some(Instant::now());
                    tr.has_cal = true;
                    tr.cal_trusted = false;
                    tr.paused_pos_sec = (!tr.was_playing).then_some(secs);
                    tr.synthetic_timeline_id = tr.synthetic_timeline_id.wrapping_add(1);
                }
                session
                    .TryChangePlaybackPositionAsync(ticks)
                    .map_err(|e| format!("跳转进度失败: {e}"))?
                    .join()
                    .map_err(|e| format!("跳转进度失败: {e}"))?;
            }
            other => return Err(format!("未知播放控制指令: {other}")),
        }

        Ok(())
    }

    unsafe extern "system" fn enum_minimize_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
            return BOOL(1);
        }

        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return BOOL(1);
        }

        let Ok(proc_handle) =
            (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return BOOL(1);
        };

        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(
                proc_handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        };
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(proc_handle) };

        if ok.is_ok() && len > 0 {
            let exe_path = String::from_utf16_lossy(&buf[..len as usize]);
            let is_official_am = Path::new(&exe_path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("AppleMusic.exe"))
                .unwrap_or(false);

            if is_official_am {
                let _ = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
                let count_ptr = lparam.0 as *mut u32;
                if !count_ptr.is_null() {
                    unsafe {
                        *count_ptr += 1;
                    }
                }
            }
        }

        BOOL(1)
    }

    pub fn minimize_official_am() -> Result<u32, String> {
        let mut count: u32 = 0;
        let ptr = (&mut count as *mut u32) as isize;
        unsafe { EnumWindows(Some(enum_minimize_proc), LPARAM(ptr)) }
            .map_err(|e| format!("枚举窗口失败: {e}"))?;
        Ok(count)
    }

    // ========================================================================
    // Windows SMTC Publisher (Provider) for Built-in Playback Backends
    // Publishes metadata, album art, sub-ms timeline & handles media keys/seek
    // via `ISystemMediaTransportControlsInterop::GetForWindow(hwnd)`.
    // ========================================================================

    struct SmtcPublisher {
        controls: SystemMediaTransportControls,
        btn_token: Option<i64>,
        pos_token: Option<i64>,
        enabled: bool,
        last_status: String,
        last_meta_key: String,
        last_track_key: String,
        last_thumb_key: String,
        last_tl_pos: f64,
        last_tl_dur: f64,
        last_tl_at: Option<Instant>,
    }

    // Strictly thread-local on the STA UI thread that owns `hwnd` and `SystemMediaTransportControls`!
    // Never touch `SystemMediaTransportControls` or `DisplayUpdater` from Tokio MTA worker threads,
    // preventing cross-apartment WinRT crashes (`RPC_E_WRONG_THREAD` / AV) and lock deadlocks!
    thread_local! {
        static SMTC_PUBLISHER: RefCell<Option<SmtcPublisher>> = const { RefCell::new(None) };
    }
    static PUBLISHED_CMD_QUEUE: OnceLock<Mutex<VecDeque<SmtcPublishedCommand>>> = OnceLock::new();

    fn cmd_queue_mtx() -> &'static Mutex<VecDeque<SmtcPublishedCommand>> {
        PUBLISHED_CMD_QUEUE.get_or_init(|| Mutex::new(VecDeque::with_capacity(16)))
    }

    pub fn enqueue_published_command(cmd: SmtcPublishedCommand) {
        if let Ok(mut q) = cmd_queue_mtx().lock() {
            if q.len() >= 32 {
                q.pop_front();
            }
            q.push_back(cmd);
        }
        crate::audio_viz::wake_registered_ui_window();
    }

    pub fn drain_published_commands() -> Vec<SmtcPublishedCommand> {
        let Ok(mut q) = cmd_queue_mtx().lock() else {
            return Vec::new();
        };
        q.drain(..).collect()
    }

    /// Initializes the WinRT SMTC provider bound to AM-Light's main `HWND`.
    /// Must be called on the STA UI thread immediately after window creation.
    pub fn init_publisher(hwnd_usize: usize) -> bool {
        if hwnd_usize == 0 {
            return false;
        }
        let already = SMTC_PUBLISHER.with(|cell| cell.borrow().is_some());
        if already {
            return true;
        }
        let interop = match windows::core::factory::<
            SystemMediaTransportControls,
            ISystemMediaTransportControlsInterop,
        >() {
            Ok(i) => i,
            Err(e) => {
                crate::logger::warn("smtc", &format!("ISystemMediaTransportControlsInterop factory failed: {e}"));
                return false;
            }
        };
        let hwnd = HWND(hwnd_usize as *mut std::ffi::c_void);
        let controls = match unsafe { interop.GetForWindow::<SystemMediaTransportControls>(hwnd) } {
            Ok(c) => c,
            Err(e) => {
                crate::logger::warn("smtc", &format!("GetForWindow failed: {e}"));
                return false;
            }
        };

        let _ = controls.SetIsPlayEnabled(true);
        let _ = controls.SetIsPauseEnabled(true);
        let _ = controls.SetIsStopEnabled(true);
        let _ = controls.SetIsNextEnabled(true);
        let _ = controls.SetIsPreviousEnabled(true);
        let _ = controls.SetIsFastForwardEnabled(false);
        let _ = controls.SetIsRewindEnabled(false);
        let _ = controls.SetIsEnabled(false);
        SMTC_PUB_ENABLED.store(false, Ordering::Release);

        let btn_token = controls
            .ButtonPressed(&TypedEventHandler::<
                SystemMediaTransportControls,
                SystemMediaTransportControlsButtonPressedEventArgs,
            >::new(|_sender, args| {
                if let Some(a) = args.as_ref() {
                    if let Ok(btn) = a.Button() {
                        let cmd = match btn {
                            SystemMediaTransportControlsButton::Play => {
                                Some(SmtcPublishedCommand::Play)
                            }
                            SystemMediaTransportControlsButton::Pause => {
                                Some(SmtcPublishedCommand::Pause)
                            }
                            SystemMediaTransportControlsButton::Stop => {
                                Some(SmtcPublishedCommand::Stop)
                            }
                            SystemMediaTransportControlsButton::Next => {
                                Some(SmtcPublishedCommand::Next)
                            }
                            SystemMediaTransportControlsButton::Previous => {
                                Some(SmtcPublishedCommand::Previous)
                            }
                            _ => None,
                        };
                        if let Some(c) = cmd {
                            enqueue_published_command(c);
                        }
                    }
                }
                Ok(())
            }))
            .ok();

        let pos_token = controls
            .PlaybackPositionChangeRequested(&TypedEventHandler::<
                SystemMediaTransportControls,
                PlaybackPositionChangeRequestedEventArgs,
            >::new(|_sender, args| {
                if let Some(a) = args.as_ref() {
                    if let Ok(ts) = a.RequestedPlaybackPosition() {
                        let secs = (ts.Duration.max(0) as f64) / (TICKS_PER_SEC as f64);
                        enqueue_published_command(SmtcPublishedCommand::SeekSec(secs));
                    }
                }
                Ok(())
            }))
            .ok();

        SMTC_PUBLISHER.with(|cell| {
            if let Ok(mut guard) = cell.try_borrow_mut() {
                *guard = Some(SmtcPublisher {
                    controls,
                    btn_token,
                    pos_token,
                    enabled: false,
                    last_status: String::new(),
                    last_meta_key: String::new(),
                    last_track_key: String::new(),
                    last_thumb_key: String::new(),
                    last_tl_pos: 0.0,
                    last_tl_dur: 0.0,
                    last_tl_at: None,
                });
            }
        });
        crate::logger::info("smtc", "Windows SMTC publisher initialized on UI STA thread");
        true
    }

    /// Publishes built-in playback status, music metadata, and sub-millisecond timeline to Windows SMTC.
    /// Must run on the STA UI thread. Returns `true` when the thumbnail for `payload.track_key` still needs upload.
    pub fn sync_publisher_state(payload: &SmtcPublishPayload, force_timeline: bool) -> bool {
        SMTC_PUBLISHER.with(|cell| {
            let Ok(mut guard) = cell.try_borrow_mut() else {
                return false;
            };
            let Some(p) = guard.as_mut() else {
                return false;
            };

            let just_enabled = !p.enabled;
            if just_enabled {
                let _ = p.controls.SetIsEnabled(true);
                p.enabled = true;
                SMTC_PUB_ENABLED.store(true, Ordering::Release);
            }

            let status_changed = just_enabled || p.last_status != payload.status;
            if status_changed {
                let pb = match payload.status.as_str() {
                    "Playing" => MediaPlaybackStatus::Playing,
                    "Paused" => MediaPlaybackStatus::Paused,
                    "Stopped" => MediaPlaybackStatus::Stopped,
                    _ => MediaPlaybackStatus::Closed,
                };
                let _ = p.controls.SetPlaybackStatus(pb);
                p.last_status = payload.status.clone();
            }

            let meta_key = format!("{}\x1f{}\x1f{}", payload.title, payload.artist, payload.album);
            let track_changed =
                just_enabled || p.last_meta_key != meta_key || p.last_track_key != payload.track_key;
            if track_changed {
                if let Ok(updater) = p.controls.DisplayUpdater() {
                    let _ = updater.ClearAll();
                    let _ = updater.SetType(MediaPlaybackType::Music);
                    if let Ok(mp) = updater.MusicProperties() {
                        let _ = mp.SetTitle(&HSTRING::from(&payload.title));
                        let _ = mp.SetArtist(&HSTRING::from(&payload.artist));
                        let _ = mp.SetAlbumTitle(&HSTRING::from(&payload.album));
                    }
                    let _ = updater.Update();
                }
                p.last_meta_key = meta_key;
                p.last_thumb_key.clear();
            }
            p.last_track_key = payload.track_key.clone();

            let dur_s = payload.duration_sec.max(0.0);
            let pos_s = if dur_s > 0.0 {
                payload.position_sec.clamp(0.0, dur_s)
            } else {
                payload.position_sec.max(0.0)
            };
            let is_playing = payload.status == "Playing";
            let elapsed_sec = p
                .last_tl_at
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(999.0);

            if compute_timeline_should_update(
                force_timeline || just_enabled,
                track_changed,
                status_changed,
                is_playing,
                pos_s,
                dur_s,
                p.last_tl_pos,
                p.last_tl_dur,
                elapsed_sec,
            ) {
                if let Ok(tl) = SystemMediaTransportControlsTimelineProperties::new() {
                    let zero = TimeSpan { Duration: 0 };
                    let end_ts = TimeSpan {
                        Duration: (dur_s * (TICKS_PER_SEC as f64)).round() as i64,
                    };
                    let pos_ts = TimeSpan {
                        Duration: (pos_s * (TICKS_PER_SEC as f64)).round() as i64,
                    };
                    let _ = tl.SetStartTime(zero);
                    let _ = tl.SetMinSeekTime(zero);
                    let _ = tl.SetEndTime(end_ts);
                    let _ = tl.SetMaxSeekTime(end_ts);
                    let _ = tl.SetPosition(pos_ts);
                    let _ = p.controls.UpdateTimelineProperties(&tl);
                    p.last_tl_pos = pos_s;
                    p.last_tl_dur = dur_s;
                    p.last_tl_at = Some(Instant::now());
                }
            }

            p.last_thumb_key != payload.track_key
        })
    }

    /// Uploads album artwork bytes into Windows SMTC strictly on the STA UI thread via an
    /// in-memory WinRT stream (sub-millisecond RAM copy; zero cross-apartment COM marshaling!).
    pub fn set_publisher_thumbnail_bytes(track_key: &str, bytes: &[u8]) -> bool {
        if bytes.len() < 16 || bytes.len() > 8 * 1024 * 1024 {
            return false;
        }
        SMTC_PUBLISHER.with(|cell| {
            let Ok(mut guard) = cell.try_borrow_mut() else {
                return false;
            };
            let Some(p) = guard.as_mut() else {
                return false;
            };
            if !p.enabled || p.last_track_key != track_key || p.last_thumb_key == track_key {
                return false;
            }
            let Ok(stream) = InMemoryRandomAccessStream::new() else {
                return false;
            };
            let Ok(writer) = DataWriter::CreateDataWriter(&stream) else {
                return false;
            };
            if writer.WriteBytes(bytes).is_err() {
                let _ = writer.DetachStream();
                return false;
            }
            let store_ok = writer
                .StoreAsync()
                .ok()
                .and_then(|op| op.join().ok())
                .is_some();
            let flush_ok = writer
                .FlushAsync()
                .ok()
                .and_then(|op| op.join().ok())
                .is_some();
            // Detach before dropping `DataWriter` so `InMemoryRandomAccessStream` stays open for SMTC!
            let _ = writer.DetachStream();
            if !store_ok || !flush_ok || stream.Seek(0).is_err() {
                return false;
            }
            let Ok(stream_ref) = RandomAccessStreamReference::CreateFromStream(&stream) else {
                return false;
            };
            if let Ok(updater) = p.controls.DisplayUpdater() {
                let _ = updater.SetThumbnail(&stream_ref);
                let _ = updater.Update();
                p.last_thumb_key = track_key.to_string();
                return true;
            }
            false
        })
    }

    /// Disables AM-Light's SMTC provider and clears the Windows media card (e.g. ExternalSmtc or toggle OFF).
    pub fn disable_publisher() {
        SMTC_PUB_ENABLED.store(false, Ordering::Release);
        SMTC_PUBLISHER.with(|cell| {
            if let Ok(mut guard) = cell.try_borrow_mut() {
                if let Some(p) = guard.as_mut() {
                    if p.enabled {
                        let _ = p.controls.SetPlaybackStatus(MediaPlaybackStatus::Stopped);
                        if let Ok(updater) = p.controls.DisplayUpdater() {
                            let _ = updater.ClearAll();
                            let _ = updater.Update();
                        }
                        let _ = p.controls.SetIsEnabled(false);
                    }
                    p.enabled = false;
                    p.last_status.clear();
                    p.last_meta_key.clear();
                    p.last_track_key.clear();
                    p.last_thumb_key.clear();
                    p.last_tl_at = None;
                }
            }
        });
        if let Ok(mut q) = cmd_queue_mtx().lock() {
            q.clear();
        }
    }

    /// Tears down the SMTC provider and unregisters WinRT event handlers cleanly on app exit.
    pub fn shutdown_publisher() {
        disable_publisher();
        let taken = SMTC_PUBLISHER.with(|cell| cell.try_borrow_mut().ok().and_then(|mut g| g.take()));
        if let Some(p) = taken {
            if let Some(tok) = p.btn_token {
                let _ = p.controls.RemoveButtonPressed(tok);
            }
            if let Some(tok) = p.pos_token {
                let _ = p.controls.RemovePlaybackPositionChangeRequested(tok);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod win {
    use super::*;

    pub fn get_smtc_state() -> SmtcState {
        SmtcState {
            status: "Stopped".to_string(),
            sampled_at: Some(Instant::now()),
            ..SmtcState::default()
        }
    }

    pub fn control_playback(_action: &str, _seek_sec: Option<f64>) -> Result<(), String> {
        Err("仅支持 Windows 平台".to_string())
    }

    pub fn minimize_official_am() -> Result<u32, String> {
        Ok(0)
    }

    pub fn enqueue_published_command(_cmd: SmtcPublishedCommand) {}
    pub fn drain_published_commands() -> Vec<SmtcPublishedCommand> {
        Vec::new()
    }
    pub fn init_publisher(_hwnd_usize: usize) -> bool {
        false
    }
    pub fn sync_publisher_state(_payload: &SmtcPublishPayload, _force_timeline: bool) -> bool {
        false
    }
    pub fn set_publisher_thumbnail_bytes(_track_key: &str, _bytes: &[u8]) -> bool {
        false
    }
    pub fn disable_publisher() {
        SMTC_PUB_ENABLED.store(false, Ordering::Release);
    }
    pub fn shutdown_publisher() {
        SMTC_PUB_ENABLED.store(false, Ordering::Release);
    }
}

pub use win::{
    control_playback, disable_publisher, drain_published_commands, enqueue_published_command,
    get_smtc_state, init_publisher, minimize_official_am, set_publisher_thumbnail_bytes,
    shutdown_publisher, sync_publisher_state,
};
