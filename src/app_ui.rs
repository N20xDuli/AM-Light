#![cfg(target_os = "windows")]

use crate::am_agent::{
    self, format_artwork_template, format_audio_traits_badge, parse_track_from_am_json,
    AgentStatus, AgentTrackInput, AudioPrefs, CachedTrackMeta,
};
use crate::am_api::{self, AuthVerifyResult};
use crate::audio_viz::{self, AudioOutputDevice, LiveVisualizerSnapshot};
use crate::binaural_dsp::{self, BinauralConfig, BinauralStatus};
use crate::d2d_renderer::{D2DRenderer, GlassBtnVariant, SfIcon, TextAlign};
use crate::downloads::{self, DownloadsOverview};
use crate::lyrics::{self, NonEmphasisLyricsEffect, ParsedLyrics};
use crate::lyrics_view::{LyricsHitAction, LyricsViewState};
use crate::smtc::{self, SmtcState};
use crate::theme::{Color, RectF, Theme};
use crate::visualizers::{VizHitAction, VizStyle, VisualizerState};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FpsCap {
    Hz60,
    Hz72,
    Adaptive60_72,
    VSync,
    Custom,
}

impl<'de> Deserialize<'de> for FpsCap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "Hz60" => Self::Hz60,
            "Hz72" => Self::Hz72,
            "Adaptive60_72" => Self::Adaptive60_72,
            "VSync" => Self::VSync,
            "Custom" | "Hz90" | "Hz120" | "Hz144" | "Hz240" => Self::Custom,
            _ => Self::VSync,
        })
    }
}

/// Resolves adaptive `[60/72]` target frame rate from the active monitor's refresh rate:
/// exact multiples/near-multiples of 72 Hz (`72, 143, 144, 288, 360`) pick `72 Hz`;
/// exact multiples/near-multiples of 60 Hz (`60, 120, 180, 239, 240`) pick `60 Hz`.
#[inline]
pub fn resolve_adaptive_60_72_hz(monitor_hz: u32) -> u32 {
    if monitor_hz < 70 {
        return 60;
    }
    let div60 = monitor_hz % 60 == 0;
    let div72 = monitor_hz % 72 == 0;
    if div72 && !div60 {
        return 72;
    }
    if div60 && !div72 {
        return 60;
    }
    if div72 && div60 {
        return 72;
    }
    for k in 1..=8u32 {
        let d72 = monitor_hz.abs_diff(72 * k);
        let d60 = monitor_hz.abs_diff(60 * k);
        if d72 <= 2 && d72 < d60 {
            return 72;
        }
        if d60 <= 2 && d60 <= d72 {
            return 60;
        }
    }
    let f = monitor_hz as f32;
    let err60 = (f / 60.0 - (f / 60.0).round()).abs();
    let err72 = (f / 72.0 - (f / 72.0).round()).abs();
    if err72 < err60 { 72 } else { 60 }
}

/// Computes `(dxgi_sync_interval, resolved_hz, software_interval_ns)` for a given `FpsCap`,
/// monitor refresh rate, and custom FPS value.
#[inline]
pub fn resolve_cap_sync_and_hz(
    cap: FpsCap,
    monitor_hz: u32,
    custom_fps: u32,
) -> (u32, u32, Option<u64>) {
    let mon = monitor_hz.clamp(30, 500);
    let cust = custom_fps.clamp(30, 360);
    if cap == FpsCap::VSync {
        return (1, mon, None);
    }
    let hz = match cap {
        FpsCap::Hz60 => 60,
        FpsCap::Hz72 => 72,
        FpsCap::Adaptive60_72 => resolve_adaptive_60_72_hz(mon),
        FpsCap::Custom => cust,
        FpsCap::VSync => mon,
    };
    if let Some(k) = (1..=4u32).find(|&k| mon.abs_diff(hz * k) <= 2) {
        (k, hz, Some(1_000_000_000u64 / (hz as u64)))
    } else {
        (
            0,
            hz,
            Some((1_000_000_000u64 / (hz as u64)).saturating_sub(150_000)),
        )
    }
}

impl FpsCap {
    pub const ALL: [FpsCap; 5] = [
        FpsCap::Hz60,
        FpsCap::Hz72,
        FpsCap::Adaptive60_72,
        FpsCap::VSync,
        FpsCap::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FpsCap::Hz60 => "60",
            FpsCap::Hz72 => "72",
            FpsCap::Adaptive60_72 => "自适应 [60/72]",
            FpsCap::VSync => "垂直同步",
            FpsCap::Custom => "自定义",
        }
    }

    pub fn resolved_label(self, monitor_hz: u32, custom_fps: u32) -> String {
        let mon = monitor_hz.clamp(30, 500);
        match self {
            FpsCap::Hz60 => "60 Hz".to_string(),
            FpsCap::Hz72 => "72 Hz".to_string(),
            FpsCap::Adaptive60_72 => format!("自适应 {}Hz", resolve_adaptive_60_72_hz(mon)),
            FpsCap::VSync => format!("垂直同步 {}Hz", mon),
            FpsCap::Custom => format!("自定义 {}Hz", custom_fps.clamp(30, 360)),
        }
    }

    pub fn target_interval_ns(self) -> Option<u64> {
        match self {
            FpsCap::Hz60 => Some(16_400_000), // slightly under 16.66ms to guarantee >=60Hz!
            FpsCap::Hz72 => Some(13_750_000),
            FpsCap::Adaptive60_72 => Some(16_400_000),
            FpsCap::VSync => None,
            FpsCap::Custom => Some(8_280_000),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_custom_fps() -> u32 {
    120
}

fn default_vsync_cap() -> FpsCap {
    FpsCap::VSync
}

pub const TOP_BAR_GUARD_H: f32 = 58.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub storefront: String,
    pub storefront_customized: bool,
    pub user_token: String,
    pub token_cleared_by_user: bool,
    pub use_builtin_service: bool,
    pub viz_fps_cap: FpsCap,
    #[serde(default = "default_custom_fps")]
    pub viz_custom_fps: u32,
    #[serde(default = "default_vsync_cap")]
    pub lyrics_fps_cap: FpsCap,
    #[serde(default = "default_custom_fps")]
    pub lyrics_custom_fps: u32,
    #[serde(default)]
    pub lyrics_limit_unfocused_fps: bool,
    pub viz_default_style: VizStyle,
    pub sidebar_collapsed: bool,
    pub window_corner_radius: f32,
    #[serde(default)]
    pub liquid_glass_enabled: bool,
    #[serde(default = "default_true")]
    pub lyrics_latin_by_word: bool,
    #[serde(default)]
    pub lyrics_non_emphasis_effect: NonEmphasisLyricsEffect,
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
    pub lyrics_free_blur: bool,
    pub hidden_sidebar_keys: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            storefront: "cn".to_string(),
            storefront_customized: false,
            user_token: String::new(),
            token_cleared_by_user: false,
            use_builtin_service: true,
            viz_fps_cap: FpsCap::VSync,
            viz_custom_fps: 120,
            lyrics_fps_cap: FpsCap::VSync,
            lyrics_custom_fps: 120,
            lyrics_limit_unfocused_fps: false,
            viz_default_style: VizStyle::AudioInspector,
            sidebar_collapsed: false,
            window_corner_radius: 12.0,
            liquid_glass_enabled: false,
            lyrics_latin_by_word: true,
            lyrics_non_emphasis_effect: NonEmphasisLyricsEffect::None,
            reduce_motion: false,
            reduce_transparency: false,
            lyrics_free_blur: false,
            hidden_sidebar_keys: Vec::new(),
        }
    }
}

