#![cfg(target_os = "windows")]

use crate::d2d_renderer::{D2DRenderer, GlassBtnVariant, LyricsLineBmpKey, SfIcon, TextAlign};
use crate::lyrics::{
    eval_frontier_glow_profile, eval_frontier_lift_weight, eval_frontier_scale_delta,
    eval_frontier_wave_shear, should_use_frontier_lift, LyricCharUnit, LyricsAnimator,
    LyricsSourcePref, NonEmphasisLyricsEffect, ParsedLyrics, Spring1D,
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
    NudgeLyricsOffset(f64),
    ResetLyricsOffset,
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

pub struct LyricsViewState {
    pub animator: LyricsAnimator,
    pub show_translation: bool,
    pub music_free_blur: bool,
    pub source_pref: LyricsSourcePref,
    pub sync_offset_sec: f64,
    pub synced_playhead_sec: f64,
    last_raw_playhead_sec: f64,
    last_calibrated_offset_sec: f64,
    last_offset_track_key: String,
    last_offset_meta_key: String,
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
    cached_layout_key: CachedLyricsLayoutKey,
    cached_line_layouts: Vec<CachedLyricLineLayout>,
}

impl Default for LyricsViewState {
    fn default() -> Self {
        Self {
            animator: LyricsAnimator::default(),
            show_translation: true,
            music_free_blur: false,
            source_pref: LyricsSourcePref::Auto,
            sync_offset_sec: 0.0,
            synced_playhead_sec: 0.0,
            last_raw_playhead_sec: -1.0,
            last_calibrated_offset_sec: 0.0,
            last_offset_track_key: String::new(),
            last_offset_meta_key: String::new(),
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
            cached_layout_key: CachedLyricsLayoutKey::default(),
            cached_line_layouts: Vec::new(),
        }
    }
}

impl LyricsViewState {
    pub fn has_active_layout_motion(&self) -> bool {
        !self.center_layout_spring.is_settled_norm() || self.cover_vel.abs() > 0.01
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
        if !reduce_motion {
            self.mesh_phase = (self.mesh_phase + dt * speed) % 1000.0;
        }

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

        // Smart instrumental / no-lyrics nonlinear centering spring (hold split layout up to 6.0s while fetching!)
        if lyrics_loading && lyrics.is_none() && self.loading_hold_sec < 6.0 {
            self.loading_hold_sec += dt;
            self.center_layout_spring.target = 0.0;
        } else {
            if !lyrics_loading {
                self.loading_hold_sec = 0.0;
            }
            let is_no_lyrics = crate::lyrics::is_effectively_instrumental_or_empty(lyrics);
            self.center_layout_spring.target = if is_no_lyrics { 1.0 } else { 0.0 };
        }
        if reduce_motion {
            self.center_layout_spring.snap(self.center_layout_spring.target);
        } else {
            self.center_layout_spring.step(dt);
        }

        if (self.sync_offset_sec - self.last_calibrated_offset_sec).abs() > 1e-5 {
            self.animator.reset_word_clamps();
        }
        self.last_raw_playhead_sec = playhead_sec;
        self.last_calibrated_offset_sec = self.sync_offset_sec;
        self.synced_playhead_sec = self.calibrated_playhead(playhead_sec);
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
        sheet_alpha: f32,
        motion_frame: Option<&MotionFrameSnapshot>,
    ) {
        let sa = sheet_alpha.clamp(0.0, 1.0);
        self.animator.latin_by_word = latin_by_word;
        self.hit_zones.clear();
        self.progress_bar_rect = RectF::default();
        self.volume_bar_rect = RectF::default();

        if !smtc.title.trim().is_empty() {
            let meta_key = format!(
                "{}|{}",
                smtc.title.trim().to_lowercase(),
                smtc.artist.trim().to_lowercase()
            );
            if !self.last_offset_meta_key.is_empty() && self.last_offset_meta_key != meta_key {
                self.sync_offset_sec = 0.0;
                self.last_raw_playhead_sec = smtc.position_sec;
                self.last_calibrated_offset_sec = 0.0;
                self.synced_playhead_sec = self.calibrated_playhead(smtc.position_sec);
            }
            self.last_offset_meta_key = meta_key;
        }

        let lyr_hash = lyrics.map(|l| l.content_hash()).unwrap_or(0);
        let cache_tag = format!(
            "{}#h:{:016x}#tr:{}",
            lyrics_key, lyr_hash, self.show_translation as u8
        );
        r.set_lyrics_bmp_track(&cache_tag);

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

        // Top-Left Close Chevron Circle
        let close_btn = RectF::new(win_rect.x + 20.0, top_y, btn_h, btn_h);
        let close_hov = close_btn.contains(hovered_pt.0, hovered_pt.1);
        r.draw_liquid_glass_button(
            close_btn,
            btn_r,
            GlassBtnVariant::Regular,
            close_hov,
            mouse_down,
            hovered_pt,
            glass_off,
        );
        r.draw_icon(SfIcon::ChevronDown, close_btn.inset(9.0, 9.0), Theme::TEXT_PRIMARY);
        self.hit_zones.push(LyricsHitZone {
            rect: close_btn,
            action: LyricsHitAction::CloseLyrics,
            line_idx: None,
        });

        // Top-Right Unified Window Controls (`─`, `□`, `×`), pixel-identical to Main UI
        let close_x = win_rect.right() - 18.0 - btn_h;
        let max_x = close_x - 8.0 - btn_h;
        let min_x = max_x - 8.0 - btn_h;
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
            r.draw_liquid_glass_button(
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
            self.hit_zones.push(LyricsHitZone {
                rect: wb,
                action: wact,
                line_idx: None,
            });
        }

        // Responsive Viewport Scale (scales proportionally on large / maximized / 4K displays!)
        let ui_scale = ((win_rect.w / 1180.0).min(win_rect.h / 760.0)).clamp(0.88, 1.80);
        let ctrl_scale = ui_scale.clamp(0.92, 1.45);
        let center_t = self.center_layout_spring.value.clamp(0.0, 1.0);

        // Split View Layout: Left ~43% Artwork & Transport, Right ~57% SyncedLyricsViewController
        let left_w = (win_rect.w * 0.43).clamp(300.0, 860.0);
        let pad_x = (36.0 * ui_scale).clamp(28.0, 76.0);
        let left_col = RectF::new(
            win_rect.x + pad_x,
            win_rect.y + 58.0,
            (left_w - pad_x * 1.35).max(220.0),
            (win_rect.h - 98.0).max(220.0),
        );
        let right_col = RectF::new(
            win_rect.x + left_w + 8.0,
            win_rect.y + 54.0,
            (win_rect.w - left_w - pad_x * 0.85 - 8.0).max(260.0),
            (win_rect.h - 108.0).max(200.0),
        );
        self.lyrics_column_rect = if center_t > 0.85 {
            RectF::default()
        } else {
            right_col
        };

        // --- Draw Left Column (or Centered Stage when Instrumental / No Lyrics): ---
        let base_cov = (left_col.w * 0.86)
            .min(left_col.h * 0.54)
            .clamp(180.0, 620.0);
        let max_cov = base_cov * (1.0 + 0.07 * center_t);
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
        // Right `•••` Liquid Glass circle aligned with title/artist
        let dots_btn = RectF::new(meta_x + meta_w - dots_sz, my + 5.0 * ctrl_scale, dots_sz, dots_sz);
        let dots_hov = dots_btn.contains(hovered_pt.0, hovered_pt.1);
        r.draw_liquid_glass_button(
            dots_btn,
            dots_sz * 0.5,
            GlassBtnVariant::Subtle,
            dots_hov,
            mouse_down,
            hovered_pt,
            glass_off,
        );
        r.draw_icon(
            SfIcon::MoreDots,
            dots_btn.inset(dots_sz * 0.25, dots_sz * 0.25),
            Color::rgba(255, 255, 255, 0.88),
        );
        self.hit_zones.push(LyricsHitZone {
            rect: dots_btn,
            action: LyricsHitAction::ImportLocalLyrics,
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
        r.draw_mono_text(
            &format_mmss(pos),
            RectF::new(meta_x, my, 76.0, 18.0),
            time_sz,
            500,
            Color::rgba(255, 255, 255, 0.54),
            TextAlign::Left,
            true,
        );
        let rem = (dur - pos).max(0.0);
        r.draw_mono_text(
            &format!("-{}", format_mmss(rem)),
            RectF::new(meta_x + meta_w - 84.0, my, 84.0, 18.0),
            time_sz,
            500,
            Color::rgba(255, 255, 255, 0.54),
            TextAlign::Right,
            true,
        );
        my += 24.0 * ctrl_scale;

        // 5-Button Liquid Glass Transport Row: Shuffle / Prev / Play-Pause / Next / Repeat
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
            if bhov || tint_active {
                r.draw_liquid_glass_button(
                    br,
                    sz * 0.5,
                    if tint_active {
                        GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                    } else {
                        GlassBtnVariant::Regular
                    },
                    bhov,
                    mouse_down,
                    hovered_pt,
                    glass_off,
                );
            }
            if is_center_play && playback_loading {
                r.draw_loading_spinner(
                    br.center_x(),
                    br.center_y(),
                    sz * 0.36,
                    spinner_phase,
                    Theme::TEXT_PRIMARY,
                );
            } else {
                let ic_col = if tint_active || sz > sz_sm + 2.0 || bhov {
                    Theme::TEXT_PRIMARY
                } else {
                    Color::rgba(255, 255, 255, 0.72)
                };
                r.draw_icon(ic, br.inset(sz * 0.22, sz * 0.22), ic_col);
            }
            self.hit_zones.push(LyricsHitZone {
                rect: br,
                action: act,
                line_idx: None,
            });
        }
        my += ctrl_row_h;

        // Interactive Volume Slider
        if my + 24.0 < win_rect.bottom() - 28.0 {
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

        // Bottom-left subtle playback & lyrics source attribution
        let is_no_lyrics = crate::lyrics::is_effectively_instrumental_or_empty(lyrics);
        let src_caption = if lyrics_loading && lyrics.is_none() {
            "AM Light · 正在匹配同步歌词...".to_string()
        } else if is_no_lyrics {
            if lyrics.is_some() {
                "AM Light · 纯音乐".to_string()
            } else {
                format!("AM Light · {}", self.source_pref.label())
            }
        } else if let Some(l) = lyrics {
            format!("AM Light · {}", l.source_name)
        } else {
            format!("AM Light · {}", self.source_pref.label())
        };
        r.draw_icon(
            SfIcon::Radio,
            RectF::new(win_rect.x + 28.0, win_rect.bottom() - 34.0, 16.0, 16.0),
            Color::rgba(255, 255, 255, 0.45),
        );
        r.draw_text(
            &src_caption,
            RectF::new(win_rect.x + 50.0, win_rect.bottom() - 36.0, 320.0, 20.0),
            11.8,
            500,
            Color::rgba(255, 255, 255, 0.48),
            TextAlign::Left,
            true,
            false,
        );

        // --- Draw Right Column: SyncedLyricsViewController (Hidden smoothly when Instrumental / No Lyrics!) ---
        let lyrics_col_alpha = ((1.0 - center_t * 1.38).clamp(0.0, 1.0) * sa).clamp(0.0, 1.0);
        if lyrics_col_alpha > 0.01 {
            let shifted_right = RectF::new(
                right_col.x + (1.0 - lyrics_col_alpha) * 32.0,
                right_col.y,
                right_col.w,
                right_col.h,
            );

            if let Some(lyr) = lyrics.filter(|l| !crate::lyrics::is_effectively_instrumental_or_empty(Some(l))) {
                let synced_playhead = if self.last_raw_playhead_sec >= 0.0
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
                    lyrics_col_alpha,
                );
            } else if lyrics_loading {
                r.draw_text(
                    "正在匹配逐字同步歌词...",
                    RectF::new(
                        shifted_right.x + 20.0,
                        shifted_right.center_y() - 30.0,
                        shifted_right.w - 40.0,
                        60.0,
                    ),
                    18.0 * ctrl_scale,
                    600,
                    Color::rgba(255, 255, 255, 0.55 * lyrics_col_alpha),
                    TextAlign::Left,
                    true,
                    true,
                );
            }
        }

        // Bottom-Right Floating Controls Capsule
        self.draw_bottom_right_glass_controls(
            r,
            win_rect,
            hovered_pt,
            mouse_down,
            glass_off,
        );
    }

    pub fn invalidate_layout_cache(&mut self) {
        self.cached_layout_key = CachedLyricsLayoutKey::default();
        self.cached_line_layouts.clear();
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
        r.release_lyrics_gpu_surfaces();
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
        let key = CachedLyricsLayoutKey {
            track_key: lyrics_key.to_string(),
            content_hash: lyr.content_hash(),
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

        let query_unit_cells = |lay: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
                                units: &[LyricCharUnit]|
         -> Vec<Vec<DisjointUnitCell>> {
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
        };

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

            let query_sub_glyphs = |lay: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
                                    units: &[LyricCharUnit],
                                    unit_cells: &[Vec<DisjointUnitCell>]|
             -> Vec<Vec<SubGlyphCell>> {
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
                                    let u16_off =
                                        cu.ch_str[..byte_idx].encode_utf16().count() as u32;
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
            };

            let word_cjk_cells = query_unit_cells(&main_layout, &line.word_cjk_units);
            let char_cells = query_unit_cells(&main_layout, &line.char_units);
            let word_cjk_sub_glyphs =
                query_sub_glyphs(&main_layout, &line.word_cjk_units, &word_cjk_cells);
            let (bg_word_cjk_cells, bg_char_cells, bg_word_cjk_sub_glyphs) =
                if let Some((ref bg_lay, _)) = bg_layout_opt {
                    let bwc = query_unit_cells(bg_lay, &line.bg_word_cjk_units);
                    let bcc = query_unit_cells(bg_lay, &line.bg_char_units);
                    let bwsg = query_sub_glyphs(bg_lay, &line.bg_word_cjk_units, &bwc);
                    (bwc, bcc, bwsg)
                } else {
                    (Vec::new(), Vec::new(), Vec::new())
                };

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
                word_cjk_cells,
                char_cells,
                bg_word_cjk_cells,
                bg_char_cells,
                word_cjk_sub_glyphs,
                bg_word_cjk_sub_glyphs,
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

        let mut visible_lines: Vec<VisibleLineSlot> = Vec::with_capacity(18);
        let mut next_glow_slot = 0usize;
        let mut blur_bakes_this_frame = 0usize;

        // PASS 1 (Clip/Layer Depth == 0!):
        // - O(1) float frustum culling using pre-cached `total_h` (zero DirectWrite calls!)
        // - Pre-warm blurred depth-of-field bitmaps ONLY for actually blurred non-vector lines
        //   (budgeted to max 2 GPU blur bakes per frame to eliminate first-frame/scroll spikes!)
        // - Bake per-character/word glyph emission into `active_glow_slots[0..2]` only when units glow
        for i in 0..lyr.lines.len() {
            let cl = &self.cached_line_layouts[i];
            if let Some(st_mut) = self.animator.line_states.get_mut(i) {
                st_mut.measured_height = cl.total_h;
            }
            let Some(st) = self.animator.line_states.get(i) else {
                continue;
            };

            let screen_y = right_col.y + st.y_spring.value - self.animator.user_scroll_offset;
            if screen_y + cl.total_h < right_col.y - 84.0 || screen_y > right_col.bottom() + 84.0 {
                continue;
            }

            let line_mid_y = screen_y + cl.total_h * 0.5;
            let top_t = ((line_mid_y - right_col.y) / 68.0).clamp(0.0, 1.0);
            let bot_t = ((right_col.bottom() - line_mid_y) / 76.0).clamp(0.0, 1.0);
            let st_top = top_t * top_t * (3.0 - 2.0 * top_t);
            let st_bot = bot_t * bot_t * (3.0 - 2.0 * bot_t);
            let edge_fade = (st_top * st_bot).clamp(0.0, 1.0);

            let line = &lyr.lines[i];
            let origin_x = if cl.align == TextAlign::Right {
                right_col.right() - vocal_max_w - 20.0
            } else {
                right_col.x + 16.0
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
                if pre_sigma > 0.04 && blur_bakes_this_frame < 20 {
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
                        let (peak, has_glow) = Self::bake_active_line_glyph_emission(
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
                            active_w * col_alpha * edge_fade,
                            non_emph_fx,
                            auto_emphasis,
                            frontier_wave,
                        );
                        if has_glow && peak > 0.005 {
                            let inner_sigma = 3.6f32;
                            let inner_alpha = (0.24 + peak * 0.26) * active_w * col_alpha * edge_fade;
                            let outer_sigma = 8.8 + peak * 6.0;
                            let outer_alpha = (0.20 + peak * 0.34) * active_w * col_alpha * edge_fade;
                            r.finish_and_bake_active_lyrics_glow_slot(
                                slot,
                                inner_sigma,
                                inner_alpha,
                                outer_sigma,
                                outer_alpha,
                            );
                            glow_peak = peak;
                            glow_slot = Some(slot);
                            next_glow_slot += 1;
                        } else {
                            r.end_active_lyrics_glow_slot();
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
            let base_x = right_col.x + 22.0;
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
                    r.draw_text(
                        &credit_txt,
                        RectF::new(right_col.x + 16.0, credit_y, vocal_max_w, 48.0),
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
    ) -> (f32, bool) {
        if char_units.is_empty() {
            if non_emph_fx.glow() {
                r.draw_text_layout_subpixel(
                    layout,
                    local_x,
                    local_y,
                    Color::rgba(255, 255, 255, 0.36 * active_w),
                );
                return (0.25 * active_w, true);
            }
            return (0.0, false);
        }

        let mut peak_emphasis = 0.0f32;
        let mut has_any_glow = false;

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
            let (body_a, peak_a, _tail_decay) = eval_frontier_glow_profile(
                prog,
                hold_after_end_sec,
                decay_sec,
                is_holding,
                is_emph,
                dur_factor,
            );
            if body_a <= 0.01 && peak_a <= 0.015 {
                continue;
            }
            peak_emphasis = peak_emphasis.max(
                peak_a * if is_emph {
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
                let sgs = &sub_glyphs[c_idx];
                for sg in sgs {
                    let g_lift = lift_y * eval_frontier_lift_weight(sg.local_u, prog, false);
                    let g_scale = if scale_enabled {
                        1.0 + eval_frontier_scale_delta(sg.local_u, prog, max_scale_delta, false)
                    } else {
                        1.0
                    };
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
                    let wl = sg.clip_rect.x + local_x;
                    let wr = sg.clip_rect.right() + local_x;
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
        (peak_emphasis, has_any_glow)
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
                    for sg in sgs {
                        let g_lift = lift_y * eval_frontier_lift_weight(sg.local_u, prog, false);
                        let g_scale = if scale_enabled {
                            1.0 + eval_frontier_scale_delta(sg.local_u, prog, max_scale_delta, false)
                        } else {
                            1.0
                        };
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
                        let wl = sg.clip_rect.x + origin_x;
                        let wr = sg.clip_rect.right() + origin_x;
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

    fn draw_bottom_right_glass_controls(
        &mut self,
        r: &mut D2DRenderer,
        win_rect: RectF,
        hovered_pt: (f32, f32),
        mouse_down: bool,
        glass_off: bool,
    ) {
        let src_lbl = self.source_pref.label();
        let blur_lbl = if self.music_free_blur {
            "流光"
        } else {
            "柔焦"
        };
        let off_active = self.sync_offset_sec.abs() >= 0.04;
        let off_lbl = if off_active {
            format!("{:+.1}s", self.sync_offset_sec)
        } else {
            "0.0s".to_string()
        };
        let items: [(String, f32, bool, LyricsHitAction); 7] = [
            (
                "-0.1s".to_string(),
                40.0,
                false,
                LyricsHitAction::NudgeLyricsOffset(-0.1),
            ),
            (
                off_lbl,
                44.0,
                off_active,
                LyricsHitAction::ResetLyricsOffset,
            ),
            (
                "+0.1s".to_string(),
                40.0,
                false,
                LyricsHitAction::NudgeLyricsOffset(0.1),
            ),
            (
                "译".to_string(),
                34.0,
                self.show_translation,
                LyricsHitAction::ToggleTranslation,
            ),
            (
                src_lbl.to_string(),
                76.0,
                self.source_pref != LyricsSourcePref::Auto,
                LyricsHitAction::CycleLyricsSource,
            ),
            (
                blur_lbl.to_string(),
                46.0,
                self.music_free_blur,
                LyricsHitAction::ToggleFreeBlur,
            ),
            (
                "导入".to_string(),
                46.0,
                false,
                LyricsHitAction::ImportLocalLyrics,
            ),
        ];

        let inner_gap = 4.0;
        let total_w: f32 =
            items.iter().map(|x| x.1).sum::<f32>() + inner_gap * ((items.len() - 1) as f32) + 12.0;
        let pill_h = 36.0;
        let pill = RectF::new(
            win_rect.right() - total_w - 22.0,
            win_rect.bottom() - pill_h - 16.0,
            total_w,
            pill_h,
        );

        r.draw_liquid_glass(
            pill,
            18.0,
            16.0,
            Color::rgba(36, 36, 44, 0.56),
            Theme::GLASS_RIM_LIGHT,
            glass_off,
        );

        let mut x = pill.x + 6.0;
        for (lbl, bw, active, act) in items {
            let br = RectF::new(x, pill.y + 4.0, bw, 28.0);
            let is_hov = br.contains(hovered_pt.0, hovered_pt.1);
            if active {
                r.draw_liquid_glass_button(
                    br,
                    14.0,
                    GlassBtnVariant::ProminentLight,
                    is_hov,
                    mouse_down,
                    hovered_pt,
                    glass_off,
                );
            } else if is_hov {
                r.draw_liquid_glass_button(
                    br,
                    14.0,
                    GlassBtnVariant::Regular,
                    true,
                    mouse_down,
                    hovered_pt,
                    glass_off,
                );
            }
            r.draw_text(
                &lbl,
                br,
                11.5,
                if active { 700 } else { 500 },
                if active {
                    Color::rgb(14, 14, 18)
                } else {
                    Theme::TEXT_PRIMARY
                },
                TextAlign::Center,
                true,
                false,
            );
            self.hit_zones.push(LyricsHitZone {
                rect: br,
                action: act,
                line_idx: None,
            });
            x += bw + inner_gap;
        }
    }
}

fn format_mmss(sec: f64) -> String {
    let s = sec.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
