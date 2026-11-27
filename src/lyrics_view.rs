#![cfg(target_os = "windows")]

use crate::d2d_renderer::{
    D2DRenderer, GlassBtnVariant, LyricsLineBmpKey, SfIcon, TextAlign, BACKDROP_DIRTY_ALL,
};
use crate::lyrics::{
    cjk_glow_density_scale, eval_frontier_glow_profile_ex, eval_frontier_lift_weight,
    eval_frontier_scale_delta, eval_frontier_wave_shear, should_use_frontier_lift, LyricCharUnit,
    LyricsAnimator, LyricsSourcePref, NonEmphasisLyricsEffect, ParsedLyrics, Spring1D,
};
use crate::motion_art::MotionFrameSnapshot;
use crate::smtc::SmtcState;
use crate::theme::{Color, RectF, Theme};
use windows_numerics::Matrix3x2;

#[derive(Clone, Debug)]
pub enum LyricsHitAction {
    CloseLyrics,
    WinMinimize,
    WinToggleMaximize,
    WinClose,
    SeekToSec(f64),
    ScrubProgressBar(RectF),
    ScrubVolumeBar(RectF),
    TogglePlayPause,
    PrevTrack,
    NextTrack,
    ToggleShuffle,
    ToggleRepeat,
    CycleLyricsSource,
    ImportLocalLyrics,
    ToggleTranslation,
    ToggleFreeBlur,
    ToggleImmersiveMode,
    OpenMoreMenu(RectF),
    OpenListenTogether,
    NudgeLyricsOffset(f64),
    ResetLyricsOffset,
    ToggleLyricsVisible,
    ToggleLyricsQueue,
    JumpLyricsQueueIndex(usize),
}

/// Evaluates target visibility (`[top_left, top_right, bottom_left, bottom_right]`) for peripheral
/// chrome on the Lyrics overlay when「沉浸模式」is enabled:
/// - All four corners fade out cleanly unless the pointer recently moved near that specific corner.
/// - Core playback controls, album artwork, synced lyrics, and flowing aurora background remain 100% visible.
pub fn eval_immersive_corner_targets(
    win_rect: RectF,
    mouse: (f32, f32),
    idle_sec: f32,
    immersive: bool,
    force_visible: bool,
) -> [f32; 4] {
    if !immersive || force_visible {
        return [1.0, 1.0, 1.0, 1.0];
    }
    if idle_sec > 2.4 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let (mx, my) = mouse;
    let in_tl = mx >= win_rect.x - 4.0
        && mx <= win_rect.x + 170.0
        && my >= win_rect.y - 4.0
        && my <= win_rect.y + 104.0;
    let in_tr = mx >= win_rect.right() - 224.0
        && mx <= win_rect.right() + 4.0
        && my >= win_rect.y - 4.0
        && my <= win_rect.y + 104.0;
    let in_bl = mx >= win_rect.x - 4.0
        && mx <= win_rect.x + 360.0
        && my >= win_rect.bottom() - 96.0
        && my <= win_rect.bottom() + 4.0;
    let in_br = mx >= win_rect.right() - 520.0
        && mx <= win_rect.right() + 4.0
        && my >= win_rect.bottom() - 108.0
        && my <= win_rect.bottom() + 4.0;
    [
        if in_tl { 1.0 } else { 0.0 },
        if in_tr { 1.0 } else { 0.0 },
        if in_bl { 1.0 } else { 0.0 },
        if in_br { 1.0 } else { 0.0 },
    ]
}

#[derive(Clone, Debug)]
pub struct LyricsHitZone {
    pub rect: RectF,
    pub action: LyricsHitAction,
    pub line_idx: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct DisjointUnitCell {
    pub raw_rect: RectF,
    pub glyph_rect: RectF,
    pub clip_rect: RectF,
}

#[derive(Clone, Copy, Debug)]
struct SubGlyphCell {
    glyph_rect: RectF,
    clip_rect: RectF,
    local_u: f32,
}

/// Partitions character/word hit-test rectangles into mathematically disjoint clip cells per visual row
/// and clamps vertical row boundaries at midpoints between adjacent wrapped rows so neither neighbor
/// characters nor wrapped lines ever bleed or overlap while lifting or animating.
pub fn build_disjoint_unit_cells(unit_rects: &[Vec<RectF>]) -> Vec<Vec<DisjointUnitCell>> {
    let mut out: Vec<Vec<DisjointUnitCell>> = vec![Vec::new(); unit_rects.len()];
    let mut flat: Vec<(usize, RectF)> = Vec::new();
    for (u_idx, rects) in unit_rects.iter().enumerate() {
        for &rc in rects {
            if rc.w > 0.05 && rc.h > 0.05 {
                flat.push((u_idx, rc));
            }
        }
    }
    if flat.is_empty() {
        return out;
    }

    flat.sort_by(|a, b| {
        a.1.center_y()
            .total_cmp(&b.1.center_y())
            .then_with(|| a.1.x.total_cmp(&b.1.x))
    });

    let mut rows: Vec<Vec<(usize, RectF)>> = Vec::new();
    for item in flat {
        let rc = item.1;
        if let Some(last_row) = rows.last_mut() {
            let ref_rc = last_row[0].1;
            let row_tol = (ref_rc.h.min(rc.h) * 0.45).max(4.0);
            if (rc.center_y() - ref_rc.center_y()).abs() <= row_tol {
                last_row.push(item);
                continue;
            }
        }
        rows.push(vec![item]);
    }

    let row_bounds: Vec<(f32, f32)> = rows
        .iter()
        .map(|row| {
            let top = row.iter().map(|x| x.1.y).fold(f32::INFINITY, f32::min);
            let bot = row
                .iter()
                .map(|x| x.1.bottom())
                .fold(f32::NEG_INFINITY, f32::max);
            (top, bot)
        })
        .collect();

    let mut row_clip_y: Vec<(f32, f32)> = Vec::with_capacity(row_bounds.len());
    for i in 0..row_bounds.len() {
        let (top, bot) = row_bounds[i];
        let y_min = if i == 0 {
            top - 4.0
        } else {
            ((row_bounds[i - 1].1 + top) * 0.5).max(top - 4.0)
        };
        let y_max = if i + 1 >= row_bounds.len() {
            bot + 4.0
        } else {
            ((bot + row_bounds[i + 1].0) * 0.5).min(bot + 4.0)
        };
        row_clip_y.push((y_min, y_max.max(y_min + 1.0)));
    }

    for (r_idx, mut row) in rows.into_iter().enumerate() {
        row.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
        let (cy_min, cy_max) = row_clip_y[r_idx];
        let mut x_lefts: Vec<f32> = Vec::with_capacity(row.len());
        let mut x_rights: Vec<f32> = Vec::with_capacity(row.len());
        for i in 0..row.len() {
            let rc = row[i].1;
            let l = if i == 0 {
                rc.x - 10.0
            } else {
                (row[i - 1].1.right() + rc.x) * 0.5
            };
            let r = if i + 1 >= row.len() {
                rc.right() + 10.0
            } else {
                (rc.right() + row[i + 1].1.x) * 0.5
            };
            x_lefts.push(l);
            x_rights.push(r.max(l + 0.25));
        }
        for (i, (u_idx, rc)) in row.into_iter().enumerate() {
            let cl = x_lefts[i];
            let cr = x_rights[i];
            out[u_idx].push(DisjointUnitCell {
                raw_rect: rc,
                glyph_rect: rc,
                clip_rect: RectF::new(cl, cy_min, cr - cl, cy_max - cy_min),
            });
        }
    }
    out
}

struct CachedLyricLineLayout {
    align: TextAlign,
    main_layout: windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
    bg_layout: Option<windows::Win32::Graphics::DirectWrite::IDWriteTextLayout>,
    trans_layout: Option<windows::Win32::Graphics::DirectWrite::IDWriteTextLayout>,
    main_h: f32,
    total_h: f32,
    bg_dy: f32,
    tr_dy: f32,
    main_font_sz: f32,
    bg_font_sz: f32,
    word_cells_measured: bool,
    char_cells_measured: bool,
    word_cjk_cells: Vec<Vec<DisjointUnitCell>>,
    char_cells: Vec<Vec<DisjointUnitCell>>,
    bg_word_cjk_cells: Vec<Vec<DisjointUnitCell>>,
    bg_char_cells: Vec<Vec<DisjointUnitCell>>,
    word_cjk_sub_glyphs: Vec<Vec<SubGlyphCell>>,
    bg_word_cjk_sub_glyphs: Vec<Vec<SubGlyphCell>>,
}

#[derive(Clone, PartialEq, Eq, Default)]
struct CachedLyricsLayoutKey {
    track_key: String,
    content_hash: u64,
    line_count: usize,
    max_w_q: u16,
    font_sz_tenths: u16,
    show_trans: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct LyricsGlassBgSig {
    valid: bool,
    drew_mesh: bool,
    aurora_bake_seq: u64,
    glass_write_gen: u64,
    in_transition_sheet: bool,
    free_blur: bool,
    win_x_q: i32,
    win_y_q: i32,
    win_w_q: i32,
    win_h_q: i32,
    fallback_phase_q: u32,
    fallback_pal_hash: u64,
}

#[derive(Clone, Copy, Default)]
struct LyricsGlowSlotCache {
    valid: bool,
    line_idx: usize,
    content_hash: u64,
    max_w_q: u16,
    active_w_q: u16,
    flags: u8,
    last_t_sweep: f64,
    last_peak: f32,
    last_baked_at: Option<std::time::Instant>,
}

struct VisibleLineSlot {
    idx: usize,
    origin_x: f32,
    screen_y: f32,
    active_w: f32,
    edge_fade: f32,
    use_vector_layer: bool,
    line_t_sweep: f64,
    bmp_key: LyricsLineBmpKey,
    glow_slot: Option<usize>,
    glow_peak: f32,
}

pub struct LyricsViewState {
    pub animator: LyricsAnimator,
    pub show_translation: bool,
    pub show_lyrics: bool,
    pub show_queue: bool,
    pub queue_tracks: Vec<crate::am_agent::AgentTrackInput>,
    pub queue_scroll_y: f32,
    pub queue_max_scroll_y: f32,
    pub queue_column_rect: RectF,
    pub left_enlarged: bool,
    pub center_axis: bool,
    pub music_free_blur: bool,
    pub source_pref: LyricsSourcePref,
    pub sync_offset_sec: f64,
    pub synced_playhead_sec: f64,
    pub end_latched_playhead: Option<f64>,
    last_track_key: String,
    last_nonzero_playhead_sec: f64,
    was_playing: bool,
    seen_clamp_reset_seq: u64,
    was_showing_queue: bool,
    last_raw_playhead_sec: f64,
    last_calibrated_offset_sec: f64,
    last_offset_track_key: String,
    last_offset_meta_key: String,
    last_offset_title: String,
    last_offset_artist: String,
    cached_lyrics_meta: Option<(usize, usize, usize, i64, i64, u64, bool)>,
    cached_bmp_track_tag: String,
    cached_bmp_track_sig: (usize, u64, bool),
    pub mesh_phase: f32,
    pub smooth_palette: [Color; 4],
    pub cover_scale: f32,
    pub cover_vel: f32,
    /// Nonlinear spring (`0.0` = split left-column layout, `1.0` = centered instrumental/no-lyrics stage)
    pub center_layout_spring: Spring1D,
    pub loading_hold_sec: f32,
    pub hit_zones: Vec<LyricsHitZone>,
    pub lyrics_column_rect: RectF,
    pub progress_bar_rect: RectF,
    pub volume_bar_rect: RectF,
    pub immersive_mode: bool,
    pub immersive_corner_alphas: [f32; 4],
    pub immersive_corner_targets: [f32; 4],
    /// Permanent top-right NetEase Listen Together pill `(in_room, label)` when NetEase backend is active
    pub listen_together_pill: Option<(bool, String)>,
    cached_layout_key: CachedLyricsLayoutKey,
    cached_line_layouts: Vec<CachedLyricLineLayout>,
    last_glass_bg_sig: LyricsGlassBgSig,
    glow_slot_cache: [LyricsGlowSlotCache; 2],
    visible_lines_scratch: Vec<VisibleLineSlot>,
    queue_tracks_rev: u64,
    cached_queue_active_key: (u64, usize, usize, String, String, String),
    cached_queue_active_idx: Option<usize>,
}

impl Default for LyricsViewState {
    fn default() -> Self {
        Self {
            animator: LyricsAnimator::default(),
            show_translation: true,
            show_lyrics: true,
            show_queue: false,
            queue_tracks: Vec::new(),
            queue_scroll_y: 0.0,
            queue_max_scroll_y: 0.0,
            queue_column_rect: RectF::default(),
            left_enlarged: true,
            center_axis: false,
            music_free_blur: false,
            source_pref: LyricsSourcePref::Auto,
            sync_offset_sec: 0.0,
            synced_playhead_sec: 0.0,
            end_latched_playhead: None,
            last_track_key: String::new(),
            last_nonzero_playhead_sec: 0.0,
            was_playing: false,
            seen_clamp_reset_seq: 0,
            was_showing_queue: false,
            last_raw_playhead_sec: -1.0,
            last_calibrated_offset_sec: 0.0,
            last_offset_track_key: String::new(),
            last_offset_meta_key: String::new(),
            last_offset_title: String::new(),
            last_offset_artist: String::new(),
            cached_lyrics_meta: None,
            cached_bmp_track_tag: String::new(),
            cached_bmp_track_sig: (0, 0, false),
            mesh_phase: 0.0,
            smooth_palette: [
                Color::rgb(122, 32, 62),
                Color::rgb(36, 58, 118),
                Color::rgb(88, 34, 108),
                Color::rgb(24, 72, 92),
            ],
            cover_scale: 0.86,
            cover_vel: 0.0,
            center_layout_spring: Spring1D::new(0.0, 1.0, 92.0, 19.8),
            loading_hold_sec: 0.0,
            hit_zones: Vec::with_capacity(64),
            lyrics_column_rect: RectF::default(),
            progress_bar_rect: RectF::default(),
            volume_bar_rect: RectF::default(),
            immersive_mode: false,
            immersive_corner_alphas: [1.0, 1.0, 1.0, 1.0],
            immersive_corner_targets: [1.0, 1.0, 1.0, 1.0],
            listen_together_pill: None,
            cached_layout_key: CachedLyricsLayoutKey::default(),
            cached_line_layouts: Vec::new(),
            last_glass_bg_sig: LyricsGlassBgSig::default(),
            glow_slot_cache: [LyricsGlowSlotCache::default(); 2],
            visible_lines_scratch: Vec::with_capacity(18),
            queue_tracks_rev: 0,
            cached_queue_active_key: (u64::MAX, 0, 0, String::new(), String::new(), String::new()),
            cached_queue_active_idx: None,
        }
    }
}

impl LyricsViewState {
    const QUEUE_ROW_STRIDE: f32 = 60.0;

    /// O(1) arithmetic visible-row slice `[start..end)` for Up Next queue virtualization.
    #[inline]
    pub fn compute_queue_visible_range(
        total_len: usize,
        scroll_y: f32,
        viewport_h: f32,
    ) -> (usize, usize) {
        if total_len == 0 || viewport_h <= 0.0 {
            return (0, 0);
        }
        const ROW_H: f32 = 56.0;
        let stride = Self::QUEUE_ROW_STRIDE;
        let sy = scroll_y.max(0.0);
        let first = ((sy - ROW_H - 4.0).max(0.0) / stride).floor() as usize;
        let last_excl = ((sy + viewport_h + 4.0).max(0.0) / stride).ceil() as usize + 1;
        let start = first.min(total_len);
        let end = last_excl.min(total_len).max(start);
        (start, end)
    }

    /// O(1) cached `(content_hash, is_effectively_instrumental_or_empty)` avoiding per-frame
    /// 150-line hashing & lowercase String allocations (~61.9 ms/s saved at 144Hz!).
    fn lyrics_meta_cached(&mut self, lyrics: Option<&ParsedLyrics>) -> (u64, bool) {
        let Some(lyr) = lyrics else {
            return (0, true);
        };
        let ptr = lyr as *const ParsedLyrics as usize;
        let n = lyr.lines.len();
        let src_len = lyr.source_name.len();
        let first_ms = lyr
            .lines
            .first()
            .map(|l| (l.start_sec * 1000.0).round() as i64)
            .unwrap_or(-1);
        let last_ms = lyr
            .lines
            .last()
            .map(|l| (l.start_sec * 1000.0).round() as i64)
            .unwrap_or(-1);
        if let Some((cp, cn, csl, cf, cl, hash, inst)) = self.cached_lyrics_meta {
            if cp == ptr && cn == n && csl == src_len && cf == first_ms && cl == last_ms {
                return (hash, inst);
            }
        }
        let hash = lyr.content_hash();
        let inst = crate::lyrics::is_effectively_instrumental_or_empty(Some(lyr));
        self.cached_lyrics_meta = Some((ptr, n, src_len, first_ms, last_ms, hash, inst));
        (hash, inst)
    }

