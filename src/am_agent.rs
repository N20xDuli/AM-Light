use crate::bplist::{decode_bplist_hex, empty_dict_hex, encode_bplist_hex, PlistValue};
use crate::smtc::SmtcState;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

static PLAY_CMD_EPOCH: AtomicU64 = AtomicU64::new(1);
static AGENT_STATE_SEQ: AtomicU64 = AtomicU64::new(0);
static PLAY_CMD_IN_FLIGHT_COUNT: AtomicU64 = AtomicU64::new(0);
static PROCESS_BOOT_INSTANT: Lazy<Instant> = Lazy::new(Instant::now);
static LAST_PLAY_INTENT_MS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn mono_ms_now() -> u64 {
    PROCESS_BOOT_INSTANT.elapsed().as_millis() as u64
}

#[inline]
pub fn current_play_cmd_epoch() -> u64 {
    PLAY_CMD_EPOCH.load(Ordering::Acquire)
}

#[inline]
pub fn resolved_play_cmd_epoch() -> u64 {
    SHARED.read().map(|s| s.resolved_epoch).unwrap_or(0)
}

/// Arms optimistic play-start state on the UI thread before `spawn_blocking(play_tracks)` runs,
/// immediately invalidating `resolved_epoch` for the previous song and signaling cold-start lyrics fetch
/// to wait briefly for `AMPLibraryAgent` connection/descriptor. Returns the upcoming `cmd_epoch`
/// (`current_play_cmd_epoch() + 1`).
pub fn prepare_optimistic_play_epoch() -> u64 {
    LAST_PLAY_INTENT_MS.store(mono_ms_now(), Ordering::Release);
    if let Ok(mut snap) = SHARED.write() {
        snap.resolved_epoch = 0;
    }
    PLAY_CMD_EPOCH.load(Ordering::Acquire) + 1
}

#[inline]
pub fn state_seq() -> u64 {
    AGENT_STATE_SEQ.load(Ordering::Acquire)
}

struct InFlightPlayGuard;
impl Drop for InFlightPlayGuard {
    fn drop(&mut self) {
        PLAY_CMD_IN_FLIGHT_COUNT.fetch_sub(1, Ordering::AcqRel);
    }
}
fn enter_play_cmd_in_flight() -> InFlightPlayGuard {
    LAST_PLAY_INTENT_MS.store(mono_ms_now(), Ordering::Release);
    PLAY_CMD_IN_FLIGHT_COUNT.fetch_add(1, Ordering::AcqRel);
    InFlightPlayGuard
}

static ACTIVE_STOREFRONT: Lazy<RwLock<String>> = Lazy::new(|| RwLock::new("cn".to_string()));

pub fn set_active_storefront(sf: &str) {
    let clean = sf.trim().to_ascii_lowercase();
    if !clean.is_empty() && clean.chars().all(|c| c.is_ascii_alphanumeric()) {
        if let Ok(mut g) = ACTIVE_STOREFRONT.write() {
            *g = clean;
        }
    }
}

pub fn get_active_storefront() -> String {
    ACTIVE_STOREFRONT
        .read()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "cn".to_string())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTrackInput {
    pub id: String,
    #[serde(default)]
    pub catalog_id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub album_name: String,
    #[serde(default)]
    pub duration_sec: f64,
    #[serde(default)]
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub audio_traits: Vec<String>,
}

impl AgentTrackInput {
    pub fn effective_adam_id(&self) -> String {
        if let Some(cid) = &self.catalog_id {
            let trimmed = cid.trim();
            if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
                return trimmed.to_string();
            }
        }
        self.id.trim().to_string()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub connected: bool,
    pub agent_pid: Option<u32>,
    pub apple_music_ui_running: bool,
    pub client_id: u32,
    pub state_code: i32,
    pub volume: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPrefs {
    pub lossless_enabled: bool,
    pub streaming_audio_quality: i64,
    pub download_audio_quality: i64,
    pub dolby_atmos_setting: i64,
    pub download_multichannel_audio: bool,
    pub sound_check_enabled: bool,
    pub crossfade_songs_enabled: bool,
    pub crossfade_songs_seconds: i64,
    pub library_location_url: Option<String>,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            lossless_enabled: true,
            streaming_audio_quality: 20,
            download_audio_quality: 20,
            dolby_atmos_setting: 10,
            download_multichannel_audio: false,
            sound_check_enabled: false,
            crossfade_songs_enabled: false,
            crossfade_songs_seconds: 6,
            library_location_url: None,
        }
    }
}