impl AppSettings {
    pub fn config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::with_capacity(2);
        for key in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(key) {
                let dir = PathBuf::from(base).join("AMLight");
                let _ = fs::create_dir_all(&dir);
                let p = dir.join("settings_v2.json");
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }
        paths
    }

    fn write_file_atomically(path: &PathBuf, content: &str) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp_path = path.with_extension("json.tmp");
        let wrote_tmp = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut f = fs::File::create(&tmp_path)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            Ok(())
        })();
        if wrote_tmp.is_ok() && fs::rename(&tmp_path, path).is_ok() {
            return;
        }
        let _ = fs::remove_file(&tmp_path);
        let _ = fs::write(path, content);
    }

    pub fn load_with_official() -> (Self, am_api::OfficialClientAccountInfo) {
        let mut loaded: Option<Self> = None;
        for p in Self::config_paths() {
            if let Ok(txt) = fs::read_to_string(&p) {
                if let Ok(mut s) = serde_json::from_str::<Self>(&txt) {
                    s.user_token = am_api::normalize_user_token(&s.user_token);
                    if loaded.is_none()
                        || (loaded.as_ref().is_some_and(|prev| prev.user_token.is_empty())
                            && !s.user_token.is_empty())
                    {
                        loaded = Some(s);
                    }
                }
            }
        }

        let mut s = loaded.unwrap_or_default();
        s.window_corner_radius = s.window_corner_radius.clamp(0.0, 28.0);
        s.viz_custom_fps = s.viz_custom_fps.clamp(30, 360);
        s.lyrics_custom_fps = s.lyrics_custom_fps.clamp(30, 360);
        let official = am_api::detect_official_am_account();
        let mut dirty = false;

        // Auto-adopt official Apple Music desktop client's signed-in storefront when not manually overridden
        if !s.storefront_customized {
            if let Some(ref off_sf) = official.storefront {
                if !off_sf.is_empty() && s.storefront != *off_sf {
                    s.storefront = off_sf.clone();
                    dirty = true;
                }
            }
        }

        // Auto-import Media-User-Token & storefront from previous am-light installation if available
        if s.user_token.is_empty() && !s.token_cleared_by_user {
            let legacy = am_api::import_legacy_amlight_credentials();
            if let Some(tok) = legacy.user_token {
                if !tok.is_empty() {
                    s.user_token = tok;
                    dirty = true;
                }
            }
            if !s.storefront_customized && official.storefront.is_none() {
                if let Some(leg_sf) = legacy.storefront {
                    if !leg_sf.is_empty() && s.storefront != leg_sf {
                        s.storefront = leg_sf;
                        dirty = true;
                    }
                }
            }
        }

        am_agent::set_active_storefront(&s.storefront);
        if dirty {
            s.save();
        }
        (s, official)
    }

    pub fn load() -> Self {
        Self::load_with_official().0
    }

    pub fn save(&self) {
        am_agent::set_active_storefront(&self.storefront);
        if let Ok(txt) = serde_json::to_string_pretty(self) {
            for p in Self::config_paths() {
                Self::write_file_atomically(&p, &txt);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageRoute {
    Home,
    Browse,
    Radio,
    Visualizer,
    Search,
    LibRecent,
    LibArtists,
    LibAlbums,
    LibSongs,
    LibForYou,
    LibDownloads,
    LibPlaylists,
    ContainerDetail {
        kind: String, // "albums", "playlists", "library-albums", "library-playlists"
        id: String,
        title_hint: String,
    },
    Settings,
}

#[derive(Clone, Debug)]
pub struct MediaCardItem {
    pub id: String,
    pub kind: String, // "albums", "playlists", "songs", "library-albums", "library-playlists"
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub badge: Option<String>,
    pub track_input: Option<AgentTrackInput>,
}

#[derive(Clone, Debug)]
pub struct ContainerDetailData {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub tracks: Vec<AgentTrackInput>,
}

pub enum AsyncMsg {
    HomeLoaded {
        albums: Vec<MediaCardItem>,
        playlists: Vec<MediaCardItem>,
        songs: Vec<AgentTrackInput>,
    },
    RadioLoaded(Vec<MediaCardItem>),
    SearchResults {
        query: String,
        albums: Vec<MediaCardItem>,
        playlists: Vec<MediaCardItem>,
        songs: Vec<AgentTrackInput>,
    },
    LibraryCardsLoaded {
        route_key: String,
        cards: Vec<MediaCardItem>,
    },
    LibrarySongsLoaded(Vec<AgentTrackInput>),
    SidebarPlaylistsLoaded(Vec<MediaCardItem>),
    ContainerLoaded(Result<ContainerDetailData, String>),
    PlaylistBackgroundUpdated(ContainerDetailData),
    PeriodicPlaylistSyncDue,
    DownloadsLoaded(Result<DownloadsOverview, String>),
    PlaybackUpdated(SmtcState),
    AgentStatusUpdated(AgentStatus),
    AudioPrefsLoaded(AudioPrefs),
    DevicesLoaded(Vec<AudioOutputDevice>),
    BinauralUpdated(BinauralStatus),
    AuthVerified(AuthVerifyResult),
    ArtworkBytes {
        url: String,
        hero: bool,
        bytes: Vec<u8>,
    },
    ArtworkDecoded {
        req_key: String,
        url: String,
        hero: bool,
        decoded: crate::d2d_renderer::DecodedArtworkCpu,
    },
    ArtworkFailed {
        req_key: String,
    },
    SmtcCatalogEnriched {
        meta_key: String,
        adam_id: String,
        artwork_url: Option<String>,
        audio_quality: Option<String>,
        stream_specs: Option<audio_viz::StreamAudioSpecs>,
        validated_with_dur: bool,
    },
    LyricsFetched {
        key: String,
        seq: u64,
        lyrics: Option<ParsedLyrics>,
    },
    BannerNotice(String),
    BannerError(String),
}

#[derive(Clone, Debug)]
pub struct CachedSmtcEnrichment {
    pub adam_id: String,
    pub artwork_url: Option<String>,
    pub audio_quality: Option<String>,
    pub stream_specs: Option<audio_viz::StreamAudioSpecs>,
    pub validated_with_dur: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum ActiveDrag {
    LyricsSeek(RectF),
    LyricsVolume(RectF),
    MiniPlayerSeek(RectF),
    PopoverVolume(RectF),
    BinauralWidth(RectF),
    BinauralRoom(RectF),
    WindowCornerRadius(RectF),
    VizCustomFps(RectF),
    LyricsCustomFps(RectF),
}

#[derive(Clone, Debug)]
pub enum UiAction {
    SwallowClick,
    WindowMinimize,
    WindowToggleMaximize,
    WindowClose,
    Navigate(PageRoute),
    NavigateBack,
    ToggleSidebar,
    ToggleSidebarEdit,
    ToggleSidebarItemHidden(String),
    ToggleSectionLibrary,
    ToggleSectionPlaylists,
    FocusSearch,
    ClearSearch,
    SetSearchFilter(u8),
    PlayContainer {
        kind: String,
        id: String,
        shuffle: bool,
    },
    PlayDetailTracks {
        start_index: usize,
        shuffle: bool,
    },
    PlayTrackList {
        source: TrackListSource,
        index: usize,
    },
    PlaySingleTrack(AgentTrackInput),
    AddContainerToLibrary {
        kind: String,
        id: String,
    },
    AddSongToLibrary(String),
    TogglePlayPause,
    PrevTrack,
    NextTrack,
    SeekRatio(f64),
    SetVolumeRatio(f64),
    ScrubMiniPlayerSeek(RectF),
    ScrubPopoverVolume(RectF),
    ToggleShuffleMode,
    ToggleRepeatMode,
    ToggleLyricsOverlay,
    ToggleQueuePopover,
    ToggleMorePopover,
    JumpQueueIndex(usize),
    RefreshCurrentView,
    // Downloads actions
    OpenDownloadsFolder,
    TogglePauseDownloads(bool),
    RevealDownload(String),
    DeleteDownload(String),
    // Settings actions
    SetStorefront(String),
    SyncOfficialClientAccount,
    FocusTokenBox,
    PasteUserToken,
    OpenWebAppleMusic,
    VerifyUserToken,
    ClearUserToken,
    SetUseBuiltinService(bool),
    DisconnectBuiltinService,
    SetStreamingQuality {
        lossless: bool,
        quality: i64,
    },
    SetAtmosMode(i64),
    ToggleSoundCheck,
    ToggleCrossfade,
    SelectOutputDevice(String),
    SetVizFpsCap(FpsCap),
    SetLyricsFpsCap(FpsCap),
    SetVizCustomFps(u32),
    ScrubVizCustomFps(RectF),
    SetLyricsCustomFps(u32),
    ScrubLyricsCustomFps(RectF),
    ToggleLyricsLimitUnfocusedFps,
    SetWindowCornerRadius(f32),
    ScrubWindowCornerRadius(RectF),
    ToggleLiquidGlass,
    ToggleLyricsLatinByWord,
    SetLyricsNonEmphasisEffect(NonEmphasisLyricsEffect),
    ToggleLyricsBlurMode,
    ToggleReduceMotion,
    ToggleReduceTransparency,
    // Hidden-by-default spatial audio flavor disclosure
    ToggleAdvancedSpatialDisclosure,
    ToggleBinauralEnabled,
    ToggleBinauralBypass,
    SetBinauralPreset(String),
    AdjustBinauralWidth(f32),
    AdjustBinauralRoom(f32),
    ScrubBinauralWidth(RectF),
    ScrubBinauralRoom(RectF),
    DismissBanner,
}

#[derive(Clone, Copy, Debug)]
pub enum TrackListSource {
    HomeSongs,
    SearchSongs,
    LibrarySongs,
    DetailTracks,
}

#[derive(Clone, Debug)]
pub struct HitRegion {
    pub rect: RectF,
    pub action: UiAction,
}

pub struct AppUi {
    pub settings: AppSettings,
    pub rt: tokio::runtime::Runtime,
    pub tx: Sender<AsyncMsg>,
    pub rx: Receiver<AsyncMsg>,
    pub wake_hwnd: usize,

    // Navigation, Page Transitions & Smooth Inertial Scroll
    pub route: PageRoute,
    pub nav_history: Vec<PageRoute>,
    pub route_trans: f32,
    pub route_trans_dir: f32,
    pub lyrics_open_spring: lyrics::Spring1D,
    pub scroll_y: f32,
    pub scroll_target_y: f32,
    pub scroll_vel_y: f32,
    pub max_scroll_y: f32,
    pub sidebar_scroll_y: f32,
    pub sidebar_scroll_target_y: f32,
    pub sidebar_scroll_vel_y: f32,
    pub sidebar_max_scroll_y: f32,
    last_wheel_inst: Option<Instant>,
    wheel_streak: f32,
    pub sidebar_nav_rect: RectF,
    pub sidebar_edit_mode: bool,
    pub sec_library_open: bool,
    pub sec_playlists_open: bool,
    pub sidebar_open_spring: lyrics::Spring1D,
    pub sec_library_spring: lyrics::Spring1D,
    pub sec_playlists_spring: lyrics::Spring1D,
    pub sidebar_edit_spring: lyrics::Spring1D,
    pub sidebar_sel_y_spring: lyrics::Spring1D,
    pub sidebar_sel_alpha_spring: lyrics::Spring1D,
    sidebar_sel_initialized: bool,
    last_hovered_interactive: bool,

    // Search & Input
    pub search_focused: bool,
    pub search_query: String,
    pub search_filter: u8, // 0=All, 1=Songs, 2=Albums, 3=Playlists
    pub last_search_typed: Option<Instant>,
    pub last_dispatched_query: String,

    // Data Caches
    pub home_albums: Vec<MediaCardItem>,
    pub home_playlists: Vec<MediaCardItem>,
    pub home_songs: Vec<AgentTrackInput>,
    pub radio_cards: Vec<MediaCardItem>,
    pub search_albums: Vec<MediaCardItem>,
    pub search_playlists: Vec<MediaCardItem>,
    pub search_songs: Vec<AgentTrackInput>,
    pub lib_cards: HashMap<String, Vec<MediaCardItem>>,
    pub lib_songs: Vec<AgentTrackInput>,
    pub sidebar_playlists: Vec<MediaCardItem>,
    pub detail_data: Option<ContainerDetailData>,
    pub detail_loading: bool,
    pub downloads_info: DownloadsOverview,
    pub downloads_paused: bool,

    // Playback, Audio, Visualizer, Lyrics
    pub smtc: SmtcState,
    pub agent_status: AgentStatus,
    pub audio_prefs: AudioPrefs,
    pub devices: Vec<AudioOutputDevice>,
    pub binaural: BinauralStatus,
    /// Must stay `false` by default per requirement: "空间音频风味等的开关和设置默认隐藏"
    pub show_advanced_spatial: bool,
    pub shuffle_on: bool,
    pub repeat_on: bool,
    pub show_lyrics_overlay: bool,
    pub show_queue_popover: bool,
    pub show_more_popover: bool,

    pub viz_state: VisualizerState,
    pub latest_viz_snap: LiveVisualizerSnapshot,
    pub lyrics_view: LyricsViewState,
    pub current_lyrics_key: String,
    pub current_lyrics: Option<ParsedLyrics>,
    pub lyrics_loading: bool,
    pub auth_result: Option<AuthVerifyResult>,
    pub official_account: am_api::OfficialClientAccountInfo,
    pub token_box_focused: bool,
    pub token_box_rect: RectF,

    // Pending artwork downloads & catalog enrichment caches
    pending_art: HashSet<String>,
    failed_art: HashSet<String>,
    art_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    enriched_adam_ids: HashSet<String>,
    smtc_catalog_cache: HashMap<String, CachedSmtcEnrichment>,

    // Transient feedback banner & click-to-play buffering state
    pub banner_text: Option<(String, bool, Instant)>,
    pub playback_loading: bool,
    pub playback_loading_started: Option<Instant>,
    pub playback_loading_track_id: Option<String>,
    pub playback_loading_container_id: Option<String>,
    pub loading_cmd_epoch: u64,
    pub last_agent_seq: u64,
    last_smtc_event_seq: u64,
    smtc_event_pull_in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub spinner_phase: f32,
    pub is_window_maximized: bool,
    pub is_window_focused: bool,
    pub current_monitor_hz: u32,

    // Pointer, Dragging & Hit-testing
    pub mouse_pos: (f32, f32),
    pub mouse_down: bool,
    pub active_drag: Option<ActiveDrag>,
    last_drag_ipc: Instant,
    last_repeat_trigger: Option<Instant>,
    pub hit_regions: Vec<HitRegion>,
    pub search_box_rect: RectF,
    pub measured_fps: f32,
    fps_frames: u32,
    fps_window_start: Instant,

    // Monotonic wall-clock playhead anchor, external SMTC phase/seek guards & lyrics duration guard
    playhead_anchor_sec: f64,
    playhead_anchor_inst: Instant,
    pub last_smtc_timeline_univ: i64,
    smtc_stale_prev_univ: Option<i64>,
    smtc_anchor_calibrated: bool,
    pub smtc_optimistic_seek: Option<(f64, Instant)>,
    smtc_seek_pre_univ: i64,
    pub lyrics_fetched_with_dur: f64,
    lyrics_req_seq: u64,
    eq_bars_smooth: [f32; 6],
    hover_alphas: HashMap<u64, f32>,
    visited_hover_keys: HashSet<u64>,
    art_fading_until: Option<Instant>,
    frame_dt: f32,
    pub playlist_cache: PlaylistDiskCache,
    pub playlist_diff_anim: Option<PlaylistDiffAnimState>,
    pub top_bar_guard_spring: lyrics::Spring1D,
    playlist_sync_in_flight: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct CachedPlaylistSummary {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub badge: Option<String>,
}

impl CachedPlaylistSummary {
    pub fn from_card(c: &MediaCardItem) -> Self {
        Self {
            id: c.id.clone(),
            kind: c.kind.clone(),
            title: c.title.clone(),
            subtitle: c.subtitle.clone(),
            description: c.description.clone(),
            artwork_url: c.artwork_url.clone(),
            badge: c.badge.clone(),
        }
    }

    pub fn to_card(&self) -> MediaCardItem {
        MediaCardItem {
            id: self.id.clone(),
            kind: self.kind.clone(),
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            description: self.description.clone(),
            artwork_url: self.artwork_url.clone(),
            badge: self.badge.clone(),
            track_input: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct CachedPlaylistDetail {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub tracks: Vec<AgentTrackInput>,
    pub updated_unix_sec: u64,
}

impl CachedPlaylistDetail {
    pub fn from_detail(d: &ContainerDetailData, now_unix: u64) -> Self {
        Self {
            id: d.id.clone(),
            kind: d.kind.clone(),
            title: d.title.clone(),
            subtitle: d.subtitle.clone(),
            description: d.description.clone(),
            artwork_url: d.artwork_url.clone(),
            tracks: d.tracks.clone(),
            updated_unix_sec: now_unix,
        }
    }

    pub fn to_detail(&self) -> ContainerDetailData {
        ContainerDetailData {
            id: self.id.clone(),
            kind: self.kind.clone(),
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            description: self.description.clone(),
            artwork_url: self.artwork_url.clone(),
            tracks: self.tracks.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct PlaylistDiskCache {
    pub sidebar_playlists: Vec<CachedPlaylistSummary>,
    pub details: HashMap<String, CachedPlaylistDetail>,
    pub last_sync_unix_sec: u64,
}

impl PlaylistDiskCache {
    pub fn cache_path() -> Option<PathBuf> {
        AppSettings::config_paths()
            .into_iter()
            .next()
            .and_then(|p| p.parent().map(|dir| dir.join("playlists_cache_v1.json")))
    }

    pub fn load_from_path(path: &std::path::Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|txt| serde_json::from_str::<Self>(&txt).ok())
            .unwrap_or_default()
    }

    pub fn save_to_path(&self, path: &std::path::Path) {
        if let Ok(txt) = serde_json::to_string(self) {
            AppSettings::write_file_atomically(&path.to_path_buf(), &txt);
        }
    }

    pub fn load() -> Self {
        Self::cache_path()
            .map(|p| Self::load_from_path(&p))
            .unwrap_or_default()
    }

    pub fn upsert_detail(&mut self, detail: &ContainerDetailData, now_unix: u64) {
        self.details.insert(
            detail.id.clone(),
            CachedPlaylistDetail::from_detail(detail, now_unix),
        );
        if self.details.len() > 64 {
            let mut ages: Vec<(String, u64)> = self
                .details
                .iter()
                .map(|(k, v)| (k.clone(), v.updated_unix_sec))
                .collect();
            ages.sort_by_key(|x| x.1);
            let drop_n = self.details.len() - 64;
            for (old_k, _) in ages.into_iter().take(drop_n) {
                self.details.remove(&old_k);
            }
        }
        self.last_sync_unix_sec = now_unix;
    }

    pub fn upsert_sidebar(&mut self, pls: &[MediaCardItem], now_unix: u64) {
        self.sidebar_playlists = pls
            .iter()
            .map(CachedPlaylistSummary::from_card)
            .collect();
        self.last_sync_unix_sec = now_unix;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaylistDiffKind {
    KeptOrMoved { old_idx: usize, new_idx: usize },
    Added { new_idx: usize },
    Removed { old_idx: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistDiffEntry {
    pub track_key: String,
    pub kind: PlaylistDiffKind,
}

#[derive(Clone, Debug)]
pub struct PlaylistDiffAnimState {
    pub container_id: String,
    pub started_at: Instant,
    pub duration_sec: f32,
    pub old_tracks: Vec<AgentTrackInput>,
    pub new_tracks: Vec<AgentTrackInput>,
    pub entries: Vec<PlaylistDiffEntry>,
    pub bake_slots: HashMap<usize, usize>,
}

fn track_diff_base_key(t: &AgentTrackInput) -> String {
    if !t.id.trim().is_empty() {
        format!("id:{}", t.id.trim())
    } else if !t.effective_adam_id().is_empty() {
        format!("aid:{}", t.effective_adam_id())
    } else {
        format!(
            "meta:{}|{}|{}",
            t.name.trim(),
            t.artist_name.trim(),
            t.album_name.trim()
        )
    }
}

fn make_unique_diff_keys(tracks: &[AgentTrackInput]) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    tracks
        .iter()
        .map(|t| {
            let base = track_diff_base_key(t);
            let occ = counts.entry(base.clone()).or_insert(0);
            *occ += 1;
            format!("{base}#{occ}")
        })
        .collect()
}

pub fn compute_playlist_diff(
    old_tracks: &[AgentTrackInput],
    new_tracks: &[AgentTrackInput],
) -> Option<Vec<PlaylistDiffEntry>> {
    let old_keys = make_unique_diff_keys(old_tracks);
    let new_keys = make_unique_diff_keys(new_tracks);
    if old_keys == new_keys {
        return None;
    }
    let old_pos: HashMap<&str, usize> = old_keys
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();
    let new_pos: HashMap<&str, usize> = new_keys
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();

    let mut entries = Vec::with_capacity(old_keys.len() + new_keys.len());
    for (old_idx, key) in old_keys.iter().enumerate() {
        if !new_pos.contains_key(key.as_str()) {
            entries.push(PlaylistDiffEntry {
                track_key: key.clone(),
                kind: PlaylistDiffKind::Removed { old_idx },
            });
        }
    }
    for (new_idx, key) in new_keys.iter().enumerate() {
        if let Some(&old_idx) = old_pos.get(key.as_str()) {
            entries.push(PlaylistDiffEntry {
                track_key: key.clone(),
                kind: PlaylistDiffKind::KeptOrMoved { old_idx, new_idx },
            });
        } else {
            entries.push(PlaylistDiffEntry {
                track_key: key.clone(),
                kind: PlaylistDiffKind::Added { new_idx },
            });
        }
    }
    Some(entries)
}

pub fn clip_hit_regions_below_topbar(regions: &mut Vec<HitRegion>, min_y: f32) {
    regions.retain_mut(|hr| {
        if hr.rect.bottom() <= min_y {
            return false;
        }
        if hr.rect.y < min_y {
            let new_h = hr.rect.bottom() - min_y;
            if new_h <= 1.0 {
                return false;
            }
            hr.rect = RectF::new(hr.rect.x, min_y, hr.rect.w, new_h);
        }
        true
    });
}

fn current_unix_sec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl AppUi {
    pub fn new(wake_hwnd: usize) -> Self {
        let (settings, official_account) = if wake_hwnd == 0 {
            (
                AppSettings::default(),
                am_api::OfficialClientAccountInfo::default(),
            )
        } else {
            AppSettings::load_with_official()
        };
        // Avoid auto-connecting AMPLibraryAgent on External SMTC lookups unless built-in playback is active/loading
        lyrics::set_builtin_agent_lyrics_enabled(false);
        // Compact 2-worker + max-4 blocking pool with 512 KB stacks keeps thread memory minimal!
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(4)
            .thread_stack_size(512 * 1024)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let (tx, rx) = mpsc::channel();

        let mut viz_state = VisualizerState::default();
        viz_state.style = settings.viz_default_style;

        let mut lyrics_view = LyricsViewState::default();
        lyrics_view.music_free_blur = settings.lyrics_free_blur;
        lyrics_view.animator.latin_by_word = settings.lyrics_latin_by_word;

        let init_sb = if settings.sidebar_collapsed { 0.0 } else { 1.0 };
        let now = Instant::now();
        let playlist_cache = if wake_hwnd != 0 {
            PlaylistDiskCache::load()
        } else {
            PlaylistDiskCache::default()
        };
        let initial_sidebar_pls = playlist_cache
            .sidebar_playlists
            .iter()
            .map(CachedPlaylistSummary::to_card)
            .collect();
        let mut ui = Self {
            settings,
            rt,
            tx,
            rx,
            wake_hwnd,
            route: PageRoute::Home,
            nav_history: Vec::new(),
            route_trans: 1.0,
            route_trans_dir: 1.0,
            lyrics_open_spring: lyrics::Spring1D::new(0.0, 1.0, 168.0, 26.2),
            scroll_y: 0.0,
            scroll_target_y: 0.0,
            scroll_vel_y: 0.0,
            max_scroll_y: 0.0,
            sidebar_scroll_y: 0.0,
            sidebar_scroll_target_y: 0.0,
            sidebar_scroll_vel_y: 0.0,
            sidebar_max_scroll_y: 0.0,
            last_wheel_inst: None,
            wheel_streak: 1.0,
            sidebar_nav_rect: RectF::default(),
            sidebar_edit_mode: false,
            sec_library_open: true,
            sec_playlists_open: true,
            sidebar_open_spring: lyrics::Spring1D::new(init_sb, 1.0, 380.0, 39.0),
            sec_library_spring: lyrics::Spring1D::new(1.0, 1.0, 380.0, 39.0),
            sec_playlists_spring: lyrics::Spring1D::new(1.0, 1.0, 380.0, 39.0),
            sidebar_edit_spring: lyrics::Spring1D::new(0.0, 1.0, 380.0, 39.0),
            sidebar_sel_y_spring: lyrics::Spring1D::new(0.0, 1.0, 420.0, 41.0),
            sidebar_sel_alpha_spring: lyrics::Spring1D::new(1.0, 1.0, 380.0, 39.0),
            sidebar_sel_initialized: false,
            last_hovered_interactive: false,
            search_focused: false,
            search_query: String::new(),
            search_filter: 0,
            last_search_typed: None,
            last_dispatched_query: String::new(),
            home_albums: Vec::new(),
            home_playlists: Vec::new(),
            home_songs: Vec::new(),
            radio_cards: Vec::new(),
            search_albums: Vec::new(),
            search_playlists: Vec::new(),
            search_songs: Vec::new(),
            lib_cards: HashMap::new(),
            lib_songs: Vec::new(),
            sidebar_playlists: initial_sidebar_pls,
            detail_data: None,
            detail_loading: false,
            downloads_info: DownloadsOverview::default(),
            downloads_paused: false,
            smtc: SmtcState::default(),
            agent_status: AgentStatus::default(),
            audio_prefs: AudioPrefs::default(),
            devices: Vec::new(),
            binaural: BinauralStatus::default(),
            show_advanced_spatial: false, // Strictly hidden by default!
            shuffle_on: false,
            repeat_on: false,
            show_lyrics_overlay: false,
            show_queue_popover: false,
            show_more_popover: false,
            viz_state,
            latest_viz_snap: LiveVisualizerSnapshot::default(),
            lyrics_view,
            current_lyrics_key: String::new(),
            current_lyrics: None,
            lyrics_loading: false,
            auth_result: None,
            official_account,
            token_box_focused: false,
            token_box_rect: RectF::default(),
            pending_art: HashSet::new(),
            failed_art: HashSet::new(),
            art_epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
            enriched_adam_ids: HashSet::new(),
            smtc_catalog_cache: HashMap::new(),
            banner_text: None,
            playback_loading: false,
            playback_loading_started: None,
            playback_loading_track_id: None,
            playback_loading_container_id: None,
            loading_cmd_epoch: am_agent::current_play_cmd_epoch(),
            last_agent_seq: am_agent::state_seq(),
            last_smtc_event_seq: smtc::smtc_event_seq(),
            smtc_event_pull_in_flight: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )),
            spinner_phase: 0.0,
            is_window_maximized: false,
            is_window_focused: true,
            current_monitor_hz: 60,
            mouse_pos: (-100.0, -100.0),
            mouse_down: false,
            active_drag: None,
            last_drag_ipc: now - std::time::Duration::from_secs(1),
            last_repeat_trigger: None,
            hit_regions: Vec::with_capacity(128),
            search_box_rect: RectF::new(16.0, 58.0, 232.0, 36.0),
            measured_fps: 0.0,
            fps_frames: 0,
            fps_window_start: now,
            playhead_anchor_sec: 0.0,
            playhead_anchor_inst: now,
            last_smtc_timeline_univ: 0,
            smtc_stale_prev_univ: None,
            smtc_anchor_calibrated: false,
            smtc_optimistic_seek: None,
            smtc_seek_pre_univ: 0,
            lyrics_fetched_with_dur: 0.0,
            lyrics_req_seq: 0,
            eq_bars_smooth: [0.0; 6],
            hover_alphas: HashMap::with_capacity(64),
            visited_hover_keys: HashSet::with_capacity(64),
            art_fading_until: None,
            frame_dt: 1.0 / 60.0,
            playlist_cache,
            playlist_diff_anim: None,
            top_bar_guard_spring: lyrics::Spring1D::new(0.0, 1.0, 210.0, 29.0),
            playlist_sync_in_flight: false,
        };

        if wake_hwnd != 0 {
            ui.fetch_initial_data();
            if let Some(arg_os) = std::env::args_os().nth(1) {
                let p = PathBuf::from(arg_os);
                let is_lyr = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttml" | "lrc" | "yrc"));
                if is_lyr && p.is_file() {
                    let _ = ui.import_lyrics_from_path(&p);
                }
            }
        }
        ui
    }

    fn post_ui_wake(hwnd_val: usize) {
        if hwnd_val != 0 {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(windows::Win32::Foundation::HWND(hwnd_val as *mut std::ffi::c_void)),
                    audio_viz::WM_APP_AUDIO_WAKE,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    }

    pub fn fetch_initial_data(&mut self) {
        self.fetch_home_charts();
        self.refresh_devices_and_prefs(false);
        if !self.settings.user_token.is_empty() {
            self.dispatch_background_playlist_sync();
        }
        // Background 2-hour auto-sync ticker (0% idle CPU while sleeping, wakes via PostMessage!)
        let sync_tx = self.tx.clone();
        let sync_hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(7200));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await; // skip immediate first tick (startup handles it)
            loop {
                interval.tick().await;
                let _ = sync_tx.send(AsyncMsg::PeriodicPlaylistSyncDue);
                Self::post_ui_wake(sync_hwnd);
            }
        });
        // Start calm background playback-state monitor (event-driven wake ONLY when state changes!)
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            let mut prev: Option<SmtcState> = None;
            loop {
                let state = tokio::task::spawn_blocking(unified_playback_snapshot)
                    .await
                    .unwrap_or_default();
                let is_playing = state.status == "Playing";
                if prev.as_ref() != Some(&state) {
                    let _ = tx.send(AsyncMsg::PlaybackUpdated(state.clone()));
                    prev = Some(state);
                    Self::post_ui_wake(hwnd);
                }
                // Sleep deeply (1.8s) when idle/silent so CPU is ~0.00%, 420ms when actively playing!
                let sleep_ms = if is_playing { 420 } else { 1800 };
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
            }
        });
    }

    pub fn update_viz_subscription(&self) {
        // Strictly capture/render audio ONLY on the Visualizer page (even when unfocused in background);
        // anywhere else (Home, Playlist Detail, Lyrics Overlay, Settings, etc.), WASAPI loopback stays stopped!
        let want_viz = self.route == PageRoute::Visualizer && !self.show_lyrics_overlay;
        audio_viz::set_viz_subscribed(want_viz);
    }

    fn eval_hover_alpha(&mut self, key_id: u64, is_hovered: bool) -> f32 {
        if self.settings.reduce_motion {
            return if is_hovered { 1.0 } else { 0.0 };
        }
        self.visited_hover_keys.insert(key_id);
        let cur = self.hover_alphas.get(&key_id).copied().unwrap_or(0.0);
        let dt = self.frame_dt.clamp(0.001, 0.05);
        let next = if is_hovered {
            cur + (1.0 - cur) * (1.0 - (-24.0 * dt).exp())
        } else {
            cur * (-5.8 * dt).exp()
        };
        if next <= 0.008 && !is_hovered {
            self.hover_alphas.remove(&key_id);
            0.0
        } else {
            self.hover_alphas.insert(key_id, next);
            next
        }
    }

    #[inline]
    pub fn glass_off(&self) -> bool {
        !self.settings.liquid_glass_enabled || self.settings.reduce_transparency
    }

    /// Returns `true` when any high-refresh interactive animation, spring, scroll, Lyrics, or Visualizer is active.
    pub fn has_active_ui_motion(&self) -> bool {
        if self.playback_loading
            || self.active_drag.is_some()
            || self.playlist_diff_anim.is_some()
        {
            return true;
        }
        if !self.top_bar_guard_spring.is_settled_norm() {
            return true;
        }
        if (self.route == PageRoute::Visualizer && !self.show_lyrics_overlay)
            || self.show_lyrics_overlay
        {
            return true;
        }
        if self.route_trans < 0.998 {
            return true;
        }
        let want_lyr = if self.show_lyrics_overlay { 1.0f32 } else { 0.0f32 };
        if (self.lyrics_open_spring.value - want_lyr).abs() > 0.004
            || self.lyrics_open_spring.velocity.abs() > 0.02
        {
            return true;
        }
        if !self.sidebar_open_spring.is_settled_norm()
            || !self.sec_library_spring.is_settled_norm()
            || !self.sec_playlists_spring.is_settled_norm()
            || !self.sidebar_edit_spring.is_settled_norm()
            || !self.sidebar_sel_y_spring.is_settled()
            || !self.sidebar_sel_alpha_spring.is_settled_norm()
        {
            return true;
        }
        if (self.scroll_y - self.scroll_target_y).abs() > 0.25 || self.scroll_vel_y.abs() > 1.5 {
            return true;
        }
        if (self.sidebar_scroll_y - self.sidebar_scroll_target_y).abs() > 0.25
            || self.sidebar_scroll_vel_y.abs() > 1.5
        {
            return true;
        }
        if !self.hover_alphas.is_empty() {
            return true;
        }
        if self
            .art_fading_until
            .is_some_and(|t| Instant::now() < t)
        {
            return true;
        }
        false
    }

    /// Returns `(dxgi_sync_interval, resolved_hz, software_interval_ns)` for the current active page,
    /// monitor refresh rate, window focus state, and independent Visualizer/Lyrics settings.
    pub fn effective_frame_pacing(&self) -> (u32, u32, Option<u64>) {
        let mon = self.current_monitor_hz.clamp(30, 500);
        if self.show_lyrics_overlay || self.lyrics_open_spring.value > 0.004 {
            let focused = resolve_cap_sync_and_hz(
                self.settings.lyrics_fps_cap,
                mon,
                self.settings.lyrics_custom_fps,
            );
            if !self.is_window_focused && self.settings.lyrics_limit_unfocused_fps {
                let adap_hz = resolve_adaptive_60_72_hz(mon);
                if focused.1 < adap_hz {
                    return focused;
                }
                return resolve_cap_sync_and_hz(FpsCap::Adaptive60_72, mon, adap_hz);
            }
            return focused;
        }
        if self.route == PageRoute::Visualizer {
            return resolve_cap_sync_and_hz(
                self.settings.viz_fps_cap,
                mon,
                self.settings.viz_custom_fps,
            );
        }
        if self.has_active_ui_motion() {
            return resolve_cap_sync_and_hz(FpsCap::VSync, mon, 120);
        }
        (0, 60, Some(16_400_000))
    }

    pub fn needs_continuous_animation(&self) -> bool {
        if self.playback_loading || self.playlist_diff_anim.is_some() {
            return true;
        }
        if !self.top_bar_guard_spring.is_settled_norm() {
            return true;
        }
        if self.route_trans < 0.998 {
            return true;
        }
        let want_lyr = if self.show_lyrics_overlay { 1.0f32 } else { 0.0f32 };
        if (self.lyrics_open_spring.value - want_lyr).abs() > 0.004
            || self.lyrics_open_spring.velocity.abs() > 0.02
        {
            return true;
        }
        if !self.sidebar_open_spring.is_settled_norm()
            || !self.sec_library_spring.is_settled_norm()
            || !self.sec_playlists_spring.is_settled_norm()
            || !self.sidebar_edit_spring.is_settled_norm()
            || !self.sidebar_sel_y_spring.is_settled()
            || !self.sidebar_sel_alpha_spring.is_settled_norm()
        {
            return true;
        }
        if (self.scroll_y - self.scroll_target_y).abs() > 0.25 || self.scroll_vel_y.abs() > 1.5 {
            return true;
        }
        if (self.sidebar_scroll_y - self.sidebar_scroll_target_y).abs() > 0.25
            || self.sidebar_scroll_vel_y.abs() > 1.5
        {
            return true;
        }
        if !self.hover_alphas.is_empty() {
            return true;
        }
        if self
            .art_fading_until
            .is_some_and(|t| Instant::now() < t)
        {
            return true;
        }
        if self.last_search_typed.is_some() {
            return true;
        }
        if self.smtc.status == "Playing" || self.eq_bars_smooth.iter().any(|&v| v > 0.015) {
            return true;
        }
        if self.show_lyrics_overlay {
            if self.lyrics_view.has_active_layout_motion() {
                return true;
            }
            if (self.lyrics_view.cover_scale - 0.83).abs() > 0.005 {
                return true;
            }
            if self.lyrics_view.animator.user_scroll_offset.abs() > 0.25
                || self.lyrics_view.animator.user_scroll_vel.abs() > 0.5
            {
                return true;
            }
            if self
                .lyrics_view
                .animator
                .line_states
                .iter()
                .any(|s| {
                    !s.y_spring.is_settled()
                        || !s.press_spring.is_settled()
                        || !s.blur_spring.is_settled()
                        || !s.active_blend.is_settled_norm()
                        || s.char_lifts.iter().any(|l| !l.is_settled_norm())
                        || s.bg_char_lifts.iter().any(|l| !l.is_settled_norm())
                })
            {
                return true;
            }
        }
        if self.route == PageRoute::Visualizer
            && !self.show_lyrics_overlay
            && !audio_viz::is_inspector_paused()
        {
            if audio_viz::is_audio_currently_active() || self.smtc.status == "Playing" {
                return true;
            }
            if self.latest_viz_snap.spectrum_32.iter().any(|&v| v > 0.008)
                || self.latest_viz_snap.peaks_64.iter().any(|&v| v > 0.01)
                || self.viz_state.has_settling_motion()
            {
                return true;
            }
        }
        false
    }

    /// Returns true if `(lx, ly)` (logical DIP) hovers an interactive button, input box, or slider.
    /// Used by `WM_NCHITTEST` (so empty immersive top-bar drags window while buttons remain clickable)
    /// and `WM_SETCURSOR` (for crisp pointer cursor feedback).
    pub fn is_interactive_at(&self, lx: f32, ly: f32) -> bool {
        if self.show_lyrics_overlay {
            return self
                .lyrics_view
                .hit_zones
                .iter()
                .any(|hz| hz.rect.contains(lx, ly));
        }
        if self.search_box_rect.contains(lx, ly) {
            return true;
        }
        if self.route == PageRoute::Settings && self.token_box_rect.contains(lx, ly) {
            return true;
        }
        if self.route == PageRoute::Visualizer && !self.show_queue_popover && !self.show_more_popover
        {
            if self
                .viz_state
                .hit_zones
                .iter()
                .any(|hz| hz.rect.contains(lx, ly))
            {
                return true;
            }
        }
        self.hit_regions.iter().any(|hr| {
            !matches!(hr.action, UiAction::SwallowClick) && hr.rect.contains(lx, ly)
        })
    }

    #[inline]
    pub fn playhead_anchor_snapshot(&self) -> (f64, Instant) {
        (self.playhead_anchor_sec, self.playhead_anchor_inst)
    }

    #[inline]
    pub fn has_pending_artwork(&self) -> bool {
        !self.pending_art.is_empty()
    }

    pub fn drain_messages(&mut self, renderer: &mut D2DRenderer) -> bool {
        let mut changed = false;

        // Same-frame (<4ms) direct pull whenever AMPLibraryAgent emits a state/track/duration callback
        let cur_agent_seq = am_agent::state_seq();
        if cur_agent_seq != self.last_agent_seq {
            self.last_agent_seq = cur_agent_seq;
            let st = am_agent::get_playback_state();
            self.on_playback_state_changed(st);
            changed = true;
        }

        // Immediate non-blocking snapshot pull on WinRT SMTC rollover / media events (no 420ms poll wait!)
        let cur_smtc_seq = smtc::smtc_event_seq();
        if cur_smtc_seq != self.last_smtc_event_seq {
            self.last_smtc_event_seq = cur_smtc_seq;
            if self.wake_hwnd != 0
                && !am_agent::is_agent_active()
                && !self
                    .smtc_event_pull_in_flight
                    .swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                let inflight = self.smtc_event_pull_in_flight.clone();
                self.rt.spawn(async move {
                    let snap = tokio::task::spawn_blocking(unified_playback_snapshot)
                        .await
                        .unwrap_or_default();
                    inflight.store(false, std::sync::atomic::Ordering::Release);
                    let _ = tx.send(AsyncMsg::PlaybackUpdated(snap));
                    Self::post_ui_wake(hwnd);
                });
            }
        }

        while let Ok(msg) = self.rx.try_recv() {
            changed = true;
            match msg {
                AsyncMsg::HomeLoaded {
                    albums,
                    playlists,
                    songs,
                } => {
                    self.home_albums = albums;
                    self.home_playlists = playlists;
                    self.home_songs = songs;
                }
                AsyncMsg::RadioLoaded(cards) => {
                    self.radio_cards = cards;
                }
                AsyncMsg::SearchResults {
                    query,
                    albums,
                    playlists,
                    songs,
                } => {
                    if query == self.search_query.trim() {
                        self.search_albums = albums;
                        self.search_playlists = playlists;
                        self.search_songs = songs;
                    }
                }
                AsyncMsg::LibraryCardsLoaded { route_key, cards } => {
                    self.lib_cards.insert(route_key, cards);
                }
                AsyncMsg::LibrarySongsLoaded(songs) => {
                    self.lib_songs = songs;
                }
                AsyncMsg::SidebarPlaylistsLoaded(pls) => {
                    self.playlist_cache
                        .upsert_sidebar(&pls, current_unix_sec());
                    self.save_playlist_cache_async();
                    self.sidebar_playlists = pls;
                }
                AsyncMsg::ContainerLoaded(res) => {
                    self.detail_loading = false;
                    match res {
                        Ok(data) => self.apply_loaded_container_detail(data),
                        Err(e) => {
                            if self.detail_data.is_none() {
                                self.set_banner(e, true);
                            }
                        }
                    }
                }
                AsyncMsg::PlaylistBackgroundUpdated(data) => {
                    let is_open = matches!(
                        self.route,
                        PageRoute::ContainerDetail { ref id, .. } if id == &data.id
                    ) && self
                        .detail_data
                        .as_ref()
                        .is_some_and(|d| d.id == data.id);
                    if is_open {
                        self.apply_loaded_container_detail(data);
                    } else if data.kind.contains("playlist") {
                        self.playlist_cache
                            .upsert_detail(&data, current_unix_sec());
                        self.save_playlist_cache_async();
                    }
                }
                AsyncMsg::PeriodicPlaylistSyncDue => {
                    self.playlist_sync_in_flight = false;
                    if !self.settings.user_token.is_empty() {
                        self.dispatch_background_playlist_sync();
                    }
                }
                AsyncMsg::DownloadsLoaded(res) => match res {
                    Ok(info) => self.downloads_info = info,
                    Err(e) => self.set_banner(e, true),
                },
                AsyncMsg::PlaybackUpdated(mut st) => {
                    if st.artwork_url.is_none() {
                        if let Some(ref b64) = st.thumbnail_base64 {
                            let smtc_key = format!("smtc://{}\x1f{}", st.title, st.artist);
                            if !renderer.has_artwork(&smtc_key, true) {
                                use base64::{engine::general_purpose, Engine as _};
                                if let Ok(raw) = general_purpose::STANDARD.decode(b64) {
                                    renderer.insert_artwork_bytes(smtc_key.clone(), &raw, true);
                                    renderer.insert_artwork_bytes(smtc_key.clone(), &raw, false);
                                }
                            }
                            if renderer.has_artwork(&smtc_key, true) {
                                st.artwork_url = Some(smtc_key);
                            }
                        }
                    }
                    self.on_playback_state_changed(st);
                }
                AsyncMsg::AgentStatusUpdated(st) => {
                    self.agent_status = st;
                }
                AsyncMsg::AudioPrefsLoaded(p) => {
                    self.audio_prefs = p;
                }
                AsyncMsg::DevicesLoaded(devs) => {
                    self.devices = devs;
                }
                AsyncMsg::BinauralUpdated(b) => {
                    self.binaural = b;
                }
                AsyncMsg::AuthVerified(res) => {
                    let msg = res.message.clone();
                    let is_err = !res.dev_token_ok || (res.has_user_token && !res.user_token_ok);
                    if res.user_token_ok {
                        if let Some(ref det_sf) = res.detected_storefront {
                            if !det_sf.is_empty() && det_sf != &self.settings.storefront {
                                self.settings.storefront = det_sf.clone();
                                self.settings.save();
                                self.fetch_home_charts();
                            }
                        }
                        self.dispatch_background_playlist_sync();
                    }
                    self.auth_result = Some(res);
                    self.set_banner(msg, is_err);
                }
                AsyncMsg::ArtworkBytes { url, hero, bytes } => {
                    let stem = am_api::artwork_canonical_stem(&url);
                    self.pending_art.remove(&format!("{hero}:{stem}"));
                    if renderer.insert_artwork_bytes(url, &bytes, hero)
                        && renderer.has_fading_artwork()
                    {
                        self.art_fading_until =
                            Some(Instant::now() + std::time::Duration::from_millis(210));
                    }
                }
                AsyncMsg::ArtworkDecoded {
                    req_key,
                    url,
                    hero,
                    decoded,
                } => {
                    self.pending_art.remove(&req_key);
                    if renderer.upload_decoded_artwork(url, decoded, hero)
                        && renderer.has_fading_artwork()
                    {
                        self.art_fading_until =
                            Some(Instant::now() + std::time::Duration::from_millis(210));
                    }
                }
                AsyncMsg::ArtworkFailed { req_key } => {
                    self.pending_art.remove(&req_key);
                    if self.failed_art.len() >= 256 {
                        self.failed_art.clear();
                    }
                    self.failed_art.insert(req_key);
                }
                AsyncMsg::SmtcCatalogEnriched {
                    meta_key,
                    adam_id,
                    artwork_url,
                    audio_quality,
                    stream_specs,
                    validated_with_dur,
                } => {
                    if self.smtc_catalog_cache.len() >= 32 {
                        self.smtc_catalog_cache.clear();
                    }
                    self.smtc_catalog_cache.insert(
                        meta_key.clone(),
                        CachedSmtcEnrichment {
                            adam_id: adam_id.clone(),
                            artwork_url: artwork_url.clone(),
                            audio_quality: audio_quality.clone(),
                            stream_specs: stream_specs.clone(),
                            validated_with_dur,
                        },
                    );
                    let cur_mk = smtc_meta_key(&self.smtc.title, &self.smtc.artist);
                    if cur_mk == meta_key {
                        let mut updated = self.smtc.clone();
                        if !adam_id.is_empty() && (updated.adam_id.is_none() || validated_with_dur) {
                            updated.adam_id = Some(adam_id);
                        }
                        if let Some(art) = artwork_url {
                            let is_low_res_smtc = updated
                                .artwork_url
                                .as_deref()
                                .is_none_or(|u| u.starts_with("smtc://"));
                            if is_low_res_smtc || validated_with_dur {
                                updated.artwork_url = Some(art);
                            }
                        }
                        if updated.audio_quality.is_none() || validated_with_dur {
                            if audio_quality.is_some() {
                                updated.audio_quality = audio_quality;
                            }
                        }
                        if updated.stream_specs.is_none() || validated_with_dur {
                            if stream_specs.is_some() {
                                updated.stream_specs = stream_specs;
                            }
                        }
                        self.on_playback_state_changed(updated);
                    }
                }
                AsyncMsg::LyricsFetched { key, seq, lyrics } => {
                    if key == self.current_lyrics_key && seq == self.lyrics_req_seq {
                        self.lyrics_loading = false;
                        self.current_lyrics = lyrics;
                        self.lyrics_view.invalidate_layout_cache();
                        if self.lyrics_fetched_with_dur <= 1.0
                            && self.smtc.duration_sec > 5.0
                            && !self.smtc.title.trim().is_empty()
                        {
                            let is_id_verified_ttml = key.starts_with("id:")
                                && self
                                    .smtc
                                    .adam_id
                                    .as_deref()
                                    .is_some_and(|a| a.chars().all(|c| c.is_ascii_digit()))
                                && self.current_lyrics.as_ref().is_some_and(|l| {
                                    l.source_name.starts_with("Apple Music")
                                        || l.source_name.starts_with("AMLL")
                                        || l.source_name.contains("本地")
                                });
                            let is_manual_local = self
                                .current_lyrics
                                .as_ref()
                                .is_some_and(|l| l.source_name.contains("本地"));
                            if !is_id_verified_ttml && !is_manual_local {
                                self.dispatch_lyrics_fetch(
                                    key,
                                    self.smtc.adam_id.clone(),
                                    self.smtc.title.clone(),
                                    self.smtc.artist.clone(),
                                    self.smtc.album.clone(),
                                    self.smtc.duration_sec,
                                    true,
                                );
                            } else {
                                self.lyrics_fetched_with_dur = self.smtc.duration_sec;
                            }
                        }
                    }
                }
                AsyncMsg::BannerNotice(s) => self.set_banner(s, false),
                AsyncMsg::BannerError(s) => {
                    self.playback_loading = false;
                    self.playback_loading_started = None;
                    self.playback_loading_track_id = None;
                    self.playback_loading_container_id = None;
                    self.set_banner(s, true);
                }
            }
        }

        // Debounced live search dispatch
        if let Some(typed_at) = self.last_search_typed {
            if typed_at.elapsed() >= std::time::Duration::from_millis(380) {
                self.last_search_typed = None;
                let q = self.search_query.trim().to_string();
                if !q.is_empty() && q != self.last_dispatched_query {
                    self.dispatch_search(q);
                }
            }
        }

        // Auto-expire banner after 4.5s
        if let Some((_, _, at)) = &self.banner_text {
            if at.elapsed() >= std::time::Duration::from_millis(4500) {
                self.banner_text = None;
                changed = true;
            }
        }

        changed
    }

    fn set_banner(&mut self, msg: String, is_err: bool) {
        self.banner_text = Some((msg, is_err, Instant::now()));
    }

    pub fn begin_optimistic_playback_loading(
        &mut self,
        track: Option<&AgentTrackInput>,
        container_id: Option<String>,
    ) {
        self.playback_loading = true;
        self.playback_loading_started = Some(Instant::now());
        self.playback_loading_container_id = container_id;
        self.loading_cmd_epoch = am_agent::prepare_optimistic_play_epoch();
        self.smtc_optimistic_seek = None;
        if let Some(tr) = track {
            let aid = tr.effective_adam_id();
            self.playback_loading_track_id = if aid.is_empty() {
                None
            } else {
                Some(aid.clone())
            };
            self.smtc.adam_id = if aid.is_empty() { None } else { Some(aid) };
            self.smtc.title = tr.name.clone();
            self.smtc.artist = tr.artist_name.clone();
            self.smtc.album = tr.album_name.clone();
            if tr.artwork_url.is_some() {
                self.smtc.artwork_url = tr.artwork_url.clone();
            }
            self.smtc.duration_sec = tr.duration_sec;
            self.smtc.position_sec = 0.0;
            self.smtc.status = "Paused".to_string();
            self.smtc.backend_mode = "agent".to_string();
            self.playhead_anchor_sec = 0.0;
            self.playhead_anchor_inst = Instant::now();

            // Pre-fetch synced lyrics in parallel with audio stream buffering (`prepare_optimistic_play_epoch`
            // ensures `get_store_lyrics_via_agent` waits briefly for cold-start AMPLibraryAgent Command 5023 TTML!)
            let new_key = lyrics::make_lyrics_cache_key(
                self.smtc.adam_id.as_deref(),
                &self.smtc.title,
                &self.smtc.artist,
            );
            if !self.smtc.title.is_empty() && new_key != self.current_lyrics_key {
                self.current_lyrics_key = new_key.clone();
                self.dispatch_lyrics_fetch(
                    new_key,
                    self.smtc.adam_id.clone(),
                    self.smtc.title.clone(),
                    self.smtc.artist.clone(),
                    self.smtc.album.clone(),
                    self.smtc.duration_sec,
                    false,
                );
            }
        } else {
            self.playback_loading_track_id = None;
        }
    }

    pub fn on_playback_state_changed(&mut self, mut st: SmtcState) {
        let live_agent = am_agent::get_live_agent_playhead_ext();
        if self.playback_loading {
            let elapsed_ms = self
                .playback_loading_started
                .map(|t| t.elapsed().as_millis())
                .unwrap_or(9999);

            // Adopt resolved `storeAdamID` immediately once `resolved_epoch >= loading_cmd_epoch`
            // (eliminates the 5.2s `0.00s` freeze on `i.xxxx` library tracks & regional store aliases!)
            if let Some((ref agent_aid, _, _, _, resolved_ep)) = live_agent {
                if resolved_ep > 0 && resolved_ep >= self.loading_cmd_epoch {
                    if st.backend_mode == "agent" && st.adam_id != *agent_aid {
                        st = am_agent::get_playback_state();
                    }
                    if let Some(ref resolved_id) = agent_aid {
                        if !resolved_id.is_empty()
                            && st.adam_id.as_deref() == Some(resolved_id.as_str())
                        {
                            self.playback_loading_track_id = Some(resolved_id.clone());
                            self.smtc.adam_id = Some(resolved_id.clone());
                        }
                    }
                } else if elapsed_ms < 5200 {
                    let want_match = self
                        .playback_loading_track_id
                        .as_deref()
                        .is_some_and(|w| st.adam_id.as_deref() == Some(w));
                    if !want_match {
                        return;
                    }
                }
            }

            if let Some(ref want_aid) = self.playback_loading_track_id {
                let epoch_resolved = live_agent
                    .as_ref()
                    .is_some_and(|(_, _, _, _, ep)| *ep > 0 && *ep >= self.loading_cmd_epoch);
                let got_match = st.adam_id.as_deref() == Some(want_aid.as_str())
                    || (epoch_resolved && st.backend_mode == "agent");
                if got_match
                    && st.status == "Playing"
                    && (st.position_sec > 0.005 || st.position_is_calibrated)
                {
                    self.playback_loading = false;
                    self.playback_loading_started = None;
                    self.playback_loading_track_id = None;
                    self.playback_loading_container_id = None;
                } else if !got_match && elapsed_ms < 5200 {
                    // Ignore stale previous-track polling snapshot while new track is still buffering!
                    return;
                }
            } else if st.status == "Playing"
                && (st.position_sec > 0.005 || st.position_is_calibrated)
            {
                self.playback_loading = false;
                self.playback_loading_started = None;
                self.playback_loading_container_id = None;
            }
        }

        // Repeat queue/track support when natural end of playback is reached
        if self.repeat_on
            && self.smtc.status == "Playing"
            && st.status != "Playing"
            && !self.smtc.title.is_empty()
            && self
                .last_repeat_trigger
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(2))
        {
            let near_end = self.smtc.duration_sec > 5.0
                && (self.smtc.position_sec >= self.smtc.duration_sec - 2.4
                    || st.position_sec <= 1.2);
            if near_end {
                self.last_repeat_trigger = Some(Instant::now());
                let q = am_agent::get_queue_tracks();
                if !q.is_empty() && (am_agent::is_agent_active() || am_agent::is_agent_connected()) {
                    self.play_tracks_async(q, 0);
                } else {
                    self.control_player("seek", Some(0.0));
                    self.control_player("play", None);
                }
            }
        }

        let is_external_smtc = live_agent.is_none() && st.backend_mode != "agent";
        let prev_mk = smtc_meta_key(&self.smtc.title, &self.smtc.artist);
        let cur_mk = smtc_meta_key(&st.title, &st.artist);
        let track_meta_changed =
            !st.title.is_empty() && !self.smtc.title.is_empty() && cur_mk != prev_mk;

        // Preprocess external SMTC sample age & clamp stale previous-track timeline BEFORE catalog/lyrics lookup!
        let mut is_stale_prev_timeline = false;
        if is_external_smtc {
            if let Some(sampled) = st.sampled_at.take() {
                if st.status == "Playing" {
                    let age = sampled.elapsed().as_secs_f64().clamp(0.0, 2.0);
                    st.position_sec += age;
                    if st.duration_sec > 0.0 {
                        st.position_sec = st.position_sec.min(st.duration_sec);
                    }
                }
            } else {
                st.sampled_at = None;
            }

            if track_meta_changed {
                self.smtc_optimistic_seek = None;
                self.smtc_anchor_calibrated = false;
                if self.last_smtc_timeline_univ != 0
                    && st.timeline_updated_univ == self.last_smtc_timeline_univ
                {
                    self.smtc_stale_prev_univ = Some(st.timeline_updated_univ);
                } else {
                    self.smtc_stale_prev_univ = None;
                }
            } else if self
                .smtc_stale_prev_univ
                .is_some_and(|u| st.timeline_updated_univ != u)
            {
                self.smtc_stale_prev_univ = None;
            }

            is_stale_prev_timeline = self
                .smtc_stale_prev_univ
                .is_some_and(|u| u != 0 && st.timeline_updated_univ == u);
            if is_stale_prev_timeline
                || (track_meta_changed
                    && st.timeline_updated_univ == 0
                    && st.position_sec > 2.5
                    && (st.position_sec - self.smtc.position_sec).abs() < 2.0)
            {
                st.position_sec = 0.0;
                st.duration_sec = 0.0;
                st.position_is_calibrated = false;
                is_stale_prev_timeline = true;
            }
        } else {
            st.sampled_at = None;
        }

        // Enrich HLS stream specs & high-res artwork automatically when playing via built-in service
        if let Some(ref aid) = st.adam_id {
            if !aid.is_empty()
                && (st.stream_specs.is_none() || st.artwork_url.is_none())
                && self.enriched_adam_ids.insert(aid.clone())
            {
                let aid_c = aid.clone();
                let sf = self.settings.storefront.clone();
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.rt.spawn(async move {
                    enrich_track_hls_and_meta(&aid_c, &sf).await;
                    let snap = tokio::task::spawn_blocking(unified_playback_snapshot)
                        .await
                        .unwrap_or_default();
                    let _ = tx.send(AsyncMsg::PlaybackUpdated(snap));
                    Self::post_ui_wake(hwnd);
                });
            }
        } else if !st.title.is_empty() {
            // External SMTC session (e.g. Apple Music desktop app): enrich adam_id + Enhanced HLS specs from catalog
            // strictly gated by `lyrics::strict_catalog_match_score` (and re-verified once real `duration_sec > 5.0` arrives!)
            let mk = cur_mk.clone();
            let has_real_dur = st.duration_sec > 5.0;
            let mut need_dur_reverify = false;
            if let Some(cached) = self.smtc_catalog_cache.get(&mk) {
                if !cached.adam_id.is_empty() {
                    st.adam_id = Some(cached.adam_id.clone());
                }
                if let Some(ref art) = cached.artwork_url {
                    if st.artwork_url.as_deref().is_none_or(|u| u.starts_with("smtc://")) {
                        st.artwork_url = Some(art.clone());
                    }
                }
                if st.audio_quality.is_none() {
                    st.audio_quality = cached.audio_quality.clone();
                }
                if st.stream_specs.is_none() {
                    st.stream_specs = cached.stream_specs.clone();
                }
                if has_real_dur && !cached.validated_with_dur {
                    need_dur_reverify = true;
                }
            }
            let enrich_tag = if has_real_dur {
                format!("smtc:{mk}:dur")
            } else {
                format!("smtc:{mk}:init")
            };
            if (st.adam_id.is_none() || need_dur_reverify)
                && self.wake_hwnd != 0
                && self.enriched_adam_ids.insert(enrich_tag)
            {
                let title_q = st.title.clone();
                let artist_q = st.artist.clone();
                let album_q = st.album.clone();
                let dur_q = st.duration_sec;
                let sf = self.settings.storefront.clone();
                let utok = non_empty_opt(&self.settings.user_token);
                let prefs = self.audio_prefs.clone();
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.rt.spawn(async move {
                    if let Some((aid, art, badge, specs)) = lookup_smtc_track_in_catalog(
                        &title_q, &artist_q, &album_q, dur_q, &sf, utok, &prefs,
                    )
                    .await
                    {
                        let _ = tx.send(AsyncMsg::SmtcCatalogEnriched {
                            meta_key: mk,
                            adam_id: aid,
                            artwork_url: art,
                            audio_quality: badge,
                            stream_specs: specs,
                            validated_with_dur: dur_q > 5.0,
                        });
                        Self::post_ui_wake(hwnd);
                    }
                });
            }
        }

        // Check if current track changed -> reset forensic envelope (when auto-reset enabled) & fetch Synced Lyrics!
        let new_key = lyrics::make_lyrics_cache_key(
            st.adam_id.as_deref(),
            &st.title,
            &st.artist,
        );
        if !st.title.is_empty() && new_key != self.current_lyrics_key {
            if !self.current_lyrics_key.is_empty()
                && track_meta_changed
                && audio_viz::is_auto_reset_silence()
            {
                audio_viz::trigger_inspector_reset();
            }
            // Preserve currently displayed lyrics smoothly while upgrading key (`meta:` -> `id:` or `i.xxx` -> `storeAdamID`) on the same song
            let saved_same_song_lyrics = if !track_meta_changed
                && !self.smtc.title.is_empty()
                && self.current_lyrics.is_some()
            {
                self.current_lyrics.clone()
            } else {
                None
            };
            self.current_lyrics_key = new_key.clone();
            self.dispatch_lyrics_fetch(
                new_key,
                st.adam_id.clone(),
                st.title.clone(),
                st.artist.clone(),
                st.album.clone(),
                st.duration_sec,
                false,
            );
            if let Some(existing) = saved_same_song_lyrics {
                if self.current_lyrics.is_none() {
                    self.current_lyrics = Some(existing);
                }
            }
        } else if !self.lyrics_loading
            && !st.title.is_empty()
            && new_key == self.current_lyrics_key
            && self.lyrics_fetched_with_dur <= 1.0
            && st.duration_sec > 5.0
        {
            // P0-4: Initial fetch completed while `duration_sec <= 1.0` (before real track duration arrived);
            // re-dispatch bypassing cache whenever fallback/empty lyrics could be wrong-edition!
            let is_id_verified_ttml = new_key.starts_with("id:")
                && st
                    .adam_id
                    .as_deref()
                    .is_some_and(|a| a.chars().all(|c| c.is_ascii_digit()))
                && self.current_lyrics.as_ref().is_some_and(|l| {
                    l.source_name.starts_with("Apple Music")
                        || l.source_name.starts_with("AMLL")
                        || l.source_name.contains("本地")
                });
            let is_manual_local = self
                .current_lyrics
                .as_ref()
                .is_some_and(|l| l.source_name.contains("本地"));
            if !is_id_verified_ttml && !is_manual_local {
                self.dispatch_lyrics_fetch(
                    new_key,
                    st.adam_id.clone(),
                    st.title.clone(),
                    st.artist.clone(),
                    st.album.clone(),
                    st.duration_sec,
                    true,
                );
            } else {
                self.lyrics_fetched_with_dur = st.duration_sec;
            }
        }

        // Preserve live scrub thumb position while user is actively dragging a seek bar;
        // otherwise anchor directly to AMPLibraryAgent's monotonic clock (zero async-channel lag!)
        // or zero-sawtooth external SMTC monotonic anchor.
        let now = Instant::now();
        if matches!(
            self.active_drag,
            Some(ActiveDrag::LyricsSeek(_) | ActiveDrag::MiniPlayerSeek(_))
        ) {
            st.position_sec = self.smtc.position_sec;
        } else if let Some((agent_aid, agent_status, live_pos, live_dur, resolved_ep)) = live_agent
        {
            let epoch_ready =
                !self.playback_loading || (resolved_ep > 0 && resolved_ep >= self.loading_cmd_epoch);
            if epoch_ready && agent_aid.is_some() && st.adam_id != agent_aid {
                st.adam_id = agent_aid.clone();
            }
            if epoch_ready && (agent_aid == st.adam_id || st.adam_id.is_none()) {
                st.position_sec = live_pos;
                if live_dur > 0.5 && st.duration_sec <= 0.5 {
                    st.duration_sec = live_dur;
                }
                st.status = agent_status;
                self.playhead_anchor_sec = live_pos;
                self.playhead_anchor_inst = now;
            } else {
                self.playhead_anchor_sec = st.position_sec;
                self.playhead_anchor_inst = now;
            }
        } else {
            // External SMTC (`#6` P1-3 & P1-4 + `#7` Section 1):
            // 1) Suppress pre-seek SMTC snapback during `smtc_optimistic_seek` (up to 2.5s until `timeline_updated_univ` updates near target)
            // 2) Leave `(playhead_anchor_sec, playhead_anchor_inst)` untouched when `timeline_updated_univ == last_smtc_timeline_univ` (zero 420ms sawtooth!)
            let mut handled_by_seek_guard = false;
            if let Some((seek_target, seek_inst)) = self.smtc_optimistic_seek {
                let seek_elapsed = seek_inst.elapsed().as_secs_f64();
                if seek_elapsed < 2.5 {
                    let expected = if st.status == "Playing" {
                        seek_target + seek_elapsed
                    } else {
                        seek_target
                    };
                    let expected_clamped = if st.duration_sec > 0.0 {
                        expected.min(st.duration_sec)
                    } else {
                        expected
                    };
                    let univ_updated = !is_stale_prev_timeline
                        && st.timeline_updated_univ != 0
                        && st.timeline_updated_univ != self.smtc_seek_pre_univ;
                    let tol = if st.position_is_calibrated { 0.55 } else { 1.15 };
                    if univ_updated && (st.position_sec - expected_clamped).abs() <= tol {
                        self.smtc_optimistic_seek = None;
                        if st.position_is_calibrated {
                            self.playhead_anchor_sec = st.position_sec;
                            self.playhead_anchor_inst = now;
                            self.smtc_anchor_calibrated = true;
                        } else {
                            st.position_sec = expected_clamped;
                            self.playhead_anchor_sec = expected_clamped;
                            self.playhead_anchor_inst = now;
                        }
                        if st.timeline_updated_univ != 0 {
                            self.last_smtc_timeline_univ = st.timeline_updated_univ;
                        }
                        handled_by_seek_guard = true;
                    } else {
                        st.position_sec = expected_clamped;
                        if st.status != "Playing" {
                            self.playhead_anchor_sec = expected_clamped;
                            self.playhead_anchor_inst = now;
                        }
                        handled_by_seek_guard = true;
                    }
                } else {
                    self.smtc_optimistic_seek = None;
                }
            }

            if !handled_by_seek_guard {
                let same_song = !track_meta_changed && !self.smtc.title.is_empty() && cur_mk == prev_mk;
                if is_stale_prev_timeline {
                    st.position_sec = 0.0;
                    self.playhead_anchor_sec = 0.0;
                    self.playhead_anchor_inst = now;
                } else if same_song && st.status == "Playing" && self.smtc.status == "Playing" {
                    let pred =
                        self.playhead_anchor_sec + self.playhead_anchor_inst.elapsed().as_secs_f64();
                    let pred_clamped = if st.duration_sec > 0.0 {
                        pred.min(st.duration_sec)
                    } else {
                        pred
                    };
                    if st.timeline_updated_univ != 0
                        && st.timeline_updated_univ == self.last_smtc_timeline_univ
                    {
                        // Identical SMTC timeline timestamp across polls -> leave monotonic anchor untouched (zero 420ms sawtooth!)
                        st.position_sec = pred_clamped;
                    } else {
                        let err = st.position_sec - pred;
                        if st.position_is_calibrated {
                            if !self.smtc_anchor_calibrated {
                                self.playhead_anchor_sec = st.position_sec;
                                self.playhead_anchor_inst = now;
                                self.smtc_anchor_calibrated = true;
                            } else if err.abs() <= 0.08 {
                                st.position_sec = pred_clamped;
                            } else if err.abs() <= 0.65 {
                                let nudged = (pred + err * 0.35).max(pred - 0.015).max(0.0);
                                st.position_sec = nudged;
                                self.playhead_anchor_sec = nudged;
                                self.playhead_anchor_inst = now;
                            } else {
                                self.playhead_anchor_sec = st.position_sec;
                                self.playhead_anchor_inst = now;
                            }
                        } else if self.smtc_anchor_calibrated && err.abs() <= 1.50 {
                            st.position_sec = pred_clamped;
                        } else if err.abs() <= 0.50 {
                            st.position_sec = pred_clamped;
                        } else if err.abs() <= 1.35 {
                            let nudged = (pred + err * 0.25).max(pred).max(0.0);
                            st.position_sec = nudged;
                            self.playhead_anchor_sec = nudged;
                            self.playhead_anchor_inst = now;
                        } else {
                            self.playhead_anchor_sec = st.position_sec;
                            self.playhead_anchor_inst = now;
                            self.smtc_anchor_calibrated = false;
                        }
                        if st.timeline_updated_univ != 0 {
                            self.last_smtc_timeline_univ = st.timeline_updated_univ;
                        }
                    }
                } else if same_song && self.smtc.status == "Playing" && st.status != "Playing" {
                    let pred =
                        self.playhead_anchor_sec + self.playhead_anchor_inst.elapsed().as_secs_f64();
                    if !st.position_is_calibrated
                        && st.position_sec < pred
                        && (pred - st.position_sec) <= 1.10
                    {
                        st.position_sec = if st.duration_sec > 0.0 {
                            pred.min(st.duration_sec)
                        } else {
                            pred
                        };
                    }
                    self.playhead_anchor_sec = st.position_sec;
                    self.playhead_anchor_inst = now;
                    self.smtc_anchor_calibrated = false;
                    if st.timeline_updated_univ != 0 {
                        self.last_smtc_timeline_univ = st.timeline_updated_univ;
                    }
                } else if same_song && self.smtc.status != "Playing" && st.status == "Playing" {
                    if !st.position_is_calibrated
                        && (st.position_sec - self.playhead_anchor_sec).abs() <= 1.15
                    {
                        st.position_sec = self.playhead_anchor_sec;
                    } else {
                        self.playhead_anchor_sec = st.position_sec;
                        self.smtc_anchor_calibrated = st.position_is_calibrated;
                    }
                    self.playhead_anchor_inst = now;
                    if st.timeline_updated_univ != 0 {
                        self.last_smtc_timeline_univ = st.timeline_updated_univ;
                    }
                } else if same_song
                    && st.status != "Playing"
                    && self.smtc.status != "Playing"
                    && self.last_smtc_timeline_univ != 0
                    && st.timeline_updated_univ == self.last_smtc_timeline_univ
                {
                    st.position_sec = self.playhead_anchor_sec;
                } else {
                    self.playhead_anchor_sec = st.position_sec;
                    self.playhead_anchor_inst = now;
                    self.smtc_anchor_calibrated = st.position_is_calibrated;
                    if st.timeline_updated_univ != 0 {
                        self.last_smtc_timeline_univ = st.timeline_updated_univ;
                    }
                }
            }
        }

        if matches!(
            self.active_drag,
            Some(ActiveDrag::LyricsVolume(_) | ActiveDrag::PopoverVolume(_))
        ) {
            st.volume = self.smtc.volume;
        }

        self.smtc = st;
        self.update_viz_subscription();
    }

    fn dispatch_lyrics_fetch(
        &mut self,
        cache_key: String,
        adam_id: Option<String>,
        title: String,
        artist: String,
        album: String,
        duration_sec: f64,
        bypass_cache: bool,
    ) {
        self.lyrics_fetched_with_dur = duration_sec;
        self.lyrics_req_seq = self.lyrics_req_seq.wrapping_add(1);
        let req_seq = self.lyrics_req_seq;
        let pref = self.lyrics_view.source_pref;
        let scoped_cache_key = if pref == lyrics::LyricsSourcePref::Auto {
            cache_key.clone()
        } else {
            format!("{}#src:{:?}", cache_key, pref)
        };
        let can_use_cache = !bypass_cache
            && (adam_id.as_deref().is_some_and(|s| !s.trim().is_empty()) || duration_sec > 5.0);
        if can_use_cache {
            if let Some(Some(parsed)) = lyrics::get_cached_lyrics(&scoped_cache_key) {
                self.current_lyrics = Some(parsed);
                self.lyrics_view.invalidate_layout_cache();
                self.lyrics_loading = false;
                return;
            }
        }
        if !(bypass_cache && self.current_lyrics.is_some()) {
            self.current_lyrics = None;
        }
        if self.wake_hwnd == 0 {
            self.lyrics_loading = false;
            return;
        }
        self.lyrics_loading = true;
        let sf = self.settings.storefront.clone();
        let utok = non_empty_opt(&self.settings.user_token);
        let use_agent_lyrics = self.settings.use_builtin_service
            && (self.playback_loading
                || am_agent::is_agent_active()
                || am_agent::is_agent_connected());
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            let res = lyrics::fetch_lyrics_with_pref_ex(
                pref,
                adam_id,
                title,
                artist,
                album,
                duration_sec,
                sf,
                utok,
                bypass_cache,
                use_agent_lyrics,
            )
            .await;
            let _ = tx.send(AsyncMsg::LyricsFetched {
                key: cache_key,
                seq: req_seq,
                lyrics: res,
            });
            Self::post_ui_wake(hwnd);
        });
    }

    pub fn request_artwork(&mut self, renderer: &D2DRenderer, url: &str, hero: bool) {
        if url.is_empty() || url.starts_with("smtc://") || renderer.has_artwork(url, hero) {
            return;
        }
        let stem = am_api::artwork_canonical_stem(url);
        let req_key = format!("{hero}:{stem}");
        if self.failed_art.contains(&req_key)
            || (!hero && self.pending_art.len() > 140)
            || !self.pending_art.insert(req_key.clone())
        {
            return;
        }
        let target_url = if hero {
            url.replace("360x360", "600x600")
                .replace("240x240", "600x600")
                .replace("120x120", "600x600")
        } else {
            url.replace("600x600", "240x240")
                .replace("360x360", "240x240")
        };
        let key_url = url.to_string();
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        let max_dim = if hero { 560u32 } else { 160u32 };
        self.rt.spawn(async move {
            match am_api::fetch_artwork_bytes(&target_url, hero).await {
                Ok(bytes) => {
                    // Decode & scale WIC pixels off the UI thread so fast scrolling never stalls D2D!
                    let decoded_opt = tokio::task::spawn_blocking(move || {
                        D2DRenderer::decode_artwork_pixels_cpu(&bytes, max_dim)
                    })
                    .await
                    .ok()
                    .flatten();
                    if let Some(decoded) = decoded_opt {
                        let _ = tx.send(AsyncMsg::ArtworkDecoded {
                            req_key,
                            url: key_url,
                            hero,
                            decoded,
                        });
                        Self::post_ui_wake(hwnd);
                    } else {
                        let _ = tx.send(AsyncMsg::ArtworkFailed { req_key });
                    }
                }
                Err(_) => {
                    let _ = tx.send(AsyncMsg::ArtworkFailed { req_key });
                }
            }
        });
    }

    // ========================================================================
    // Backend Data Fetchers
    // ========================================================================

    pub fn fetch_home_charts(&self) {
        let sf = self.settings.storefront.clone();
        let utok = non_empty_opt(&self.settings.user_token);
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            let path = format!(
                "v1/catalog/{sf}/charts?types=albums,playlists,songs&extend=audioTraits&limit=18&l=zh-Hans-CN"
            );
            for attempt in 0..2 {
                match am_api::fetch_am_api(path.clone(), utok.clone(), None, Some(sf.clone())).await
                {
                    Ok(json) => {
                        let albums =
                            parse_media_cards_array(json.pointer("/results/albums/0/data"), "albums");
                        let playlists = parse_media_cards_array(
                            json.pointer("/results/playlists/0/data"),
                            "playlists",
                        );
                        let songs = json
                            .pointer("/results/songs/0/data")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| parse_track_from_am_json(v, None, None))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let _ = tx.send(AsyncMsg::HomeLoaded {
                            albums,
                            playlists,
                            songs,
                        });
                        Self::post_ui_wake(hwnd);
                        return;
                    }
                    Err(e) => {
                        if attempt == 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                        } else {
                            let _ = tx.send(AsyncMsg::BannerError(format!("获取精选内容失败: {e}")));
                            Self::post_ui_wake(hwnd);
                        }
                    }
                }
            }
        });
    }

    pub fn fetch_radio_mixes(&self) {
        let sf = self.settings.storefront.clone();
        let utok = non_empty_opt(&self.settings.user_token);
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            let path = format!(
                "v1/catalog/{sf}/search?term=Apple+Music+%E7%94%B5%E5%8F%B0&types=playlists&limit=18&l=zh-Hans-CN"
            );
            if let Ok(json) = am_api::fetch_am_api(path, utok, None, Some(sf)).await {
                let cards =
                    parse_media_cards_array(json.pointer("/results/playlists/data"), "playlists");
                let _ = tx.send(AsyncMsg::RadioLoaded(cards));
                Self::post_ui_wake(hwnd);
            }
        });
    }

    pub fn dispatch_search(&mut self, query: String) {
        if query.trim().is_empty() {
            return;
        }
        self.last_dispatched_query = query.clone();
        if self.route != PageRoute::Search {
            self.navigate_to(PageRoute::Search);
        }
        let sf = self.settings.storefront.clone();
        let utok = non_empty_opt(&self.settings.user_token);
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        let enc: String = url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect();
        self.rt.spawn(async move {
            let path = format!(
                "v1/catalog/{sf}/search?term={enc}&types=songs,albums,playlists&extend=audioTraits&limit=20&l=zh-Hans-CN"
            );
            if let Ok(json) = am_api::fetch_am_api(path, utok, None, Some(sf)).await {
                let albums =
                    parse_media_cards_array(json.pointer("/results/albums/data"), "albums");
                let playlists =
                    parse_media_cards_array(json.pointer("/results/playlists/data"), "playlists");
                let songs = json
                    .pointer("/results/songs/data")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| parse_track_from_am_json(v, None, None))
                            .collect()
                    })
                    .unwrap_or_default();
                let _ = tx.send(AsyncMsg::SearchResults {
                    query,
                    albums,
                    playlists,
                    songs,
                });
                Self::post_ui_wake(hwnd);
            }
        });
    }

    pub fn fetch_sidebar_playlists(&self) {
        let Some(utok) = non_empty_opt(&self.settings.user_token) else {
            return;
        };
        let sf = self.settings.storefront.clone();
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            let path = "v1/me/library/playlists?limit=24&l=zh-Hans-CN".to_string();
            if let Ok(json) = am_api::fetch_am_api(path, Some(utok), None, Some(sf)).await {
                let cards = parse_media_cards_array(json.get("data"), "library-playlists");
                let _ = tx.send(AsyncMsg::SidebarPlaylistsLoaded(cards));
                Self::post_ui_wake(hwnd);
            }
        });
    }

    pub fn fetch_library_route(&self, route: &PageRoute) {
        if matches!(route, PageRoute::LibDownloads) {
            let tx = self.tx.clone();
            let hwnd = self.wake_hwnd;
            self.rt.spawn(async move {
                let res = tokio::task::spawn_blocking(|| {
                    let hint = am_agent::get_cached_library_url();
                    downloads::list_downloads(hint.as_deref())
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let _ = tx.send(AsyncMsg::DownloadsLoaded(res));
                Self::post_ui_wake(hwnd);
            });
            return;
        }

        let Some(utok) = non_empty_opt(&self.settings.user_token) else {
            return;
        };
        let sf = self.settings.storefront.clone();
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;

        match route {
            PageRoute::LibSongs => {
                self.rt.spawn(async move {
                    let path =
                        "v1/me/library/songs?include=catalog&extend=audioTraits&limit=80&l=zh-Hans-CN"
                            .to_string();
                    if let Ok(json) = am_api::fetch_am_api(path, Some(utok), None, Some(sf)).await {
                        let songs = json
                            .get("data")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| parse_track_from_am_json(v, None, None))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let _ = tx.send(AsyncMsg::LibrarySongsLoaded(songs));
                        Self::post_ui_wake(hwnd);
                    }
                });
            }
            PageRoute::LibRecent
            | PageRoute::LibAlbums
            | PageRoute::LibArtists
            | PageRoute::LibPlaylists
            | PageRoute::LibForYou => {
                let (rkey, path, default_kind) = match route {
                    PageRoute::LibRecent => (
                        "recent",
                        "v1/me/library/recently-added?limit=25&l=zh-Hans-CN",
                        "library-albums",
                    ),
                    PageRoute::LibAlbums => (
                        "albums",
                        "v1/me/library/albums?limit=40&l=zh-Hans-CN",
                        "library-albums",
                    ),
                    PageRoute::LibArtists => (
                        "artists",
                        "v1/me/library/artists?limit=40&l=zh-Hans-CN",
                        "library-artists",
                    ),
                    PageRoute::LibPlaylists => (
                        "playlists",
                        "v1/me/library/playlists?limit=40&l=zh-Hans-CN",
                        "library-playlists",
                    ),
                    _ => (
                        "foryou",
                        "v1/me/recommendations?limit=12&l=zh-Hans-CN",
                        "playlists",
                    ),
                };
                let rkey_s = rkey.to_string();
                let path_s = path.to_string();
                let dkind_s = default_kind.to_string();
                self.rt.spawn(async move {
                    if let Ok(json) = am_api::fetch_am_api(path_s, Some(utok), None, Some(sf)).await
                    {
                        let cards = if rkey_s == "foryou" {
                            let mut acc = Vec::new();
                            if let Some(arr) = json.get("data").and_then(Value::as_array) {
                                for group in arr {
                                    let sub = parse_media_cards_array(
                                        group.pointer("/relationships/contents/data"),
                                        &dkind_s,
                                    );
                                    acc.extend(sub);
                                }
                            }
                            acc
                        } else {
                            parse_media_cards_array(json.get("data"), &dkind_s)
                        };
                        let _ = tx.send(AsyncMsg::LibraryCardsLoaded {
                            route_key: rkey_s,
                            cards,
                        });
                        Self::post_ui_wake(hwnd);
                    }
                });
            }
            _ => {}
        }
    }

    pub fn save_playlist_cache_async(&self) {
        let Some(path) = PlaylistDiskCache::cache_path() else {
            return;
        };
        let cache = self.playlist_cache.clone();
        self.rt.spawn_blocking(move || {
            cache.save_to_path(&path);
        });
    }

    fn apply_loaded_container_detail(&mut self, data: ContainerDetailData) {
        if data.kind.contains("playlist") {
            self.playlist_cache
                .upsert_detail(&data, current_unix_sec());
            self.save_playlist_cache_async();
        }
        let same_open_playlist = data.kind.contains("playlist")
            && matches!(
                self.route,
                PageRoute::ContainerDetail { ref id, .. } if id == &data.id
            );
        if same_open_playlist && !self.settings.reduce_motion {
            if let Some(ref prev) = self.detail_data {
                if prev.id == data.id && prev.tracks != data.tracks {
                    if let Some(entries) = compute_playlist_diff(&prev.tracks, &data.tracks) {
                        self.playlist_diff_anim = Some(PlaylistDiffAnimState {
                            container_id: data.id.clone(),
                            started_at: Instant::now(),
                            duration_sec: 0.58,
                            old_tracks: prev.tracks.clone(),
                            new_tracks: data.tracks.clone(),
                            entries,
                            bake_slots: HashMap::with_capacity(24),
                        });
                    }
                }
            }
        }
        self.detail_data = Some(data);
    }

    pub fn dispatch_background_playlist_sync(&mut self) {
        let Some(utok) = non_empty_opt(&self.settings.user_token) else {
            return;
        };
        if self.playlist_sync_in_flight {
            // Reset flag immediately on next sync cycle; coalesce rapid duplicate calls
            return;
        }
        let sf = self.settings.storefront.clone();
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;

        let mut targets: Vec<(String, String, String)> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        if let PageRoute::ContainerDetail {
            ref kind,
            ref id,
            ref title_hint,
        } = self.route
        {
            if kind.contains("playlist") && seen_ids.insert(id.clone()) {
                targets.push((kind.clone(), id.clone(), title_hint.clone()));
            }
        }
        for det in self.playlist_cache.details.values() {
            if det.kind.contains("playlist") && seen_ids.insert(det.id.clone()) {
                targets.push((det.kind.clone(), det.id.clone(), det.title.clone()));
            }
        }
        for pl in &self.sidebar_playlists {
            if pl.kind.contains("playlist") && seen_ids.insert(pl.id.clone()) {
                targets.push((pl.kind.clone(), pl.id.clone(), pl.title.clone()));
            }
        }

        self.rt.spawn(async move {
            let path = "v1/me/library/playlists?limit=24&l=zh-Hans-CN".to_string();
            if let Ok(json) = am_api::fetch_am_api(path, Some(utok.clone()), None, Some(sf.clone())).await {
                let cards = parse_media_cards_array(json.get("data"), "library-playlists");
                for c in &cards {
                    if c.kind.contains("playlist") && seen_ids.insert(c.id.clone()) {
                        targets.push((c.kind.clone(), c.id.clone(), c.title.clone()));
                    }
                }
                let _ = tx.send(AsyncMsg::SidebarPlaylistsLoaded(cards));
                Self::post_ui_wake(hwnd);
            }
            targets.truncate(14);
            for (kind, id, title_hint) in targets {
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                if let Ok(detail) =
                    Self::fetch_container_detail_api(sf.clone(), Some(utok.clone()), kind, id, title_hint)
                        .await
                {
                    let _ = tx.send(AsyncMsg::PlaylistBackgroundUpdated(detail));
                    Self::post_ui_wake(hwnd);
                }
            }
        });
    }

    pub fn fetch_container_detail(&mut self, kind: String, id: String, title_hint: String) {
        self.playlist_diff_anim = None;
        if kind.contains("playlist") {
            if let Some(cached) = self.playlist_cache.details.get(&id) {
                self.detail_data = Some(cached.to_detail());
                self.detail_loading = false;
            } else {
                self.detail_loading = true;
                self.detail_data = None;
            }
        } else {
            self.detail_loading = true;
            self.detail_data = None;
        }
        let sf = self.settings.storefront.clone();
        let utok = non_empty_opt(&self.settings.user_token);
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;

        self.rt.spawn(async move {
            let res = Self::fetch_container_detail_api(sf, utok, kind, id, title_hint).await;
            let _ = tx.send(AsyncMsg::ContainerLoaded(res));
            Self::post_ui_wake(hwnd);
        });
    }

    pub async fn fetch_container_detail_api(
        sf: String,
        utok: Option<String>,
        kind: String,
        id: String,
        title_hint: String,
    ) -> Result<ContainerDetailData, String> {
                if kind == "library-artists" || kind == "artists" {
                    let artist_name = if kind == "library-artists" {
                        let meta_p = format!("v1/me/library/artists/{id}?l=zh-Hans-CN");
                        am_api::fetch_am_api(meta_p, utok.clone(), None, Some(sf.clone()))
                            .await
                            .ok()
                            .and_then(|j| {
                                j.pointer("/data/0/attributes/name")
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string())
                            })
                            .unwrap_or_else(|| title_hint.clone())
                    } else {
                        title_hint.clone()
                    };

                    if kind == "artists" {
                        let p = format!(
                            "v1/catalog/{sf}/artists/{id}?views=top-songs&extend=audioTraits&l=zh-Hans-CN"
                        );
                        let json = am_api::fetch_am_api(p, utok, None, Some(sf)).await?;
                        let root = json
                            .pointer("/data/0")
                            .ok_or_else(|| "未找到该艺人".to_string())?;
                        let title = root
                            .pointer("/attributes/name")
                            .and_then(Value::as_str)
                            .unwrap_or(&artist_name)
                            .to_string();
                        let subtitle = root
                            .pointer("/attributes/genreNames/0")
                            .and_then(Value::as_str)
                            .map(|g| format!("艺人 · {g}"))
                            .unwrap_or_else(|| "Apple Music 艺人".to_string());
                        let description = root
                            .pointer("/attributes/editorialNotes/short")
                            .or_else(|| root.pointer("/attributes/editorialNotes/standard"))
                            .and_then(Value::as_str)
                            .map(strip_html_tags)
                            .unwrap_or_default();
                        let artwork_url = root
                            .pointer("/attributes/artwork/url")
                            .and_then(Value::as_str)
                            .map(|u| format_artwork_template(u, 600));
                        let tracks = root
                            .pointer("/views/top-songs/data")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|t| {
                                        parse_track_from_am_json(t, None, artwork_url.as_deref())
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        return Ok(ContainerDetailData {
                            id,
                            kind,
                            title,
                            subtitle,
                            description,
                            artwork_url,
                            tracks,
                        });
                    }

                    // Library artist -> search catalog for high-res portrait + playable top songs
                    let enc: String =
                        url::form_urlencoded::byte_serialize(artist_name.trim().as_bytes())
                            .collect();
                    let sp = format!(
                        "v1/catalog/{sf}/search?term={enc}&types=artists,songs&extend=audioTraits&limit=25&l=zh-Hans-CN"
                    );
                    let json = am_api::fetch_am_api(sp, utok, None, Some(sf)).await?;
                    let art_node = json.pointer("/results/artists/data/0");
                    let artwork_url = art_node
                        .and_then(|n| n.pointer("/attributes/artwork/url"))
                        .and_then(Value::as_str)
                        .map(|u| format_artwork_template(u, 600));
                    let tracks = json
                        .pointer("/results/songs/data")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| parse_track_from_am_json(t, None, artwork_url.as_deref()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Ok(ContainerDetailData {
                        id,
                        kind,
                        title: artist_name,
                        subtitle: "资料库艺人 · 热门曲目".to_string(),
                        description: String::new(),
                        artwork_url,
                        tracks,
                    })
                } else if kind == "library-albums" || kind == "library-playlists" {
                    let sub = if kind == "library-albums" {
                        "albums"
                    } else {
                        "playlists"
                    };
                    let meta_path = format!("v1/me/library/{sub}/{id}?l=zh-Hans-CN");
                    let tracks_path = format!(
                        "v1/me/library/{sub}/{id}/tracks?include=catalog&extend=audioTraits&limit=100&l=zh-Hans-CN"
                    );
                    let meta_json = am_api::fetch_am_api(meta_path, utok.clone(), None, Some(sf.clone())).await.ok();
                    let tr_json = am_api::fetch_am_api(tracks_path, utok, None, Some(sf)).await?;

                    let root = meta_json.as_ref().and_then(|j| j.pointer("/data/0"));
                    let title = root
                        .and_then(|r| r.pointer("/attributes/name"))
                        .and_then(Value::as_str)
                        .unwrap_or(&title_hint)
                        .to_string();
                    let subtitle = root
                        .and_then(|r| {
                            r.pointer("/attributes/artistName")
                                .or_else(|| r.pointer("/attributes/curatorName"))
                        })
                        .and_then(Value::as_str)
                        .unwrap_or("资料库")
                        .to_string();
                    let description = root
                        .and_then(|r| r.pointer("/attributes/description/standard"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let artwork_url = root
                        .and_then(|r| r.pointer("/attributes/artwork/url"))
                        .and_then(Value::as_str)
                        .map(|u| format_artwork_template(u, 600));

                    let tracks = tr_json
                        .get("data")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    parse_track_from_am_json(t, Some(&title), artwork_url.as_deref())
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    Ok(ContainerDetailData {
                        id,
                        kind,
                        title,
                        subtitle,
                        description,
                        artwork_url,
                        tracks,
                    })
                } else {
                    let path = format!(
                        "v1/catalog/{sf}/{kind}/{id}?include=tracks&extend=audioTraits&l=zh-Hans-CN"
                    );
                    let json = am_api::fetch_am_api(path, utok, None, Some(sf)).await?;
                    let root = json
                        .pointer("/data/0")
                        .ok_or_else(|| "未找到该专辑或播放列表".to_string())?;
                    let title = root
                        .pointer("/attributes/name")
                        .and_then(Value::as_str)
                        .unwrap_or(&title_hint)
                        .to_string();
                    let subtitle = root
                        .pointer("/attributes/artistName")
                        .or_else(|| root.pointer("/attributes/curatorName"))
                        .and_then(Value::as_str)
                        .unwrap_or("Apple Music")
                        .to_string();
                    let description = root
                        .pointer("/attributes/editorialNotes/short")
                        .or_else(|| root.pointer("/attributes/editorialNotes/standard"))
                        .or_else(|| root.pointer("/attributes/description/short"))
                        .or_else(|| root.pointer("/attributes/description/standard"))
                        .and_then(Value::as_str)
                        .map(strip_html_tags)
                        .unwrap_or_default();
                    let artwork_url = root
                        .pointer("/attributes/artwork/url")
                        .and_then(Value::as_str)
                        .map(|u| format_artwork_template(u, 600));

                    let tracks = root
                        .pointer("/relationships/tracks/data")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    parse_track_from_am_json(t, Some(&title), artwork_url.as_deref())
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    Ok(ContainerDetailData {
                        id,
                        kind,
                        title,
                        subtitle,
                        description,
                        artwork_url,
                        tracks,
                    })
                }
    }

    pub fn refresh_devices_and_prefs(&self, connect_agent: bool) {
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            if let Ok(Ok(devs)) = tokio::task::spawn_blocking(audio_viz::list_output_devices).await {
                let _ = tx.send(AsyncMsg::DevicesLoaded(devs));
            }
            if let Ok(st) = tokio::task::spawn_blocking(am_agent::get_agent_status).await {
                let _ = tx.send(AsyncMsg::AgentStatusUpdated(st));
            }
            if let Ok(Ok(p)) =
                tokio::task::spawn_blocking(move || am_agent::get_audio_prefs(connect_agent)).await
            {
                let _ = tx.send(AsyncMsg::AudioPrefsLoaded(p));
            }
            if let Ok(b) = tokio::task::spawn_blocking(|| {
                let _ = binaural_dsp::refresh_binaural_can_enable();
                binaural_dsp::get_binaural_status()
            })
            .await
            {
                let _ = tx.send(AsyncMsg::BinauralUpdated(b));
            }
            Self::post_ui_wake(hwnd);
        });
    }

    pub fn navigate_to(&mut self, next: PageRoute) {
        if self.route == next {
            return;
        }
        self.art_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let prev = std::mem::replace(&mut self.route, next.clone());
        self.nav_history.push(prev);
        if self.nav_history.len() > 24 {
            self.nav_history.remove(0);
        }
        self.route_trans = if self.settings.reduce_motion { 1.0 } else { 0.0 };
        self.route_trans_dir = 1.0;
        self.scroll_y = 0.0;
        self.scroll_target_y = 0.0;
        self.scroll_vel_y = 0.0;
        self.show_lyrics_overlay = false;
        self.show_queue_popover = false;
        self.show_more_popover = false;
        self.update_viz_subscription();

        match &next {
            PageRoute::Home | PageRoute::Browse => {
                if self.home_albums.is_empty() {
                    self.fetch_home_charts();
                }
            }
            PageRoute::Radio => {
                if self.radio_cards.is_empty() {
                    self.fetch_radio_mixes();
                }
            }
            PageRoute::LibRecent
            | PageRoute::LibArtists
            | PageRoute::LibAlbums
            | PageRoute::LibSongs
            | PageRoute::LibForYou
            | PageRoute::LibDownloads
            | PageRoute::LibPlaylists => {
                self.fetch_library_route(&next);
            }
            PageRoute::ContainerDetail {
                kind,
                id,
                title_hint,
            } => {
                self.fetch_container_detail(kind.clone(), id.clone(), title_hint.clone());
            }
            PageRoute::Settings => {
                self.refresh_devices_and_prefs(false);
            }
            PageRoute::Visualizer => {
                self.refresh_devices_and_prefs(false);
            }
            PageRoute::Search => {}
        }
    }

    // ========================================================================
    // Input & Action Dispatch
    // ========================================================================

    pub fn on_wheel(&mut self, delta_notches: f32) {
        let now = Instant::now();
        let dt_ms = self
            .last_wheel_inst
            .map(|t| now.duration_since(t).as_millis() as f32)
            .unwrap_or(500.0);
        self.last_wheel_inst = Some(now);
        if dt_ms < 95.0 {
            self.wheel_streak = (self.wheel_streak + 0.24).min(1.92);
        } else if dt_ms > 220.0 {
            self.wheel_streak = 1.0;
        } else {
            self.wheel_streak = (self.wheel_streak * 0.85).max(1.0);
        }
        let px = delta_notches * 84.0 * self.wheel_streak;

        if self.show_lyrics_overlay {
            if self
                .lyrics_view
                .lyrics_column_rect
                .contains(self.mouse_pos.0, self.mouse_pos.1)
            {
                self.lyrics_view.animator.on_user_wheel(px);
            }
            return;
        }
        if self.sidebar_nav_rect.w > 0.0
            && self
                .sidebar_nav_rect
                .contains(self.mouse_pos.0, self.mouse_pos.1)
        {
            self.sidebar_scroll_target_y =
                (self.sidebar_scroll_target_y - px).clamp(0.0, self.sidebar_max_scroll_y);
            if self.settings.reduce_motion {
                self.sidebar_scroll_y = self.sidebar_scroll_target_y;
            }
            return;
        }
        if self.route != PageRoute::Visualizer {
            self.scroll_target_y = (self.scroll_target_y - px).clamp(0.0, self.max_scroll_y);
            if self.settings.reduce_motion {
                self.scroll_y = self.scroll_target_y;
            }
        }
    }

    pub fn on_char_input(&mut self, ch: char) -> bool {
        if !self.search_focused {
            // Global shortcuts when not typing in search box
            match ch {
                ' ' => {
                    self.execute_action(UiAction::TogglePlayPause);
                    return true;
                }
                'l' | 'L' => {
                    self.execute_action(UiAction::ToggleLyricsOverlay);
                    return true;
                }
                _ => return false,
            }
        }
        if ch == '\u{08}' {
            // Backspace
            self.search_query.pop();
            self.last_search_typed = Some(Instant::now());
            return true;
        }
        if ch == '\r' || ch == '\n' {
            self.last_search_typed = None;
            let q = self.search_query.trim().to_string();
            if !q.is_empty() {
                self.dispatch_search(q);
            }
            return true;
        }
        if ch == '\u{1b}' {
            self.search_focused = false;
            return true;
        }
        if !ch.is_control() {
            self.search_query.push(ch);
            self.last_search_typed = Some(Instant::now());
            return true;
        }
        false
    }

    pub fn apply_pasted_user_token(&mut self, raw: &str) -> bool {
        let norm = am_api::normalize_user_token(raw);
        if norm.is_empty() {
            return false;
        }
        self.settings.user_token = norm.clone();
        self.settings.token_cleared_by_user = false;
        self.settings.save();
        self.token_box_focused = false;

        let cur_route = self.route.clone();
        self.dispatch_background_playlist_sync();
        if matches!(
            cur_route,
            PageRoute::LibRecent
                | PageRoute::LibArtists
                | PageRoute::LibAlbums
                | PageRoute::LibSongs
                | PageRoute::LibForYou
                | PageRoute::LibPlaylists
        ) {
            self.fetch_library_route(&cur_route);
        }

        // Automatically verify & detect account storefront on paste
        let sf = Some(self.settings.storefront.clone());
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        let utok_verify = Some(norm.clone());
        self.rt.spawn(async move {
            if let Ok(res) = am_api::verify_am_auth(utok_verify, sf).await {
                let _ = tx.send(AsyncMsg::AuthVerified(res));
                Self::post_ui_wake(hwnd);
            }
        });

        self.set_banner(
            format!(
                "已保存 Media-User-Token（{} 字符，重启后自动保留）",
                norm.len()
            ),
            false,
        );
        true
    }

    pub fn on_paste_text(&mut self, pasted: &str) -> bool {
        if self.show_lyrics_overlay {
            let trimmed = pasted.trim().trim_matches('"');
            let p = std::path::Path::new(trimmed);
            if !trimmed.is_empty() && p.is_file() {
                return self.import_lyrics_from_path(p);
            }
            if let Some(parsed) = lyrics::parse_any_lyrics_file(pasted) {
                let key = if self.current_lyrics_key.is_empty() {
                    "manual:imported".to_string()
                } else {
                    self.current_lyrics_key.clone()
                };
                self.current_lyrics_key = key.clone();
                self.lyrics_req_seq = self.lyrics_req_seq.wrapping_add(1);
                lyrics::put_custom_lyrics_in_cache(&key, parsed.clone());
                self.current_lyrics = Some(parsed);
                self.lyrics_view.invalidate_layout_cache();
                self.lyrics_loading = false;
                self.set_banner("已从剪贴板载入同步歌词".to_string(), false);
                return true;
            }
        }
        let clean: String = pasted.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return false;
        }
        // Smart token paste: if token box is focused OR clipboard clearly contains a Media-User-Token / Cookie,
        // save it immediately as user_token regardless of current page or search focus!
        if self.token_box_focused || am_api::looks_like_media_user_token(pasted) {
            self.search_focused = false;
            return self.apply_pasted_user_token(pasted);
        }
        if self.search_focused {
            self.search_query.push_str(&clean);
            self.last_search_typed = Some(Instant::now());
            return true;
        }
        if self.route == PageRoute::Settings
            || matches!(
                self.route,
                PageRoute::LibRecent
                    | PageRoute::LibArtists
                    | PageRoute::LibAlbums
                    | PageRoute::LibSongs
                    | PageRoute::LibForYou
                    | PageRoute::LibPlaylists
            )
        {
            return self.apply_pasted_user_token(pasted);
        }
        false
    }

    pub fn on_mouse_move(&mut self, x: f32, y: f32) -> bool {
        let prev = self.mouse_pos;
        self.mouse_pos = (x, y);
        if self.mouse_down && self.active_drag.is_some() {
            self.apply_drag_at(x, false);
            return true;
        }
        let was_in_tb = prev.0 >= 0.0 && prev.1 >= 0.0 && prev.1 < TOP_BAR_GUARD_H;
        let now_in_tb = x >= 0.0 && y >= 0.0 && y < TOP_BAR_GUARD_H;
        let tb_changed = was_in_tb != now_in_tb;
        let over_now = x >= 0.0 && y >= 0.0 && self.is_interactive_at(x, y);
        let changed_over = over_now != self.last_hovered_interactive;
        self.last_hovered_interactive = over_now;
        !self.glass_off()
            || over_now
            || changed_over
            || tb_changed
            || !self.top_bar_guard_spring.is_settled_norm()
            || !self.hover_alphas.is_empty()
    }

    pub fn on_mouse_down(&mut self, x: f32, y: f32) -> bool {
        self.mouse_pos = (x, y);
        self.mouse_down = true;
        self.active_drag = None;

        if self.show_lyrics_overlay {
            if let Some(hz) = self
                .lyrics_view
                .hit_zones
                .iter()
                .rev()
                .find(|hz| hz.rect.contains(x, y))
                .cloned()
            {
                match hz.action {
                    LyricsHitAction::ScrubProgressBar(bar_r) => {
                        self.active_drag = Some(ActiveDrag::LyricsSeek(bar_r));
                        self.last_drag_ipc = Instant::now();
                        self.apply_drag_at(x, false);
                        return true;
                    }
                    LyricsHitAction::ScrubVolumeBar(bar_r) => {
                        self.active_drag = Some(ActiveDrag::LyricsVolume(bar_r));
                        self.apply_drag_at(x, false);
                        return true;
                    }
                    _ => {
                        self.lyrics_view.animator.set_pressed_line(hz.line_idx);
                        return true;
                    }
                }
            }
            self.lyrics_view.animator.set_pressed_line(None);
            return true;
        }

        // Check interactive sliders for immediate mouse-down + drag scrubbing
        if let Some(act) = self
            .hit_regions
            .iter()
            .rev()
            .find(|hr| hr.rect.contains(x, y))
            .map(|hr| hr.action.clone())
        {
            match act {
                UiAction::ScrubMiniPlayerSeek(bar_r) => {
                    self.active_drag = Some(ActiveDrag::MiniPlayerSeek(bar_r));
                    self.last_drag_ipc = Instant::now();
                    self.apply_drag_at(x, false);
                    return true;
                }
                UiAction::ScrubPopoverVolume(bar_r) => {
                    self.active_drag = Some(ActiveDrag::PopoverVolume(bar_r));
                    self.apply_drag_at(x, false);
                    return true;
                }
                UiAction::ScrubBinauralWidth(bar_r) => {
                    self.active_drag = Some(ActiveDrag::BinauralWidth(bar_r));
                    self.apply_drag_at(x, false);
                    return true;
                }
                UiAction::ScrubBinauralRoom(bar_r) => {
                    self.active_drag = Some(ActiveDrag::BinauralRoom(bar_r));
                    self.apply_drag_at(x, false);
                    return true;
                }
                UiAction::ScrubWindowCornerRadius(bar_r) => {
                    self.active_drag = Some(ActiveDrag::WindowCornerRadius(bar_r));
                    self.apply_drag_at(x, false);
                    return true;
                }
                UiAction::ScrubVizCustomFps(bar_r) => {
                    self.active_drag = Some(ActiveDrag::VizCustomFps(bar_r));
                    self.apply_drag_at(x, false);
                    return true;
                }
                UiAction::ScrubLyricsCustomFps(bar_r) => {
                    self.active_drag = Some(ActiveDrag::LyricsCustomFps(bar_r));
                    self.apply_drag_at(x, false);
                    return true;
                }
                _ => {}
            }
        }

        let was_search = self.search_focused;
        let was_tok = self.token_box_focused;
        self.search_focused = self.search_box_rect.contains(x, y);
        self.token_box_focused =
            self.route == PageRoute::Settings && self.token_box_rect.contains(x, y);
        was_search != self.search_focused || was_tok != self.token_box_focused
    }

    pub fn on_mouse_up(&mut self, x: f32, y: f32) -> bool {
        self.mouse_pos = (x, y);
        self.mouse_down = false;

        if self.active_drag.is_some() {
            self.apply_drag_at(x, true);
            self.active_drag = None;
            return true;
        }

        if self.show_lyrics_overlay {
            self.lyrics_view.animator.set_pressed_line(None);
            let hit = self
                .lyrics_view
                .hit_zones
                .iter()
                .rev()
                .find(|hz| hz.rect.contains(x, y))
                .map(|hz| hz.action.clone());
            if let Some(act) = hit {
                self.handle_lyrics_action(act);
                return true;
            }
            return false;
        }

        // Check Visualizer-specific hit zones first when on Visualizer page
        if self.route == PageRoute::Visualizer && !self.show_queue_popover && !self.show_more_popover
        {
            let vhit = self
                .viz_state
                .hit_zones
                .iter()
                .rev()
                .find(|z| z.rect.contains(x, y))
                .map(|z| z.action.clone());
            if let Some(vact) = vhit {
                self.handle_viz_action(vact);
                return true;
            }
        }

        // Check global & page hit regions (reverse order so floating popovers/MiniPlayer win over underlying rows!)
        let hit = self
            .hit_regions
            .iter()
            .rev()
            .find(|hr| hr.rect.contains(x, y))
            .map(|hr| hr.action.clone());

        if let Some(act) = hit {
            self.execute_action(act);
            return true;
        }

        if self.show_queue_popover || self.show_more_popover {
            self.show_queue_popover = false;
            self.show_more_popover = false;
            return true;
        }

        false
    }

    fn apply_drag_at(&mut self, x: f32, commit: bool) {
        let Some(drag) = self.active_drag else {
            return;
        };
        let send_now = commit || self.last_drag_ipc.elapsed() >= std::time::Duration::from_millis(85);
        match drag {
            ActiveDrag::LyricsSeek(bar) | ActiveDrag::MiniPlayerSeek(bar) => {
                let ratio = ((x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0) as f64;
                if self.smtc.duration_sec > 0.5 {
                    let sec = ratio * self.smtc.duration_sec;
                    self.smtc.position_sec = sec;
                    self.playhead_anchor_sec = sec;
                    self.playhead_anchor_inst = Instant::now();
                    self.lyrics_view.animator.reset_word_clamps();
                    if commit || self.last_drag_ipc.elapsed() >= std::time::Duration::from_millis(240) {
                        self.last_drag_ipc = Instant::now();
                        self.control_player("seek", Some(sec));
                    }
                }
            }
            ActiveDrag::LyricsVolume(bar) | ActiveDrag::PopoverVolume(bar) => {
                let ratio = ((x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0) as f64;
                self.smtc.volume = Some(ratio);
                if send_now {
                    self.last_drag_ipc = Instant::now();
                    self.execute_action(UiAction::SetVolumeRatio(ratio));
                }
            }
            ActiveDrag::BinauralWidth(bar) => {
                let t = ((x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                let width = 0.60 + t * 1.00; // 0.60 .. 1.60
                self.binaural.config.spatial_width = width;
                if send_now {
                    self.last_drag_ipc = Instant::now();
                    self.execute_action(UiAction::AdjustBinauralWidth(width));
                }
            }
            ActiveDrag::BinauralRoom(bar) => {
                let t = ((x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                let room = t * 0.80; // 0.00 .. 0.80
                self.binaural.config.room_ambience = room;
                if send_now {
                    self.last_drag_ipc = Instant::now();
                    self.execute_action(UiAction::AdjustBinauralRoom(room));
                }
            }
            ActiveDrag::WindowCornerRadius(bar) => {
                let t = ((x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                let rad = (t * 28.0).round();
                self.settings.window_corner_radius = rad;
                if commit && self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            ActiveDrag::VizCustomFps(bar) => {
                let t = ((x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                let fps = (30.0 + t * 330.0).round() as u32;
                self.settings.viz_custom_fps = fps.clamp(30, 360);
                self.settings.viz_fps_cap = FpsCap::Custom;
                if commit && self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            ActiveDrag::LyricsCustomFps(bar) => {
                let t = ((x - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                let fps = (30.0 + t * 330.0).round() as u32;
                self.settings.lyrics_custom_fps = fps.clamp(30, 360);
                self.settings.lyrics_fps_cap = FpsCap::Custom;
                if commit && self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
        }
    }

    pub fn handle_lyrics_action(&mut self, act: LyricsHitAction) {
        match act {
            LyricsHitAction::CloseLyrics => {
                self.show_lyrics_overlay = false;
                self.update_viz_subscription();
            }
            LyricsHitAction::WinMinimize => {
                self.execute_action(UiAction::WindowMinimize);
            }
            LyricsHitAction::WinToggleMaximize => {
                self.execute_action(UiAction::WindowToggleMaximize);
            }
            LyricsHitAction::WinClose => {
                self.execute_action(UiAction::WindowClose);
            }
            LyricsHitAction::SeekToSec(sec) => {
                let s = sec.max(0.0);
                self.smtc.position_sec = s;
                self.playhead_anchor_sec = s;
                self.playhead_anchor_inst = Instant::now();
                self.control_player("seek", Some(s));
            }
            LyricsHitAction::ScrubProgressBar(bar) => {
                let ratio = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0) as f64;
                self.execute_action(UiAction::SeekRatio(ratio));
            }
            LyricsHitAction::ScrubVolumeBar(bar) => {
                let ratio = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0) as f64;
                self.execute_action(UiAction::SetVolumeRatio(ratio));
            }
            LyricsHitAction::TogglePlayPause => {
                self.execute_action(UiAction::TogglePlayPause);
            }
            LyricsHitAction::PrevTrack => {
                self.execute_action(UiAction::PrevTrack);
            }
            LyricsHitAction::NextTrack => {
                self.execute_action(UiAction::NextTrack);
            }
            LyricsHitAction::ToggleShuffle => {
                self.execute_action(UiAction::ToggleShuffleMode);
            }
            LyricsHitAction::ToggleRepeat => {
                self.execute_action(UiAction::ToggleRepeatMode);
            }
            LyricsHitAction::CycleLyricsSource => {
                self.lyrics_view.source_pref = self.lyrics_view.source_pref.next();
                self.lyrics_view.sync_offset_sec = 0.0;
                self.current_lyrics = None;
                self.lyrics_view.invalidate_layout_cache();
                let lbl = self.lyrics_view.source_pref.label();
                if !self.smtc.title.is_empty() {
                    let key = lyrics::make_lyrics_cache_key(
                        self.smtc.adam_id.as_deref(),
                        &self.smtc.title,
                        &self.smtc.artist,
                    );
                    self.current_lyrics_key = key.clone();
                    self.dispatch_lyrics_fetch(
                        key,
                        self.smtc.adam_id.clone(),
                        self.smtc.title.clone(),
                        self.smtc.artist.clone(),
                        self.smtc.album.clone(),
                        self.smtc.duration_sec,
                        true,
                    );
                }
                self.set_banner(format!("歌词来源已切换为：{lbl}"), false);
            }
            LyricsHitAction::NudgeLyricsOffset(delta) => {
                let next = ((self.lyrics_view.sync_offset_sec + delta) * 10.0)
                    .round()
                    .clamp(-50.0, 50.0)
                    / 10.0;
                self.lyrics_view.sync_offset_sec = next;
            }
            LyricsHitAction::ResetLyricsOffset => {
                self.lyrics_view.sync_offset_sec = 0.0;
            }
            LyricsHitAction::ToggleTranslation => {
                self.lyrics_view.show_translation = !self.lyrics_view.show_translation;
            }
            LyricsHitAction::ToggleFreeBlur => {
                self.lyrics_view.music_free_blur = !self.lyrics_view.music_free_blur;
                self.settings.lyrics_free_blur = self.lyrics_view.music_free_blur;
                self.settings.save();
            }
            LyricsHitAction::ImportLocalLyrics => {
                if let Some(path) = pick_local_lyrics_file_dialog(self.wake_hwnd) {
                    let _ = self.import_lyrics_from_path(&path);
                }
            }
        }
    }

    pub fn import_lyrics_from_path(&mut self, path: &std::path::Path) -> bool {
        let Ok(txt) = fs::read_to_string(path) else {
            self.set_banner("读取本地歌词文件失败".to_string(), true);
            return false;
        };
        if let Some(parsed) = lyrics::parse_any_lyrics_file(&txt) {
            let key = if self.current_lyrics_key.is_empty() {
                "manual:imported".to_string()
            } else {
                self.current_lyrics_key.clone()
            };
            self.current_lyrics_key = key.clone();
            self.lyrics_req_seq = self.lyrics_req_seq.wrapping_add(1);
            lyrics::put_custom_lyrics_in_cache(&key, parsed.clone());
            self.current_lyrics = Some(parsed);
            self.lyrics_view.invalidate_layout_cache();
            self.lyrics_loading = false;
            self.show_lyrics_overlay = true;
            self.update_viz_subscription();
            self.set_banner("已载入本地同步歌词文件".to_string(), false);
            true
        } else {
            self.set_banner("无法解析该歌词文件格式".to_string(), true);
            false
        }
    }

    fn handle_viz_action(&mut self, act: VizHitAction) {
        match act {
            VizHitAction::SelectStyle(st) => {
                self.viz_state.style = st;
                self.settings.viz_default_style = st;
                self.settings.save();
            }
            VizHitAction::InspectorReset => {
                audio_viz::trigger_inspector_reset();
            }
            VizHitAction::InspectorTogglePause => {
                let next = !audio_viz::is_inspector_paused();
                audio_viz::set_inspector_paused(next);
            }
            VizHitAction::InspectorToggleAutoReset => {
                let next = !audio_viz::is_auto_reset_silence();
                audio_viz::set_auto_reset_silence(next);
            }
            VizHitAction::SwitchQualityTier(q, lossless, atmos) => {
                let mut next = self.audio_prefs.clone();
                next.streaming_audio_quality = q;
                next.lossless_enabled = lossless;
                next.dolby_atmos_setting = atmos;
                self.save_audio_prefs_async(next);
                audio_viz::trigger_inspector_reset();
            }
            VizHitAction::CycleOutputDevice => {
                if self.devices.len() > 1 {
                    let cur_idx = self
                        .devices
                        .iter()
                        .position(|d| d.is_default)
                        .unwrap_or(0);
                    let next_id = self.devices[(cur_idx + 1) % self.devices.len()].id.clone();
                    self.execute_action(UiAction::SelectOutputDevice(next_id));
                }
            }
            VizHitAction::CycleXpSubPreset => {
                self.viz_state.xp_sub_preset = (self.viz_state.xp_sub_preset + 1) % 3;
            }
            VizHitAction::ToggleReactorSubStyle => {
                self.viz_state.reactor_sub_style = (self.viz_state.reactor_sub_style + 1) % 2;
            }
        }
    }

    pub fn execute_action(&mut self, act: UiAction) {
        match act {
            UiAction::SwallowClick => {}
            UiAction::WindowMinimize => {
                if self.wake_hwnd != 0 {
                    unsafe {
                        let hw = windows::Win32::Foundation::HWND(
                            self.wake_hwnd as *mut std::ffi::c_void,
                        );
                        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            Some(hw),
                            windows::Win32::UI::WindowsAndMessaging::WM_SYSCOMMAND,
                            windows::Win32::Foundation::WPARAM(
                                windows::Win32::UI::WindowsAndMessaging::SC_MINIMIZE as usize,
                            ),
                            windows::Win32::Foundation::LPARAM(0),
                        );
                    }
                }
            }
            UiAction::WindowToggleMaximize => {
                if self.wake_hwnd != 0 {
                    unsafe {
                        let hw = windows::Win32::Foundation::HWND(
                            self.wake_hwnd as *mut std::ffi::c_void,
                        );
                        let zoomed =
                            windows::Win32::UI::WindowsAndMessaging::IsZoomed(hw).as_bool();
                        let sc = if zoomed {
                            windows::Win32::UI::WindowsAndMessaging::SC_RESTORE
                        } else {
                            windows::Win32::UI::WindowsAndMessaging::SC_MAXIMIZE
                        };
                        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            Some(hw),
                            windows::Win32::UI::WindowsAndMessaging::WM_SYSCOMMAND,
                            windows::Win32::Foundation::WPARAM(sc as usize),
                            windows::Win32::Foundation::LPARAM(0),
                        );
                    }
                }
            }
            UiAction::WindowClose => {
                if self.wake_hwnd != 0 {
                    unsafe {
                        let hw = windows::Win32::Foundation::HWND(
                            self.wake_hwnd as *mut std::ffi::c_void,
                        );
                        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            Some(hw),
                            windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                            windows::Win32::Foundation::WPARAM(0),
                            windows::Win32::Foundation::LPARAM(0),
                        );
                    }
                }
            }
            UiAction::Navigate(r) => self.navigate_to(r),
            UiAction::NavigateBack => {
                if let Some(prev) = self.nav_history.pop() {
                    self.route = prev;
                    self.route_trans = if self.settings.reduce_motion { 1.0 } else { 0.0 };
                    self.route_trans_dir = -1.0;
                    self.scroll_y = 0.0;
                    self.scroll_target_y = 0.0;
                    self.scroll_vel_y = 0.0;
                    self.update_viz_subscription();
                }
            }
            UiAction::ToggleSidebar => {
                self.settings.sidebar_collapsed = !self.settings.sidebar_collapsed;
                self.sidebar_open_spring.target = if self.settings.sidebar_collapsed {
                    0.0
                } else {
                    1.0
                };
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ToggleSidebarEdit => {
                self.sidebar_edit_mode = !self.sidebar_edit_mode;
                self.sidebar_edit_spring.target = if self.sidebar_edit_mode { 1.0 } else { 0.0 };
            }
            UiAction::ToggleSidebarItemHidden(k) => {
                if let Some(pos) = self.settings.hidden_sidebar_keys.iter().position(|x| x == &k) {
                    self.settings.hidden_sidebar_keys.remove(pos);
                } else {
                    self.settings.hidden_sidebar_keys.push(k);
                }
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ToggleSectionLibrary => {
                self.sec_library_open = !self.sec_library_open;
                self.sec_library_spring.target = if self.sec_library_open { 1.0 } else { 0.0 };
            }
            UiAction::ToggleSectionPlaylists => {
                self.sec_playlists_open = !self.sec_playlists_open;
                self.sec_playlists_spring.target = if self.sec_playlists_open { 1.0 } else { 0.0 };
            }
            UiAction::FocusSearch => {
                self.search_focused = true;
                if self.route != PageRoute::Search && !self.search_query.is_empty() {
                    self.navigate_to(PageRoute::Search);
                }
            }
            UiAction::ClearSearch => {
                self.search_query.clear();
                self.last_dispatched_query.clear();
                self.search_focused = true;
            }
            UiAction::SetSearchFilter(f) => {
                self.search_filter = f;
            }
            UiAction::PlayContainer { kind, id, shuffle } => {
                if kind == "artists" || kind == "library-artists" {
                    self.navigate_to(PageRoute::ContainerDetail {
                        kind,
                        id,
                        title_hint: String::new(),
                    });
                    return;
                }
                let sf = self.settings.storefront.clone();
                if !self.settings.use_builtin_service {
                    let uri_kind = if kind.contains("playlist") {
                        "playlist"
                    } else {
                        "album"
                    };
                    let uri = format!("https://music.apple.com/{sf}/{uri_kind}/{id}");
                    let _ = am_api::launch_official_apple_music_uri(&uri);
                    return;
                }
                let utok = non_empty_opt(&self.settings.user_token);
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.shuffle_on = shuffle;
                self.begin_optimistic_playback_loading(None, Some(id.clone()));
                self.rt.spawn(async move {
                    match am_agent::play_container_by_api(kind, id, utok, Some(sf)).await {
                        Ok(st) => {
                            let _ = tx.send(AsyncMsg::PlaybackUpdated(st));
                        }
                        Err(e) => {
                            let _ = tx.send(AsyncMsg::BannerError(e));
                        }
                    }
                    Self::post_ui_wake(hwnd);
                });
            }
            UiAction::PlayDetailTracks {
                start_index,
                shuffle,
            } => {
                let Some(ref detail) = self.detail_data else {
                    return;
                };
                if detail.tracks.is_empty() {
                    return;
                }
                let det_id = detail.id.clone();
                let mut list = detail.tracks.clone();
                let mut idx = start_index.min(list.len() - 1);
                if (shuffle || self.shuffle_on) && list.len() > 1 {
                    if shuffle {
                        self.shuffle_on = true;
                    }
                    list.swap(0, idx);
                    shuffle_slice_from(&mut list, 1);
                    idx = 0;
                }
                self.play_tracks_async_with_container(list, idx, Some(det_id));
            }
            UiAction::PlayTrackList { source, index } => {
                let mut list = match source {
                    TrackListSource::HomeSongs => self.home_songs.clone(),
                    TrackListSource::SearchSongs => self.search_songs.clone(),
                    TrackListSource::LibrarySongs => self.lib_songs.clone(),
                    TrackListSource::DetailTracks => self
                        .detail_data
                        .as_ref()
                        .map(|d| d.tracks.clone())
                        .unwrap_or_default(),
                };
                if !list.is_empty() {
                    let mut idx = index.min(list.len() - 1);
                    if self.shuffle_on && list.len() > 2 {
                        list.swap(0, idx);
                        shuffle_slice_from(&mut list, 1);
                        idx = 0;
                    }
                    self.play_tracks_async(list, idx);
                }
            }
            UiAction::PlaySingleTrack(tr) => {
                self.play_tracks_async(vec![tr], 0);
            }
            UiAction::AddContainerToLibrary { kind, id } => {
                self.add_to_library_async(kind, id);
            }
            UiAction::AddSongToLibrary(id) => {
                self.add_to_library_async("songs".to_string(), id);
            }
            UiAction::TogglePlayPause => {
                self.control_player("toggle", None);
            }
            UiAction::PrevTrack => {
                self.control_player("prev", None);
            }
            UiAction::NextTrack => {
                self.control_player("next", None);
            }
            UiAction::SeekRatio(ratio) => {
                if self.smtc.duration_sec > 0.5 {
                    let sec = ratio.clamp(0.0, 1.0) * self.smtc.duration_sec;
                    self.smtc.position_sec = sec;
                    self.playhead_anchor_sec = sec;
                    self.playhead_anchor_inst = Instant::now();
                    self.control_player("seek", Some(sec));
                }
            }
            UiAction::SetVolumeRatio(vol) => {
                let v = vol.clamp(0.0, 1.0);
                self.smtc.volume = Some(v);
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.rt.spawn(async move {
                    if let Ok(Ok(st)) =
                        tokio::task::spawn_blocking(move || am_agent::control("volume", Some(v)))
                            .await
                    {
                        let _ = tx.send(AsyncMsg::PlaybackUpdated(st));
                        Self::post_ui_wake(hwnd);
                    }
                });
            }
            UiAction::ScrubMiniPlayerSeek(bar) => {
                let ratio = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0) as f64;
                self.execute_action(UiAction::SeekRatio(ratio));
            }
            UiAction::ScrubPopoverVolume(bar) => {
                let ratio = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0) as f64;
                self.execute_action(UiAction::SetVolumeRatio(ratio));
            }
            UiAction::ScrubBinauralWidth(bar) => {
                let t = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                self.execute_action(UiAction::AdjustBinauralWidth(0.60 + t * 1.00));
            }
            UiAction::ScrubBinauralRoom(bar) => {
                let t = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                self.execute_action(UiAction::AdjustBinauralRoom(t * 0.80));
            }
            UiAction::ToggleShuffleMode => {
                self.shuffle_on = !self.shuffle_on;
            }
            UiAction::ToggleRepeatMode => {
                self.repeat_on = !self.repeat_on;
            }
            UiAction::ToggleLyricsOverlay => {
                self.show_lyrics_overlay = !self.show_lyrics_overlay;
                self.show_queue_popover = false;
                self.show_more_popover = false;
                self.update_viz_subscription();
            }
            UiAction::ToggleQueuePopover => {
                self.show_queue_popover = !self.show_queue_popover;
                self.show_more_popover = false;
            }
            UiAction::ToggleMorePopover => {
                self.show_more_popover = !self.show_more_popover;
                self.show_queue_popover = false;
            }
            UiAction::JumpQueueIndex(idx) => {
                let q = am_agent::get_queue_tracks();
                if idx < q.len() {
                    self.show_queue_popover = false;
                    self.play_tracks_async(q, idx);
                }
            }
            UiAction::RefreshCurrentView => {
                let cur = self.route.clone();
                match cur {
                    PageRoute::Home | PageRoute::Browse => self.fetch_home_charts(),
                    PageRoute::Radio => self.fetch_radio_mixes(),
                    PageRoute::Search => {
                        let q = self.search_query.trim().to_string();
                        if !q.is_empty() {
                            self.dispatch_search(q);
                        }
                    }
                    PageRoute::ContainerDetail {
                        kind,
                        id,
                        title_hint,
                    } => self.fetch_container_detail(kind, id, title_hint),
                    other => self.fetch_library_route(&other),
                }
            }
            UiAction::OpenDownloadsFolder => {
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let res = tokio::task::spawn_blocking(|| {
                        let hint = am_agent::get_cached_library_url();
                        downloads::open_media_folder(hint.as_deref())
                    })
                    .await;
                    if let Ok(Err(e)) = res {
                        let _ = tx.send(AsyncMsg::BannerError(e));
                    }
                });
            }
            UiAction::TogglePauseDownloads(pause) => {
                self.downloads_paused = pause;
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    if let Ok(Err(e)) =
                        tokio::task::spawn_blocking(move || am_agent::control_downloads_queue(pause))
                            .await
                    {
                        let _ = tx.send(AsyncMsg::BannerError(e));
                    }
                });
            }
            UiAction::RevealDownload(p) => {
                let _ = downloads::reveal_in_explorer(&p);
            }
            UiAction::DeleteDownload(p) => {
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.rt.spawn(async move {
                    let res = tokio::task::spawn_blocking(move || {
                        let hint = am_agent::get_cached_library_url();
                        downloads::delete_download_item(&p, hint.as_deref())?;
                        downloads::list_downloads(hint.as_deref())
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                    let _ = tx.send(AsyncMsg::DownloadsLoaded(res));
                    Self::post_ui_wake(hwnd);
                });
            }
            UiAction::SetStorefront(sf) => {
                self.settings.storefront = sf;
                self.settings.storefront_customized = true;
                self.settings.save();
                self.fetch_home_charts();
            }
            UiAction::SyncOfficialClientAccount => {
                self.official_account = am_api::detect_official_am_account();
                let mut synced_parts = Vec::new();
                if let Some(ref off_sf) = self.official_account.storefront {
                    self.settings.storefront = off_sf.clone();
                    self.settings.storefront_customized = false;
                    synced_parts.push(format!("地区 {}", off_sf.to_uppercase()));
                }
                if let Some(ref name) = self.official_account.display_name {
                    synced_parts.push(format!("账号 {name}"));
                }
                if self.settings.user_token.is_empty() {
                    let legacy = am_api::import_legacy_amlight_credentials();
                    if let Some(tok) = legacy.user_token {
                        self.settings.user_token = tok;
                        self.settings.token_cleared_by_user = false;
                        synced_parts.push("已恢复资料库授权 Token".to_string());
                    }
                }
                self.settings.save();
                self.fetch_home_charts();
                if !self.settings.user_token.is_empty() {
                    self.dispatch_background_playlist_sync();
                }
                if synced_parts.is_empty() && !self.official_account.signed_in {
                    self.set_banner("未检测到桌面版 Apple Music 登录记录".to_string(), true);
                } else {
                    let detail = if synced_parts.is_empty() {
                        "原装客户端播放服务就绪".to_string()
                    } else {
                        synced_parts.join(" · ")
                    };
                    self.set_banner(format!("已同步原装端信息：{detail}"), false);
                }
            }
            UiAction::FocusTokenBox => {
                self.token_box_focused = true;
                self.search_focused = false;
            }
            UiAction::PasteUserToken => {
                if let Some(clip) = read_clipboard_unicode_text() {
                    if !self.apply_pasted_user_token(&clip) {
                        self.set_banner("剪贴板中未识别到有效 Token 内容".to_string(), true);
                    }
                } else {
                    self.set_banner("剪贴板为空或无法读取".to_string(), true);
                }
            }
            UiAction::OpenWebAppleMusic => {
                let url = format!("https://music.apple.com/{}/browse", self.settings.storefront);
                open_external_browser(&url);
            }
            UiAction::VerifyUserToken => {
                let utok = non_empty_opt(&self.settings.user_token);
                let sf = Some(self.settings.storefront.clone());
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.rt.spawn(async move {
                    match am_api::verify_am_auth(utok, sf).await {
                        Ok(res) => {
                            let _ = tx.send(AsyncMsg::AuthVerified(res));
                        }
                        Err(e) => {
                            let _ = tx.send(AsyncMsg::BannerError(e));
                        }
                    }
                    Self::post_ui_wake(hwnd);
                });
            }
            UiAction::ClearUserToken => {
                self.settings.user_token.clear();
                self.settings.token_cleared_by_user = true;
                self.settings.save();
                self.sidebar_playlists.clear();
                self.playlist_cache = PlaylistDiskCache::default();
                if let Some(p) = PlaylistDiskCache::cache_path() {
                    let _ = fs::remove_file(p);
                }
                self.lib_cards.clear();
                self.lib_songs.clear();
                self.auth_result = None;
                self.set_banner("已清除 Media-User-Token".to_string(), false);
            }
            UiAction::SetUseBuiltinService(v) => {
                self.settings.use_builtin_service = v;
                self.settings.save();
            }
            UiAction::DisconnectBuiltinService => {
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.rt.spawn(async move {
                    let _ = tokio::task::spawn_blocking(am_agent::disconnect_agent).await;
                    let st = tokio::task::spawn_blocking(am_agent::get_agent_status)
                        .await
                        .unwrap_or_default();
                    let _ = tx.send(AsyncMsg::AgentStatusUpdated(st));
                    let _ = tx.send(AsyncMsg::BannerNotice("已停止并释放播放服务会话".to_string()));
                    Self::post_ui_wake(hwnd);
                });
            }
            UiAction::SetStreamingQuality { lossless, quality } => {
                let mut next = self.audio_prefs.clone();
                next.lossless_enabled = lossless;
                next.streaming_audio_quality = quality;
                next.download_audio_quality = quality;
                self.save_audio_prefs_async(next);
            }
            UiAction::SetAtmosMode(m) => {
                let mut next = self.audio_prefs.clone();
                next.dolby_atmos_setting = m;
                self.save_audio_prefs_async(next);
            }
            UiAction::ToggleSoundCheck => {
                let mut next = self.audio_prefs.clone();
                next.sound_check_enabled = !next.sound_check_enabled;
                self.save_audio_prefs_async(next);
            }
            UiAction::ToggleCrossfade => {
                let mut next = self.audio_prefs.clone();
                next.crossfade_songs_enabled = !next.crossfade_songs_enabled;
                self.save_audio_prefs_async(next);
            }
            UiAction::SelectOutputDevice(dev_id) => {
                let tx = self.tx.clone();
                let hwnd = self.wake_hwnd;
                self.rt.spawn(async move {
                    let res = tokio::task::spawn_blocking(move || {
                        let bst = binaural_dsp::get_binaural_status();
                        if bst.active && dev_id.trim() != bst.config.capture_bus_id {
                            let mut cfg = bst.config;
                            cfg.target_device_id = dev_id;
                            let _ = binaural_dsp::apply_binaural_config(cfg)?;
                            audio_viz::list_output_devices()
                        } else {
                            if bst.active {
                                binaural_dsp::shutdown_binaural_cleanly();
                            }
                            audio_viz::set_output_device(&dev_id)
                        }
                    })
                    .await;
                    match res {
                        Ok(Ok(devs)) => {
                            let _ = tx.send(AsyncMsg::DevicesLoaded(devs));
                        }
                        Ok(Err(e)) => {
                            let _ = tx.send(AsyncMsg::BannerError(e));
                        }
                        _ => {}
                    }
                    Self::post_ui_wake(hwnd);
                });
            }
            UiAction::SetVizFpsCap(cap) => {
                self.settings.viz_fps_cap = cap;
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::SetLyricsFpsCap(cap) => {
                self.settings.lyrics_fps_cap = cap;
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::SetVizCustomFps(fps) => {
                self.settings.viz_fps_cap = FpsCap::Custom;
                self.settings.viz_custom_fps = fps.clamp(30, 360);
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ScrubVizCustomFps(bar) => {
                let t = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                self.settings.viz_fps_cap = FpsCap::Custom;
                self.settings.viz_custom_fps = ((30.0 + t * 330.0).round() as u32).clamp(30, 360);
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::SetLyricsCustomFps(fps) => {
                self.settings.lyrics_fps_cap = FpsCap::Custom;
                self.settings.lyrics_custom_fps = fps.clamp(30, 360);
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ScrubLyricsCustomFps(bar) => {
                let t = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                self.settings.lyrics_fps_cap = FpsCap::Custom;
                self.settings.lyrics_custom_fps = ((30.0 + t * 330.0).round() as u32).clamp(30, 360);
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ToggleLyricsLimitUnfocusedFps => {
                self.settings.lyrics_limit_unfocused_fps = !self.settings.lyrics_limit_unfocused_fps;
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::SetWindowCornerRadius(rad) => {
                self.settings.window_corner_radius = rad.clamp(0.0, 28.0).round();
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ScrubWindowCornerRadius(bar) => {
                let t = ((self.mouse_pos.0 - bar.x) / bar.w.max(1.0)).clamp(0.0, 1.0);
                self.settings.window_corner_radius = (t * 28.0).round();
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ToggleLiquidGlass => {
                self.settings.liquid_glass_enabled = !self.settings.liquid_glass_enabled;
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ToggleLyricsLatinByWord => {
                self.settings.lyrics_latin_by_word = !self.settings.lyrics_latin_by_word;
                self.lyrics_view.animator.latin_by_word = self.settings.lyrics_latin_by_word;
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::SetLyricsNonEmphasisEffect(fx) => {
                self.settings.lyrics_non_emphasis_effect = fx;
                if self.wake_hwnd != 0 {
                    self.settings.save();
                }
            }
            UiAction::ToggleLyricsBlurMode => {
                self.settings.lyrics_free_blur = !self.settings.lyrics_free_blur;
                self.lyrics_view.music_free_blur = self.settings.lyrics_free_blur;
                self.settings.save();
            }
            UiAction::ToggleReduceMotion => {
                self.settings.reduce_motion = !self.settings.reduce_motion;
                self.settings.save();
            }
            UiAction::ToggleReduceTransparency => {
                self.settings.reduce_transparency = !self.settings.reduce_transparency;
                self.settings.save();
            }
            UiAction::ToggleAdvancedSpatialDisclosure => {
                self.show_advanced_spatial = !self.show_advanced_spatial;
            }
            UiAction::ToggleBinauralEnabled => {
                let target_on = !self.binaural.active;
                let def_id = self
                    .devices
                    .iter()
                    .find(|d| d.is_default)
                    .map(|d| d.id.clone())
                    .unwrap_or_default();
                let mut cfg = if self.binaural.config.preset.is_empty() {
                    BinauralConfig::from_preset("apple_spatial", target_on, def_id, String::new())
                } else {
                    self.binaural.config.clone()
                };
                cfg.enabled = target_on;
                if cfg.target_device_id.is_empty() {
                    cfg.target_device_id = self
                        .devices
                        .iter()
                        .find(|d| d.is_default)
                        .map(|d| d.id.clone())
                        .unwrap_or_default();
                }
                self.apply_binaural_async(cfg);
            }
            UiAction::ToggleBinauralBypass => {
                let mut cfg = self.binaural.config.clone();
                cfg.bypass = !cfg.bypass;
                self.apply_binaural_async(cfg);
            }
            UiAction::SetBinauralPreset(preset) => {
                let next = BinauralConfig::from_preset(
                    &preset,
                    self.binaural.active,
                    self.binaural.config.target_device_id.clone(),
                    self.binaural.config.capture_bus_id.clone(),
                );
                self.apply_binaural_async(next);
            }
            UiAction::AdjustBinauralWidth(w) => {
                let mut cfg = self.binaural.config.clone();
                cfg.spatial_width = w.clamp(0.6, 1.6);
                self.apply_binaural_async(cfg);
            }
            UiAction::AdjustBinauralRoom(rm) => {
                let mut cfg = self.binaural.config.clone();
                cfg.room_ambience = rm.clamp(0.0, 0.8);
                self.apply_binaural_async(cfg);
            }
            UiAction::DismissBanner => {
                self.banner_text = None;
            }
        }
    }

    fn play_tracks_async(&mut self, tracks: Vec<AgentTrackInput>, start_index: usize) {
        self.play_tracks_async_with_container(tracks, start_index, None);
    }

    fn play_tracks_async_with_container(
        &mut self,
        tracks: Vec<AgentTrackInput>,
        start_index: usize,
        container_id: Option<String>,
    ) {
        if tracks.is_empty() {
            return;
        }
        let clamped_idx = start_index.min(tracks.len() - 1);
        if !self.settings.use_builtin_service {
            let aid = tracks[clamped_idx].effective_adam_id();
            let uri = format!(
                "https://music.apple.com/{}/song/{aid}",
                self.settings.storefront
            );
            let _ = am_api::launch_official_apple_music_uri(&uri);
            return;
        }
        self.begin_optimistic_playback_loading(tracks.get(clamped_idx), container_id);
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            match tokio::task::spawn_blocking(move || am_agent::play_tracks(tracks, start_index))
                .await
            {
                Ok(Ok(st)) => {
                    let _ = tx.send(AsyncMsg::PlaybackUpdated(st));
                }
                Ok(Err(e)) => {
                    let _ = tx.send(AsyncMsg::BannerError(e));
                }
                Err(e) => {
                    let _ = tx.send(AsyncMsg::BannerError(e.to_string()));
                }
            }
            Self::post_ui_wake(hwnd);
        });
    }

    fn control_player(&mut self, action: &str, seek_sec: Option<f64>) {
        if action.eq_ignore_ascii_case("seek") {
            if let Some(sec) = seek_sec {
                let clamped = sec.max(0.0);
                am_agent::notify_optimistic_seek(clamped);
                self.smtc_optimistic_seek = Some((clamped, Instant::now()));
                self.smtc_seek_pre_univ = self.last_smtc_timeline_univ;
                self.playhead_anchor_sec = clamped;
                self.playhead_anchor_inst = Instant::now();
                self.lyrics_view.animator.reset_word_clamps();
            }
        } else if action.eq_ignore_ascii_case("next") || action.eq_ignore_ascii_case("prev") {
            self.smtc_optimistic_seek = None;
        }
        if self.wake_hwnd == 0 && !am_agent::is_agent_active() {
            return;
        }
        let act = action.to_string();
        let builtin = self.settings.use_builtin_service;
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                if am_agent::is_agent_active()
                    || am_agent::is_agent_connected()
                    || (builtin && !am_agent::get_queue_tracks().is_empty())
                {
                    am_agent::control(&act, seek_sec)
                } else {
                    smtc::control_playback(&act, seek_sec)?;
                    Ok(unified_playback_snapshot())
                }
            })
            .await;
            match res {
                Ok(Ok(st)) => {
                    let _ = tx.send(AsyncMsg::PlaybackUpdated(st));
                }
                Ok(Err(e)) => {
                    let _ = tx.send(AsyncMsg::BannerError(e));
                }
                _ => {}
            }
            Self::post_ui_wake(hwnd);
        });
    }

    fn save_audio_prefs_async(&mut self, prefs: AudioPrefs) {
        self.audio_prefs = prefs.clone();
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            match tokio::task::spawn_blocking(move || am_agent::set_audio_prefs(prefs)).await {
                Ok(Ok(saved)) => {
                    let _ = tx.send(AsyncMsg::AudioPrefsLoaded(saved));
                    let snap = tokio::task::spawn_blocking(unified_playback_snapshot)
                        .await
                        .unwrap_or_default();
                    let _ = tx.send(AsyncMsg::PlaybackUpdated(snap));
                }
                Ok(Err(e)) => {
                    let _ = tx.send(AsyncMsg::BannerError(e));
                }
                _ => {}
            }
            Self::post_ui_wake(hwnd);
        });
    }

    fn apply_binaural_async(&self, cfg: BinauralConfig) {
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            match tokio::task::spawn_blocking(move || binaural_dsp::apply_binaural_config(cfg))
                .await
            {
                Ok(Ok(st)) => {
                    let _ = tx.send(AsyncMsg::BinauralUpdated(st));
                    if let Ok(Ok(devs)) =
                        tokio::task::spawn_blocking(audio_viz::list_output_devices).await
                    {
                        let _ = tx.send(AsyncMsg::DevicesLoaded(devs));
                    }
                }
                Ok(Err(e)) => {
                    let _ = tx.send(AsyncMsg::BannerError(e));
                }
                _ => {}
            }
            Self::post_ui_wake(hwnd);
        });
    }

    fn add_to_library_async(&self, kind: String, id: String) {
        let Some(utok) = non_empty_opt(&self.settings.user_token) else {
            let _ = self
                .tx
                .send(AsyncMsg::BannerError("请先在「设置」中配置 Media-User-Token".to_string()));
            Self::post_ui_wake(self.wake_hwnd);
            return;
        };
        let sf = self.settings.storefront.clone();
        let tx = self.tx.clone();
        let hwnd = self.wake_hwnd;
        self.rt.spawn(async move {
            match am_api::add_to_library(&kind, &id, Some(utok), Some(sf)).await {
                Ok(()) => {
                    let _ = tx.send(AsyncMsg::BannerNotice("已添加到资料库".to_string()));
                }
                Err(e) => {
                    let _ = tx.send(AsyncMsg::BannerError(e));
                }
            }
            Self::post_ui_wake(hwnd);
        });
    }

    // ========================================================================
    // Master Frame Step & Direct2D Render Pass
    // ========================================================================

    pub fn step_and_render(&mut self, r: &mut D2DRenderer, dt_sec: f32) {
        self.step_and_render_ex(r, dt_sec, false);
    }

    pub fn step_and_render_ex(
        &mut self,
        r: &mut D2DRenderer,
        dt_sec: f32,
        live_resize_no_vsync: bool,
    ) {
        let dt = dt_sec.clamp(0.001, 0.08);
        self.frame_dt = dt;
        self.visited_hover_keys.clear();

        // Same-frame (<4ms) gapless & duration/onset sync whenever AMPLibraryAgent increments `state_seq()`!
        let cur_agent_seq = am_agent::state_seq();
        if cur_agent_seq != self.last_agent_seq {
            self.last_agent_seq = cur_agent_seq;
            let st = am_agent::get_playback_state();
            self.on_playback_state_changed(st);
        }

        let animating_now = self.needs_continuous_animation();
        if animating_now {
            let win_elapsed = self.fps_window_start.elapsed().as_secs_f32();
            if win_elapsed > 1.2 {
                self.fps_window_start = Instant::now();
                self.fps_frames = 0;
            } else {
                self.fps_frames += 1;
                if win_elapsed >= 0.25 {
                    self.measured_fps = ((self.fps_frames as f32) / win_elapsed).clamp(1.0, 500.0);
                    self.fps_frames = 0;
                    self.fps_window_start = Instant::now();
                } else if self.measured_fps <= 1.0 {
                    self.measured_fps = (1.0 / dt_sec.max(0.001)).clamp(30.0, 500.0);
                }
            }
        } else {
            self.fps_frames = 0;
        }

        // Advance loading spinner phase & safety-expire stuck buffering after 14s
        if self.playback_loading {
            self.spinner_phase = (self.spinner_phase + dt * 1.35) % 100.0;
            if self
                .playback_loading_started
                .is_some_and(|t| t.elapsed() >= std::time::Duration::from_secs(14))
            {
                self.playback_loading = false;
                self.playback_loading_started = None;
                self.playback_loading_track_id = None;
                self.playback_loading_container_id = None;
            }
        }

        let win_w = r.logical_width();
        let win_h = r.logical_height();
        let win_rect = RectF::new(0.0, 0.0, win_w, win_h);
        let want_sb = if !self.settings.sidebar_collapsed && win_w >= Theme::SIDEBAR_AUTO_COLLAPSE_WIDTH
        {
            1.0f32
        } else {
            0.0f32
        };
        let want_lib = if self.sec_library_open { 1.0f32 } else { 0.0f32 };
        let want_pls = if self.sec_playlists_open { 1.0f32 } else { 0.0f32 };
        let want_edit = if self.sidebar_edit_mode { 1.0f32 } else { 0.0f32 };
        let cur_sb_w = Theme::SIDEBAR_WIDTH * self.sidebar_open_spring.value.clamp(0.0, 1.0);
        let want_tb_guard = if !self.show_lyrics_overlay
            && self.route != PageRoute::Visualizer
            && self.mouse_pos.1 >= 0.0
            && self.mouse_pos.1 < TOP_BAR_GUARD_H
            && self.mouse_pos.0 >= cur_sb_w
        {
            1.0f32
        } else {
            0.0f32
        };

        // Advance nonlinear page transition, Lyrics expand/collapse spring & full Sidebar springs!
        if self.settings.reduce_motion {
            self.route_trans = 1.0;
            self.lyrics_open_spring
                .snap(if self.show_lyrics_overlay { 1.0 } else { 0.0 });
            self.sidebar_open_spring.snap(want_sb);
            self.sec_library_spring.snap(want_lib);
            self.sec_playlists_spring.snap(want_pls);
            self.sidebar_edit_spring.snap(want_edit);
            self.top_bar_guard_spring.snap(want_tb_guard);
            self.sidebar_sel_y_spring.snap(self.sidebar_sel_y_spring.target);
            self.sidebar_sel_alpha_spring
                .snap(self.sidebar_sel_alpha_spring.target);
        } else {
            if self.route_trans < 1.0 {
                self.route_trans = (self.route_trans + dt / 0.25).min(1.0);
            }
            self.lyrics_open_spring.target = if self.show_lyrics_overlay { 1.0 } else { 0.0 };
            self.lyrics_open_spring.step(dt);
            if self.lyrics_open_spring.is_settled_norm() {
                self.lyrics_open_spring.snap(self.lyrics_open_spring.target);
            }
            self.sidebar_open_spring.target = want_sb;
            self.sidebar_open_spring.step(dt);
            if self.sidebar_open_spring.is_settled_norm() {
                self.sidebar_open_spring.snap(self.sidebar_open_spring.target);
            }
            self.sec_library_spring.target = want_lib;
            self.sec_library_spring.step(dt);
            if self.sec_library_spring.is_settled_norm() {
                self.sec_library_spring.snap(self.sec_library_spring.target);
            }
            self.sec_playlists_spring.target = want_pls;
            self.sec_playlists_spring.step(dt);
            if self.sec_playlists_spring.is_settled_norm() {
                self.sec_playlists_spring.snap(self.sec_playlists_spring.target);
            }
            self.sidebar_edit_spring.target = want_edit;
            self.sidebar_edit_spring.step(dt);
            if self.sidebar_edit_spring.is_settled_norm() {
                self.sidebar_edit_spring.snap(self.sidebar_edit_spring.target);
            }
            self.top_bar_guard_spring.target = want_tb_guard;
            self.top_bar_guard_spring.step(dt);
            if self.top_bar_guard_spring.is_settled_norm() {
                self.top_bar_guard_spring.snap(self.top_bar_guard_spring.target);
            }
            self.sidebar_sel_y_spring.step(dt);
            if self.sidebar_sel_y_spring.is_settled() {
                self.sidebar_sel_y_spring.snap(self.sidebar_sel_y_spring.target);
            }
            self.sidebar_sel_alpha_spring.step(dt);
            if self.sidebar_sel_alpha_spring.is_settled_norm() {
                self.sidebar_sel_alpha_spring
                    .snap(self.sidebar_sel_alpha_spring.target);
            }
        }
        let lyr_open_t = self.lyrics_open_spring.value.clamp(0.0, 1.0);

        // Zero-drift monotonic wall-clock playhead sync (eliminates cumulative `f32 dt` drift!)
        let scrubbing_seek = matches!(
            self.active_drag,
            Some(ActiveDrag::LyricsSeek(_) | ActiveDrag::MiniPlayerSeek(_))
        );
        if !scrubbing_seek {
            if let Some((agent_aid, agent_status, live_pos, live_dur, resolved_ep)) =
                am_agent::get_live_agent_playhead_ext()
            {
                let epoch_ready = !self.playback_loading
                    || (resolved_ep > 0 && resolved_ep >= self.loading_cmd_epoch);
                if epoch_ready && agent_aid.is_some() && self.smtc.adam_id != agent_aid {
                    self.smtc.adam_id = agent_aid.clone();
                    if self.playback_loading {
                        self.playback_loading_track_id = agent_aid.clone();
                    }
                }
                if epoch_ready
                    && (agent_aid == self.smtc.adam_id
                        || self.smtc.adam_id.is_none()
                        || agent_aid.is_none())
                {
                    if live_dur > 0.5 && self.smtc.duration_sec <= 0.5 {
                        self.smtc.duration_sec = live_dur;
                    }
                    self.smtc.status = agent_status.clone();
                    self.smtc.position_sec = live_pos;
                    self.playhead_anchor_sec = live_pos;
                    self.playhead_anchor_inst = Instant::now();
                    if self.playback_loading
                        && agent_status == "Playing"
                        && (live_pos > 0.005 || am_agent::get_playback_state().position_is_calibrated)
                    {
                        self.playback_loading = false;
                        self.playback_loading_started = None;
                        self.playback_loading_track_id = None;
                        self.playback_loading_container_id = None;
                    }
                }
            } else if self.smtc.status == "Playing" {
                let live =
                    self.playhead_anchor_sec + self.playhead_anchor_inst.elapsed().as_secs_f64();
                self.smtc.position_sec = if self.smtc.duration_sec > 0.0 {
                    live.min(self.smtc.duration_sec)
                } else {
                    live
                };
            }
        }

        // Critically-damped 2nd-order smooth inertial scroll for main content & sidebar
        if self.settings.reduce_motion {
            self.scroll_y = self.scroll_target_y;
            self.scroll_vel_y = 0.0;
            self.sidebar_scroll_y = self.sidebar_scroll_target_y;
            self.sidebar_scroll_vel_y = 0.0;
        } else {
            step_critically_damped_scroll(
                &mut self.scroll_y,
                &mut self.scroll_vel_y,
                self.scroll_target_y,
                dt,
            );
            step_critically_damped_scroll(
                &mut self.sidebar_scroll_y,
                &mut self.sidebar_scroll_vel_y,
                self.sidebar_scroll_target_y,
                dt,
            );
        }

        // Sample live WASAPI audio visualizer ONLY when actively viewing the Visualizer page
        // (even in unfocused background window); zero WASAPI capture or FFT elsewhere!
        let is_playing_now = self.smtc.status == "Playing";
        let on_visualizer_page = self.route == PageRoute::Visualizer && !self.show_lyrics_overlay;
        if on_visualizer_page {
            self.latest_viz_snap = audio_viz::sample_live_visualizer(dt);
            self.viz_state
                .step(&self.latest_viz_snap, dt, self.settings.reduce_motion);
        } else {
            self.latest_viz_snap.active_signal = false;
            self.latest_viz_snap.beat_pulse = 0.0;
            self.latest_viz_snap.bass_energy = if is_playing_now && !self.settings.reduce_motion {
                let p = (self.smtc.position_sec as f32) * 2.8;
                0.22 + 0.14 * p.sin().abs()
            } else {
                0.0
            };
        }

        if is_playing_now && !self.settings.reduce_motion {
            let s = &self.latest_viz_snap;
            let beat = s.beat_pulse * 0.24;
            let raw_6 = if on_visualizer_page && s.active_signal {
                [
                    (s.spectrum_32[1] * 1.15 + beat).clamp(0.14, 1.0),
                    (s.spectrum_32[5] * 1.25 + beat * 0.8).clamp(0.18, 1.0),
                    (s.spectrum_32[10] * 1.30 + s.mid_energy * 0.35).clamp(0.16, 1.0),
                    (s.spectrum_32[16] * 1.30 + s.mid_energy * 0.30).clamp(0.16, 1.0),
                    (s.spectrum_32[22] * 1.35 + s.high_energy * 0.35).clamp(0.14, 1.0),
                    (s.spectrum_32[27] * 1.35 + s.high_energy * 0.30).clamp(0.12, 1.0),
                ]
            } else {
                let p = (self.smtc.position_sec as f32) * 5.8;
                [
                    0.26 + 0.34 * (p * 1.00 + 0.1).sin().abs(),
                    0.34 + 0.42 * (p * 1.28 + 1.1).sin().abs(),
                    0.30 + 0.44 * (p * 1.54 + 2.3).sin().abs(),
                    0.28 + 0.40 * (p * 1.78 + 3.4).sin().abs(),
                    0.22 + 0.36 * (p * 2.04 + 4.5).sin().abs(),
                    0.20 + 0.30 * (p * 2.26 + 5.2).sin().abs(),
                ]
            };
            let atk = (1.0 - (-24.0 * dt).exp()).clamp(0.05, 1.0);
            let rel = (1.0 - (-11.0 * dt).exp()).clamp(0.03, 1.0);
            for i in 0..6 {
                let k = if raw_6[i] > self.eq_bars_smooth[i] { atk } else { rel };
                self.eq_bars_smooth[i] += (raw_6[i] - self.eq_bars_smooth[i]) * k;
            }
        } else {
            let decay = (-12.0 * dt).exp();
            for b in &mut self.eq_bars_smooth {
                *b *= decay;
                if *b < 0.008 {
                    *b = 0.0;
                }
            }
        }

        // Request current track artwork if needed
        if let Some(art_url) = self.smtc.artwork_url.clone() {
            self.request_artwork(r, &art_url, true);
            self.request_artwork(r, &art_url, false);
        }

        if lyr_open_t > 0.004 {
            let pal = self
                .smtc
                .artwork_url
                .as_deref()
                .and_then(|u| r.get_artwork_palette(u));
            self.lyrics_view.step(
                self.current_lyrics.as_ref(),
                self.lyrics_loading,
                &self.current_lyrics_key,
                self.smtc.position_sec,
                is_playing_now,
                self.latest_viz_snap.bass_energy,
                pal,
                win_h - 84.0,
                dt,
                self.settings.reduce_motion,
                self.settings.lyrics_latin_by_word,
            );
        }

        // Fast path when Lyrics view is fully expanded (`lyr_open_t >= 0.996`)
        if lyr_open_t >= 0.996 {
            self.hover_alphas.clear();
            r.begin_draw(Theme::BG_PRIMARY);
            let art_key = self.smtc.artwork_url.clone();
            let lyr_key = self.current_lyrics_key.clone();
            let glass_off = self.glass_off();
            self.lyrics_view.draw(
                r,
                win_rect,
                &self.smtc,
                art_key.as_deref(),
                self.current_lyrics.as_ref(),
                &lyr_key,
                self.lyrics_loading,
                self.playback_loading,
                self.spinner_phase,
                self.latest_viz_snap.bass_energy,
                self.shuffle_on,
                self.repeat_on,
                self.mouse_pos,
                self.mouse_down,
                self.is_window_maximized,
                self.settings.reduce_motion,
                self.settings.reduce_transparency,
                glass_off,
                self.settings.lyrics_latin_by_word,
                self.settings.lyrics_non_emphasis_effect,
            );
            let sync_int = if live_resize_no_vsync {
                0
            } else {
                self.effective_frame_pacing().0
            };
            r.end_window_draw_and_present(
                self.settings.window_corner_radius,
                self.is_window_maximized,
                sync_int,
            );
            return;
        }

        if !self.show_lyrics_overlay && lyr_open_t <= 0.002 {
            self.lyrics_view.release_gpu_caches(r);
        }
        self.hit_regions.clear();
        r.begin_draw(Theme::BG_PRIMARY);

        // Pre-bakeAdded/Removed playlist diff rows into offscreen GPU bitmaps at clip/layer depth == 0!
        let mut diff_finished = false;
        if let Some(mut diff) = self.playlist_diff_anim.take() {
            let elapsed = diff.started_at.elapsed().as_secs_f32();
            if elapsed >= diff.duration_sec
                || !matches!(
                    self.route,
                    PageRoute::ContainerDetail { ref id, .. } if id == &diff.container_id
                )
            {
                diff_finished = true;
            } else {
                let sb_open_t = self.sidebar_open_spring.value.clamp(0.0, 1.0);
                let sb_layout_w = Theme::SIDEBAR_WIDTH * sb_open_t;
                let area_w = (win_w - sb_layout_w).max(200.0);
                let inner_w = (area_w - 64.0).max(200.0);
                let ease_trans = 1.0 - (1.0 - self.route_trans.clamp(0.0, 1.0)).powi(4);
                let trans_dy = if self.settings.reduce_motion {
                    0.0
                } else {
                    (1.0 - ease_trans) * 18.0 * self.route_trans_dir
                };
                let art_sz = 228.0f32.min(inner_w * 0.34).max(164.0);
                let rows_y0 = (62.0 - self.scroll_y + trans_dy) + art_sz + 28.0;
                let row_h = Theme::TRACK_ROW_HEIGHT;
                let row_w = inner_w + 16.0;
                diff.bake_slots.clear();
                let mut next_slot = 0usize;
                for (e_idx, ent) in diff.entries.iter().enumerate() {
                    if next_slot >= 24 {
                        break;
                    }
                    let (tr_opt, ry, disp_num): (Option<&AgentTrackInput>, f32, usize) =
                        match ent.kind {
                            PlaylistDiffKind::Removed { old_idx } => (
                                diff.old_tracks.get(old_idx),
                                rows_y0 + (old_idx as f32) * row_h,
                                old_idx + 1,
                            ),
                            PlaylistDiffKind::Added { new_idx } => (
                                diff.new_tracks.get(new_idx),
                                rows_y0 + (new_idx as f32) * row_h,
                                new_idx + 1,
                            ),
                            PlaylistDiffKind::KeptOrMoved { .. } => continue,
                        };
                    if ry + row_h < -32.0 || ry > win_h + 32.0 {
                        continue;
                    }
                    if let Some(tr) = tr_opt {
                        if let Some(ref u) = tr.artwork_url {
                            self.request_artwork(r, u, false);
                        }
                        if let Some(pad) = r.begin_row_fx_bake(next_slot, row_w, row_h) {
                            Self::render_track_row_visuals(
                                r,
                                pad + 8.0,
                                pad,
                                inner_w,
                                disp_num,
                                tr,
                                false,
                                false,
                                0.0,
                                false,
                                0.0,
                                [0.0; 4],
                                false,
                                false,
                                (0.0, 0.0),
                                true,
                            );
                            r.end_row_fx_bake();
                            diff.bake_slots.insert(e_idx, next_slot);
                            next_slot += 1;
                        }
                    }
                }
                self.playlist_diff_anim = Some(diff);
            }
        }
        if diff_finished {
            r.release_row_fx_surfaces();
        }

        // Smoothly animated sidebar width (`0.0 .. Theme::SIDEBAR_WIDTH`) + horizontal slide!
        let sb_open_t = self.sidebar_open_spring.value.clamp(0.0, 1.0);
        let sb_layout_w = Theme::SIDEBAR_WIDTH * sb_open_t;
        if sb_open_t <= 0.003 {
            self.sidebar_nav_rect = RectF::default();
        }
        let content_area = RectF::new(sb_layout_w, 0.0, (win_w - sb_layout_w).max(200.0), win_h);

        // 1. LAYER 1: Content Layer with smooth nonlinear page-transition entrance!
        self.draw_main_content_layer(r, content_area);
        clip_hit_regions_below_topbar(&mut self.hit_regions, TOP_BAR_GUARD_H);

        // Top-Bar Placeholder Scrim (fades in smoothly when cursor hovers into top bar while browsing!)
        let tb_guard_a = if self.route == PageRoute::Visualizer {
            0.0
        } else {
            self.top_bar_guard_spring.value.clamp(0.0, 1.0)
        };
        if tb_guard_a > 0.006 {
            let guard_rect = RectF::new(content_area.x, 0.0, content_area.w, TOP_BAR_GUARD_H);
            let bg_a = if self.glass_off() { 0.90 } else { 0.72 };
            r.fill_rect(guard_rect, Color::rgba(16, 16, 21, bg_a * tb_guard_a));
            r.draw_line(
                content_area.x,
                TOP_BAR_GUARD_H - 0.5,
                content_area.right(),
                TOP_BAR_GUARD_H - 0.5,
                Theme::SEPARATOR.with_alpha(Theme::SEPARATOR.a * tb_guard_a),
                1.0,
            );
            let grip = RectF::new(content_area.center_x() - 28.0, 26.0, 56.0, 4.0);
            r.fill_rounded_rect(grip, 2.0, Color::rgba(255, 255, 255, 0.30 * tb_guard_a));
        }

        // 2. LAYER 2: Functional Layer (Animated Sliding Sidebar, Unified Top Bar, Floating MiniPlayer, Popovers)
        if sb_open_t > 0.003 {
            let sb_x = -(1.0 - sb_open_t) * Theme::SIDEBAR_WIDTH;
            let sb_rect = RectF::new(sb_x, 0.0, Theme::SIDEBAR_WIDTH, win_h);
            let sb_alpha = (sb_open_t * 1.28).clamp(0.0, 1.0);
            let pushed_sb = if sb_alpha < 0.995 {
                r.push_opacity_layer(RectF::new(0.0, 0.0, sb_layout_w + 4.0, win_h), sb_alpha)
            } else {
                false
            };
            self.draw_ipad_sidebar(r, sb_rect);
            if pushed_sb {
                r.pop_layer();
            }
        }

        self.draw_top_floating_nav(r, content_area, sb_open_t);
        self.draw_floating_mini_player(r, content_area);

        if self.show_queue_popover {
            self.draw_queue_popover(r, content_area);
        }
        if self.show_more_popover {
            self.draw_more_popover(r, content_area);
        }
        if let Some((ref msg, is_err, _)) = self.banner_text.clone() {
            self.draw_banner_toast(r, content_area, msg, is_err);
        }

        // Decay any hover entries whose items scrolled offscreen so `hover_alphas` reaches empty!
        if !self.hover_alphas.is_empty() {
            let decay = (-5.8 * dt).exp();
            let visited = &self.visited_hover_keys;
            self.hover_alphas.retain(|k, v| {
                if !visited.contains(k) {
                    *v *= decay;
                }
                *v > 0.008
            });
        }

        // Smooth nonlinear Lyrics sheet expand / collapse overlay (`0.004 < lyr_open_t < 0.996`)!
        if lyr_open_t > 0.004 {
            let ease_alpha = lyr_open_t.powf(0.82).clamp(0.0, 1.0);
            let slide_y = (1.0 - lyr_open_t) * win_h * 0.12;
            let lyr_rect = RectF::new(0.0, slide_y, win_w, win_h);
            let pushed = r.push_opacity_layer(win_rect, ease_alpha);
            let art_key = self.smtc.artwork_url.clone();
            let lyr_key = self.current_lyrics_key.clone();
            let glass_off = self.glass_off();
            self.lyrics_view.draw(
                r,
                lyr_rect,
                &self.smtc,
                art_key.as_deref(),
                self.current_lyrics.as_ref(),
                &lyr_key,
                self.lyrics_loading,
                self.playback_loading,
                self.spinner_phase,
                self.latest_viz_snap.bass_energy,
                self.shuffle_on,
                self.repeat_on,
                self.mouse_pos,
                self.mouse_down,
                self.is_window_maximized,
                self.settings.reduce_motion,
                self.settings.reduce_transparency,
                glass_off,
                self.settings.lyrics_latin_by_word,
                self.settings.lyrics_non_emphasis_effect,
            );
            if pushed {
                r.pop_layer();
            }
        }

        // Never combine Present(1..=4, 0) VBlank block with explicit hr_timer wait!
        let sync_int = if live_resize_no_vsync {
            0
        } else {
            self.effective_frame_pacing().0
        };
        r.end_window_draw_and_present(
            self.settings.window_corner_radius,
            self.is_window_maximized,
            sync_int,
        );
    }

    // ========================================================================
    // iPadOS Apple Music Left Sidebar (`media_1794844113582_e39fc57c.jpg`)
    // ========================================================================
    fn draw_ipad_sidebar(&mut self, r: &mut D2DRenderer, sb: RectF) {
        let sb_fill = if self.glass_off() {
            Theme::SIDEBAR_OPAQUE
        } else {
            Theme::SIDEBAR_GLASS
        };
        r.fill_rect(sb, sb_fill);
        r.draw_line(sb.right(), sb.y, sb.right(), sb.bottom(), Theme::SEPARATOR, 1.0);

        let x0 = sb.x;
        let pad_x = 14.0;
        let top_y = Theme::TOP_BAR_Y;
        let btn_h = Theme::TOP_BAR_H;
        let btn_r = Theme::TOP_BAR_RADIUS;
        let edit_t = self.sidebar_edit_spring.value.clamp(0.0, 1.0);

        // Top Header Row: Left `编辑` pill + Right Sidebar Collapse Circle (`sb.x`-relative for smooth slide!)
        let edit_pill = RectF::new(x0 + pad_x, top_y, 58.0, btn_h);
        let edit_hov = edit_pill.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            edit_pill,
            btn_r,
            if self.sidebar_edit_mode {
                GlassBtnVariant::Tinted(Theme::ACCENT_RED)
            } else {
                GlassBtnVariant::Regular
            },
            edit_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            if self.sidebar_edit_mode { "完成" } else { "编辑" },
            edit_pill,
            13.0,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: edit_pill,
            action: UiAction::ToggleSidebarEdit,
        });

        let collapse_btn = RectF::new(sb.right() - pad_x - btn_h, top_y, btn_h, btn_h);
        let col_hov = collapse_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            collapse_btn,
            btn_r,
            GlassBtnVariant::Regular,
            col_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_icon(
            SfIcon::SidebarLeft,
            collapse_btn.inset(8.0, 8.0),
            Theme::TEXT_PRIMARY,
        );
        self.hit_regions.push(HitRegion {
            rect: collapse_btn,
            action: UiAction::ToggleSidebar,
        });

        let mut y = top_y + btn_h + 12.0;

        // Search Bar Capsule (`#2B2930` rounded pill, red magnifying glass + `搜索`)
        let sbox = RectF::new(x0 + pad_x, y, sb.w - pad_x * 2.0, 36.0);
        self.search_box_rect = sbox;
        r.fill_rounded_rect(sbox, 18.0, Theme::SIDEBAR_PILL);
        if self.search_focused {
            r.stroke_rounded_rect(sbox, 18.0, Theme::ACCENT_RED.with_alpha(0.70), 1.2);
        }
        r.draw_icon(
            SfIcon::Search,
            RectF::new(sbox.x + 12.0, sbox.y + 9.5, 17.0, 17.0),
            Theme::ACCENT_RED,
        );
        let disp_q = if self.search_query.is_empty() {
            if self.search_focused {
                "|"
            } else {
                "搜索"
            }
        } else if self.search_focused {
            &format!("{}|", self.search_query)
        } else {
            &self.search_query
        };
        r.draw_text(
            disp_q,
            RectF::new(sbox.x + 36.0, sbox.y, sbox.w - 64.0, sbox.h),
            14.0,
            400,
            if self.search_query.is_empty() {
                Theme::TEXT_SECONDARY
            } else {
                Theme::TEXT_PRIMARY
            },
            TextAlign::Left,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: sbox,
            action: UiAction::FocusSearch,
        });
        if !self.search_query.is_empty() {
            let clr = RectF::new(sbox.right() - 28.0, sbox.y + 6.0, 22.0, 24.0);
            r.draw_text(
                "×",
                clr,
                15.0,
                600,
                Theme::TEXT_SECONDARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: clr,
                action: UiAction::ClearSearch,
            });
        }

        y += 46.0;

        // Scrollable Sidebar Navigation Items (reserve 56px at bottom for user/settings bar)
        let nav_clip = RectF::new(x0, y, sb.w - 1.0, (sb.h - y - 58.0).max(80.0));
        self.sidebar_nav_rect = nav_clip;
        r.push_clip(nav_clip);

        let nav_origin_y = y;
        y -= self.sidebar_scroll_y;

        // Smoothly gliding active-route selection pill (drawn behind sidebar rows!)
        let sel_pill_alpha = self.sidebar_sel_alpha_spring.value.clamp(0.0, 1.0) * (1.0 - edit_t);
        if self.sidebar_sel_initialized && sel_pill_alpha > 0.008 {
            let sel_y = nav_origin_y + self.sidebar_sel_y_spring.value - self.sidebar_scroll_y;
            let sel_pill = RectF::new(x0 + 10.0, sel_y, sb.w - 20.0, 36.0);
            if sel_pill.bottom() >= nav_clip.y && sel_pill.y <= nav_clip.bottom() {
                r.fill_rounded_rect(
                    sel_pill,
                    9.0,
                    Theme::SIDEBAR_SELECTED.with_alpha(Theme::SIDEBAR_SELECTED.a * sel_pill_alpha),
                );
            }
        }

        let mut found_sel_rel_y: Option<f32> = None;

        // Primary Items (`主页`, `新发现`, `广播`, `可视化`)
        let primary_items = [
            ("home", "主页", SfIcon::House, PageRoute::Home),
            ("browse", "新发现", SfIcon::Grid, PageRoute::Browse),
            ("radio", "广播", SfIcon::Radio, PageRoute::Radio),
            ("viz", "可视化", SfIcon::Visualizer, PageRoute::Visualizer),
        ];
        for (key, label, icon, target_route) in primary_items {
            let hidden = self.settings.hidden_sidebar_keys.iter().any(|k| k == key);
            if hidden && edit_t <= 0.005 {
                continue;
            }
            if !hidden && !self.sidebar_edit_mode && self.route == target_route {
                found_sel_rel_y = Some(y - nav_origin_y + self.sidebar_scroll_y);
            }
            y = self.draw_sidebar_row(
                r,
                x0,
                sb.w,
                y,
                nav_clip,
                key,
                label,
                icon,
                target_route,
                hidden,
                1.0,
                edit_t,
            );
        }

        y += 12.0;

        // Collapsible Section: `资料库` (smooth accordion height + continuous chevron rotation!)
        let lib_t = self.sec_library_spring.value.clamp(0.0, 1.0);
        let lib_hdr = RectF::new(x0 + pad_x, y, sb.w - pad_x * 2.0, 28.0);
        if lib_hdr.bottom() >= nav_clip.y && lib_hdr.y <= nav_clip.bottom() {
            let lh_hov = nav_clip.contains(self.mouse_pos.0, self.mouse_pos.1)
                && lib_hdr.contains(self.mouse_pos.0, self.mouse_pos.1);
            let lh_t = self.eval_hover_alpha(fnv1a_hover_key("sb_hdr", "lib"), lh_hov);
            if lh_t > 0.01 {
                r.fill_rounded_rect(
                    RectF::new(x0 + 10.0, lib_hdr.y, sb.w - 20.0, lib_hdr.h),
                    7.0,
                    Theme::ROW_HOVER.with_alpha(Theme::ROW_HOVER.a * 0.65 * lh_t),
                );
            }
            let hdr_col = Theme::TEXT_MUTED.lerp(Theme::TEXT_PRIMARY, lh_t * 0.65);
            r.draw_text(
                "资料库",
                RectF::new(lib_hdr.x + 4.0, lib_hdr.y, 120.0, 28.0),
                12.5,
                600,
                hdr_col,
                TextAlign::Left,
                true,
                false,
            );
            r.draw_chevron_morph(
                RectF::new(lib_hdr.right() - 18.0, lib_hdr.y + 7.5, 13.0, 13.0),
                lib_t,
                hdr_col,
            );
            if let Some(clipped) = intersect_rect(lib_hdr, nav_clip) {
                self.hit_regions.push(HitRegion {
                    rect: clipped,
                    action: UiAction::ToggleSectionLibrary,
                });
            }
        }
        y += 30.0;

        if lib_t > 0.004 {
            let lib_items = [
                ("lib_recent", "最近添加", SfIcon::Clock, PageRoute::LibRecent),
                ("lib_artists", "艺人", SfIcon::Mic, PageRoute::LibArtists),
                ("lib_albums", "专辑", SfIcon::Albums, PageRoute::LibAlbums),
                ("lib_songs", "歌曲", SfIcon::MusicNote, PageRoute::LibSongs),
                ("lib_foryou", "专属推荐", SfIcon::PersonSquare, PageRoute::LibForYou),
                ("lib_dl", "已下载", SfIcon::DownloadCircle, PageRoute::LibDownloads),
            ];
            let mut full_lib_h = 0.0f32;
            for (key, _, _, _) in &lib_items {
                let hidden = self.settings.hidden_sidebar_keys.iter().any(|k| k == key);
                if !hidden {
                    full_lib_h += 37.0;
                } else if edit_t > 0.005 {
                    full_lib_h += 37.0 * edit_t;
                }
            }
            let vis_lib_h = full_lib_h * lib_t;
            let sec_box = RectF::new(x0, y, sb.w - 1.0, vis_lib_h);
            let sec_clip = intersect_rect(sec_box, nav_clip).unwrap_or_default();
            let pushed_sec = lib_t < 0.996;
            if pushed_sec && sec_clip.h > 0.5 {
                r.push_clip(sec_clip);
            }
            let active_clip = if pushed_sec { sec_clip } else { nav_clip };
            let mut inner_y = y - (1.0 - lib_t) * 8.0;
            let row_alpha = lib_t.powf(0.65);

            if !pushed_sec || sec_clip.h > 0.5 {
                for (key, label, icon, target_route) in lib_items {
                    let hidden = self.settings.hidden_sidebar_keys.iter().any(|k| k == key);
                    if hidden && edit_t <= 0.005 {
                        continue;
                    }
                    if !hidden
                        && !self.sidebar_edit_mode
                        && lib_t > 0.45
                        && self.route == target_route
                    {
                        found_sel_rel_y = Some(inner_y - nav_origin_y + self.sidebar_scroll_y);
                    }
                    inner_y = self.draw_sidebar_row(
                        r,
                        x0,
                        sb.w,
                        inner_y,
                        active_clip,
                        key,
                        label,
                        icon,
                        target_route,
                        hidden,
                        row_alpha,
                        edit_t,
                    );
                }
            }
            if pushed_sec && sec_clip.h > 0.5 {
                r.pop_clip();
            }
            y += vis_lib_h;
        }

        y += 12.0;

        // Collapsible Section: `播放列表` (smooth accordion height + continuous chevron rotation!)
        let pl_t = self.sec_playlists_spring.value.clamp(0.0, 1.0);
        let pl_hdr = RectF::new(x0 + pad_x, y, sb.w - pad_x * 2.0, 28.0);
        if pl_hdr.bottom() >= nav_clip.y && pl_hdr.y <= nav_clip.bottom() {
            let ph_hov = nav_clip.contains(self.mouse_pos.0, self.mouse_pos.1)
                && pl_hdr.contains(self.mouse_pos.0, self.mouse_pos.1);
            let ph_t = self.eval_hover_alpha(fnv1a_hover_key("sb_hdr", "pls"), ph_hov);
            if ph_t > 0.01 {
                r.fill_rounded_rect(
                    RectF::new(x0 + 10.0, pl_hdr.y, sb.w - 20.0, pl_hdr.h),
                    7.0,
                    Theme::ROW_HOVER.with_alpha(Theme::ROW_HOVER.a * 0.65 * ph_t),
                );
            }
            let hdr_col = Theme::TEXT_MUTED.lerp(Theme::TEXT_PRIMARY, ph_t * 0.65);
            r.draw_text(
                "播放列表",
                RectF::new(pl_hdr.x + 4.0, pl_hdr.y, 120.0, 28.0),
                12.5,
                600,
                hdr_col,
                TextAlign::Left,
                true,
                false,
            );
            r.draw_chevron_morph(
                RectF::new(pl_hdr.right() - 18.0, pl_hdr.y + 7.5, 13.0, 13.0),
                pl_t,
                hdr_col,
            );
            if let Some(clipped) = intersect_rect(pl_hdr, nav_clip) {
                self.hit_regions.push(HitRegion {
                    rect: clipped,
                    action: UiAction::ToggleSectionPlaylists,
                });
            }
        }
        y += 30.0;

        if pl_t > 0.004 {
            let hidden_all = self
                .settings
                .hidden_sidebar_keys
                .iter()
                .any(|k| k == "pl_all");
            let pls_count = self.sidebar_playlists.len().min(24);
            let mut full_pl_h = (pls_count as f32) * 37.0;
            if !hidden_all {
                full_pl_h += 37.0;
            } else if edit_t > 0.005 {
                full_pl_h += 37.0 * edit_t;
            }

            let vis_pl_h = full_pl_h * pl_t;
            let sec_box = RectF::new(x0, y, sb.w - 1.0, vis_pl_h);
            let sec_clip = intersect_rect(sec_box, nav_clip).unwrap_or_default();
            let pushed_sec = pl_t < 0.996;
            if pushed_sec && sec_clip.h > 0.5 {
                r.push_clip(sec_clip);
            }
            let active_clip = if pushed_sec { sec_clip } else { nav_clip };
            let mut inner_y = y - (1.0 - pl_t) * 8.0;
            let row_alpha = pl_t.powf(0.65);

            if !pushed_sec || sec_clip.h > 0.5 {
                if !hidden_all || edit_t > 0.005 {
                    if !hidden_all
                        && !self.sidebar_edit_mode
                        && pl_t > 0.45
                        && self.route == PageRoute::LibPlaylists
                    {
                        found_sel_rel_y = Some(inner_y - nav_origin_y + self.sidebar_scroll_y);
                    }
                    inner_y = self.draw_sidebar_row(
                        r,
                        x0,
                        sb.w,
                        inner_y,
                        active_clip,
                        "pl_all",
                        "所有播放列表",
                        SfIcon::PlaylistGrid,
                        PageRoute::LibPlaylists,
                        hidden_all,
                        row_alpha,
                        edit_t,
                    );
                }

                // Zero-clone stack move of `sidebar_playlists`!
                let pls = std::mem::take(&mut self.sidebar_playlists);
                for pl in pls.iter().take(24) {
                    let row = RectF::new(x0 + 10.0, inner_y, sb.w - 20.0, 36.0);
                    let is_sel = !self.sidebar_edit_mode
                        && matches!(
                            &self.route,
                            PageRoute::ContainerDetail { id, .. } if id == &pl.id
                        );
                    if is_sel && pl_t > 0.45 {
                        found_sel_rel_y = Some(inner_y - nav_origin_y + self.sidebar_scroll_y);
                    }
                    if row.bottom() >= active_clip.y && row.y <= active_clip.bottom() {
                        let is_hov = active_clip.h > 1.0
                            && active_clip.contains(self.mouse_pos.0, self.mouse_pos.1)
                            && row.contains(self.mouse_pos.0, self.mouse_pos.1);
                        let hov_t = self.eval_hover_alpha(fnv1a_hover_key("sb_pl", &pl.id), is_hov);
                        if !is_sel && hov_t > 0.005 {
                            r.fill_rounded_rect(
                                row,
                                9.0,
                                Theme::ROW_HOVER.with_alpha(Theme::ROW_HOVER.a * hov_t * row_alpha),
                            );
                        }

                        let thumb = RectF::new(row.x + 8.0, row.y + 6.0, 24.0, 24.0);
                        let mut drew_t = false;
                        if let Some(ref u) = pl.artwork_url {
                            self.request_artwork(r, u, false);
                            drew_t = r.draw_rounded_artwork(u, false, thumb, 5.0, row_alpha);
                        }
                        if !drew_t {
                            r.fill_rounded_rect(
                                thumb,
                                5.0,
                                Color::rgb(245, 245, 247).with_alpha(row_alpha),
                            );
                            r.draw_icon(
                                SfIcon::Star,
                                thumb.inset(4.5, 4.5),
                                Theme::ACCENT_RED.with_alpha(row_alpha),
                            );
                        }

                        r.draw_text(
                            &pl.title,
                            RectF::new(row.x + 40.0, row.y, row.w - 46.0, row.h),
                            13.8,
                            if is_sel { 600 } else { 400 },
                            Theme::TEXT_PRIMARY.with_alpha(row_alpha),
                            TextAlign::Left,
                            true,
                            false,
                        );
                        if pl_t > 0.55 {
                            if let Some(clipped) = intersect_rect(row, active_clip) {
                                self.hit_regions.push(HitRegion {
                                    rect: clipped,
                                    action: UiAction::Navigate(PageRoute::ContainerDetail {
                                        kind: pl.kind.clone(),
                                        id: pl.id.clone(),
                                        title_hint: pl.title.clone(),
                                    }),
                                });
                            }
                        }
                    }
                    inner_y += 37.0;
                }
                self.sidebar_playlists = pls;
            }
            if pushed_sec && sec_clip.h > 0.5 {
                r.pop_clip();
            }
            y += vis_pl_h;
        }

        // Update sliding selection pill spring target!
        if let Some(rel_y) = found_sel_rel_y {
            if !self.sidebar_sel_initialized
                || self.sidebar_sel_alpha_spring.value < 0.08
                || self.settings.reduce_motion
            {
                self.sidebar_sel_y_spring.snap(rel_y);
                self.sidebar_sel_initialized = true;
            } else {
                self.sidebar_sel_y_spring.target = rel_y;
            }
            self.sidebar_sel_alpha_spring.target = 1.0;
            if self.settings.reduce_motion {
                self.sidebar_sel_alpha_spring.snap(1.0);
            }
        } else {
            self.sidebar_sel_alpha_spring.target = 0.0;
            if self.settings.reduce_motion {
                self.sidebar_sel_alpha_spring.snap(0.0);
            }
        }

        let nav_total_h = (y + self.sidebar_scroll_y) - nav_origin_y + 8.0;
        self.sidebar_max_scroll_y = (nav_total_h - nav_clip.h).max(0.0);
        if self.sidebar_scroll_target_y > self.sidebar_max_scroll_y {
            self.sidebar_scroll_target_y = self.sidebar_max_scroll_y;
        }
        if self.sidebar_scroll_y > self.sidebar_max_scroll_y {
            self.sidebar_scroll_y = self.sidebar_max_scroll_y;
        }

        r.pop_clip();

        // Bottom Account / Settings Footer (`sb.x`-relative)
        let foot = RectF::new(x0 + 8.0, sb.bottom() - 52.0, sb.w - 16.0, 44.0);
        let is_set = self.route == PageRoute::Settings;
        let foot_hov = foot.contains(self.mouse_pos.0, self.mouse_pos.1);
        let foot_hov_t = self.eval_hover_alpha(fnv1a_hover_key("sb_foot", "settings"), foot_hov);
        if is_set {
            r.fill_rounded_rect(foot, 10.0, Theme::SIDEBAR_SELECTED);
        } else if foot_hov_t > 0.005 {
            r.fill_rounded_rect(
                foot,
                10.0,
                Theme::ROW_HOVER.with_alpha(Theme::ROW_HOVER.a * foot_hov_t),
            );
        }

        let av_cx = foot.x + 22.0;
        let av_cy = foot.center_y();
        r.fill_circle(av_cx, av_cy, 14.0, Color::rgb(98, 92, 168));
        let av_glyph: String = if let Some(ref name) = self.official_account.display_name {
            name.chars().next().map(|c| c.to_string()).unwrap_or_else(|| "♫".to_string())
        } else if self.settings.user_token.is_empty() {
            "A".to_string()
        } else {
            "♫".to_string()
        };
        r.draw_text(
            &av_glyph,
            RectF::new(av_cx - 14.0, av_cy - 14.0, 28.0, 28.0),
            12.5,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );

        let acct_label = if let Some(ref name) = self.official_account.display_name {
            format!("{name} · {}", self.settings.storefront.to_uppercase())
        } else if !self.settings.user_token.is_empty() || self.official_account.signed_in {
            format!("个人资料库 · {}", self.settings.storefront.to_uppercase())
        } else {
            format!("偏好设置 ({})", self.settings.storefront.to_uppercase())
        };
        r.draw_text(
            &acct_label,
            RectF::new(foot.x + 44.0, foot.y, foot.w - 74.0, foot.h),
            13.5,
            500,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_icon(
            SfIcon::Gear,
            RectF::new(foot.right() - 26.0, foot.center_y() - 8.5, 17.0, 17.0),
            Theme::TEXT_SECONDARY,
        );
        self.hit_regions.push(HitRegion {
            rect: foot,
            action: UiAction::Navigate(PageRoute::Settings),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_sidebar_row(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        sb_w: f32,
        y: f32,
        nav_clip: RectF,
        key: &str,
        label: &str,
        icon: SfIcon,
        target_route: PageRoute,
        hidden: bool,
        sec_alpha: f32,
        edit_t: f32,
    ) -> f32 {
        let step_h = if hidden { 37.0 * edit_t } else { 37.0 };
        let row = RectF::new(x0 + 10.0, y, sb_w - 20.0, 36.0);
        if row.bottom() < nav_clip.y || row.y > nav_clip.bottom() || nav_clip.h <= 0.5 {
            return y + step_h;
        }
        let is_sel = self.route == target_route && !self.sidebar_edit_mode;
        let is_hov = nav_clip.contains(self.mouse_pos.0, self.mouse_pos.1)
            && row.contains(self.mouse_pos.0, self.mouse_pos.1);
        let hov_t = self.eval_hover_alpha(fnv1a_hover_key("sb_row", key), is_hov);

        // Active selection background is rendered by the smoothly gliding `sel_pill` behind rows;
        // draw hover highlight only on non-selected (or edit-mode) rows!
        if (!is_sel || edit_t > 0.05) && hov_t > 0.005 {
            r.fill_rounded_rect(
                row,
                9.0,
                Theme::ROW_HOVER.with_alpha(Theme::ROW_HOVER.a * hov_t * sec_alpha),
            );
        }

        let vis_alpha = (if hidden { 0.38 * edit_t } else { 1.0 }) * sec_alpha;
        let ic_rect = RectF::new(row.x + 10.0, row.y + 8.5, 19.0, 19.0);
        if icon == SfIcon::Visualizer && self.eq_bars_smooth.iter().any(|&v| v > 0.02) {
            let eq6 = self.eq_bars_smooth;
            r.draw_live_equalizer_bars(
                ic_rect.inset(1.0, 1.5),
                Theme::ACCENT_RED.with_alpha(vis_alpha),
                &eq6,
                true,
            );
        } else {
            r.draw_icon(icon, ic_rect, Theme::ACCENT_RED.with_alpha(vis_alpha));
        }
        r.draw_text(
            label,
            RectF::new(row.x + 38.0, row.y, row.w - 66.0, row.h),
            14.2,
            if is_sel { 600 } else { 400 },
            Theme::TEXT_PRIMARY.with_alpha(vis_alpha),
            TextAlign::Left,
            true,
            false,
        );

        if edit_t > 0.02 {
            let s = 0.65 + 0.35 * edit_t;
            let chk = RectF::new(row.right() - 26.0, row.y + 9.0, 18.0, 18.0);
            if !hidden {
                r.fill_circle(
                    chk.center_x(),
                    chk.center_y(),
                    9.0 * s,
                    Theme::ACCENT_RED.with_alpha(edit_t * sec_alpha),
                );
                r.draw_icon(
                    SfIcon::Check,
                    chk.inset(9.0 - 5.5 * s, 9.0 - 5.5 * s),
                    Theme::TEXT_PRIMARY.with_alpha(edit_t * sec_alpha),
                );
            } else {
                r.stroke_circle(
                    chk.center_x(),
                    chk.center_y(),
                    8.5 * s,
                    Theme::TEXT_MUTED.with_alpha(edit_t * sec_alpha),
                    1.2,
                );
            }
        }

        if sec_alpha > 0.45 {
            if self.sidebar_edit_mode {
                if let Some(clipped) = intersect_rect(row, nav_clip) {
                    self.hit_regions.push(HitRegion {
                        rect: clipped,
                        action: UiAction::ToggleSidebarItemHidden(key.to_string()),
                    });
                }
            } else if !hidden {
                if let Some(clipped) = intersect_rect(row, nav_clip) {
                    self.hit_regions.push(HitRegion {
                        rect: clipped,
                        action: UiAction::Navigate(target_route),
                    });
                }
            }
        }

        y + step_h
    }

    // ========================================================================
    // Floating Top Immersive Bar (`<` Back + Reopen Sidebar + Functional + Window Controls)
    // Strict pixel-level centerline alignment (`Y = 14.0, H = 34.0, R = 17.0, center_y = 31.0`)!
    // ========================================================================
    fn draw_top_floating_nav(
        &mut self,
        r: &mut D2DRenderer,
        area: RectF,
        sb_open_t: f32,
    ) {
        let mut lx = area.x + 18.0;
        let ty = Theme::TOP_BAR_Y;
        let bh = Theme::TOP_BAR_H;
        let br = Theme::TOP_BAR_RADIUS;

        let reopen_t = (1.0 - sb_open_t).clamp(0.0, 1.0);
        if reopen_t > 0.01 {
            let sb_btn = RectF::new(lx, ty, bh, bh);
            let sb_hov = reopen_t > 0.45 && sb_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
            let pushed = if reopen_t < 0.99 {
                r.push_opacity_layer(sb_btn.inset(-4.0, -4.0), reopen_t)
            } else {
                false
            };
            r.draw_liquid_glass_button(
                sb_btn,
                br,
                GlassBtnVariant::Regular,
                sb_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_icon(SfIcon::SidebarLeft, sb_btn.inset(8.0, 8.0), Theme::TEXT_PRIMARY);
            if pushed {
                r.pop_layer();
            }
            if reopen_t > 0.45 {
                self.hit_regions.push(HitRegion {
                    rect: sb_btn,
                    action: UiAction::ToggleSidebar,
                });
            }
            lx += (bh + 8.0) * reopen_t;
        }

        let back_btn = RectF::new(lx, ty, bh, bh);
        let can_back = !self.nav_history.is_empty();
        let back_hov = can_back && back_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            back_btn,
            br,
            if can_back {
                GlassBtnVariant::Regular
            } else {
                GlassBtnVariant::Subtle
            },
            back_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_icon(
            SfIcon::ChevronLeft,
            back_btn.inset(9.0, 9.0),
            if can_back {
                Theme::TEXT_PRIMARY
            } else {
                Theme::TEXT_MUTED
            },
        );
        self.hit_regions.push(HitRegion {
            rect: back_btn,
            action: if can_back {
                UiAction::NavigateBack
            } else {
                UiAction::SwallowClick
            },
        });

        // Top-right Unified Liquid Glass Window Controls (`─`, `□`, `×`) — identical geometry & material
        let cls_r = RectF::new(area.right() - 18.0 - bh, ty, bh, bh);
        let max_r = RectF::new(cls_r.x - 8.0 - bh, ty, bh, bh);
        let min_r = RectF::new(max_r.x - 8.0 - bh, ty, bh, bh);

        for (wr, ic, is_close, act) in [
            (min_r, SfIcon::WinMinimize, false, UiAction::WindowMinimize),
            (
                max_r,
                if self.is_window_maximized {
                    SfIcon::WinRestore
                } else {
                    SfIcon::WinMaximize
                },
                false,
                UiAction::WindowToggleMaximize,
            ),
            (cls_r, SfIcon::WinClose, true, UiAction::WindowClose),
        ] {
            let hov = wr.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                wr,
                br,
                if is_close && hov {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_icon(
                ic,
                wr.inset(9.5, 9.5),
                if hov {
                    Theme::TEXT_PRIMARY
                } else {
                    Theme::TEXT_SECONDARY
                },
            );
            self.hit_regions.push(HitRegion {
                rect: wr,
                action: act,
            });
        }

        // Top-right Unified Liquid Glass Functional Buttons (`刷新`, `偏好设置 •••`) — same 34x34 / r=17 centerline!
        let more_r = RectF::new(min_r.x - 14.0 - bh, ty, bh, bh);
        let ref_r = RectF::new(more_r.x - 8.0 - bh, ty, bh, bh);
        for (hr, ic, is_active, act) in [
            (ref_r, SfIcon::Refresh, false, UiAction::RefreshCurrentView),
            (
                more_r,
                SfIcon::MoreDots,
                self.route == PageRoute::Settings,
                UiAction::Navigate(PageRoute::Settings),
            ),
        ] {
            let hov = hr.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                hr,
                br,
                if is_active {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_icon(ic, hr.inset(8.5, 8.5), Theme::TEXT_PRIMARY);
            self.hit_regions.push(HitRegion {
                rect: hr,
                action: act,
            });
        }
    }

    // ========================================================================
    // Bottom Center Floating Liquid Glass MiniPlayer Capsule (1:1 with Screenshot!)
    // ========================================================================
    fn draw_floating_mini_player(&mut self, r: &mut D2DRenderer, area: RectF) {
        let pill_w = (area.w - 48.0).clamp(380.0, Theme::MINI_PLAYER_MAX_WIDTH);
        let pill_h = Theme::MINI_PLAYER_HEIGHT;
        let pill_x = area.center_x() - pill_w * 0.5;
        let pill_y = area.bottom() - pill_h - Theme::MINI_PLAYER_BOTTOM_MARGIN;
        let pill = RectF::new(pill_x, pill_y, pill_w, pill_h);

        // Swallow clicks on empty glass inside the floating MiniPlayer so they never click underlying track rows!
        self.hit_regions.push(HitRegion {
            rect: pill,
            action: UiAction::SwallowClick,
        });

        // Ambient elevation shadow underneath the floating capsule
        r.fill_rounded_rect(
            RectF::new(pill.x + 6.0, pill.y + 6.0, pill.w - 12.0, pill.h),
            31.0,
            Color::rgba(0, 0, 0, 0.42),
        );

        // True GPU-blurred Liquid Glass material over live scrolled content!
        r.draw_liquid_glass(
            pill,
            31.0,
            18.0,
            Theme::GLASS_REGULAR_FILL,
            Theme::GLASS_RIM_LIGHT,
            self.glass_off(),
        );

        let cy = pill.center_y();
        let is_playing = self.smtc.status == "Playing";

        // Left Transport Cluster (`Shuffle`, `Prev`, `Play/Pause`, `Next`, `Repeat` — Liquid Glass hover!)
        let mut cx = pill.x + 26.0;
        let shuf_r = RectF::new(cx - 14.0, cy - 14.0, 28.0, 28.0);
        let shuf_hov = shuf_r.contains(self.mouse_pos.0, self.mouse_pos.1);
        if shuf_hov || self.shuffle_on {
            r.draw_liquid_glass_button(
                shuf_r,
                14.0,
                if self.shuffle_on {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Subtle
                },
                shuf_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
        }
        r.draw_icon(
            SfIcon::Shuffle,
            shuf_r.inset(6.5, 6.5),
            if self.shuffle_on || shuf_hov {
                Theme::TEXT_PRIMARY
            } else {
                Theme::TEXT_SECONDARY
            },
        );
        self.hit_regions.push(HitRegion {
            rect: shuf_r,
            action: UiAction::ToggleShuffleMode,
        });
        cx += 34.0;

        let prev_r = RectF::new(cx - 16.0, cy - 16.0, 32.0, 32.0);
        let prev_hov = prev_r.contains(self.mouse_pos.0, self.mouse_pos.1);
        if prev_hov {
            r.draw_liquid_glass_button(
                prev_r,
                16.0,
                GlassBtnVariant::Regular,
                true,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
        }
        r.draw_icon(SfIcon::Prev, prev_r.inset(7.0, 7.0), Theme::TEXT_PRIMARY);
        self.hit_regions.push(HitRegion {
            rect: prev_r,
            action: UiAction::PrevTrack,
        });
        cx += 38.0;

        let play_r = RectF::new(cx - 19.0, cy - 19.0, 38.0, 38.0);
        let play_hov = play_r.contains(self.mouse_pos.0, self.mouse_pos.1);
        if play_hov || self.playback_loading {
            r.draw_liquid_glass_button(
                play_r,
                19.0,
                GlassBtnVariant::Regular,
                play_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
        }
        if self.playback_loading {
            r.draw_loading_spinner(cx, cy, 11.5, self.spinner_phase, Theme::TEXT_PRIMARY);
        } else {
            r.draw_icon(
                if is_playing {
                    SfIcon::Pause
                } else {
                    SfIcon::Play
                },
                play_r.inset(7.0, 7.0),
                Theme::TEXT_PRIMARY,
            );
        }
        self.hit_regions.push(HitRegion {
            rect: play_r,
            action: UiAction::TogglePlayPause,
        });
        cx += 38.0;

        let next_r = RectF::new(cx - 16.0, cy - 16.0, 32.0, 32.0);
        let next_hov = next_r.contains(self.mouse_pos.0, self.mouse_pos.1);
        if next_hov {
            r.draw_liquid_glass_button(
                next_r,
                16.0,
                GlassBtnVariant::Regular,
                true,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
        }
        r.draw_icon(SfIcon::Next, next_r.inset(7.0, 7.0), Theme::TEXT_PRIMARY);
        self.hit_regions.push(HitRegion {
            rect: next_r,
            action: UiAction::NextTrack,
        });
        cx += 34.0;

        let rep_r = RectF::new(cx - 14.0, cy - 14.0, 28.0, 28.0);
        let rep_hov = rep_r.contains(self.mouse_pos.0, self.mouse_pos.1);
        if rep_hov || self.repeat_on {
            r.draw_liquid_glass_button(
                rep_r,
                14.0,
                if self.repeat_on {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Subtle
                },
                rep_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
        }
        r.draw_icon(
            SfIcon::Repeat,
            rep_r.inset(6.5, 6.5),
            if self.repeat_on || rep_hov {
                Theme::TEXT_PRIMARY
            } else {
                Theme::TEXT_SECONDARY
            },
        );
        self.hit_regions.push(HitRegion {
            rect: rep_r,
            action: UiAction::ToggleRepeatMode,
        });

        // Center-Left: Album Cover Thumbnail (42x42 rounded 8px) + Buffering Spinner + Title & Artist
        let art_x = cx + 22.0;
        let art_rect = RectF::new(art_x, cy - 21.0, 42.0, 42.0);
        if let Some(ref u) = self.smtc.artwork_url.clone() {
            self.request_artwork(r, u, false);
        }
        let drew_cov = self
            .smtc
            .artwork_url
            .as_deref()
            .map(|u| r.draw_rounded_artwork(u, false, art_rect, 8.0, 1.0))
            .unwrap_or(false);
        if !drew_cov {
            r.fill_rounded_rect(art_rect, 8.0, Color::rgba(255, 255, 255, 0.08));
            r.draw_icon(SfIcon::MusicNote, art_rect.inset(11.0, 11.0), Theme::TEXT_MUTED);
        }
        if self.playback_loading {
            r.fill_rounded_rect(art_rect, 8.0, Color::rgba(8, 8, 12, 0.56));
            r.draw_loading_spinner(
                art_rect.center_x(),
                art_rect.center_y(),
                10.5,
                self.spinner_phase,
                Color::rgba(255, 255, 255, 0.96),
            );
        }

        let right_cluster_start = pill.right() - 122.0;
        let meta_rect = RectF::new(
            art_rect.right() + 12.0,
            pill.y + 8.0,
            (right_cluster_start - art_rect.right() - 18.0).max(60.0),
            pill.h - 18.0,
        );

        let title_str = if self.smtc.title.is_empty() {
            "Apple Music"
        } else {
            &self.smtc.title
        };
        let sub_owned: String;
        let sub_str: &str = if self.playback_loading {
            if self.smtc.artist.is_empty() {
                "正在缓冲音频流…"
            } else {
                sub_owned = format!("正在准备播放… · {}", self.smtc.artist);
                &sub_owned
            }
        } else if self.smtc.artist.is_empty() {
            "选择任意歌曲即可开始播放"
        } else {
            &self.smtc.artist
        };
        r.draw_text(
            title_str,
            RectF::new(meta_rect.x, pill.y + 11.0, meta_rect.w, 20.0),
            13.8,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            sub_str,
            RectF::new(meta_rect.x, pill.y + 30.0, meta_rect.w, 18.0),
            12.0,
            400,
            if self.playback_loading {
                Color::rgba(255, 255, 255, 0.86)
            } else {
                Theme::TEXT_SECONDARY
            },
            TextAlign::Left,
            true,
            false,
        );

        // Clicking cover or song metadata opens LyricsBlossom Immersive Synced Lyrics View!
        let open_lyrics_zone = RectF::new(
            art_rect.x,
            pill.y + 4.0,
            (meta_rect.right() - art_rect.x).max(44.0),
            pill.h - 16.0,
        );
        self.hit_regions.push(HitRegion {
            rect: open_lyrics_zone,
            action: UiAction::ToggleLyricsOverlay,
        });

        // Progress bar hairline along bottom of track info (indeterminate shimmer while buffering!)
        let pb = RectF::new(
            art_rect.x,
            pill.bottom() - 6.0,
            right_cluster_start - art_rect.x - 8.0,
            3.0,
        );
        if pb.w > 40.0 {
            if self.playback_loading {
                r.fill_rounded_rect(pb, 1.5, Color::rgba(255, 255, 255, 0.16));
                let sweep = ((self.spinner_phase * 0.95).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
                let seg_w = (pb.w * 0.28).max(28.0);
                let seg_x = pb.x + (pb.w - seg_w) * sweep;
                r.fill_rounded_rect(
                    RectF::new(seg_x, pb.y, seg_w, pb.h),
                    1.5,
                    Color::rgba(255, 255, 255, 0.90),
                );
            } else if self.smtc.duration_sec > 0.5 {
                r.fill_rounded_rect(pb, 1.5, Color::rgba(255, 255, 255, 0.14));
                let frac = (self.smtc.position_sec / self.smtc.duration_sec).clamp(0.0, 1.0) as f32;
                if frac > 0.002 {
                    r.fill_rounded_rect(
                        RectF::new(pb.x, pb.y, (pb.w * frac).max(3.0), pb.h),
                        1.5,
                        Color::rgba(255, 255, 255, 0.82),
                    );
                }
                let seek_hit = RectF::new(pb.x, pill.bottom() - 13.0, pb.w, 13.0);
                self.hit_regions.push(HitRegion {
                    rect: seek_hit,
                    action: UiAction::ScrubMiniPlayerSeek(pb),
                });
            }
        }

        // Right Cluster: `•••` (Volume/Output), `LyricsBubble` (LyricsBlossom), `QueueList` (Up Next)
        for (i, (ic, active, act)) in [
            (SfIcon::MoreDots, self.show_more_popover, UiAction::ToggleMorePopover),
            (SfIcon::LyricsBubble, self.show_lyrics_overlay, UiAction::ToggleLyricsOverlay),
            (SfIcon::QueueList, self.show_queue_popover, UiAction::ToggleQueuePopover),
        ]
        .into_iter()
        .enumerate()
        {
            let bx = right_cluster_start + (i as f32) * 36.0;
            let br = RectF::new(bx, cy - 16.0, 32.0, 32.0);
            let bhov = br.contains(self.mouse_pos.0, self.mouse_pos.1);
            if active || bhov {
                r.draw_liquid_glass_button(
                    br,
                    16.0,
                    if active {
                        GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                    } else {
                        GlassBtnVariant::Regular
                    },
                    bhov,
                    self.mouse_down,
                    self.mouse_pos,
                    self.glass_off(),
                );
            }
            r.draw_icon(ic, br.inset(7.0, 7.0), Theme::TEXT_PRIMARY);
            self.hit_regions.push(HitRegion {
                rect: br,
                action: act,
            });
        }
    }

    // ========================================================================
    // Content Layer Router (Home, Browse, Radio, Detail, Search, Library, Visualizer, Settings)
    // ========================================================================
    fn draw_main_content_layer(&mut self, r: &mut D2DRenderer, area: RectF) {
        // Nonlinear quintic ease-out page transition (`1 - (1 - t)^4`) across ALL routes!
        let trans_t = self.route_trans.clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - trans_t).powi(4);
        let trans_dy = if self.settings.reduce_motion {
            0.0
        } else {
            (1.0 - ease) * 18.0 * self.route_trans_dir
        };
        let trans_alpha = (0.20 + 0.80 * ease).clamp(0.0, 1.0);
        let pushed_trans = if trans_alpha < 0.996 && !self.settings.reduce_motion {
            r.push_opacity_layer(area, trans_alpha)
        } else {
            false
        };

        if self.route == PageRoute::Visualizer {
            let viz_area = RectF::new(area.x, area.y + trans_dy, area.w, area.h);
            let smtc_snap = self.smtc.clone();
            let exp_specs = self.smtc.stream_specs.clone();
            let exp_badge = self.smtc.audio_quality.clone();
            let prefs = self.audio_prefs.clone();
            let devs = self.devices.clone();
            let art_key = self.smtc.artwork_url.clone();
            let fps = self.measured_fps;
            let is_anim = self.needs_continuous_animation();
            let cap_lbl = self
                .settings
                .viz_fps_cap
                .resolved_label(self.current_monitor_hz, self.settings.viz_custom_fps);
            let show_sp_pill = self.show_advanced_spatial;
            let mpos = self.mouse_pos;
            let mdown = self.mouse_down;
            let red_tr = self.glass_off();
            let snap = self.latest_viz_snap.clone();
            self.viz_state.draw(
                r,
                viz_area,
                &snap,
                &smtc_snap,
                exp_specs.as_ref(),
                exp_badge.as_deref(),
                &prefs,
                &devs,
                art_key.as_deref(),
                fps,
                is_anim,
                &cap_lbl,
                show_sp_pill,
                mpos,
                mdown,
                red_tr,
            );
            if pushed_trans {
                r.pop_layer();
            }
            self.max_scroll_y = 0.0;
            return;
        }

        r.push_clip(area);
        let pad_x = 32.0;
        let start_y = 62.0 - self.scroll_y + trans_dy;
        let inner_w = (area.w - pad_x * 2.0).max(200.0);
        let x0 = area.x + pad_x;

        let end_y = match self.route.clone() {
            PageRoute::Home => self.draw_home_or_browse(r, x0, start_y, inner_w, "主页"),
            PageRoute::Browse => self.draw_home_or_browse(r, x0, start_y, inner_w, "新发现"),
            PageRoute::Radio => {
                let cards = std::mem::take(&mut self.radio_cards);
                let out_y = self.draw_cards_page(r, x0, start_y, inner_w, "广播与精选电台", &cards, false);
                self.radio_cards = cards;
                out_y
            }
            PageRoute::ContainerDetail { .. } => {
                self.draw_container_detail_page(r, x0, start_y, inner_w)
            }
            PageRoute::Search => self.draw_search_page(r, x0, start_y, inner_w),
            PageRoute::LibSongs => self.draw_library_songs_page(r, x0, start_y, inner_w),
            PageRoute::LibDownloads => self.draw_downloads_page(r, x0, start_y, inner_w),
            PageRoute::LibRecent => {
                let c = self.lib_cards.remove("recent").unwrap_or_default();
                let out_y = self.draw_cards_page(r, x0, start_y, inner_w, "最近添加", &c, true);
                if !c.is_empty() {
                    self.lib_cards.insert("recent".to_string(), c);
                }
                out_y
            }
            PageRoute::LibAlbums => {
                let c = self.lib_cards.remove("albums").unwrap_or_default();
                let out_y = self.draw_cards_page(r, x0, start_y, inner_w, "专辑", &c, true);
                if !c.is_empty() {
                    self.lib_cards.insert("albums".to_string(), c);
                }
                out_y
            }
            PageRoute::LibArtists => {
                let c = self.lib_cards.remove("artists").unwrap_or_default();
                let out_y = self.draw_cards_page(r, x0, start_y, inner_w, "艺人", &c, true);
                if !c.is_empty() {
                    self.lib_cards.insert("artists".to_string(), c);
                }
                out_y
            }
            PageRoute::LibPlaylists => {
                let c = self.lib_cards.remove("playlists").unwrap_or_default();
                let out_y = self.draw_cards_page(r, x0, start_y, inner_w, "所有播放列表", &c, true);
                if !c.is_empty() {
                    self.lib_cards.insert("playlists".to_string(), c);
                }
                out_y
            }
            PageRoute::LibForYou => {
                let c = self.lib_cards.remove("foryou").unwrap_or_default();
                let out_y = self.draw_cards_page(r, x0, start_y, inner_w, "专属推荐", &c, true);
                if !c.is_empty() {
                    self.lib_cards.insert("foryou".to_string(), c);
                }
                out_y
            }
            PageRoute::Settings => self.draw_settings_page(r, x0, start_y, inner_w),
            PageRoute::Visualizer => start_y,
        };

        r.pop_clip();
        if pushed_trans {
            r.pop_layer();
        }

        let content_total_h = (end_y - trans_dy + self.scroll_y) + 164.0;
        self.max_scroll_y = (content_total_h - area.h).max(0.0);
        if self.scroll_target_y > self.max_scroll_y {
            self.scroll_target_y = self.max_scroll_y;
        }
    }

    // ========================================================================
    // Album / Playlist Hero + Tracklist Page (1:1 Match with Screenshot `WWDC26`!)
    // ========================================================================
    fn draw_container_detail_page(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        mut y: f32,
        w: f32,
    ) -> f32 {
        if self.detail_loading {
            r.draw_text(
                "正在载入曲目列表...",
                RectF::new(x0, y + 60.0, w, 32.0),
                16.0,
                500,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            return y + 120.0;
        }

        let Some(detail) = self.detail_data.take() else {
            r.draw_text(
                "未能载入内容，请点击右上角刷新重试。",
                RectF::new(x0, y + 60.0, w, 32.0),
                15.0,
                500,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            return y + 120.0;
        };

        // Hero Section: Left 228x228 Rounded Artwork + Right Metadata & Liquid Glass Action Buttons
        let art_sz = 228.0f32.min(w * 0.34).max(164.0);
        let art_rect = RectF::new(x0, y, art_sz, art_sz);

        // Soft ambient shadow beneath hero artwork
        r.fill_rounded_rect(
            RectF::new(art_rect.x + 4.0, art_rect.y + 8.0, art_sz - 8.0, art_sz),
            14.0,
            Color::rgba(0, 0, 0, 0.45),
        );

        let mut drew_hero = false;
        if let Some(ref u) = detail.artwork_url {
            self.request_artwork(r, u, true);
            drew_hero = r.draw_rounded_artwork(u, true, art_rect, 14.0, 1.0);
        }
        if !drew_hero {
            r.fill_rounded_rect(art_rect, 14.0, Theme::BG_ELEVATED);
            r.draw_icon(
                SfIcon::Albums,
                art_rect.inset(art_sz * 0.32, art_sz * 0.32),
                Theme::TEXT_MUTED,
            );
        }
        let hero_is_loading = self.playback_loading
            && (self.playback_loading_container_id.as_deref() == Some(detail.id.as_str())
                || self.playback_loading_track_id.as_deref().is_some_and(|tid| {
                    detail.tracks.iter().any(|t| t.effective_adam_id() == tid)
                }));
        if hero_is_loading {
            r.fill_rounded_rect(art_rect, 14.0, Color::rgba(8, 8, 14, 0.48));
            r.draw_loading_spinner(
                art_rect.center_x(),
                art_rect.center_y(),
                22.0,
                self.spinner_phase,
                Color::rgba(255, 255, 255, 0.96),
            );
        }
        r.stroke_rounded_rect(art_rect, 14.0, Color::rgba(255, 255, 255, 0.12), 1.0);

        let rx = art_rect.right() + 28.0;
        let rw = (x0 + w - rx).max(140.0);

        r.draw_text(
            &detail.title,
            RectF::new(rx, y + 24.0, rw, 36.0),
            26.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            false,
            false,
        );
        r.draw_text(
            &detail.subtitle,
            RectF::new(rx, y + 60.0, rw, 26.0),
            18.5,
            500,
            Theme::TEXT_PRIMARY.with_alpha(0.90),
            TextAlign::Left,
            false,
            false,
        );

        if !detail.description.is_empty() {
            r.draw_text(
                &detail.description,
                RectF::new(rx, y + 106.0, rw.min(540.0), 56.0),
                13.5,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                false,
                true,
            );
        }

        // Hero Action Row: [Shuffle Liquid Glass Circle]  [  ▶ 播放 Prominent Capsule ]  [ + Liquid Glass Circle ]
        let allow_page_hov = self.mouse_pos.1 >= TOP_BAR_GUARD_H;
        let btn_y = art_rect.bottom() - 42.0;
        let shuf_btn = RectF::new(rx, btn_y, 42.0, 42.0);
        let shuf_hov = allow_page_hov && shuf_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            shuf_btn,
            21.0,
            GlassBtnVariant::Regular,
            shuf_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_icon(SfIcon::Shuffle, shuf_btn.inset(11.0, 11.0), Theme::TEXT_PRIMARY);
        self.hit_regions.push(HitRegion {
            rect: shuf_btn,
            action: UiAction::PlayDetailTracks {
                start_index: 0,
                shuffle: true,
            },
        });

        let play_pill = RectF::new(shuf_btn.right() + 12.0, btn_y, 138.0, 42.0);
        let play_hov = allow_page_hov && play_pill.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            play_pill,
            21.0,
            GlassBtnVariant::ProminentLight,
            play_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        if hero_is_loading {
            r.draw_loading_spinner(
                play_pill.x + 36.0,
                play_pill.center_y(),
                8.5,
                self.spinner_phase,
                Theme::HERO_PLAY_FG,
            );
            r.draw_text(
                "加载中",
                RectF::new(play_pill.x + 52.0, play_pill.y, 72.0, play_pill.h),
                14.5,
                700,
                Theme::HERO_PLAY_FG,
                TextAlign::Left,
                true,
                false,
            );
        } else {
            r.draw_icon(
                SfIcon::Play,
                RectF::new(play_pill.x + 36.0, play_pill.y + 13.0, 16.0, 16.0),
                Theme::HERO_PLAY_FG,
            );
            r.draw_text(
                "播放",
                RectF::new(play_pill.x + 56.0, play_pill.y, 60.0, play_pill.h),
                15.0,
                700,
                Theme::HERO_PLAY_FG,
                TextAlign::Left,
                true,
                false,
            );
        }
        self.hit_regions.push(HitRegion {
            rect: play_pill,
            action: UiAction::PlayDetailTracks {
                start_index: 0,
                shuffle: false,
            },
        });

        if !detail.kind.starts_with("library-") {
            let add_btn = RectF::new(play_pill.right() + 12.0, btn_y, 42.0, 42.0);
            let add_hov = allow_page_hov && add_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                add_btn,
                21.0,
                GlassBtnVariant::Regular,
                add_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_icon(SfIcon::Plus, add_btn.inset(11.5, 11.5), Theme::TEXT_PRIMARY);
            self.hit_regions.push(HitRegion {
                rect: add_btn,
                action: UiAction::AddContainerToLibrary {
                    kind: detail.kind.clone(),
                    id: detail.id.clone(),
                },
            });
        }

        y = art_rect.bottom() + 26.0;
        r.draw_line(x0, y, x0 + w, y, Theme::SEPARATOR, 1.0);
        y += 2.0;

        let out_y = self.draw_tracklist_rows(r, x0, y, w, &detail.tracks, TrackListSource::DetailTracks);
        self.detail_data = Some(detail);
        out_y
    }

    #[allow(clippy::too_many_arguments)]
    fn render_track_row_visuals(
        r: &mut D2DRenderer,
        x0: f32,
        ry: f32,
        w: f32,
        track_num: usize,
        tr: &AgentTrackInput,
        is_cur: bool,
        is_row_loading: bool,
        spinner_phase: f32,
        is_playing_now: bool,
        hov_t: f32,
        bars4: [f32; 4],
        dots_hov: bool,
        mouse_down: bool,
        mouse_pos: (f32, f32),
        glass_off: bool,
    ) {
        let row_h = Theme::TRACK_ROW_HEIGHT;
        let artist_col_x = x0 + w * 0.56;
        let dur_col_x = x0 + w - 82.0;
        let row = RectF::new(x0 - 8.0, ry, w + 16.0, row_h);

        if is_cur || is_row_loading {
            r.fill_rounded_rect(row, 9.0, Theme::ROW_ACTIVE);
        } else if hov_t > 0.005 {
            r.fill_rounded_rect(
                row,
                9.0,
                Theme::ROW_HOVER.with_alpha(Theme::ROW_HOVER.a * hov_t),
            );
        }

        let thumb = RectF::new(x0, ry + 7.0, 42.0, 42.0);
        let mut drew_art = false;
        if let Some(ref u) = tr.artwork_url {
            drew_art = r.draw_rounded_artwork(u, false, thumb, 6.0, 1.0);
        }
        if !drew_art {
            r.fill_rounded_rect(thumb, 6.0, Theme::BG_ELEVATED);
            r.draw_mono_text(
                &format!("{track_num}"),
                thumb,
                12.0,
                500,
                Theme::TEXT_MUTED,
                TextAlign::Center,
                true,
            );
        }
        if is_row_loading {
            r.fill_rounded_rect(thumb, 6.0, Color::rgba(6, 6, 10, 0.62));
            r.draw_loading_spinner(
                thumb.center_x(),
                thumb.center_y(),
                10.0,
                spinner_phase,
                Color::rgba(255, 255, 255, 0.95),
            );
        } else if is_cur && is_playing_now {
            r.fill_rounded_rect(thumb, 6.0, Color::rgba(0, 0, 0, 0.48));
            r.draw_live_equalizer_bars(thumb.inset(9.5, 10.0), Theme::ACCENT_RED, &bars4, false);
        } else if is_cur {
            r.fill_rounded_rect(thumb, 6.0, Color::rgba(0, 0, 0, 0.44));
            r.draw_icon(SfIcon::Play, thumb.inset(11.0, 11.0), Theme::ACCENT_RED);
        } else if hov_t > 0.02 {
            r.fill_rounded_rect(thumb, 6.0, Color::rgba(0, 0, 0, 0.44 * hov_t));
            r.draw_icon(
                SfIcon::Play,
                thumb.inset(11.0, 11.0),
                Theme::TEXT_PRIMARY.with_alpha(hov_t),
            );
        }

        let title_w = (artist_col_x - (thumb.right() + 14.0) - 16.0).max(80.0);
        r.draw_text(
            &tr.name,
            RectF::new(thumb.right() + 14.0, ry, title_w, row_h),
            14.5,
            if is_cur || is_row_loading { 600 } else { 400 },
            if is_cur || is_row_loading {
                Theme::ACCENT_RED
            } else {
                Theme::TEXT_PRIMARY
            },
            TextAlign::Left,
            true,
            false,
        );

        let artist_w = (dur_col_x - artist_col_x - 12.0).max(60.0);
        r.draw_text(
            &tr.artist_name,
            RectF::new(artist_col_x, ry, artist_w, row_h),
            13.8,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );

        r.draw_mono_text(
            &format_dur(tr.duration_sec),
            RectF::new(dur_col_x, ry, 46.0, row_h),
            13.2,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Right,
            true,
        );

        let dots = RectF::new(x0 + w - 28.0, ry + 14.0, 28.0, 28.0);
        if dots_hov {
            r.draw_liquid_glass_button(
                dots,
                14.0,
                GlassBtnVariant::Regular,
                true,
                mouse_down,
                mouse_pos,
                glass_off,
            );
        }
        r.draw_icon(
            SfIcon::MoreDots,
            dots.inset(6.5, 6.5),
            if dots_hov {
                Theme::TEXT_PRIMARY
            } else {
                Theme::TEXT_SECONDARY
            },
        );

        r.draw_line(
            thumb.right() + 14.0,
            ry + row_h,
            x0 + w,
            ry + row_h,
            Theme::SEPARATOR,
            1.0,
        );
    }

    fn draw_tracklist_rows(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        mut y: f32,
        w: f32,
        tracks: &[AgentTrackInput],
        source: TrackListSource,
    ) -> f32 {
        let win_h = r.logical_height();
        let row_h = Theme::TRACK_ROW_HEIGHT;
        let pre_top = if self.scroll_vel_y < -40.0 { -960.0 } else { -360.0 };
        let pre_bot = if self.scroll_vel_y > 40.0 {
            win_h + 1080.0
        } else {
            win_h + 560.0
        };
        let bars4 = [
            self.eq_bars_smooth[0],
            self.eq_bars_smooth[2],
            self.eq_bars_smooth[3],
            self.eq_bars_smooth[5],
        ];
        let is_playing_now = self.smtc.status == "Playing";
        let allow_page_hov = self.mouse_pos.1 >= TOP_BAR_GUARD_H;
        let glass_off = self.glass_off();

        if matches!(source, TrackListSource::DetailTracks) {
            if let Some(diff) = self.playlist_diff_anim.take() {
                let raw_t =
                    (diff.started_at.elapsed().as_secs_f32() / diff.duration_sec).clamp(0.0, 1.0);
                let ease_move = raw_t * raw_t * raw_t * (raw_t * (raw_t * 6.0 - 15.0) + 10.0);
                let ease_in = raw_t * raw_t * raw_t;
                let ease_out = 1.0 - (1.0 - raw_t).powi(3);

                // Pass 1: Removed rows (Gaussian blur-out fade)
                for (e_idx, ent) in diff.entries.iter().enumerate() {
                    if let PlaylistDiffKind::Removed { old_idx } = ent.kind {
                        let ry = y + (old_idx as f32) * row_h;
                        if ry + row_h < -32.0 || ry > win_h + 32.0 {
                            continue;
                        }
                        let alpha = (1.0 - ease_out).clamp(0.0, 1.0);
                        let sigma = ease_in * 12.0;
                        if let Some(&slot) = diff.bake_slots.get(&e_idx) {
                            r.draw_row_fx_blurred(slot, x0 - 8.0, ry, sigma, alpha);
                        }
                    }
                }

                // Pass 2: Added rows (Gaussian blur-in fade)
                for (e_idx, ent) in diff.entries.iter().enumerate() {
                    if let PlaylistDiffKind::Added { new_idx } = ent.kind {
                        let ry = y + (new_idx as f32) * row_h;
                        if ry + row_h < -32.0 || ry > win_h + 32.0 {
                            continue;
                        }
                        let alpha = ease_out;
                        let sigma = (1.0 - ease_out) * 12.0;
                        if let Some(&slot) = diff.bake_slots.get(&e_idx) {
                            r.draw_row_fx_blurred(slot, x0 - 8.0, ry, sigma, alpha);
                        }
                    }
                }

                // Pass 3: Retained / reordered rows (zero blur, nonlinear slide to new index!)
                for ent in &diff.entries {
                    if let PlaylistDiffKind::KeptOrMoved { old_idx, new_idx } = ent.kind {
                        let Some(tr) = diff.new_tracks.get(new_idx) else {
                            continue;
                        };
                        let cur_idx_f =
                            (old_idx as f32) + ((new_idx as f32) - (old_idx as f32)) * ease_move;
                        let ry = y + cur_idx_f * row_h;
                        if ry + row_h >= pre_top && ry <= pre_bot {
                            if let Some(ref u) = tr.artwork_url {
                                self.request_artwork(r, u, false);
                            }
                        }
                        if ry + row_h < 0.0 || ry > win_h {
                            continue;
                        }
                        let eff_id = tr.effective_adam_id();
                        let is_cur = self
                            .smtc
                            .adam_id
                            .as_deref()
                            .is_some_and(|a| !a.is_empty() && a == eff_id);
                        let is_row_loading = self.playback_loading
                            && self
                                .playback_loading_track_id
                                .as_deref()
                                .is_some_and(|tid| !tid.is_empty() && tid == eff_id);
                        let row = RectF::new(x0 - 8.0, ry, w + 16.0, row_h);
                        let dots = RectF::new(x0 + w - 28.0, ry + 14.0, 28.0, 28.0);
                        let is_hov =
                            allow_page_hov && row.contains(self.mouse_pos.0, self.mouse_pos.1);
                        let dots_hov =
                            allow_page_hov && dots.contains(self.mouse_pos.0, self.mouse_pos.1);
                        let hov_key = fnv1a_hover_key("tr_det", &ent.track_key);
                        let hov_t = self.eval_hover_alpha(hov_key, is_hov);
                        Self::render_track_row_visuals(
                            r,
                            x0,
                            ry,
                            w,
                            new_idx + 1,
                            tr,
                            is_cur,
                            is_row_loading,
                            self.spinner_phase,
                            is_playing_now,
                            hov_t,
                            bars4,
                            dots_hov,
                            self.mouse_down,
                            self.mouse_pos,
                            glass_off,
                        );
                        self.hit_regions.push(HitRegion {
                            rect: RectF::new(row.x, row.y, row.w - 36.0, row.h),
                            action: UiAction::PlayTrackList {
                                source,
                                index: new_idx,
                            },
                        });
                        self.hit_regions.push(HitRegion {
                            rect: dots,
                            action: UiAction::AddSongToLibrary(eff_id),
                        });
                    }
                }

                let old_h = (diff.old_tracks.len() as f32) * row_h;
                let new_h = (diff.new_tracks.len() as f32) * row_h;
                self.playlist_diff_anim = Some(diff);
                return y + old_h + (new_h - old_h) * ease_move;
            }
        }

        for (idx, tr) in tracks.iter().enumerate() {
            let ry = y;
            y += row_h;

            // Directional velocity-aware cover prefetch BEFORE frustum culling!
            if ry + row_h >= pre_top && ry <= pre_bot {
                if let Some(ref u) = tr.artwork_url {
                    self.request_artwork(r, u, false);
                }
            }

            // Frustum cull offscreen track rows!
            if ry + row_h < 0.0 || ry > win_h {
                continue;
            }

            let row = RectF::new(x0 - 8.0, ry, w + 16.0, row_h);
            let eff_id = tr.effective_adam_id();
            let is_cur = self
                .smtc
                .adam_id
                .as_deref()
                .is_some_and(|a| !a.is_empty() && a == eff_id);
            let is_row_loading = self.playback_loading
                && self
                    .playback_loading_track_id
                    .as_deref()
                    .is_some_and(|tid| !tid.is_empty() && tid == eff_id);
            let is_hov = allow_page_hov && row.contains(self.mouse_pos.0, self.mouse_pos.1);
            let dots = RectF::new(x0 + w - 28.0, ry + 14.0, 28.0, 28.0);
            let dots_hov = allow_page_hov && dots.contains(self.mouse_pos.0, self.mouse_pos.1);
            let hov_key = fnv1a_hover_key(
                match source {
                    TrackListSource::HomeSongs => "tr_home",
                    TrackListSource::SearchSongs => "tr_srch",
                    TrackListSource::LibrarySongs => "tr_lib",
                    TrackListSource::DetailTracks => "tr_det",
                },
                if eff_id.is_empty() { &tr.name } else { &eff_id },
            ) ^ (idx as u64);
            let hov_t = self.eval_hover_alpha(hov_key, is_hov);

            Self::render_track_row_visuals(
                r,
                x0,
                ry,
                w,
                idx + 1,
                tr,
                is_cur,
                is_row_loading,
                self.spinner_phase,
                is_playing_now,
                hov_t,
                bars4,
                dots_hov,
                self.mouse_down,
                self.mouse_pos,
                glass_off,
            );

            let play_row_rect = RectF::new(row.x, row.y, row.w - 36.0, row.h);
            self.hit_regions.push(HitRegion {
                rect: play_row_rect,
                action: UiAction::PlayTrackList {
                    source,
                    index: idx,
                },
            });
            self.hit_regions.push(HitRegion {
                rect: dots,
                action: UiAction::AddSongToLibrary(eff_id),
            });
        }
        y
    }

    // ========================================================================
    // Home / Browse / Cards / Search / Library / Downloads / Settings Pages
    // ========================================================================
    fn draw_home_or_browse(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        mut y: f32,
        w: f32,
        heading: &str,
    ) -> f32 {
        r.draw_text(
            heading,
            RectF::new(x0, y, w, 38.0),
            28.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        y += 48.0;

        if self.home_albums.is_empty() && self.home_playlists.is_empty() {
            r.draw_text(
                "正在获取 Apple Music 精选内容...",
                RectF::new(x0, y + 20.0, w, 28.0),
                15.0,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            return y + 80.0;
        }

        if !self.home_playlists.is_empty() {
            r.draw_text(
                "精选歌单",
                RectF::new(x0, y, w, 28.0),
                19.5,
                700,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            y += 34.0;
            let pls = std::mem::take(&mut self.home_playlists);
            y = self.draw_media_card_grid(r, x0, y, w, &pls[..pls.len().min(10)]);
            self.home_playlists = pls;
            y += 22.0;
        }

        if !self.home_albums.is_empty() {
            r.draw_text(
                "热门专辑",
                RectF::new(x0, y, w, 28.0),
                19.5,
                700,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            y += 34.0;
            let albs = std::mem::take(&mut self.home_albums);
            y = self.draw_media_card_grid(r, x0, y, w, &albs[..albs.len().min(10)]);
            self.home_albums = albs;
            y += 26.0;
        }

        if !self.home_songs.is_empty() {
            r.draw_text(
                "热门单曲",
                RectF::new(x0, y, w, 28.0),
                19.5,
                700,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            y += 34.0;
            let songs = std::mem::take(&mut self.home_songs);
            y = self.draw_tracklist_rows(r, x0, y, w, &songs, TrackListSource::HomeSongs);
            self.home_songs = songs;
        }

        y
    }

    fn draw_media_card_grid(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        mut y: f32,
        w: f32,
        items: &[MediaCardItem],
    ) -> f32 {
        let gap = 18.0;
        let cols = ((w + gap) / 184.0).floor().clamp(2.0, 7.0) as usize;
        let card_w = (w - gap * ((cols - 1) as f32)) / (cols as f32);
        let card_h = card_w + 48.0;
        let win_h = r.logical_height();
        let pre_top = if self.scroll_vel_y < -40.0 { -960.0 } else { -360.0 };
        let pre_bot = if self.scroll_vel_y > 40.0 {
            win_h + 1080.0
        } else {
            win_h + 560.0
        };

        for (i, item) in items.iter().enumerate() {
            let col = i % cols;
            let row_idx = i / cols;
            let cx = x0 + (col as f32) * (card_w + gap);
            let cy = y + (row_idx as f32) * (card_h + 14.0);

            if cy + card_h >= pre_top && cy <= pre_bot {
                if let Some(ref u) = item.artwork_url {
                    self.request_artwork(r, u, false);
                }
            }

            if cy + card_h < 0.0 || cy > win_h {
                continue;
            }

            let art_r = RectF::new(cx, cy, card_w, card_w);
            let full_r = RectF::new(cx, cy, card_w, card_h);
            let is_hov = self.mouse_pos.1 >= TOP_BAR_GUARD_H
                && full_r.contains(self.mouse_pos.0, self.mouse_pos.1);
            let hov_t = self.eval_hover_alpha(
                fnv1a_hover_key("card", &item.id) ^ ((i as u64) << 32),
                is_hov,
            );
            let card_loading = self.playback_loading
                && self.playback_loading_container_id.as_deref() == Some(item.id.as_str());

            if hov_t > 0.008 {
                r.fill_rounded_rect(
                    full_r.inset(-6.0, -6.0),
                    14.0,
                    Color::rgba(255, 255, 255, 0.052 * hov_t),
                );
            }

            let mut drew = false;
            if let Some(ref u) = item.artwork_url {
                drew = r.draw_rounded_artwork(u, false, art_r, 11.0, 1.0);
            }
            if !drew {
                r.fill_rounded_rect(art_r, 11.0, Theme::BG_ELEVATED);
                r.draw_icon(
                    SfIcon::Albums,
                    art_r.inset(card_w * 0.34, card_w * 0.34),
                    Theme::TEXT_MUTED,
                );
            }
            if card_loading {
                r.fill_rounded_rect(art_r, 11.0, Color::rgba(8, 8, 14, 0.54));
                r.draw_loading_spinner(
                    art_r.center_x(),
                    art_r.center_y(),
                    18.0,
                    self.spinner_phase,
                    Color::rgba(255, 255, 255, 0.96),
                );
            }
            r.stroke_rounded_rect(
                art_r,
                11.0,
                Color::rgba(255, 255, 255, 0.08 + 0.18 * hov_t),
                1.0,
            );

            // Liquid Glass Play button overlay in bottom-right corner of artwork (smooth fade-in/out!)
            let play_circle = RectF::new(art_r.right() - 40.0, art_r.bottom() - 40.0, 32.0, 32.0);
            if hov_t > 0.02 && !card_loading {
                let pc_hov = play_circle.contains(self.mouse_pos.0, self.mouse_pos.1);
                let pushed_pc = if hov_t < 0.98 {
                    r.push_opacity_layer(play_circle.inset(-4.0, -4.0), hov_t)
                } else {
                    false
                };
                r.draw_liquid_glass_button(
                    play_circle,
                    16.0,
                    GlassBtnVariant::Regular,
                    pc_hov,
                    self.mouse_down,
                    self.mouse_pos,
                    self.glass_off(),
                );
                r.draw_icon(SfIcon::Play, play_circle.inset(8.0, 8.0), Theme::TEXT_PRIMARY);
                if pushed_pc {
                    r.pop_layer();
                }
            }

            r.draw_text(
                &item.title,
                RectF::new(cx, art_r.bottom() + 7.0, card_w, 19.0),
                13.5,
                600,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            r.draw_text(
                &item.subtitle,
                RectF::new(cx, art_r.bottom() + 26.0, card_w, 17.0),
                12.2,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );

            self.hit_regions.push(HitRegion {
                rect: full_r,
                action: UiAction::Navigate(PageRoute::ContainerDetail {
                    kind: item.kind.clone(),
                    id: item.id.clone(),
                    title_hint: item.title.clone(),
                }),
            });
            self.hit_regions.push(HitRegion {
                rect: play_circle,
                action: UiAction::PlayContainer {
                    kind: item.kind.clone(),
                    id: item.id.clone(),
                    shuffle: false,
                },
            });
        }

        let rows = (items.len() + cols - 1) / cols;
        y += (rows as f32) * (card_h + 14.0);
        y
    }

    fn draw_cards_page(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        mut y: f32,
        w: f32,
        heading: &str,
        cards: &[MediaCardItem],
        requires_token: bool,
    ) -> f32 {
        r.draw_text(
            heading,
            RectF::new(x0, y, w, 38.0),
            28.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        y += 48.0;

        if requires_token && self.settings.user_token.is_empty() {
            return self.draw_connect_account_prompt(r, x0, y, w);
        }

        if cards.is_empty() {
            r.draw_text(
                "暂无内容或正在加载中...",
                RectF::new(x0, y + 20.0, w, 28.0),
                14.5,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            return y + 80.0;
        }

        self.draw_media_card_grid(r, x0, y, w, cards)
    }

    fn draw_library_songs_page(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        mut y: f32,
        w: f32,
    ) -> f32 {
        r.draw_text(
            "资料库歌曲",
            RectF::new(x0, y, w, 38.0),
            28.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        y += 48.0;

        if self.settings.user_token.is_empty() {
            return self.draw_connect_account_prompt(r, x0, y, w);
        }

        if self.lib_songs.is_empty() {
            r.draw_text(
                "正在同步资料库歌曲...",
                RectF::new(x0, y + 20.0, w, 28.0),
                14.5,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            return y + 80.0;
        }

        let songs = std::mem::take(&mut self.lib_songs);
        let out_y = self.draw_tracklist_rows(r, x0, y, w, &songs, TrackListSource::LibrarySongs);
        self.lib_songs = songs;
        out_y
    }

    fn draw_connect_account_prompt(
        &mut self,
        r: &mut D2DRenderer,
        x0: f32,
        y: f32,
        w: f32,
    ) -> f32 {
        let card = RectF::new(x0, y + 12.0, w.min(560.0), 140.0);
        r.fill_rounded_rect(card, 14.0, Theme::BG_ELEVATED);
        r.stroke_rounded_rect(card, 14.0, Theme::SEPARATOR, 1.0);
        r.draw_text(
            "连接个人资料库",
            RectF::new(card.x + 22.0, card.y + 18.0, card.w - 44.0, 24.0),
            16.5,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            "播放已自动读取桌面版 Apple Music 登录状态；如需加载云端歌单与收藏，可直接按 Ctrl+V 或点击下方粘贴 Media-User-Token。",
            RectF::new(card.x + 22.0, card.y + 46.0, card.w - 44.0, 40.0),
            12.8,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            false,
            true,
        );
        let paste_btn = RectF::new(card.x + 22.0, card.bottom() - 40.0, 146.0, 30.0);
        let phov = paste_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            paste_btn,
            15.0,
            GlassBtnVariant::Tinted(Theme::ACCENT_RED),
            phov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            "直接粘贴 Token",
            paste_btn,
            13.0,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: paste_btn,
            action: UiAction::PasteUserToken,
        });

        let btn = RectF::new(paste_btn.right() + 10.0, card.bottom() - 40.0, 112.0, 30.0);
        let bhov = btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            btn,
            15.0,
            GlassBtnVariant::Regular,
            bhov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            "前往设置",
            btn,
            13.0,
            500,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: btn,
            action: UiAction::Navigate(PageRoute::Settings),
        });
        card.bottom() + 24.0
    }

    fn draw_search_page(&mut self, r: &mut D2DRenderer, x0: f32, mut y: f32, w: f32) -> f32 {
        let hdr = if self.search_query.trim().is_empty() {
            "搜索 Apple Music".to_string()
        } else {
            format!("“{}” 的搜索结果", self.search_query.trim())
        };
        r.draw_text(
            &hdr,
            RectF::new(x0, y, w, 36.0),
            26.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        y += 44.0;

        // Liquid Glass Filter Pills: 热门结果 / 歌曲 / 专辑 / 播放列表
        let filters = ["热门结果", "歌曲", "专辑", "播放列表"];
        let mut fx = x0;
        for (i, &fl) in filters.iter().enumerate() {
            let sel = self.search_filter == (i as u8);
            let pill = RectF::new(fx, y, 84.0, 30.0);
            let hov = pill.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                pill,
                15.0,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                fl,
                pill,
                12.8,
                if sel { 600 } else { 400 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: pill,
                action: UiAction::SetSearchFilter(i as u8),
            });
            fx += 92.0;
        }
        y += 44.0;

        if self.search_query.trim().is_empty() {
            r.draw_text(
                "在左侧搜索栏输入歌曲、艺人或专辑名称（支持中文输入法与 Ctrl+V 粘贴）。",
                RectF::new(x0, y + 12.0, w, 26.0),
                14.0,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            return y + 60.0;
        }

        if (self.search_filter == 0 || self.search_filter == 1) && !self.search_songs.is_empty() {
            r.draw_text(
                "歌曲",
                RectF::new(x0, y, w, 26.0),
                18.0,
                700,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            y += 32.0;
            let s = std::mem::take(&mut self.search_songs);
            let lim = if self.search_filter == 0 {
                s.len().min(6)
            } else {
                s.len()
            };
            y = self.draw_tracklist_rows(r, x0, y, w, &s[..lim], TrackListSource::SearchSongs);
            self.search_songs = s;
            y += 22.0;
        }

        if (self.search_filter == 0 || self.search_filter == 2) && !self.search_albums.is_empty() {
            r.draw_text(
                "专辑",
                RectF::new(x0, y, w, 26.0),
                18.0,
                700,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            y += 32.0;
            let a = std::mem::take(&mut self.search_albums);
            y = self.draw_media_card_grid(r, x0, y, w, &a);
            self.search_albums = a;
            y += 22.0;
        }

        if (self.search_filter == 0 || self.search_filter == 3) && !self.search_playlists.is_empty()
        {
            r.draw_text(
                "播放列表",
                RectF::new(x0, y, w, 26.0),
                18.0,
                700,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            y += 32.0;
            let p = std::mem::take(&mut self.search_playlists);
            y = self.draw_media_card_grid(r, x0, y, w, &p);
            self.search_playlists = p;
        }

        y
    }

    fn draw_downloads_page(&mut self, r: &mut D2DRenderer, x0: f32, mut y: f32, w: f32) -> f32 {
        r.draw_text(
            "已下载",
            RectF::new(x0, y, 220.0, 38.0),
            28.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );

        let open_btn = RectF::new(x0 + w - 118.0, y + 4.0, 118.0, 30.0);
        let open_hov = open_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            open_btn,
            15.0,
            GlassBtnVariant::Regular,
            open_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            "打开本地目录",
            open_btn,
            12.5,
            500,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: open_btn,
            action: UiAction::OpenDownloadsFolder,
        });

        let pause_btn = RectF::new(open_btn.x - 114.0, y + 4.0, 104.0, 30.0);
        let pause_hov = pause_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            pause_btn,
            15.0,
            if self.downloads_paused {
                GlassBtnVariant::Tinted(Theme::SIGNAL_AMBER)
            } else {
                GlassBtnVariant::Regular
            },
            pause_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            if self.downloads_paused {
                "恢复下载队列"
            } else {
                "暂停下载队列"
            },
            pause_btn,
            12.5,
            500,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: pause_btn,
            action: UiAction::TogglePauseDownloads(!self.downloads_paused),
        });

        y += 46.0;
        let mb = (self.downloads_info.total_bytes as f64) / (1024.0 * 1024.0);
        let summary = format!(
            "共 {} 项离线音频缓存 · 占用 {:.1} MB · 路径：{}",
            self.downloads_info.items.len(),
            mb,
            if self.downloads_info.media_folder.is_empty() {
                "默认媒体目录"
            } else {
                &self.downloads_info.media_folder
            }
        );
        r.draw_text(
            &summary,
            RectF::new(x0, y, w, 22.0),
            12.8,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        y += 32.0;

        let items = self.downloads_info.items.clone();
        if items.is_empty() {
            r.draw_text(
                "本地目录暂无已缓存的离线音频文件。",
                RectF::new(x0, y + 20.0, w, 26.0),
                14.0,
                400,
                Theme::TEXT_MUTED,
                TextAlign::Left,
                true,
                false,
            );
            return y + 70.0;
        }

        for item in items {
            let row = RectF::new(x0, y, w, 54.0);
            r.fill_rounded_rect(row, 10.0, Theme::BG_ELEVATED);
            r.draw_text(
                &item.title,
                RectF::new(row.x + 16.0, row.y + 7.0, row.w - 230.0, 20.0),
                14.0,
                600,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            let meta = format!(
                "{} · {} · {:.1} MB",
                item.artist,
                item.format,
                (item.size_bytes as f64) / (1024.0 * 1024.0)
            );
            r.draw_text(
                &meta,
                RectF::new(row.x + 16.0, row.y + 28.0, row.w - 230.0, 18.0),
                12.0,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );

            let rev_btn = RectF::new(row.right() - 106.0, row.y + 13.0, 44.0, 28.0);
            let del_btn = RectF::new(row.right() - 54.0, row.y + 13.0, 40.0, 28.0);
            let rev_hov = rev_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
            let del_hov = del_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                rev_btn,
                7.0,
                GlassBtnVariant::Regular,
                rev_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_icon(SfIcon::Folder, rev_btn.inset(6.0, 6.0), Theme::TEXT_PRIMARY);
            r.draw_liquid_glass_button(
                del_btn,
                7.0,
                GlassBtnVariant::Tinted(Theme::ACCENT_RED),
                del_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_icon(SfIcon::Trash, del_btn.inset(6.0, 6.0), Theme::TEXT_PRIMARY);

            self.hit_regions.push(HitRegion {
                rect: rev_btn,
                action: UiAction::RevealDownload(item.path.clone()),
            });
            self.hit_regions.push(HitRegion {
                rect: del_btn,
                action: UiAction::DeleteDownload(item.path),
            });
            y += 62.0;
        }
        y
    }

    // ========================================================================
    // Settings Page (`settings.md`, Liquid Glass Hover Buttons, Window Corner Radius, Hidden Spatial)
    // ========================================================================
    fn draw_settings_page(&mut self, r: &mut D2DRenderer, x0: f32, mut y: f32, w: f32) -> f32 {
        let cw = w.min(760.0);
        r.draw_text(
            "设置",
            RectF::new(x0, y, cw, 38.0),
            28.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        y += 48.0;

        // --- Group 1 (TOP): Apple Music 账号与地区 ---
        r.draw_text(
            "账号与地区",
            RectF::new(x0 + 4.0, y, cw, 22.0),
            13.0,
            600,
            Theme::TEXT_MUTED,
            TextAlign::Left,
            true,
            false,
        );
        y += 26.0;

        let g_acct = RectF::new(x0, y, cw, 212.0);
        r.fill_rounded_rect(g_acct, 14.0, Theme::BG_ELEVATED);
        r.stroke_rounded_rect(g_acct, 14.0, Theme::SEPARATOR, 1.0);

        // Row 1: Official Apple Music Desktop Client Login Status
        r.draw_text(
            "原装客户端账号",
            RectF::new(x0 + 18.0, y + 14.0, 136.0, 26.0),
            14.0,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        let off_desc = if self.official_account.signed_in {
            let mut parts = Vec::new();
            parts.push("已读取登录".to_string());
            if let Some(ref n) = self.official_account.display_name {
                parts.push(n.clone());
            }
            if let Some(ref sf) = self.official_account.storefront {
                parts.push(format!("{} 区", sf.to_uppercase()));
            }
            if self.official_account.subscription_active {
                parts.push("订阅有效（可直接播放）".to_string());
            }
            parts.join(" · ")
        } else {
            "未检测到桌面版 Apple Music 登录记录".to_string()
        };
        r.draw_text(
            &off_desc,
            RectF::new(x0 + 154.0, y + 14.0, cw - 284.0, 26.0),
            12.5,
            500,
            if self.official_account.signed_in {
                Theme::SIGNAL_GREEN
            } else {
                Theme::TEXT_SECONDARY
            },
            TextAlign::Left,
            true,
            false,
        );
        let sync_btn = RectF::new(x0 + cw - 118.0, y + 13.0, 100.0, 28.0);
        let sync_hov = sync_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            sync_btn,
            8.5,
            GlassBtnVariant::Regular,
            sync_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            "同步原装端",
            sync_btn,
            12.0,
            if sync_hov { 600 } else { 500 },
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: sync_btn,
            action: UiAction::SyncOfficialClientAccount,
        });
        r.draw_line(x0 + 18.0, y + 52.0, x0 + cw - 18.0, y + 52.0, Theme::SEPARATOR, 1.0);

        // Row 2: Storefront Selector
        r.draw_text(
            "内容地区",
            RectF::new(x0 + 18.0, y + 64.0, 110.0, 26.0),
            14.0,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        let regions = ["cn", "hk", "tw", "jp", "kr", "us", "gb", "sg"];
        let mut sx = x0 + cw - 18.0 - (45.0 * (regions.len() as f32) + 6.0 * ((regions.len() - 1) as f32));
        for &code in &regions {
            let sel = self.settings.storefront == code;
            let sr = RectF::new(sx, y + 64.0, 45.0, 26.0);
            let shov = sr.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                sr,
                7.5,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                shov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_mono_text(
                &code.to_uppercase(),
                sr,
                11.6,
                600,
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
            );
            self.hit_regions.push(HitRegion {
                rect: sr,
                action: UiAction::SetStorefront(code.to_string()),
            });
            sx += 51.0;
        }
        r.draw_line(x0 + 18.0, y + 102.0, x0 + cw - 18.0, y + 102.0, Theme::SEPARATOR, 1.0);

        // Row 3: Personal Library Token Input Box + Liquid Glass Action Buttons
        let tok_status = if self.settings.user_token.is_empty() {
            "个人资料库授权 (Media-User-Token)：未配置".to_string()
        } else {
            format!(
                "个人资料库授权：已持久保存 ({} 字符)",
                self.settings.user_token.len()
            )
        };
        r.draw_text(
            &tok_status,
            RectF::new(x0 + 18.0, y + 112.0, cw - 330.0, 22.0),
            13.6,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );

        let mut bx = x0 + cw - 18.0;
        for (lbl, bw, act) in [
            ("清除", 52.0f32, UiAction::ClearUserToken),
            ("验证", 52.0, UiAction::VerifyUserToken),
            ("打开网页端", 84.0, UiAction::OpenWebAppleMusic),
            ("粘贴保存", 84.0, UiAction::PasteUserToken),
        ] {
            bx -= bw;
            let br = RectF::new(bx, y + 110.0, bw, 28.0);
            let bhov = br.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                br,
                8.5,
                if lbl == "粘贴保存" {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                bhov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                lbl,
                br,
                12.0,
                if bhov || lbl == "粘贴保存" { 600 } else { 500 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: br,
                action: act,
            });
            bx -= 7.0;
        }

        // Interactive Clickable Token Input Box (hover highlight + click-to-Ctrl+V!)
        let tbox = RectF::new(x0 + 18.0, y + 148.0, cw - 36.0, 36.0);
        self.token_box_rect = tbox;
        let tbox_hov = tbox.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.fill_rounded_rect(
            tbox,
            8.5,
            if tbox_hov {
                Color::rgba(22, 22, 28, 0.92)
            } else {
                Color::rgba(14, 14, 18, 0.85)
            },
        );
        r.stroke_rounded_rect(
            tbox,
            8.5,
            if self.token_box_focused {
                Theme::ACCENT_RED
            } else if tbox_hov {
                Color::rgba(255, 255, 255, 0.34)
            } else if !self.settings.user_token.is_empty() {
                Theme::SIGNAL_GREEN.with_alpha(0.55)
            } else {
                Theme::SEPARATOR
            },
            if self.token_box_focused || tbox_hov { 1.4 } else { 1.0 },
        );
        let box_text = if !self.settings.user_token.is_empty() {
            let masked = am_api::mask_user_token_preview(&self.settings.user_token);
            if self.token_box_focused {
                format!("{masked}   (按 Ctrl+V 可覆盖更新)")
            } else {
                format!("已保存：{masked}   (重启后自动保留 · 点击可按 Ctrl+V 更新)")
            }
        } else if self.token_box_focused {
            "请按 Ctrl+V 粘贴 Media-User-Token 或完整 Cookie 字符串...".to_string()
        } else {
            "点击此处按 Ctrl+V 粘贴 Media-User-Token（用于同步云端歌单与收藏专辑）".to_string()
        };
        r.draw_mono_text(
            &box_text,
            tbox.inset(12.0, 0.0),
            12.0,
            if self.settings.user_token.is_empty() { 400 } else { 500 },
            if !self.settings.user_token.is_empty() {
                Theme::SIGNAL_GREEN
            } else if self.token_box_focused || tbox_hov {
                Theme::TEXT_PRIMARY
            } else {
                Theme::TEXT_SECONDARY
            },
            TextAlign::Left,
            true,
        );
        self.hit_regions.push(HitRegion {
            rect: tbox,
            action: UiAction::FocusTokenBox,
        });

        if let Some(ref ar) = self.auth_result {
            r.draw_text(
                &ar.message,
                RectF::new(x0 + 18.0, y + 188.0, cw - 36.0, 18.0),
                11.4,
                400,
                if ar.user_token_ok {
                    Theme::SIGNAL_GREEN
                } else {
                    Theme::TEXT_SECONDARY
                },
                TextAlign::Left,
                true,
                false,
            );
        }

        y += 232.0;

        // --- Group 2: 音频与播放 (Restrained phrasing; Spatial/Atmos hidden by default below!) ---
        r.draw_text(
            "音频与播放",
            RectF::new(x0 + 4.0, y, cw, 22.0),
            13.0,
            600,
            Theme::TEXT_MUTED,
            TextAlign::Left,
            true,
            false,
        );
        y += 26.0;

        let g_aud = RectF::new(x0, y, cw, 212.0);
        r.fill_rounded_rect(g_aud, 14.0, Theme::BG_ELEVATED);
        r.stroke_rounded_rect(g_aud, 14.0, Theme::SEPARATOR, 1.0);

        // Row 1: Streaming Quality
        r.draw_text(
            "音频质量",
            RectF::new(x0 + 18.0, y + 16.0, 160.0, 26.0),
            14.2,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        let q_opts = [
            ("Hi-Res 无损 (24/192)", true, 20i64, 146.0f32),
            ("无损 (24/48)", true, 15, 108.0),
            ("高质量 AAC", false, 10, 98.0),
        ];
        let mut qx = x0 + cw - 18.0 - (146.0 + 108.0 + 98.0 + 16.0);
        for &(lbl, loss, qv, bw) in &q_opts {
            let sel = if !loss {
                !self.audio_prefs.lossless_enabled
            } else {
                self.audio_prefs.lossless_enabled && self.audio_prefs.streaming_audio_quality == qv
            };
            let br = RectF::new(qx, y + 15.0, bw, 28.0);
            let bhov = br.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                br,
                8.5,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                bhov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                lbl,
                br,
                12.0,
                if sel || bhov { 600 } else { 400 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: br,
                action: UiAction::SetStreamingQuality {
                    lossless: loss,
                    quality: qv,
                },
            });
            qx += bw + 8.0;
        }
        r.draw_line(x0 + 18.0, y + 56.0, x0 + cw - 18.0, y + 56.0, Theme::SEPARATOR, 1.0);

        // Row 2: Output Audio Device
        let def_dev = self
            .devices
            .iter()
            .find(|d| d.is_default)
            .map(|d| format!("{} ({:.1}kHz/{}bit)", d.name, d.sample_rate as f32 / 1000.0, d.bit_depth))
            .unwrap_or_else(|| "系统默认输出设备".to_string());
        r.draw_text(
            "音频输出设备",
            RectF::new(x0 + 18.0, y + 68.0, 140.0, 26.0),
            14.2,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        let dev_pill = RectF::new(x0 + cw - 338.0, y + 67.0, 320.0, 28.0);
        let dev_hov = self.devices.len() > 1 && dev_pill.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            dev_pill,
            8.5,
            GlassBtnVariant::Regular,
            dev_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            &def_dev,
            dev_pill.inset(10.0, 0.0),
            12.0,
            if dev_hov { 600 } else { 500 },
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        if self.devices.len() > 1 {
            let cur_i = self.devices.iter().position(|d| d.is_default).unwrap_or(0);
            let next_id = self.devices[(cur_i + 1) % self.devices.len()].id.clone();
            self.hit_regions.push(HitRegion {
                rect: dev_pill,
                action: UiAction::SelectOutputDevice(next_id),
            });
        }
        r.draw_line(x0 + 18.0, y + 108.0, x0 + cw - 18.0, y + 108.0, Theme::SEPARATOR, 1.0);

        // Row 3: Calm Playback Mode (`内置播放服务` vs `Apple Music 应用` + optional session release)
        r.draw_text(
            "播放方式",
            RectF::new(x0 + 18.0, y + 118.0, 140.0, 24.0),
            14.2,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        let svc_sub = if self.agent_status.connected {
            "内置服务已连接"
        } else {
            "按需自动连接"
        };
        r.draw_text(
            svc_sub,
            RectF::new(x0 + 18.0, y + 140.0, 150.0, 18.0),
            11.8,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        if self.agent_status.connected {
            let rel_btn = RectF::new(x0 + 132.0, y + 137.0, 72.0, 23.0);
            let rel_hov = rel_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                rel_btn,
                6.5,
                GlassBtnVariant::Regular,
                rel_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                "释放会话",
                rel_btn,
                11.0,
                if rel_hov { 600 } else { 500 },
                if rel_hov {
                    Theme::TEXT_PRIMARY
                } else {
                    Theme::TEXT_SECONDARY
                },
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: rel_btn,
                action: UiAction::DisconnectBuiltinService,
            });
        }

        let m1 = RectF::new(x0 + cw - 246.0, y + 122.0, 110.0, 28.0);
        let m2 = RectF::new(x0 + cw - 128.0, y + 122.0, 110.0, 28.0);
        for (mr, lbl, val) in [(m1, "内置播放服务", true), (m2, "Apple Music 应用", false)] {
            let sel = self.settings.use_builtin_service == val;
            let mhov = mr.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                mr,
                8.5,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                mhov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                lbl,
                mr,
                12.0,
                if sel || mhov { 600 } else { 400 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: mr,
                action: UiAction::SetUseBuiltinService(val),
            });
        }
        r.draw_line(x0 + 18.0, y + 164.0, x0 + cw - 18.0, y + 164.0, Theme::SEPARATOR, 1.0);

        // Row 4: Sound Check & Crossfade compact toggles
        self.draw_toggle_row(
            r,
            RectF::new(x0 + 18.0, y + 170.0, (cw - 52.0) * 0.5, 36.0),
            "音量平衡",
            "",
            self.audio_prefs.sound_check_enabled,
            UiAction::ToggleSoundCheck,
        );
        self.draw_toggle_row(
            r,
            RectF::new(x0 + cw * 0.5 + 8.0, y + 170.0, (cw - 52.0) * 0.5, 36.0),
            "歌曲交叉渐入渐出",
            "",
            self.audio_prefs.crossfade_songs_enabled,
            UiAction::ToggleCrossfade,
        );

        y += 232.0;

        // --- Group 3: 可视化与显示体验 (Includes FPS Cap up to VSync + Adjustable Window Corner Radius!) ---
        r.draw_text(
            "可视化与显示",
            RectF::new(x0 + 4.0, y, cw, 22.0),
            13.0,
            600,
            Theme::TEXT_MUTED,
            TextAlign::Left,
            true,
            false,
        );
        y += 26.0;

        let viz_custom_open = self.settings.viz_fps_cap == FpsCap::Custom;
        let lyr_custom_open = self.settings.lyrics_fps_cap == FpsCap::Custom;
        let viz_extra = if viz_custom_open { 38.0 } else { 0.0 };
        let lyr_extra = if lyr_custom_open { 38.0 } else { 0.0 };
        let card_h = 594.0 + viz_extra + lyr_extra;
        let g_viz = RectF::new(x0, y, cw, card_h);
        r.fill_rounded_rect(g_viz, 14.0, Theme::BG_ELEVATED);
        r.stroke_rounded_rect(g_viz, 14.0, Theme::SEPARATOR, 1.0);

        let mon_hz = self.current_monitor_hz.clamp(30, 500);
        let mut ry = y + 14.0;

        // 1. 可视化页面刷新率 + 自定义滑杆
        let viz_resolved = self
            .settings
            .viz_fps_cap
            .resolved_label(mon_hz, self.settings.viz_custom_fps);
        r.draw_text(
            "可视化页面刷新率",
            RectF::new(x0 + 18.0, ry, 240.0, 22.0),
            14.2,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            &format!("当前屏幕 {} Hz · 目标刷新率：{}", mon_hz, viz_resolved),
            RectF::new(x0 + 18.0, ry + 22.0, cw - 36.0, 18.0),
            12.0,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        let mut fx = x0 + 18.0;
        let fy = ry + 46.0;
        for cap in FpsCap::ALL {
            let sel = self.settings.viz_fps_cap == cap;
            let bw = match cap {
                FpsCap::Hz60 | FpsCap::Hz72 => 52.0,
                FpsCap::Adaptive60_72 => 116.0,
                FpsCap::VSync => 78.0,
                FpsCap::Custom => 68.0,
            };
            let br = RectF::new(fx, fy, bw, 28.0);
            let bhov = self.mouse_pos.1 >= TOP_BAR_GUARD_H && br.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                br,
                8.5,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                bhov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                cap.label(),
                br,
                12.0,
                if sel || bhov { 600 } else { 400 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: br,
                action: UiAction::SetVizFpsCap(cap),
            });
            fx += bw + 8.0;
        }
        ry += 82.0;

        if viz_custom_open {
            let cur_fps = self.settings.viz_custom_fps.clamp(30, 360);
            let dec_r = RectF::new(x0 + 18.0, ry + 2.0, 28.0, 24.0);
            let dec_hov = self.mouse_pos.1 >= TOP_BAR_GUARD_H && dec_r.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                dec_r,
                7.0,
                GlassBtnVariant::Regular,
                dec_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text("-", dec_r, 13.0, 600, Theme::TEXT_PRIMARY, TextAlign::Center, true, false);
            self.hit_regions.push(HitRegion {
                rect: dec_r,
                action: UiAction::SetVizCustomFps(cur_fps.saturating_sub(1).max(30)),
            });

            let bar_x = x0 + 54.0;
            let bar_w = (cw - 176.0).max(120.0);
            let fps_bar = RectF::new(bar_x, ry + 11.0, bar_w, 6.0);
            let fps_hit = RectF::new(bar_x - 6.0, ry - 2.0, bar_w + 12.0, 32.0);
            let fps_hov = fps_hit.contains(self.mouse_pos.0, self.mouse_pos.1)
                || matches!(self.active_drag, Some(ActiveDrag::VizCustomFps(_)));
            let f_norm = ((cur_fps as f32 - 30.0) / 330.0).clamp(0.0, 1.0);
            r.fill_rounded_rect(fps_bar, 3.0, Color::rgba(255, 255, 255, 0.14));
            if f_norm > 0.005 {
                r.fill_rounded_rect(
                    RectF::new(fps_bar.x, fps_bar.y, (fps_bar.w * f_norm).max(4.0), fps_bar.h),
                    3.0,
                    Theme::ACCENT_RED,
                );
            }
            let fcx = fps_bar.x + fps_bar.w * f_norm;
            let ftr = if fps_hov { 8.0 } else { 6.5 };
            r.fill_circle(fcx, fps_bar.center_y(), ftr, Color::rgb(252, 252, 255));
            self.hit_regions.push(HitRegion {
                rect: fps_hit,
                action: UiAction::ScrubVizCustomFps(fps_bar),
            });

            let inc_r = RectF::new(bar_x + bar_w + 8.0, ry + 2.0, 28.0, 24.0);
            let inc_hov = self.mouse_pos.1 >= TOP_BAR_GUARD_H && inc_r.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                inc_r,
                7.0,
                GlassBtnVariant::Regular,
                inc_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text("+", inc_r, 13.0, 600, Theme::TEXT_PRIMARY, TextAlign::Center, true, false);
            self.hit_regions.push(HitRegion {
                rect: inc_r,
                action: UiAction::SetVizCustomFps((cur_fps + 1).min(360)),
            });

            r.draw_text(
                &format!("{} Hz", cur_fps),
                RectF::new(inc_r.right() + 10.0, ry + 2.0, 64.0, 24.0),
                12.5,
                600,
                Theme::TEXT_PRIMARY,
                TextAlign::Right,
                true,
                false,
            );
            ry += 38.0;
        }

        r.draw_line(x0 + 18.0, ry + 4.0, x0 + cw - 18.0, ry + 4.0, Theme::SEPARATOR, 1.0);
        ry += 14.0;

        // 2. 歌词页面刷新率 + 自定义滑杆
        let lyr_resolved = self
            .settings
            .lyrics_fps_cap
            .resolved_label(mon_hz, self.settings.lyrics_custom_fps);
        r.draw_text(
            "歌词页面刷新率",
            RectF::new(x0 + 18.0, ry, 240.0, 22.0),
            14.2,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            &format!("当前屏幕 {} Hz · 目标刷新率：{}", mon_hz, lyr_resolved),
            RectF::new(x0 + 18.0, ry + 22.0, cw - 36.0, 18.0),
            12.0,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        let mut lfx = x0 + 18.0;
        let lfy = ry + 46.0;
        for cap in FpsCap::ALL {
            let sel = self.settings.lyrics_fps_cap == cap;
            let bw = match cap {
                FpsCap::Hz60 | FpsCap::Hz72 => 52.0,
                FpsCap::Adaptive60_72 => 116.0,
                FpsCap::VSync => 78.0,
                FpsCap::Custom => 68.0,
            };
            let br = RectF::new(lfx, lfy, bw, 28.0);
            let bhov = self.mouse_pos.1 >= TOP_BAR_GUARD_H && br.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                br,
                8.5,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                bhov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                cap.label(),
                br,
                12.0,
                if sel || bhov { 600 } else { 400 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: br,
                action: UiAction::SetLyricsFpsCap(cap),
            });
            lfx += bw + 8.0;
        }
        ry += 82.0;

        if lyr_custom_open {
            let cur_fps = self.settings.lyrics_custom_fps.clamp(30, 360);
            let dec_r = RectF::new(x0 + 18.0, ry + 2.0, 28.0, 24.0);
            let dec_hov = self.mouse_pos.1 >= TOP_BAR_GUARD_H && dec_r.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                dec_r,
                7.0,
                GlassBtnVariant::Regular,
                dec_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text("-", dec_r, 13.0, 600, Theme::TEXT_PRIMARY, TextAlign::Center, true, false);
            self.hit_regions.push(HitRegion {
                rect: dec_r,
                action: UiAction::SetLyricsCustomFps(cur_fps.saturating_sub(1).max(30)),
            });

            let bar_x = x0 + 54.0;
            let bar_w = (cw - 176.0).max(120.0);
            let fps_bar = RectF::new(bar_x, ry + 11.0, bar_w, 6.0);
            let fps_hit = RectF::new(bar_x - 6.0, ry - 2.0, bar_w + 12.0, 32.0);
            let fps_hov = fps_hit.contains(self.mouse_pos.0, self.mouse_pos.1)
                || matches!(self.active_drag, Some(ActiveDrag::LyricsCustomFps(_)));
            let f_norm = ((cur_fps as f32 - 30.0) / 330.0).clamp(0.0, 1.0);
            r.fill_rounded_rect(fps_bar, 3.0, Color::rgba(255, 255, 255, 0.14));
            if f_norm > 0.005 {
                r.fill_rounded_rect(
                    RectF::new(fps_bar.x, fps_bar.y, (fps_bar.w * f_norm).max(4.0), fps_bar.h),
                    3.0,
                    Theme::ACCENT_RED,
                );
            }
            let fcx = fps_bar.x + fps_bar.w * f_norm;
            let ftr = if fps_hov { 8.0 } else { 6.5 };
            r.fill_circle(fcx, fps_bar.center_y(), ftr, Color::rgb(252, 252, 255));
            self.hit_regions.push(HitRegion {
                rect: fps_hit,
                action: UiAction::ScrubLyricsCustomFps(fps_bar),
            });

            let inc_r = RectF::new(bar_x + bar_w + 8.0, ry + 2.0, 28.0, 24.0);
            let inc_hov = self.mouse_pos.1 >= TOP_BAR_GUARD_H && inc_r.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                inc_r,
                7.0,
                GlassBtnVariant::Regular,
                inc_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text("+", inc_r, 13.0, 600, Theme::TEXT_PRIMARY, TextAlign::Center, true, false);
            self.hit_regions.push(HitRegion {
                rect: inc_r,
                action: UiAction::SetLyricsCustomFps((cur_fps + 1).min(360)),
            });

            r.draw_text(
                &format!("{} Hz", cur_fps),
                RectF::new(inc_r.right() + 10.0, ry + 2.0, 64.0, 24.0),
                12.5,
                600,
                Theme::TEXT_PRIMARY,
                TextAlign::Right,
                true,
                false,
            );
            ry += 38.0;
        }

        r.draw_line(x0 + 18.0, ry + 4.0, x0 + cw - 18.0, ry + 4.0, Theme::SEPARATOR, 1.0);
        ry += 10.0;

        self.draw_toggle_row(
            r,
            RectF::new(x0 + 18.0, ry, cw - 36.0, 54.0),
            "窗口失去焦点时限制歌词页帧率为 60/72 自适应",
            "默认关闭（失焦时仍保持满帧率渲染）；开启后失焦自动切换为当前屏幕 60/72 自适应帧率",
            self.settings.lyrics_limit_unfocused_fps,
            UiAction::ToggleLyricsLimitUnfocusedFps,
        );
        ry += 58.0;

        r.draw_line(x0 + 18.0, ry + 4.0, x0 + cw - 18.0, ry + 4.0, Theme::SEPARATOR, 1.0);
        ry += 14.0;

        // Immersive Window Border Corner Radius (`直角 0px` / `默认 12px` / `圆润 22px` + manual `0..28 px` slider!)
        let cur_rad = self.settings.window_corner_radius.clamp(0.0, 28.0);
        r.draw_text(
            "沉浸式窗口边框圆角",
            RectF::new(x0 + 18.0, ry, 176.0, 22.0),
            14.0,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            &format!(
                "当前 {:.0} px · 可选直角或拖动滑杆精细调节圆角大小",
                cur_rad
            ),
            RectF::new(x0 + 18.0, ry + 22.0, 310.0, 18.0),
            11.8,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );

        let presets: &[(&str, f32, f32)] = &[
            ("直角 0px", 0.0, 68.0),
            ("默认 12px", 12.0, 74.0),
            ("大圆角 22px", 22.0, 82.0),
        ];
        let mut rx_btn = x0 + 320.0;
        for &(plbl, pval, pw) in presets {
            let sel = (cur_rad - pval).abs() < 0.45;
            let pr = RectF::new(rx_btn, ry + 6.0, pw, 28.0);
            let phov = pr.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                pr,
                8.5,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                phov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                plbl,
                pr,
                11.8,
                if sel || phov { 600 } else { 400 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: pr,
                action: UiAction::SetWindowCornerRadius(pval),
            });
            rx_btn += pw + 7.0;
        }

        let rad_bar_x = (rx_btn + 8.0).min(x0 + cw - 148.0);
        let rad_bar_w = (x0 + cw - 22.0 - rad_bar_x).max(90.0);
        let rad_bar = RectF::new(rad_bar_x, ry + 17.0, rad_bar_w, 6.0);
        let rad_hit = RectF::new(rad_bar.x - 6.0, ry + 4.0, rad_bar.w + 12.0, 32.0);
        let rad_hov = rad_hit.contains(self.mouse_pos.0, self.mouse_pos.1)
            || matches!(self.active_drag, Some(ActiveDrag::WindowCornerRadius(_)));
        let rad_norm = (cur_rad / 28.0).clamp(0.0, 1.0);
        r.fill_rounded_rect(rad_bar, 3.0, Color::rgba(255, 255, 255, 0.14));
        if rad_norm > 0.005 {
            r.fill_rounded_rect(
                RectF::new(rad_bar.x, rad_bar.y, (rad_bar.w * rad_norm).max(4.0), rad_bar.h),
                3.0,
                Theme::ACCENT_RED,
            );
        }
        let thumb_cx = rad_bar.x + rad_bar.w * rad_norm;
        let thumb_r = if rad_hov { 8.5 } else { 7.0 };
        r.fill_circle(
            thumb_cx,
            rad_bar.center_y() + 1.5,
            thumb_r + 1.0,
            Color::rgba(0, 0, 0, 0.36),
        );
        r.fill_circle(
            thumb_cx,
            rad_bar.center_y(),
            thumb_r,
            Color::rgb(252, 252, 255),
        );
        if rad_hov {
            r.stroke_circle(thumb_cx, rad_bar.center_y(), thumb_r + 1.5, Theme::ACCENT_RED, 1.2);
        }
        self.hit_regions.push(HitRegion {
            rect: rad_hit,
            action: UiAction::ScrubWindowCornerRadius(rad_bar),
        });
        ry += 52.0;

        r.draw_line(x0 + 18.0, ry + 4.0, x0 + cw - 18.0, ry + 4.0, Theme::SEPARATOR, 1.0);
        ry += 12.0;

        self.draw_toggle_row(
            r,
            RectF::new(x0 + 18.0, ry, (cw - 52.0) * 0.5, 54.0),
            "液态玻璃材质",
            "默认关闭；开启后启用实时折射与流光边",
            self.settings.liquid_glass_enabled,
            UiAction::ToggleLiquidGlass,
        );
        self.draw_toggle_row(
            r,
            RectF::new(x0 + cw * 0.5 + 8.0, ry, (cw - 52.0) * 0.5, 54.0),
            "西文歌词按词律动",
            "英语以 Word 为单位（关闭则逐字母细分）",
            self.settings.lyrics_latin_by_word,
            UiAction::ToggleLyricsLatinByWord,
        );
        ry += 60.0;

        r.draw_line(x0 + 18.0, ry + 4.0, x0 + cw - 18.0, ry + 4.0, Theme::SEPARATOR, 1.0);
        ry += 10.0;

        self.draw_toggle_row(
            r,
            RectF::new(x0 + 18.0, ry, (cw - 52.0) * 0.5, 54.0),
            "减弱动态效果",
            "收敛弹簧与背景粒子运动",
            self.settings.reduce_motion,
            UiAction::ToggleReduceMotion,
        );
        self.draw_toggle_row(
            r,
            RectF::new(x0 + cw * 0.5 + 8.0, ry, (cw - 52.0) * 0.5, 54.0),
            "降低透明度",
            "将半透明浮层切换为纯色表面",
            self.settings.reduce_transparency,
            UiAction::ToggleReduceTransparency,
        );
        ry += 60.0;

        r.draw_line(x0 + 18.0, ry + 4.0, x0 + cw - 18.0, ry + 4.0, Theme::SEPARATOR, 1.0);
        ry += 10.0;
        self.draw_toggle_row(
            r,
            RectF::new(x0 + 18.0, ry, cw - 36.0, 42.0),
            "歌词视图纯净柔焦背景",
            "",
            self.settings.lyrics_free_blur,
            UiAction::ToggleLyricsBlurMode,
        );
        ry += 48.0;

        r.draw_line(x0 + 18.0, ry + 4.0, x0 + cw - 18.0, ry + 4.0, Theme::SEPARATOR, 1.0);
        ry += 10.0;
        r.draw_text(
            "为非强调歌词加入发光/放大效果",
            RectF::new(x0 + 18.0, ry, 260.0, 22.0),
            14.0,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            "默认仅上浮，仅强调词句高亮放大与发光",
            RectF::new(x0 + 18.0, ry + 24.0, 290.0, 18.0),
            11.8,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        let fx_choices = [
            (NonEmphasisLyricsEffect::None, "关闭（默认）", 92.0f32),
            (NonEmphasisLyricsEffect::GlowOnly, "仅发光", 68.0),
            (NonEmphasisLyricsEffect::ScaleOnly, "仅放大", 68.0),
            (NonEmphasisLyricsEffect::Both, "发光 + 放大", 88.0),
        ];
        let total_fx_w: f32 = fx_choices.iter().map(|c| c.2).sum::<f32>() + 3.0 * 6.0;
        let mut fx_x = (x0 + cw - 18.0 - total_fx_w).max(x0 + 280.0);
        let fx_y = ry + 9.0;
        for (opt, lbl, bw) in fx_choices {
            let sel = self.settings.lyrics_non_emphasis_effect == opt;
            let br = RectF::new(fx_x, fx_y, bw, 28.0);
            let bhov = self.mouse_pos.1 >= TOP_BAR_GUARD_H && br.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                br,
                8.5,
                if sel {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Regular
                },
                bhov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                lbl,
                br,
                11.8,
                if sel || bhov { 600 } else { 400 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: br,
                action: UiAction::SetLyricsNonEmphasisEffect(opt),
            });
            fx_x += bw + 6.0;
        }

        y += card_h + 20.0;

        // --- Group 4: Hidden-by-Default Spatial Audio & Flavor Disclosure (`空间音频风味等的开关和设置默认隐藏`!) ---
        let disc_row = RectF::new(x0, y, 224.0, 30.0);
        let disc_hov = disc_row.contains(self.mouse_pos.0, self.mouse_pos.1);
        if disc_hov {
            r.draw_liquid_glass_button(
                disc_row,
                9.0,
                GlassBtnVariant::Subtle,
                true,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
        }
        r.draw_icon(
            if self.show_advanced_spatial {
                SfIcon::ChevronDown
            } else {
                SfIcon::ChevronRight
            },
            RectF::new(x0 + 8.0, y + 8.5, 13.0, 13.0),
            if disc_hov {
                Theme::TEXT_PRIMARY
            } else {
                Theme::TEXT_MUTED
            },
        );
        r.draw_text(
            "空间音频与声场选项",
            RectF::new(x0 + 26.0, y, 194.0, 30.0),
            12.8,
            if disc_hov { 600 } else { 500 },
            if disc_hov {
                Theme::TEXT_PRIMARY
            } else {
                Theme::TEXT_MUTED
            },
            TextAlign::Left,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: disc_row,
            action: UiAction::ToggleAdvancedSpatialDisclosure,
        });
        y += 36.0;

        if self.show_advanced_spatial {
            let sp_card = RectF::new(x0, y, cw, 224.0);
            r.fill_rounded_rect(sp_card, 14.0, Theme::BG_ELEVATED);
            r.stroke_rounded_rect(sp_card, 14.0, Theme::SEPARATOR, 1.0);

            // Row A: Dolby Atmos (collapsed inside spatial disclosure by default!)
            r.draw_text(
                "杜比全景声",
                RectF::new(x0 + 18.0, y + 14.0, 160.0, 28.0),
                14.0,
                600,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            let mut ax = x0 + cw - 18.0 - (72.0 * 3.0 + 16.0);
            for &(lbl, av) in &[("自动", 10i64), ("始终开启", 20), ("关闭", 30)] {
                let sel = self.audio_prefs.dolby_atmos_setting == av;
                let br = RectF::new(ax, y + 14.0, 72.0, 28.0);
                let bhov = br.contains(self.mouse_pos.0, self.mouse_pos.1);
                r.draw_liquid_glass_button(
                    br,
                    8.5,
                    if sel {
                        GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                    } else {
                        GlassBtnVariant::Regular
                    },
                    bhov,
                    self.mouse_down,
                    self.mouse_pos,
                    self.glass_off(),
                );
                r.draw_text(
                    lbl,
                    br,
                    12.0,
                    if sel || bhov { 600 } else { 400 },
                    Theme::TEXT_PRIMARY,
                    TextAlign::Center,
                    true,
                    false,
                );
                self.hit_regions.push(HitRegion {
                    rect: br,
                    action: UiAction::SetAtmosMode(av),
                });
                ax += 80.0;
            }
            r.draw_line(x0 + 18.0, y + 54.0, x0 + cw - 18.0, y + 54.0, Theme::SEPARATOR, 1.0);

            // Row B: Binaural HRTF toggle + Bypass Liquid Glass button
            self.draw_toggle_row(
                r,
                RectF::new(x0 + 18.0, y + 60.0, cw - 134.0, 44.0),
                "本地双耳空间渲染 (HRTF)",
                "对立体声信号进行外化头影与早期反射渲染（需两路可用输出端点）",
                self.binaural.active,
                UiAction::ToggleBinauralEnabled,
            );
            let byp_btn = RectF::new(x0 + cw - 102.0, y + 69.0, 84.0, 26.0);
            let byp_hov = byp_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
            r.draw_liquid_glass_button(
                byp_btn,
                7.5,
                if self.binaural.config.bypass {
                    GlassBtnVariant::Tinted(Theme::SIGNAL_AMBER)
                } else {
                    GlassBtnVariant::Regular
                },
                byp_hov,
                self.mouse_down,
                self.mouse_pos,
                self.glass_off(),
            );
            r.draw_text(
                if self.binaural.config.bypass {
                    "直通中"
                } else {
                    "直通对比"
                },
                byp_btn,
                11.8,
                if byp_hov || self.binaural.config.bypass { 600 } else { 500 },
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: byp_btn,
                action: UiAction::ToggleBinauralBypass,
            });
            r.draw_line(x0 + 18.0, y + 112.0, x0 + cw - 18.0, y + 112.0, Theme::SEPARATOR, 1.0);

            // Row C: Spatial Flavor Presets (`影院空间`, `近场监听`, `音乐厅`, `人声聚焦`)
            r.draw_text(
                "声场风味预设",
                RectF::new(x0 + 18.0, y + 124.0, 100.0, 28.0),
                13.5,
                500,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            let mut px = x0 + 124.0;
            for &(pkey, plbl) in &[
                ("apple_spatial", "影院空间"),
                ("studio_nearfield", "近场监听"),
                ("concert_hall", "音乐厅"),
                ("intimate_vocal", "人声聚焦"),
            ] {
                let sel = self.binaural.config.preset == pkey;
                let pr = RectF::new(px, y + 124.0, 82.0, 28.0);
                let phov = pr.contains(self.mouse_pos.0, self.mouse_pos.1);
                r.draw_liquid_glass_button(
                    pr,
                    8.5,
                    if sel {
                        GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                    } else {
                        GlassBtnVariant::Regular
                    },
                    phov,
                    self.mouse_down,
                    self.mouse_pos,
                    self.glass_off(),
                );
                r.draw_text(
                    plbl,
                    pr,
                    12.0,
                    if sel || phov { 600 } else { 400 },
                    Theme::TEXT_PRIMARY,
                    TextAlign::Center,
                    true,
                    false,
                );
                self.hit_regions.push(HitRegion {
                    rect: pr,
                    action: UiAction::SetBinauralPreset(pkey.to_string()),
                });
                px += 90.0;
            }
            r.draw_line(x0 + 18.0, y + 164.0, x0 + cw - 18.0, y + 164.0, Theme::SEPARATOR, 1.0);

            // Row D: Fine Spatial Width & Room Ambience Sliders with Hover Thumbs
            let half_w = (cw - 56.0) * 0.5;
            let cur_w = self.binaural.config.spatial_width.clamp(0.6, 1.6);
            let w_norm = (cur_w - 0.6) / 1.0;
            r.draw_text(
                &format!("声场宽度  {:.2}x", cur_w),
                RectF::new(x0 + 18.0, y + 174.0, 120.0, 20.0),
                12.5,
                500,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            let w_bar = RectF::new(x0 + 18.0, y + 200.0, half_w, 6.0);
            let w_hit = RectF::new(w_bar.x, y + 190.0, w_bar.w, 24.0);
            let w_hov = w_hit.contains(self.mouse_pos.0, self.mouse_pos.1)
                || matches!(self.active_drag, Some(ActiveDrag::BinauralWidth(_)));
            r.fill_rounded_rect(w_bar, 3.0, Theme::BG_TERTIARY);
            r.fill_rounded_rect(
                RectF::new(w_bar.x, w_bar.y, (w_bar.w * w_norm).max(6.0), w_bar.h),
                3.0,
                Theme::ACCENT_RED,
            );
            r.fill_circle(
                w_bar.x + w_bar.w * w_norm,
                w_bar.center_y(),
                if w_hov { 7.5 } else { 6.0 },
                Color::rgb(252, 252, 255),
            );
            self.hit_regions.push(HitRegion {
                rect: w_hit,
                action: UiAction::ScrubBinauralWidth(w_bar),
            });

            let rx0 = x0 + cw * 0.5 + 10.0;
            let cur_rm = self.binaural.config.room_ambience.clamp(0.0, 0.8);
            let rm_norm = cur_rm / 0.8;
            r.draw_text(
                &format!("房间混响  {:.0}%", cur_rm * 100.0),
                RectF::new(rx0, y + 174.0, 120.0, 20.0),
                12.5,
                500,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            let rm_bar = RectF::new(rx0, y + 200.0, half_w, 6.0);
            let rm_hit = RectF::new(rm_bar.x, y + 190.0, rm_bar.w, 24.0);
            let rm_hov = rm_hit.contains(self.mouse_pos.0, self.mouse_pos.1)
                || matches!(self.active_drag, Some(ActiveDrag::BinauralRoom(_)));
            r.fill_rounded_rect(rm_bar, 3.0, Theme::BG_TERTIARY);
            r.fill_rounded_rect(
                RectF::new(rm_bar.x, rm_bar.y, (rm_bar.w * rm_norm).max(6.0), rm_bar.h),
                3.0,
                Theme::ACCENT_RED,
            );
            r.fill_circle(
                rm_bar.x + rm_bar.w * rm_norm,
                rm_bar.center_y(),
                if rm_hov { 7.5 } else { 6.0 },
                Color::rgb(252, 252, 255),
            );
            self.hit_regions.push(HitRegion {
                rect: rm_hit,
                action: UiAction::ScrubBinauralRoom(rm_bar),
            });

            y += 238.0;
        }

        y
    }

    fn draw_toggle_row(
        &mut self,
        r: &mut D2DRenderer,
        rect: RectF,
        title: &str,
        subtitle: &str,
        is_on: bool,
        action: UiAction,
    ) {
        let is_hov = rect.contains(self.mouse_pos.0, self.mouse_pos.1);
        if is_hov {
            r.fill_rounded_rect(
                RectF::new(rect.x - 6.0, rect.y + 1.0, rect.w + 12.0, rect.h - 2.0),
                8.0,
                Color::rgba(255, 255, 255, 0.045),
            );
        }

        if subtitle.is_empty() {
            r.draw_text(
                title,
                RectF::new(rect.x, rect.y, rect.w - 56.0, rect.h),
                13.8,
                if is_hov { 600 } else { 500 },
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
        } else {
            r.draw_text(
                title,
                RectF::new(rect.x, rect.y + 6.0, rect.w - 56.0, 20.0),
                13.8,
                600,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            r.draw_text(
                subtitle,
                RectF::new(rect.x, rect.y + 26.0, rect.w - 56.0, 18.0),
                11.5,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
        }

        let sw = RectF::new(rect.right() - 46.0, rect.center_y() - 13.0, 44.0, 26.0);
        r.draw_liquid_glass_button(
            sw,
            13.0,
            if is_on {
                GlassBtnVariant::Tinted(Theme::SIGNAL_GREEN)
            } else {
                GlassBtnVariant::Regular
            },
            is_hov,
            self.mouse_down && is_hov,
            self.mouse_pos,
            self.glass_off(),
        );
        let knob_x = if is_on { sw.right() - 13.0 } else { sw.x + 13.0 };
        r.fill_circle(
            knob_x,
            sw.center_y() + 1.0,
            10.8,
            Color::rgba(0, 0, 0, 0.30),
        );
        r.fill_circle(
            knob_x,
            sw.center_y(),
            if is_hov { 10.8 } else { 10.2 },
            Color::rgb(255, 255, 255),
        );

        self.hit_regions.push(HitRegion { rect, action });
    }

    // ========================================================================
    // Floating Popovers (Up Next Queue & Volume/Output Quick Popover) + Banner
    // ========================================================================
    fn draw_queue_popover(&mut self, r: &mut D2DRenderer, area: RectF) {
        let q = am_agent::get_queue_tracks();
        let pop_w = 320.0;
        let pop_h = 340.0;
        let pop = RectF::new(
            (area.center_x() + Theme::MINI_PLAYER_MAX_WIDTH * 0.5 - pop_w).min(area.right() - pop_w - 18.0),
            area.bottom() - Theme::MINI_PLAYER_HEIGHT - Theme::MINI_PLAYER_BOTTOM_MARGIN - pop_h - 10.0,
            pop_w,
            pop_h,
        );
        // Swallow background clicks on popover glass so they never click underlying rows!
        self.hit_regions.push(HitRegion {
            rect: pop,
            action: UiAction::SwallowClick,
        });
        r.draw_liquid_glass(
            pop,
            18.0,
            18.0,
            Color::rgba(32, 32, 38, 0.86),
            Theme::GLASS_RIM_LIGHT,
            self.glass_off(),
        );
        r.draw_text(
            &format!("待播清单 ({})", q.len()),
            RectF::new(pop.x + 16.0, pop.y + 12.0, pop.w - 32.0, 22.0),
            14.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );

        if q.is_empty() {
            r.draw_text(
                "当前待播列表为空",
                RectF::new(pop.x + 16.0, pop.center_y() - 14.0, pop.w - 32.0, 28.0),
                13.0,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Center,
                true,
                false,
            );
            return;
        }

        let mut y = pop.y + 42.0;
        for (idx, tr) in q.iter().enumerate().take(7) {
            let row = RectF::new(pop.x + 10.0, y, pop.w - 20.0, 38.0);
            let is_cur = self
                .smtc
                .adam_id
                .as_deref()
                .is_some_and(|a| a == tr.effective_adam_id());
            if is_cur {
                r.fill_rounded_rect(row, 8.0, Theme::ROW_ACTIVE);
            } else if row.contains(self.mouse_pos.0, self.mouse_pos.1) {
                r.fill_rounded_rect(row, 8.0, Theme::ROW_HOVER);
            }
            r.draw_text(
                &tr.name,
                RectF::new(row.x + 10.0, row.y + 3.0, row.w - 20.0, 18.0),
                13.0,
                if is_cur { 600 } else { 400 },
                if is_cur {
                    Theme::ACCENT_RED
                } else {
                    Theme::TEXT_PRIMARY
                },
                TextAlign::Left,
                true,
                false,
            );
            r.draw_text(
                &tr.artist_name,
                RectF::new(row.x + 10.0, row.y + 19.0, row.w - 20.0, 15.0),
                11.2,
                400,
                Theme::TEXT_SECONDARY,
                TextAlign::Left,
                true,
                false,
            );
            self.hit_regions.push(HitRegion {
                rect: row,
                action: UiAction::JumpQueueIndex(idx),
            });
            y += 40.0;
        }
    }

    fn draw_more_popover(&mut self, r: &mut D2DRenderer, area: RectF) {
        let pop_w = 270.0;
        let pop_h = 136.0;
        let pop = RectF::new(
            (area.center_x() + Theme::MINI_PLAYER_MAX_WIDTH * 0.5 - pop_w - 40.0)
                .min(area.right() - pop_w - 24.0),
            area.bottom() - Theme::MINI_PLAYER_HEIGHT - Theme::MINI_PLAYER_BOTTOM_MARGIN - pop_h - 10.0,
            pop_w,
            pop_h,
        );
        self.hit_regions.push(HitRegion {
            rect: pop,
            action: UiAction::SwallowClick,
        });
        r.draw_liquid_glass(
            pop,
            18.0,
            18.0,
            Color::rgba(32, 32, 38, 0.88),
            Theme::GLASS_RIM_LIGHT,
            self.glass_off(),
        );

        let vol = self.smtc.volume.unwrap_or(0.75).clamp(0.0, 1.0) as f32;
        r.draw_text(
            &format!("播放音量  {:.0}%", vol * 100.0),
            RectF::new(pop.x + 16.0, pop.y + 14.0, pop.w - 32.0, 20.0),
            12.8,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );

        let vbar = RectF::new(pop.x + 16.0, pop.y + 44.0, pop.w - 32.0, 6.0);
        r.fill_rounded_rect(vbar, 3.0, Color::rgba(255, 255, 255, 0.18));
        r.fill_rounded_rect(
            RectF::new(vbar.x, vbar.y, (vbar.w * vol).max(6.0), vbar.h),
            3.0,
            Theme::TEXT_PRIMARY,
        );
        self.hit_regions.push(HitRegion {
            rect: RectF::new(vbar.x, pop.y + 34.0, vbar.w, 26.0),
            action: UiAction::ScrubPopoverVolume(vbar),
        });

        let set_btn = RectF::new(pop.x + 14.0, pop.bottom() - 46.0, pop.w - 28.0, 32.0);
        let set_hov = set_btn.contains(self.mouse_pos.0, self.mouse_pos.1);
        r.draw_liquid_glass_button(
            set_btn,
            9.5,
            GlassBtnVariant::Regular,
            set_hov,
            self.mouse_down,
            self.mouse_pos,
            self.glass_off(),
        );
        r.draw_text(
            "打开音频与可视化设置...",
            set_btn,
            12.5,
            if set_hov { 600 } else { 500 },
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: set_btn,
            action: UiAction::Navigate(PageRoute::Settings),
        });
    }

    fn draw_banner_toast(&mut self, r: &mut D2DRenderer, area: RectF, msg: &str, is_err: bool) {
        let bw = (msg.chars().count() as f32 * 14.0 + 48.0).clamp(220.0, area.w - 60.0);
        let toast = RectF::new(area.center_x() - bw * 0.5, 16.0, bw, 36.0);
        r.fill_rounded_rect(
            toast,
            18.0,
            if is_err {
                Color::rgba(195, 36, 54, 0.94)
            } else {
                Color::rgba(42, 42, 48, 0.94)
            },
        );
        r.stroke_rounded_rect(toast, 18.0, Theme::GLASS_RIM_LIGHT, 1.0);
        r.draw_text(
            msg,
            toast.inset(16.0, 0.0),
            12.8,
            500,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
            false,
        );
        self.hit_regions.push(HitRegion {
            rect: toast,
            action: UiAction::DismissBanner,
        });
    }
}

// ============================================================================
// Pure Helper Functions
// ============================================================================

fn smtc_meta_key(title: &str, artist: &str) -> String {
    format!("{}\x1f{}", title.trim().to_lowercase(), artist.trim().to_lowercase())
}

fn shuffle_slice_from<T>(slice: &mut [T], start: usize) {
    if slice.len() <= start + 1 {
        return;
    }
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ ((slice.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    for i in ((start + 1)..slice.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let span = (i - start + 1) as u64;
        let j = start + ((state >> 32) % span) as usize;
        slice.swap(i, j);
    }
}

fn intersect_rect(a: RectF, b: RectF) -> Option<RectF> {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = a.right().min(b.right());
    let y2 = a.bottom().min(b.bottom());
    if x2 > x1 + 1.0 && y2 > y1 + 1.0 {
        Some(RectF::new(x1, y1, x2 - x1, y2 - y1))
    } else {
        None
    }
}

/// Scores Apple Music Catalog search candidates for External SMTC enrichment using
/// `lyrics::strict_catalog_match_score` (edition-preserving filter + hard artist overlap +
/// `|Δdur| <= 2.2s` gate), NEVER falling back to `arr[0]` on mismatches.
pub fn pick_best_smtc_catalog_candidate<'a>(
    arr: &'a [Value],
    title: &str,
    artist: &str,
    album: &str,
    duration_sec: f64,
) -> Option<&'a Value> {
    let mut best: Option<(f64, &'a Value)> = None;
    for item in arr {
        let attrs = item.get("attributes").unwrap_or(item);
        let c_title = attrs.get("name").and_then(Value::as_str).unwrap_or("");
        let c_artist = attrs.get("artistName").and_then(Value::as_str).unwrap_or("");
        let c_album = attrs.get("albumName").and_then(Value::as_str).unwrap_or("");
        let c_dur = attrs
            .get("durationInMillis")
            .and_then(Value::as_f64)
            .map(|ms| ms / 1000.0)
            .unwrap_or(0.0);

        if lyrics::has_mismatched_edition_tag(title, album, &format!("{c_title} {c_album}")) {
            continue;
        }
        let Some(mut score) =
            lyrics::strict_catalog_match_score(title, artist, duration_sec, c_title, c_artist, c_dur)
        else {
            continue;
        };
        if !album.trim().is_empty() && !c_album.trim().is_empty() {
            let a_low = album.trim().to_lowercase();
            let ca_low = c_album.trim().to_lowercase();
            if a_low == ca_low || ca_low.contains(&a_low) {
                score += 12.0;
            }
        }
        if best.as_ref().is_none_or(|(bs, _)| score > *bs) {
            best = Some((score, item));
        }
    }
    best.map(|(_, item)| item)
}

async fn lookup_smtc_track_in_catalog(
    title: &str,
    artist: &str,
    album: &str,
    duration_sec: f64,
    sf: &str,
    utok: Option<String>,
    prefs: &AudioPrefs,
) -> Option<(
    String,
    Option<String>,
    Option<String>,
    Option<audio_viz::StreamAudioSpecs>,
)> {
    let q_raw = format!("{} {}", title.trim(), artist.trim());
    let enc: String = url::form_urlencoded::byte_serialize(q_raw.trim().as_bytes()).collect();
    let search_primary = format!(
        "v1/catalog/{sf}/search?term={enc}&types=songs&extend=extendedAssetUrls,audioTraits&limit=10"
    );
    let json = match am_api::fetch_am_api(
        search_primary,
        utok.clone(),
        None,
        Some(sf.to_string()),
    )
    .await
    {
        Ok(j) => j,
        Err(_) => {
            let fallback_path = format!(
                "v1/catalog/cn/search?term={enc}&types=songs&extend=extendedAssetUrls,audioTraits&limit=10&l=zh-Hans-CN"
            );
            am_api::fetch_am_api(fallback_path, utok, None, Some("cn".to_string()))
                .await
                .ok()?
        }
    };
    let arr = json.pointer("/results/songs/data")?.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let best = pick_best_smtc_catalog_candidate(arr, title, artist, album, duration_sec)?;

    let parsed = parse_track_from_am_json(best, None, None)?;
    let aid = parsed.effective_adam_id();
    let badge = format_audio_traits_badge(&parsed.audio_traits);
    let art = parsed.artwork_url.clone();

    let mut specs = None;
    if let Some(hls_url) = best
        .pointer("/attributes/extendedAssetUrls/enhancedHls")
        .and_then(Value::as_str)
    {
        if let Ok(m3u8) = am_api::fetch_enhanced_hls_manifest(hls_url).await {
            specs = audio_viz::parse_enhanced_hls_variants(
                &m3u8,
                prefs.lossless_enabled,
                prefs.streaming_audio_quality,
                prefs.dolby_atmos_setting,
            );
        }
    } else if !aid.is_empty() {
        let song_path = format!(
            "v1/catalog/{sf}/songs/{aid}?extend=extendedAssetUrls,audioTraits"
        );
        if let Ok(sj) = am_api::fetch_am_api(song_path, None, None, Some(sf.to_string())).await {
            if let Some(hls_url) = sj
                .pointer("/data/0/attributes/extendedAssetUrls/enhancedHls")
                .and_then(Value::as_str)
            {
                if let Ok(m3u8) = am_api::fetch_enhanced_hls_manifest(hls_url).await {
                    specs = audio_viz::parse_enhanced_hls_variants(
                        &m3u8,
                        prefs.lossless_enabled,
                        prefs.streaming_audio_quality,
                        prefs.dolby_atmos_setting,
                    );
                }
            }
        }
    }

    Some((aid, art, badge, specs))
}

fn non_empty_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn unified_playback_snapshot() -> SmtcState {
    if am_agent::is_agent_active() {
        return am_agent::get_playback_state();
    }
    let os = smtc::get_smtc_state();
    if os.active {
        return os;
    }
    if am_agent::is_agent_connected() {
        return am_agent::get_playback_state();
    }
    os
}

async fn enrich_track_hls_and_meta(aid: &str, sf_primary: &str) {
    let mut sfs = vec![sf_primary.to_string()];
    for fb in ["cn", "us"] {
        if !sfs.iter().any(|s| s == fb) {
            sfs.push(fb.to_string());
        }
    }
    for sf in sfs {
        let path =
            format!("v1/catalog/{sf}/songs/{aid}?extend=extendedAssetUrls,audioTraits&l=zh-Hans-CN");
        let Ok(json) = am_api::fetch_am_api(path, None, None, Some(sf)).await else {
            continue;
        };
        let Some(item) = json.pointer("/data/0") else {
            continue;
        };
        if let Some(parsed) = parse_track_from_am_json(item, None, None) {
            am_agent::enrich_cached_track(
                aid,
                CachedTrackMeta {
                    title: parsed.name,
                    artist: parsed.artist_name,
                    album: parsed.album_name,
                    duration_sec: parsed.duration_sec,
                    artwork_url: parsed.artwork_url,
                    audio_quality: format_audio_traits_badge(&parsed.audio_traits),
                },
            );
        }
        if let Some(hls_url) = item
            .pointer("/attributes/extendedAssetUrls/enhancedHls")
            .and_then(Value::as_str)
        {
            if let Ok(m3u8) = am_api::fetch_enhanced_hls_manifest(hls_url).await {
                am_agent::set_track_hls_manifest(aid, m3u8);
            }
        }
        break;
    }
}

fn parse_media_cards_array(val: Option<&Value>, fallback_kind: &str) -> Vec<MediaCardItem> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(fallback_kind);
        let attrs = item.get("attributes").unwrap_or(item);
        let title = attrs
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("未知标题")
            .to_string();
        let subtitle = attrs
            .get("artistName")
            .or_else(|| attrs.get("curatorName"))
            .and_then(Value::as_str)
            .unwrap_or("Apple Music")
            .to_string();
        let artwork_url = attrs
            .pointer("/artwork/url")
            .and_then(Value::as_str)
            .map(|u| format_artwork_template(u, 360));
        let badge = attrs
            .get("audioTraits")
            .and_then(Value::as_array)
            .and_then(|tr| {
                let v: Vec<String> = tr
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.to_string())
                    .collect();
                format_audio_traits_badge(&v)
            });

        out.push(MediaCardItem {
            id: id.to_string(),
            kind: item_type.to_string(),
            title,
            subtitle,
            description: String::new(),
            artwork_url,
            badge,
            track_input: None,
        });
    }
    out
}

fn strip_html_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut inside = false;
    for c in raw.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .trim()
        .to_string()
}

fn format_dur(sec: f64) -> String {
    let s = sec.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Critically-damped 2nd-order smooth scroll integrator (`zeta = 1.0`, `omega = 21.5`).
/// Builds velocity smoothly without instantaneous jumps and settles with zero overshoot or steppiness!
fn step_critically_damped_scroll(pos: &mut f32, vel: &mut f32, target: f32, dt_sec: f32) {
    let dt = dt_sec.clamp(0.001, 0.05);
    if (*pos - target).abs() < 0.12 && vel.abs() < 0.8 {
        *pos = target;
        *vel = 0.0;
        return;
    }
    let steps = ((dt / 0.004).ceil() as usize).clamp(1, 16);
    let h = dt / (steps as f32);
    let omega = 21.5f32;
    let k = omega * omega;
    let c = 2.0 * omega;
    for _ in 0..steps {
        let err = *pos - target;
        let acc = -k * err - c * (*vel);
        *vel += acc * h;
        *pos += (*vel) * h;
    }
    if (*pos - target).abs() < 0.12 && vel.abs() < 0.8 {
        *pos = target;
        *vel = 0.0;
    }
}

fn fnv1a_hover_key(ns: &str, id: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in ns.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= 0xff;
    h = h.wrapping_mul(0x0000_0100_0000_01b3);
    for b in id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn read_clipboard_unicode_text() -> Option<String> {
    unsafe {
        use windows::Win32::System::DataExchange::{
            CloseClipboard, GetClipboardData, OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        use windows::Win32::System::Ole::CF_UNICODETEXT;

        let mut opened = false;
        for _ in 0..6 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        if !opened {
            return None;
        }
        let mut res = None;
        if let Ok(hmem) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
            let hglobal = windows::Win32::Foundation::HGLOBAL(hmem.0);
            let ptr = GlobalLock(hglobal) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while *ptr.add(len) != 0 && len < 65_536 {
                    len += 1;
                }
                res = Some(String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len)));
                let _ = GlobalUnlock(hglobal);
            }
        } else if let Ok(hmem) = GetClipboardData(1 /* CF_TEXT */) {
            let hglobal = windows::Win32::Foundation::HGLOBAL(hmem.0);
            let ptr = GlobalLock(hglobal) as *const u8;
            if !ptr.is_null() {
                let mut len = 0usize;
                while *ptr.add(len) != 0 && len < 65_536 {
                    len += 1;
                }
                res = Some(String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned());
                let _ = GlobalUnlock(hglobal);
            }
        }
        let _ = CloseClipboard();
        res
    }
}

fn open_external_browser(url: &str) {
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let op = HSTRING::from("open");
        let target = HSTRING::from(url);
        let _ = ShellExecuteW(None, &op, &target, None, None, SW_SHOWNORMAL);
    }
}

fn pick_local_lyrics_file_dialog(hwnd_val: usize) -> Option<PathBuf> {
    unsafe {
        use windows::core::PWSTR;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        };

        let mut file_buf = [0u16; 520];
        let filter: Vec<u16> = "歌词文件 (*.ttml;*.lrc;*.yrc)\0*.ttml;*.lrc;*.yrc\0所有文件 (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();

        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: HWND(hwnd_val as *mut std::ffi::c_void),
            lpstrFilter: windows::core::PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(file_buf.as_mut_ptr()),
            nMaxFile: file_buf.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };

        if GetOpenFileNameW(&mut ofn).as_bool() {
            let end = file_buf.iter().position(|&c| c == 0).unwrap_or(0);
            if end > 0 {
                return Some(PathBuf::from(String::from_utf16_lossy(&file_buf[..end])));
            }
        }
        None
    }
}