    #[inline]
    pub fn set_queue_tracks(&mut self, tracks: Vec<crate::am_agent::AgentTrackInput>) {
        self.queue_tracks = tracks;
        self.queue_tracks_rev = self.queue_tracks_rev.wrapping_add(1);
        self.cached_queue_active_idx = None;
        if self.queue_tracks.is_empty() {
            self.queue_scroll_y = 0.0;
            self.queue_max_scroll_y = 0.0;
            self.was_showing_queue = false;
        } else {
            let est_view_h = if self.queue_column_rect.h > 80.0 {
                (self.queue_column_rect.h - 62.0).max(120.0)
            } else {
                420.0
            };
            let total_h = (self.queue_tracks.len() as f32) * Self::QUEUE_ROW_STRIDE;
            self.queue_max_scroll_y = (total_h - est_view_h).max(0.0);
            self.queue_scroll_y = self.queue_scroll_y.clamp(0.0, self.queue_max_scroll_y);
        }
    }

    pub fn on_queue_wheel(&mut self, px: f32) {
        self.queue_scroll_y = (self.queue_scroll_y - px).clamp(0.0, self.queue_max_scroll_y);
    }

    pub fn clear_end_latch(&mut self) {
        self.end_latched_playhead = None;
        self.last_nonzero_playhead_sec = 0.0;
    }

    pub fn has_active_layout_motion(&self) -> bool {
        let corner_settling = (0..4).any(|i| {
            (self.immersive_corner_alphas[i] - self.immersive_corner_targets[i]).abs() > 0.008
        });
        !self.center_layout_spring.is_settled_norm() || self.cover_vel.abs() > 0.01 || corner_settling
    }

    pub fn step_immersive_corners(
        &mut self,
        win_rect: RectF,
        hovered_pt: (f32, f32),
        mouse_idle_sec: f32,
        immersive: bool,
        force_visible: bool,
        dt_sec: f32,
        reduce_motion: bool,
    ) {
        self.immersive_mode = immersive;
        let targets = eval_immersive_corner_targets(
            win_rect,
            hovered_pt,
            mouse_idle_sec,
            immersive,
            force_visible,
        );
        self.immersive_corner_targets = targets;
        if reduce_motion {
            self.immersive_corner_alphas = targets;
        } else {
            let dt = dt_sec.clamp(0.001, 0.05);
            let k_in = (1.0 - (-16.0 * dt).exp()).clamp(0.05, 1.0);
            let k_out = (1.0 - (-8.5 * dt).exp()).clamp(0.03, 1.0);
            for i in 0..4 {
                let k = if targets[i] > self.immersive_corner_alphas[i] {
                    k_in
                } else {
                    k_out
                };
                self.immersive_corner_alphas[i] +=
                    (targets[i] - self.immersive_corner_alphas[i]) * k;
                if (self.immersive_corner_alphas[i] - targets[i]).abs() < 0.004 {
                    self.immersive_corner_alphas[i] = targets[i];
                }
            }
        }
    }