impl AudioPrefs {
    pub fn sanitized(&self) -> Self {
        let norm_tier = |v: i64| -> i64 {
            if v >= 20 {
                20
            } else if v >= 15 {
                15
            } else {
                10
            }
        };
        let norm_atmos = match self.dolby_atmos_setting {
            20 => 20,
            30 => 30,
            _ => 10,
        };
        Self {
            lossless_enabled: self.lossless_enabled,
            streaming_audio_quality: norm_tier(self.streaming_audio_quality),
            download_audio_quality: norm_tier(self.download_audio_quality),
            dolby_atmos_setting: norm_atmos,
            download_multichannel_audio: self.download_multichannel_audio,
            sound_check_enabled: self.sound_check_enabled,
            crossfade_songs_enabled: self.crossfade_songs_enabled,
            crossfade_songs_seconds: self.crossfade_songs_seconds.clamp(1, 12),
            library_location_url: self.library_location_url.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CachedTrackMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_sec: f64,
    pub artwork_url: Option<String>,
    pub audio_quality: Option<String>,
}

pub fn format_audio_traits_badge(traits: &[String]) -> Option<String> {
    let has_hires = traits.iter().any(|t| t == "hi-res-lossless");
    let has_lossless = traits.iter().any(|t| t == "lossless" || t == "hi-res-lossless");
    let has_atmos = traits.iter().any(|t| t == "atmos" || t == "spatial");

    match (has_hires, has_lossless, has_atmos) {
        (true, _, true) => Some("Hi-Res 无损 · 杜比全景声".to_string()),
        (true, _, false) => Some("Hi-Res 无损 ALAC".to_string()),
        (false, true, true) => Some("无损 ALAC · 杜比全景声".to_string()),
        (false, false, true) => Some("杜比全景声".to_string()),
        (false, true, false) => Some("无损 ALAC".to_string()),
        _ => None,
    }
}

pub fn format_artwork_template(template: &str, size: u32) -> String {
    let s = size.to_string();
    template
        .replace("{w}", &s)
        .replace("{h}", &s)
        .replace("{c}", "bb")
        .replace("{f}", "jpg")
}

pub fn parse_track_from_am_json(
    item: &Value,
    fallback_album: Option<&str>,
    fallback_artwork: Option<&str>,
) -> Option<AgentTrackInput> {
    let attrs = item.get("attributes")?;
    let raw_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
    let play_params = attrs.get("playParams");
    let catalog_id = play_params
        .and_then(|p| p.get("catalogId"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            item.pointer("/relationships/catalog/data/0/id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .or_else(|| {
            let pid = play_params.and_then(|p| p.get("id")).and_then(Value::as_str)?;
            if pid.chars().all(|c| c.is_ascii_digit()) {
                Some(pid.to_string())
            } else {
                None
            }
        });

    let effective_id = catalog_id
        .clone()
        .or_else(|| {
            play_params
                .and_then(|p| p.get("id"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| raw_id.to_string());

    if effective_id.is_empty() {
        return None;
    }

    let name = attrs
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("未知曲目")
        .to_string();
    let artist_name = attrs
        .get("artistName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let album_name = attrs
        .get("albumName")
        .and_then(Value::as_str)
        .or(fallback_album)
        .unwrap_or_default()
        .to_string();
    let duration_sec = attrs
        .get("durationInMillis")
        .and_then(Value::as_f64)
        .map(|ms| ms / 1000.0)
        .unwrap_or(0.0);

    let artwork_url = attrs
        .pointer("/artwork/url")
        .and_then(Value::as_str)
        .map(|u| format_artwork_template(u, 600))
        .or_else(|| {
            item.pointer("/relationships/catalog/data/0/attributes/artwork/url")
                .and_then(Value::as_str)
                .map(|u| format_artwork_template(u, 600))
        })
        .or_else(|| fallback_artwork.map(|s| s.to_string()));

    let mut audio_traits: Vec<String> = attrs
        .get("audioTraits")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    if audio_traits.is_empty() {
        if let Some(cat_traits) =
            item.pointer("/relationships/catalog/data/0/attributes/audioTraits")
                .and_then(Value::as_array)
        {
            audio_traits = cat_traits
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect();
        }
    }

    Some(AgentTrackInput {
        id: effective_id,
        catalog_id,
        kind: Some("song".to_string()),
        name,
        artist_name,
        album_name,
        duration_sec,
        artwork_url,
        audio_traits,
    })
}

#[derive(Clone, Debug)]
struct SharedSnapshot {
    connected: bool,
    agent_pid: Option<u32>,
    client_id: u32,
    state_code: i32,
    active: bool,
    status: String,
    adam_id: Option<String>,
    requested_track_id: Option<String>,
    pqi_id: Option<u64>,
    transition_from_pqi: Option<u64>,
    cmd_in_flight: bool,
    pending_play_tracks: bool,
    cmd_epoch: u64,
    resolved_epoch: u64,
    desired_playing: bool,
    audio_onset_watch_epoch: u64,
    play_btn_shows_pause: bool,
    seeking_in_flight: u32,
    current_descriptor: Option<PlistValue>,
    title: String,
    artist: String,
    album: String,
    duration_sec: f64,
    anchor_pos_sec: f64,
    anchor_instant: Option<Instant>,
    track_started_at: Option<Instant>,
    awaiting_audio_onset: bool,
    scrubber_indeterminate: bool,
    can_be_positioned: bool,
    pending_reload_seek: Option<f64>,
    optimistic_seek_target: Option<(f64, Instant)>,
    artwork_url: Option<String>,
    audio_quality: Option<String>,
    live_agent_sample_rate: Option<u32>,
    live_agent_bit_depth: Option<u16>,
    live_agent_format: Option<String>,
    volume: f64,
    prefs: AudioPrefs,
    stream_specs: Option<crate::audio_viz::StreamAudioSpecs>,
    track_cache: HashMap<String, CachedTrackMeta>,
    track_traits_cache: HashMap<String, Vec<String>>,
    hls_cache: HashMap<String, String>,
    agent_lyrics_cache: HashMap<String, (String, bool)>,
    queue_tracks: Vec<AgentTrackInput>,
}

impl Default for SharedSnapshot {
    fn default() -> Self {
        Self {
            connected: false,
            agent_pid: None,
            client_id: 0,
            state_code: 0,
            active: false,
            status: "Stopped".to_string(),
            adam_id: None,
            requested_track_id: None,
            pqi_id: None,
            transition_from_pqi: None,
            cmd_in_flight: false,
            pending_play_tracks: false,
            cmd_epoch: 0,
            resolved_epoch: 0,
            desired_playing: false,
            audio_onset_watch_epoch: 0,
            play_btn_shows_pause: false,
            seeking_in_flight: 0,
            current_descriptor: None,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            duration_sec: 0.0,
            anchor_pos_sec: 0.0,
            anchor_instant: None,
            track_started_at: None,
            awaiting_audio_onset: false,
            scrubber_indeterminate: false,
            can_be_positioned: false,
            pending_reload_seek: None,
            optimistic_seek_target: None,
            artwork_url: None,
            audio_quality: None,
            live_agent_sample_rate: None,
            live_agent_bit_depth: None,
            live_agent_format: None,
            volume: 0.75,
            prefs: AudioPrefs::default(),
            stream_specs: None,
            track_cache: HashMap::new(),
            track_traits_cache: HashMap::new(),
            hls_cache: HashMap::new(),
            agent_lyrics_cache: HashMap::new(),
            queue_tracks: Vec::new(),
        }
    }
}

fn format_specs_badge(sp: &crate::audio_viz::StreamAudioSpecs) -> String {
    let khz = (sp.sample_rate_hz as f64) / 1000.0;
    let sr_str = if (khz.fract()).abs() < 0.01 {
        format!("{:.0}kHz", khz)
    } else {
        format!("{:.1}kHz", khz)
    };
    if sp.codec.eq_ignore_ascii_case("ALAC") {
        let bd = sp.bit_depth.unwrap_or(24);
        if sp.hires_catalog_trait && sp.sample_rate_hz <= 48_000 {
            format!("Hi-Res ALAC {bd}-bit/最高192kHz")
        } else if sp.available_variants.is_empty() {
            format!("ALAC {bd}-bit / {sr_str}")
        } else {
            format!("ALAC {bd}-bit/{sr_str} • {} kbps", sp.bitrate_kbps)
        }
    } else if sp.tier_label.contains("全景声") {
        format!("Dolby Atmos • {} kbps", sp.bitrate_kbps)
    } else {
        format!("{} {sr_str} • {} kbps", sp.codec_label, sp.bitrate_kbps)
    }
}

impl SharedSnapshot {
    fn current_position_sec(&self) -> f64 {
        let mut pos = self.anchor_pos_sec;
        // Never advance playhead while FairPlay / HLS ALAC stream is still buffering (`awaiting_audio_onset`
        // or `scrubber_indeterminate`). When onset locks, `anchor_instant` is reset cleanly to `Instant::now()`
        // so there is ZERO teleport jump regardless of whether buffering took 0.3s or 4.5s!
        if self.status == "Playing" && !self.awaiting_audio_onset && !self.scrubber_indeterminate {
            if let Some(t0) = self.anchor_instant {
                pos += t0.elapsed().as_secs_f64();
            }
        }
        if self.duration_sec > 0.0 && pos > self.duration_sec {
            pos = self.duration_sec;
        }
        pos.max(0.0)
    }

    fn cache_lyrics_bounded(&mut self, aid: String, lyrics_body: String, is_ttml: bool) {
        if aid.is_empty() || lyrics_body.trim().is_empty() {
            return;
        }
        if self.agent_lyrics_cache.len() >= 24 {
            self.agent_lyrics_cache.clear();
        }
        self.agent_lyrics_cache.insert(aid, (lyrics_body, is_ttml));
    }

    fn recompute_current_stream_specs(&mut self) {
        let aid_opt = self.adam_id.as_deref();
        let base_specs = aid_opt
            .and_then(|aid| self.hls_cache.get(aid))
            .and_then(|m3u8| {
                let spatial_ready = crate::audio_viz::is_default_endpoint_spatial_ready();
                crate::audio_viz::parse_enhanced_hls_variants_with_endpoint(
                    m3u8,
                    self.prefs.lossless_enabled,
                    self.prefs.streaming_audio_quality,
                    self.prefs.dolby_atmos_setting,
                    spatial_ready,
                )
            });
        let has_hires_trait = aid_opt
            .and_then(|aid| self.track_traits_cache.get(aid))
            .is_some_and(|tr| tr.iter().any(|t| t == "hi-res-lossless"))
            || self
                .audio_quality
                .as_deref()
                .is_some_and(|q| q.contains("Hi-Res"));
        self.stream_specs = crate::audio_viz::enrich_stream_specs_with_live_and_traits(
            base_specs,
            self.live_agent_sample_rate,
            self.live_agent_bit_depth,
            self.live_agent_format.as_deref(),
            has_hires_trait,
            self.prefs.lossless_enabled,
            self.prefs.streaming_audio_quality,
            self.prefs.dolby_atmos_setting,
        );
    }

    fn to_smtc_state(&self) -> SmtcState {
        let dynamic_badge = self
            .stream_specs
            .as_ref()
            .map(format_specs_badge)
            .or_else(|| self.audio_quality.clone())
            .or_else(|| self.active.then(|| "Apple Music 音频流".to_string()));

        SmtcState {
            active: self.active,
            app_id: "AMPLibraryAgent".to_string(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            status: self.status.clone(),
            position_sec: self.current_position_sec(),
            duration_sec: self.duration_sec,
            thumbnail_base64: None,
            artwork_url: self.artwork_url.clone(),
            backend_mode: if self.active || self.connected {
                "agent".to_string()
            } else {
                String::new()
            },
            audio_quality: dynamic_badge,
            volume: Some(self.volume),
            agent_pid: self.agent_pid,
            adam_id: self.adam_id.clone(),
            stream_specs: self.stream_specs.clone(),
            sampled_at: Some(Instant::now()),
            timeline_updated_univ: 0,
            position_is_calibrated: !self.awaiting_audio_onset && !self.scrubber_indeterminate,
        }
    }
}

static SHARED: Lazy<RwLock<SharedSnapshot>> = Lazy::new(|| RwLock::new(SharedSnapshot::default()));

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::fs;
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicI32, AtomicPtr};
    use std::sync::Once;

    const AMP_PROXY_DLL_BYTES: &[u8] = include_bytes!("../c_proxy/amp_proxy.dll");

    type HRESULT = i32;
    type ULONG = u32;
    type HSTRING = *mut c_void;
    type HANDLE = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct GUID {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const IID_IUNKNOWN: GUID = GUID {
        data1: 0x0000_0000,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };
    const IID_IASYNC_INFO: GUID = GUID {
        data1: 0x0000_0036,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };
    const CLSID_AMP_MUSIC_LIBRARY: GUID = GUID {
        data1: 0x68E7_097C,
        data2: 0xF969,
        data3: 0x4006,
        data4: [0xAA, 0xC3, 0x95, 0x11, 0x5F, 0x0E, 0xD1, 0xC4],
    };
    const IID_IAMP_LIBRARY: GUID = GUID {
        data1: 0xF707_A913,
        data2: 0xE0CE,
        data3: 0x4FD4,
        data4: [0xBC, 0xE3, 0x42, 0x5D, 0xD1, 0x53, 0x28, 0x5B],
    };
    const IID_IPLAYBACK_EVENT_DELEGATE: GUID = GUID {
        data1: 0xF0E2_09BE,
        data2: 0x7FB1,
        data3: 0x5E1E,
        data4: [0x98, 0x79, 0xB6, 0x22, 0xAE, 0x7C, 0xCA, 0x41],
    };
    const IID_IPLAYBACK_EVENT_CB_DELEGATE: GUID = GUID {
        data1: 0xC153_DB3C,
        data2: 0xA412,
        data3: 0x5A70,
        data4: [0xA8, 0xF9, 0x55, 0xEF, 0x31, 0x87, 0x8E, 0xE3],
    };

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct EventRegistrationToken {
        value: i64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RegisterClientResult {
        client_id: u32,
        persistent_machine_id: u64,
        status: i32,
    }

    #[repr(C)]
    struct IAMPLibrary {
        lp_vtbl: *const IAMPLibraryVtbl,
    }

    #[repr(C)]
    struct IAMPLibraryVtbl {
        // IUnknown
        query_interface: unsafe extern "system" fn(*mut IAMPLibrary, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut IAMPLibrary) -> ULONG,
        release: unsafe extern "system" fn(*mut IAMPLibrary) -> ULONG,
        // IInspectable
        get_iids: unsafe extern "system" fn(*mut IAMPLibrary, *mut ULONG, *mut *mut GUID) -> HRESULT,
        get_runtime_class_name: unsafe extern "system" fn(*mut IAMPLibrary, *mut HSTRING) -> HRESULT,
        get_trust_level: unsafe extern "system" fn(*mut IAMPLibrary, *mut i32) -> HRESULT,
        // IAMPLibrary
        get_version: unsafe extern "system" fn(*mut IAMPLibrary, *mut HSTRING) -> HRESULT,
        get_server_info: unsafe extern "system" fn(*mut IAMPLibrary, *mut HSTRING) -> HRESULT,
        get_state: unsafe extern "system" fn(*mut IAMPLibrary, *mut i32) -> HRESULT,
        setup: unsafe extern "system" fn(*mut IAMPLibrary, u32, *mut u32, *mut i32) -> HRESULT,
        cleanup: unsafe extern "system" fn(*mut IAMPLibrary, u32, *mut i32) -> HRESULT,
        media_app_starting: unsafe extern "system" fn(*mut IAMPLibrary, i32, *mut HSTRING) -> HRESULT,
        media_app_quitting: unsafe extern "system" fn(*mut IAMPLibrary, *mut i32) -> HRESULT,
        media_app_get_library_info_async: unsafe extern "system" fn(*mut IAMPLibrary, *mut *mut c_void) -> HRESULT,
        media_app_open_library_async: unsafe extern "system" fn(*mut IAMPLibrary, *mut *mut c_void) -> HRESULT,
        media_app_set_library_location_async: unsafe extern "system" fn(*mut IAMPLibrary, HSTRING, *mut *mut c_void) -> HRESULT,
        media_app_exec_fetch_request_async: unsafe extern "system" fn(*mut IAMPLibrary, HSTRING, *mut *mut c_void) -> HRESULT,
        media_app_release_fetch_request_write_stream: unsafe extern "system" fn(*mut IAMPLibrary, u64, u64, *mut i32) -> HRESULT,
        media_app_close_fetch_request: unsafe extern "system" fn(*mut IAMPLibrary, u64, u64, *mut i32) -> HRESULT,
        media_app_exec_agent_command_async: unsafe extern "system" fn(*mut IAMPLibrary, HSTRING, *mut *mut c_void) -> HRESULT,
        get_client_id: unsafe extern "system" fn(*mut IAMPLibrary, *mut u32) -> HRESULT,
        get_media_domains_opened: unsafe extern "system" fn(*mut IAMPLibrary, *mut u8) -> HRESULT,
        get_server_protocol_version: unsafe extern "system" fn(*mut IAMPLibrary, *mut HSTRING) -> HRESULT,
        register_library_client: unsafe extern "system" fn(*mut IAMPLibrary, HSTRING, u32, *mut RegisterClientResult) -> HRESULT,
        get_domain_info_async: unsafe extern "system" fn(*mut IAMPLibrary, u32, u32, HSTRING, *mut *mut c_void) -> HRESULT,
        open_domains_async: unsafe extern "system" fn(*mut IAMPLibrary, u32, u32, HSTRING, *mut *mut c_void) -> HRESULT,
        close_domains_for_client_id: unsafe extern "system" fn(*mut IAMPLibrary, u32, *mut i32) -> HRESULT,
        send_db_changes_to_library_async: unsafe extern "system" fn(*mut IAMPLibrary, u32, HSTRING, *mut *mut c_void) -> HRESULT,
        fetch_library_from_revision_async: unsafe extern "system" fn(*mut IAMPLibrary, u32, u32, u8, *mut *mut c_void) -> HRESULT,
        save_agent_log_to_file_async: unsafe extern "system" fn(*mut IAMPLibrary, HSTRING, HSTRING, *mut *mut c_void) -> HRESULT,
        add_playback_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_playback_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        add_playback_event_with_callback_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_playback_event_with_callback_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        playback_event_response: unsafe extern "system" fn(*mut IAMPLibrary, u32, HSTRING) -> HRESULT,
        add_fetch_invalidation_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_fetch_invalidation_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        add_activity_state_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_activity_state_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        add_cloud_library_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_cloud_library_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        add_library_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_library_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        add_store_services_ui_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_store_services_ui_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        store_services_ui_event_response: unsafe extern "system" fn(*mut IAMPLibrary, u32, HSTRING) -> HRESULT,
        add_busy_changed_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_busy_changed_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        add_library_revision_changed_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_library_revision_changed_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        add_device_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, *mut c_void, *mut EventRegistrationToken) -> HRESULT,
        remove_device_event_handler: unsafe extern "system" fn(*mut IAMPLibrary, EventRegistrationToken) -> HRESULT,
        kill: unsafe extern "system" fn(*mut IAMPLibrary) -> HRESULT,
    }

    #[repr(C)]
    struct PROCESSENTRY32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_coinit: u32) -> HRESULT;
        fn CoInitializeSecurity(
            p_sec_desc: *mut c_void,
            c_auth_svc: i32,
            as_auth_svc: *mut c_void,
            p_reserved1: *mut c_void,
            dw_authn_level: u32,
            dw_imp_level: u32,
            p_auth_list: *mut c_void,
            dw_capabilities: u32,
            p_reserved3: *mut c_void,
        ) -> HRESULT;
        fn CoCreateInstance(
            rclsid: *const GUID,
            p_unk_outer: *mut c_void,
            dw_cls_context: u32,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> HRESULT;
    }

    #[link(name = "runtimeobject")]
    extern "system" {
        fn WindowsCreateString(source_string: *const u16, length: u32, string: *mut HSTRING) -> HRESULT;
        fn WindowsDeleteString(string: HSTRING) -> HRESULT;
        fn WindowsGetStringRawBuffer(string: HSTRING, length: *mut u32) -> *const u16;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
        fn CloseHandle(h_object: HANDLE) -> i32;
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> HANDLE;
        fn Process32FirstW(h_snapshot: HANDLE, lppe: *mut PROCESSENTRY32W) -> i32;
        fn Process32NextW(h_snapshot: HANDLE, lppe: *mut PROCESSENTRY32W) -> i32;
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> HANDLE;
        fn TerminateProcess(h_process: HANDLE, u_exit_code: u32) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(lp_enum_func: unsafe extern "system" fn(*mut c_void, isize) -> i32, l_param: isize) -> i32;
        fn GetWindowThreadProcessId(h_wnd: *mut c_void, lpdw_process_id: *mut u32) -> u32;
        fn GetClassNameW(h_wnd: *mut c_void, lp_class_name: *mut u16, n_max_count: i32) -> i32;
        fn PostMessageW(h_wnd: *mut c_void, msg: u32, w_param: usize, l_param: isize) -> i32;
        fn IsWindowVisible(h_wnd: *mut c_void) -> i32;
        fn ShowWindow(h_wnd: *mut c_void, n_cmd_show: i32) -> i32;
        fn SetForegroundWindow(h_wnd: *mut c_void) -> i32;
        fn MessageBoxW(
            h_wnd: *mut c_void,
            lp_text: *const u16,
            lp_caption: *const u16,
            u_type: u32,
        ) -> i32;
    }

    // COM delegate singletons for PlaybackEventDelegate & PlaybackEventWithCallbackDelegate (1:1 from proven am-light)
    #[repr(C)]
    struct PlaybackDelegateVtbl {
        query_interface: unsafe extern "system" fn(*mut PlaybackDelegateObj, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut PlaybackDelegateObj) -> ULONG,
        release: unsafe extern "system" fn(*mut PlaybackDelegateObj) -> ULONG,
        invoke: unsafe extern "system" fn(*mut PlaybackDelegateObj, u32, HSTRING) -> HRESULT,
    }

    #[repr(C)]
    struct PlaybackDelegateObj {
        lp_vtbl: *const PlaybackDelegateVtbl,
        ref_count: AtomicI32,
    }

    unsafe impl Sync for PlaybackDelegateObj {}

    unsafe extern "system" fn pb_qi(
        this: *mut PlaybackDelegateObj,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if ppv.is_null() || riid.is_null() {
            return -2147467261i32; // E_POINTER
        }
        let iid = *riid;
        // Reject IID_IAgileObject so AMPLibraryAgent's `make_agile_delegate` wraps our out-of-proc proxy
        // via `RoGetAgileReference`, enabling cross-apartment STA -> MTA invocation instead of `RPC_E_WRONG_THREAD`!
        if iid == IID_IUNKNOWN || iid == IID_IPLAYBACK_EVENT_DELEGATE {
            *ppv = this as *mut c_void;
            (*this).ref_count.fetch_add(1, Ordering::Relaxed);
            0
        } else {
            *ppv = null_mut();
            -2147467262i32 // E_NOINTERFACE
        }
    }

    unsafe extern "system" fn pb_add_ref(this: *mut PlaybackDelegateObj) -> ULONG {
        ((*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1) as ULONG
    }

    unsafe extern "system" fn pb_release(this: *mut PlaybackDelegateObj) -> ULONG {
        ((*this).ref_count.fetch_sub(1, Ordering::Relaxed) - 1).max(1) as ULONG
    }

    static PB_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
    static LAST_PB_REG_HRS: AtomicU64 = AtomicU64::new(0);
    static LAST_PB_REG_TOKS: AtomicU64 = AtomicU64::new(0);

    unsafe extern "system" fn pb_invoke(
        _this: *mut PlaybackDelegateObj,
        _message: u32,
        params_dict: HSTRING,
    ) -> HRESULT {
        PB_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
        let hex = hstring_to_rust(params_dict);
        if let Ok(plist) = decode_bplist_hex(&hex) {
            handle_incoming_playback_event(&plist);
        }
        0
    }

    static PB_VTBL: PlaybackDelegateVtbl = PlaybackDelegateVtbl {
        query_interface: pb_qi,
        add_ref: pb_add_ref,
        release: pb_release,
        invoke: pb_invoke,
    };

    static mut PB_DELEGATE: PlaybackDelegateObj = PlaybackDelegateObj {
        lp_vtbl: &PB_VTBL,
        ref_count: AtomicI32::new(1),
    };

    #[repr(C)]
    struct PlaybackCbDelegateVtbl {
        query_interface: unsafe extern "system" fn(*mut PlaybackCbDelegateObj, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut PlaybackCbDelegateObj) -> ULONG,
        release: unsafe extern "system" fn(*mut PlaybackCbDelegateObj) -> ULONG,
        invoke: unsafe extern "system" fn(*mut PlaybackCbDelegateObj, u32, u32, HSTRING) -> HRESULT,
    }

    #[repr(C)]
    struct PlaybackCbDelegateObj {
        lp_vtbl: *const PlaybackCbDelegateVtbl,
        ref_count: AtomicI32,
    }

    unsafe impl Sync for PlaybackCbDelegateObj {}

    static ACTIVE_LIB_PTR: AtomicPtr<IAMPLibrary> = AtomicPtr::new(null_mut());

    unsafe extern "system" fn pbcb_qi(
        this: *mut PlaybackCbDelegateObj,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if ppv.is_null() || riid.is_null() {
            return -2147467261i32;
        }
        let iid = *riid;
        if iid == IID_IUNKNOWN || iid == IID_IPLAYBACK_EVENT_CB_DELEGATE {
            *ppv = this as *mut c_void;
            (*this).ref_count.fetch_add(1, Ordering::Relaxed);
            0
        } else {
            *ppv = null_mut();
            -2147467262i32
        }
    }

    unsafe extern "system" fn pbcb_add_ref(this: *mut PlaybackCbDelegateObj) -> ULONG {
        ((*this).ref_count.fetch_add(1, Ordering::Relaxed) + 1) as ULONG
    }

    unsafe extern "system" fn pbcb_release(this: *mut PlaybackCbDelegateObj) -> ULONG {
        ((*this).ref_count.fetch_sub(1, Ordering::Relaxed) - 1).max(1) as ULONG
    }

    unsafe extern "system" fn pbcb_invoke(
        _this: *mut PlaybackCbDelegateObj,
        event_id: u32,
        _message: u32,
        params_dict: HSTRING,
    ) -> HRESULT {
        PB_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
        let hex = hstring_to_rust(params_dict);
        if let Ok(plist) = decode_bplist_hex(&hex) {
            handle_incoming_playback_event(&plist);
        }
        let lib = ACTIVE_LIB_PTR.load(Ordering::Acquire);
        if !lib.is_null() {
            if let Ok(h_empty) = rust_to_hstring(&empty_dict_hex()) {
                let _ = ((*(*lib).lp_vtbl).playback_event_response)(lib, event_id, h_empty);
                let _ = WindowsDeleteString(h_empty);
            }
        }
        0
    }

    static PBCB_VTBL: PlaybackCbDelegateVtbl = PlaybackCbDelegateVtbl {
        query_interface: pbcb_qi,
        add_ref: pbcb_add_ref,
        release: pbcb_release,
        invoke: pbcb_invoke,
    };

    static mut PBCB_DELEGATE: PlaybackCbDelegateObj = PlaybackCbDelegateObj {
        lp_vtbl: &PBCB_VTBL,
        ref_count: AtomicI32::new(1),
    };

    fn extract_adam_id_from_desc(desc: Option<&PlistValue>) -> Option<String> {
        let parse_val = |v: &PlistValue| -> Option<String> {
            v.as_str()
                .map(|s| s.trim().to_string())
                .or_else(|| v.as_u64().map(|n| n.to_string()))
                .or_else(|| v.as_i64().filter(|n| *n > 0).map(|n| n.to_string()))
                .filter(|s| !s.is_empty() && s != "0")
        };
        desc.and_then(|d| {
            d.get("identifiers")
                .and_then(|ids| {
                    ids.get("storeAdamID")
                        .or_else(|| ids.get("subscriptionAdamID"))
                        .or_else(|| ids.get("adamID"))
                })
                .and_then(parse_val)
                .or_else(|| {
                    d.get("storeAdamID")
                        .or_else(|| d.get("subscriptionAdamID"))
                        .or_else(|| d.get("adamID"))
                        .and_then(parse_val)
                })
        })
    }

    fn handle_incoming_playback_event(plist: &PlistValue) {
        let root = plist.get("responseDict").unwrap_or(plist);
        let time_d = root.get("time");
        let ctrl_d = root.get("controls");
        let now_d = root.get("nowplaying");
        let media_d = root
            .get("media")
            .or_else(|| now_d.and_then(|n| n.get("media")));

        let cbp_opt = root
            .get("canBePositioned")
            .or_else(|| time_d.and_then(|d| d.get("canBePositioned")))
            .and_then(PlistValue::as_bool);
        let scrubber_opt = root
            .get("scrubberState")
            .or_else(|| time_d.and_then(|d| d.get("scrubberState")))
            .and_then(PlistValue::as_str);
        let cur_time_opt = root
            .get("currentTime")
            .or_else(|| time_d.and_then(|d| d.get("currentTime")))
            .or_else(|| now_d.and_then(|d| d.get("currentTime")))
            .and_then(PlistValue::as_f64);
        let dur_opt = root
            .get("duration")
            .or_else(|| now_d.and_then(|d| d.get("duration")))
            .or_else(|| time_d.and_then(|d| d.get("duration")))
            .and_then(PlistValue::as_f64);

        // Authoritative AMPLibraryAgent controls decoding (`AMP.Services.dll` `HandleUpdatePlayerStateEvent`):
        // - `playButtonState == "pause"` => CanPause=true (player IS actively playing audio!)
        // - `playButtonState == "play"`  => CanPlay=true  (player is paused or still preparing!)
        // - `playButtonState == "stop"`  => player is stopped!
        // Note: `controls.targetRate` is already 1.0 DURING network buffering, so never treat `targetRate`
        // alone as active audio onset while `scrubberState == "indeterminate"` or `!canBePositioned`!
        let explicit_is_playing = root.get("isPlaying").and_then(PlistValue::as_bool);
        let play_btn_state = root
            .get("playButtonState")
            .or_else(|| ctrl_d.and_then(|d| d.get("playButtonState")))
            .and_then(PlistValue::as_str);
        let target_rate_opt = root
            .get("targetRate")
            .or_else(|| ctrl_d.and_then(|d| d.get("targetRate")))
            .and_then(PlistValue::as_f64);

        let np_pqi = now_d.and_then(|d| d.get("pqiID")).and_then(PlistValue::as_u64);
        let np_desc = now_d.and_then(|d| d.get("descriptor")).cloned();
        let np_aid = extract_adam_id_from_desc(np_desc.as_ref())
            .or_else(|| extract_adam_id_from_desc(now_d));
        let np_title = now_d
            .and_then(|d| d.get("line1").or_else(|| d.get("name")))
            .and_then(PlistValue::as_str);
        let np_artist = now_d.and_then(|d| d.get("artist")).and_then(PlistValue::as_str);
        let np_album = now_d.and_then(|d| d.get("album")).and_then(PlistValue::as_str);
        let np_line2 = now_d.and_then(|d| d.get("line2")).and_then(PlistValue::as_str);

        let lyr_state = root
            .get("lyricsState")
            .or_else(|| now_d.and_then(|d| d.get("lyricsState")))
            .and_then(PlistValue::as_str);
        let lyr_body = root
            .get("lyrics")
            .or_else(|| now_d.and_then(|d| d.get("lyrics")))
            .and_then(PlistValue::as_str);

        let sr_opt = root
            .get("audioFormatSampleRate")
            .or_else(|| media_d.and_then(|d| d.get("audioFormatSampleRate")))
            .or_else(|| media_d.and_then(|d| d.get("sampleRate")))
            .and_then(PlistValue::as_u64);
        let bd_opt = root
            .get("audioFormatBitDepth")
            .or_else(|| media_d.and_then(|d| d.get("audioFormatBitDepth")))
            .or_else(|| media_d.and_then(|d| d.get("bitDepth")))
            .and_then(PlistValue::as_u64);
        let fmt_opt = root
            .get("audioFormat")
            .or_else(|| media_d.and_then(|d| d.get("audioFormat")))
            .and_then(PlistValue::as_str);

        let now_inst = Instant::now();
        let mut changed = false;
        if let Ok(mut snap) = SHARED.write() {
            let was_advancing = snap.status == "Playing"
                && !snap.awaiting_audio_onset
                && !snap.scrubber_indeterminate
                && snap.seeking_in_flight == 0
                && snap.anchor_instant.is_some();
            let mut just_switched_pqi = false;

            // 1. Update `nowplaying` item identity (`pqiID` / `descriptor` / `storeAdamID`) while
            // strictly rejecting in-flight callbacks belonging to the outgoing `transition_from_pqi`!
            if let Some(pqi) = np_pqi {
                let is_stale_outgoing = snap.transition_from_pqi == Some(pqi)
                    && (snap.cmd_in_flight
                        || snap.pending_play_tracks
                        || cur_time_opt.is_none_or(|t| t >= 0.5));
                if is_stale_outgoing {
                    return;
                }

                if let Some(desc) = np_desc.clone() {
                    snap.current_descriptor = Some(desc);
                }

                if snap.pending_play_tracks {
                    snap.pqi_id = Some(pqi);
                    snap.transition_from_pqi = None;
                    snap.pending_play_tracks = false;
                    snap.resolved_epoch = snap.cmd_epoch.max(1);
                    if let Some(ref resolved_aid) = np_aid {
                        if let Some(ref cur_aid) = snap.adam_id.clone() {
                            if cur_aid != resolved_aid {
                                if let Some(meta) = snap.track_cache.get(cur_aid).cloned() {
                                    snap.track_cache.entry(resolved_aid.clone()).or_insert(meta);
                                }
                                snap.adam_id = Some(resolved_aid.clone());
                            }
                        } else {
                            snap.adam_id = Some(resolved_aid.clone());
                        }
                    }
                    snap.recompute_current_stream_specs();
                    changed = true;
                } else {
                    let pqi_switched = snap.pqi_id.is_some_and(|old| old != pqi);
                    snap.pqi_id = Some(pqi);
                    if snap.transition_from_pqi == Some(pqi) {
                        snap.transition_from_pqi = None;
                        snap.resolved_epoch = snap.cmd_epoch.max(1);
                    }
                    if pqi_switched {
                        just_switched_pqi = true;
                        snap.transition_from_pqi = None;
                        snap.resolved_epoch = snap
                            .cmd_epoch
                            .max(PLAY_CMD_EPOCH.load(Ordering::Acquire))
                            .max(1);
                        snap.active = true;
                        snap.track_started_at = Some(now_inst);
                        snap.play_btn_shows_pause = false;
                        snap.requested_track_id = None;
                        snap.duration_sec = dur_opt.filter(|&d| d > 0.0).unwrap_or(0.0);
                        snap.artwork_url = None;
                        snap.audio_quality = None;
                        snap.live_agent_sample_rate = None;
                        snap.live_agent_bit_depth = None;
                        snap.live_agent_format = None;
                        if cur_time_opt.is_none_or(|t| t <= 0.02) {
                            snap.anchor_pos_sec = cur_time_opt.unwrap_or(0.0).max(0.0);
                            snap.anchor_instant = None;
                            snap.awaiting_audio_onset = true;
                            if play_btn_state.is_none() && explicit_is_playing.is_none() {
                                snap.status = "Paused".to_string();
                            }
                        }
                        if let Some(ref aid) = np_aid {
                            snap.adam_id = Some(aid.clone());
                            if let Some(meta) = snap.track_cache.get(aid).cloned() {
                                if !meta.title.is_empty() {
                                    snap.title = meta.title;
                                }
                                snap.artist = meta.artist;
                                snap.album = meta.album;
                                snap.artwork_url = meta.artwork_url;
                                snap.audio_quality = meta.audio_quality;
                                if snap.duration_sec <= 0.0 && meta.duration_sec > 0.0 {
                                    snap.duration_sec = meta.duration_sec;
                                }
                            }
                        } else if np_title.is_some_and(|t| !t.is_empty() && t != snap.title) {
                            snap.adam_id = None;
                        }
                        if let Some(t) = np_title.filter(|s| !s.is_empty()) {
                            if snap
                                .adam_id
                                .as_ref()
                                .is_none_or(|id| !snap.track_cache.contains_key(id))
                            {
                                snap.title = t.to_string();
                            }
                        }
                        if let Some(ar) = np_artist.filter(|s| !s.is_empty()) {
                            snap.artist = ar.to_string();
                        } else if let Some(l2) = np_line2.filter(|s| !s.is_empty()) {
                            if let Some((ar, al)) = l2.split_once(" — ") {
                                snap.artist = ar.to_string();
                                snap.album = al.to_string();
                            } else {
                                snap.artist = l2.to_string();
                            }
                        }
                        if let Some(al) = np_album.filter(|s| !s.is_empty()) {
                            snap.album = al.to_string();
                        }
                        snap.recompute_current_stream_specs();
                        changed = true;
                    } else if snap.adam_id.is_none() && np_aid.is_some() {
                        snap.adam_id = np_aid.clone();
                        changed = true;
                    } else if snap.transition_from_pqi.is_none() {
                        if let (Some(ref cur_aid), Some(ref resolved_aid)) =
                            (&snap.adam_id.clone(), &np_aid)
                        {
                            if cur_aid != resolved_aid {
                                if let Some(meta) = snap.track_cache.get(cur_aid).cloned() {
                                    snap.track_cache.entry(resolved_aid.clone()).or_insert(meta);
                                }
                                snap.adam_id = Some(resolved_aid.clone());
                                snap.recompute_current_stream_specs();
                                changed = true;
                            }
                        }
                    }
                }
            } else {
                if snap.transition_from_pqi.is_some()
                    && (snap.cmd_in_flight || snap.pending_play_tracks)
                {
                    // Reject `time`-only / `controls`-only packets from outgoing track while command/switch is in flight!
                    return;
                }
                if let Some(desc) = np_desc {
                    snap.current_descriptor = Some(desc);
                }
                if let Some(aid) = np_aid.as_ref().filter(|_| snap.adam_id.is_none()) {
                    snap.adam_id = Some(aid.clone());
                    snap.recompute_current_stream_specs();
                    changed = true;
                }
            }

            if let Some(lyrics_txt) = lyr_body.filter(|s| !s.trim().is_empty()) {
                let is_ttml = lyr_state.is_some_and(|st| st.eq_ignore_ascii_case("ttml"))
                    || lyrics_txt.contains("<tt");
                if let Some(aid) = np_aid.clone().or_else(|| snap.adam_id.clone()) {
                    snap.cache_lyrics_bounded(aid.clone(), lyrics_txt.to_string(), is_ttml);
                    if let Some(req_id) = snap.requested_track_id.clone() {
                        if req_id != aid {
                            snap.cache_lyrics_bounded(req_id, lyrics_txt.to_string(), is_ttml);
                        }
                    }
                }
            }

            // 2. Update `scrubberState` & `canBePositioned`
            // Disassembly `0xA0D889` & `0x2B683E`: `canBePositioned` is omitted from `time` when `false`!
            if let Some(sc) = scrubber_opt {
                snap.scrubber_indeterminate =
                    sc.eq_ignore_ascii_case("indeterminate") || sc.eq_ignore_ascii_case("unknown");
                changed = true;
            }
            if let Some(cbp) = cbp_opt {
                snap.can_be_positioned = cbp;
                if cbp && scrubber_opt.is_none() {
                    snap.scrubber_indeterminate = false;
                }
                changed = true;
            } else if time_d.is_some() {
                snap.can_be_positioned = false;
            }

            // 3. Determine playback status (`Playing` / `Paused` / `Stopped`) & onset readiness
            let mut next_status: Option<&'static str> = None;
            let cmd_starting = snap.cmd_in_flight || snap.pending_play_tracks;

            if let Some(exp_play) = explicit_is_playing {
                next_status = Some(if exp_play { "Playing" } else { "Paused" });
                snap.play_btn_shows_pause = exp_play;
                snap.desired_playing = exp_play;
                if exp_play {
                    snap.scrubber_indeterminate = false;
                } else {
                    snap.awaiting_audio_onset = false;
                }
            } else if let Some(pbs) = play_btn_state {
                if pbs.eq_ignore_ascii_case("pause") {
                    next_status = Some("Playing");
                    snap.play_btn_shows_pause = true;
                    snap.desired_playing = true;
                    if scrubber_opt.is_none() {
                        snap.scrubber_indeterminate = false;
                    }
                } else if pbs.eq_ignore_ascii_case("stop") {
                    snap.play_btn_shows_pause = false;
                    if !cmd_starting {
                        snap.desired_playing = false;
                        snap.awaiting_audio_onset = false;
                        snap.scrubber_indeterminate = false;
                        next_status = Some("Stopped");
                    }
                } else if pbs.eq_ignore_ascii_case("play") {
                    let was_confirmed_playing = snap.status == "Playing"
                        && !snap.awaiting_audio_onset
                        && !snap.scrubber_indeterminate;
                    let rate_wants_play = target_rate_opt.is_some_and(|tr| tr > 0.5);
                    if rate_wants_play {
                        snap.desired_playing = true;
                        if !was_confirmed_playing {
                            snap.play_btn_shows_pause = false;
                            next_status = Some("Paused");
                        }
                    } else if !cmd_starting {
                        snap.play_btn_shows_pause = false;
                        snap.desired_playing = false;
                        snap.awaiting_audio_onset = false;
                        next_status = Some("Paused");
                    }
                }
            } else if let Some(tr) = target_rate_opt {
                if tr <= 0.01 && !cmd_starting && !snap.awaiting_audio_onset {
                    next_status = Some("Paused");
                    snap.desired_playing = false;
                    snap.play_btn_shows_pause = false;
                } else if tr > 0.01 {
                    snap.desired_playing = true;
                }
            }

            // 4. Update `duration`
            if let Some(dur) = dur_opt {
                if dur > 0.0 {
                    snap.duration_sec = dur;
                    changed = true;
                }
            }

            // 5. Apply authoritative backend playback status + `currentTime` with soft-PLL anti-snapback
            let cur_pos_before = snap.current_position_sec();
            if let Some(st_str) = next_status {
                snap.status = st_str.to_string();
                snap.active = st_str != "Stopped";
                changed = true;
            }

            let backend_playing_confirmed = (explicit_is_playing == Some(true)
                || snap.play_btn_shows_pause)
                && !snap.scrubber_indeterminate;

            if let Some(t) = cur_time_opt {
                let suppress_zero = snap
                    .pending_reload_seek
                    .map(|target| target > 1.0 && t < 1.0)
                    .unwrap_or(false);
                let stale_pre_seek = snap.status == "Playing"
                    && snap.optimistic_seek_target.is_some_and(|(target, until)| {
                        (now_inst < until || snap.seeking_in_flight > 0)
                            && (t - target).abs() > 0.35
                    });
                if snap.status != "Playing"
                    || snap.optimistic_seek_target.is_some_and(|(target, until)| {
                        (now_inst >= until && snap.seeking_in_flight == 0)
                            || (t - target).abs() <= 0.35
                    })
                {
                    snap.optimistic_seek_target = None;
                }
                if !suppress_zero && !stale_pre_seek {
                    let scrubber_is_indet = scrubber_opt.is_some_and(|sc| {
                        sc.eq_ignore_ascii_case("indeterminate")
                            || sc.eq_ignore_ascii_case("unknown")
                    });
                    let cbp_ready =
                        cbp_opt == Some(true) || (snap.can_be_positioned && scrubber_opt.is_none());
                    let time_confirms_onset = !scrubber_is_indet
                        && (t > 0.02
                            || cbp_ready
                            || backend_playing_confirmed
                            || explicit_is_playing == Some(true));
                    if time_confirms_onset {
                        snap.awaiting_audio_onset = false;
                        snap.scrubber_indeterminate = false;
                        if (snap.desired_playing
                            || snap.play_btn_shows_pause
                            || explicit_is_playing == Some(true))
                            && !matches!(next_status, Some("Paused") | Some("Stopped"))
                        {
                            snap.status = "Playing".to_string();
                            snap.active = true;
                            snap.play_btn_shows_pause = true;
                        }
                    }
                    let can_advance_on_time_event = snap.status == "Playing"
                        && !snap.awaiting_audio_onset
                        && !snap.scrubber_indeterminate;
                    let effective_t = if can_advance_on_time_event && t > 0.02 {
                        t.max(0.0) + 0.018
                    } else {
                        t.max(0.0)
                    };
                    if was_advancing
                        && can_advance_on_time_event
                        && !just_switched_pqi
                        && snap.seeking_in_flight == 0
                    {
                        let err = effective_t - cur_pos_before;
                        if err.abs() <= 0.045 {
                            // Deadband: leave monotonic `(anchor_pos_sec, anchor_instant)` untouched!
                        } else if err.abs() < 0.14 {
                            let nudged = (cur_pos_before + 0.25 * err).max(cur_pos_before);
                            snap.anchor_pos_sec = nudged;
                            snap.anchor_instant = Some(now_inst);
                            changed = true;
                        } else {
                            snap.anchor_pos_sec = effective_t;
                            snap.anchor_instant = Some(now_inst);
                            changed = true;
                        }
                    } else {
                        snap.anchor_pos_sec = effective_t;
                        snap.anchor_instant = can_advance_on_time_event.then_some(now_inst);
                        changed = true;
                    }
                }
            } else if snap.status == "Playing" {
                if snap.awaiting_audio_onset && backend_playing_confirmed {
                    snap.awaiting_audio_onset = false;
                    snap.anchor_instant = (snap.seeking_in_flight == 0).then_some(now_inst);
                    changed = true;
                } else if !snap.awaiting_audio_onset
                    && !snap.scrubber_indeterminate
                    && !was_advancing
                    && snap.seeking_in_flight == 0
                {
                    snap.anchor_pos_sec = cur_pos_before;
                    snap.anchor_instant = Some(now_inst);
                    changed = true;
                }
            } else {
                // Paused or Stopped without explicit `currentTime`: freeze monotonic position cleanly
                snap.anchor_pos_sec = if snap.status == "Stopped" {
                    0.0
                } else {
                    cur_pos_before
                };
                snap.anchor_instant = None;
            }

            if let Some(sr) = sr_opt {
                if sr > 0 {
                    snap.live_agent_sample_rate = Some(sr as u32);
                    if let Some(bd) = bd_opt.filter(|&b| b >= 16 && b <= 32) {
                        snap.live_agent_bit_depth = Some(bd as u16);
                    }
                    if let Some(fmt) = fmt_opt {
                        snap.live_agent_format = Some(fmt.to_string());
                    }
                    let khz = (sr as f64) / 1000.0;
                    let sr_text = if (khz.fract()).abs() < 0.01 {
                        format!("{:.0}kHz", khz)
                    } else {
                        format!("{:.1}kHz", khz)
                    };
                    let is_aac = fmt_opt.is_some_and(|f| f.eq_ignore_ascii_case("aac"));
                    snap.audio_quality = Some(if is_aac {
                        format!("AAC {sr_text}")
                    } else {
                        let bd = bd_opt.unwrap_or(24);
                        format!("ALAC {bd}-bit / {sr_text}")
                    });
                    snap.recompute_current_stream_specs();
                    changed = true;
                }
            }
        }
        if changed {
            AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
            crate::audio_viz::wake_registered_ui_window();
        }
    }

    unsafe fn rust_to_hstring(s: &str) -> Result<HSTRING, String> {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let mut h: HSTRING = null_mut();
        let hr = WindowsCreateString(wide.as_ptr(), wide.len() as u32, &mut h);
        if hr < 0 {
            Err(format!("WindowsCreateString 失败: 0x{:08X}", hr as u32))
        } else {
            Ok(h)
        }
    }

    unsafe fn hstring_to_rust(h: HSTRING) -> String {
        if h.is_null() {
            return String::new();
        }
        let mut len: u32 = 0;
        let ptr = WindowsGetStringRawBuffer(h, &mut len);
        if ptr.is_null() || len == 0 {
            return String::new();
        }
        let slice = std::slice::from_raw_parts(ptr, len as usize);
        String::from_utf16_lossy(slice)
    }

    unsafe fn await_async_hstring(op: *mut c_void, timeout_ms: u64) -> Result<String, String> {
        if op.is_null() {
            return Err("IAsyncOperation_HSTRING 为空".to_string());
        }
        let unk_vtbl = *(op as *const *const unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT);
        let qi_fn = *unk_vtbl;
        let release_fn: unsafe extern "system" fn(*mut c_void) -> ULONG =
            std::mem::transmute(*(unk_vtbl.add(2)));

        let mut async_info: *mut c_void = null_mut();
        let hr = qi_fn(op, &IID_IASYNC_INFO, &mut async_info);
        if hr < 0 || async_info.is_null() {
            release_fn(op);
            return Err(format!("QueryInterface(IAsyncInfo) 失败: 0x{:08X}", hr as u32));
        }

        let ai_vtbl = *(async_info as *const *const *const c_void);
        let get_status_fn: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT =
            std::mem::transmute(*(ai_vtbl.add(7)));
        let close_fn: unsafe extern "system" fn(*mut c_void) -> HRESULT =
            std::mem::transmute(*(ai_vtbl.add(10)));
        let ai_release_fn: unsafe extern "system" fn(*mut c_void) -> ULONG =
            std::mem::transmute(*(ai_vtbl.add(2)));

        let start = Instant::now();
        let mut status: i32 = 0;
        loop {
            let s_hr = get_status_fn(async_info, &mut status);
            if s_hr < 0 || status != 0 {
                break;
            }
            if start.elapsed() > Duration::from_millis(timeout_ms) {
                let _ = close_fn(async_info);
                ai_release_fn(async_info);
                release_fn(op);
                return Err("等待 AMPLibraryAgent 异步命令超时".to_string());
            }
            std::thread::sleep(Duration::from_millis(12));
        }

        let mut output = String::new();
        if status == 1 {
            let op_vtbl = *(op as *const *const *const c_void);
            let get_results_fn: unsafe extern "system" fn(*mut c_void, *mut HSTRING) -> HRESULT =
                std::mem::transmute(*(op_vtbl.add(8)));
            let mut h_res: HSTRING = null_mut();
            let r_hr = get_results_fn(op, &mut h_res);
            if r_hr >= 0 && !h_res.is_null() {
                output = hstring_to_rust(h_res);
                let _ = WindowsDeleteString(h_res);
            }
        }

        let _ = close_fn(async_info);
        ai_release_fn(async_info);
        release_fn(op);

        if status == 1 {
            Ok(output)
        } else {
            Err(format!("AMPLibraryAgent 异步状态异常: status={status}"))
        }
    }

    fn ensure_amp_proxy_dll() -> Result<std::path::PathBuf, String> {
        let mut candidates = Vec::new();
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(dir) = exe_path.parent() {
                candidates.push(dir.join("amp_proxy.dll"));
            }
        }
        candidates.push(std::env::temp_dir().join("am_light_amp_proxy.dll"));

        for dll_path in &candidates {
            let needs_write = match fs::metadata(dll_path) {
                Ok(m) => m.len() != AMP_PROXY_DLL_BYTES.len() as u64,
                Err(_) => true,
            };
            if !needs_write || fs::write(dll_path, AMP_PROXY_DLL_BYTES).is_ok() {
                if dll_path.exists() {
                    return Ok(dll_path.clone());
                }
            }
        }
        Err("释放 amp_proxy.dll 失败".to_string())
    }

    static COM_SEC_ONCE: Once = Once::new();
    static COM_SEC_HR: AtomicI32 = AtomicI32::new(i32::MIN);

    pub fn init_process_com_security() -> i32 {
        unsafe {
            COM_SEC_ONCE.call_once(|| {
                // NULL security descriptor + RPC_C_AUTHN_LEVEL_NONE disables DCOM ACL rejection
                // for reverse callbacks from MSIX AppContainer `AMPLibraryAgent.exe`.
                let mut hr = CoInitializeSecurity(
                    null_mut(),
                    -1,
                    null_mut(),
                    null_mut(),
                    1, // RPC_C_AUTHN_LEVEL_NONE
                    2, // RPC_C_IMP_LEVEL_IDENTIFY
                    null_mut(),
                    0, // EOAC_NONE
                    null_mut(),
                );
                if hr == 0x8004_01F0u32 as i32 {
                    let _ = CoInitializeEx(null_mut(), 0);
                    hr = CoInitializeSecurity(
                        null_mut(),
                        -1,
                        null_mut(),
                        null_mut(),
                        1, // RPC_C_AUTHN_LEVEL_NONE
                        2, // RPC_C_IMP_LEVEL_IDENTIFY
                        null_mut(),
                        0,
                        null_mut(),
                    );
                }
                if hr == 0x8001_0119u32 as i32 {
                    hr = 0;
                }
                COM_SEC_HR.store(hr, Ordering::Release);
            });
            COM_SEC_HR.load(Ordering::Acquire)
        }
    }

    pub fn com_security_init_hr() -> i32 {
        COM_SEC_HR.load(Ordering::Acquire)
    }

    pub fn playback_event_count() -> u64 {
        PB_EVENT_COUNT.load(Ordering::Relaxed)
    }

    pub fn playback_reg_diagnostics() -> (i32, i32, u64) {
        let hrs = LAST_PB_REG_HRS.load(Ordering::Acquire);
        let toks = LAST_PB_REG_TOKS.load(Ordering::Acquire);
        ((hrs >> 32) as u32 as i32, (hrs & 0xFFFF_FFFF) as u32 as i32, toks)
    }

    pub fn query_playback_delegate_iid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> i32 {
        unsafe {
            let iid = GUID {
                data1,
                data2,
                data3,
                data4,
            };
            let mut out: *mut c_void = null_mut();
            let hr = pb_qi(&raw mut PB_DELEGATE, &iid, &mut out);
            if hr >= 0 && !out.is_null() {
                let _ = pb_release(&raw mut PB_DELEGATE);
            }
            hr
        }
    }

    pub fn dispatch_playback_event_plist(plist: &PlistValue) {
        PB_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
        handle_incoming_playback_event(plist);
    }

    unsafe fn init_com_and_security() {
        let _ = CoInitializeEx(null_mut(), 0); // COINIT_MULTITHREADED = 0
        let _ = init_process_com_security();
    }

    pub fn list_matching_pids(exe_name_lower: &str) -> Vec<u32> {
        let mut pids = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(0x0000_0002 /* TH32CS_SNAPPROCESS */, 0);
            if snap.is_null() || (snap as isize) == -1 {
                return pids;
            }
            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dw_size = size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    let end = entry
                        .sz_exe_file
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.sz_exe_file.len());
                    let name = String::from_utf16_lossy(&entry.sz_exe_file[..end]);
                    if name.eq_ignore_ascii_case(exe_name_lower) {
                        pids.push(entry.th32_process_id);
                    }
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        pids
    }

    unsafe extern "system" fn close_window_for_pid_proc(hwnd: *mut c_void, lparam: isize) -> i32 {
        let target_pid = lparam as u32;
        let mut wnd_pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut wnd_pid);
        if wnd_pid == target_pid {
            let _ = PostMessageW(hwnd, 0x0010 /* WM_CLOSE */, 0, 0);
        }
        1
    }

    pub fn close_apple_music_frontend_if_running() {
        let pids = list_matching_pids("AppleMusic.exe");
        if pids.is_empty() {
            return;
        }
        unsafe {
            for &pid in &pids {
                let _ = EnumWindows(close_window_for_pid_proc, pid as isize);
            }
        }
        std::thread::sleep(Duration::from_millis(280));
        let remaining = list_matching_pids("AppleMusic.exe");
        unsafe {
            for pid in remaining {
                let h = OpenProcess(0x0001 /* PROCESS_TERMINATE */, 0, pid);
                if !h.is_null() {
                    let _ = TerminateProcess(h, 0);
                    let _ = CloseHandle(h);
                }
            }
        }
        if !pids.is_empty() {
            std::thread::sleep(Duration::from_millis(180));
        }
    }

    struct FindVisibleCtx {
        target_pid: u32,
        found_hwnd: *mut c_void,
    }

    unsafe extern "system" fn find_visible_window_proc(hwnd: *mut c_void, lparam: isize) -> i32 {
        let ctx = &mut *(lparam as *mut FindVisibleCtx);
        let mut wnd_pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut wnd_pid);
        if wnd_pid == ctx.target_pid {
            let mut cls_buf = [0u16; 64];
            let cls_len = GetClassNameW(hwnd, cls_buf.as_mut_ptr(), cls_buf.len() as i32);
            if cls_len > 0 {
                let cls_name = String::from_utf16_lossy(&cls_buf[..cls_len as usize]);
                if cls_name == "AMLightNativeWindowClass" && IsWindowVisible(hwnd) != 0 {
                    ctx.found_hwnd = hwnd;
                    return 0;
                }
            }
        }
        1
    }

    pub fn ensure_single_ui_instance_or_focus() -> bool {
        unsafe {
            let my_pid = GetCurrentProcessId();
            let exe_name = std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "AM-Light.exe".to_string());

            let mut killed_zombie = false;
            for pid in list_matching_pids(&exe_name) {
                if pid == my_pid {
                    continue;
                }
                let mut ctx = FindVisibleCtx {
                    target_pid: pid,
                    found_hwnd: null_mut(),
                };
                let _ = EnumWindows(find_visible_window_proc, (&mut ctx as *mut FindVisibleCtx) as isize);
                if !ctx.found_hwnd.is_null() {
                    let _ = ShowWindow(ctx.found_hwnd, 9 /* SW_RESTORE */);
                    let _ = SetForegroundWindow(ctx.found_hwnd);
                    return false;
                } else {
                    let h = OpenProcess(0x0001 /* PROCESS_TERMINATE */, 0, pid);
                    if !h.is_null() {
                        let _ = TerminateProcess(h, 0);
                        let _ = CloseHandle(h);
                        killed_zombie = true;
                    }
                }
            }
            if killed_zombie {
                std::thread::sleep(Duration::from_millis(380));
            }
            true
        }
    }

    pub fn show_fatal_error_dialog(msg: &str) {
        unsafe {
            let mut text_w: Vec<u16> = msg.encode_utf16().collect();
            text_w.push(0);
            let mut title_w: Vec<u16> = "AM Light 启动异常".encode_utf16().collect();
            title_w.push(0);
            let _ = MessageBoxW(null_mut(), text_w.as_ptr(), title_w.as_ptr(), 0x10 /* MB_ICONERROR */);
        }
    }

    enum WorkerCmd {
        PlayTracks {
            tracks: Vec<AgentTrackInput>,
            start_index: usize,
            reply: mpsc::SyncSender<Result<SmtcState, String>>,
        },
        Control {
            action: String,
            value: Option<f64>,
            reply: mpsc::SyncSender<Result<SmtcState, String>>,
        },
        EnsureConnected {
            reply: mpsc::SyncSender<Result<AgentStatus, String>>,
        },
        GetPrefs {
            reply: mpsc::SyncSender<Result<AudioPrefs, String>>,
        },
        SetPrefs {
            prefs: AudioPrefs,
            reply: mpsc::SyncSender<Result<AudioPrefs, String>>,
        },
        DownloadQueueAction {
            pause: bool,
            reply: mpsc::SyncSender<Result<(), String>>,
        },
        SaveAgentLog {
            folder: String,
            filename: String,
            reply: mpsc::SyncSender<Result<String, String>>,
        },
        GetLyrics {
            adam_id: String,
            connect_if_needed: bool,
            reply: mpsc::SyncSender<Result<Option<(String, bool)>, String>>,
        },
        Disconnect {
            reply: mpsc::SyncSender<Result<(), String>>,
        },
    }

    fn plist_bool_or(dict: &PlistValue, key: &str, default: bool) -> bool {
        dict.get(key)
            .and_then(|v| v.as_bool().or_else(|| v.as_i64().map(|n| n != 0)))
            .unwrap_or(default)
    }

    fn plist_i64_or(dict: &PlistValue, key: &str, default: i64) -> i64 {
        dict.get(key).and_then(PlistValue::as_i64).unwrap_or(default)
    }

    fn build_store_item_plist(t: &AgentTrackInput) -> PlistValue {
        let aid = t.effective_adam_id();
        let sf = get_active_storefront();
        let kind = t.kind.as_deref().unwrap_or("song");
        let play_params = PlistValue::dict([
            ("id", PlistValue::String(aid.clone())),
            ("kind", PlistValue::String(kind.to_string())),
        ]);

        // AMPLibraryAgent's CreateOptimisticTrackFromResourceDict reads items[i]["attributes"]["audioTraits"]
        // to populate trackInfo->aucp (0x04=lossless, 0x10=hi-res-lossless, 0x01=atmos/spatial).
        // Without attributes.audioTraits, aucp is 0 and AMPLibraryAgent stays on legacy SNPlayer (AAC) instead of CMPlayer!
        let traits_list: Vec<PlistValue> = if t.audio_traits.is_empty() {
            [
                "hi-res-lossless",
                "lossless",
                "atmos",
                "spatial",
                "lossy-stereo",
            ]
            .into_iter()
            .map(|s| PlistValue::String(s.to_string()))
            .collect()
        } else {
            let mut v: Vec<PlistValue> = t
                .audio_traits
                .iter()
                .map(|s| PlistValue::String(s.clone()))
                .collect();
            if !t.audio_traits.iter().any(|s| s == "lossy-stereo") {
                v.push(PlistValue::String("lossy-stereo".to_string()));
            }
            v
        };

        let mut attrs = BTreeMap::new();
        if !t.name.is_empty() {
            attrs.insert("name".to_string(), PlistValue::String(t.name.clone()));
        }
        if !t.artist_name.is_empty() {
            attrs.insert(
                "artistName".to_string(),
                PlistValue::String(t.artist_name.clone()),
            );
        }
        if !t.album_name.is_empty() {
            attrs.insert(
                "albumName".to_string(),
                PlistValue::String(t.album_name.clone()),
            );
        }
        if t.duration_sec > 0.0 {
            attrs.insert(
                "durationInMillis".to_string(),
                PlistValue::Int((t.duration_sec * 1000.0).round() as i64),
            );
        }
        attrs.insert(
            "url".to_string(),
            PlistValue::String(format!("https://music.apple.com/{sf}/song/{aid}")),
        );
        attrs.insert("audioTraits".to_string(), PlistValue::Array(traits_list));
        attrs.insert("playParams".to_string(), play_params.clone());

        let mut item_map = BTreeMap::new();
        item_map.insert("id".to_string(), PlistValue::String(aid));
        item_map.insert("type".to_string(), PlistValue::String("songs".to_string()));
        item_map.insert("kind".to_string(), PlistValue::String(kind.to_string()));
        if !t.name.is_empty() {
            item_map.insert("name".to_string(), PlistValue::String(t.name.clone()));
        }
        if !t.artist_name.is_empty() {
            item_map.insert(
                "artistName".to_string(),
                PlistValue::String(t.artist_name.clone()),
            );
        }
        if !t.album_name.is_empty() {
            item_map.insert(
                "albumName".to_string(),
                PlistValue::String(t.album_name.clone()),
            );
        }
        item_map.insert("attributes".to_string(), PlistValue::Dict(attrs));
        item_map.insert("playParams".to_string(), play_params);
        PlistValue::Dict(item_map)
    }

    struct AgentSession {
        lib: *mut IAMPLibrary,
        my_pid: u32,
        client_id: u32,
        pb_tok: EventRegistrationToken,
        pbcb_tok: EventRegistrationToken,
        next_cid: u64,
        grace_until: Instant,
        last_queue: Vec<AgentTrackInput>,
    }

    impl AgentSession {
        unsafe fn connect() -> Result<Self, String> {
            let _ = ensure_amp_proxy_dll()?;
            init_com_and_security();

            // Ensure AppleMusic.exe UI frontend is NOT occupying the single Music MediaApp slot.
            close_apple_music_frontend_if_running();

            let mut raw_lib: *mut c_void = null_mut();
            let hr = CoCreateInstance(
                &CLSID_AMP_MUSIC_LIBRARY,
                null_mut(),
                0x4, // CLSCTX_LOCAL_SERVER
                &IID_IAMP_LIBRARY,
                &mut raw_lib,
            );
            if hr < 0 || raw_lib.is_null() {
                if (hr as u32) == 0x80070005 {
                    return Err(
                        "连接 Apple Music 播放服务被系统权限拦截 (0x80070005：当前 exe 处于 Windows 低完整性级别 Low Mandatory Level)。请打开正常权限目录下的 AM-Light.exe，或在终端执行 icacls \"程序目录\" /setintegritylevel Medium /T".to_string(),
                    );
                }
                return Err(format!(
                    "连接 Apple Music 播放服务 (AMPLibraryAgent) 失败 (HRESULT=0x{:08X})。请确认系统已安装 Apple Music。",
                    hr as u32
                ));
            }
            let lib = raw_lib as *mut IAMPLibrary;
            ACTIVE_LIB_PTR.store(lib, Ordering::Release);

            let my_pid = GetCurrentProcessId();
            let mut agent_pid: u32 = 0;
            let mut setup_res: i32 = 0;
            let hr_setup = ((*(*lib).lp_vtbl).setup)(lib, my_pid, &mut agent_pid, &mut setup_res);
            if hr_setup < 0 {
                let _ = ((*(*lib).lp_vtbl).release)(lib);
                ACTIVE_LIB_PTR.store(null_mut(), Ordering::Release);
                if (hr_setup as u32) == 0x80070005 {
                    return Err(
                        "AMPLibrary.Setup 被系统权限拦截 (0x80070005：当前 exe 处于 Windows 低完整性级别)。请打开正常权限目录下的 AM-Light.exe，或执行 icacls \"程序目录\" /setintegritylevel Medium /T".to_string(),
                    );
                }
                return Err(format!("AMPLibrary.Setup 失败: 0x{:08X}", hr_setup as u32));
            }

            let mut pb_tok = EventRegistrationToken::default();
            let mut pbcb_tok = EventRegistrationToken::default();
            let hr_pb = ((*(*lib).lp_vtbl).add_playback_event_handler)(
                lib,
                (&raw mut PB_DELEGATE) as *mut c_void,
                &mut pb_tok,
            );
            let hr_pbcb = ((*(*lib).lp_vtbl).add_playback_event_with_callback_handler)(
                lib,
                (&raw mut PBCB_DELEGATE) as *mut c_void,
                &mut pbcb_tok,
            );
            LAST_PB_REG_HRS.store(((hr_pb as u32 as u64) << 32) | (hr_pbcb as u32 as u64), Ordering::Release);
            LAST_PB_REG_TOKS.store(((pb_tok.value as u64) << 32) | (pbcb_tok.value as u32 as u64), Ordering::Release);

            let mut h_start_res: HSTRING = null_mut();
            let hr_start = ((*(*lib).lp_vtbl).media_app_starting)(lib, 1 /* Music */, &mut h_start_res);
            if !h_start_res.is_null() {
                let _ = WindowsDeleteString(h_start_res);
            }
            if hr_start < 0 {
                let _ = ((*(*lib).lp_vtbl).release)(lib);
                ACTIVE_LIB_PTR.store(null_mut(), Ordering::Release);
                return Err(format!(
                    "AMPLibrary.MediaAppStarting 失败: 0x{:08X}",
                    hr_start as u32
                ));
            }

            let mut open_op: *mut c_void = null_mut();
            let hr_open = ((*(*lib).lp_vtbl).media_app_open_library_async)(lib, &mut open_op);
            if hr_open < 0 {
                let _ = ((*(*lib).lp_vtbl).release)(lib);
                ACTIVE_LIB_PTR.store(null_mut(), Ordering::Release);
                return Err(format!(
                    "AMPLibrary.MediaAppOpenLibraryAsync 失败: 0x{:08X}",
                    hr_open as u32
                ));
            }
            let _ = await_async_hstring(open_op, 20_000)?;

            // Query library location URL for offline downloads folder resolution
            let mut lib_loc_url: Option<String> = None;
            let mut info_op: *mut c_void = null_mut();
            if ((*(*lib).lp_vtbl).media_app_get_library_info_async)(lib, &mut info_op) >= 0
                && !info_op.is_null()
            {
                if let Ok(info_hex) = await_async_hstring(info_op, 5_000) {
                    if let Ok(info_plist) = decode_bplist_hex(&info_hex) {
                        lib_loc_url = info_plist
                            .get("responseDict")
                            .and_then(|d| d.get("library-location-url"))
                            .and_then(PlistValue::as_str)
                            .map(|s| s.to_string());
                    }
                }
            }

            let mut client_id: u32 = 0;
            let _ = ((*(*lib).lp_vtbl).get_client_id)(lib, &mut client_id);
            if client_id == 0 {
                if let Ok(h_name) = rust_to_hstring("Music") {
                    let mut reg_res = RegisterClientResult::default();
                    let _ = ((*(*lib).lp_vtbl).register_library_client)(lib, h_name, 200, &mut reg_res);
                    let _ = WindowsDeleteString(h_name);
                    if reg_res.client_id != 0 {
                        client_id = reg_res.client_id;
                    }
                }
                let _ = ((*(*lib).lp_vtbl).get_client_id)(lib, &mut client_id);
                if client_id == 0 {
                    client_id = 200;
                }
            }

            if let Ok(h_empty) = rust_to_hstring(&empty_dict_hex()) {
                let mut dom_op: *mut c_void = null_mut();
                if ((*(*lib).lp_vtbl).open_domains_async)(lib, 1, client_id, h_empty, &mut dom_op) >= 0
                    && !dom_op.is_null()
                {
                    let _ = await_async_hstring(dom_op, 12_000);
                }
                let _ = WindowsDeleteString(h_empty);
            }

            let mut state_code: i32 = 0;
            let _ = ((*(*lib).lp_vtbl).get_state)(lib, &mut state_code);

            let initial_vol = SHARED.read().map(|s| s.volume).unwrap_or(0.75);
            if let Ok(mut snap) = SHARED.write() {
                snap.connected = true;
                snap.agent_pid = if agent_pid > 0 {
                    Some(agent_pid)
                } else {
                    list_matching_pids("AMPLibraryAgent.exe").into_iter().next()
                };
                snap.client_id = client_id;
                snap.state_code = state_code;
                if let Some(loc) = lib_loc_url {
                    snap.prefs.library_location_url = Some(loc);
                }
            }

            let mut sess = Self {
                lib,
                my_pid,
                client_id,
                pb_tok,
                pbcb_tok,
                next_cid: 100,
                grace_until: Instant::now(),
                last_queue: Vec::new(),
            };

            // Apply initial volume and sync live audio quality prefs from AMPLibraryAgent
            let _ = sess.exec_playback_cmd(
                5013,
                Some(PlistValue::dict([("volume", PlistValue::Real(initial_vol))])),
            );
            let _ = sess.sync_prefs_from_agent();

            Ok(sess)
        }

        unsafe fn exec_subsystem_cmd(
            &mut self,
            subsystem_id: i64,
            command_id: u32,
            params: Option<PlistValue>,
        ) -> Result<PlistValue, String> {
            self.next_cid += 1;
            let mut envelope = BTreeMap::new();
            envelope.insert("cid".to_string(), PlistValue::UInt(self.next_cid));
            envelope.insert("subsystem-id".to_string(), PlistValue::Int(subsystem_id));
            envelope.insert("command-id".to_string(), PlistValue::Int(command_id as i64));
            if let Some(p) = params {
                envelope.insert("command-params".to_string(), p);
            }

            let hex_cmd = encode_bplist_hex(&PlistValue::Dict(envelope));
            let h_cmd = rust_to_hstring(&hex_cmd)?;
            let mut op: *mut c_void = null_mut();
            let hr = ((*(*self.lib).lp_vtbl).media_app_exec_agent_command_async)(
                self.lib,
                h_cmd,
                &mut op,
            );
            let _ = WindowsDeleteString(h_cmd);
            if hr < 0 {
                return Err(format!(
                    "执行播放服务指令 ({subsystem_id}:{command_id}) 失败: HRESULT=0x{:08X}",
                    hr as u32
                ));
            }
            let out_hex = await_async_hstring(op, 15_000)?;
            decode_bplist_hex(&out_hex)
        }

        unsafe fn exec_playback_cmd(
            &mut self,
            command_id: u32,
            params: Option<PlistValue>,
        ) -> Result<PlistValue, String> {
            self.exec_subsystem_cmd(2, command_id, params)
        }

        unsafe fn sync_prefs_from_agent(&mut self) -> Result<AudioPrefs, String> {
            let names = [
                "losslessEnabled",
                "streamingAudioQuality",
                "downloadAudioQuality",
                "dolbyAtmosSetting",
                "downloadMultichannelAudio",
                "soundCheckEnabled",
                "crossfadeSongsEnabled",
                "crossfadeSongsSeconds",
            ]
            .into_iter()
            .map(|s| PlistValue::String(s.to_string()))
            .collect();

            let resp = self.exec_playback_cmd(
                5031,
                Some(PlistValue::dict([("[prefNames]", PlistValue::Array(names))])),
            )?;

            let err_code = resp.get("err").and_then(PlistValue::as_i64).unwrap_or(0);
            if err_code != 0 {
                return Err(format!("读取 AMPLibraryAgent 音质配置失败 (err={err_code})"));
            }

            let cur = SHARED.read().map(|s| s.prefs.clone()).unwrap_or_default();
            let next = if let Some(pdict) = resp.get("responseDict").and_then(|d| d.get("prefsDict"))
            {
                AudioPrefs {
                    lossless_enabled: plist_bool_or(pdict, "losslessEnabled", cur.lossless_enabled),
                    streaming_audio_quality: plist_i64_or(
                        pdict,
                        "streamingAudioQuality",
                        cur.streaming_audio_quality,
                    ),
                    download_audio_quality: plist_i64_or(
                        pdict,
                        "downloadAudioQuality",
                        cur.download_audio_quality,
                    ),
                    dolby_atmos_setting: plist_i64_or(
                        pdict,
                        "dolbyAtmosSetting",
                        cur.dolby_atmos_setting,
                    ),
                    download_multichannel_audio: plist_bool_or(
                        pdict,
                        "downloadMultichannelAudio",
                        cur.download_multichannel_audio,
                    ),
                    sound_check_enabled: plist_bool_or(
                        pdict,
                        "soundCheckEnabled",
                        cur.sound_check_enabled,
                    ),
                    crossfade_songs_enabled: plist_bool_or(
                        pdict,
                        "crossfadeSongsEnabled",
                        cur.crossfade_songs_enabled,
                    ),
                    crossfade_songs_seconds: plist_i64_or(
                        pdict,
                        "crossfadeSongsSeconds",
                        cur.crossfade_songs_seconds,
                    ),
                    library_location_url: cur.library_location_url,
                }
                .sanitized()
            } else {
                cur
            };

            if let Ok(mut snap) = SHARED.write() {
                snap.prefs = next.clone();
                snap.recompute_current_stream_specs();
            }

            Ok(next)
        }

        unsafe fn wait_until_player_seekable(&mut self, max_wait_ms: u64) {
            let start = Instant::now();
            let mut resolved_since: Option<Instant> = None;
            let mut step = 0u32;
            while start.elapsed() < Duration::from_millis(max_wait_ms) {
                if SHARED.read().map(|s| s.can_be_positioned).unwrap_or(false) {
                    std::thread::sleep(Duration::from_millis(90));
                    return;
                }
                if step % 3 == 2 {
                    if let Ok(resp) = self.exec_playback_cmd(5025, None) {
                        let dur = resp
                            .get("responseDict")
                            .and_then(|d| d.get("Sections"))
                            .and_then(PlistValue::as_array)
                            .and_then(|secs| {
                                secs.iter().find(|s| {
                                    s.get("kind").and_then(PlistValue::as_str) == Some("current")
                                })
                            })
                            .and_then(|s| s.get("items"))
                            .and_then(PlistValue::as_array)
                            .and_then(|items| items.first())
                            .and_then(|it| it.get("duration"))
                            .and_then(PlistValue::as_f64)
                            .unwrap_or(0.0);
                        if dur > 0.0 {
                            let t0 = *resolved_since.get_or_insert_with(Instant::now);
                            if t0.elapsed() >= Duration::from_millis(520) {
                                return;
                            }
                        }
                    }
                }
                step += 1;
                std::thread::sleep(Duration::from_millis(80));
            }
        }

        unsafe fn reload_active_stream_at_position(&mut self) {
            let reload_ctx = SHARED.read().ok().and_then(|snap| {
                if !snap.active {
                    return None;
                }
                let aid = snap.adam_id.clone()?;
                if aid.is_empty() {
                    return None;
                }
                Some((
                    aid,
                    snap.current_position_sec(),
                    snap.status == "Playing",
                    snap.volume,
                    snap.title.clone(),
                    snap.artist.clone(),
                    snap.album.clone(),
                    snap.duration_sec,
                    snap.artwork_url.clone(),
                ))
            });

            let Some((aid, resume_sec, was_playing, cur_vol, title, artist, album, dur, art)) =
                reload_ctx
            else {
                return;
            };

            let (queue_items, selected_idx) = if let Some(pos) = self
                .last_queue
                .iter()
                .position(|t| t.effective_adam_id() == aid)
            {
                (
                    self.last_queue
                        .iter()
                        .map(build_store_item_plist)
                        .collect::<Vec<_>>(),
                    pos,
                )
            } else {
                let fallback_track = AgentTrackInput {
                    id: aid.clone(),
                    catalog_id: Some(aid),
                    kind: Some("song".to_string()),
                    name: title,
                    artist_name: artist,
                    album_name: album,
                    duration_sec: dur,
                    artwork_url: art,
                    audio_traits: Vec::new(),
                };
                (vec![build_store_item_plist(&fallback_track)], 0)
            };

            let action_str = if was_playing { "play" } else { "pause" };
            let target_sec = resume_sec.max(0.0);
            if let Ok(mut snap) = SHARED.write() {
                snap.can_be_positioned = false;
                snap.pending_reload_seek = (target_sec > 0.4).then_some(target_sec);
            }

            // Mute during stream re-negotiation so 0:00 startup before seek is completely silent
            let _ = self.exec_playback_cmd(
                5013,
                Some(PlistValue::dict([("volume", PlistValue::Real(0.0))])),
            );

            let params = PlistValue::dict([
                (
                    "storeParams",
                    PlistValue::dict([
                        ("items", PlistValue::Array(queue_items)),
                        ("selectedTrackIndex", PlistValue::Int(selected_idx as i64)),
                    ]),
                ),
                ("action", PlistValue::String(action_str.to_string())),
            ]);

            self.grace_until = Instant::now() + Duration::from_secs(6);
            if let Ok(resp) = self.exec_playback_cmd(5000, Some(params)) {
                if resp.get("err").and_then(PlistValue::as_i64).unwrap_or(0) == 0 {
                    if target_sec > 0.4 {
                        self.wait_until_player_seekable(2_800);
                        let _ = self.exec_playback_cmd(
                            5010,
                            Some(PlistValue::dict([(
                                "currentTime",
                                PlistValue::Real(target_sec),
                            )])),
                        );
                    }
                    let _ = self.exec_playback_cmd(
                        5013,
                        Some(PlistValue::dict([(
                            "volume",
                            PlistValue::Real(cur_vol.clamp(0.0, 1.0)),
                        )])),
                    );
                    if let Ok(mut snap) = SHARED.write() {
                        snap.pqi_id = None;
                        snap.pending_reload_seek = None;
                        snap.awaiting_audio_onset = false;
                        snap.scrubber_indeterminate = false;
                        snap.desired_playing = was_playing;
                        snap.play_btn_shows_pause = was_playing;
                        snap.anchor_pos_sec = target_sec;
                        snap.anchor_instant = was_playing.then(Instant::now);
                        snap.status = if was_playing {
                            "Playing".to_string()
                        } else {
                            "Paused".to_string()
                        };
                    }
                    return;
                }
            }

            // Restore volume & clear pending flag on error fallback
            let _ = self.exec_playback_cmd(
                5013,
                Some(PlistValue::dict([(
                    "volume",
                    PlistValue::Real(cur_vol.clamp(0.0, 1.0)),
                )])),
            );
            if let Ok(mut snap) = SHARED.write() {
                snap.pending_reload_seek = None;
            }
        }

        unsafe fn apply_prefs_to_agent(&mut self, prefs: AudioPrefs) -> Result<AudioPrefs, String> {
            let prev = SHARED.read().map(|s| s.prefs.clone()).unwrap_or_default();
            let clean = prefs.sanitized();
            let stream_tier_changed = prev.lossless_enabled != clean.lossless_enabled
                || prev.streaming_audio_quality != clean.streaming_audio_quality
                || prev.dolby_atmos_setting != clean.dolby_atmos_setting;

            let prefs_dict = PlistValue::dict([
                ("losslessEnabled", PlistValue::Bool(clean.lossless_enabled)),
                (
                    "streamingAudioQuality",
                    PlistValue::Int(clean.streaming_audio_quality),
                ),
                (
                    "downloadAudioQuality",
                    PlistValue::Int(clean.download_audio_quality),
                ),
                (
                    "dolbyAtmosSetting",
                    PlistValue::Int(clean.dolby_atmos_setting),
                ),
                (
                    "downloadMultichannelAudio",
                    PlistValue::Bool(clean.download_multichannel_audio),
                ),
                (
                    "soundCheckEnabled",
                    PlistValue::Bool(clean.sound_check_enabled),
                ),
                (
                    "crossfadeSongsEnabled",
                    PlistValue::Bool(clean.crossfade_songs_enabled),
                ),
                (
                    "crossfadeSongsSeconds",
                    PlistValue::Int(clean.crossfade_songs_seconds),
                ),
            ]);

            let resp = self.exec_playback_cmd(
                5032,
                Some(PlistValue::dict([("prefsDict", prefs_dict)])),
            )?;
            let err_code = resp.get("err").and_then(PlistValue::as_i64).unwrap_or(0);
            if err_code != 0 {
                return Err(format!("写入 AMPLibraryAgent 音质配置失败 (err={err_code})"));
            }

            let synced = self.sync_prefs_from_agent()?;
            if stream_tier_changed {
                self.reload_active_stream_at_position();
            }
            Ok(synced)
        }

        unsafe fn sync_queue_state(&mut self) -> Result<(), String> {
            let resp = self.exec_playback_cmd(5025 /* GetPlayQueueState */, None)?;
            let Some(resp_dict) = resp.get("responseDict") else {
                return Ok(());
            };
            let Some(sections) = resp_dict.get("Sections").and_then(PlistValue::as_array) else {
                return Ok(());
            };

            let mut current_item: Option<&PlistValue> = None;
            for sec in sections {
                if sec.get("kind").and_then(PlistValue::as_str) == Some("current") {
                    if let Some(items) = sec.get("items").and_then(PlistValue::as_array) {
                        current_item = items.first();
                    }
                    break;
                }
            }

            let Ok(mut snap) = SHARED.write() else {
                return Ok(());
            };

            if let Some(item) = current_item {
                let pqi_id = item.get("pqiID").and_then(PlistValue::as_u64);
                let item_desc = item.get("descriptor").cloned();
                let adam_id = extract_adam_id_from_desc(item_desc.as_ref());
                let dur = item.get("duration").and_then(PlistValue::as_f64).unwrap_or(0.0);
                let line1 = item
                    .get("line1")
                    .and_then(PlistValue::as_str)
                    .unwrap_or_default()
                    .to_string();
                let line2 = item
                    .get("line2")
                    .and_then(PlistValue::as_str)
                    .unwrap_or_default()
                    .to_string();

                if let Some(desc) = item_desc {
                    snap.current_descriptor = Some(desc);
                }

                if pqi_id.is_some() && snap.transition_from_pqi == pqi_id && snap.pending_play_tracks {
                    return Ok(());
                }

                if snap.pending_play_tracks {
                    if let (Some(ref prev_aid), Some(ref resolved_aid)) =
                        (&snap.adam_id.clone(), &adam_id)
                    {
                        if prev_aid != resolved_aid {
                            if let Some(meta) = snap.track_cache.get(prev_aid).cloned() {
                                snap.track_cache.entry(resolved_aid.clone()).or_insert(meta);
                            }
                        }
                    }
                    if pqi_id.is_some() {
                        snap.pqi_id = pqi_id;
                    }
                    if adam_id.is_some() {
                        snap.adam_id = adam_id;
                    }
                    if dur > 0.0 && snap.duration_sec <= 0.0 {
                        snap.duration_sec = dur;
                    }
                    snap.transition_from_pqi = None;
                    snap.pending_play_tracks = false;
                    snap.resolved_epoch = snap.cmd_epoch.max(1);
                    snap.active = true;
                    snap.recompute_current_stream_specs();
                    AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                } else {
                    let recently_started = snap
                        .track_started_at
                        .is_some_and(|t0| t0.elapsed() < Duration::from_secs(6));
                    let pqi_switched =
                        pqi_id.is_some() && snap.pqi_id.is_some() && snap.pqi_id != pqi_id;
                    let aid_switched_without_pqi = pqi_id.is_none()
                        && adam_id.is_some()
                        && snap.adam_id != adam_id
                        && !recently_started;
                    let track_changed = pqi_switched || aid_switched_without_pqi;

                    if snap.pqi_id.is_none() && pqi_id.is_some() && recently_started {
                        if let (Some(ref prev_aid), Some(ref resolved_aid)) =
                            (&snap.adam_id.clone(), &adam_id)
                        {
                            if prev_aid != resolved_aid {
                                if let Some(meta) = snap.track_cache.get(prev_aid).cloned() {
                                    snap.track_cache.entry(resolved_aid.clone()).or_insert(meta);
                                }
                            }
                        }
                    }

                    snap.pqi_id = pqi_id;
                    if snap.pending_play_tracks {
                        snap.pending_play_tracks = false;
                        snap.transition_from_pqi = None;
                        snap.requested_track_id = None;
                        snap.duration_sec = if dur > 0.0 { dur } else { snap.duration_sec };
                        snap.resolved_epoch = snap
                            .cmd_epoch
                            .max(PLAY_CMD_EPOCH.load(Ordering::Acquire))
                            .max(1);
                    } else if track_changed {
                        let keep_playing = snap.desired_playing || snap.status == "Playing";
                        snap.desired_playing = keep_playing;
                        snap.anchor_pos_sec = 0.0;
                        snap.awaiting_audio_onset = keep_playing;
                        snap.anchor_instant = None;
                        snap.track_started_at = Some(Instant::now());
                        snap.play_btn_shows_pause = keep_playing;
                        snap.status = if keep_playing {
                            snap.audio_onset_watch_epoch =
                                crate::audio_viz::reset_amp_agent_audio_onset();
                            "Playing".to_string()
                        } else {
                            snap.audio_onset_watch_epoch = 0;
                            "Paused".to_string()
                        };
                        snap.transition_from_pqi = None;
                        snap.requested_track_id = None;
                        snap.duration_sec = if dur > 0.0 { dur } else { 0.0 };
                        snap.artwork_url = None;
                        snap.audio_quality = None;
                        snap.live_agent_sample_rate = None;
                        snap.live_agent_bit_depth = None;
                        snap.live_agent_format = None;
                        snap.resolved_epoch = snap
                            .cmd_epoch
                            .max(PLAY_CMD_EPOCH.load(Ordering::Acquire))
                            .max(1);
                    } else if snap.resolved_epoch == 0 && pqi_id.is_some() {
                        snap.transition_from_pqi = None;
                        snap.resolved_epoch = snap
                            .cmd_epoch
                            .max(PLAY_CMD_EPOCH.load(Ordering::Acquire))
                            .max(1);
                    }
                    snap.active = true;

                    let lookup_aid = adam_id.as_ref().or(snap.adam_id.as_ref()).cloned();
                    let cached = lookup_aid
                        .as_ref()
                        .and_then(|id| snap.track_cache.get(id))
                        .cloned();

                    if let Some(aid) = adam_id {
                        snap.adam_id = Some(aid);
                    } else if track_changed && !line1.is_empty() && line1 != snap.title {
                        snap.adam_id = None;
                    }
                    if let Some(meta) = cached {
                        snap.title = if !meta.title.is_empty() {
                            meta.title
                        } else {
                            line1
                        };
                        snap.artist = meta.artist;
                        snap.album = meta.album;
                        snap.artwork_url = meta.artwork_url;
                        snap.audio_quality = meta.audio_quality;
                        snap.duration_sec = if dur > 0.0 { dur } else { meta.duration_sec };
                    } else {
                        if !line1.is_empty() {
                            snap.title = line1;
                        }
                        if !line2.is_empty() {
                            if let Some((ar, al)) = line2.split_once(" — ") {
                                snap.artist = ar.to_string();
                                snap.album = al.to_string();
                            } else {
                                snap.artist = line2;
                            }
                        }
                        if dur > 0.0 {
                            snap.duration_sec = dur;
                        }
                    }
                    snap.recompute_current_stream_specs();
                    if track_changed {
                        AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                    }
                }
            } else if Instant::now() > self.grace_until && snap.active {
                snap.active = false;
                snap.desired_playing = false;
                snap.awaiting_audio_onset = false;
                snap.audio_onset_watch_epoch = 0;
                snap.status = "Stopped".to_string();
                snap.anchor_pos_sec = 0.0;
                snap.anchor_instant = None;
                snap.pqi_id = None;
                snap.stream_specs = None;
                crate::audio_viz::clear_amp_agent_audio_onset_watch();
            }

            Ok(())
        }

        unsafe fn resume_queue_on_fresh_session(
            &mut self,
            preferred_aid: Option<&str>,
            resume_sec: f64,
        ) -> Result<PlistValue, String> {
            let queue = if !self.last_queue.is_empty() {
                self.last_queue.clone()
            } else {
                SHARED
                    .read()
                    .ok()
                    .map(|s| s.queue_tracks.iter().take(120).cloned().collect())
                    .unwrap_or_default()
            };
            if queue.is_empty() {
                return self.exec_playback_cmd(5001, None);
            }
            let selected_idx = preferred_aid
                .and_then(|aid| queue.iter().position(|t| t.effective_adam_id() == aid))
                .unwrap_or(0);
            let items_plist: Vec<PlistValue> = queue.iter().map(build_store_item_plist).collect();
            self.last_queue = queue;
            self.grace_until = Instant::now() + Duration::from_secs(6);
            let params = PlistValue::dict([
                (
                    "storeParams",
                    PlistValue::dict([
                        ("items", PlistValue::Array(items_plist)),
                        ("selectedTrackIndex", PlistValue::Int(selected_idx as i64)),
                    ]),
                ),
                ("action", PlistValue::String("play".to_string())),
            ]);
            let resp = self.exec_playback_cmd(5000, Some(params))?;
            if resp.get("err").and_then(PlistValue::as_i64).unwrap_or(0) == 0 && resume_sec > 1.5 {
                self.wait_until_player_seekable(2_400);
                let _ = self.exec_playback_cmd(
                    5011,
                    Some(PlistValue::dict([("currentTime", PlistValue::Real(resume_sec))])),
                );
            }
            Ok(resp)
        }

        unsafe fn shutdown(mut self) {
            let _ = self.exec_playback_cmd(5003 /* Stop */, None);
            if self.pbcb_tok.value != 0 {
                let _ = ((*(*self.lib).lp_vtbl).remove_playback_event_with_callback_handler)(
                    self.lib,
                    self.pbcb_tok,
                );
            }
            if self.pb_tok.value != 0 {
                let _ = ((*(*self.lib).lp_vtbl).remove_playback_event_handler)(
                    self.lib,
                    self.pb_tok,
                );
            }
            let mut dummy: i32 = 0;
            if self.client_id != 0 {
                let _ = ((*(*self.lib).lp_vtbl).close_domains_for_client_id)(
                    self.lib,
                    self.client_id,
                    &mut dummy,
                );
            }
            let _ = ((*(*self.lib).lp_vtbl).media_app_quitting)(self.lib, &mut dummy);
            let _ = ((*(*self.lib).lp_vtbl).cleanup)(self.lib, self.my_pid, &mut dummy);
            ACTIVE_LIB_PTR.store(null_mut(), Ordering::Release);
            let _ = ((*(*self.lib).lp_vtbl).release)(self.lib);
            self.lib = null_mut();

            if let Ok(mut snap) = SHARED.write() {
                snap.connected = false;
                snap.active = false;
                snap.desired_playing = false;
                snap.awaiting_audio_onset = false;
                snap.audio_onset_watch_epoch = 0;
                snap.status = "Stopped".to_string();
                snap.agent_pid = None;
                snap.anchor_pos_sec = 0.0;
                snap.anchor_instant = None;
                snap.pqi_id = None;
            }
            crate::audio_viz::clear_amp_agent_audio_onset_watch();
        }
    }

    unsafe fn try_complete_audio_onset_from_meter() -> bool {
        let onset_ep = SHARED
            .read()
            .ok()
            .and_then(|s| {
                (s.awaiting_audio_onset && s.desired_playing)
                    .then_some(s.audio_onset_watch_epoch)
            })
            .unwrap_or(0);
        if onset_ep == 0 || !crate::audio_viz::has_amp_agent_audio_onset_since(onset_ep) {
            return false;
        }
        let mut promoted = false;
        if let Ok(mut snap) = SHARED.write() {
            if snap.awaiting_audio_onset && snap.desired_playing {
                snap.awaiting_audio_onset = false;
                snap.scrubber_indeterminate = false;
                snap.play_btn_shows_pause = true;
                snap.status = "Playing".to_string();
                snap.active = true;
                if snap.anchor_instant.is_none() {
                    snap.anchor_instant = Some(Instant::now());
                }
                if snap.resolved_epoch == 0 {
                    snap.resolved_epoch = snap
                        .cmd_epoch
                        .max(PLAY_CMD_EPOCH.load(Ordering::Acquire))
                        .max(1);
                }
                promoted = true;
            }
        }
        if promoted {
            AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
            crate::audio_viz::wake_registered_ui_window();
        }
        promoted
    }

    static WORKER_TX: Lazy<Mutex<Option<Sender<WorkerCmd>>>> = Lazy::new(|| Mutex::new(None));

    fn get_worker_sender() -> Result<Sender<WorkerCmd>, String> {
        let mut guard = WORKER_TX
            .lock()
            .map_err(|_| "Agent 线程锁异常".to_string())?;
        if let Some(tx) = guard.as_ref() {
            return Ok(tx.clone());
        }

        let (tx, rx) = mpsc::channel::<WorkerCmd>();
        std::thread::Builder::new()
            .name("am-playback-service".to_string())
            .stack_size(512 * 1024)
            .spawn(move || unsafe {
                init_com_and_security();
                let mut session: Option<AgentSession> = None;
                let mut last_queue_sync = Instant::now();

                loop {
                    let timeout = if session.is_some() {
                        let (awaiting, active_playing) = SHARED
                            .read()
                            .map(|s| {
                                (
                                    s.awaiting_audio_onset && s.desired_playing,
                                    s.active && s.status == "Playing",
                                )
                            })
                            .unwrap_or((false, false));
                        if awaiting {
                            Duration::from_millis(50)
                        } else if active_playing {
                            Duration::from_millis(950)
                        } else {
                            Duration::from_secs(30)
                        }
                    } else {
                        Duration::from_secs(30)
                    };

                    match rx.recv_timeout(timeout) {
                        Ok(WorkerCmd::EnsureConnected { reply }) => {
                            if session.is_none() {
                                match AgentSession::connect() {
                                    Ok(s) => session = Some(s),
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                        continue;
                                    }
                                }
                            }
                            let _ = reply.send(Ok(get_agent_status()));
                        }
                        Ok(WorkerCmd::PlayTracks {
                            tracks,
                            start_index,
                            reply,
                        }) => {
                            if tracks.is_empty() {
                                let _ = reply.send(Err("无可播放曲目".to_string()));
                                continue;
                            }
                            if session.is_none() {
                                match AgentSession::connect() {
                                    Ok(s) => session = Some(s),
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                        continue;
                                    }
                                }
                            }

                            // Slice window of up to 120 tracks around start_index for fast IPC
                            let safe_idx = start_index.min(tracks.len() - 1);
                            let win_start = safe_idx.saturating_sub(25);
                            let win_end = (win_start + 120).min(tracks.len());
                            let rel_selected = safe_idx - win_start;
                            let windowed = &tracks[win_start..win_end];

                            if let Ok(mut snap) = SHARED.write() {
                                snap.queue_tracks = tracks.clone();
                                for t in &tracks {
                                    let aid = t.effective_adam_id();
                                    if !aid.is_empty() {
                                        if !t.audio_traits.is_empty() {
                                            snap.track_traits_cache
                                                .insert(aid.clone(), t.audio_traits.clone());
                                        }
                                        snap.track_cache.insert(
                                            aid,
                                            CachedTrackMeta {
                                                title: t.name.clone(),
                                                artist: t.artist_name.clone(),
                                                album: t.album_name.clone(),
                                                duration_sec: t.duration_sec,
                                                artwork_url: t.artwork_url.clone(),
                                                audio_quality: format_audio_traits_badge(&t.audio_traits),
                                            },
                                        );
                                    }
                                }
                            }

                            let items_plist: Vec<PlistValue> =
                                windowed.iter().map(build_store_item_plist).collect();

                            let chosen = &tracks[safe_idx];
                            let cmd_ep = PLAY_CMD_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
                            let onset_ep = crate::audio_viz::reset_amp_agent_audio_onset();
                            // Initialize clean pre-buffering state BEFORE `exec_playback_cmd(5000)` while
                            // preserving `transition_from_pqi = snap.pqi_id` so out-of-proc `pb_invoke`
                            // callbacks from the outgoing track are strictly rejected!
                            if let Ok(mut snap) = SHARED.write() {
                                snap.transition_from_pqi = snap.pqi_id;
                                snap.cmd_in_flight = true;
                                snap.pending_play_tracks = true;
                                snap.cmd_epoch = cmd_ep;
                                snap.resolved_epoch = 0;
                                snap.desired_playing = true;
                                snap.audio_onset_watch_epoch = onset_ep;
                                snap.play_btn_shows_pause = false;
                                snap.seeking_in_flight = 0;
                                snap.optimistic_seek_target = None;
                                snap.active = true;
                                snap.status = "Paused".to_string();
                                snap.requested_track_id = Some(chosen.id.clone());
                                snap.adam_id = Some(chosen.effective_adam_id());
                                snap.current_descriptor = None;
                                snap.title = chosen.name.clone();
                                snap.artist = chosen.artist_name.clone();
                                snap.album = chosen.album_name.clone();
                                snap.duration_sec = chosen.duration_sec;
                                snap.artwork_url = chosen.artwork_url.clone();
                                snap.audio_quality = format_audio_traits_badge(&chosen.audio_traits);
                                snap.live_agent_sample_rate = None;
                                snap.live_agent_bit_depth = None;
                                snap.live_agent_format = None;
                                snap.anchor_pos_sec = 0.0;
                                snap.anchor_instant = None;
                                snap.track_started_at = Some(Instant::now());
                                snap.awaiting_audio_onset = true;
                                snap.scrubber_indeterminate = true;
                                snap.can_be_positioned = false;
                                snap.pending_reload_seek = None;
                                snap.recompute_current_stream_specs();
                            }
                            AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);

                            let params = PlistValue::dict([
                                (
                                    "storeParams",
                                    PlistValue::dict([
                                        ("items", PlistValue::Array(items_plist)),
                                        (
                                            "selectedTrackIndex",
                                            PlistValue::Int(rel_selected as i64),
                                        ),
                                    ]),
                                ),
                                ("action", PlistValue::String("play".to_string())),
                            ]);

                            let mut cmd_res = if let Some(sess) = session.as_mut() {
                                sess.last_queue = windowed.to_vec();
                                sess.grace_until = Instant::now() + Duration::from_secs(6);
                                sess.exec_playback_cmd(5000, Some(params.clone()))
                            } else {
                                Err("会话未初始化".to_string())
                            };

                            // Auto-reconnect once if RPC server restarted
                            if cmd_res.is_err() {
                                session = None;
                                if let Ok(mut fresh) = AgentSession::connect() {
                                    fresh.last_queue = windowed.to_vec();
                                    fresh.grace_until = Instant::now() + Duration::from_secs(6);
                                    cmd_res = fresh.exec_playback_cmd(5000, Some(params));
                                    session = Some(fresh);
                                }
                            }

                            if let Ok(mut snap) = SHARED.write() {
                                snap.cmd_in_flight = false;
                            }

                            match cmd_res {
                                Ok(resp) => {
                                    let err_code = resp.get("err").and_then(PlistValue::as_i64).unwrap_or(0);
                                    if err_code != 0 {
                                        if let Ok(mut snap) = SHARED.write() {
                                            snap.status = "Stopped".to_string();
                                            snap.active = false;
                                            snap.desired_playing = false;
                                            snap.awaiting_audio_onset = false;
                                            snap.audio_onset_watch_epoch = 0;
                                            snap.scrubber_indeterminate = false;
                                            snap.pending_play_tracks = false;
                                            snap.transition_from_pqi = None;
                                        }
                                        crate::audio_viz::clear_amp_agent_audio_onset_watch();
                                        AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                                        let _ = reply.send(Err(format!(
                                            "AMPLibraryAgent 返回错误码 err={err_code}（请确认桌面版 Apple Music 已登录订阅账号）"
                                        )));
                                        continue;
                                    }
                                    let cur_vol = SHARED.read().map(|s| s.volume).unwrap_or(0.75);
                                    if let Some(sess) = session.as_mut() {
                                        let _ = sess.exec_playback_cmd(
                                            5013,
                                            Some(PlistValue::dict([(
                                                "volume",
                                                PlistValue::Real(cur_vol.clamp(0.0, 1.0)),
                                            )])),
                                        );
                                        // Sync initial `pqiID` & `descriptor` right away so Command 5023 (`GetLyrics`)
                                        // and gapless queue tracking have the exact item descriptor immediately!
                                        let _ = sess.sync_queue_state();
                                    }
                                    let _ = try_complete_audio_onset_from_meter();
                                    last_queue_sync = Instant::now();
                                    let state = if let Ok(mut snap) = SHARED.write() {
                                        snap.active = true;
                                        if snap.resolved_epoch == 0 {
                                            snap.resolved_epoch = cmd_ep;
                                            snap.transition_from_pqi = None;
                                            snap.pending_play_tracks = false;
                                        }
                                        snap.recompute_current_stream_specs();
                                        snap.to_smtc_state()
                                    } else {
                                        SmtcState::default()
                                    };
                                    AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                                    let _ = reply.send(Ok(state));
                                }
                                Err(e) => {
                                    if let Ok(mut snap) = SHARED.write() {
                                        snap.status = "Stopped".to_string();
                                        snap.active = false;
                                        snap.desired_playing = false;
                                        snap.awaiting_audio_onset = false;
                                        snap.audio_onset_watch_epoch = 0;
                                        snap.scrubber_indeterminate = false;
                                        snap.pending_play_tracks = false;
                                        snap.transition_from_pqi = None;
                                    }
                                    crate::audio_viz::clear_amp_agent_audio_onset_watch();
                                    AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                                    let _ = reply.send(Err(e));
                                }
                            }
                        }
                        Ok(WorkerCmd::GetPrefs { reply }) => {
                            if session.is_none() {
                                match AgentSession::connect() {
                                    Ok(s) => session = Some(s),
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                        continue;
                                    }
                                }
                            }
                            let res = if let Some(sess) = session.as_mut() {
                                sess.sync_prefs_from_agent()
                            } else {
                                Err("播放服务未连接".to_string())
                            };
                            let _ = reply.send(res);
                        }
                        Ok(WorkerCmd::SetPrefs { prefs, reply }) => {
                            if session.is_none() {
                                match AgentSession::connect() {
                                    Ok(s) => session = Some(s),
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                        continue;
                                    }
                                }
                            }
                            let res = if let Some(sess) = session.as_mut() {
                                sess.apply_prefs_to_agent(prefs)
                            } else {
                                Err("播放服务未连接".to_string())
                            };
                            let _ = reply.send(res);
                        }
                        Ok(WorkerCmd::DownloadQueueAction { pause, reply }) => {
                            if session.is_none() {
                                match AgentSession::connect() {
                                    Ok(s) => session = Some(s),
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                        continue;
                                    }
                                }
                            }
                            let cmd_id = if pause { 1005 } else { 1006 };
                            let res = if let Some(sess) = session.as_mut() {
                                sess.exec_subsystem_cmd(4, cmd_id, None).map(|_| ())
                            } else {
                                Err("播放服务未连接".to_string())
                            };
                            let _ = reply.send(res);
                        }
                        Ok(WorkerCmd::Control {
                            action,
                            value,
                            reply,
                        }) => {
                            let (prev_status, prev_aid, prev_pos, last_queue_backup) = {
                                let snap_ctx = SHARED
                                    .read()
                                    .ok()
                                    .map(|s| {
                                        (
                                            s.status.clone(),
                                            s.adam_id.clone(),
                                            s.current_position_sec(),
                                        )
                                    })
                                    .unwrap_or_else(|| ("Stopped".to_string(), None, 0.0));
                                let q = session
                                    .as_ref()
                                    .map(|s| s.last_queue.clone())
                                    .unwrap_or_default();
                                (snap_ctx.0, snap_ctx.1, snap_ctx.2, q)
                            };

                            let mut just_connected = false;
                            if session.is_none() {
                                match AgentSession::connect() {
                                    Ok(mut s) => {
                                        s.last_queue = last_queue_backup.clone();
                                        session = Some(s);
                                        just_connected = true;
                                    }
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                        continue;
                                    }
                                }
                            }

                            let mut effective_action = if action == "toggle" {
                                if just_connected || prev_status != "Playing" {
                                    "play"
                                } else {
                                    "pause"
                                }
                            } else {
                                action.as_str()
                            };

                            let mut res = if let Some(sess) = session.as_mut() {
                                match effective_action {
                                    "play" => {
                                        if just_connected {
                                            sess.resume_queue_on_fresh_session(
                                                prev_aid.as_deref(),
                                                prev_pos,
                                            )
                                        } else {
                                            sess.exec_playback_cmd(5001, None)
                                        }
                                    }
                                    "pause" => sess.exec_playback_cmd(5002, None),
                                    "stop" => sess.exec_playback_cmd(5003, None),
                                    "prev" => {
                                        let cmd_ep =
                                            PLAY_CMD_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
                                        let onset_ep =
                                            crate::audio_viz::reset_amp_agent_audio_onset();
                                        if let Ok(mut snap) = SHARED.write() {
                                            snap.transition_from_pqi = snap.pqi_id;
                                            snap.cmd_in_flight = true;
                                            snap.pending_play_tracks = false;
                                            snap.cmd_epoch = cmd_ep;
                                            snap.resolved_epoch = 0;
                                            snap.desired_playing = true;
                                            snap.audio_onset_watch_epoch = onset_ep;
                                            snap.play_btn_shows_pause = false;
                                            snap.seeking_in_flight = 0;
                                            snap.optimistic_seek_target = None;
                                            snap.anchor_pos_sec = 0.0;
                                            snap.anchor_instant = None;
                                            snap.track_started_at = Some(Instant::now());
                                            snap.awaiting_audio_onset = true;
                                            snap.scrubber_indeterminate = true;
                                            snap.can_be_positioned = false;
                                            snap.status = "Paused".to_string();
                                        }
                                        AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                                        sess.grace_until = Instant::now() + Duration::from_secs(5);
                                        sess.exec_playback_cmd(5006, None)
                                    }
                                    "next" => {
                                        let cmd_ep =
                                            PLAY_CMD_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
                                        let onset_ep =
                                            crate::audio_viz::reset_amp_agent_audio_onset();
                                        if let Ok(mut snap) = SHARED.write() {
                                            snap.transition_from_pqi = snap.pqi_id;
                                            snap.cmd_in_flight = true;
                                            snap.pending_play_tracks = false;
                                            snap.cmd_epoch = cmd_ep;
                                            snap.resolved_epoch = 0;
                                            snap.desired_playing = true;
                                            snap.audio_onset_watch_epoch = onset_ep;
                                            snap.play_btn_shows_pause = false;
                                            snap.seeking_in_flight = 0;
                                            snap.optimistic_seek_target = None;
                                            snap.anchor_pos_sec = 0.0;
                                            snap.anchor_instant = None;
                                            snap.track_started_at = Some(Instant::now());
                                            snap.awaiting_audio_onset = true;
                                            snap.scrubber_indeterminate = true;
                                            snap.can_be_positioned = false;
                                            snap.status = "Paused".to_string();
                                        }
                                        AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                                        sess.grace_until = Instant::now() + Duration::from_secs(5);
                                        sess.exec_playback_cmd(5007, None)
                                    }
                                    "seek" => {
                                        let sec = value.unwrap_or(0.0).max(0.0);
                                        if let Ok(mut snap) = SHARED.write() {
                                            snap.seeking_in_flight =
                                                snap.seeking_in_flight.saturating_add(1);
                                            snap.anchor_pos_sec = sec;
                                            snap.anchor_instant = None;
                                            snap.optimistic_seek_target = Some((
                                                sec,
                                                Instant::now() + Duration::from_millis(2400),
                                            ));
                                        }
                                        let needs_wait = !SHARED
                                            .read()
                                            .map(|s| s.can_be_positioned)
                                            .unwrap_or(true)
                                            && Instant::now() < sess.grace_until;
                                        if needs_wait {
                                            sess.wait_until_player_seekable(2_400);
                                        }
                                        let seek_params = PlistValue::dict([(
                                            "currentTime",
                                            PlistValue::Real(sec),
                                        )]);
                                        let r5011 =
                                            sess.exec_playback_cmd(5011, Some(seek_params.clone()));
                                        match r5011 {
                                            Ok(ref p)
                                                if p.get("err")
                                                    .and_then(PlistValue::as_i64)
                                                    .unwrap_or(0)
                                                    == 0 =>
                                            {
                                                r5011
                                            }
                                            _ => sess.exec_playback_cmd(5010, Some(seek_params)),
                                        }
                                    }
                                    "volume" => {
                                        let vol = value.unwrap_or(0.75).clamp(0.0, 1.0);
                                        sess.exec_playback_cmd(
                                            5013,
                                            Some(PlistValue::dict([(
                                                "volume",
                                                PlistValue::Real(vol),
                                            )])),
                                        )
                                    }
                                    other => Err(format!("未知播放控制动作: {other}")),
                                }
                            } else {
                                Err("播放服务尚未启动".to_string())
                            };

                            // Seamless auto-reconnect once if AMPLibraryAgent died/restarted externally
                            let mut reconnected_fresh = just_connected;
                            if res.is_err()
                                && !just_connected
                                && matches!(
                                    effective_action,
                                    "play" | "pause" | "stop" | "prev" | "next" | "seek" | "volume"
                                )
                            {
                                session = None;
                                ACTIVE_LIB_PTR.store(null_mut(), Ordering::Release);
                                if let Ok(mut fresh) = AgentSession::connect() {
                                    fresh.last_queue = last_queue_backup;
                                    reconnected_fresh = true;
                                    if action == "toggle" {
                                        effective_action = "play";
                                    }
                                    res = match effective_action {
                                        "play" | "next" | "prev" => fresh
                                            .resume_queue_on_fresh_session(
                                                prev_aid.as_deref(),
                                                prev_pos,
                                            ),
                                        "pause" | "stop" => Ok(PlistValue::Null),
                                        "seek" => fresh.resume_queue_on_fresh_session(
                                            prev_aid.as_deref(),
                                            value.unwrap_or(0.0).max(0.0),
                                        ),
                                        "volume" => {
                                            let vol = value.unwrap_or(0.75).clamp(0.0, 1.0);
                                            fresh.exec_playback_cmd(
                                                5013,
                                                Some(PlistValue::dict([(
                                                    "volume",
                                                    PlistValue::Real(vol),
                                                )])),
                                            )
                                        }
                                        _ => Err("未知播放控制动作".to_string()),
                                    };
                                    session = Some(fresh);
                                }
                            }

                            if matches!(effective_action, "next" | "prev") {
                                if let Ok(mut snap) = SHARED.write() {
                                    snap.cmd_in_flight = false;
                                }
                            }

                            match res {
                                Ok(_) => {
                                    if let Ok(mut snap) = SHARED.write() {
                                        match effective_action {
                                            "play" => {
                                                snap.desired_playing = true;
                                                snap.play_btn_shows_pause = true;
                                                snap.status = "Playing".to_string();
                                                snap.active = true;
                                                if reconnected_fresh
                                                    || snap.anchor_pos_sec > 0.02
                                                    || snap.can_be_positioned
                                                {
                                                    snap.awaiting_audio_onset = false;
                                                    snap.scrubber_indeterminate = false;
                                                    if snap.resolved_epoch == 0 {
                                                        snap.resolved_epoch = snap
                                                            .cmd_epoch
                                                            .max(PLAY_CMD_EPOCH.load(Ordering::Acquire))
                                                            .max(1);
                                                    }
                                                }
                                                if snap.anchor_instant.is_none()
                                                    && !snap.awaiting_audio_onset
                                                    && !snap.scrubber_indeterminate
                                                    && snap.seeking_in_flight == 0
                                                {
                                                    snap.anchor_instant = Some(Instant::now());
                                                }
                                            }
                                            "pause" => {
                                                let cur = snap.current_position_sec();
                                                snap.desired_playing = false;
                                                snap.awaiting_audio_onset = false;
                                                snap.audio_onset_watch_epoch = 0;
                                                snap.anchor_pos_sec = cur;
                                                snap.anchor_instant = None;
                                                snap.play_btn_shows_pause = false;
                                                snap.status = "Paused".to_string();
                                                crate::audio_viz::clear_amp_agent_audio_onset_watch();
                                            }
                                            "stop" => {
                                                snap.active = false;
                                                snap.desired_playing = false;
                                                snap.status = "Stopped".to_string();
                                                snap.anchor_pos_sec = 0.0;
                                                snap.anchor_instant = None;
                                                snap.awaiting_audio_onset = false;
                                                snap.audio_onset_watch_epoch = 0;
                                                snap.scrubber_indeterminate = false;
                                                snap.play_btn_shows_pause = false;
                                                snap.seeking_in_flight = 0;
                                                snap.stream_specs = None;
                                                crate::audio_viz::clear_amp_agent_audio_onset_watch();
                                            }
                                            "seek" => {
                                                let sec = value.unwrap_or(0.0).max(0.0);
                                                snap.seeking_in_flight =
                                                    snap.seeking_in_flight.saturating_sub(1);
                                                snap.awaiting_audio_onset = false;
                                                snap.scrubber_indeterminate = false;
                                                if snap.seeking_in_flight == 0 {
                                                    snap.optimistic_seek_target = None;
                                                    if snap.anchor_instant.is_none()
                                                        || (snap.current_position_sec() - sec).abs()
                                                            > 0.30
                                                    {
                                                        snap.anchor_pos_sec = sec;
                                                        snap.anchor_instant =
                                                            (snap.status == "Playing")
                                                                .then(Instant::now);
                                                    }
                                                }
                                            }
                                            "volume" => {
                                                snap.volume = value.unwrap_or(0.75).clamp(0.0, 1.0);
                                            }
                                            "next" | "prev" => {
                                                if reconnected_fresh {
                                                    snap.desired_playing = true;
                                                    snap.play_btn_shows_pause = true;
                                                    snap.awaiting_audio_onset = false;
                                                    snap.scrubber_indeterminate = false;
                                                    snap.status = "Playing".to_string();
                                                    snap.anchor_instant = Some(Instant::now());
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    if matches!(effective_action, "next" | "prev")
                                        || reconnected_fresh
                                    {
                                        if let Some(sess) = session.as_mut() {
                                            std::thread::sleep(Duration::from_millis(140));
                                            let _ = sess.sync_queue_state();
                                            last_queue_sync = Instant::now();
                                        }
                                        if reconnected_fresh
                                            && matches!(effective_action, "play" | "next" | "prev")
                                        {
                                            if let Ok(mut snap) = SHARED.write() {
                                                snap.desired_playing = true;
                                                snap.play_btn_shows_pause = true;
                                                snap.awaiting_audio_onset = false;
                                                snap.scrubber_indeterminate = false;
                                                snap.status = "Playing".to_string();
                                                if snap.anchor_instant.is_none() {
                                                    snap.anchor_instant = Some(Instant::now());
                                                }
                                            }
                                        }
                                    }
                                    let _ = try_complete_audio_onset_from_meter();
                                    AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
                                    let _ = reply.send(Ok(get_playback_state()));
                                }
                                Err(e) => {
                                    if effective_action == "seek" {
                                        if let Ok(mut snap) = SHARED.write() {
                                            snap.seeking_in_flight =
                                                snap.seeking_in_flight.saturating_sub(1);
                                            if snap.seeking_in_flight == 0
                                                && snap.status == "Playing"
                                                && !snap.awaiting_audio_onset
                                                && !snap.scrubber_indeterminate
                                                && snap.anchor_instant.is_none()
                                            {
                                                snap.anchor_instant = Some(Instant::now());
                                            }
                                        }
                                    }
                                    let _ = reply.send(Err(e));
                                }
                            }
                        }
                        Ok(WorkerCmd::SaveAgentLog {
                            folder,
                            filename,
                            reply,
                        }) => {
                            let Some(sess) = session.as_mut() else {
                                let _ = reply.send(Err("播放服务未连接".to_string()));
                                continue;
                            };
                            let res = (|| -> Result<String, String> {
                                let h_dir = rust_to_hstring(&folder)?;
                                let h_name = rust_to_hstring(&filename)?;
                                let mut op: *mut c_void = null_mut();
                                let hr = ((*(*sess.lib).lp_vtbl).save_agent_log_to_file_async)(
                                    sess.lib,
                                    h_dir,
                                    h_name,
                                    &mut op,
                                );
                                let _ = WindowsDeleteString(h_dir);
                                let _ = WindowsDeleteString(h_name);
                                if hr < 0 {
                                    return Err(format!(
                                        "SaveAgentLogToFileAsync 失败: 0x{:08X}",
                                        hr as u32
                                    ));
                                }
                                await_async_hstring(op, 10_000)
                            })();
                            let _ = reply.send(res);
                        }
                        Ok(WorkerCmd::GetLyrics {
                            adam_id,
                            connect_if_needed,
                            reply,
                        }) => {
                            let cached_hit = SHARED.read().ok().and_then(|s| {
                                s.agent_lyrics_cache.get(&adam_id).cloned().or_else(|| {
                                    if s.requested_track_id.as_deref() == Some(adam_id.as_str()) {
                                        s.adam_id
                                            .as_ref()
                                            .and_then(|aid| s.agent_lyrics_cache.get(aid).cloned())
                                    } else {
                                        None
                                    }
                                })
                            });
                            if let Some(hit) = cached_hit {
                                let _ = reply.send(Ok(Some(hit)));
                                continue;
                            }
                            if session.is_none() {
                                if !connect_if_needed {
                                    let _ = reply.send(Ok(None));
                                    continue;
                                }
                                match AgentSession::connect() {
                                    Ok(s) => session = Some(s),
                                    Err(e) => {
                                        let _ = reply.send(Err(e));
                                        continue;
                                    }
                                }
                            }
                            let Some(sess) = session.as_mut() else {
                                let _ = reply.send(Ok(None));
                                continue;
                            };

                            // Build exact `descriptor` for Command 5023 (`DoGetLyricsAsync@AMPPlaybackAgentShim`):
                            // Prefer the active queue item's descriptor when `storeAdamID` or `requested_track_id`
                            // (`i.xxxx`) matches `adam_id` (or when `adam_id` is non-numeric on an active item);
                            // otherwise construct a store song descriptor from `adam_id` (or resolved `s.adam_id`).
                            let is_numeric_req = adam_id.trim().bytes().all(|b| b.is_ascii_digit());
                            let maybe_desc = SHARED
                                .read()
                                .ok()
                                .and_then(|s| {
                                    let matches_active =
                                        s.adam_id.as_deref() == Some(adam_id.as_str())
                                            || s.requested_track_id.as_deref()
                                                == Some(adam_id.as_str())
                                            || (!is_numeric_req
                                                && s.active
                                                && s.current_descriptor.is_some());
                                    if matches_active {
                                        if let Some(d) = s.current_descriptor.clone() {
                                            return Some(d);
                                        }
                                        if let Some(resolved_i64) =
                                            s.adam_id.as_deref().and_then(|a| a.parse::<i64>().ok())
                                        {
                                            return Some(PlistValue::dict([
                                                (
                                                    "kind",
                                                    PlistValue::String("song".to_string()),
                                                ),
                                                (
                                                    "identifiers",
                                                    PlistValue::dict([(
                                                        "storeAdamID",
                                                        PlistValue::Int(resolved_i64),
                                                    )]),
                                                ),
                                            ]));
                                        }
                                    }
                                    None
                                })
                                .or_else(|| {
                                    let aid_i64 = adam_id.trim().parse::<i64>().ok()?;
                                    Some(PlistValue::dict([
                                        ("kind", PlistValue::String("song".to_string())),
                                        (
                                            "identifiers",
                                            PlistValue::dict([(
                                                "storeAdamID",
                                                PlistValue::Int(aid_i64),
                                            )]),
                                        ),
                                    ]))
                                });

                            let Some(desc) = maybe_desc else {
                                let _ = reply.send(Ok(None));
                                continue;
                            };

                            let req_params = PlistValue::dict([
                                ("descriptor", desc),
                                ("lyricsSource", PlistValue::String("store".to_string())),
                                (
                                    "metrics",
                                    PlistValue::dict([
                                        (
                                            "AppVersion",
                                            PlistValue::String("1.1540.23042.0".to_string()),
                                        ),
                                        (
                                            "AppUserAgentString",
                                            PlistValue::String(
                                                "AMPWin/1.5.4 (Windows; 10.0)".to_string(),
                                            ),
                                        ),
                                    ]),
                                ),
                            ]);

                            match sess.exec_playback_cmd(5023 /* GetLyrics */, Some(req_params)) {
                                Ok(resp) => {
                                    let err_code =
                                        resp.get("err").and_then(PlistValue::as_i64).unwrap_or(0);
                                    if err_code != 0 {
                                        let _ = reply.send(Ok(None));
                                        continue;
                                    }
                                    let rdict = resp.get("responseDict").unwrap_or(&resp);
                                    let ldict = rdict.get("lyricsInfo").unwrap_or(rdict);
                                    let st_str = ldict
                                        .get("lyricsState")
                                        .or_else(|| rdict.get("lyricsState"))
                                        .and_then(PlistValue::as_str)
                                        .unwrap_or("");
                                    let body_opt = ldict
                                        .get("lyrics")
                                        .or_else(|| rdict.get("lyrics"))
                                        .and_then(PlistValue::as_str)
                                        .map(|s| s.trim().to_string())
                                        .filter(|s| !s.is_empty());

                                    if let Some(body) = body_opt {
                                        let is_ttml = st_str.eq_ignore_ascii_case("ttml")
                                            || body.contains("<tt");
                                        if let Ok(mut snap) = SHARED.write() {
                                            snap.cache_lyrics_bounded(
                                                adam_id.clone(),
                                                body.clone(),
                                                is_ttml,
                                            );
                                            if snap.requested_track_id.as_deref()
                                                == Some(adam_id.as_str())
                                            {
                                                if let Some(resolved_aid) = snap.adam_id.clone() {
                                                    if resolved_aid != adam_id {
                                                        snap.cache_lyrics_bounded(
                                                            resolved_aid,
                                                            body.clone(),
                                                            is_ttml,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        let _ = reply.send(Ok(Some((body, is_ttml))));
                                    } else {
                                        let _ = reply.send(Ok(None));
                                    }
                                }
                                Err(_) => {
                                    let _ = reply.send(Ok(None));
                                }
                            }
                        }
                        Ok(WorkerCmd::Disconnect { reply }) => {
                            if let Some(sess) = session.take() {
                                sess.shutdown();
                            }
                            let _ = reply.send(Ok(()));
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = try_complete_audio_onset_from_meter();
                            let (is_active, need_fast_sync) = SHARED
                                .read()
                                .map(|s| {
                                    (
                                        s.active,
                                        s.active && (s.pending_play_tracks || s.pqi_id.is_none()),
                                    )
                                })
                                .unwrap_or((false, false));
                            if is_active
                                && (last_queue_sync.elapsed() >= Duration::from_millis(900)
                                    || (need_fast_sync
                                        && last_queue_sync.elapsed() >= Duration::from_millis(320)))
                            {
                                last_queue_sync = Instant::now();
                                if let Some(sess) = session.as_mut() {
                                    if sess.sync_queue_state().is_err() {
                                        session = None;
                                        ACTIVE_LIB_PTR.store(null_mut(), Ordering::Release);
                                        if let Ok(mut snap) = SHARED.write() {
                                            snap.connected = false;
                                            snap.active = false;
                                            snap.desired_playing = false;
                                            snap.awaiting_audio_onset = false;
                                            snap.audio_onset_watch_epoch = 0;
                                            snap.status = "Stopped".to_string();
                                        }
                                        crate::audio_viz::clear_amp_agent_audio_onset_watch();
                                    }
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            if let Some(sess) = session.take() {
                                sess.shutdown();
                            }
                            break;
                        }
                    }
                }
            })
            .map_err(|e| format!("启动播放服务工作线程失败: {e}"))?;

        *guard = Some(tx.clone());
        Ok(tx)
    }

    pub fn is_agent_active() -> bool {
        SHARED.read().map(|s| s.active).unwrap_or(false)
    }

    pub fn is_agent_connected() -> bool {
        SHARED.read().map(|s| s.connected).unwrap_or(false)
    }

    pub fn get_playback_state() -> SmtcState {
        SHARED
            .read()
            .map(|s| s.to_smtc_state())
            .unwrap_or_default()
    }

    pub fn get_agent_status() -> AgentStatus {
        let apple_music_ui_running = !list_matching_pids("AppleMusic.exe").is_empty();
        let live_agent_pid = list_matching_pids("AMPLibraryAgent.exe").into_iter().next();
        if let Ok(snap) = SHARED.read() {
            AgentStatus {
                connected: snap.connected,
                agent_pid: snap.agent_pid.or(live_agent_pid),
                apple_music_ui_running,
                client_id: snap.client_id,
                state_code: snap.state_code,
                volume: snap.volume,
            }
        } else {
            AgentStatus {
                apple_music_ui_running,
                agent_pid: live_agent_pid,
                volume: 0.75,
                ..AgentStatus::default()
            }
        }
    }

    pub fn ensure_agent_connected() -> Result<AgentStatus, String> {
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::EnsureConnected { reply: r_tx })
            .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(25))
            .map_err(|_| "初始化 AMPLibraryAgent 超时".to_string())?
    }

    pub fn get_audio_prefs(connect_if_needed: bool) -> Result<AudioPrefs, String> {
        if !connect_if_needed && !is_agent_connected() {
            return Ok(SHARED.read().map(|s| s.prefs.clone()).unwrap_or_default());
        }
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::GetPrefs { reply: r_tx })
            .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(20))
            .map_err(|_| "读取音质配置超时".to_string())?
    }

    pub fn set_audio_prefs(prefs: AudioPrefs) -> Result<AudioPrefs, String> {
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::SetPrefs {
            prefs,
            reply: r_tx,
        })
        .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(20))
            .map_err(|_| "保存音质配置超时".to_string())?
    }

