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

#[inline]
pub fn smtc_event_seq() -> u64 {
    SMTC_EVENT_SEQ.load(Ordering::Acquire)
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use windows::core::{Interface, BOOL, PWSTR};
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus, MediaPropertiesChangedEventArgs,
        PlaybackInfoChangedEventArgs, SessionsChangedEventArgs, TimelinePropertiesChangedEventArgs,
    };
    use windows::Storage::Streams::DataReader;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible, ShowWindow, SW_MINIMIZE,
    };

    const TICKS_PER_SEC: i64 = 10_000_000;
    /// Sub-second WinRT/SMTC event IPC lag compensation (`LyricsBlossom` `0x40210c`: `now - 295ms`).
    const ROLLOVER_IPC_COMP_SEC: f64 = 0.295;

    fn ensure_com() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = crate::am_agent::init_process_com_security();
        }
    }

    #[inline]
    fn is_apple_music_aumid(app_id_lower: &str) -> bool {
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
    static SMTC_THUMB_CACHE: OnceLock<Mutex<(String, Option<String>)>> = OnceLock::new();

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
                let old_s = tr.bound_session.take()?;
                Some((
                    old_s,
                    tr.tl_token.take(),
                    tr.pb_token.take(),
                    tr.mp_token.take(),
                ))
            });
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
                tr.prev_raw_ticks = -1;
                tr.has_cal = false;
                tr.cal_trusted = false;
                tr.cal_anchor_inst = None;
                tr.paused_pos_sec = None;
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

        // Refresh MediaProperties immediately on `MediaPropertiesChanged`, empty cache, or 2.0s watchdog (`#7` `0x404ca5`)
        let need_media_fetch = SMTC_MEDIA_DIRTY.swap(false, Ordering::AcqRel)
            || tracker_mtx()
                .lock()
                .map(|tr| {
                    tr.cached_title.is_empty()
                        || tr
                            .last_media_fetch
                            .is_none_or(|t0| t0.elapsed() >= Duration::from_millis(2000))
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
                    let cache_mtx = SMTC_THUMB_CACHE
                        .get_or_init(|| Mutex::new((String::new(), None)));
                    let thumb = if !t.is_empty() {
                        let cached_hit = cache_mtx.lock().ok().and_then(|g| {
                            if g.0 == thumb_key {
                                Some(g.1.clone())
                            } else {
                                None
                            }
                        });
                        match cached_hit {
                            Some(existing) => existing,
                            None => {
                                let fresh = read_thumbnail_b64(&props);
                                if let Ok(mut g) = cache_mtx.lock() {
                                    *g = (thumb_key, fresh.clone());
                                }
                                fresh
                            }
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
}

pub use win::{control_playback, get_smtc_state, minimize_official_am};