    #[inline]
    fn calibrated_playhead(&self, raw_playhead_sec: f64) -> f64 {
        let acoustic_lat = if raw_playhead_sec > 0.0 {
            let bin_lat = crate::binaural_dsp::fast_binaural_latency_sec();
            if bin_lat > 0.0 {
                bin_lat
            } else {
                0.010
            }
        } else {
            0.0
        };
        (raw_playhead_sec - acoustic_lat + self.sync_offset_sec).max(0.0)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        lyrics: Option<&ParsedLyrics>,
        lyrics_loading: bool,
        track_key: &str,
        playhead_sec: f64,
        is_playing: bool,
        bass_energy: f32,
        target_palette: Option<[Color; 4]>,
        viewport_h: f32,
        dt_sec: f32,
        reduce_motion: bool,
        latin_by_word: bool,
    ) {
        if !track_key.is_empty() && self.last_track_key != track_key {
            let prev = self.last_track_key.as_str();
            let same_meta_base = prev.starts_with("meta:")
                && track_key.starts_with("meta:")
                && prev[5..].split('#').next() == track_key[5..].split('#').next();
            let id_upgrade = (prev.starts_with("meta:") && track_key.starts_with("id:"))
                || (prev.starts_with("id:i.") && track_key.starts_with("id:"));
            if !prev.is_empty() && !same_meta_base && !id_upgrade {
                self.end_latched_playhead = None;
                self.last_nonzero_playhead_sec = 0.0;
                self.was_playing = false;
            }
            self.last_track_key = track_key.to_string();
        }
        if !track_key.is_empty() && self.last_offset_track_key != track_key {
            if self.last_offset_meta_key.is_empty() {
                let prev = self.last_offset_track_key.as_str();
                let same_meta_base = prev.starts_with("meta:")
                    && track_key.starts_with("meta:")
                    && prev[5..].split('#').next() == track_key[5..].split('#').next();
                let id_upgrade = (prev.starts_with("meta:") && track_key.starts_with("id:"))
                    || (prev.starts_with("id:i.") && track_key.starts_with("id:"));
                if !prev.is_empty() && !same_meta_base && !id_upgrade {
                    self.sync_offset_sec = 0.0;
                }
            } else if let Some(rest) = track_key.strip_prefix("meta:") {
                let tk_meta = rest.split('#').next().unwrap_or("");
                if !tk_meta.is_empty() && tk_meta != self.last_offset_meta_key {
                    self.sync_offset_sec = 0.0;
                    self.last_offset_meta_key = tk_meta.to_string();
                }
            }
            self.last_offset_track_key = track_key.to_string();
        }
        self.animator.latin_by_word = latin_by_word;
        let dt = dt_sec.clamp(0.001, 0.05);
        let speed = if is_playing {
            0.34 + bass_energy * 0.65
        } else {
            0.08
        };
        // Always advance flowing aurora phase so「减弱动态效果」/「降低透明度」never freeze the lyrics aurora!
        self.mesh_phase = (self.mesh_phase + dt * speed) % 1000.0;

        if let Some(pal) = target_palette {
            let k = (dt * 2.4).clamp(0.01, 0.25);
            for i in 0..4 {
                self.smooth_palette[i] = self.smooth_palette[i].lerp(pal[i], k);
            }
        }

        // Apple Music cover spring: 1.00 when playing, 0.83 when paused
        let cov_target = if is_playing { 1.0 } else { 0.83 };
        let cov_force = -135.0 * (self.cover_scale - cov_target) - 18.0 * self.cover_vel;
        self.cover_vel += cov_force * dt;
        self.cover_scale = (self.cover_scale + self.cover_vel * dt).clamp(0.75, 1.06);

        // Smart centering spring:
        // - Queue open -> NEVER center cover (split layout for right-column Up Next queue!)
        // - Lyrics explicitly hidden (and queue closed) -> smoothly center album cover + controls!
        // - Otherwise -> hold split up to 6.0s while fetching, then center if instrumental/no lyrics
        if self.show_queue {
            self.loading_hold_sec = 0.0;
            self.center_layout_spring.target = 0.0;
        } else if !self.show_lyrics {
            self.loading_hold_sec = 0.0;
            self.center_layout_spring.target = 1.0;
        } else if lyrics_loading && lyrics.is_none() && self.loading_hold_sec < 6.0 {
            self.loading_hold_sec += dt;
            self.center_layout_spring.target = 0.0;
        } else {
            if !lyrics_loading {
                self.loading_hold_sec = 0.0;
            }
            let (_, is_no_lyrics) = self.lyrics_meta_cached(lyrics);
            self.center_layout_spring.target = if is_no_lyrics { 1.0 } else { 0.0 };
        }
        if reduce_motion {
            self.center_layout_spring.snap(self.center_layout_spring.target);
        } else {
            self.center_layout_spring.step(dt);
        }

        // Detect external user seek / scrub / prev-next reset before our own offset clamp check
        let external_seek = self.animator.clamp_reset_seq() != self.seen_clamp_reset_seq;
        if external_seek {
            self.end_latched_playhead = None;
            self.last_nonzero_playhead_sec = playhead_sec;
        }

        // End-of-playlist scroll-back latch:
        // When a playlist finishes and playback stops on the same track, SMTC/agent often drops
        // `position_sec` back to `0.0`. Latch the end playhead so lyrics rest serenely at the
        // final line instead of scrolling all the way back to line 0, and release immediately
        // on explicit seek/scrub, play resume, or track change.
        let effective_playhead = if is_playing {
            if self.end_latched_playhead.is_some()
                || playhead_sec > 0.05
                || self.last_nonzero_playhead_sec <= 1.5
            {
                self.last_nonzero_playhead_sec = playhead_sec;
            }
            self.end_latched_playhead = None;
            playhead_sec
        } else if playhead_sec > 1.5 {
            self.last_nonzero_playhead_sec = playhead_sec;
            self.end_latched_playhead = None;
            playhead_sec
        } else if playhead_sec <= 1.2 {
            if let Some(latched) = self.end_latched_playhead {
                if playhead_sec > 0.05 && (playhead_sec - self.last_raw_playhead_sec).abs() > 0.05
                {
                    self.end_latched_playhead = None;
                    self.last_nonzero_playhead_sec = playhead_sec;
                    playhead_sec
                } else {
                    latched
                }
            } else {
                let near_lyrics_end = lyrics
                    .and_then(|l| l.lines.last())
                    .map(|last_line| {
                        self.last_nonzero_playhead_sec > 1.5
                            && self.last_nonzero_playhead_sec
                                >= (last_line.start_sec - 4.0).max(1.5)
                    })
                    .unwrap_or(false);
                let abrupt_stop_snap =
                    self.was_playing && self.last_nonzero_playhead_sec >= 12.0;
                if !external_seek && (near_lyrics_end || abrupt_stop_snap) {
                    self.end_latched_playhead = Some(self.last_nonzero_playhead_sec);
                    self.last_nonzero_playhead_sec
                } else {
                    playhead_sec
                }
            }
        } else {
            playhead_sec
        };
        self.was_playing = is_playing;

        if (self.sync_offset_sec - self.last_calibrated_offset_sec).abs() > 1e-5 {
            self.animator.reset_word_clamps();
        }
        self.seen_clamp_reset_seq = self.animator.clamp_reset_seq();
        self.last_raw_playhead_sec = playhead_sec;
        self.last_calibrated_offset_sec = self.sync_offset_sec;
        self.synced_playhead_sec = self.calibrated_playhead(effective_playhead);
        if let Some(lyr) = lyrics {
            self.animator.bind_lyrics(track_key, lyr);
            self.animator
                .step(lyr, self.synced_playhead_sec, viewport_h, dt);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        r: &mut D2DRenderer,
        win_rect: RectF,
        smtc: &SmtcState,
        artwork_key: Option<&str>,
        lyrics: Option<&ParsedLyrics>,
        lyrics_key: &str,
        lyrics_loading: bool,
        playback_loading: bool,
        spinner_phase: f32,
        bass_energy: f32,
        shuffle_on: bool,
        repeat_on: bool,
        hovered_pt: (f32, f32),
        mouse_down: bool,
        is_maximized: bool,
        reduce_motion: bool,
        _reduce_transparency: bool,
        glass_off: bool,
        latin_by_word: bool,
        non_emph_fx: NonEmphasisLyricsEffect,
        auto_emphasis: bool,
        frontier_wave: bool,
        end_hold_glow: bool,
        sheet_alpha: f32,
        motion_frame: Option<&MotionFrameSnapshot>,
    ) {
        let sa = sheet_alpha.clamp(0.0, 1.0);
        self.animator.latin_by_word = latin_by_word;
        self.hit_zones.clear();
        self.progress_bar_rect = RectF::default();
        self.volume_bar_rect = RectF::default();

        let title_trim = smtc.title.trim();
        let artist_trim = smtc.artist.trim();
        if !title_trim.is_empty() {
            let meta_changed = !self.last_offset_title.is_empty()
                && (!self.last_offset_title.eq_ignore_ascii_case(title_trim)
                    || !self.last_offset_artist.eq_ignore_ascii_case(artist_trim));
            if meta_changed {
                self.sync_offset_sec = 0.0;
                self.end_latched_playhead = None;
                self.last_nonzero_playhead_sec = 0.0;
                self.was_playing = false;
                self.last_raw_playhead_sec = smtc.position_sec;
                self.last_calibrated_offset_sec = 0.0;
                self.synced_playhead_sec = self.calibrated_playhead(smtc.position_sec);
            }
            if !self.last_offset_title.eq_ignore_ascii_case(title_trim)
                || !self.last_offset_artist.eq_ignore_ascii_case(artist_trim)
            {
                self.last_offset_title.clear();
                self.last_offset_title.push_str(title_trim);
                self.last_offset_artist.clear();
                self.last_offset_artist.push_str(artist_trim);
                self.last_offset_meta_key = format!(
                    "{}|{}",
                    title_trim.to_lowercase(),
                    artist_trim.to_lowercase()
                );
            }
        }

        let (lyr_hash, is_lyrics_instrumental) = self.lyrics_meta_cached(lyrics);
        let bmp_sig = (lyrics_key.len(), lyr_hash, self.show_translation);
        if self.cached_bmp_track_sig != bmp_sig
            || !self.cached_bmp_track_tag.starts_with(lyrics_key)
        {
            self.cached_bmp_track_tag = format!(
                "{}#h:{:016x}#tr:{}",
                lyrics_key, lyr_hash, self.show_translation as u8
            );
            self.cached_bmp_track_sig = bmp_sig;
        }
        r.set_lyrics_bmp_track(&self.cached_bmp_track_tag);

        // 1. True GPU Rotating Album-Artwork Mesh Background (1:1 with LyricsBlossom & MusicFreeBlur!)
        // Always preserved even when「降低透明度」is enabled so UI opacity never disables Lyrics aurora!
        let drew_mesh = r.draw_lyrics_artwork_mesh_bg(
            win_rect,
            artwork_key,
            self.mesh_phase,
            bass_energy,
            &self.smooth_palette,
            self.music_free_blur,
        );

        if !drew_mesh {
            let base_bg = self.smooth_palette[0].lerp(Color::rgb(8, 8, 12), 0.72);
            r.fill_rect(win_rect, base_bg);

            let w = win_rect.w;
            let h = win_rect.h;
            let p = self.mesh_phase;
            let breath = 1.0 + bass_energy * 0.18;
            let radius_mul = if self.music_free_blur { 0.56 } else { 0.88 };

            let orbs = [
                (
                    w * (0.24 + 0.16 * (p * 0.7).cos()),
                    h * (0.28 + 0.18 * (p * 0.5).sin()),
                    w.max(h) * 0.62 * radius_mul * breath,
                    self.smooth_palette[0].with_alpha(0.76),
                ),
                (
                    w * (0.76 + 0.15 * (p * 0.55 + 1.4).sin()),
                    h * (0.24 + 0.16 * (p * 0.8 + 0.7).cos()),
                    w.max(h) * 0.58 * radius_mul,
                    self.smooth_palette[1].with_alpha(0.68),
                ),
                (
                    w * (0.32 + 0.18 * (p * 0.45 + 2.3).sin()),
                    h * (0.78 + 0.14 * (p * 0.65).cos()),
                    w.max(h) * 0.64 * radius_mul * breath,
                    self.smooth_palette[2].with_alpha(0.64),
                ),
                (
                    w * (0.74 + 0.14 * (p * 0.6 + 3.8).cos()),
                    h * (0.76 + 0.16 * (p * 0.5 + 1.9).sin()),
                    w.max(h) * 0.56 * radius_mul,
                    self.smooth_palette[3].with_alpha(0.62),
                ),
            ];

            for (ox, oy, orad, ocol) in orbs {
                r.fill_radial_glow(ox, oy, orad, orad * 0.92, ocol, ocol.with_alpha(0.0));
            }
        }

        // Optical contrast veil so white glowing lyrics pop cleanly over vibrant covers
        let veil_alpha = if self.music_free_blur { 0.24 } else { 0.34 };
        r.fill_rect(win_rect, Color::rgba(7, 7, 12, veil_alpha));

        // Unified Top-Bar Geometry (`Y = 14.0`, `H = 34.0`, `R = 17.0` -> `center_y = 31.0`)
        let top_y = win_rect.y + Theme::TOP_BAR_Y;
        let btn_h = Theme::TOP_BAR_H;
        let btn_r = Theme::TOP_BAR_RADIUS;
        let [a_tl, a_tr, _a_bl, a_br] = self.immersive_corner_alphas;

        // Precompute exact corner button-cluster bounds for tight X-ROI glass strips (P0-8:
        // only refresh strips when the 30 Hz aurora/background or layout/visibility actually changed!).
        let close_btn = RectF::new(win_rect.x + 20.0, top_y, btn_h, btn_h);
        let close_x = win_rect.right() - 18.0 - btn_h;
        let max_x = close_x - 8.0 - btn_h;
        let min_x = max_x - 8.0 - btn_h;
        let lt_pill_w = match self.listen_together_pill {
            Some((true, _)) => 126.0f32,
            Some((false, _)) => 78.0f32,
            None => 0.0f32,
        };
        let tr_left = if lt_pill_w > 0.0 {
            min_x - 12.0 - lt_pill_w
        } else {
            min_x
        };
        const BR_PILL_W: f32 = 5.0 * 2.0 + 32.0 * 3.0 + 6.0 * 2.0; // 118.0 DIP
        const BR_PILL_H: f32 = 32.0 + 5.0 * 2.0; // 42.0 DIP

        let use_lyrics_glass_batch = !glass_off && (a_tl > 0.02 || a_tr > 0.02 || a_br > 0.02);
        if use_lyrics_glass_batch {
            let mut top_spans = [(0.0f32, 0.0f32); 2];
            let mut top_span_cnt = 0u8;
            if a_tl > 0.02 {
                top_spans[top_span_cnt as usize] = (close_btn.x - 8.0, close_btn.right() + 8.0);
                top_span_cnt += 1;
            }
            if a_tr > 0.02 {
                top_spans[top_span_cnt as usize] = (tr_left - 8.0, close_x + btn_h + 8.0);
                top_span_cnt += 1;
            }
            let bot_span = if a_br > 0.02 {
                let br_x = win_rect.right() - BR_PILL_W - 24.0;
                Some((br_x - 8.0, br_x + BR_PILL_W + 8.0))
            } else {
                None
            };
            let top_strip_dip = if top_span_cnt > 0 {
                (top_y + btn_h + 12.0).max(60.0)
            } else {
                0.0
            };
            let bot_strip_dip = if a_br > 0.02 {
                let pill_top = win_rect.bottom() - BR_PILL_H - 18.0;
                (r.logical_height() - pill_top + 12.0).max(BR_PILL_H + 30.0)
            } else {
                0.0
            };

            let fallback_pal_hash = if drew_mesh {
                0u64
            } else {
                let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                for c in &self.smooth_palette {
                    for ch in [c.r, c.g, c.b] {
                        let q = (ch.clamp(0.0, 1.0) * 255.0).round() as u64;
                        h ^= q;
                        h = h.wrapping_mul(0x0100_0000_01b3);
                    }
                }
                let bq = (bass_energy.clamp(0.0, 2.0) * 100.0).round() as u64;
                h ^= bq;
                h.wrapping_mul(0x0100_0000_01b3)
            };
            let mut cur_sig = LyricsGlassBgSig {
                valid: true,
                drew_mesh,
                aurora_bake_seq: r.lyrics_aurora_bake_seq(),
                glass_write_gen: r.glass_snapshot_write_gen(),
                in_transition_sheet: r.is_in_lyrics_transition_sheet(),
                free_blur: self.music_free_blur,
                win_x_q: (win_rect.x * 2.0).round() as i32,
                win_y_q: (win_rect.y * 2.0).round() as i32,
                win_w_q: (win_rect.w * 2.0).round() as i32,
                win_h_q: (win_rect.h * 2.0).round() as i32,
                fallback_phase_q: if drew_mesh {
                    0
                } else {
                    (self.mesh_phase * 100.0).round() as u32
                },
                fallback_pal_hash,
            };
            let dirty_mask = if self.last_glass_bg_sig == cur_sig {
                0
            } else {
                BACKDROP_DIRTY_ALL
            };
            r.set_glass_snapshot_x_rois(top_spans, top_span_cnt, bot_span);
            r.begin_glass_backdrop_snapshot_strips_ex(
                false,
                top_strip_dip,
                bot_strip_dip,
                None,
                dirty_mask,
            );
            cur_sig.glass_write_gen = r.glass_snapshot_write_gen();
            self.last_glass_bg_sig = cur_sig;
        } else {
            self.last_glass_bg_sig.valid = false;
        }

        // Top-Left Close Chevron Circle (hidden cleanly in Immersive Mode until mouse nears top-left)
        if a_tl > 0.02 {
            let pushed_tl = if a_tl < 0.995 {
                r.push_opacity_layer(close_btn.inset(-14.0, -14.0), a_tl)
            } else {
                false
            };
            let close_hov = close_btn.contains(hovered_pt.0, hovered_pt.1);
            r.draw_liquid_glass_floating_button(
                close_btn,
                btn_r,
                GlassBtnVariant::Regular,
                close_hov,
                mouse_down,
                hovered_pt,
                glass_off,
            );
            r.draw_icon(SfIcon::ChevronDown, close_btn.inset(9.0, 9.0), Theme::TEXT_PRIMARY);
            if pushed_tl {
                r.pop_layer();
            }
            if a_tl > 0.20 {
                self.hit_zones.push(LyricsHitZone {
                    rect: close_btn,
                    action: LyricsHitAction::CloseLyrics,
                    line_idx: None,
                });
            }
        }

        // Top-Right Unified Window Controls (`─`, `□`, `×`) + permanent NetEase `一起听` pill
        if a_tr > 0.02 {
            let tr_bounds = RectF::new(
                tr_left - 14.0,
                top_y - 12.0,
                (close_x + btn_h - tr_left) + 28.0,
                btn_h + 24.0,
            );
            let pushed_tr = if a_tr < 0.995 {
                r.push_opacity_layer(tr_bounds, a_tr)
            } else {
                false
            };
            if let Some((lt_active, ref lt_lbl)) = self.listen_together_pill.clone() {
                let lt_r = RectF::new(min_x - 12.0 - lt_pill_w, top_y, lt_pill_w, btn_h);
                let lt_hov = lt_r.contains(hovered_pt.0, hovered_pt.1);
                r.draw_liquid_glass_floating_button(
                    lt_r,
                    btn_r,
                    if lt_active {
                        GlassBtnVariant::Tinted(Color::rgb(236, 65, 65))
                    } else {
                        GlassBtnVariant::Regular
                    },
                    lt_hov,
                    mouse_down,
                    hovered_pt,
                    glass_off,
                );
                if lt_active {
                    r.fill_circle(lt_r.x + 13.0, lt_r.center_y(), 3.6, Theme::SIGNAL_GREEN);
                    r.draw_text(
                        lt_lbl,
                        RectF::new(lt_r.x + 21.0, lt_r.y, lt_r.w - 27.0, lt_r.h),
                        12.0,
                        600,
                        Theme::TEXT_PRIMARY,
                        TextAlign::Left,
                        true,
                        false,
                    );
                } else {
                    r.draw_text(
                        lt_lbl,
                        lt_r.inset(6.0, 0.0),
                        12.2,
                        if lt_hov { 600 } else { 500 },
                        Theme::TEXT_PRIMARY,
                        TextAlign::Center,
                        true,
                        false,
                    );
                }
                if a_tr > 0.20 {
                    self.hit_zones.push(LyricsHitZone {
                        rect: lt_r,
                        action: LyricsHitAction::OpenListenTogether,
                        line_idx: None,
                    });
                }
            }
            for (wx, wic, wact, is_cls) in [
                (min_x, SfIcon::WinMinimize, LyricsHitAction::WinMinimize, false),
                (
                    max_x,
                    if is_maximized {
                        SfIcon::WinRestore
                    } else {
                        SfIcon::WinMaximize
                    },
                    LyricsHitAction::WinToggleMaximize,
                    false,
                ),
                (close_x, SfIcon::WinClose, LyricsHitAction::WinClose, true),
            ] {
                let wb = RectF::new(wx, top_y, btn_h, btn_h);
                let whov = wb.contains(hovered_pt.0, hovered_pt.1);
                r.draw_liquid_glass_floating_button(
                    wb,
                    btn_r,
                    if is_cls && whov {
                        GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                    } else {
                        GlassBtnVariant::Regular
                    },
                    whov,
                    mouse_down,
                    hovered_pt,
                    glass_off,
                );
                r.draw_icon(wic, wb.inset(9.5, 9.5), Theme::TEXT_PRIMARY);
                if a_tr > 0.20 {
                    self.hit_zones.push(LyricsHitZone {
                        rect: wb,
                        action: wact,
                        line_idx: None,
                    });
                }
            }
            if pushed_tr {
                r.pop_layer();
            }
        }

        // Responsive Viewport Scale (scales proportionally on large / maximized / 4K displays!)
        let ui_scale = ((win_rect.w / 1180.0).min(win_rect.h / 760.0)).clamp(0.88, 1.80);
        let pad_x = (36.0 * ui_scale).clamp(28.0, 76.0);
        let center_t = self.center_layout_spring.value.clamp(0.0, 1.0);

        // Split View Layout: supports `left_enlarged` (default true) and `center_axis` (default false)
        let (left_col, right_col) = if self.center_axis {
            // Midline alignment: right column (lyrics / queue) left alignment line sits strictly on `win_rect.center_x()`
            let left_half_x = win_rect.x + pad_x;
            let left_half_w = (win_rect.center_x() - 24.0 - left_half_x).max(220.0);
            let lc = RectF::new(
                left_half_x,
                win_rect.y + 58.0,
                left_half_w,
                (win_rect.h - 98.0).max(220.0),
            );
            let rc = RectF::new(
                win_rect.center_x(),
                win_rect.y + 54.0,
                (win_rect.right() - win_rect.center_x() - pad_x * 0.75).max(240.0),
                (win_rect.h - 108.0).max(200.0),
            );
            (lc, rc)
        } else {
            let left_w = if self.left_enlarged {
                (win_rect.w * 0.46).clamp(340.0, 920.0)
            } else {
                (win_rect.w * 0.43).clamp(300.0, 860.0)
            };
            let lc = RectF::new(
                win_rect.x + pad_x,
                win_rect.y + 58.0,
                (left_w - pad_x * 1.35).max(220.0),
                (win_rect.h - 98.0).max(220.0),
            );
            let rc = RectF::new(
                win_rect.x + left_w + 8.0,
                win_rect.y + 54.0,
                (win_rect.w - left_w - pad_x * 0.85 - 8.0).max(260.0),
                (win_rect.h - 108.0).max(200.0),
            );
            (lc, rc)
        };

        if self.show_queue {
            self.queue_column_rect = right_col;
            self.lyrics_column_rect = RectF::default();
        } else {
            self.queue_column_rect = RectF::default();
            self.lyrics_column_rect = if center_t > 0.85 || !self.show_lyrics {
                RectF::default()
            } else {
                right_col
            };
        }

        // --- Draw Left Column (or Centered Stage when Lyrics Hidden / Instrumental): ---
        let avail_h = (left_col.h - 12.0).max(240.0);
        let mut ctrl_scale = if self.left_enlarged {
            (ui_scale * 1.14).clamp(1.02, 1.62)
        } else {
            ui_scale.clamp(0.92, 1.45)
        };
        if self.left_enlarged {
            let max_ctrl_scale = ((avail_h - 190.0) / 194.0).clamp(0.84, 1.62);
            ctrl_scale = ctrl_scale.min(max_ctrl_scale);
        }
        let controls_h = 194.0 * ctrl_scale;
        let max_cov_for_h = (avail_h - controls_h).max(160.0);
        let base_cov = if self.left_enlarged {
            (left_col.w * 0.90)
                .min(left_col.h * 0.62)
                .clamp(210.0, 700.0)
        } else {
            (left_col.w * 0.86)
                .min(left_col.h * 0.54)
                .clamp(180.0, 620.0)
        };
        let max_cov_w = left_col.w * (1.0 - center_t) + (win_rect.w * 0.65) * center_t;
        let max_cov = (base_cov * (1.0 + 0.07 * center_t))
            .min(max_cov_for_h)
            .min(max_cov_w.max(180.0));
        let cov_cx = left_col.center_x() * (1.0 - center_t) + win_rect.center_x() * center_t;

        // Vertically center the [cover + title/artist + scrubber + transport + volume] stack!
        let title_sz = (19.5 * ctrl_scale).clamp(18.0, 28.0);
        let artist_sz = (15.0 * ctrl_scale).clamp(14.0, 21.0);
        let gap_below_cov = 22.0 * ctrl_scale;
        let title_step = 26.0 * ctrl_scale;
        let artist_step = 28.0 * ctrl_scale;
        let scrub_step = 42.0 * ctrl_scale;
        let ctrl_row_h = 52.0 * ctrl_scale;
        let vol_row_h = 24.0 * ctrl_scale;
        let total_stack_h = max_cov
            + gap_below_cov
            + title_step
            + artist_step
            + scrub_step
            + ctrl_row_h
            + vol_row_h;
        let stack_top = (left_col.center_y() - total_stack_h * 0.5).max(left_col.y + 6.0);
        let cov_cy = stack_top + max_cov * 0.5;
        let scaled_cov = max_cov * self.cover_scale;
        let cov_radius = (16.0 * ctrl_scale).clamp(14.0, 24.0);
        let cov_rect = RectF::new(
            cov_cx - scaled_cov * 0.5,
            cov_cy - scaled_cov * 0.5,
            scaled_cov,
            scaled_cov,
        );

        // Soft ambient drop shadow beneath album cover
        r.fill_radial_glow(
            cov_cx,
            cov_cy + scaled_cov * 0.14,
            scaled_cov * 0.70,
            scaled_cov * 0.66,
            Color::rgba(0, 0, 0, 0.56),
            Color::rgba(0, 0, 0, 0.0),
        );

        let drew_motion = if !reduce_motion {
            motion_frame
                .map(|mf| r.draw_motion_artwork(mf, cov_rect, cov_radius, sa))
                .unwrap_or(false)
        } else {
            false
        };
        let drew_art = drew_motion
            || artwork_key
                .map(|k| r.draw_rounded_artwork(k, true, cov_rect, cov_radius, sa))
                .unwrap_or(false);
        if !drew_art {
            r.fill_rounded_rect(cov_rect, cov_radius, Color::rgba(255, 255, 255, 0.10));
            r.draw_icon(
                SfIcon::MusicNote,
                cov_rect.inset(scaled_cov * 0.30, scaled_cov * 0.30),
                Color::rgba(255, 255, 255, 0.45),
            );
        }
        if playback_loading {
            r.fill_rounded_rect(cov_rect, cov_radius, Color::rgba(8, 8, 14, 0.46));
            r.draw_loading_spinner(
                cov_rect.center_x(),
                cov_rect.center_y(),
                (scaled_cov * 0.10).clamp(16.0, 32.0),
                spinner_phase,
                Color::rgba(255, 255, 255, 0.95),
            );
        }
        r.stroke_rounded_rect(cov_rect, cov_radius, Color::rgba(255, 255, 255, 0.16), 1.0);

        let meta_x = cov_cx - max_cov * 0.5;
        let meta_w = max_cov;
        let mut my = cov_cy + max_cov * 0.5 + gap_below_cov;

        let title = if smtc.title.is_empty() {
            "未在播放"
        } else {
            &smtc.title
        };
        let artist = if playback_loading {
            "正在连接音源准备播放…"
        } else if smtc.artist.is_empty() {
            "选择曲目开始聆听"
        } else {
            &smtc.artist
        };
        let dots_sz = (30.0 * ctrl_scale).clamp(28.0, 38.0);
        r.draw_text(
            title,
            RectF::new(meta_x, my, meta_w - dots_sz - 10.0, title_sz + 8.0),
            title_sz,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        // Clean borderless `•••` icon aligned with title/artist (ZERO circular border!)
        let dots_btn = RectF::new(meta_x + meta_w - dots_sz, my + 5.0 * ctrl_scale, dots_sz, dots_sz);
        let dots_hov = dots_btn.contains(hovered_pt.0, hovered_pt.1);
        if dots_hov && mouse_down {
            r.fill_rounded_rect(dots_btn, dots_sz * 0.5, Color::rgba(255, 255, 255, 0.12));
        }
        let dots_scale = if dots_hov && mouse_down {
            0.93
        } else if dots_hov {
            1.08
        } else {
            1.0
        };
        let dots_ic_sz = dots_sz * 0.54 * dots_scale;
        let dots_ic_rect = RectF::new(
            dots_btn.center_x() - dots_ic_sz * 0.5,
            dots_btn.center_y() - dots_ic_sz * 0.5,
            dots_ic_sz,
            dots_ic_sz,
        );
        r.draw_icon(
            SfIcon::MoreDots,
            dots_ic_rect,
            if dots_hov {
                Theme::TEXT_PRIMARY
            } else {
                Color::rgba(255, 255, 255, 0.68)
            },
        );
        self.hit_zones.push(LyricsHitZone {
            rect: dots_btn,
            action: LyricsHitAction::OpenMoreMenu(dots_btn),
            line_idx: None,
        });

        my += title_step;
        r.draw_text(
            artist,
            RectF::new(meta_x, my, meta_w - dots_sz - 10.0, artist_sz + 8.0),
            artist_sz,
            500,
            Color::rgba(255, 255, 255, 0.66),
            TextAlign::Left,
            true,
            false,
        );
        my += artist_step;

        // Interactive Progress Scrubber
        let prog_bar = RectF::new(meta_x, my, meta_w, 16.0 * ctrl_scale);
        let track_h = if prog_bar.contains(hovered_pt.0, hovered_pt.1) {
            7.5 * ctrl_scale
        } else {
            5.2 * ctrl_scale
        };
        let track_r = RectF::new(meta_x, my + (prog_bar.h - track_h) * 0.5, meta_w, track_h);
        self.progress_bar_rect = track_r;
        r.fill_rounded_rect(track_r, track_h * 0.5, Color::rgba(255, 255, 255, 0.22));
        let dur = smtc.duration_sec.max(0.0);
        let pos = smtc.position_sec.clamp(0.0, dur.max(0.01));
        let ratio = if dur > 0.5 {
            (pos / dur).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        if playback_loading {
            let shim_w = (track_r.w * 0.28).max(24.0);
            let shim_x = track_r.x + (track_r.w - shim_w) * (spinner_phase.rem_euclid(1.0));
            r.fill_rounded_rect(
                RectF::new(shim_x, track_r.y, shim_w, track_r.h),
                track_h * 0.5,
                Color::rgba(255, 255, 255, 0.85),
            );
        } else if ratio > 0.002 {
            r.fill_rounded_rect(
                RectF::new(track_r.x, track_r.y, (track_r.w * ratio).max(track_h), track_r.h),
                track_h * 0.5,
                Color::rgba(255, 255, 255, 0.92),
            );
        }
        if dur > 0.5 {
            self.hit_zones.push(LyricsHitZone {
                rect: prog_bar.inset(0.0, -5.0),
                action: LyricsHitAction::ScrubProgressBar(track_r),
                line_idx: None,
            });
        }
        my += 18.0 * ctrl_scale;
        let time_sz = (11.2 * ctrl_scale).clamp(11.0, 14.5);
        r.draw_mono_text_volatile(
            &format_mmss(pos),
            RectF::new(meta_x, my, 76.0, 18.0),
            time_sz,
            500,
            Color::rgba(255, 255, 255, 0.54),
            TextAlign::Left,
            true,
        );
        let rem = (dur - pos).max(0.0);
        r.draw_mono_text_volatile(
            &format!("-{}", format_mmss(rem)),
            RectF::new(meta_x + meta_w - 84.0, my, 84.0, 18.0),
            time_sz,
            500,
            Color::rgba(255, 255, 255, 0.54),
            TextAlign::Right,
            true,
        );
        my += 24.0 * ctrl_scale;

        // 5-Button Borderless Apple Music Transport Row: Shuffle / Prev / Play-Pause / Next / Repeat
        let is_playing = smtc.status == "Playing";
        let ctrl_cy = my + 20.0 * ctrl_scale;
        let outer_dx = (meta_w * 0.44).clamp(96.0, 210.0);
        let inner_dx = (meta_w * 0.22).clamp(50.0, 104.0);
        let sz_sm = (32.0 * ctrl_scale).clamp(32.0, 44.0);
        let sz_md = (40.0 * ctrl_scale).clamp(40.0, 56.0);
        let sz_lg = (48.0 * ctrl_scale).clamp(48.0, 68.0);

        for (cx_pos, sz, ic, tint_active, is_center_play, act) in [
            (
                cov_cx - outer_dx,
                sz_sm,
                SfIcon::Shuffle,
                shuffle_on,
                false,
                LyricsHitAction::ToggleShuffle,
            ),
            (
                cov_cx - inner_dx,
                sz_md,
                SfIcon::Prev,
                false,
                false,
                LyricsHitAction::PrevTrack,
            ),
            (
                cov_cx,
                sz_lg,
                if is_playing {
                    SfIcon::Pause
                } else {
                    SfIcon::Play
                },
                false,
                true,
                LyricsHitAction::TogglePlayPause,
            ),
            (
                cov_cx + inner_dx,
                sz_md,
                SfIcon::Next,
                false,
                false,
                LyricsHitAction::NextTrack,
            ),
            (
                cov_cx + outer_dx,
                sz_sm,
                SfIcon::Repeat,
                repeat_on,
                false,
                LyricsHitAction::ToggleRepeat,
            ),
        ] {
            let br = RectF::new(cx_pos - sz * 0.5, ctrl_cy - sz * 0.5, sz, sz);
            let bhov = br.contains(hovered_pt.0, hovered_pt.1);
            let b_scale = if bhov && mouse_down {
                0.93
            } else if bhov {
                1.08
            } else {
                1.0
            };
            if is_center_play && playback_loading {
                r.draw_loading_spinner(
                    br.center_x(),
                    br.center_y(),
                    sz * 0.36,
                    spinner_phase,
                    Theme::TEXT_PRIMARY,
                );
            } else {
                let ic_col = if tint_active || bhov {
                    Theme::TEXT_PRIMARY
                } else if is_center_play {
                    Color::rgba(255, 255, 255, 0.94)
                } else if sz > sz_sm + 2.0 {
                    Color::rgba(255, 255, 255, 0.86)
                } else {
                    Color::rgba(255, 255, 255, 0.56)
                };
                let ic_sz = sz * 0.56 * b_scale;
                let ic_rect = RectF::new(
                    br.center_x() - ic_sz * 0.5,
                    br.center_y() - ic_sz * 0.5,
                    ic_sz,
                    ic_sz,
                );
                r.draw_icon(ic, ic_rect, ic_col);
                if tint_active {
                    // Subtle 4px active indicator dot underneath Shuffle / Repeat (ZERO circular border!)
                    r.fill_circle(
                        cx_pos,
                        ctrl_cy + sz * 0.44,
                        2.0,
                        Color::rgba(255, 255, 255, 0.92),
                    );
                }
            }
            self.hit_zones.push(LyricsHitZone {
                rect: br,
                action: act,
                line_idx: None,
            });
        }
        my += ctrl_row_h;

        // Interactive Volume Slider
        if my + 16.0 < win_rect.bottom() - 16.0 {
            let vol = smtc.volume.unwrap_or(0.75).clamp(0.0, 1.0) as f32;
            let spk_sz = (15.5 * ctrl_scale).clamp(15.0, 20.0);
            r.draw_icon(
                SfIcon::Speaker,
                RectF::new(meta_x, my + 2.0, spk_sz, spk_sz),
                Color::rgba(255, 255, 255, 0.52),
            );
            let vx = meta_x + spk_sz + 9.0;
            let vw = (meta_w - spk_sz - 13.0).max(60.0);
            let vh = 5.0 * ctrl_scale;
            let vbar = RectF::new(vx, my + spk_sz * 0.5 - vh * 0.5 + 2.0, vw, vh);
            self.volume_bar_rect = vbar;
            r.fill_rounded_rect(vbar, vh * 0.5, Color::rgba(255, 255, 255, 0.20));
            r.fill_rounded_rect(
                RectF::new(vx, vbar.y, (vw * vol).max(vh), vbar.h),
                vh * 0.5,
                Color::rgba(255, 255, 255, 0.88),
            );
            self.hit_zones.push(LyricsHitZone {
                rect: RectF::new(vx, my - 2.0, vw, 24.0 * ctrl_scale),
                action: LyricsHitAction::ScrubVolumeBar(vbar),
                line_idx: None,
            });
        }

        // --- Draw Right Column: Up Next Queue OR SyncedLyricsViewController ---
        let right_col_alpha = ((1.0 - center_t * 1.38).clamp(0.0, 1.0) * sa).clamp(0.0, 1.0);
        if self.show_queue {
            if right_col_alpha > 0.01 {
                let shifted_right = RectF::new(
                    right_col.x + (1.0 - right_col_alpha) * 32.0,
                    right_col.y,
                    right_col.w,
                    right_col.h,
                );
                self.draw_queue_column(
                    r,
                    shifted_right,
                    smtc,
                    hovered_pt,
                    mouse_down,
                    ctrl_scale,
                    right_col_alpha,
                );
            }
            self.was_showing_queue = true;
        } else {
            self.was_showing_queue = false;
            if self.show_lyrics && right_col_alpha > 0.01 {
                let shifted_right = RectF::new(
                    right_col.x + (1.0 - right_col_alpha) * 32.0,
                    right_col.y,
                    right_col.w,
                    right_col.h,
                );

                if let Some(lyr) = lyrics.filter(|_| !is_lyrics_instrumental) {
                    let effective_raw = self.end_latched_playhead.unwrap_or(smtc.position_sec);
                    let synced_playhead = if self.end_latched_playhead.is_some() {
                        self.calibrated_playhead(effective_raw)
                    } else if self.last_raw_playhead_sec >= 0.0
                        && (smtc.position_sec - self.last_raw_playhead_sec).abs() <= 0.25
                        && (self.sync_offset_sec - self.last_calibrated_offset_sec).abs() < 1e-6
                    {
                        self.synced_playhead_sec
                    } else {
                        let t = self.calibrated_playhead(smtc.position_sec);
                        self.last_raw_playhead_sec = smtc.position_sec;
                        self.last_calibrated_offset_sec = self.sync_offset_sec;
                        self.synced_playhead_sec = t;
                        t
                    };
                    self.draw_synced_lyrics_column(
                        r,
                        shifted_right,
                        lyr,
                        lyrics_key,
                        synced_playhead,
                        hovered_pt,
                        reduce_motion,
                        non_emph_fx,
                        auto_emphasis,
                        frontier_wave,
                        end_hold_glow,
                        right_col_alpha,
                    );
                } else if lyrics_loading {
                    let load_inset = if self.center_axis { 0.0 } else { 20.0 };
                    r.draw_text(
                        "正在匹配逐字同步歌词...",
                        RectF::new(
                            shifted_right.x + load_inset,
                            shifted_right.center_y() - 30.0,
                            (shifted_right.w - load_inset * 2.0).max(160.0),
                            60.0,
                        ),
                        18.0 * ctrl_scale.min(1.3),
                        600,
                        Color::rgba(255, 255, 255, 0.55 * right_col_alpha),
                        TextAlign::Left,
                        true,
                        true,
                    );
                }
            }
        }

        // Bottom-Right Floating Controls Capsule (hidden cleanly in Immersive Mode until mouse nears bottom-right)
        if a_br > 0.02 {
            self.draw_bottom_right_glass_controls(
                r,
                win_rect,
                hovered_pt,
                mouse_down,
                glass_off,
                a_br,
            );
        }
        if use_lyrics_glass_batch {
            r.end_glass_backdrop_snapshot();
        }
    }

    pub fn invalidate_layout_cache(&mut self) {
        self.cached_layout_key = CachedLyricsLayoutKey::default();
        self.cached_line_layouts.clear();
        self.cached_lyrics_meta = None;
        self.glow_slot_cache = [LyricsGlowSlotCache::default(); 2];
        self.last_glass_bg_sig.valid = false;
    }

    pub fn on_viewport_changed(&mut self) {
        self.invalidate_layout_cache();
        self.animator.reset_layout_snap();
    }

    pub fn release_gpu_caches(&mut self, r: &mut D2DRenderer) {
        if !self.cached_line_layouts.is_empty()
            || self.cached_layout_key != CachedLyricsLayoutKey::default()
        {
            self.invalidate_layout_cache();
        }
        self.glow_slot_cache = [LyricsGlowSlotCache::default(); 2];
        self.last_glass_bg_sig.valid = false;
        r.release_lyrics_gpu_surfaces();
    }

    fn query_unit_cells_for_layout(
        lay: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
        units: &[LyricCharUnit],
    ) -> Vec<Vec<DisjointUnitCell>> {
        let raw_rects: Vec<Vec<RectF>> = units
            .iter()
            .map(|cu| {
                if cu.utf16_len == 0 {
                    Vec::new()
                } else {
                    D2DRenderer::hit_test_syllable_rects(
                        lay,
                        cu.utf16_start,
                        cu.utf16_len,
                        0.0,
                        0.0,
                    )
                }
            })
            .collect();
        let mut cells = build_disjoint_unit_cells(&raw_rects);
        for (u_idx, cu) in units.iter().enumerate() {
            if cells.get(u_idx).is_none_or(|c| c.len() != 1) {
                continue;
            }
            let trimmed = cu.ch_str.trim();
            if trimmed.is_empty() || trimmed.len() == cu.ch_str.len() {
                continue;
            }
            let lead_u16: u32 = cu
                .ch_str
                .chars()
                .take_while(|ch| ch.is_whitespace())
                .map(|ch| ch.len_utf16() as u32)
                .sum();
            let trim_u16 = trimmed.encode_utf16().count() as u32;
            if trim_u16 > 0 {
                let tight = D2DRenderer::hit_test_syllable_rects(
                    lay,
                    cu.utf16_start + lead_u16,
                    trim_u16,
                    0.0,
                    0.0,
                );
                if tight.len() == 1 {
                    cells[u_idx][0].glyph_rect = tight[0];
                }
            }
        }
        cells
    }

    fn query_sub_glyphs_for_layout(
        lay: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
        units: &[LyricCharUnit],
        unit_cells: &[Vec<DisjointUnitCell>],
    ) -> Vec<Vec<SubGlyphCell>> {
        units
            .iter()
            .enumerate()
            .map(|(u_idx, cu)| {
                let Some(cells) = unit_cells.get(u_idx) else {
                    return Vec::new();
                };
                if cells.len() != 1 || cu.utf16_len <= 1 {
                    return Vec::new();
                }
                let cell = cells[0];
                let non_ws_chars: Vec<(u32, u32)> = cu
                    .ch_str
                    .char_indices()
                    .filter_map(|(byte_idx, ch)| {
                        if ch.is_whitespace() {
                            None
                        } else {
                            let u16_off = cu.ch_str[..byte_idx].encode_utf16().count() as u32;
                            Some((u16_off, ch.len_utf16() as u32))
                        }
                    })
                    .collect();
                if non_ws_chars.len() <= 1 {
                    return Vec::new();
                }
                let total_chars = non_ws_chars.len();
                let mut glyphs: Vec<(RectF, f32)> = Vec::with_capacity(total_chars);
                for (g_idx, (rel_u16, len_u16)) in non_ws_chars.into_iter().enumerate() {
                    let rects = D2DRenderer::hit_test_syllable_rects(
                        lay,
                        cu.utf16_start + rel_u16,
                        len_u16,
                        0.0,
                        0.0,
                    );
                    if rects.len() == 1 && rects[0].w > 0.1 && rects[0].h > 0.1 {
                        let local_u = (g_idx as f32 + 0.5) / (total_chars as f32);
                        glyphs.push((rects[0], local_u));
                    }
                }
                if glyphs.len() <= 1 {
                    return Vec::new();
                }
                glyphs.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
                let n = glyphs.len();
                let mut out = Vec::with_capacity(n);
                for k in 0..n {
                    let (rc, lu) = glyphs[k];
                    let cl = if k == 0 {
                        cell.clip_rect.x.min(rc.x - 4.0)
                    } else {
                        (glyphs[k - 1].0.right() + rc.x) * 0.5
                    };
                    let cr = if k + 1 >= n {
                        cell.clip_rect.right().max(rc.right() + 4.0)
                    } else {
                        (rc.right() + glyphs[k + 1].0.x) * 0.5
                    };
                    out.push(SubGlyphCell {
                        glyph_rect: rc,
                        clip_rect: RectF::new(
                            cl,
                            cell.clip_rect.y,
                            (cr - cl).max(0.25),
                            cell.clip_rect.h,
                        ),
                        local_u: lu,
                    });
                }
                out
            })
            .collect()
    }

    /// Lazily measures `HitTestTextRange` cells ONLY for the single active mode (`by_word` OR `char`)
    /// and ONLY when a word-synced line is actually visible/active (eliminates 50-120ms resize freezes!).
    fn ensure_line_cells_measured(
        cl: &mut CachedLyricLineLayout,
        line: &crate::lyrics::LyricLine,
        by_word: bool,
    ) {
        if by_word {
            if cl.word_cells_measured {
                return;
            }
            cl.word_cells_measured = true;
            if line.has_word_sync {
                cl.word_cjk_cells =
                    Self::query_unit_cells_for_layout(&cl.main_layout, &line.word_cjk_units);
                cl.word_cjk_sub_glyphs = Self::query_sub_glyphs_for_layout(
                    &cl.main_layout,
                    &line.word_cjk_units,
                    &cl.word_cjk_cells,
                );
            }
            if let Some(ref bg_lay) = cl.bg_layout {
                if !line.bg_word_cjk_units.is_empty() {
                    cl.bg_word_cjk_cells =
                        Self::query_unit_cells_for_layout(bg_lay, &line.bg_word_cjk_units);
                    cl.bg_word_cjk_sub_glyphs = Self::query_sub_glyphs_for_layout(
                        bg_lay,
                        &line.bg_word_cjk_units,
                        &cl.bg_word_cjk_cells,
                    );
                }
            }
        } else {
            if cl.char_cells_measured {
                return;
            }
            cl.char_cells_measured = true;
            if line.has_word_sync {
                cl.char_cells = Self::query_unit_cells_for_layout(&cl.main_layout, &line.char_units);
            }
            if let Some(ref bg_lay) = cl.bg_layout {
                if !line.bg_char_units.is_empty() {
                    cl.bg_char_cells = Self::query_unit_cells_for_layout(bg_lay, &line.bg_char_units);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_cached_line_layouts(
        &mut self,
        r: &mut D2DRenderer,
        lyr: &ParsedLyrics,
        lyrics_key: &str,
        vocal_max_w: f32,
        max_w_q: u16,
        base_font_sz: f32,
        bg_font_sz: f32,
        trans_font_sz: f32,
    ) {
        let (lyr_hash, _) = self.lyrics_meta_cached(Some(lyr));
        let key = CachedLyricsLayoutKey {
            track_key: lyrics_key.to_string(),
            content_hash: lyr_hash,
            line_count: lyr.lines.len(),
            max_w_q,
            font_sz_tenths: (base_font_sz * 10.0).round() as u16,
            show_trans: self.show_translation,
        };
        if self.cached_layout_key == key && self.cached_line_layouts.len() == lyr.lines.len() {
            return;
        }
        if !self.cached_line_layouts.is_empty() {
            r.clear_lyrics_line_cache();
        }

        self.cached_line_layouts.clear();
        self.cached_line_layouts.reserve(lyr.lines.len());
        for (i, line) in lyr.lines.iter().enumerate() {
            let align = if line.is_duet_secondary || line.is_rtl {
                TextAlign::Right
            } else {
                TextAlign::Left
            };
            let txt_ref = if line.text.is_empty() { " " } else { &line.text };
            let Some((main_layout, main_h)) =
                r.create_lyrics_text_layout(txt_ref, base_font_sz, 700, vocal_max_w, align)
            else {
                self.cached_layout_key = CachedLyricsLayoutKey::default();
                self.cached_line_layouts.clear();
                return;
            };

            let bg_layout_opt = if let Some(ref bg_t) = line.bg_text {
                r.create_lyrics_text_layout(bg_t, bg_font_sz, 700, vocal_max_w, align)
            } else {
                None
            };
            let trans_layout_opt = if self.show_translation {
                if let Some(ref tr_t) = line.translation {
                    r.create_lyrics_text_layout(tr_t, trans_font_sz, 600, vocal_max_w, align)
                } else {
                    None
                }
            } else {
                None
            };

            let mut total_h = main_h;
            let bg_dy = total_h + LyricsAnimator::BG_VOCALS_TOP_SPACING;
            if let Some((_, bh)) = &bg_layout_opt {
                total_h = bg_dy + *bh;
            }
            let tr_dy = total_h + 8.0;
            if let Some((_, th)) = &trans_layout_opt {
                total_h = tr_dy + *th;
            }
            if let Some(st) = self.animator.line_states.get_mut(i) {
                st.measured_height = total_h;
            }

            // Defer expensive `HitTestTextRange` cell measurements lazily until a line enters the viewport!
            self.cached_line_layouts.push(CachedLyricLineLayout {
                align,
                main_layout,
                bg_layout: bg_layout_opt.map(|(l, _)| l),
                trans_layout: trans_layout_opt.map(|(l, _)| l),
                main_h,
                total_h,
                bg_dy,
                tr_dy,
                main_font_sz: base_font_sz,
                bg_font_sz,
                word_cells_measured: false,
                char_cells_measured: false,
                word_cjk_cells: Vec::new(),
                char_cells: Vec::new(),
                bg_word_cjk_cells: Vec::new(),
                bg_char_cells: Vec::new(),
                word_cjk_sub_glyphs: Vec::new(),
                bg_word_cjk_sub_glyphs: Vec::new(),
            });
        }
        self.cached_layout_key = key;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_synced_lyrics_column(
        &mut self,
        r: &mut D2DRenderer,
        right_col: RectF,
        lyr: &ParsedLyrics,
        lyrics_key: &str,
        playhead_sec: f64,
        hovered_pt: (f32, f32),
        reduce_motion: bool,
        non_emph_fx: NonEmphasisLyricsEffect,
        auto_emphasis: bool,
        frontier_wave: bool,
        end_hold_glow: bool,
        col_alpha: f32,
    ) {
        r.set_lyrics_subpixel_text_mode(true);
        let vocal_max_w = (right_col.w * 0.85).max(180.0); // Specs.vocalGroupWidthCoefficient = 0.85 (+496)
        let max_w_q = vocal_max_w.round().clamp(120.0, 2400.0) as u16;
        // Match official Apple Music generous bold proportions (`right_col.w * 0.056`) with a
        // gentle height guard so ultrawide/short windows stay balanced across all DPI scales
        let base_font_sz = (right_col.w * 0.056)
            .min((right_col.h * 0.066).max(30.0))
            .clamp(28.0, 58.0);
        let bg_font_sz = (base_font_sz * 0.67).clamp(18.0, 38.0);
        let trans_font_sz = (base_font_sz * 0.50).clamp(14.0, 28.0);

        // Build DirectWrite layouts + HitTestTextRange boxes ONCE per song/width (0 calls on subsequent frames!)
        self.ensure_cached_line_layouts(
            r,
            lyr,
            lyrics_key,
            vocal_max_w,
            max_w_q,
            base_font_sz,
            bg_font_sz,
            trans_font_sz,
        );
        if self.cached_line_layouts.len() != lyr.lines.len() {
            r.set_lyrics_subpixel_text_mode(false);
            return;
        }

        let t_sweep = playhead_sec + LyricsAnimator::SWEEP_HEADSTART_SEC;
        let by_word = self.animator.latin_by_word;
        let user_scrolling = self.animator.is_user_scrolling();
        let anchor_idx = self.animator.primary_active_idx.unwrap_or(0);
        let lyr_content_hash = self.cached_layout_key.content_hash;

        // Reuse persistent scratch Vec across frames (0 per-frame heap allocation!)
        let mut visible_lines = std::mem::take(&mut self.visible_lines_scratch);
        visible_lines.clear();
        let mut next_glow_slot = 0usize;
        let mut blur_bakes_this_frame = 0usize;

        // PASS 1 (Clip/Layer Depth == 0!):
        // - O(1) float frustum culling using pre-cached `total_h` (zero DirectWrite calls!)
        // - Pre-warm blurred depth-of-field bitmaps ONLY for actually blurred non-vector lines
        //   (budgeted to max 2 GPU blur bakes per frame to eliminate first-frame/scroll spikes!)
        // - Bake per-character/word glyph emission into `active_glow_slots[0..2]` only when units glow
        for i in 0..lyr.lines.len() {
            let line = &lyr.lines[i];
            let cl_total_h = self.cached_line_layouts[i].total_h;
            if let Some(st_mut) = self.animator.line_states.get_mut(i) {
                st_mut.measured_height = cl_total_h;
            }
            let Some(st) = self.animator.line_states.get(i) else {
                continue;
            };

            let screen_y = right_col.y + st.y_spring.value - self.animator.user_scroll_offset;
            if screen_y + cl_total_h < right_col.y - 84.0 || screen_y > right_col.bottom() + 84.0 {
                continue;
            }
            // Lazily measure DirectWrite unit cells ONLY for visible word-synced lines in the active mode!
            if line.has_word_sync || !line.bg_syllables.is_empty() {
                Self::ensure_line_cells_measured(&mut self.cached_line_layouts[i], line, by_word);
            }
            let cl = &self.cached_line_layouts[i];

            let line_mid_y = screen_y + cl.total_h * 0.5;
            let top_t = ((line_mid_y - right_col.y) / 68.0).clamp(0.0, 1.0);
            let bot_t = ((right_col.bottom() - line_mid_y) / 76.0).clamp(0.0, 1.0);
            let st_top = top_t * top_t * (3.0 - 2.0 * top_t);
            let st_bot = bot_t * bot_t * (3.0 - 2.0 * bot_t);
            let edge_fade = (st_top * st_bot).clamp(0.0, 1.0);

            let line = &lyr.lines[i];
            let left_inset = if self.center_axis { 0.0 } else { 16.0 };
            let right_inset = if self.center_axis { 8.0 } else { 20.0 };
            let origin_x = if cl.align == TextAlign::Right {
                right_col.right() - vocal_max_w - right_inset
            } else {
                right_col.x + left_inset
            };
            let active_w = st.active_blend.value.clamp(0.0, 1.0);
            let line_t_sweep = st.clamped_word_time(t_sweep);
            let bmp_key = LyricsLineBmpKey {
                line_idx: i,
                max_w_q,
                show_trans: self.show_translation,
            };

            let hit_pill = RectF::new(
                origin_x - 16.0,
                screen_y - 10.0,
                vocal_max_w + 32.0,
                cl.total_h + 20.0,
            );
            let is_hov_pass1 = hit_pill.contains(hovered_pt.0, hovered_pt.1)
                && right_col.contains(hovered_pt.0, hovered_pt.1);
            let is_line_selected = st.active_blend.target > 0.5;
            let use_vector_layer = if line.has_word_sync {
                is_line_selected || active_w > 0.72
            } else {
                is_line_selected
                    || (active_w > 0.72
                        && (reduce_motion || is_hov_pass1 || user_scrolling || st.blur_spring.value <= 0.12))
            };

            if !use_vector_layer && edge_fade > 0.01 {
                let handoff_raw = if is_line_selected {
                    0.0
                } else {
                    ((0.72 - active_w) / 0.72).clamp(0.0, 1.0)
                };
                let handoff_blur = handoff_raw * handoff_raw * (3.0 - 2.0 * handoff_raw);
                let pre_sigma = if reduce_motion || is_hov_pass1 || user_scrolling {
                    0.0
                } else {
                    (st.blur_spring.value * handoff_blur).clamp(0.0, 6.2)
                };
                if pre_sigma > 0.04 && blur_bakes_this_frame < 2 {
                    r.ensure_lyrics_line_bitmap(
                        bmp_key,
                        vocal_max_w,
                        cl.total_h,
                        cl.align,
                        &cl.main_layout,
                        cl.bg_layout.as_ref().map(|l| (l, cl.bg_dy)),
                        cl.trans_layout.as_ref().map(|l| (l, cl.tr_dy)),
                    );
                    if r.prepare_lyrics_line_blurred(bmp_key, pre_sigma) {
                        blur_bakes_this_frame += 1;
                    }
                }
            }

            let mut glow_slot = None;
            let mut glow_peak = 0.0f32;

            if !reduce_motion && active_w > 0.012 && next_glow_slot < 2 && col_alpha * edge_fade > 0.02 {
                let units: &[LyricCharUnit] = if line.has_word_sync {
                    line.units_for_mode(by_word)
                } else {
                    &[]
                };
                let any_glow_candidate = if line.has_word_sync {
                    units.iter().any(|cu| {
                        (cu.is_emphasized(auto_emphasis) || non_emph_fx.glow())
                            && cu.sweep_progress(line_t_sweep) > 0.002
                    })
                } else {
                    non_emph_fx.glow()
                };
                if any_glow_candidate {
                    let slot = next_glow_slot;
                    let vis_mul = active_w * col_alpha * edge_fade;
                    let active_w_q = (vis_mul * 255.0).round().clamp(0.0, 255.0) as u16;
                    let fx_flags: u8 = ((non_emph_fx.glow() as u8) << 0)
                        | ((non_emph_fx.scale() as u8) << 1)
                        | ((auto_emphasis as u8) << 2)
                        | ((frontier_wave as u8) << 3)
                        | ((end_hold_glow as u8) << 4)
                        | ((by_word as u8) << 5);
                    let gc = self.glow_slot_cache[slot];
                    let can_reuse_baked_glow = gc.valid
                        && gc.line_idx == i
                        && gc.content_hash == lyr_content_hash
                        && gc.max_w_q == max_w_q
                        && gc.flags == fx_flags
                        && gc.active_w_q.abs_diff(active_w_q) <= 2
                        && (line_t_sweep - gc.last_t_sweep).abs() < 0.018
                        && gc
                            .last_baked_at
                            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(16))
                        && r.has_active_lyrics_glow_slot_baked(slot);
                    if can_reuse_baked_glow {
                        glow_peak = gc.last_peak;
                        glow_slot = Some(slot);
                        next_glow_slot += 1;
                    } else {
                        let unit_cells = if by_word {
                            &cl.word_cjk_cells
                        } else {
                            &cl.char_cells
                        };
                        let sub_glyphs: &[Vec<SubGlyphCell>] = if by_word {
                            &cl.word_cjk_sub_glyphs
                        } else {
                            &[]
                        };
                        let font_sz = cl.main_font_sz;
                        if let Some(pad_dip) =
                            r.begin_active_lyrics_glow_slot(slot, vocal_max_w, cl.total_h)
                        {
                            let (peak, has_glow, cjk_frac) = Self::bake_active_line_glyph_emission(
                                r,
                                &cl.main_layout,
                                font_sz,
                                units,
                                unit_cells,
                                sub_glyphs,
                                &st.char_lifts,
                                pad_dip,
                                pad_dip,
                                line_t_sweep,
                                line.end_sec,
                                line.is_rtl,
                                vis_mul,
                                non_emph_fx,
                                auto_emphasis,
                                frontier_wave,
                                end_hold_glow,
                            );
                            if has_glow && peak > 0.005 {
                                // CJK ideographs pack ~3× denser strokes than Latin letters: keep the tight
                                // inner halo whisper-soft & slightly wider so inner counters stay razor-sharp,
                                // while preserving a lush atmospheric outer bloom!
                                let cf = cjk_frac.clamp(0.0, 1.0);
                                let inner_sigma = 3.8 + 1.3 * cf;
                                let inner_alpha =
                                    ((0.18 - 0.09 * cf) + peak * (0.18 - 0.08 * cf)) * vis_mul;
                                let outer_sigma = (9.5 + 2.4 * cf) + peak * 5.8;
                                let outer_alpha =
                                    ((0.18 - 0.04 * cf) + peak * (0.24 - 0.06 * cf)) * vis_mul;
                                r.finish_and_bake_active_lyrics_glow_slot(
                                    slot,
                                    inner_sigma,
                                    inner_alpha,
                                    outer_sigma,
                                    outer_alpha,
                                );
                                self.glow_slot_cache[slot] = LyricsGlowSlotCache {
                                    valid: true,
                                    line_idx: i,
                                    content_hash: lyr_content_hash,
                                    max_w_q,
                                    active_w_q,
                                    flags: fx_flags,
                                    last_t_sweep: line_t_sweep,
                                    last_peak: peak,
                                    last_baked_at: Some(std::time::Instant::now()),
                                };
                                glow_peak = peak;
                                glow_slot = Some(slot);
                                next_glow_slot += 1;
                            } else {
                                self.glow_slot_cache[slot].valid = false;
                                r.end_active_lyrics_glow_slot();
                            }
                        }
                    }
                }
            }

            visible_lines.push(VisibleLineSlot {
                idx: i,
                origin_x,
                screen_y,
                active_w,
                edge_fade,
                use_vector_layer,
                line_t_sweep,
                bmp_key,
                glow_slot,
                glow_peak,
            });
        }

        // PASS 2: Fast hardware axis-aligned clip with +/- 64 DIP horizontal glow bleed
        // (replaces expensive 4K full-column PushLayer; smooth vertical edge fade is applied per-line!)
        let fade_left = (right_col.x - 64.0).max(0.0);
        let fade_rect = RectF::new(
            fade_left,
            right_col.y,
            right_col.w + (right_col.x - fade_left) + 64.0,
            right_col.h,
        );
        r.push_clip(fade_rect);

        // Draw Instrumental Break 3-Dot Indicator (`Specs.instrumentalBreakCountdownDotCount = 3`)
        if self.animator.interlude.visible && self.animator.interlude.scale_alpha > 0.02 {
            let dots_y = match self.animator.interlude.after_line_idx {
                None => self
                    .animator
                    .line_states
                    .first()
                    .map(|s| right_col.y + s.y_spring.value - 44.0)
                    .unwrap_or(right_col.y + 80.0),
                Some(prev_i) => self
                    .animator
                    .line_states
                    .get(prev_i)
                    .map(|s| right_col.y + s.y_spring.value + s.measured_height + 16.0)
                    .unwrap_or(right_col.y + 120.0),
            } - self.animator.user_scroll_offset;

            let env = self.animator.interlude.scale_alpha;
            let prog = self.animator.interlude.progress;
            let base_x = right_col.x + if self.center_axis { 6.0 } else { 22.0 };
            for d in 0..3 {
                let seg_p = ((prog * 3.0) - (d as f32)).clamp(0.0, 1.0);
                let wave = if reduce_motion {
                    0.0
                } else {
                    (self.mesh_phase * 4.2 - (d as f32) * 0.75).sin() * 2.5 * env
                };
                let rad = (5.0 + seg_p * 2.2) * env;
                let alpha = (0.26 + seg_p * 0.72) * env * col_alpha;
                r.fill_circle(
                    base_x + (d as f32) * 20.0,
                    dots_y + 16.0 + wave,
                    rad,
                    Color::rgba(255, 255, 255, alpha),
                );
            }
        }

        let mut hovered_line_now: Option<usize> = None;

        for pl in &visible_lines {
            let i = pl.idx;
            let cl = &self.cached_line_layouts[i];
            let line = &lyr.lines[i];
            let st = &self.animator.line_states[i];

            let hit_pill = RectF::new(
                pl.origin_x - 16.0,
                pl.screen_y - 10.0,
                vocal_max_w + 32.0,
                cl.total_h + 20.0,
            );
            let is_hov = hit_pill.contains(hovered_pt.0, hovered_pt.1)
                && right_col.contains(hovered_pt.0, hovered_pt.1);
            if is_hov {
                hovered_line_now = Some(i);
                // `Specs.highlightViewBackgroundColor = UIColor(white: 1.0, alpha: 0.08)`, `cornerRadius = 16.0`
                r.fill_rounded_rect(hit_pill, 16.0, Color::rgba(255, 255, 255, 0.08));
            }
            // Only register click-to-seek for lines within the visible unmasked body
            if pl.screen_y + cl.total_h > right_col.y + 18.0 && pl.screen_y < right_col.bottom() - 22.0 {
                let seek_target = (line.start_sec - self.sync_offset_sec).max(0.0);
                self.hit_zones.push(LyricsHitZone {
                    rect: hit_pill,
                    action: LyricsHitAction::SeekToSec(seek_target),
                    line_idx: Some(i),
                });
            }

            let combined_scale = st.scale_spring.value * st.press_spring.value;
            let pivot_x = if cl.align == TextAlign::Right {
                pl.origin_x + vocal_max_w
            } else {
                pl.origin_x
            };
            let pivot_y = pl.screen_y + cl.main_h * 0.5;
            let line_xform = Matrix3x2 {
                M11: combined_scale,
                M12: 0.0,
                M21: 0.0,
                M22: combined_scale,
                M31: pivot_x * (1.0 - combined_scale),
                M32: pivot_y * (1.0 - combined_scale),
            };
            r.set_transform(&line_xform);

            let dist = (i as isize - anchor_idx as isize).unsigned_abs();
            let inactive_base_alpha = if is_hov {
                0.85 // `Specs.highlightLabelAlpha = 0.85` (+432)
            } else if user_scrolling {
                0.62 // `Specs.deselectedScrollTextColor` unblurs & brightens on manual scroll!
            } else {
                match dist {
                    0 => 0.46,
                    1 => 0.34,
                    2 => 0.26,
                    3 => 0.20,
                    _ => 0.14,
                }
            };

            // Exclusive single-layer rendering: NEVER draw blurred Layer A and lifted/scaled Layer B
            // simultaneously so there is zero double-layer ghosting ("露馅")!
            let line_vis_alpha = col_alpha * pl.edge_fade;
            if line_vis_alpha <= 0.005 {
                r.reset_transform();
                continue;
            }
            if !pl.use_vector_layer {
                let is_line_selected = st.active_blend.target > 0.5;
                let handoff_raw = if is_line_selected {
                    0.0
                } else {
                    ((0.72 - pl.active_w) / 0.72).clamp(0.0, 1.0)
                };
                let handoff_blur = handoff_raw * handoff_raw * (3.0 - 2.0 * handoff_raw);
                let sigma = if reduce_motion || is_hov || user_scrolling {
                    0.0
                } else {
                    (st.blur_spring.value * handoff_blur).clamp(0.0, 6.2)
                };
                let draw_a =
                    (inactive_base_alpha + (0.99 - inactive_base_alpha) * pl.active_w) * line_vis_alpha;
                let drew_cached = sigma > 0.04
                    && r.draw_cached_lyrics_line_blurred(
                        pl.bmp_key,
                        pl.origin_x,
                        pl.screen_y,
                        sigma,
                        draw_a,
                    );
                if !drew_cached {
                    r.draw_text_layout_subpixel(
                        &cl.main_layout,
                        pl.origin_x,
                        pl.screen_y,
                        Color::rgba(255, 255, 255, draw_a),
                    );
                    if let Some(ref bg_lay) = cl.bg_layout {
                        r.draw_text_layout_subpixel(
                            bg_lay,
                            pl.origin_x,
                            pl.screen_y + cl.bg_dy,
                            Color::rgba(255, 255, 255, draw_a * 0.72),
                        );
                    }
                    if let Some(ref tr_lay) = cl.trans_layout {
                        r.draw_text_layout_subpixel(
                            tr_lay,
                            pl.origin_x,
                            pl.screen_y + cl.tr_dy,
                            Color::rgba(255, 255, 255, draw_a * 0.68),
                        );
                    }
                }
            } else {
                if let Some(slot) = pl.glow_slot {
                    let inner_sigma = 3.8f32;
                    let inner_alpha = (0.48 + pl.glow_peak * 0.44) * pl.active_w * line_vis_alpha;
                    let outer_sigma = 9.0 + pl.glow_peak * 6.5;
                    let outer_alpha = (0.22 + pl.glow_peak * 0.44) * pl.active_w * line_vis_alpha;
                    r.composite_active_lyrics_glow_slot(
                        slot,
                        pl.origin_x,
                        pl.screen_y,
                        inner_sigma,
                        inner_alpha,
                        outer_sigma,
                        outer_alpha,
                    );
                    r.set_transform(&line_xform);
                }

                let sung_a =
                    (inactive_base_alpha + (0.99 - inactive_base_alpha) * pl.active_w) * line_vis_alpha;
                let up_a =
                    (inactive_base_alpha + (0.34 - inactive_base_alpha) * pl.active_w) * line_vis_alpha;
                let sung_col = Color::rgba(255, 255, 255, sung_a);
                let up_col = Color::rgba(255, 255, 255, up_a);
                let main_units = line.units_for_mode(by_word);
                let main_cells = if by_word {
                    &cl.word_cjk_cells
                } else {
                    &cl.char_cells
                };
                let main_font_sz = cl.main_font_sz;

                if line.has_word_sync && !main_units.is_empty() {
                    let main_sub_glyphs: &[Vec<SubGlyphCell>] = if by_word {
                        &cl.word_cjk_sub_glyphs
                    } else {
                        &[]
                    };
                    Self::draw_char_sequence(
                        r,
                        &cl.main_layout,
                        main_font_sz,
                        main_units,
                        main_cells,
                        main_sub_glyphs,
                        &st.char_lifts,
                        pl.origin_x,
                        pl.screen_y,
                        pivot_x,
                        pivot_y,
                        combined_scale,
                        pl.line_t_sweep,
                        line.is_rtl,
                        reduce_motion,
                        true,
                        non_emph_fx,
                        auto_emphasis,
                        frontier_wave,
                        sung_col,
                        up_col,
                    );
                    r.set_transform(&line_xform);
                } else {
                    r.draw_text_layout_subpixel(&cl.main_layout, pl.origin_x, pl.screen_y, sung_col);
                }

                // Active Background Vocals (`x-bg`) — CJK per-`字` / Latin per-`word` (or char)!
                if let Some(ref bg_lay) = cl.bg_layout {
                    let bg_y = pl.screen_y + cl.bg_dy;
                    let bg_sung_a =
                        (inactive_base_alpha * 0.72 + (0.78 - inactive_base_alpha * 0.72) * pl.active_w)
                            * line_vis_alpha;
                    let bg_up_a =
                        (inactive_base_alpha * 0.72 + (0.20 - inactive_base_alpha * 0.72) * pl.active_w)
                            * line_vis_alpha;
                    let bg_sung = Color::rgba(255, 255, 255, bg_sung_a);
                    let bg_up = Color::rgba(255, 255, 255, bg_up_a);
                    let bg_units = line.bg_units_for_mode(by_word);
                    let bg_cells = if by_word {
                        &cl.bg_word_cjk_cells
                    } else {
                        &cl.bg_char_cells
                    };
                    let bg_sub_glyphs: &[Vec<SubGlyphCell>] = if by_word {
                        &cl.bg_word_cjk_sub_glyphs
                    } else {
                        &[]
                    };
                    let bg_font_sz = cl.bg_font_sz;
                    if !line.bg_syllables.is_empty() && !bg_units.is_empty() {
                        Self::draw_char_sequence(
                            r,
                            bg_lay,
                            bg_font_sz,
                            bg_units,
                            bg_cells,
                            bg_sub_glyphs,
                            &st.bg_char_lifts,
                            pl.origin_x,
                            bg_y,
                            pivot_x,
                            pivot_y,
                            combined_scale,
                            pl.line_t_sweep,
                            line.is_rtl,
                            reduce_motion,
                            false,
                            non_emph_fx,
                            auto_emphasis,
                            frontier_wave,
                            bg_sung,
                            bg_up,
                        );
                        r.set_transform(&line_xform);
                    } else {
                        r.draw_text_layout_subpixel(bg_lay, pl.origin_x, bg_y, bg_sung);
                    }
                }

                // Active Translation Line
                if let Some(ref tr_lay) = cl.trans_layout {
                    let tr_alpha =
                        (inactive_base_alpha * 0.72 + (0.68 - inactive_base_alpha * 0.72) * pl.active_w)
                            * line_vis_alpha;
                    r.draw_text_layout_subpixel(
                        tr_lay,
                        pl.origin_x,
                        pl.screen_y + cl.tr_dy,
                        Color::rgba(255, 255, 255, tr_alpha),
                    );
                }
            }

            r.reset_transform();
        }

        // Songwriters Credit Footer (`Specs.writtenByFont` / `songwritersNamesFont`)
        if !lyr.songwriters.is_empty() {
            if let Some(last_st) = self.animator.line_states.last() {
                let credit_y = right_col.y
                    + last_st.y_spring.value
                    + last_st.measured_height
                    + 42.0
                    - self.animator.user_scroll_offset;
                if credit_y < right_col.bottom() + 40.0 && credit_y + 60.0 > right_col.y {
                    let credit_txt = format!("词曲作者：{}", lyr.songwriters.join("、"));
                    let credit_x = right_col.x + if self.center_axis { 0.0 } else { 16.0 };
                    r.draw_text(
                        &credit_txt,
                        RectF::new(credit_x, credit_y, vocal_max_w, 48.0),
                        (trans_font_sz * 0.92).clamp(13.5, 22.0),
                        500,
                        Color::rgba(255, 255, 255, 0.38 * col_alpha),
                        TextAlign::Left,
                        false,
                        true,
                    );
                }
            }
        }

        self.animator.hovered_line = hovered_line_now;
        self.visible_lines_scratch = visible_lines;

        r.pop_clip();
        r.set_lyrics_subpixel_text_mode(false);
    }

    /// Renders per-character (`字`) / per-word glyph-exact emissive light into `active_glow_slots[slot]`.
    /// Default: only explicitly marked / sustained emphasized units glow; non-emphasized units respect
    /// `non_emph_fx`. Scaled units render via isolated single-unit layouts so neighbors never bleed or clip!
    #[allow(clippy::too_many_arguments)]
    fn bake_active_line_glyph_emission(
        r: &mut D2DRenderer,
        layout: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
        _font_sz: f32,
        char_units: &[LyricCharUnit],
        unit_cells: &[Vec<DisjointUnitCell>],
        sub_glyphs: &[Vec<SubGlyphCell>],
        lifts: &[Spring1D],
        local_x: f32,
        local_y: f32,
        t_headstart: f64,
        line_end_sec: f64,
        is_rtl: bool,
        active_w: f32,
        non_emph_fx: NonEmphasisLyricsEffect,
        auto_emphasis: bool,
        frontier_wave: bool,
        end_hold_glow: bool,
    ) -> (f32, bool, f32) {
        if char_units.is_empty() {
            if non_emph_fx.glow() {
                r.draw_text_layout_subpixel(
                    layout,
                    local_x,
                    local_y,
                    Color::rgba(255, 255, 255, 0.28 * active_w),
                );
                return (0.22 * active_w, true, 0.5);
            }
            return (0.0, false, 0.0);
        }

        let mut peak_emphasis = 0.0f32;
        let mut has_any_glow = false;
        let mut cjk_glow_weight = 0.0f32;
        let mut total_glow_units = 0usize;

        for (c_idx, cu) in char_units.iter().enumerate() {
            if cu.utf16_len == 0 {
                continue;
            }
            let is_emph = cu.is_emphasized(auto_emphasis);
            let glow_enabled = is_emph || non_emph_fx.glow();
            if !glow_enabled {
                continue;
            }
            let prog = cu.sweep_progress(t_headstart);
            if prog <= 0.002 {
                continue;
            }

            let next_boundary_sec = char_units[c_idx + 1..]
                .iter()
                .find(|ncu| ncu.utf16_len > 0 && !ncu.ch_str.trim().is_empty())
                .map(|ncu| ncu.start_sec.max(cu.end_sec))
                .unwrap_or_else(|| line_end_sec.max(cu.end_sec + 0.55));
            let has_hold_gap = next_boundary_sec > cu.end_sec + 0.025;
            let is_holding = has_hold_gap && prog >= 0.996 && t_headstart < next_boundary_sec;
            let hold_after_end_sec = if is_holding {
                (t_headstart - cu.end_sec).max(0.0) as f32
            } else if has_hold_gap {
                (next_boundary_sec - cu.end_sec).max(0.0) as f32
            } else {
                0.0
            };
            let decay_sec = if is_holding {
                0.0
            } else if has_hold_gap {
                (t_headstart - next_boundary_sec).max(0.0) as f32
            } else {
                (t_headstart - cu.end_sec).max(0.0) as f32
            };

            let dur_factor = (((cu.word_dur_sec - 0.28) / 1.40).clamp(0.32, 1.0)) as f32;
            let (raw_body_a, raw_peak_a, _tail_decay) = eval_frontier_glow_profile_ex(
                prog,
                hold_after_end_sec,
                decay_sec,
                is_holding,
                is_emph,
                dur_factor,
                end_hold_glow,
            );
            let cjk_scale = cjk_glow_density_scale(&cu.ch_str);
            let body_a = raw_body_a * cjk_scale;
            let peak_a = raw_peak_a * cjk_scale;
            if body_a <= 0.01 && peak_a <= 0.015 {
                continue;
            }
            if cjk_scale < 0.85 {
                cjk_glow_weight += 1.0;
            }
            total_glow_units += 1;
            peak_emphasis = peak_emphasis.max(
                raw_peak_a * if is_emph {
                    0.35 + 0.65 * dur_factor
                } else {
                    0.45
                },
            );

            let Some(cells) = unit_cells.get(c_idx) else {
                continue;
            };
            if cells.is_empty() {
                continue;
            }
            let lift_y = lifts.get(c_idx).map(|sp| sp.value).unwrap_or(0.0);
            let scale_enabled = is_emph || non_emph_fx.scale();
            let max_scale_delta = if is_emph {
                LyricsAnimator::EMPHASIZING_SCALE_DELTA * dur_factor
            } else if scale_enabled {
                0.055
            } else {
                0.0
            };
            let unit_w: f32 = cells
                .iter()
                .map(|c| c.raw_rect.w.max(1.0))
                .sum::<f32>()
                .max(1.0);
            let use_frontier = frontier_wave
                && should_use_frontier_lift(
                    is_emph || scale_enabled,
                    cu.end_sec - cu.start_sec,
                    unit_w,
                );

            if cells.len() == 1
                && use_frontier
                && prog < 0.996
                && sub_glyphs.get(c_idx).is_some_and(|v| v.len() > 1)
            {
                let cell = cells[0];
                let gr = RectF::new(
                    cell.raw_rect.x + local_x,
                    cell.raw_rect.y + local_y,
                    cell.raw_rect.w.max(1.0),
                    cell.raw_rect.h.max(1.0),
                );
                let clip_r = RectF::new(
                    cell.clip_rect.x + local_x,
                    cell.clip_rect.y + local_y,
                    cell.clip_rect.w,
                    cell.clip_rect.h,
                );
                let sweep_rc = RectF::new(gr.x, clip_r.y, gr.w, clip_r.h);
                let feather = (gr.w * 0.55).clamp(8.0, LyricsAnimator::GRADIENT_FEATHER_PX);
                let sw = sweep_rc.w.max(2.0);
                let back_px = (feather * 0.72).clamp(sw * 0.12, sw * 0.55);
                let front_px = (feather * 0.34).clamp(sw * 0.06, sw * 0.32);
                let frontier_x = if is_rtl {
                    sweep_rc.right() - prog * sw
                } else {
                    sweep_rc.x + prog * sw
                };
                let sgs = &sub_glyphs[c_idx];
                // Coalesce contiguous already-swept unscaled letters behind the glow frontier into ONE DrawTextLayout,
                // and skip letters strictly ahead of the glow frontier (where emission alpha == 0.0)!
                let mut settled_glow_run: Option<(RectF, f32)> = None;
                let flush_settled_glow = |r: &D2DRenderer, run: &mut Option<(RectF, f32)>| {
                    if let Some((rc, ly)) = run.take() {
                        r.set_transform(&Matrix3x2::translation(0.0, ly));
                        r.push_clip(rc);
                        r.draw_text_layout_subpixel(
                            layout,
                            local_x,
                            local_y,
                            Color::rgba(255, 255, 255, body_a * active_w),
                        );
                        r.pop_clip();
                    }
                };
                for sg in sgs {
                    let wl = sg.clip_rect.x + local_x;
                    let wr = sg.clip_rect.right() + local_x;
                    let ahead_of_frontier = if is_rtl {
                        wr <= frontier_x - front_px - 0.5
                    } else {
                        wl >= frontier_x + front_px + 0.5
                    };
                    if ahead_of_frontier {
                        continue;
                    }
                    let g_lift = lift_y * eval_frontier_lift_weight(sg.local_u, prog, false);
                    let g_scale = if scale_enabled {
                        1.0 + eval_frontier_scale_delta(sg.local_u, prog, max_scale_delta, false)
                    } else {
                        1.0
                    };
                    let behind_frontier = if is_rtl {
                        wl >= frontier_x + back_px + 0.5
                    } else {
                        wr <= frontier_x - back_px - 0.5
                    };
                    if behind_frontier && (g_scale - 1.0).abs() < 0.002 {
                        let sg_r = RectF::new(
                            wl,
                            sg.clip_rect.y + local_y,
                            (wr - wl).max(0.25),
                            sg.clip_rect.h,
                        );
                        if let Some((cur_r, cur_ly)) = settled_glow_run.as_mut() {
                            if (g_lift - *cur_ly).abs() < 0.04 {
                                let nx = cur_r.x.min(sg_r.x);
                                let nr = cur_r.right().max(sg_r.right());
                                let ny = cur_r.y.min(sg_r.y);
                                let nb = cur_r.bottom().max(sg_r.bottom());
                                *cur_r = RectF::new(nx, ny, nr - nx, nb - ny);
                                has_any_glow = true;
                                continue;
                            }
                        }
                        flush_settled_glow(r, &mut settled_glow_run);
                        settled_glow_run = Some((sg_r, g_lift));
                        has_any_glow = true;
                        continue;
                    }
                    flush_settled_glow(r, &mut settled_glow_run);
                    let scx = sg.glyph_rect.center_x() + local_x;
                    let scy = sg.glyph_rect.center_y() + local_y + g_lift;
                    let xf = Matrix3x2 {
                        M11: g_scale,
                        M12: 0.0,
                        M21: 0.0,
                        M22: g_scale,
                        M31: scx * (1.0 - g_scale),
                        M32: scy * (1.0 - g_scale) + g_lift * g_scale,
                    };
                    let cl = scx + (wl - scx) / g_scale;
                    let cr = scx + (wr - scx) / g_scale;
                    let sg_clip = RectF::new(
                        cl,
                        sg.clip_rect.y + local_y,
                        (cr - cl).max(0.25),
                        sg.clip_rect.h,
                    );
                    r.set_transform(&xf);
                    r.push_clip(sg_clip);
                    r.draw_text_layout_frontier_glow_sweep(
                        layout,
                        local_x,
                        local_y,
                        sweep_rc,
                        prog,
                        feather,
                        is_rtl,
                        body_a * active_w,
                        peak_a * active_w,
                    );
                    r.pop_clip();
                    has_any_glow = true;
                }
                flush_settled_glow(r, &mut settled_glow_run);
                continue;
            }

            let mut acc_w = 0.0f32;
            for cell in cells {
                let gr = RectF::new(
                    cell.raw_rect.x + local_x,
                    cell.raw_rect.y + local_y,
                    cell.raw_rect.w,
                    cell.raw_rect.h,
                );
                let clip_r = RectF::new(
                    cell.clip_rect.x + local_x,
                    cell.clip_rect.y + local_y,
                    cell.clip_rect.w,
                    cell.clip_rect.h,
                );
                let seg_w = gr.w.max(1.0);
                let sub_prog = if prog >= 0.996 {
                    1.0
                } else {
                    ((prog * unit_w - acc_w) / seg_w).clamp(0.0, 1.0)
                };
                acc_w += seg_w;
                if sub_prog <= 0.002 {
                    continue;
                }

                let c_scale = if scale_enabled && prog < 0.996 && cells.len() == 1 {
                    1.0 + eval_frontier_scale_delta(0.5, sub_prog, max_scale_delta * 0.85, is_rtl)
                } else {
                    1.0
                };
                let (c_lift, c_shear) = if use_frontier && prog < 0.996 && cells.len() == 1 {
                    eval_frontier_wave_shear(lift_y, gr.w, sub_prog, is_rtl)
                } else {
                    (lift_y, 0.0)
                };
                let scx = gr.center_x();
                let scy = gr.center_y();
                r.set_transform(&Matrix3x2 {
                    M11: c_scale,
                    M12: c_shear,
                    M21: 0.0,
                    M22: c_scale,
                    M31: scx * (1.0 - c_scale),
                    M32: scy * (1.0 - c_scale) + c_lift - scx * c_shear,
                });
                let cl = scx + (clip_r.x - scx) / c_scale;
                let cr = scx + (clip_r.right() - scx) / c_scale;
                let adj_clip = RectF::new(cl, clip_r.y - 8.0, (cr - cl).max(0.25), clip_r.h + 16.0);
                r.push_clip(adj_clip);
                let sweep_rc = RectF::new(gr.x, clip_r.y, gr.w, clip_r.h);
                let feather = (gr.w * 0.55).clamp(8.0, LyricsAnimator::GRADIENT_FEATHER_PX);
                r.draw_text_layout_frontier_glow_sweep(
                    layout,
                    local_x,
                    local_y,
                    sweep_rc,
                    sub_prog,
                    feather,
                    is_rtl,
                    body_a * active_w,
                    peak_a * active_w,
                );
                r.pop_clip();
                has_any_glow = true;
            }
        }
        r.reset_transform();
        let cjk_frac = if total_glow_units == 0 {
            0.0
        } else {
            (cjk_glow_weight / (total_glow_units as f32)).clamp(0.0, 1.0)
        };
        (peak_emphasis, has_any_glow, cjk_frac)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_char_sequence(
        r: &mut D2DRenderer,
        layout: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
        _font_sz: f32,
        char_units: &[LyricCharUnit],
        unit_cells: &[Vec<DisjointUnitCell>],
        unit_sub_glyphs: &[Vec<SubGlyphCell>],
        lifts: &[Spring1D],
        origin_x: f32,
        origin_y: f32,
        pivot_x: f32,
        pivot_y: f32,
        combined_scale: f32,
        t_headstart: f64,
        is_rtl: bool,
        reduce_motion: bool,
        allow_emphasis: bool,
        non_emph_fx: NonEmphasisLyricsEffect,
        auto_emphasis: bool,
        frontier_wave: bool,
        sung_col: Color,
        up_col: Color,
    ) {
        // `(clip_rect, lift_y, is_sung)` batch for unscaled contiguous glyphs on the same visual row!
        let mut solid_run: Option<(RectF, f32, bool)> = None;
        let flush_run = |r: &D2DRenderer, run: &mut Option<(RectF, f32, bool)>| {
            if let Some((clip_r, ly, is_sung)) = run.take() {
                r.set_transform(&Matrix3x2 {
                    M11: combined_scale,
                    M12: 0.0,
                    M21: 0.0,
                    M22: combined_scale,
                    M31: pivot_x * (1.0 - combined_scale),
                    M32: pivot_y * (1.0 - combined_scale) + ly * combined_scale,
                });
                r.push_clip(clip_r);
                r.draw_text_layout_subpixel(
                    layout,
                    origin_x,
                    origin_y,
                    if is_sung { sung_col } else { up_col },
                );
                r.pop_clip();
            }
        };

        for (c_idx, cu) in char_units.iter().enumerate() {
            if cu.utf16_len == 0 {
                continue;
            }
            let Some(cells) = unit_cells.get(c_idx) else {
                continue;
            };
            if cells.is_empty() {
                continue;
            }
            let lift_y = lifts
                .get(c_idx)
                .map(|sp| if reduce_motion { 0.0 } else { sp.value })
                .unwrap_or(0.0);

            let prog = cu.sweep_progress(t_headstart);

            // Fast batch path for unscaled completely unsung/sung units using strictly disjoint clip cells
            if prog <= 0.002 || prog >= 0.996 {
                let is_sung = prog >= 0.996;
                for cell in cells {
                    let p_clip = RectF::new(
                        cell.clip_rect.x + origin_x,
                        cell.clip_rect.y + origin_y,
                        cell.clip_rect.w,
                        cell.clip_rect.h,
                    );
                    if let Some((cur, cur_ly, cur_sung)) = solid_run.as_mut() {
                        if *cur_sung == is_sung
                            && (p_clip.y - cur.y).abs() < 2.0
                            && (lift_y - *cur_ly).abs() < 0.002
                        {
                            let nx = cur.x.min(p_clip.x);
                            let nr = cur.right().max(p_clip.right());
                            let ny = cur.y.min(p_clip.y);
                            let nb = cur.bottom().max(p_clip.bottom());
                            *cur = RectF::new(nx, ny, nr - nx, nb - ny);
                            continue;
                        }
                    }
                    flush_run(r, &mut solid_run);
                    solid_run = Some((p_clip, lift_y, is_sung));
                }
                continue;
            }

            flush_run(r, &mut solid_run);

            let is_emph = cu.is_emphasized(auto_emphasis);
            let scale_enabled = allow_emphasis && !reduce_motion && (is_emph || non_emph_fx.scale());
            let dur_factor = (((cu.word_dur_sec - 0.28) / 1.40).clamp(0.32, 1.0)) as f32;
            let max_scale_delta = if !scale_enabled {
                0.0
            } else if is_emph {
                LyricsAnimator::EMPHASIZING_SCALE_DELTA * dur_factor
            } else {
                0.055
            };
            let sum_w: f32 = cells
                .iter()
                .map(|c| c.raw_rect.w.max(1.0))
                .sum::<f32>()
                .max(1.0);
            let use_frontier = frontier_wave
                && !reduce_motion
                && should_use_frontier_lift(
                    is_emph || scale_enabled,
                    cu.end_sec - cu.start_sec,
                    sum_w,
                );

            if use_frontier && cells.len() == 1 {
                let cell = cells[0];
                let gr = RectF::new(
                    cell.raw_rect.x + origin_x,
                    cell.raw_rect.y + origin_y,
                    cell.raw_rect.w.max(1.0),
                    cell.raw_rect.h.max(1.0),
                );
                let clip_r = RectF::new(
                    cell.clip_rect.x + origin_x,
                    cell.clip_rect.y + origin_y,
                    cell.clip_rect.w,
                    cell.clip_rect.h,
                );
                let word_sweep_rc = RectF::new(gr.x, clip_r.y, gr.w, clip_r.h);
                let feather = (gr.w * 0.55).clamp(5.0, LyricsAnimator::GRADIENT_FEATHER_PX);

                if let Some(sgs) = unit_sub_glyphs.get(c_idx).filter(|v| v.len() > 1) {
                    // Multi-character English/Latin word: per-glyph whole-letter frontier lift & scale!
                    // Coalesce contiguous unscaled letters outside the sweep feather band into `solid_run`
                    // so only letters actively crossing the sweep wave submit per-glyph `DrawTextLayout`!
                    let eff_feather = feather.min(word_sweep_rc.w * 0.85).max(8.0);
                    let total_travel = word_sweep_rc.w + eff_feather;
                    let edge_center = if is_rtl {
                        (word_sweep_rc.x + word_sweep_rc.w + eff_feather * 0.5)
                            - prog * total_travel
                    } else {
                        word_sweep_rc.x - eff_feather * 0.5 + prog * total_travel
                    };
                    let sweep_lo = edge_center - eff_feather * 0.5;
                    let sweep_hi = edge_center + eff_feather * 0.5;

                    for sg in sgs {
                        let g_lift = lift_y * eval_frontier_lift_weight(sg.local_u, prog, false);
                        let g_scale = if scale_enabled {
                            1.0 + eval_frontier_scale_delta(sg.local_u, prog, max_scale_delta, false)
                        } else {
                            1.0
                        };
                        let wl = sg.clip_rect.x + origin_x;
                        let wr = sg.clip_rect.right() + origin_x;
                        let solid_side = if (g_scale - 1.0).abs() < 0.002 {
                            let is_fully_sung = if is_rtl {
                                wl >= sweep_hi + 0.5
                            } else {
                                wr <= sweep_lo - 0.5
                            };
                            let is_fully_up = if is_rtl {
                                wr <= sweep_lo - 0.5
                            } else {
                                wl >= sweep_hi + 0.5
                            };
                            if is_fully_sung {
                                Some(true)
                            } else if is_fully_up {
                                Some(false)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        if let Some(is_sung) = solid_side {
                            let p_clip = RectF::new(
                                wl,
                                sg.clip_rect.y + origin_y,
                                (wr - wl).max(0.25),
                                sg.clip_rect.h,
                            );
                            if let Some((cur, cur_ly, cur_sung)) = solid_run.as_mut() {
                                if *cur_sung == is_sung
                                    && (p_clip.y - cur.y).abs() < 2.0
                                    && (g_lift - *cur_ly).abs() < 0.04
                                {
                                    let nx = cur.x.min(p_clip.x);
                                    let nr = cur.right().max(p_clip.right());
                                    let ny = cur.y.min(p_clip.y);
                                    let nb = cur.bottom().max(p_clip.bottom());
                                    *cur = RectF::new(nx, ny, nr - nx, nb - ny);
                                    continue;
                                }
                            }
                            flush_run(r, &mut solid_run);
                            solid_run = Some((p_clip, g_lift, is_sung));
                            continue;
                        }
                        flush_run(r, &mut solid_run);
                        let scx = sg.glyph_rect.center_x() + origin_x;
                        let scy = sg.glyph_rect.center_y() + origin_y;
                        let xf = Matrix3x2 {
                            M11: combined_scale * g_scale,
                            M12: 0.0,
                            M21: 0.0,
                            M22: combined_scale * g_scale,
                            M31: pivot_x * (1.0 - combined_scale)
                                + scx * (1.0 - g_scale) * combined_scale,
                            M32: pivot_y * (1.0 - combined_scale)
                                + (scy * (1.0 - g_scale) + g_lift) * combined_scale,
                        };
                        let cl = scx + (wl - scx) / g_scale;
                        let cr = scx + (wr - scx) / g_scale;
                        let sg_clip = RectF::new(
                            cl,
                            sg.clip_rect.y + origin_y - 6.0,
                            (cr - cl).max(0.25),
                            sg.clip_rect.h + 12.0,
                        );
                        r.set_transform(&xf);
                        r.push_clip(sg_clip);
                        r.draw_text_layout_feathered_sweep(
                            layout,
                            origin_x,
                            origin_y,
                            word_sweep_rc,
                            prog,
                            feather,
                            is_rtl,
                            sung_col,
                            up_col,
                        );
                        r.pop_clip();
                    }
                    continue;
                }

                // Single-character / CJK sustained or slow unit: continuous zero-tear affine elastic shear!
                // Never slices a Chinese character into disjoint vertical strips — strokes stay 100% unbroken!
                let (c_lift, c_shear) = eval_frontier_wave_shear(lift_y, gr.w, prog, is_rtl);
                let s_scale = if scale_enabled {
                    1.0 + eval_frontier_scale_delta(0.5, prog, max_scale_delta, is_rtl)
                } else {
                    1.0
                };
                let scx = gr.center_x();
                let scy = gr.center_y();
                let xf = Matrix3x2 {
                    M11: combined_scale * s_scale,
                    M12: combined_scale * c_shear,
                    M21: 0.0,
                    M22: combined_scale * s_scale,
                    M31: pivot_x * (1.0 - combined_scale)
                        + scx * (1.0 - s_scale) * combined_scale,
                    M32: pivot_y * (1.0 - combined_scale)
                        + (scy * (1.0 - s_scale) + c_lift - scx * c_shear) * combined_scale,
                };
                let cl = scx + (clip_r.x - scx) / s_scale;
                let cr = scx + (clip_r.right() - scx) / s_scale;
                let glyph_clip = RectF::new(
                    cl,
                    clip_r.y - 8.0,
                    (cr - cl).max(0.25),
                    clip_r.h + 16.0,
                );
                r.set_transform(&xf);
                r.push_clip(glyph_clip);
                r.draw_text_layout_feathered_sweep(
                    layout,
                    origin_x,
                    origin_y,
                    word_sweep_rc,
                    prog,
                    feather,
                    is_rtl,
                    sung_col,
                    up_col,
                );
                r.pop_clip();
                continue;
            }

            // Whole-unit path (fast non-emphasized words, wrapped multi-cell lines, or when frontier_wave is disabled)
            let mut acc_w = 0.0f32;
            for cell in cells {
                let gr = RectF::new(
                    cell.raw_rect.x + origin_x,
                    cell.raw_rect.y + origin_y,
                    cell.raw_rect.w,
                    cell.raw_rect.h,
                );
                let clip_r = RectF::new(
                    cell.clip_rect.x + origin_x,
                    cell.clip_rect.y + origin_y,
                    cell.clip_rect.w,
                    cell.clip_rect.h,
                );
                let seg_w = gr.w.max(1.0);
                let sub_prog = if prog <= 0.001 {
                    0.0
                } else if prog >= 0.999 {
                    1.0
                } else {
                    ((prog * sum_w - acc_w) / seg_w).clamp(0.0, 1.0)
                };
                acc_w += seg_w;

                let unit_scale = if scale_enabled && cells.len() == 1 {
                    1.0 + eval_frontier_scale_delta(0.5, sub_prog, max_scale_delta * 0.85, is_rtl)
                } else {
                    1.0
                };
                let scx = gr.center_x();
                let scy = gr.center_y();
                r.set_transform(&Matrix3x2 {
                    M11: combined_scale * unit_scale,
                    M12: 0.0,
                    M21: 0.0,
                    M22: combined_scale * unit_scale,
                    M31: pivot_x * (1.0 - combined_scale)
                        + scx * (1.0 - unit_scale) * combined_scale,
                    M32: pivot_y * (1.0 - combined_scale)
                        + (scy * (1.0 - unit_scale) + lift_y) * combined_scale,
                });
                let cl = scx + (clip_r.x - scx) / unit_scale;
                let cr = scx + (clip_r.right() - scx) / unit_scale;
                let adj_clip = RectF::new(cl, clip_r.y - 4.0, (cr - cl).max(0.25), clip_r.h + 8.0);
                r.push_clip(adj_clip);
                let sweep_rc = RectF::new(gr.x, clip_r.y, gr.w, clip_r.h);
                let feather = (gr.w * 0.55).clamp(5.0, LyricsAnimator::GRADIENT_FEATHER_PX);
                r.draw_text_layout_feathered_sweep(
                    layout,
                    origin_x,
                    origin_y,
                    sweep_rc,
                    sub_prog,
                    feather,
                    is_rtl,
                    sung_col,
                    up_col,
                );
                r.pop_clip();
            }
        }
        flush_run(r, &mut solid_run);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_queue_column(
        &mut self,
        r: &mut D2DRenderer,
        right_col: RectF,
        smtc: &SmtcState,
        hovered_pt: (f32, f32),
        mouse_down: bool,
        ctrl_scale: f32,
        col_alpha: f32,
    ) {
        let left_pad = if self.center_axis { 0.0 } else { 14.0 };
        let col_x = right_col.x + left_pad;
        let col_w = (right_col.w - left_pad - 12.0).max(200.0);

        // Header: "待播清单" + track count
        let header_h = 46.0;
        let title_sz = (21.0 * ctrl_scale.clamp(0.95, 1.24)).clamp(19.0, 26.0);
        r.draw_text(
            "待播清单",
            RectF::new(col_x + 4.0, right_col.y + 4.0, col_w - 110.0, 32.0),
            title_sz,
            700,
            Color::rgba(255, 255, 255, 0.96 * col_alpha),
            TextAlign::Left,
            true,
            false,
        );
        let count_lbl = format!("{} 首曲目", self.queue_tracks.len());
        r.draw_text(
            &count_lbl,
            RectF::new(col_x + col_w - 112.0, right_col.y + 8.0, 108.0, 26.0),
            12.8,
            500,
            Color::rgba(255, 255, 255, 0.52 * col_alpha),
            TextAlign::Right,
            true,
            false,
        );
        r.fill_rect(
            RectF::new(col_x + 2.0, right_col.y + header_h - 4.0, col_w - 4.0, 1.0),
            Color::rgba(255, 255, 255, 0.11 * col_alpha),
        );

        let list_rect = RectF::new(
            col_x,
            right_col.y + header_h + 4.0,
            col_w,
            (right_col.h - header_h - 14.0).max(120.0),
        );

        if self.queue_tracks.is_empty() {
            self.queue_max_scroll_y = 0.0;
            self.queue_scroll_y = 0.0;
            let ic_sz = 34.0;
            r.draw_icon(
                SfIcon::QueueList,
                RectF::new(
                    list_rect.center_x() - ic_sz * 0.5,
                    list_rect.center_y() - 42.0,
                    ic_sz,
                    ic_sz,
                ),
                Color::rgba(255, 255, 255, 0.30 * col_alpha),
            );
            r.draw_text(
                "当前待播清单为空",
                RectF::new(list_rect.x, list_rect.center_y() + 2.0, list_rect.w, 26.0),
                15.0,
                600,
                Color::rgba(255, 255, 255, 0.56 * col_alpha),
                TextAlign::Center,
                true,
                false,
            );
            return;
        }

        // Resolve active track index with O(1) memoization across frames (zero O(Q) scan at 144 Hz!)
        let q_len = self.queue_tracks.len();
        let q_ptr = self.queue_tracks.as_ptr() as usize;
        let smtc_aid = smtc.adam_id.as_deref().unwrap_or("");
        let smtc_t = smtc.title.trim();
        let smtc_a = smtc.artist.trim();
        let active_key_matches = self.cached_queue_active_key.0 == self.queue_tracks_rev
            && self.cached_queue_active_key.1 == q_len
            && self.cached_queue_active_key.2 == q_ptr
            && self.cached_queue_active_key.3 == smtc_aid
            && self.cached_queue_active_key.4 == smtc_t
            && self.cached_queue_active_key.5 == smtc_a;
        let active_idx = if active_key_matches {
            self.cached_queue_active_idx
        } else {
            let resolved = if !smtc_aid.is_empty() {
                self.queue_tracks
                    .iter()
                    .position(|t| t.effective_adam_id_str() == smtc_aid || t.id == smtc_aid)
            } else {
                None
            }
            .or_else(|| {
                if smtc_t.is_empty() {
                    None
                } else {
                    self.queue_tracks.iter().position(|t| {
                        t.name.trim().eq_ignore_ascii_case(smtc_t)
                            && (smtc_a.is_empty()
                                || t.artist_name.trim().eq_ignore_ascii_case(smtc_a))
                    })
                }
            });
            self.cached_queue_active_key = (
                self.queue_tracks_rev,
                q_len,
                q_ptr,
                smtc_aid.to_string(),
                smtc_t.to_string(),
                smtc_a.to_string(),
            );
            self.cached_queue_active_idx = resolved;
            resolved
        };

        let row_h = 56.0f32;
        let row_stride = Self::QUEUE_ROW_STRIDE;
        let total_h = (self.queue_tracks.len() as f32) * row_stride;
        self.queue_max_scroll_y = (total_h - list_rect.h).max(0.0);
        if !self.was_showing_queue && self.queue_scroll_y <= 0.5 {
            if let Some(ai) = active_idx {
                self.queue_scroll_y = ((ai as f32) * row_stride - list_rect.h * 0.25)
                    .clamp(0.0, self.queue_max_scroll_y);
            }
        }
        self.queue_scroll_y = self.queue_scroll_y.clamp(0.0, self.queue_max_scroll_y);

        let sb_reserve = if self.queue_max_scroll_y > 1.0 { 14.0 } else { 2.0 };
        let row_w = (list_rect.w - sb_reserve).max(160.0);
        let thumb_sz = 42.0f32;
        let thumb_r = 7.0f32;
        let dur_w = 54.0f32;

        // O(1) arithmetic viewport slice — renders ONLY visible rows without scanning offscreen tracks!
        let (vis_start, vis_end) = Self::compute_queue_visible_range(
            self.queue_tracks.len(),
            self.queue_scroll_y,
            list_rect.h,
        );
        r.push_clip(list_rect);
        for idx in vis_start..vis_end {
            let tr = &self.queue_tracks[idx];
            let y = list_rect.y + (idx as f32) * row_stride - self.queue_scroll_y;
            if y + row_h < list_rect.y - 4.0 || y > list_rect.bottom() + 4.0 {
                continue;
            }
            let row = RectF::new(list_rect.x, y, row_w, row_h);
            let clip_y0 = row.y.max(list_rect.y);
            let clip_y1 = row.bottom().min(list_rect.bottom());
            let hit_r = RectF::new(row.x, clip_y0, row.w, (clip_y1 - clip_y0).max(0.0));
            let is_cur = active_idx == Some(idx);
            let is_hov = hit_r.h > 1.0 && hit_r.contains(hovered_pt.0, hovered_pt.1);

            if is_cur {
                let bg_a = if is_hov { 0.20 } else { 0.15 };
                r.fill_rounded_rect(row, 12.0, Color::rgba(255, 255, 255, bg_a * col_alpha));
                r.stroke_rounded_rect(
                    row,
                    12.0,
                    Color::rgba(255, 255, 255, 0.14 * col_alpha),
                    1.0,
                );
            } else if is_hov {
                let hov_a = if mouse_down { 0.15 } else { 0.095 };
                r.fill_rounded_rect(row, 12.0, Color::rgba(255, 255, 255, hov_a * col_alpha));
            }

            // Album thumbnail (42x42 rounded)
            let thumb_rect = RectF::new(
                row.x + 8.0,
                row.center_y() - thumb_sz * 0.5,
                thumb_sz,
                thumb_sz,
            );
            let drew_thumb = tr
                .artwork_url
                .as_deref()
                .filter(|u| !u.is_empty())
                .map(|u| r.draw_rounded_artwork(u, false, thumb_rect, thumb_r, col_alpha))
                .unwrap_or(false);
            if !drew_thumb {
                r.fill_rounded_rect(
                    thumb_rect,
                    thumb_r,
                    Color::rgba(255, 255, 255, 0.10 * col_alpha),
                );
                r.draw_icon(
                    SfIcon::MusicNote,
                    thumb_rect.inset(11.0, 11.0),
                    Color::rgba(255, 255, 255, 0.42 * col_alpha),
                );
            }
            if is_cur {
                r.fill_rounded_rect(
                    thumb_rect,
                    thumb_r,
                    Color::rgba(0, 0, 0, 0.44 * col_alpha),
                );
                r.draw_icon(
                    SfIcon::Visualizer,
                    thumb_rect.inset(11.0, 11.0),
                    Color::rgba(255, 255, 255, 0.96 * col_alpha),
                );
            }
            r.stroke_rounded_rect(
                thumb_rect,
                thumb_r,
                Color::rgba(255, 255, 255, 0.12 * col_alpha),
                1.0,
            );

            // Track duration (tabular mono digits, right-aligned)
            let dur_x = row.right() - dur_w - 10.0;
            if tr.duration_sec > 0.5 {
                let dur_col = if is_cur {
                    Color::rgba(255, 255, 255, 0.88 * col_alpha)
                } else {
                    Color::rgba(255, 255, 255, 0.48 * col_alpha)
                };
                r.draw_mono_text(
                    &format_mmss(tr.duration_sec),
                    RectF::new(dur_x, row.y, dur_w, row.h),
                    12.2,
                    500,
                    dur_col,
                    TextAlign::Right,
                    true,
                );
            }

            // Track title + artist
            let text_x = thumb_rect.right() + 12.0;
            let text_w = (dur_x - text_x - 8.0).max(64.0);
            let title_col = if is_cur {
                Color::rgba(255, 255, 255, 1.0 * col_alpha)
            } else {
                Color::rgba(255, 255, 255, 0.92 * col_alpha)
            };
            let artist_col = if is_cur {
                Color::rgba(255, 255, 255, 0.78 * col_alpha)
            } else {
                Color::rgba(255, 255, 255, 0.56 * col_alpha)
            };
            r.draw_text(
                &tr.name,
                RectF::new(text_x, row.y + 8.0, text_w, 21.0),
                14.2,
                if is_cur { 700 } else { 600 },
                title_col,
                TextAlign::Left,
                true,
                false,
            );
            let sub = if tr.artist_name.is_empty() {
                tr.album_name.as_str()
            } else {
                tr.artist_name.as_str()
            };
            r.draw_text(
                sub,
                RectF::new(text_x, row.y + 30.0, text_w, 18.0),
                12.4,
                400,
                artist_col,
                TextAlign::Left,
                true,
                false,
            );

            if hit_r.h > 2.0 && col_alpha > 0.25 {
                self.hit_zones.push(LyricsHitZone {
                    rect: hit_r,
                    action: LyricsHitAction::JumpLyricsQueueIndex(idx),
                    line_idx: None,
                });
            }
        }
        r.pop_clip();

        // Translucent scrollbar on right edge
        if self.queue_max_scroll_y > 1.0 {
            let track_h = (list_rect.h - 8.0).max(24.0);
            let thumb_h = ((list_rect.h / total_h.max(1.0)) * track_h).clamp(28.0, track_h);
            let frac = (self.queue_scroll_y / self.queue_max_scroll_y).clamp(0.0, 1.0);
            let thumb_y = list_rect.y + 4.0 + frac * (track_h - thumb_h);
            let sb_x = list_rect.right() - 5.0;
            r.fill_rounded_rect(
                RectF::new(sb_x, list_rect.y + 4.0, 3.8, track_h),
                2.0,
                Color::rgba(255, 255, 255, 0.08 * col_alpha),
            );
            r.fill_rounded_rect(
                RectF::new(sb_x, thumb_y, 3.8, thumb_h),
                2.0,
                Color::rgba(255, 255, 255, 0.36 * col_alpha),
            );
        }
    }

    fn draw_bottom_right_glass_controls(
        &mut self,
        r: &mut D2DRenderer,
        win_rect: RectF,
        hovered_pt: (f32, f32),
        mouse_down: bool,
        glass_off: bool,
        corner_alpha: f32,
    ) {
        // Sleek 3-icon Apple Music cluster (left -> right):
        // 1. 翻译 (Translate), 2. 歌词显示开关 (LyricsBubble), 3. 播放列表 (QueueList)
        let items: [(SfIcon, bool, LyricsHitAction); 3] = [
            (
                SfIcon::Translate,
                self.show_translation,
                LyricsHitAction::ToggleTranslation,
            ),
            (
                SfIcon::LyricsBubble,
                self.show_lyrics && !self.show_queue,
                LyricsHitAction::ToggleLyricsVisible,
            ),
            (
                SfIcon::QueueList,
                self.show_queue,
                LyricsHitAction::ToggleLyricsQueue,
            ),
        ];

        let btn_w = 32.0f32;
        let btn_h = 32.0f32;
        let inner_gap = 6.0f32;
        let pad_h = 5.0f32;
        let pad_v = 5.0f32;
        let total_w = pad_h * 2.0
            + btn_w * (items.len() as f32)
            + inner_gap * ((items.len() - 1) as f32);
        let pill_h = btn_h + pad_v * 2.0;
        let pill = RectF::new(
            win_rect.right() - total_w - 24.0,
            win_rect.bottom() - pill_h - 18.0,
            total_w,
            pill_h,
        );

        let pushed_br = if corner_alpha < 0.995 {
            r.push_opacity_layer(pill.inset(-16.0, -16.0), corner_alpha)
        } else {
            false
        };

        r.draw_liquid_glass(
            pill,
            pill_h * 0.5,
            14.0,
            Color::rgba(22, 24, 30, 0.28),
            Theme::GLASS_RIM_LIGHT,
            glass_off,
        );

        let mut x = pill.x + pad_h;
        for (icon, active, act) in items {
            let br = RectF::new(x, pill.y + pad_v, btn_w, btn_h);
            let is_hov = br.contains(hovered_pt.0, hovered_pt.1);
            let scale = if is_hov && mouse_down {
                0.94
            } else if is_hov {
                1.06
            } else {
                1.0
            };

            if active {
                let fill_a = if is_hov && mouse_down {
                    0.30
                } else if is_hov {
                    0.28
                } else {
                    0.24
                };
                r.fill_rounded_rect(br, btn_h * 0.5, Color::rgba(255, 255, 255, fill_a));
                r.stroke_rounded_rect(br, btn_h * 0.5, Color::rgba(255, 255, 255, 0.28), 1.0);
            } else if is_hov {
                let hov_a = if mouse_down { 0.20 } else { 0.14 };
                r.fill_rounded_rect(br, btn_h * 0.5, Color::rgba(255, 255, 255, hov_a));
            }

            let ic_sz = 16.5 * scale;
            let ic_rect = RectF::new(
                br.center_x() - ic_sz * 0.5,
                br.center_y() - ic_sz * 0.5,
                ic_sz,
                ic_sz,
            );
            let ic_col = if active {
                Color::rgb(255, 255, 255)
            } else if is_hov {
                Color::rgba(255, 255, 255, 0.92)
            } else {
                Color::rgba(255, 255, 255, 0.56)
            };
            r.draw_icon(icon, ic_rect, ic_col);

            if corner_alpha > 0.20 {
                self.hit_zones.push(LyricsHitZone {
                    rect: br,
                    action: act,
                    line_idx: None,
                });
            }
            x += btn_w + inner_gap;
        }
        if pushed_br {
            r.pop_layer();
        }
    }
}

fn format_mmss(sec: f64) -> String {
    let s = sec.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