    pub fn control_downloads_queue(pause: bool) -> Result<(), String> {
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::DownloadQueueAction {
            pause,
            reply: r_tx,
        })
        .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(15))
            .map_err(|_| "控制离线下载队列超时".to_string())?
    }

    pub fn play_tracks(
        tracks: Vec<AgentTrackInput>,
        start_index: usize,
    ) -> Result<SmtcState, String> {
        let _guard = enter_play_cmd_in_flight();
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::PlayTracks {
            tracks,
            start_index,
            reply: r_tx,
        })
        .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(25))
            .map_err(|_| "发送播放指令超时".to_string())?
    }

    pub fn control(action: &str, value: Option<f64>) -> Result<SmtcState, String> {
        let _guard = matches!(action, "next" | "prev").then(enter_play_cmd_in_flight);
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::Control {
            action: action.to_string(),
            value,
            reply: r_tx,
        })
        .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(15))
            .map_err(|_| "执行播放控制指令超时".to_string())?
    }

    pub fn save_agent_log(folder: &str, filename: &str) -> Result<String, String> {
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::SaveAgentLog {
            folder: folder.to_string(),
            filename: filename.to_string(),
            reply: r_tx,
        })
        .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(12))
            .map_err(|_| "导出诊断日志超时".to_string())?
    }

    /// Queries official Apple Music store TTML syllable-synced lyrics directly via `AMPLibraryAgent`
    /// Command 5023 (`GetLyrics` -> `bag://musicSubscription/ttmlLyrics`) using the user's native
    /// signed-in desktop Apple Music session — works with ZERO manual `Media-User-Token`!
    pub fn get_store_lyrics_via_agent(
        adam_id: &str,
        connect_if_needed: bool,
    ) -> Result<Option<(String, bool)>, String> {
        let clean_id = adam_id.trim();
        if clean_id.is_empty() {
            return Ok(None);
        }
        let check_cache = || -> Option<(String, bool)> {
            SHARED.read().ok().and_then(|s| {
                s.agent_lyrics_cache.get(clean_id).cloned().or_else(|| {
                    if s.requested_track_id.as_deref() == Some(clean_id) {
                        s.adam_id
                            .as_ref()
                            .and_then(|aid| s.agent_lyrics_cache.get(aid).cloned())
                    } else {
                        None
                    }
                })
            })
        };
        if let Some(hit) = check_cache() {
            return Ok(Some(hit));
        }

        let is_numeric_aid = clean_id.bytes().all(|b| b.is_ascii_digit());
        let recent_play_intent = || -> bool {
            let last_ms = LAST_PLAY_INTENT_MS.load(Ordering::Acquire);
            last_ms > 0 && mono_ms_now().saturating_sub(last_ms) < 2_200
        };
        let play_in_flight =
            || PLAY_CMD_IN_FLIGHT_COUNT.load(Ordering::Acquire) > 0 || recent_play_intent();

        if !connect_if_needed && !is_agent_connected() && !play_in_flight() {
            return Ok(None);
        }

        // Wait up to 850ms when cold-start `PlayTracks` is connecting or resolving an `i.xxxx` descriptor
        // so `GetLyrics` never races ahead of `PlayTracks` or hangs UI lyrics startup!
        let wait_start = Instant::now();
        while wait_start.elapsed() < Duration::from_millis(850) {
            if let Some(hit) = check_cache() {
                return Ok(Some(hit));
            }
            let connected = is_agent_connected();
            let in_flight = PLAY_CMD_IN_FLIGHT_COUNT.load(Ordering::Acquire) > 0;
            if is_numeric_aid {
                if connected && !in_flight {
                    break;
                }
                if !in_flight && !recent_play_intent() {
                    break;
                }
            } else {
                let has_active_desc = SHARED
                    .read()
                    .ok()
                    .is_some_and(|s| s.active && (s.current_descriptor.is_some() || s.adam_id.as_deref().is_some_and(|a| a.bytes().all(|b| b.is_ascii_digit()))));
                if connected && has_active_desc && !in_flight {
                    break;
                }
                if !in_flight && !recent_play_intent() {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(35));
        }

        if let Some(hit) = check_cache() {
            return Ok(Some(hit));
        }
        if !connect_if_needed && !is_agent_connected() {
            return Ok(None);
        }
        let tx = get_worker_sender()?;
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        tx.send(WorkerCmd::GetLyrics {
            adam_id: clean_id.to_string(),
            connect_if_needed,
            reply: r_tx,
        })
        .map_err(|_| "播放服务工作线程已停止".to_string())?;
        r_rx.recv_timeout(Duration::from_secs(9))
            .map_err(|_| "获取 Apple Music 歌词超时".to_string())?
    }

    pub fn disconnect_agent() -> Result<(), String> {
        let maybe_tx = WORKER_TX.lock().ok().and_then(|g| g.clone());
        let Some(tx) = maybe_tx else {
            return Ok(());
        };
        let (r_tx, r_rx) = mpsc::sync_channel(1);
        if tx.send(WorkerCmd::Disconnect { reply: r_tx }).is_ok() {
            let _ = r_rx.recv_timeout(Duration::from_secs(5));
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod win {
    use super::*;

    pub fn is_agent_active() -> bool {
        false
    }
    pub fn is_agent_connected() -> bool {
        false
    }
    pub fn get_playback_state() -> SmtcState {
        SmtcState::default()
    }
    pub fn get_agent_status() -> AgentStatus {
        AgentStatus::default()
    }
    pub fn ensure_agent_connected() -> Result<AgentStatus, String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn get_audio_prefs(_connect_if_needed: bool) -> Result<AudioPrefs, String> {
        Ok(AudioPrefs::default())
    }
    pub fn set_audio_prefs(_prefs: AudioPrefs) -> Result<AudioPrefs, String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn control_downloads_queue(_pause: bool) -> Result<(), String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn play_tracks(
        _tracks: Vec<AgentTrackInput>,
        _start_index: usize,
    ) -> Result<SmtcState, String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn control(_action: &str, _value: Option<f64>) -> Result<SmtcState, String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn save_agent_log(_folder: &str, _filename: &str) -> Result<String, String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn get_store_lyrics_via_agent(
        _adam_id: &str,
        _connect_if_needed: bool,
    ) -> Result<Option<(String, bool)>, String> {
        Ok(None)
    }
    pub fn ensure_single_ui_instance_or_focus() -> bool {
        true
    }
    pub fn show_fatal_error_dialog(msg: &str) {
        eprintln!("AM Light fatal error: {msg}");
    }
    pub fn disconnect_agent() -> Result<(), String> {
        Ok(())
    }
    pub fn init_process_com_security() -> i32 {
        0
    }
    pub fn com_security_init_hr() -> i32 {
        0
    }
    pub fn playback_event_count() -> u64 {
        0
    }
    pub fn playback_reg_diagnostics() -> (i32, i32, u64) {
        (0, 0, 0)
    }
    pub fn query_playback_delegate_iid(
        _data1: u32,
        _data2: u16,
        _data3: u16,
        _data4: [u8; 8],
    ) -> i32 {
        0
    }
    pub fn dispatch_playback_event_plist(_plist: &PlistValue) {}
}

pub use win::{
    com_security_init_hr, control, control_downloads_queue, disconnect_agent,
    dispatch_playback_event_plist, ensure_agent_connected, ensure_single_ui_instance_or_focus,
    get_agent_status, get_audio_prefs, get_playback_state, get_store_lyrics_via_agent,
    init_process_com_security, is_agent_active, is_agent_connected, play_tracks,
    playback_event_count, playback_reg_diagnostics, query_playback_delegate_iid, save_agent_log,
    set_audio_prefs, show_fatal_error_dialog,
};

pub fn set_track_hls_manifest(adam_id: &str, m3u8_text: String) {
    if adam_id.is_empty() || m3u8_text.is_empty() {
        return;
    }
    if let Ok(mut snap) = SHARED.write() {
        snap.hls_cache.insert(adam_id.to_string(), m3u8_text);
        if snap.adam_id.as_deref() == Some(adam_id) {
            snap.recompute_current_stream_specs();
        }
    }
}

pub fn set_track_audio_traits(adam_id: &str, traits: Vec<String>) {
    if adam_id.is_empty() || traits.is_empty() {
        return;
    }
    if let Ok(mut snap) = SHARED.write() {
        snap.track_traits_cache.insert(adam_id.to_string(), traits);
        if snap.adam_id.as_deref() == Some(adam_id) {
            snap.recompute_current_stream_specs();
        }
    }
}

pub fn has_cached_hls_manifest(adam_id: &str) -> bool {
    SHARED
        .read()
        .ok()
        .map(|s| s.hls_cache.contains_key(adam_id))
        .unwrap_or(false)
}

pub fn get_cached_library_url() -> Option<String> {
    SHARED
        .read()
        .ok()
        .and_then(|s| s.prefs.library_location_url.clone())
}

pub fn enrich_cached_track(adam_id: &str, meta: CachedTrackMeta) {
    if let Ok(mut snap) = SHARED.write() {
        if snap.adam_id.as_deref() == Some(adam_id) {
            if !meta.title.is_empty() {
                snap.title = meta.title.clone();
            }
            if !meta.artist.is_empty() {
                snap.artist = meta.artist.clone();
            }
            if !meta.album.is_empty() {
                snap.album = meta.album.clone();
            }
            if meta.artwork_url.is_some() {
                snap.artwork_url = meta.artwork_url.clone();
            }
            if meta.audio_quality.is_some() {
                snap.audio_quality = meta.audio_quality.clone();
            }
            if snap.duration_sec <= 0.0 && meta.duration_sec > 0.0 {
                snap.duration_sec = meta.duration_sec;
            }
        }
        snap.track_cache.insert(adam_id.to_string(), meta);
    }
}

pub fn has_cached_artwork(adam_id: &str) -> bool {
    SHARED
        .read()
        .ok()
        .and_then(|s| s.track_cache.get(adam_id).and_then(|m| m.artwork_url.clone()))
        .is_some()
}

/// Extended lock-free / sub-microsecond read of the authoritative built-in playback clock
/// returning `(adam_id, status, position_sec, duration_sec, resolved_cmd_epoch)`.
pub fn get_live_agent_playhead_ext() -> Option<(Option<String>, String, f64, f64, u64)> {
    let s = SHARED.read().ok()?;
    if !s.active {
        return None;
    }
    Some((
        s.adam_id.clone(),
        s.status.clone(),
        s.current_position_sec(),
        s.duration_sec,
        s.resolved_epoch,
    ))
}

/// Lock-free / sub-microsecond read of the authoritative built-in playback clock for zero-drift 240Hz lyrics sync.
pub fn get_live_agent_playhead() -> Option<(Option<String>, String, f64, f64)> {
    let (aid, st, pos, dur, _ep) = get_live_agent_playhead_ext()?;
    Some((aid, st, pos, dur))
}

pub fn get_queue_tracks() -> Vec<AgentTrackInput> {
    SHARED
        .read()
        .ok()
        .map(|s| s.queue_tracks.clone())
        .unwrap_or_default()
}

pub async fn play_container_by_api(
    kind: String,
    id: String,
    user_token: Option<String>,
    storefront: Option<String>,
) -> Result<SmtcState, String> {
    let sf = storefront
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(get_active_storefront);
    set_active_storefront(&sf);

    let tracks: Vec<AgentTrackInput> = match kind.as_str() {
        "songs" | "song" => {
            let path = format!("v1/catalog/{sf}/songs/{id}?extend=audioTraits&l=zh-Hans-CN");
            let json = crate::am_api::fetch_am_api(path, user_token, None, Some(sf)).await?;
            let item = json
                .pointer("/data/0")
                .ok_or_else(|| "未找到该歌曲信息".to_string())?;
            vec![parse_track_from_am_json(item, None, None)
                .ok_or_else(|| "解析歌曲信息失败".to_string())?]
        }
        "albums" | "playlists" => {
            let path = format!("v1/catalog/{sf}/{kind}/{id}?include=tracks&extend=audioTraits&l=zh-Hans-CN");
            let json = crate::am_api::fetch_am_api(path, user_token, None, Some(sf)).await?;
            let root = json
                .pointer("/data/0")
                .ok_or_else(|| "未找到专辑/歌单详情".to_string())?;
            let fallback_album = root.pointer("/attributes/name").and_then(Value::as_str);
            let fallback_art = root
                .pointer("/attributes/artwork/url")
                .and_then(Value::as_str)
                .map(|u| format_artwork_template(u, 600));
            let arr = root
                .pointer("/relationships/tracks/data")
                .and_then(Value::as_array)
                .ok_or_else(|| "该列表暂无可播放曲目".to_string())?;
            arr.iter()
                .filter_map(|t| parse_track_from_am_json(t, fallback_album, fallback_art.as_deref()))
                .collect()
        }
        "library-albums" | "library-playlists" => {
            let sub = if kind == "library-albums" {
                "albums"
            } else {
                "playlists"
            };
            let path = format!("v1/me/library/{sub}/{id}/tracks?include=catalog&extend=audioTraits&l=zh-Hans-CN");
            let json = crate::am_api::fetch_am_api(path, user_token, None, Some(sf)).await?;
            let arr = json
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| "资料库列表暂无可播放曲目".to_string())?;
            arr.iter()
                .filter_map(|t| parse_track_from_am_json(t, None, None))
                .collect()
        }
        "stations" | "station" | "radioStation" | "radio-stations" => {
            let clean_id = id.trim().to_string();
            if clean_id.is_empty() {
                return Err("电台 ID 不能为空".to_string());
            }
            // Play Apple Music stations natively through AMPLibraryAgent without ever launching
            // the official Apple Music desktop GUI (`musics://`).
            // Note: Apple Music `POST /v1/me/stations/next-tracks/{id}` enforces `limit <= 10`;
            // fetching 2 deduplicated batches of 10 yields a smooth ~18-20 song radio queue!
            let mut station_tracks: Vec<AgentTrackInput> = Vec::new();
            let mut seen_ids = std::collections::HashSet::new();
            for batch in 0..2 {
                let Ok(next_json) = crate::am_api::fetch_station_next_tracks(
                    &clean_id,
                    user_token.clone(),
                    Some(sf.clone()),
                    crate::am_api::STATION_NEXT_TRACKS_MAX_LIMIT,
                )
                .await
                else {
                    break;
                };
                let mut batch_added = 0usize;
                if let Some(arr) = next_json.get("data").and_then(Value::as_array) {
                    for t in arr {
                        if let Some(parsed) = parse_track_from_am_json(t, None, None) {
                            let dedup_key = parsed.effective_adam_id();
                            if seen_ids.insert(dedup_key) {
                                station_tracks.push(parsed);
                                batch_added += 1;
                            }
                        }
                    }
                }
                if batch_added == 0 || (batch == 0 && station_tracks.len() < 4) {
                    break;
                }
            }

            if station_tracks.is_empty() {
                // Fallback for live broadcast stations (`format == "stream"`, e.g. Apple Music 1 / Hits / Chill)
                // or catalog stations: resolve metadata (with storefront fallback if `cn` lacks live catalog)
                // and pull its official curated playlist / show tracks natively inside AM Light!
                let mut st_json_opt: Option<Value> = None;
                let mut resolved_sf = sf.clone();
                for cand_sf in [sf.as_str(), "jp", "us"] {
                    let st_path = format!(
                        "v1/catalog/{cand_sf}/stations/{clean_id}?include=next-tracks&extend=audioTraits&l=zh-Hans-CN"
                    );
                    if let Ok(j) = crate::am_api::fetch_am_api(
                        st_path,
                        user_token.clone(),
                        None,
                        Some(cand_sf.to_string()),
                    )
                    .await
                    {
                        if j.pointer("/data/0").is_some() {
                            st_json_opt = Some(j);
                            resolved_sf = cand_sf.to_string();
                            break;
                        }
                    }
                }

                if let Some(st_json) = st_json_opt {
                    let root = st_json.pointer("/data/0");
                    let st_name = root
                        .and_then(|r| r.pointer("/attributes/name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let st_art = root
                        .and_then(|r| r.pointer("/attributes/artwork/url"))
                        .and_then(Value::as_str)
                        .map(|u| format_artwork_template(u, 600));
                    let is_live = root
                        .and_then(|r| r.pointer("/attributes/isLive"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);

                    if let Some(arr) = root
                        .and_then(|r| r.pointer("/relationships/next-tracks/data"))
                        .and_then(Value::as_array)
                    {
                        station_tracks = arr
                            .iter()
                            .filter_map(|t| {
                                parse_track_from_am_json(t, Some(&st_name), st_art.as_deref())
                            })
                            .collect();
                    }

                    if station_tracks.is_empty() && !st_name.is_empty() {
                        // Build search terms from exact station name + concise fallback keyword for live channels
                        let mut terms = vec![st_name.clone()];
                        let lower = st_name.to_ascii_lowercase();
                        if lower.contains("chill") {
                            terms.push("Apple Music Chill".to_string());
                            terms.push("Chill".to_string());
                        } else if lower.contains("club") {
                            terms.push("Dance".to_string());
                        } else if lower.contains("country") {
                            terms.push("Country".to_string());
                        } else if lower.contains("holiday") || st_name.contains("节日") {
                            terms.push("Holiday Hits".to_string());
                        } else if lower.contains("música") || lower.contains("musica") {
                            terms.push("Latin".to_string());
                        } else if lower.contains("hits") {
                            terms.push("Today's Hits".to_string());
                        }

                        'search_terms: for term in terms {
                            let enc: String =
                                url::form_urlencoded::byte_serialize(term.as_bytes()).collect();
                            let search_p = format!(
                                "v1/catalog/{resolved_sf}/search?term={enc}&types=playlists,songs&limit=8&l=zh-Hans-CN"
                            );
                            let Ok(sj) = crate::am_api::fetch_am_api(
                                search_p,
                                user_token.clone(),
                                None,
                                Some(resolved_sf.clone()),
                            )
                            .await
                            else {
                                continue;
                            };

                            if let Some(pls) = sj
                                .pointer("/results/playlists/data")
                                .and_then(Value::as_array)
                            {
                                for pl in pls.iter().take(5) {
                                    let Some(pl_id) = pl.get("id").and_then(Value::as_str) else {
                                        continue;
                                    };
                                    let pl_p = format!(
                                        "v1/catalog/{resolved_sf}/playlists/{pl_id}?include=tracks&extend=audioTraits&l=zh-Hans-CN"
                                    );
                                    if let Ok(pj) = crate::am_api::fetch_am_api(
                                        pl_p,
                                        user_token.clone(),
                                        None,
                                        Some(resolved_sf.clone()),
                                    )
                                    .await
                                    {
                                        if let Some(arr) = pj
                                            .pointer("/data/0/relationships/tracks/data")
                                            .and_then(Value::as_array)
                                        {
                                            let parsed: Vec<AgentTrackInput> = arr
                                                .iter()
                                                .filter_map(|t| {
                                                    parse_track_from_am_json(
                                                        t,
                                                        Some(&st_name),
                                                        st_art.as_deref(),
                                                    )
                                                })
                                                .collect();
                                            if !parsed.is_empty() {
                                                station_tracks = parsed;
                                                break 'search_terms;
                                            }
                                        }
                                    }
                                }
                            }
                            if station_tracks.is_empty() {
                                if let Some(arr) = sj
                                    .pointer("/results/songs/data")
                                    .and_then(Value::as_array)
                                {
                                    station_tracks = arr
                                        .iter()
                                        .filter_map(|t| {
                                            parse_track_from_am_json(
                                                t,
                                                Some(&st_name),
                                                st_art.as_deref(),
                                            )
                                        })
                                        .collect();
                                    if !station_tracks.is_empty() {
                                        break 'search_terms;
                                    }
                                }
                            }
                        }
                    }

                    if station_tracks.is_empty() && is_live {
                        return Err(format!(
                            "「{st_name}」为实时直播电台流，当前未获取到可点播节目曲目（已保持内置播放，不唤起原版客户端）"
                        ));
                    }
                }
            }

            if station_tracks.is_empty() {
                if user_token.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    return Err("播放个人或推荐电台需在设置中配置 Media-User-Token（已保持内置模式，不唤起原版客户端）".to_string());
                }
                return Err("该电台暂未返回可播放曲目，请稍后重试".to_string());
            }
            station_tracks
        }
        other => return Err(format!("不支持的播放容器类型: {other}")),
    };

    if tracks.is_empty() {
        return Err("未解析到可播放曲目".to_string());
    }

    tokio::task::spawn_blocking(move || play_tracks(tracks, 0))
        .await
        .map_err(|e| format!("播放线程异常: {e}"))?
}

/// Immediately updates the lock-free playhead anchor on the UI thread when the user seeks,
/// preventing 120Hz `get_live_agent_playhead` frames from rubber-banding back to the pre-seek
/// position while the out-of-proc COM seek RPC (`5011` / `5010`) is in flight.
pub fn notify_optimistic_seek(pos_sec: f64) {
    let clamped = pos_sec.max(0.0);
    let now = Instant::now();
    if let Ok(mut snap) = SHARED.write() {
        if snap.active {
            snap.anchor_pos_sec = clamped;
            snap.anchor_instant = None;
            snap.optimistic_seek_target = Some((clamped, now + Duration::from_millis(1600)));
            AGENT_STATE_SEQ.fetch_add(1, Ordering::Release);
        }
    }
}
