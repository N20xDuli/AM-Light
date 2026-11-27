#![cfg(target_os = "windows")]

use crate::am_agent::AudioPrefs;
use crate::audio_viz::{AudioOutputDevice, LiveVisualizerSnapshot, StreamAudioSpecs};
use crate::d2d_renderer::{D2DRenderer, GlassBtnVariant, SfIcon, TextAlign};
use crate::smtc::SmtcState;
use crate::theme::{Color, RectF, Theme};
use std::collections::VecDeque;
use std::f32::consts::{FRAC_1_SQRT_2, PI};

/// Maps stereo `(L, R)` samples into an upper-semicircle vector goniometer coordinate `(nx, ny)`
/// with `ny >= 0.0` and `nx^2 + ny^2 <= 1.0`, folding negative mid across the origin while
/// preserving stereo panning polarity (`nx < 0` Left, `nx > 0` Right).
#[inline]
pub fn map_lr_to_semicircle(l: f32, r: f32, gain: f32) -> (f32, f32) {
    let mut s = (r - l) * FRAC_1_SQRT_2 * gain;
    let mut m = (l + r) * FRAC_1_SQRT_2 * gain;
    if m < 0.0 {
        m = -m;
        s = -s;
    }
    let rad = (s * s + m * m).sqrt();
    if rad <= 1e-6 {
        return (0.0, 0.0);
    }
    let comp = rad.tanh().min(0.96);
    let scale = comp / rad;
    (s * scale, (m * scale).max(0.0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VizStyle {
    AudioInspector,
    OsuCircle,
    ChernobylRods,
    WinXpRetro,
    SilkAurora,
}

impl VizStyle {
    pub const ALL: [VizStyle; 5] = [
        VizStyle::AudioInspector,
        VizStyle::OsuCircle,
        VizStyle::ChernobylRods,
        VizStyle::WinXpRetro,
        VizStyle::SilkAurora,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VizStyle::AudioInspector => "音质鉴真",
            VizStyle::OsuCircle => "律动圆环",
            VizStyle::ChernobylRods => "反应堆芯",
            VizStyle::WinXpRetro => "XP 复古",
            VizStyle::SilkAurora => "流光丝绸",
        }
    }
}

#[derive(Clone, Debug)]
pub enum VizHitAction {
    SelectStyle(VizStyle),
    InspectorReset,
    InspectorTogglePause,
    InspectorToggleAutoReset,
    SwitchQualityTier(i64, bool, i64), // (streaming_quality, lossless_enabled, atmos_setting)
    CycleOutputDevice,
    CycleXpSubPreset,
    ToggleReactorSubStyle,
    ToggleSoundstageColorMode,
}

#[derive(Clone, Debug)]
pub struct VizHitZone {
    pub rect: RectF,
    pub action: VizHitAction,
}

struct OsuParticle {
    angle: f32,
    dist_norm: f32,
    speed: f32,
    size: f32,
    alpha: f32,
}

struct ShockwaveRing {
    radius_extra: f32,
    alpha: f32,
}

#[derive(Clone, Copy)]
struct ReactorRod {
    gx: i32,
    gy: i32,
    dist_norm: f32,
    angle: f32,
    rod_kind: u8, // 0 = fuel, 1 = red manual rod, 2 = yellow short rod, 3 = blue emergency AZ-5 rod
    z: f32,
    vz: f32,
}

#[derive(Clone, Copy)]
struct MatrixPillar {
    gx: i32,
    gy: i32,
    dist_norm: f32,
    angle: f32,
    seed: f32,
    z: f32,
    vz: f32,
}

#[derive(Clone, Copy)]
struct PhosphorDot {
    sx: f32,
    sy: f32,
    band: u8,
    life: f32,
}

/// Default monochrome Soundstage phosphor color (clean electric studio blue)
pub const SOUNDSTAGE_MONO_BLUE: Color = Color::rgb(56, 178, 255);

/// 5 frequency-band Soundstage palette from Bass (Low) to Treble (High)
pub const SOUNDSTAGE_BAND_COLORS: [Color; 5] = [
    Color::rgb(255, 88, 66),  // 0: Sub / Bass (20–160 Hz) — Warm Red-Orange
    Color::rgb(255, 190, 56), // 1: Low-Mid (160–500 Hz) — Amber Gold
    Color::rgb(52, 214, 118), // 2: Mid (500–2000 Hz) — Emerald Green
    Color::rgb(50, 184, 242), // 3: High-Mid (2–6 kHz) — Cyan Blue
    Color::rgb(196, 108, 255),// 4: Treble (6 kHz+) — Violet Magenta
];

pub const SOUNDSTAGE_BAND_LABELS: [&str; 5] = ["低", "中低", "中", "中高", "高"];

#[inline]
pub fn soundstage_band_color(band: u8) -> Color {
    SOUNDSTAGE_BAND_COLORS[(band as usize).min(4)]
}

/// Computes the adaptive spectrogram & FFT display frequency ceiling:
/// - Defaults to `min(24 kHz, nyquist)` so standard 44.1/48 kHz music fills the full chart cleanly.
/// - Automatically expands to `48 kHz` or `96 kHz` (clamped to `nyquist`) when real ultrasonic
///   signal energy (`> -78 dBFS` across adjacent bins) is detected above `24.5 kHz` / `48.5 kHz`.
/// - Resets cleanly back to `24 kHz` when `analyzed_frames < 2` (track switch / Reset).
pub fn compute_adaptive_display_max_hz(
    cur_max_hz: f32,
    nyquist_hz: f32,
    inst_db: &[f32],
    peak_db: &[f32],
    cutoff_hz: f32,
    analyzed_frames: u64,
) -> f32 {
    let nyq = nyquist_hz.max(8_000.0);
    let base = nyq.min(24_000.0);
    if nyq <= 24_800.0 {
        return nyq;
    }
    if analyzed_frames < 2 {
        return base;
    }

    let mut target = cur_max_hz.clamp(base, nyq);
    let has_band_energy = |bins: &[f32], lo_hz: f32, hi_hz: f32, thresh_db: f32| -> bool {
        let n = bins.len();
        if n < 32 || lo_hz >= nyq {
            return false;
        }
        let b0 = (((lo_hz / nyq) * (n as f32)).floor() as usize).clamp(1, n - 1);
        let b1 = (((hi_hz.min(nyq) / nyq) * (n as f32)).ceil() as usize).clamp(b0 + 1, n);
        let mut hits = 0usize;
        for b in b0..b1 {
            if bins[b] >= thresh_db {
                hits += 1;
                if hits >= 2 {
                    return true;
                }
            }
        }
        false
    };

    let ultra_48 = cutoff_hz > 48_500.0
        || has_band_energy(peak_db, 48_500.0, nyq, -78.0)
        || has_band_energy(inst_db, 48_500.0, nyq, -74.0);
    let ultra_24 = cutoff_hz > 24_500.0
        || has_band_energy(peak_db, 24_500.0, 48_500.0, -78.0)
        || has_band_energy(inst_db, 24_500.0, 48_500.0, -74.0);

    if ultra_48 {
        target = target.max(nyq.min(96_000.0));
    } else if ultra_24 {
        target = target.max(nyq.min(48_000.0));
    }
    target.clamp(base, nyq)
}

pub struct VisualizerState {
    pub style: VizStyle,
    /// `0` = Default 3D Luminescent Cuboid Matrix Core (`media_1794910079586_825fccbd.jpg`),
    /// `1` = Classic RBMK-1000 Cherenkov Control Rods (click reactor viewport to toggle!)
    pub reactor_sub_style: u8,
    pub xp_sub_preset: u8, // 0 = WMP Fire, 1 = WMP Emerald, 2 = WMP Alchemy
    pub time_sec: f32,
    pub last_dt_sec: f32,
    osu_rot: f32,
    emblem_scale: f32,
    emblem_vel: f32,
    particles: Vec<OsuParticle>,
    ripples: Vec<ShockwaveRing>,
    rods: Vec<ReactorRod>,
    matrix_pillars: Vec<MatrixPillar>,
    soundstage_phosphor: VecDeque<PhosphorDot>,
    /// `false` (default) = clean studio blue monochrome dots; `true` = 5 frequency-band colors.
    /// Click anywhere on the Soundstage panel to toggle!
    pub soundstage_multicolor: bool,
    soundstage_gain: f32,
    soundstage_rms_ema: f32,
    adaptive_max_hz: f32,
    smooth_lvl_l: f32,
    smooth_lvl_r: f32,
    reactor_agc: f32,
    broadband_ema: f32,
    bass_ema: f32,
    mid_ema: f32,
    high_ema: f32,
    shockwave_radius: f32,
    shockwave_strength: f32,
    matrix_kick_cd: f32,
    flux_cooldown_sec: f32,
    onset_counter: u32,
    xp_peaks: [f32; 48],
    xp_peak_vel: [f32; 48],
    /// Cached semicircle reference arcs for AudioInspector Soundstage (`cx, cy, rad` bits)
    soundstage_arcs_cache: (u32, u32, u32, [Vec<(f32, f32)>; 3]),
    /// Cached RBMK "Elena" steel biological shield rim polyline (`cx, cy, rx, ry` bits)
    elena_ring_cache: (u32, u32, u32, u32, Vec<(f32, f32)>),
    /// Reusable polyline scratch buffers to eliminate per-frame heap allocations in FFT / Scope / Silk
    scratch_pts_a: Vec<(f32, f32)>,
    scratch_pts_b: Vec<(f32, f32)>,
    /// Telemetry: number of 3D Matrix Core cuboids actually drawn in the latest frame
    pub last_matrix_drawn_count: usize,
    pub hit_zones: Vec<VizHitZone>,
}

impl Default for VisualizerState {
    fn default() -> Self {
        let mut particles = Vec::with_capacity(56);
        for i in 0..56 {
            let seed = (i as f32) * 1.618_034;
            particles.push(OsuParticle {
                angle: (seed * 2.0 * PI) % (2.0 * PI),
                dist_norm: ((i as f32) / 56.0).fract(),
                speed: 0.14 + ((i * 7 % 13) as f32) * 0.018,
                size: 1.4 + ((i * 3 % 5) as f32) * 0.55,
                alpha: 0.25 + ((i * 5 % 7) as f32) * 0.08,
            });
        }

        // Build denser RBMK-1000 circular 15x15 biological shield core map (~177 channels)
        let mut rods = Vec::with_capacity(185);
        for gy in -7i32..=7 {
            for gx in -7i32..=7 {
                let r2 = (gx * gx + gy * gy) as f32;
                if r2 <= 53.5 {
                    let dist_norm = (r2.sqrt() / 7.32).clamp(0.0, 1.0);
                    let angle = (gy as f32).atan2(gx as f32);
                    let rod_kind = if (gx + gy * 2).rem_euclid(7) == 0 {
                        1 // Red control rod
                    } else if (gx * 3 - gy).rem_euclid(11) == 0 {
                        2 // Yellow control rod
                    } else if gx == 0 && gy == 0 || (gx.abs() == 3 && gy.abs() == 3) {
                        3 // Blue AZ-5 rod
                    } else {
                        0 // Standard steel fuel cap
                    };
                    rods.push(ReactorRod {
                        gx,
                        gy,
                        dist_norm,
                        angle,
                        rod_kind,
                        z: 0.0,
                        vz: 0.0,
                    });
                }
            }
        }
        // Painter's algorithm sort for isometric projection: back-to-front by (gx + gy)
        rods.sort_by_key(|r| r.gx + r.gy);

        // Build high-density 37x37 (1,369 cuboids!) isometric luminescent matrix core
        let mut matrix_pillars = Vec::with_capacity(37 * 37);
        for gy in -18i32..=18 {
            for gx in -18i32..=18 {
                let r = ((gx * gx + gy * gy) as f32).sqrt();
                let dist_norm = (r / 18.8).clamp(0.0, 1.38);
                let angle = (gy as f32).atan2(gx as f32);
                let hash = ((gx * 73856093) ^ (gy * 19349663)).unsigned_abs();
                let seed = ((hash % 1000) as f32) / 1000.0;
                matrix_pillars.push(MatrixPillar {
                    gx,
                    gy,
                    dist_norm,
                    angle,
                    seed,
                    z: 0.0,
                    vz: 0.0,
                });
            }
        }
        matrix_pillars.sort_by_key(|p| p.gx + p.gy);

        Self {
            style: VizStyle::AudioInspector,
            reactor_sub_style: 0, // Default = 3D Luminescent Cuboid Matrix Core per user request!
            xp_sub_preset: 0,
            time_sec: 0.0,
            last_dt_sec: 1.0 / 60.0,
            osu_rot: 0.0,
            emblem_scale: 1.0,
            emblem_vel: 0.0,
            particles,
            ripples: Vec::with_capacity(8),
            rods,
            matrix_pillars,
            soundstage_phosphor: VecDeque::with_capacity(448),
            soundstage_multicolor: false, // Default = Blue monochrome; click soundstage to toggle 5-color
            soundstage_gain: 1.85,
            soundstage_rms_ema: 0.18,
            adaptive_max_hz: 24_000.0,
            smooth_lvl_l: 0.0,
            smooth_lvl_r: 0.0,
            reactor_agc: 0.24,
            broadband_ema: 0.0,
            bass_ema: 0.0,
            mid_ema: 0.0,
            high_ema: 0.0,
            shockwave_radius: 2.0,
            shockwave_strength: 0.0,
            matrix_kick_cd: 0.0,
            flux_cooldown_sec: 0.0,
            onset_counter: 0,
            xp_peaks: [0.0; 48],
            xp_peak_vel: [0.0; 48],
            soundstage_arcs_cache: (0, 0, 0, [Vec::new(), Vec::new(), Vec::new()]),
            elena_ring_cache: (0, 0, 0, 0, Vec::new()),
            scratch_pts_a: Vec::with_capacity(256),
            scratch_pts_b: Vec::with_capacity(256),
            last_matrix_drawn_count: 0,
            hit_zones: Vec::with_capacity(16),
        }
    }
}

pub const INSPECTOR_HEADER_FONT_SIZE: f32 = 11.2;

pub fn inspector_detected_header_avail_w(det_card_w: f32) -> f32 {
    (det_card_w - 146.0 - 14.0 - 6.0).max(56.0)
}

pub fn format_inspector_detected_header(
    active_signal: bool,
    confidence_pct: u8,
    avail_width_dip: f32,
) -> String {
    if !active_signal {
        return "静音待命".to_string();
    }
    if avail_width_dip >= 120.0 {
        format!("实测 · 置信度 {confidence_pct}%")
    } else {
        format!("实测 · {confidence_pct}%")
    }
}

/// Formats the restrained 3-line Apple-style copy for the "应有音质" card:
/// Line 1: `Playing` / `Paused` / `Idle`
/// Line 2: `高解析度无损` / `无损` / `杜比全景声` / `高质量 AAC`
/// Line 3: `24-bit / 192 kHz · ALAC  @5440 kbps`
pub fn format_expected_quality_lines(
    playback_status: &str,
    expected_specs: Option<&StreamAudioSpecs>,
    expected_badge: Option<&str>,
    prefs: &AudioPrefs,
    snap: &LiveVisualizerSnapshot,
) -> (String, String, String) {
    let status_line = match playback_status {
        "Playing" => "Playing",
        "Paused" => "Paused",
        _ => {
            if snap.active_signal {
                "Playing"
            } else {
                "Idle"
            }
        }
    }
    .to_string();

    if let Some(sp) = expected_specs {
        let is_alac = sp.codec.eq_ignore_ascii_case("ALAC");
        let is_atmos = sp.codec.contains("EC-3")
            || sp.codec.contains("AC-4")
            || sp.tier_label.contains("全景声");
        let is_hires_eff = is_alac
            && (sp.sample_rate_hz > 48_000
                || (sp.hires_catalog_trait
                    && prefs.lossless_enabled
                    && prefs.streaming_audio_quality >= 20));

        let tier_line = if is_atmos {
            "杜比全景声"
        } else if is_hires_eff {
            "高解析度无损"
        } else if is_alac {
            "无损"
        } else if sp.codec.contains("40.5") {
            "高效 AAC"
        } else {
            "高质量 AAC"
        }
        .to_string();

        let codec_short = if is_alac {
            "ALAC"
        } else if is_atmos {
            "Atmos"
        } else if sp.codec.contains("40.5") {
            "HE-AAC"
        } else {
            "AAC"
        };

        let eff_sr_hz = if sp.hires_catalog_trait
            && sp.sample_rate_hz <= 48_000
            && prefs.lossless_enabled
            && prefs.streaming_audio_quality >= 20
            && is_alac
            && snap.cutoff_hz > 25_000.0
            && snap.sample_rate > 48_000
        {
            snap.sample_rate.min(192_000)
        } else {
            sp.sample_rate_hz
        };

        let sr_khz = (eff_sr_hz as f32) / 1000.0;
        let sr_str = if sp.hires_catalog_trait
            && eff_sr_hz <= 48_000
            && prefs.lossless_enabled
            && prefs.streaming_audio_quality >= 20
            && is_alac
        {
            "96–192 kHz".to_string()
        } else if (sr_khz - sr_khz.round()).abs() < 0.05 {
            format!("{:.0} kHz", sr_khz)
        } else {
            format!("{:.1} kHz", sr_khz)
        };

        let bd_prefix = sp
            .bit_depth
            .map(|b| format!("{b}-bit / "))
            .unwrap_or_default();

        let eff_kbps = if eff_sr_hz > sp.sample_rate_hz && is_alac {
            let bd = sp.bit_depth.unwrap_or(24) as u32;
            ((eff_sr_hz * bd * 2) / 1000) * 58 / 100
        } else {
            sp.bitrate_kbps
        };

        let spec_line = if eff_kbps > 0 {
            format!("{bd_prefix}{sr_str} · {codec_short}  @{eff_kbps} kbps")
        } else {
            format!("{bd_prefix}{sr_str} · {codec_short}")
        };

        (status_line, tier_line, spec_line)
    } else {
        let (tier_line, fallback_spec) = if prefs.dolby_atmos_setting == 20 {
            ("杜比全景声", "48 kHz · Atmos")
        } else if !prefs.lossless_enabled {
            ("高质量 AAC", "44.1 kHz · AAC  @256 kbps")
        } else if prefs.streaming_audio_quality >= 20 {
            ("高解析度无损", "24-bit / 192 kHz · ALAC")
        } else {
            ("无损", "24-bit / 48 kHz · ALAC")
        };
        let spec_line = expected_badge
            .filter(|b| !b.trim().is_empty())
            .map(|b| format!("{b} · {fallback_spec}"))
            .unwrap_or_else(|| fallback_spec.to_string());
        (status_line, tier_line.to_string(), spec_line)
    }
}

impl VisualizerState {
    /// Returns `(mean_lift_px, max_lift_px)` across all 3D Luminescent Matrix Core pillars.
    pub fn matrix_pillar_stats(&self) -> (f32, f32) {
        if self.matrix_pillars.is_empty() {
            return (0.0, 0.0);
        }
        let mut sum = 0.0f32;
        let mut max_z = 0.0f32;
        for p in &self.matrix_pillars {
            sum += p.z;
            if p.z > max_z {
                max_z = p.z;
            }
        }
        (sum / (self.matrix_pillars.len() as f32), max_z)
    }

    /// Returns total count of 3D Matrix Core cuboids (`37x37 = 1,369`).
    pub fn matrix_pillar_count(&self) -> usize {
        self.matrix_pillars.len()
    }

    /// Returns the number of active 3D Matrix Core cuboids selected for rendering (`<= 361` via
    /// crisp 2x2 lattice stride + outer-corner culling, cutting D2D calls by >75%).
    pub fn matrix_render_active_count(&self) -> usize {
        self.matrix_pillars
            .iter()
            .filter(|p| (p.gx & 1) == 0 && (p.gy & 1) == 0 && !(p.dist_norm > 1.12 && p.z < 0.8))
            .count()
    }

    /// Returns `(scatter_history_count, smooth_lvl_l, smooth_lvl_r)` for Soundstage verification.
    pub fn soundstage_stats(&self) -> (usize, f32, f32) {
        (
            self.soundstage_phosphor.len(),
            self.smooth_lvl_l,
            self.smooth_lvl_r,
        )
    }

    /// Exposes `(sx, sy, life)` of current phosphor persistence dots for unit testing coordinate lock.
    pub fn phosphor_dots_snapshot(&self) -> Vec<(f32, f32, f32)> {
        self.soundstage_phosphor
            .iter()
            .map(|d| (d.sx, d.sy, d.life))
            .collect()
    }

    pub fn adaptive_display_max_hz(&self) -> f32 {
        self.adaptive_max_hz
    }

    pub fn soundstage_gain_value(&self) -> f32 {
        self.soundstage_gain
    }

    pub fn soundstage_band_counts(&self) -> [usize; 5] {
        let mut c = [0usize; 5];
        for d in &self.soundstage_phosphor {
            c[(d.band as usize).min(4)] += 1;
        }
        c
    }

    /// Returns `(mean_lift_px, max_lift_px, std_dev_lift_px)` across all RBMK reactor rods.
    pub fn rod_lift_stats(&self) -> (f32, f32, f32) {
        if self.rods.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let n = self.rods.len() as f32;
        let mut sum = 0.0f32;
        let mut max_z = 0.0f32;
        for r in &self.rods {
            sum += r.z;
            if r.z > max_z {
                max_z = r.z;
            }
        }
        let mean = sum / n;
        let var = self
            .rods
            .iter()
            .map(|r| {
                let d = r.z - mean;
                d * d
            })
            .sum::<f32>()
            / n;
        (mean, max_z, var.sqrt())
    }

    pub fn has_settling_motion(&self) -> bool {
        match self.style {
            VizStyle::AudioInspector => !self.soundstage_phosphor.is_empty(),
            VizStyle::ChernobylRods => {
                if self.reactor_sub_style == 0 {
                    self.matrix_pillars
                        .iter()
                        .any(|p| p.z > 0.05 || p.vz.abs() > 0.25)
                } else {
                    self.rods.iter().any(|r| r.z > 0.05 || r.vz.abs() > 0.25)
                }
            }
            VizStyle::WinXpRetro => self.xp_peaks.iter().any(|&p| p > 0.01),
            _ => false,
        }
    }

    /// Steps ALL visualizer subsystems (used by unit tests verifying multiple styles on a single `VisualizerState`).
    /// Production UI rendering calls `step_active()` to step ONLY the currently visible `self.style` & sub-style!
    pub fn step(&mut self, snap: &LiveVisualizerSnapshot, dt_sec: f32, reduce_motion: bool) {
        self.step_impl(snap, dt_sec, reduce_motion, true);
    }

    /// Production hot-path stepper: steps ONLY the active `self.style` (and active Reactor sub-style),
    /// skipping 1,369 matrix pillars + 193 control rods + soundstage dots when another style is shown!
    pub fn step_active(&mut self, snap: &LiveVisualizerSnapshot, dt_sec: f32, reduce_motion: bool) {
        crate::audio_viz::set_active_viz_style_inspector(self.style == VizStyle::AudioInspector);
        self.step_impl(snap, dt_sec, reduce_motion, false);
    }

    fn step_impl(
        &mut self,
        snap: &LiveVisualizerSnapshot,
        dt_sec: f32,
        reduce_motion: bool,
        step_all_styles: bool,
    ) {
        let dt = dt_sec.clamp(0.001, 0.05);
        self.last_dt_sec = dt;
        if snap.paused {
            return;
        }
        self.time_sec = (self.time_sec + dt) % 10_000.0;

        let peak_db = snap.peak_db_l.max(snap.peak_db_r);
        let spec_peak = snap.spectrum_64[4..58]
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        let is_audible = snap.active_signal
            && (peak_db > -64.0
                || snap.rms_db > -68.0
                || spec_peak > 0.02
                || snap.mid_energy > 0.015
                || snap.bass_energy > 0.015);

        let run_inspector = step_all_styles || self.style == VizStyle::AudioInspector;
        let run_osu = step_all_styles || self.style == VizStyle::OsuCircle;
        let run_reactor = step_all_styles || self.style == VizStyle::ChernobylRods;
        let run_matrix = step_all_styles || (run_reactor && self.reactor_sub_style == 0);
        let run_rods = step_all_styles || (run_reactor && self.reactor_sub_style != 0);
        let run_xp = step_all_styles || self.style == VizStyle::WinXpRetro;

        if run_inspector {
            // Update adaptive spectrogram / FFT frequency ceiling (`24 kHz` default -> `48 kHz` / `96 kHz` on ultrasonic content)
            self.adaptive_max_hz = compute_adaptive_display_max_hz(
                self.adaptive_max_hz,
                snap.nyquist_hz,
                &snap.inst_spectrum_db,
                &snap.peak_envelope_db,
                snap.cutoff_hz,
                snap.analyzed_frames,
            );

            // 0. True Fixed-Coordinate CRT Phosphor Persistence (`声场图`)
            let phos_decay = (-2.25 * dt).exp();
            for dot in &mut self.soundstage_phosphor {
                dot.life *= phos_decay;
            }
            self.soundstage_phosphor.retain(|d| d.life > 0.018);

            if is_audible && !snap.soundstage_lr.is_empty() {
                let mut sum_l2 = 0.0f32;
                let mut sum_r2 = 0.0f32;
                for &[l, rr] in &snap.soundstage_lr {
                    sum_l2 += l * l;
                    sum_r2 += rr * rr;
                }
                let n_inv = 1.0 / (snap.soundstage_lr.len() as f32);
                let rms_l = (sum_l2 * n_inv).sqrt();
                let rms_r = (sum_r2 * n_inv).sqrt();
                let rms_avg = ((sum_l2 + sum_r2) * 0.5 * n_inv).sqrt();
                let rms_l_db = 20.0 * rms_l.max(1.58e-5).log10();
                let rms_r_db = 20.0 * rms_r.max(1.58e-5).log10();
                let eff_l_db = rms_l_db.max(snap.peak_db_l - 6.0);
                let eff_r_db = rms_r_db.max(snap.peak_db_r - 6.0);
                let tgt_l = ((eff_l_db + 58.0) / 56.0).clamp(0.0, 1.0);
                let tgt_r = ((eff_r_db + 58.0) / 56.0).clamp(0.0, 1.0);
                let k_bar = (1.0 - (-18.0 * dt).exp()).clamp(0.05, 1.0);
                self.smooth_lvl_l += (tgt_l - self.smooth_lvl_l) * k_bar;
                self.smooth_lvl_r += (tgt_r - self.smooth_lvl_r) * k_bar;

                let rms_k = if rms_avg > self.soundstage_rms_ema {
                    (1.0 - (-10.0 * dt).exp()).clamp(0.04, 1.0)
                } else {
                    (1.0 - (-2.6 * dt).exp()).clamp(0.02, 1.0)
                };
                self.soundstage_rms_ema += (rms_avg.max(0.0025) - self.soundstage_rms_ema) * rms_k;
                let target_gain = (0.42 / self.soundstage_rms_ema.max(0.0035)).clamp(0.65, 24.0);
                let gain_rate = if target_gain < self.soundstage_gain {
                    (9.0 * dt).min(1.0)
                } else {
                    (3.0 * dt).min(1.0)
                };
                self.soundstage_gain += (target_gain - self.soundstage_gain) * gain_rate;
                let gain = self.soundstage_gain;

                let fallback_bands;
                let band_pts: &[[f32; 3]] = if !snap.soundstage_band_pts.is_empty() {
                    &snap.soundstage_band_pts
                } else {
                    fallback_bands = crate::audio_viz::split_stereo_5band_soundstage(
                        &snap.soundstage_lr,
                        snap.sample_rate,
                    );
                    &fallback_bands
                };
                // Use a stride coprime to 5 (1, 2, or 3) so all 5 interleaved frequency bands are
                // sampled evenly while keeping phosphor dot density bounded at <= 420 dots.
                let stride = if band_pts.len() >= 220 {
                    3
                } else if band_pts.len() >= 110 {
                    2
                } else {
                    1
                };
                for &[l, rr, bf] in band_pts.iter().step_by(stride) {
                    let (sx, sy) = map_lr_to_semicircle(l, rr, gain);
                    if sx.abs() + sy.abs() < 0.012 {
                        continue;
                    }
                    if self.soundstage_phosphor.len() >= 420 {
                        self.soundstage_phosphor.pop_front();
                    }
                    self.soundstage_phosphor.push_back(PhosphorDot {
                        sx,
                        sy,
                        band: (bf.round() as u8).min(4),
                        life: 1.0,
                    });
                }
            } else {
                let decay = (-10.0 * dt).exp();
                self.smooth_lvl_l *= decay;
                self.smooth_lvl_r *= decay;
                if self.smooth_lvl_l < 0.004 {
                    self.smooth_lvl_l = 0.0;
                }
                if self.smooth_lvl_r < 0.004 {
                    self.smooth_lvl_r = 0.0;
                }
            }
        }

        if run_osu {
            if reduce_motion {
                self.emblem_scale = 1.0 + snap.bass_energy * 0.03;
            } else {
                let bpm_norm = ((snap.estimated_bpm - 70.0) / 110.0).clamp(0.0, 1.2);
                self.osu_rot = (self.osu_rot
                    + dt * (0.11 + snap.bass_energy * 0.22 + bpm_norm * 0.06))
                    % (2.0 * PI);
                if snap.beat_triggered && is_audible {
                    self.emblem_vel += 2.65 + snap.bass_energy * 1.45;
                    if self.ripples.len() < 7 {
                        self.ripples.push(ShockwaveRing {
                            radius_extra: 0.0,
                            alpha: 0.64,
                        });
                    }
                }
                let pll_groove = if is_audible {
                    (-snap.beat_phase.clamp(0.0, 1.0) * 5.4).exp() * 0.055
                } else {
                    0.0
                };
                let target_scale = 1.0 + pll_groove + snap.bass_energy * 0.035;
                let force = -205.0 * (self.emblem_scale - target_scale) - 19.0 * self.emblem_vel;
                self.emblem_vel += force * dt;
                self.emblem_scale = (self.emblem_scale + self.emblem_vel * dt).clamp(0.94, 1.28);

                for rip in &mut self.ripples {
                    rip.radius_extra += (155.0 + snap.bass_energy * 165.0) * dt;
                    rip.alpha -= 0.78 * dt;
                }
                self.ripples.retain(|r| r.alpha > 0.01);

                let energy_boost = 1.0 + snap.bass_energy * 2.6 + snap.beat_pulse * 1.9;
                for p in &mut self.particles {
                    p.dist_norm += p.speed * energy_boost * dt;
                    if p.dist_norm >= 1.0 {
                        p.dist_norm -= 1.0;
                    }
                }
            }
        }

        if run_reactor {
            if reduce_motion {
                if run_matrix {
                    for p in &mut self.matrix_pillars {
                        p.vz = 0.0;
                        p.z = if is_audible {
                            let dome = (-2.6 * p.dist_norm * p.dist_norm).exp();
                            let b = ((p.dist_norm * 46.0) as usize).min(60);
                            ((snap.spectrum_64[b] * 0.85 + snap.bass_energy * dome) * 52.0)
                                .clamp(0.0, 56.0)
                        } else {
                            0.0
                        };
                    }
                }
                if run_rods {
                    for rod in &mut self.rods {
                        rod.vz = 0.0;
                        rod.z = if is_audible {
                            let b = (11 + ((rod.dist_norm * 32.0) as usize)).min(58);
                            (snap.spectrum_64[b] * 34.0).clamp(0.0, 38.0)
                        } else {
                            0.0
                        };
                    }
                }
            } else if is_audible {
                let target_agc = spec_peak.clamp(0.14, 0.88);
                let agc_rate = if target_agc > self.reactor_agc {
                    (8.5 * dt).min(1.0)
                } else {
                    (1.4 * dt).min(1.0)
                };
                self.reactor_agc += (target_agc - self.reactor_agc) * agc_rate;

                let effective_db = if snap.rms_db > -90.0 {
                    snap.rms_db.max(peak_db - 5.5)
                } else {
                    -56.0 + spec_peak * 48.0
                };
                let loud_scale = ((effective_db + 56.0) / 46.0).clamp(0.32, 1.02);
                let agc_inv = (1.0 / self.reactor_agc.max(0.14)).clamp(0.95, 3.8);

                let bass_flux = (snap.bass_energy - self.bass_ema).max(0.0);
                let mid_flux = (snap.mid_energy - self.mid_ema).max(0.0);
                let high_flux = (snap.high_energy - self.high_ema).max(0.0);
                let band_ema_k = (1.0 - (-5.2 * dt).exp()).clamp(0.01, 1.0);
                self.bass_ema += (snap.bass_energy - self.bass_ema) * band_ema_k;
                self.mid_ema += (snap.mid_energy - self.mid_ema) * band_ema_k;
                self.high_ema += (snap.high_energy - self.high_ema) * band_ema_k;

                let broadband_now =
                    snap.bass_energy * 0.90 + snap.mid_energy * 1.15 + snap.high_energy * 0.78;
                let bb_rise = (broadband_now - self.broadband_ema).max(0.0);
                self.broadband_ema += (broadband_now - self.broadband_ema) * band_ema_k;
                self.flux_cooldown_sec = (self.flux_cooldown_sec - dt).max(0.0);
                self.matrix_kick_cd = (self.matrix_kick_cd - dt).max(0.0);

                let flux_onset = (bb_rise > 0.058 || bass_flux > 0.068 || mid_flux > 0.072)
                    && self.flux_cooldown_sec <= 0.0;
                let transient_hit = snap.beat_triggered || flux_onset;
                if transient_hit {
                    self.onset_counter = self.onset_counter.wrapping_add(1);
                    if !snap.beat_triggered {
                        self.flux_cooldown_sec = 0.125;
                    }
                }

                if run_rods {
                    let onset_group = (self.onset_counter as usize) % 3;
                    let wave_phase = self.time_sec * 6.8;
                    for (idx, rod) in self.rods.iter_mut().enumerate() {
                        let azim_slot =
                            (((rod.angle + PI) / (2.0 * PI)) * 14.0).floor() as usize % 14;
                        let musical_b =
                            (10 + ((rod.dist_norm * 30.0) as usize) + azim_slot).min(58);
                        let upper_b = (musical_b + 5).min(62);
                        let raw_band = snap.spectrum_64[musical_b]
                            .max(snap.spectrum_64[upper_b] * 0.88)
                            .max(snap.mid_energy * 0.72 + snap.high_energy * 0.42 * rod.dist_norm);
                        let bass_punch =
                            (1.0 - rod.dist_norm * 0.70).max(0.0) * snap.bass_energy * 0.88;
                        let flux_boost = bass_flux * 0.85 + mid_flux * 0.92 + high_flux * 0.70;
                        let norm_exc = ((raw_band + bass_punch + flux_boost) * agc_inv)
                            .clamp(0.0, 1.25)
                            * loud_scale;

                        let radial_wave =
                            (rod.dist_norm * 7.2 - wave_phase + rod.angle * 2.0).sin() * 0.5 + 0.5;
                        let phase_i = (idx as f32) * 2.399_963;
                        let chatter_freq = 10.6 + ((idx % 5) as f32) * 1.40;
                        let chatter_osc =
                            (self.time_sec * chatter_freq + phase_i).sin() * 0.5 + 0.5;
                        let kind_gain = if rod.rod_kind != 0 { 1.24 } else { 0.98 };
                        let core_surge = (1.0 - rod.dist_norm * 0.62).max(0.0)
                            * snap.beat_pulse
                            * 0.48
                            * loud_scale;

                        let drive = norm_exc
                            * (0.40 + 0.34 * radial_wave + 0.38 * chatter_osc)
                            * kind_gain
                            + core_surge;
                        let raw_z = drive * 38.0;
                        let u_z = (raw_z / 54.0).max(0.0);
                        let target_z = (54.0 * (u_z / (1.0 + 0.45 * u_z))).clamp(0.0, 56.0);

                        if snap.beat_triggered && rod.dist_norm < 0.68 {
                            rod.vz += (1.0 - rod.dist_norm * 0.72) * 130.0 * loud_scale;
                        } else if transient_hit && (idx % 3 == onset_group) {
                            let flux_kick =
                                (bb_rise * agc_inv * 115.0).clamp(24.0, 98.0) * loud_scale;
                            rod.vz += flux_kick * kind_gain;
                        }

                        let gravity = if rod.z > target_z { -102.0 } else { 0.0 };
                        let accel = (target_z - rod.z) * 160.0 - rod.vz * 13.8 + gravity;
                        rod.vz += accel * dt;
                        rod.z = (rod.z + rod.vz * dt).min(58.0);
                        if rod.z < 0.0 {
                            rod.z = 0.0;
                            if rod.vz < -12.0 {
                                rod.vz = -rod.vz * 0.24;
                            } else {
                                rod.vz = 0.0;
                            }
                        }
                    }
                }

                if run_matrix {
                    if (snap.beat_triggered || bass_flux > 0.078 || mid_flux > 0.088)
                        && self.matrix_kick_cd <= 0.0
                    {
                        self.shockwave_radius = 0.04;
                        self.shockwave_strength = (0.42
                            + snap.bass_energy * 0.45
                            + bass_flux * 1.8
                            + mid_flux * 1.4)
                            .clamp(0.32, 0.98);
                        self.matrix_kick_cd = 0.135;
                    } else {
                        self.shockwave_radius += (2.15 + snap.bass_energy * 0.95) * dt;
                        self.shockwave_strength *= (-3.4 * dt).exp();
                    }

                    let radial_phase = self.time_sec * 4.8;
                    for p in &mut self.matrix_pillars {
                        let d = p.dist_norm;
                        let core_dome = (-2.65 * d * d).exp();
                        let mid_ring = (-((d - 0.52) * 3.1).powi(2)).exp();
                        let outer_skirt =
                            ((d - 0.55) * 1.4).clamp(0.0, 1.0) * (1.38 - d).clamp(0.0, 1.0);

                        let azim = (((p.angle + PI) / (2.0 * PI)) * 20.0).floor() as usize % 20;
                        let spec_bin = ((d * 38.0) as usize + azim).min(61);
                        let hi_bin = (36 + ((p.seed * 24.0) as usize)).min(62);
                        let band_e = (snap.spectrum_64[spec_bin] * agc_inv).clamp(0.0, 1.25);
                        let hi_e = (snap.spectrum_64[hi_bin] * agc_inv).clamp(0.0, 1.15);

                        let ripple = (d * 10.5 - radial_phase + p.seed * 4.2 + p.angle * 2.0).sin()
                            * 0.5
                            + 0.5;
                        let micro =
                            (self.time_sec * (5.6 + p.seed * 4.8) + p.seed * 23.0).sin() * 0.5
                                + 0.5;

                        let sw_dist = d - self.shockwave_radius;
                        let shockwave_lift =
                            (-sw_dist * sw_dist * 38.0).exp() * self.shockwave_strength * 18.0;

                        let core_drive = core_dome
                            * (0.20
                                + snap.bass_energy * 1.05
                                + snap.mid_energy * 0.76
                                + snap.beat_pulse * 0.62
                                + bass_flux * 1.6)
                            * (0.62 + 0.36 * micro + 0.30 * p.seed);
                        let mid_drive = (mid_ring * 0.80 + (1.0 - (d * 0.65).min(0.92)) * 0.48)
                            * (band_e * 0.92 + mid_flux * 1.5)
                            * (0.34 + 0.42 * ripple + 0.24 * micro);
                        let high_drive = outer_skirt
                            * (hi_e * 0.78 + snap.high_energy * 0.98 + high_flux * 2.0)
                            * (0.35 + 0.58 * micro);

                        let raw_z = (core_drive * 40.0
                            + mid_drive * 26.0
                            + high_drive * 19.0
                            + shockwave_lift)
                            * loud_scale;
                        let u_z = (raw_z / 76.0).max(0.0);
                        let target_z = (76.0 * (u_z / (1.0 + 0.45 * u_z))).clamp(0.0, 80.0);

                        if snap.beat_triggered && d < 0.65 {
                            p.vz += core_dome * (92.0 + p.seed * 52.0) * loud_scale;
                        } else if flux_onset && p.seed > 0.58 {
                            p.vz +=
                                (mid_flux + high_flux) * 125.0 * (0.4 + 0.6 * p.seed) * loud_scale;
                        }

                        let accel = (target_z - p.z) * 175.0 - p.vz * 13.8;
                        p.vz += accel * dt;
                        p.z = (p.z + p.vz * dt).clamp(0.0, 88.0);
                    }
                }
            } else {
                self.broadband_ema = 0.0;
                self.bass_ema = 0.0;
                self.mid_ema = 0.0;
                self.high_ema = 0.0;
                self.shockwave_strength = 0.0;
                self.matrix_kick_cd = 0.0;
                self.flux_cooldown_sec = 0.0;
                if run_rods {
                    for rod in &mut self.rods {
                        if rod.z > 0.0 || rod.vz != 0.0 {
                            let accel = -rod.z * 155.0 - rod.vz * 16.5 - 105.0;
                            rod.vz += accel * dt;
                            rod.z += rod.vz * dt;
                            if rod.z <= 0.02 && rod.vz.abs() < 6.0 {
                                rod.z = 0.0;
                                rod.vz = 0.0;
                            } else if rod.z < 0.0 {
                                rod.z = 0.0;
                                rod.vz = -rod.vz * 0.16;
                            }
                        }
                    }
                }
                if run_matrix {
                    for p in &mut self.matrix_pillars {
                        if p.z > 0.0 || p.vz != 0.0 {
                            let accel = -p.z * 165.0 - p.vz * 22.0 - 95.0;
                            p.vz += accel * dt;
                            p.z += p.vz * dt;
                            if p.z <= 0.02 && p.vz.abs() < 6.0 {
                                p.z = 0.0;
                                p.vz = 0.0;
                            } else if p.z < 0.0 {
                                p.z = 0.0;
                                p.vz = -p.vz * 0.12;
                            }
                        }
                    }
                }
            }
        }

        if run_xp && !reduce_motion {
            for i in 0..48 {
                let src_idx = (i * 64 / 48).min(63);
                let val = if is_audible {
                    snap.spectrum_64[src_idx]
                } else {
                    0.0
                };
                if val >= self.xp_peaks[i] && val > 0.0 {
                    self.xp_peaks[i] = val;
                    self.xp_peak_vel[i] = -0.04;
                } else if self.xp_peaks[i] > 0.0 {
                    let grav = if is_audible { 0.62 } else { 1.35 };
                    self.xp_peak_vel[i] += grav * dt;
                    self.xp_peaks[i] =
                        (self.xp_peaks[i] - self.xp_peak_vel[i].max(0.0) * dt).max(0.0);
                    if self.xp_peaks[i] == 0.0 {
                        self.xp_peak_vel[i] = 0.0;
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        r: &mut D2DRenderer,
        area: RectF,
        snap: &LiveVisualizerSnapshot,
        smtc: &SmtcState,
        expected_specs: Option<&StreamAudioSpecs>,
        expected_badge: Option<&str>,
        prefs: &AudioPrefs,
        devices: &[AudioOutputDevice],
        artwork_key: Option<&str>,
        measured_fps: f32,
        is_animating: bool,
        fps_cap_label: &str,
        show_spatial_pill: bool,
        hovered_pt: (f32, f32),
        mouse_down: bool,
        reduce_transparency: bool,
    ) {
        self.hit_zones.clear();
        crate::audio_viz::set_active_viz_style_inspector(self.style == VizStyle::AudioInspector);

        let pad = 22.0;
        let top_y = area.y + 56.0;

        // Header Row: Title "可视化" + Liquid Glass Segmented Style Switcher + Live FPS Telemetry Badge
        r.draw_text(
            "可视化",
            RectF::new(area.x + pad, top_y, 120.0, 34.0),
            24.0,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );

        // Liquid Glass Container (`GlassEffectContainer`) for style switcher
        let seg_w = 420.0f32.min(area.w - 280.0).max(280.0);
        let seg_x = (area.center_x() - seg_w * 0.5).max(area.x + 128.0);
        let seg_rect = RectF::new(seg_x, top_y, seg_w, 34.0);
        r.draw_liquid_glass_button(
            seg_rect,
            17.0,
            GlassBtnVariant::Subtle,
            false,
            false,
            hovered_pt,
            reduce_transparency,
        );

        let item_w = (seg_w - 8.0) / (VizStyle::ALL.len() as f32);
        for (idx, &st) in VizStyle::ALL.iter().enumerate() {
            let ir = RectF::new(
                seg_rect.x + 4.0 + (idx as f32) * item_w,
                seg_rect.y + 3.0,
                item_w,
                28.0,
            );
            let is_sel = self.style == st;
            let is_hov = ir.contains(hovered_pt.0, hovered_pt.1);
            if is_sel {
                r.draw_liquid_glass_button(
                    ir,
                    14.0,
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED),
                    is_hov,
                    mouse_down,
                    hovered_pt,
                    reduce_transparency,
                );
            } else if is_hov {
                r.draw_liquid_glass_button(
                    ir,
                    14.0,
                    GlassBtnVariant::Regular,
                    true,
                    mouse_down,
                    hovered_pt,
                    reduce_transparency,
                );
            }
            r.draw_text(
                st.label(),
                ir,
                12.5,
                if is_sel { 600 } else { 400 },
                if is_sel || is_hov {
                    Theme::TEXT_PRIMARY
                } else {
                    Theme::TEXT_SECONDARY
                },
                TextAlign::Center,
                true,
                false,
            );
            self.hit_zones.push(VizHitZone {
                rect: ir,
                action: VizHitAction::SelectStyle(st),
            });
        }

        // Right FPS indicator (`charts.md` clarity, genuine measured frame rate vs 0% GPU idle standby)
        let fps_txt = if !is_animating {
            format!("静默待机 · {}", fps_cap_label)
        } else if measured_fps > 1.0 {
            format!("{:.0} Hz · {}", measured_fps, fps_cap_label)
        } else {
            format!("60+ Hz · {}", fps_cap_label)
        };
        r.draw_mono_text(
            &fps_txt,
            RectF::new(area.right() - pad - 185.0, top_y, 185.0, 34.0),
            11.5,
            500,
            Theme::TEXT_MUTED,
            TextAlign::Right,
            true,
        );

        let body = RectF::new(
            area.x + pad,
            top_y + 44.0,
            area.w - pad * 2.0,
            (area.bottom() - (top_y + 44.0) - 86.0).max(220.0),
        );

        match self.style {
            VizStyle::AudioInspector => {
                self.draw_audio_inspector_style(
                    r,
                    body,
                    snap,
                    smtc,
                    expected_specs,
                    expected_badge,
                    prefs,
                    devices,
                    show_spatial_pill,
                    hovered_pt,
                    mouse_down,
                    reduce_transparency,
                );
            }
            VizStyle::OsuCircle => {
                self.draw_osu_circle_style(r, body, snap, artwork_key);
            }
            VizStyle::ChernobylRods => {
                if self.reactor_sub_style == 0 {
                    self.draw_reactor_matrix_core_style(r, body, snap, smtc, artwork_key);
                } else {
                    self.draw_chernobyl_rods_style(r, body, snap);
                }
            }
            VizStyle::WinXpRetro => {
                self.draw_winxp_retro_style(r, body, snap, hovered_pt, mouse_down, reduce_transparency);
            }
            VizStyle::SilkAurora => {
                self.draw_silk_aurora_style(r, body, snap);
            }
        }
    }

    // ========================================================================
    // Style 0: Merged Audio Quality Inspector + Soundstage (ONLY style showing quality details!)
    // ========================================================================
    #[allow(clippy::too_many_arguments)]
    fn draw_audio_inspector_style(
        &mut self,
        r: &mut D2DRenderer,
        body: RectF,
        snap: &LiveVisualizerSnapshot,
        smtc: &SmtcState,
        expected_specs: Option<&StreamAudioSpecs>,
        expected_badge: Option<&str>,
        prefs: &AudioPrefs,
        devices: &[AudioOutputDevice],
        show_spatial_pill: bool,
        hovered_pt: (f32, f32),
        mouse_down: bool,
        reduce_transparency: bool,
    ) {
        let gap = 12.0;
        let top_cards_h = 124.0f32.min(body.h * 0.29).max(106.0);
        let half_w = (body.w - gap) * 0.5;

        // --- Card A: Expected Stream Audio Quality (restrained 3-line hierarchy) ---
        let exp_card = RectF::new(body.x, body.y, half_w, top_cards_h);
        r.fill_rounded_rect(exp_card, 12.0, Theme::BG_ELEVATED);
        r.stroke_rounded_rect(exp_card, 12.0, Theme::SEPARATOR, 1.0);

        let pb_status = if smtc.active {
            smtc.status.as_str()
        } else {
            ""
        };
        let (exp_status, exp_tier, exp_spec) = format_expected_quality_lines(
            pb_status,
            expected_specs,
            expected_badge,
            prefs,
            snap,
        );

        let status_col = if exp_status == "Playing" {
            Theme::SIGNAL_GREEN
        } else {
            Theme::TEXT_MUTED
        };
        r.draw_text(
            &exp_status,
            RectF::new(exp_card.x + 14.0, exp_card.y + 10.0, exp_card.w - 28.0, 16.0),
            11.5,
            600,
            status_col,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            &exp_tier,
            RectF::new(exp_card.x + 14.0, exp_card.y + 30.0, exp_card.w - 28.0, 26.0),
            17.5,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_mono_text(
            &exp_spec,
            RectF::new(exp_card.x + 14.0, exp_card.y + 59.0, exp_card.w - 28.0, 18.0),
            12.2,
            500,
            Theme::SIGNAL_CYAN,
            TextAlign::Left,
            true,
        );

        // Quick Tier Pills at bottom of Expected Card (spatial/Atmos hidden by default unless enabled!)
        let tiers: &[(&str, i64, bool, i64)] = if show_spatial_pill || prefs.dolby_atmos_setting == 20 {
            &[
                ("Hi-Res", 20i64, true, 30i64),
                ("无损", 15, true, 30),
                ("AAC", 10, false, 30),
                ("全景声", 20, true, 20),
            ]
        } else {
            &[
                ("Hi-Res", 20i64, true, 30i64),
                ("无损", 15, true, 30),
                ("AAC", 10, false, 30),
            ]
        };
        let mut pill_x = exp_card.x + 14.0;
        let pill_y = exp_card.bottom() - 28.0;
        for &(lbl, q, loss, atm) in tiers {
            let active = if atm == 20 {
                prefs.dolby_atmos_setting == 20
            } else if !loss {
                !prefs.lossless_enabled && prefs.dolby_atmos_setting != 20
            } else {
                prefs.lossless_enabled
                    && prefs.dolby_atmos_setting != 20
                    && prefs.streaming_audio_quality == q
            };
            let pr = RectF::new(pill_x, pill_y, 52.0, 21.0);
            let is_hov = pr.contains(hovered_pt.0, hovered_pt.1);
            r.draw_liquid_glass_button(
                pr,
                10.5,
                if active {
                    GlassBtnVariant::Tinted(Theme::ACCENT_RED)
                } else {
                    GlassBtnVariant::Subtle
                },
                is_hov,
                mouse_down,
                hovered_pt,
                reduce_transparency,
            );
            r.draw_text(
                lbl,
                pr,
                11.0,
                if active { 600 } else { 400 },
                if active || is_hov {
                    Theme::TEXT_PRIMARY
                } else {
                    Theme::TEXT_SECONDARY
                },
                TextAlign::Center,
                true,
                false,
            );
            self.hit_zones.push(VizHitZone {
                rect: pr,
                action: VizHitAction::SwitchQualityTier(q, loss, atm),
            });
            pill_x += 57.0;
        }

        if devices.len() > 1 {
            let dev_btn = RectF::new(exp_card.right() - 80.0, pill_y, 66.0, 21.0);
            if dev_btn.x > pill_x + 4.0 {
                let dev_hov = dev_btn.contains(hovered_pt.0, hovered_pt.1);
                r.draw_liquid_glass_button(
                    dev_btn,
                    10.5,
                    GlassBtnVariant::Subtle,
                    dev_hov,
                    mouse_down,
                    hovered_pt,
                    reduce_transparency,
                );
                r.draw_text(
                    "切换设备",
                    dev_btn,
                    10.8,
                    500,
                    if dev_hov {
                        Theme::TEXT_PRIMARY
                    } else {
                        Theme::TEXT_SECONDARY
                    },
                    TextAlign::Center,
                    true,
                    false,
                );
                self.hit_zones.push(VizHitZone {
                    rect: dev_btn,
                    action: VizHitAction::CycleOutputDevice,
                });
            }
        }

        // --- Card B: Actual Detected System Audio Quality (restrained studio layout) ---
        let det_card = RectF::new(exp_card.right() + gap, body.y, half_w, top_cards_h);
        r.fill_rounded_rect(det_card, 12.0, Theme::BG_ELEVATED);
        let verdict_accent = if snap.tier_code == "hires_lossless" {
            Theme::SIGNAL_GREEN
        } else if snap.tier_code.starts_with("lossless") {
            Theme::SIGNAL_CYAN
        } else if snap.tier_code == "narrowband_analyzing" {
            Theme::SIGNAL_CYAN.lerp(Theme::TEXT_SECONDARY, 0.32)
        } else if snap.tier_code.starts_with("lossy") {
            Theme::SIGNAL_AMBER
        } else {
            Theme::TEXT_MUTED
        };
        r.stroke_rounded_rect(det_card, 12.0, verdict_accent.with_alpha(0.32), 1.0);

        // Compact action buttons: 自动清零 / 冻结 / 重测
        let rst_btn = RectF::new(det_card.right() - 48.0, det_card.y + 8.0, 36.0, 21.0);
        let frz_btn = RectF::new(rst_btn.x - 42.0, det_card.y + 8.0, 38.0, 21.0);
        let auto_btn = RectF::new(frz_btn.x - 56.0, det_card.y + 8.0, 52.0, 21.0);

        let hdr_avail_w = inspector_detected_header_avail_w(det_card.w);
        let status_hdr =
            format_inspector_detected_header(snap.active_signal, snap.confidence_pct, hdr_avail_w);
        r.draw_text(
            &status_hdr,
            RectF::new(det_card.x + 14.0, det_card.y + 10.0, hdr_avail_w, 16.0),
            INSPECTOR_HEADER_FONT_SIZE,
            600,
            verdict_accent,
            TextAlign::Left,
            true,
            false,
        );
        for (br, lbl, active_tint, act) in [
            (
                auto_btn,
                if snap.auto_reset_silence {
                    "自动清零"
                } else {
                    "保持"
                },
                snap.auto_reset_silence,
                VizHitAction::InspectorToggleAutoReset,
            ),
            (
                frz_btn,
                if snap.paused { "恢复" } else { "冻结" },
                snap.paused,
                VizHitAction::InspectorTogglePause,
            ),
            (rst_btn, "重测", false, VizHitAction::InspectorReset),
        ] {
            let bhov = br.contains(hovered_pt.0, hovered_pt.1);
            r.draw_liquid_glass_button(
                br,
                10.5,
                if active_tint {
                    GlassBtnVariant::Tinted(Theme::SIGNAL_CYAN.with_alpha(0.65))
                } else {
                    GlassBtnVariant::Subtle
                },
                bhov,
                mouse_down,
                hovered_pt,
                reduce_transparency,
            );
            r.draw_text(
                lbl,
                br,
                10.6,
                500,
                Theme::TEXT_PRIMARY,
                TextAlign::Center,
                true,
                false,
            );
            self.hit_zones.push(VizHitZone {
                rect: br,
                action: act,
            });
        }

        r.draw_text(
            &snap.verdict_title,
            RectF::new(det_card.x + 14.0, det_card.y + 30.0, det_card.w - 28.0, 26.0),
            17.5,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );

        let cut_str = if snap.cutoff_hz > 1000.0 {
            format!("{:.1} kHz", snap.cutoff_hz / 1000.0)
        } else {
            "--".to_string()
        };
        let det_spec = if snap.active_signal || snap.paused {
            format!(
                "{cut_str} · {} · {}",
                snap.detected_bit_grid, snap.estimated_bitrate
            )
        } else {
            format!("{:.1} kHz / {}-bit", snap.sample_rate as f32 / 1000.0, snap.bit_depth)
        };
        r.draw_mono_text(
            &det_spec,
            RectF::new(det_card.x + 14.0, det_card.y + 59.0, det_card.w - 28.0, 18.0),
            12.2,
            500,
            verdict_accent,
            TextAlign::Left,
            true,
        );

        let det_foot = if snap.active_signal || snap.paused {
            format!(
                "落差 {:.1} dB · 空洞 {:.1}% · {:.1} dBFS",
                snap.brickwall_drop_db, snap.mdct_hole_pct, snap.rms_db
            )
        } else {
            snap.device_name.clone()
        };
        r.draw_mono_text(
            &det_foot,
            RectF::new(det_card.x + 14.0, det_card.bottom() - 26.0, det_card.w - 28.0, 16.0),
            11.0,
            400,
            Theme::TEXT_MUTED,
            TextAlign::Left,
            true,
        );

        // --- 2x2 High-Refresh Instrument Grid ---
        let grid_y = body.y + top_cards_h + gap;
        let grid_h = (body.bottom() - grid_y).max(160.0);
        let cell_h = (grid_h - gap) * 0.5;

        let p_tl = RectF::new(body.x, grid_y, half_w, cell_h);
        let p_tr = RectF::new(body.x + half_w + gap, grid_y, half_w, cell_h);
        let p_bl = RectF::new(body.x, grid_y + cell_h + gap, half_w, cell_h);
        let p_br = RectF::new(body.x + half_w + gap, grid_y + cell_h + gap, half_w, cell_h);

        // Panel 1 (TL): Spek Waterfall Spectrogram
        self.draw_waterfall_panel(r, p_tl, snap);
        // Panel 2 (TR): 4096/8192-pt Blackman-Harris FFT Curve & Cutoff
        self.draw_fft_cutoff_panel(r, p_tr, snap);
        // Panel 3 (BL): Stereo Soundstage / Goniometer (声场图)
        self.draw_soundstage_panel(r, p_bl, snap);
        // Panel 4 (BR): 32-Band Musical Spectrum + Stereo Oscilloscope
        self.draw_musical_scope_panel(r, p_br, snap);
    }

    fn draw_waterfall_panel(
        &self,
        r: &mut D2DRenderer,
        panel: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        r.fill_rounded_rect(panel, 12.0, Color::rgb(8, 9, 14));
        r.stroke_rounded_rect(panel, 12.0, Theme::SEPARATOR, 1.0);

        let nyq_hz = snap.nyquist_hz.max(8_000.0);
        let disp_max_hz = self.adaptive_max_hz.clamp(8_000.0, nyq_hz);
        let disp_khz = disp_max_hz / 1000.0;
        r.draw_text(
            &format!("声谱瀑布 · 0–{disp_khz:.0} kHz"),
            RectF::new(panel.x + 12.0, panel.y + 6.0, panel.w - 24.0, 18.0),
            11.5,
            600,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );

        let plot = RectF::new(
            panel.x + 40.0,
            panel.y + 30.0,
            (panel.w - 50.0).max(40.0),
            (panel.h - 38.0).max(40.0),
        );
        r.update_and_draw_waterfall(
            plot,
            &snap.inst_spectrum_db,
            Some(&snap.inst_spectrum_low24_db),
            nyq_hz,
            disp_max_hz,
            !snap.paused && snap.active_signal,
            self.last_dt_sec,
        );

        let step_k = if disp_khz > 52.0 {
            16.0
        } else if disp_khz > 28.0 {
            8.0
        } else {
            4.0
        };
        let mut khz = 0.0f32;
        while khz <= disp_khz + 0.05 {
            let y = plot.y + plot.h * (1.0 - (khz / disp_khz).clamp(0.0, 1.0));
            r.draw_line(plot.x, y, plot.right(), y, Color::rgba(255, 255, 255, 0.08), 1.0);
            r.draw_mono_text(
                &format!("{khz:.0}k"),
                RectF::new(panel.x + 2.0, y - 7.0, 34.0, 14.0),
                10.0,
                400,
                Theme::TEXT_MUTED,
                TextAlign::Right,
                true,
            );
            khz += step_k;
        }

        if snap.cutoff_hz > 4000.0 && snap.analyzed_frames >= 4 {
            let ratio = (snap.cutoff_hz / disp_max_hz.max(1.0)).clamp(0.0, 1.0);
            let cy = plot.y + plot.h * (1.0 - ratio);
            let is_lossy_cut = snap.tier_code.starts_with("lossy");
            let line_col = if is_lossy_cut {
                Theme::ACCENT_RED
            } else {
                Theme::SIGNAL_CYAN
            };
            r.draw_line(plot.x, cy, plot.right(), cy, line_col, 1.5);
            let tag_rect = RectF::new(plot.x + 6.0, (cy - 18.0).max(plot.y + 2.0), 74.0, 16.0);
            r.fill_rounded_rect(tag_rect, 4.0, Color::rgba(8, 10, 16, 0.84));
            let tag_lbl = format!("{:.1} kHz", snap.cutoff_hz / 1000.0);
            r.draw_mono_text(
                &tag_lbl,
                tag_rect,
                10.0,
                600,
                line_col,
                TextAlign::Center,
                true,
            );
        }
    }

    fn draw_fft_cutoff_panel(
        &mut self,
        r: &mut D2DRenderer,
        panel: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        r.fill_rounded_rect(panel, 12.0, Color::rgb(8, 9, 14));
        r.stroke_rounded_rect(panel, 12.0, Theme::SEPARATOR, 1.0);

        let nyq = snap.nyquist_hz.max(8_000.0);
        let disp_max = self.adaptive_max_hz.clamp(8_000.0, nyq);
        let disp_khz = disp_max / 1000.0;
        r.draw_text(
            &format!("FFT 频谱 · 0–{disp_khz:.0} kHz"),
            RectF::new(panel.x + 12.0, panel.y + 6.0, panel.w - 24.0, 18.0),
            11.5,
            600,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );

        let plot = RectF::new(
            panel.x + 38.0,
            panel.y + 30.0,
            (panel.w - 48.0).max(40.0),
            (panel.h - 46.0).max(40.0),
        );
        let min_db = -108.0f32;
        let max_db = 0.0f32;

        for &db in &[0.0f32, -24.0, -48.0, -72.0, -96.0] {
            let y = plot.y + plot.h * ((max_db - db) / (max_db - min_db));
            r.draw_line(plot.x, y, plot.right(), y, Color::rgba(255, 255, 255, 0.065), 1.0);
            r.draw_mono_text(
                &format!("{db:.0}"),
                RectF::new(panel.x + 2.0, y - 7.0, 32.0, 14.0),
                9.8,
                400,
                Theme::TEXT_MUTED,
                TextAlign::Right,
                true,
            );
        }

        for &(hz, col) in &[
            (16_000.0f32, Color::rgba(250, 45, 72, 0.25)),
            (20_000.0f32, Color::rgba(191, 90, 242, 0.28)),
            (22_050.0f32, Color::rgba(48, 209, 88, 0.28)),
            (24_000.0f32, Color::rgba(50, 173, 230, 0.26)),
            (48_000.0f32, Color::rgba(255, 190, 56, 0.26)),
        ] {
            if hz < disp_max - 200.0 {
                let x = plot.x + plot.w * (hz / disp_max);
                r.draw_line(x, plot.y, x, plot.bottom(), col, 1.0);
            }
        }

        let step_k = if disp_khz > 52.0 {
            16.0
        } else if disp_khz > 28.0 {
            8.0
        } else {
            4.0
        };
        let mut k = 0.0f32;
        while k <= disp_khz + 0.05 {
            let x = plot.x + plot.w * (k / disp_khz).clamp(0.0, 1.0);
            r.draw_mono_text(
                &format!("{k:.0}k"),
                RectF::new(x - 16.0, plot.bottom() + 2.0, 32.0, 14.0),
                9.8,
                400,
                Theme::TEXT_MUTED,
                TextAlign::Center,
                true,
            );
            k += step_k;
        }

        let use_low24 = disp_max <= 24_050.0 && snap.peak_envelope_low24_db.len() >= 64;
        let low_cap_hz = nyq.min(24_000.0);
        // Decimate to 128 vertices into reusable scratch buffer (zero per-frame heap alloc, 2x leaner PathGeometry)
        let fill_curve = |pts: &mut Vec<(f32, f32)>, bins: &[f32], src_max_hz: f32| {
            let n_src = bins.len();
            pts.clear();
            const CURVE_SAMPLES: usize = 128;
            for i in 0..CURVE_SAMPLES {
                let u = (i as f32) / ((CURVE_SAMPLES - 1) as f32);
                let freq = u * disp_max;
                let pos = ((freq / src_max_hz.max(1.0)) * ((n_src - 1) as f32))
                    .clamp(0.0, (n_src - 1) as f32);
                let i0 = pos.floor() as usize;
                let i1 = (i0 + 1).min(n_src - 1);
                let frac = pos - (i0 as f32);
                let db = (bins[i0] + (bins[i1] - bins[i0]) * frac).clamp(min_db, max_db);
                let x = plot.x + u * plot.w;
                let y = plot.y + plot.h * ((max_db - db) / (max_db - min_db));
                pts.push((x, y));
            }
        };

        let env_bins = if use_low24 {
            (&snap.peak_envelope_low24_db, low_cap_hz)
        } else {
            (&snap.peak_envelope_db, nyq)
        };
        if env_bins.0.len() >= 64 {
            fill_curve(&mut self.scratch_pts_a, env_bins.0, env_bins.1);
            r.draw_polyline(&self.scratch_pts_a, Theme::SIGNAL_CYAN.with_alpha(0.92), 1.6, false);
        }

        let inst_bins = if use_low24 && snap.inst_spectrum_low24_db.len() >= 64 {
            (&snap.inst_spectrum_low24_db, low_cap_hz)
        } else {
            (&snap.inst_spectrum_db, nyq)
        };
        if inst_bins.0.len() >= 64 {
            fill_curve(&mut self.scratch_pts_a, inst_bins.0, inst_bins.1);
            r.draw_polyline(&self.scratch_pts_a, Theme::ACCENT_RED.with_alpha(0.84), 1.25, false);
        }

        if snap.cutoff_hz > 4000.0 && snap.analyzed_frames >= 4 {
            let cx = plot.x + (snap.cutoff_hz / disp_max).clamp(0.0, 1.0) * plot.w;
            r.draw_line(cx, plot.y, cx, plot.bottom(), Theme::SIGNAL_AMBER, 1.6);
        }
    }

    /// Panel 3 (BL): Stereo Soundstage / Goniometer / Vectorscope (`声场图`)
    /// Dots default to monochrome studio blue; clicking the panel toggles 5-band colors <-> blue monochrome.
    fn draw_soundstage_panel(
        &mut self,
        r: &mut D2DRenderer,
        panel: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        self.hit_zones.push(VizHitZone {
            rect: panel,
            action: VizHitAction::ToggleSoundstageColorMode,
        });

        r.fill_rounded_rect(panel, 12.0, Color::rgb(8, 9, 14));
        r.stroke_rounded_rect(panel, 12.0, Theme::SEPARATOR, 1.0);

        r.draw_text(
            "声场图",
            RectF::new(panel.x + 12.0, panel.y + 6.0, 64.0, 18.0),
            11.5,
            600,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        let stage_hdr_metrics = if snap.active_signal || snap.paused {
            format!(
                "{:.0}% · {:+.2}",
                snap.stereo_width_pct, snap.phase_correlation
            )
        } else {
            "-- · --".to_string()
        };
        r.draw_mono_text(
            &stage_hdr_metrics,
            RectF::new(panel.right() - 120.0, panel.y + 6.0, 108.0, 18.0),
            10.5,
            500,
            if snap.active_signal {
                Theme::SIGNAL_CYAN
            } else {
                Theme::TEXT_MUTED
            },
            TextAlign::Right,
            true,
        );

        let stage_rect = RectF::new(
            panel.x + 12.0,
            panel.y + 26.0,
            panel.w - 24.0,
            (panel.h - 46.0).max(40.0),
        );
        let cx = stage_rect.center_x();
        let rad = (stage_rect.w * 0.44).min(stage_rect.h - 22.0).max(28.0);
        let cy = (stage_rect.bottom() - 10.0).max(stage_rect.y + rad + 12.0);

        // Upper-semicircle concentric reference arcs (0.33, 0.66, 1.0) — cached when panel size unchanged!
        let arc_key = (cx.to_bits(), cy.to_bits(), rad.to_bits());
        if self.soundstage_arcs_cache.0 != arc_key.0
            || self.soundstage_arcs_cache.1 != arc_key.1
            || self.soundstage_arcs_cache.2 != arc_key.2
            || self.soundstage_arcs_cache.3[0].is_empty()
        {
            const FRACS: [f32; 3] = [1.0, 0.66, 0.33];
            for (idx, &frac) in FRACS.iter().enumerate() {
                let rr = rad * frac;
                let dst = &mut self.soundstage_arcs_cache.3[idx];
                dst.clear();
                dst.reserve(37);
                for k in 0..=36 {
                    let ang = (k as f32 / 36.0) * PI;
                    dst.push((cx + rr * ang.cos(), cy - rr * ang.sin()));
                }
            }
            self.soundstage_arcs_cache.0 = arc_key.0;
            self.soundstage_arcs_cache.1 = arc_key.1;
            self.soundstage_arcs_cache.2 = arc_key.2;
        }
        for (idx, a) in [(0usize, 0.13f32), (1, 0.08), (2, 0.05)] {
            r.draw_polyline(
                &self.soundstage_arcs_cache.3[idx],
                Color::rgba(255, 255, 255, a),
                1.0,
                false,
            );
        }
        // Horizontal baseline diameter (antiphase S axis) & center vertical axis (Mono Mid)
        r.draw_line(cx - rad, cy, cx + rad, cy, Color::rgba(255, 255, 255, 0.12), 1.0);
        r.draw_line(cx, cy, cx, cy - rad, Color::rgba(255, 255, 255, 0.12), 1.0);
        // Diagonal Left (-45 deg) & Right (+45 deg) rays inside upper semicircle
        let diag = rad * FRAC_1_SQRT_2;
        r.draw_line(
            cx,
            cy,
            cx - diag,
            cy - diag,
            Color::rgba(50, 173, 230, 0.22),
            1.0,
        );
        r.draw_line(
            cx,
            cy,
            cx + diag,
            cy - diag,
            Color::rgba(250, 45, 72, 0.22),
            1.0,
        );

        // Stage labels: C/M (Center), L (-45°), R (+45°)
        r.draw_mono_text(
            "M",
            RectF::new(cx - 10.0, cy - rad - 13.0, 20.0, 12.0),
            9.5,
            600,
            Theme::TEXT_MUTED,
            TextAlign::Center,
            true,
        );
        r.draw_mono_text(
            "L",
            RectF::new(cx - diag - 15.0, cy - diag - 11.0, 15.0, 12.0),
            9.5,
            600,
            Theme::SIGNAL_CYAN,
            TextAlign::Center,
            true,
        );
        r.draw_mono_text(
            "R",
            RectF::new(cx + diag + 1.0, cy - diag - 11.0, 15.0, 12.0),
            9.5,
            600,
            Theme::ACCENT_RED,
            TextAlign::Center,
            true,
        );

        // Compact color mode indicator in top-left of Soundstage panel (click panel to toggle)
        let mut lx = panel.x + 12.0;
        let ly = panel.y + 26.0;
        if self.soundstage_multicolor {
            for b in 0..5 {
                let col = SOUNDSTAGE_BAND_COLORS[b];
                r.fill_circle(lx + 4.0, ly + 6.0, 3.0, col);
                let lbl = SOUNDSTAGE_BAND_LABELS[b];
                let lw = if lbl.chars().count() > 1 { 24.0 } else { 15.0 };
                r.draw_text(
                    lbl,
                    RectF::new(lx + 9.0, ly, lw, 12.0),
                    9.2,
                    500,
                    Theme::TEXT_MUTED,
                    TextAlign::Left,
                    true,
                    false,
                );
                lx += lw + 12.0;
            }
        } else {
            r.fill_circle(lx + 4.0, ly + 6.0, 3.0, SOUNDSTAGE_MONO_BLUE);
            r.draw_text(
                "单色",
                RectF::new(lx + 10.0, ly, 34.0, 12.0),
                9.2,
                500,
                Theme::TEXT_MUTED,
                TextAlign::Left,
                true,
                false,
            );
        }

        // Plot stereo pairs folded cleanly into the upper semicircle (default blue monochrome or 5-band colors)
        if !self.soundstage_phosphor.is_empty()
            || (snap.active_signal && snap.soundstage_lr.len() >= 16)
        {
            r.fill_radial_glow(
                cx,
                cy - rad * 0.22,
                rad * (0.22 + snap.stereo_width_pct * 0.0038),
                rad * 0.34,
                Color::rgba(56, 178, 255, 0.10),
                Color::rgba(56, 178, 255, 0.0),
            );

            // 1. Fixed-coordinate Phosphor Persistence Cloud inside upper semicircle
            // Spatial cell deduplication (32x16 grid, max 2 dots/cell) cuts overlapping FillEllipse calls ~6x!
            let mut cell_hits = [0u8; 32 * 16];
            for dot in self.soundstage_phosphor.iter().rev() {
                if dot.life < 0.045 {
                    continue;
                }
                let gx = (((dot.sx + 1.0) * 15.5).round() as isize).clamp(0, 31) as usize;
                let gy = ((dot.sy.max(0.0) * 15.0).round() as isize).clamp(0, 15) as usize;
                let cell = &mut cell_hits[gy * 32 + gx];
                if *cell >= 2 {
                    continue;
                }
                *cell += 1;
                let px = cx + dot.sx * rad;
                let py = cy - dot.sy.max(0.0) * rad;
                let glow_a = (dot.life.powf(1.38) * 0.54).clamp(0.018, 0.56);
                let base_col = if self.soundstage_multicolor {
                    soundstage_band_color(dot.band)
                } else {
                    SOUNDSTAGE_MONO_BLUE
                };
                r.fill_circle(px, py, 1.05, base_col.with_alpha(glow_a));
            }

            // 2. Live-frame excitations (decimated by coprime stride 2 for smooth 144Hz rendering)
            if snap.active_signal && !snap.soundstage_band_pts.is_empty() {
                let gain = self.soundstage_gain;
                let n = snap.soundstage_band_pts.len().max(1) as f32;
                let live_step = if snap.soundstage_band_pts.len() >= 180 { 2 } else { 1 };
                for (i, &[l, rr, bf]) in snap.soundstage_band_pts.iter().enumerate().step_by(live_step) {
                    let (sx, sy) = map_lr_to_semicircle(l, rr, gain);
                    let px = cx + sx * rad;
                    let py = cy - sy * rad;
                    let fresh = (i as f32) / n;
                    let bcol = if self.soundstage_multicolor {
                        soundstage_band_color(bf.round() as u8)
                    } else {
                        SOUNDSTAGE_MONO_BLUE.lerp(Color::rgb(168, 232, 255), fresh * 0.35)
                    };
                    if i % 10 == 0 && (sx.abs() + sy.abs()) > 0.28 {
                        r.fill_circle(px, py, 1.9, bcol.with_alpha(0.14 * fresh));
                    }
                    r.fill_circle(px, py, 1.12, bcol.with_alpha(0.45 + 0.50 * fresh));
                }
            }
        } else {
            r.fill_circle(cx, cy - 2.0, 1.8, SOUNDSTAGE_MONO_BLUE.with_alpha(0.58));
        }

        // Bottom Symmetric Left (`L`) & Right (`R`) Stereo Channel Level Bars!
        // Fixes the previous phase-correlation bar that always only filled the right half on stereo music.
        let bar_y = panel.bottom() - 16.0;
        let total_bar = RectF::new(panel.x + 26.0, bar_y, (panel.w - 52.0).max(40.0), 6.0);
        let gap_mid = 6.0;
        let half_bar_w = ((total_bar.w - gap_mid) * 0.5).max(12.0);
        let left_track = RectF::new(total_bar.x, bar_y, half_bar_w, 6.0);
        let right_track = RectF::new(total_bar.x + half_bar_w + gap_mid, bar_y, half_bar_w, 6.0);

        r.fill_rounded_rect(left_track, 3.0, Color::rgba(255, 255, 255, 0.08));
        r.fill_rounded_rect(right_track, 3.0, Color::rgba(255, 255, 255, 0.08));
        r.fill_circle(total_bar.center_x(), bar_y + 3.0, 1.6, Theme::TEXT_MUTED);

        let (lvl_l, lvl_r) = if snap.paused && snap.active_signal {
            (
                ((snap.peak_db_l + 58.0) / 56.0).clamp(0.0, 1.0),
                ((snap.peak_db_r + 58.0) / 56.0).clamp(0.0, 1.0),
            )
        } else {
            (self.smooth_lvl_l, self.smooth_lvl_r)
        };

        if lvl_l > 0.01 {
            let fw_l = (lvl_l * half_bar_w).clamp(3.0, half_bar_w);
            r.fill_rounded_rect(
                RectF::new(left_track.right() - fw_l, bar_y, fw_l, 6.0),
                3.0,
                Theme::SIGNAL_CYAN,
            );
        }
        if lvl_r > 0.01 {
            let fw_r = (lvl_r * half_bar_w).clamp(3.0, half_bar_w);
            r.fill_rounded_rect(
                RectF::new(right_track.x, bar_y, fw_r, 6.0),
                3.0,
                Theme::ACCENT_RED,
            );
        }

        r.draw_mono_text(
            "L",
            RectF::new(panel.x + 6.0, bar_y - 4.0, 18.0, 14.0),
            9.5,
            600,
            Theme::SIGNAL_CYAN,
            TextAlign::Left,
            true,
        );
        r.draw_mono_text(
            "R",
            RectF::new(panel.right() - 24.0, bar_y - 4.0, 18.0, 14.0),
            9.5,
            600,
            Theme::ACCENT_RED,
            TextAlign::Right,
            true,
        );
    }

    /// Panel 4 (BR): Merged 32-Band Musical Spectrum + Stereo PCM Oscilloscope
    fn draw_musical_scope_panel(
        &mut self,
        r: &mut D2DRenderer,
        panel: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        r.fill_rounded_rect(panel, 12.0, Color::rgb(8, 9, 14));
        r.stroke_rounded_rect(panel, 12.0, Theme::SEPARATOR, 1.0);

        r.draw_text(
            "频谱与示波",
            RectF::new(panel.x + 12.0, panel.y + 6.0, panel.w - 150.0, 18.0),
            11.5,
            600,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_mono_text(
            &format!(
                "Peak {:.1} · DR {:.1} dB",
                snap.peak_db_l.max(snap.peak_db_r),
                snap.dynamic_range_db
            ),
            RectF::new(panel.right() - 148.0, panel.y + 6.0, 136.0, 18.0),
            10.5,
            500,
            Theme::TEXT_MUTED,
            TextAlign::Right,
            true,
        );

        let inner = RectF::new(
            panel.x + 12.0,
            panel.y + 26.0,
            (panel.w - 24.0).max(40.0),
            (panel.h - 36.0).max(40.0),
        );
        let wave_h = inner.h * 0.38;
        let spec_y = inner.y + wave_h + 6.0;
        let spec_h = (inner.bottom() - spec_y).max(24.0);

        // Top sub-region: Stereo L/R PCM Oscilloscope
        let mid_y = inner.y + wave_h * 0.5;
        r.draw_line(
            inner.x,
            mid_y,
            inner.right(),
            mid_y,
            Color::rgba(255, 255, 255, 0.07),
            1.0,
        );
        if snap.wave_l.len() >= 16 {
            let n = snap.wave_l.len();
            let step_i = if n >= 160 { 2 } else { 1 };
            self.scratch_pts_a.clear();
            self.scratch_pts_b.clear();
            let denom = (n - 1) as f32;
            for i in (0..n).step_by(step_i) {
                let x = inner.x + (i as f32 / denom) * inner.w;
                self.scratch_pts_a
                    .push((x, mid_y - snap.wave_l[i] * wave_h * 0.45));
                self.scratch_pts_b
                    .push((x, mid_y - snap.wave_r[i] * wave_h * 0.45));
            }
            r.draw_polyline(&self.scratch_pts_b, Theme::SIGNAL_CYAN.with_alpha(0.68), 1.3, false);
            r.draw_polyline(&self.scratch_pts_a, Theme::ACCENT_RED.with_alpha(0.90), 1.6, false);
        }

        // Bottom sub-region: 32-band musical spectrum with peak-hold caps
        let step = inner.w / 32.0;
        let bw = (step * 0.68).max(2.5);
        let base_y = spec_y + spec_h;
        for i in 0..32 {
            let v = snap.spectrum_32[i].clamp(0.0, 1.0);
            let bh = (v * spec_h).max(2.0);
            let bx = inner.x + (i as f32) * step + (step - bw) * 0.5;
            let bar_col = Theme::SIGNAL_CYAN.lerp(Theme::ACCENT_RED, (i as f32) / 31.0);
            r.fill_rounded_rect(RectF::new(bx, base_y - bh, bw, bh), 2.0, bar_col);

            let pk = snap.peaks_32[i].clamp(0.0, 1.0);
            if pk > 0.03 {
                r.fill_rect(
                    RectF::new(bx, (base_y - pk * spec_h - 2.0).max(spec_y), bw, 2.0),
                    Color::rgba(255, 255, 255, 0.80),
                );
            }
        }
    }

    // ========================================================================
    // Style 1: osu! Circular Spectrum + BPM Emblem Bounce (律动圆环)
    // ========================================================================
    fn draw_osu_circle_style(
        &self,
        r: &mut D2DRenderer,
        body: RectF,
        snap: &LiveVisualizerSnapshot,
        artwork_key: Option<&str>,
    ) {
        r.fill_rounded_rect(body, 16.0, Color::rgb(9, 9, 14));
        r.stroke_rounded_rect(body, 16.0, Theme::SEPARATOR, 1.0);
        r.push_clip(body);

        let cx = body.center_x();
        let cy = body.center_y();
        let min_dim = body.w.min(body.h);
        let base_r = (min_dim * 0.215).clamp(58.0, 138.0);
        let pulse_r = base_r * self.emblem_scale;

        // Ambient radial stage bloom reactive to bass & beat
        r.fill_radial_glow(
            cx,
            cy,
            pulse_r * 2.8,
            pulse_r * 2.8,
            Color::rgba(250, 45, 72, 0.11 + snap.beat_pulse * 0.14),
            Color::rgba(16, 12, 28, 0.0),
        );

        // Starlight particles drifting outward like osu! kiai/menu particles
        let max_p_dist = min_dim * 0.54;
        for p in &self.particles {
            let d = pulse_r * 0.95 + p.dist_norm * max_p_dist;
            let px = cx + d * p.angle.cos();
            let py = cy + d * p.angle.sin();
            let fade = (1.0 - p.dist_norm).max(0.0).powf(0.75) * p.alpha;
            r.fill_circle(px, py, p.size, Color::rgba(255, 220, 238, fade));
        }

        // Shockwave beat ripple rings
        for rip in &self.ripples {
            r.stroke_circle(
                cx,
                cy,
                pulse_r + rip.radius_extra,
                Color::rgba(255, 110, 165, rip.alpha),
                2.0,
            );
        }

        // 120-bar symmetric mirrored radial spectrum around the pulsing emblem!
        let max_bar_len = (min_dim * 0.23).clamp(48.0, 148.0);
        let total_bars = 160usize;
        let bar_r_start = pulse_r + 6.0;
        for i in 0..total_bars {
            // Mirror smoothly around circle so 0 deg and 360 deg meet with zero discontinuity
            let half = total_bars / 2;
            let mirror_t = if i < half {
                (i as f32) / (half as f32)
            } else {
                ((total_bars - i) as f32) / (half as f32)
            };
            let bin = ((mirror_t * 119.0).round() as usize).min(119);
            let val = snap.radial_120[bin].clamp(0.0, 1.0);
            let len = 4.0 + val.powf(1.12) * max_bar_len;

            let ang = -PI * 0.5 + self.osu_rot + (i as f32 / total_bars as f32) * 2.0 * PI;
            let (sa, ca) = ang.sin_cos();
            let x0 = cx + bar_r_start * ca;
            let y0 = cy + bar_r_start * sa;
            let x1 = cx + (bar_r_start + len) * ca;
            let y1 = cy + (bar_r_start + len) * sa;

            let col = Color::rgb(255, 72, 138)
                .lerp(Color::rgb(64, 210, 255), mirror_t)
                .with_alpha(0.88);
            r.draw_line(x0, y0, x1, y1, col, 3.1);

            // Subtle inner halo bar
            let in_len = val * 14.0;
            if in_len > 1.0 {
                let ix1 = cx + (bar_r_start + len + 4.0 + in_len * 0.35) * ca;
                let iy1 = cy + (bar_r_start + len + 4.0 + in_len * 0.35) * sa;
                r.fill_circle(ix1, iy1, 1.6, Color::rgba(255, 255, 255, 0.55 * val));
            }
        }

        // Central osu!-style Emblem Disc (circular album cover or stylized glowing badge)
        r.fill_circle(cx, cy, pulse_r + 4.0, Color::rgba(255, 255, 255, 0.92));
        let inner_r = pulse_r;
        let art_rect = RectF::new(cx - inner_r, cy - inner_r, inner_r * 2.0, inner_r * 2.0);
        let drew_art = artwork_key
            .map(|k| r.draw_rounded_artwork(k, true, art_rect, inner_r, 1.0))
            .unwrap_or(false);

        if !drew_art {
            r.fill_radial_glow(
                cx,
                cy,
                inner_r,
                inner_r,
                Color::rgb(255, 68, 128),
                Color::rgb(168, 28, 84),
            );
            r.draw_icon(
                SfIcon::MusicNote,
                RectF::new(
                    cx - inner_r * 0.42,
                    cy - inner_r * 0.48,
                    inner_r * 0.84,
                    inner_r * 0.84,
                ),
                Color::rgb(255, 255, 255),
            );
        }

        // Subtle BPM pill inside lower edge of emblem (with PLL beat-phase indicator dot)
        let bpm_pill = RectF::new(cx - 48.0, cy + inner_r * 0.46, 96.0, 22.0);
        let beat_flash = (-snap.beat_phase.clamp(0.0, 1.0) * 6.0).exp();
        r.fill_rounded_rect(bpm_pill, 11.0, Color::rgba(10, 10, 15, 0.76));
        r.stroke_rounded_rect(
            bpm_pill,
            11.0,
            Color::rgba(255, 110, 165, 0.18 + 0.45 * beat_flash),
            1.0,
        );
        r.draw_mono_text(
            &format!("{:.0} BPM", snap.estimated_bpm.round()),
            bpm_pill,
            11.0,
            600,
            Theme::TEXT_PRIMARY,
            TextAlign::Center,
            true,
        );

        r.pop_clip();
    }

    // ========================================================================
    // Style 2A (Default): 3D Luminescent Cuboid Matrix Core (`media_1794910079586_825fccbd.jpg`)
    // Click anywhere on the reactor viewport to toggle to Classic RBMK Control Rods!
    // ========================================================================
    fn draw_reactor_matrix_core_style(
        &mut self,
        r: &mut D2DRenderer,
        body: RectF,
        snap: &LiveVisualizerSnapshot,
        smtc: &SmtcState,
        artwork_key: Option<&str>,
    ) {
        // Click anywhere in reactor canvas to switch to classic Chernobyl rods style!
        self.hit_zones.push(VizHitZone {
            rect: body,
            action: VizHitAction::ToggleReactorSubStyle,
        });

        // Deep midnight slate-indigo -> dark abyssal floor gradient (`#24283E` -> `#0D0B17`)
        r.fill_linear_gradient_rect(
            body,
            16.0,
            body.x,
            body.y,
            body.x,
            body.bottom(),
            Color::rgb(35, 39, 61),
            Color::rgb(13, 11, 23),
        );
        r.stroke_rounded_rect(body, 16.0, Theme::SEPARATOR, 1.0);
        r.push_clip(body);

        let cx = body.center_x() - body.w * 0.02;
        let cy = body.y + body.h * 0.63;
        let span_w = (body.w * 0.90).clamp(340.0, 1280.0);
        let base_tile_w = (span_w / 34.2).clamp(8.6, 31.0);
        let base_tile_h = base_tile_w * 0.47;

        let mean_z = self
            .matrix_pillars
            .iter()
            .map(|p| p.z)
            .sum::<f32>()
            / (self.matrix_pillars.len().max(1) as f32);
        let core_glow_energy = ((mean_z / 11.5) + snap.bass_energy * 0.52).clamp(0.16, 1.0);

        // Soft ambient purple-magenta under-glow beneath the matrix floor
        r.fill_radial_glow(
            cx,
            cy - base_tile_h * 1.6,
            span_w * 0.46,
            span_w * 0.23,
            Color::rgba(154, 56, 244, 0.30 * core_glow_energy),
            Color::rgba(18, 12, 30, 0.0),
        );

        // Render crisp 19x19 (<= 361 active) foreshortened 3D cuboids sampled at stride 2 across
        // the full 37x37 lattice + outer-corner culling, cutting D2D calls from 5,476 -> ~1,150/frame!
        let mut drawn_cuboids = 0usize;
        for p in &self.matrix_pillars {
            if (p.gx & 1) != 0 || (p.gy & 1) != 0 {
                continue;
            }
            let d = p.dist_norm;
            if d > 1.12 && p.z < 0.8 {
                continue;
            }
            let fx = p.gx as f32;
            let fy = p.gy as f32;
            let depth_row = (fx + fy) / 36.0; // -1.0 (back) .. +1.0 (front)
            let persp = 1.0 + depth_row * 0.17;

            let step_x = base_tile_w * 0.50 * persp;
            let step_y = base_tile_h * 0.50 * (0.94 + depth_row * 0.11);
            let iso_x = cx + (fx - fy) * step_x;
            let iso_y_base = cy + (fx + fy) * step_y;

            // Skip tiles completely outside viewport
            if iso_x < body.x - 48.0
                || iso_x > body.right() + 48.0
                || iso_y_base > body.bottom() + 48.0
                || iso_y_base < body.y - 28.0
            {
                continue;
            }

            let rest_dome = (-2.9 * d * d).exp() * (2.6 + p.seed * 2.4);
            let h = (p.z * persp + rest_dome).clamp(0.5, 92.0);
            let h_norm = (h / 54.0).clamp(0.0, 1.0);
            let core_prox = (1.0 - (d / 0.88).clamp(0.0, 1.0)).powf(1.55);

            // Multi-stop thermal luminescence palette:
            // Obsidian-Slate -> Electric Violet -> Neon Orchid -> Sakura Pink -> Peach-Gold Core!
            let heat = (h_norm * 0.66 + core_prox * (0.26 + core_glow_energy * 0.30)).clamp(0.0, 1.0);
            let top_col = matrix_core_heat_color(heat);
            let left_col = Color {
                r: (top_col.r * 0.88 + 0.04 * heat).min(1.0),
                g: (top_col.g * 0.84 + 0.02 * heat).min(1.0),
                b: (top_col.b * 0.90).min(1.0),
                a: 1.0,
            };
            let right_col = Color {
                r: top_col.r * 0.56,
                g: top_col.g * 0.50,
                b: (top_col.b * 0.66 + 0.03).min(1.0),
                a: 1.0,
            };
            let bevel_col = if heat > 0.10 {
                top_col
                    .lerp(Color::rgb(255, 245, 252), 0.36)
                    .with_alpha((0.20 + heat * 0.54).min(0.84))
            } else {
                Color::rgba(92, 64, 148, 0.18)
            };

            let half_tw = step_x * 1.76;
            let half_th = step_y * 1.76;
            r.draw_iso_matrix_cuboid(
                iso_x,
                iso_y_base,
                h,
                half_tw,
                half_th,
                left_col,
                right_col,
                top_col,
                bevel_col,
            );
            drawn_cuboids += 1;
        }
        self.last_matrix_drawn_count = drawn_cuboids;

        // Soft incandescent core bloom veil over the central peach-gold cluster
        r.fill_radial_glow(
            cx,
            cy - 18.0 - mean_z * 0.7,
            span_w * 0.22,
            span_w * 0.12,
            Color::rgba(255, 195, 148, 0.16 * core_glow_energy),
            Color::rgba(218, 78, 240, 0.0),
        );

        // Top-Right Floating Track Card (`media_1794910079586_825fccbd.jpg` 1:1!)
        let card_w = 296.0f32.min(body.w - 32.0);
        let card_h = 82.0f32;
        if card_w >= 200.0 {
            let card = RectF::new(body.right() - card_w - 20.0, body.y + 18.0, card_w, card_h);
            r.fill_rounded_rect(card, 14.0, Color::rgba(28, 31, 46, 0.82));
            r.stroke_rounded_rect(card, 14.0, Color::rgba(255, 255, 255, 0.09), 1.0);

            let art_r = RectF::new(card.x + 12.0, card.y + 11.0, 60.0, 60.0);
            let drew = artwork_key
                .map(|k| r.draw_rounded_artwork(k, false, art_r, 10.0, 1.0))
                .unwrap_or(false);
            if !drew {
                r.fill_rounded_rect(art_r, 10.0, Color::rgba(255, 255, 255, 0.08));
                r.draw_icon(SfIcon::MusicNote, art_r.inset(16.0, 16.0), Theme::TEXT_MUTED);
            }

            let tx = art_r.right() + 14.0;
            let tw = (card.right() - tx - 14.0).max(60.0);
            let title = if smtc.title.is_empty() {
                "AM Light"
            } else {
                &smtc.title
            };
            let artist = if smtc.artist.is_empty() {
                "正在监听系统音频流"
            } else {
                &smtc.artist
            };
            r.draw_text(
                title,
                RectF::new(tx, card.y + 14.0, tw, 20.0),
                14.2,
                700,
                Theme::TEXT_PRIMARY,
                TextAlign::Left,
                true,
                false,
            );
            r.draw_text(
                artist,
                RectF::new(tx, card.y + 34.0, tw, 16.0),
                11.8,
                400,
                Color::rgba(200, 204, 220, 0.62),
                TextAlign::Left,
                true,
                false,
            );

            let dur = smtc.duration_sec.max(0.0);
            let pos = smtc.position_sec.clamp(0.0, dur.max(0.0));
            let ts_str = format!("{} / {}", fmt_viz_mmss(pos), fmt_viz_mmss(dur));
            r.draw_mono_text(
                &ts_str,
                RectF::new(card.right() - 78.0, card.bottom() - 23.0, 66.0, 14.0),
                10.0,
                500,
                Color::rgba(200, 204, 220, 0.68),
                TextAlign::Right,
                true,
            );

            let bar_w = (tw - 74.0).max(36.0);
            let bar = RectF::new(tx, card.bottom() - 17.5, bar_w, 3.2);
            r.fill_rounded_rect(bar, 1.6, Color::rgba(255, 255, 255, 0.10));
            let frac = if dur > 0.5 {
                (pos / dur).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            if frac > 0.005 {
                r.fill_rounded_rect(
                    RectF::new(bar.x, bar.y, (bar.w * frac).max(3.2), bar.h),
                    1.6,
                    Color::rgb(156, 198, 232),
                );
            }
        }

        // Subtle switch hint in bottom-left
        r.draw_text(
            "立体发光堆芯 · 点击画面切换控制棒视图",
            RectF::new(body.x + 16.0, body.bottom() - 26.0, 260.0, 18.0),
            11.0,
            500,
            Color::rgba(210, 198, 240, 0.44),
            TextAlign::Left,
            true,
            false,
        );

        r.pop_clip();
    }

    // ========================================================================
    // Style 2B: Chernobyl RBMK-1000 Biological Shield Control Rods (点击切换)
    // ========================================================================
    fn draw_chernobyl_rods_style(
        &mut self,
        r: &mut D2DRenderer,
        body: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        self.hit_zones.push(VizHitZone {
            rect: body,
            action: VizHitAction::ToggleReactorSubStyle,
        });

        r.fill_rounded_rect(body, 16.0, Color::rgb(7, 10, 16));
        r.stroke_rounded_rect(body, 16.0, Theme::SEPARATOR, 1.0);
        r.push_clip(body);

        let cx = body.center_x();
        let cy = body.center_y() + body.h * 0.04;

        // Ambient Cherenkov radiation under-glow beneath the RBMK core vessel
        let total_lift: f32 =
            self.rods.iter().map(|rd| rd.z).sum::<f32>() / (self.rods.len().max(1) as f32);
        let cherenkov_intensity = if total_lift > 0.05 {
            (total_lift / 22.0 + snap.bass_energy * 0.35).clamp(0.08, 1.0)
        } else {
            0.05
        };
        let core_rx = (body.w.min(body.h * 1.65) * 0.42).clamp(150.0, 380.0);
        let core_ry = core_rx * 0.53;

        r.fill_radial_glow(
            cx,
            cy,
            core_rx * 1.18,
            core_ry * 1.25,
            Color::rgba(0, 174, 255, 0.24 * cherenkov_intensity),
            Color::rgba(0, 60, 140, 0.0),
        );

        // Outer steel biological shield vessel rim ("Elena" ring) — cached when viewport unchanged!
        let ring_key = (cx.to_bits(), cy.to_bits(), core_rx.to_bits(), core_ry.to_bits());
        if self.elena_ring_cache.0 != ring_key.0
            || self.elena_ring_cache.1 != ring_key.1
            || self.elena_ring_cache.2 != ring_key.2
            || self.elena_ring_cache.3 != ring_key.3
            || self.elena_ring_cache.4.is_empty()
        {
            self.elena_ring_cache.4.clear();
            self.elena_ring_cache.4.reserve(49);
            for i in 0..=48 {
                let a = (i as f32 / 48.0) * 2.0 * PI;
                self.elena_ring_cache
                    .4
                    .push((cx + core_rx * 1.06 * a.cos(), cy + core_ry * 1.06 * a.sin()));
            }
            self.elena_ring_cache.0 = ring_key.0;
            self.elena_ring_cache.1 = ring_key.1;
            self.elena_ring_cache.2 = ring_key.2;
            self.elena_ring_cache.3 = ring_key.3;
        }
        r.draw_polyline(
            &self.elena_ring_cache.4,
            Color::rgba(80, 110, 145, 0.35),
            2.0,
            true,
        );

        // Isometric tile basis vectors
        let tile_w = (core_rx / 9.2).clamp(16.0, 38.0);
        let tile_h = tile_w * 0.52;
        let half_tw = tile_w * 0.46;
        let half_th = tile_h * 0.46;

        // Draw each steel biological shield block from back to front (zero-allocation GPU affine batch!)
        for rod in &self.rods {
            let fx = rod.gx as f32;
            let fy = rod.gy as f32;
            let iso_x = cx + (fx - fy) * (tile_w * 0.52);
            let iso_y_base = cy + (fx + fy) * (tile_h * 0.52);
            if iso_x < body.x - 42.0
                || iso_x > body.right() + 42.0
                || iso_y_base > body.bottom() + 42.0
                || iso_y_base < body.y - 42.0
            {
                continue;
            }
            let lift = rod.z;
            let wall_h = (lift + 5.5).min(52.0);

            let glow_a = if lift > 1.2 {
                ((lift / 28.0).clamp(0.08, 0.92)) * 0.85
            } else {
                0.0
            };

            let blue_tint = (lift / 34.0).clamp(0.0, 0.72);
            let left_col = Color::rgb(34, 39, 48).lerp(Color::rgb(18, 128, 215), blue_tint * 0.75);
            let right_col = Color::rgb(24, 28, 36).lerp(Color::rgb(10, 96, 185), blue_tint * 0.65);

            let cap_base = match rod.rod_kind {
                1 => Color::rgb(176, 56, 58),  // Manual control rod cap (Red)
                2 => Color::rgb(182, 148, 48), // Shortened absorber rod (Yellow)
                3 => Color::rgb(46, 118, 196), // AZ-5 emergency rod (Blue)
                _ => Color::rgb(64, 71, 82),   // Fuel channel steel cap
            };
            let cap_lit = cap_base.lerp(Color::rgb(175, 235, 255), blue_tint * 0.35);
            let rim_col = Color::rgba(205, 230, 255, 0.18 + blue_tint * 0.35);

            r.draw_iso_reactor_rod(
                iso_x,
                iso_y_base,
                lift,
                half_tw,
                half_th,
                wall_h,
                glow_a,
                left_col,
                right_col,
                cap_lit,
                rim_col,
            );
        }

        // Restrained technical telemetry caption in top-left of reactor viewport
        let mwt = if snap.active_signal && total_lift > 0.05 {
            3200.0 * (0.18 + cherenkov_intensity * 1.45)
        } else {
            0.0
        };
        r.draw_mono_text(
            &format!("RBMK-1000 · UPPER BIOLOGICAL SHIELD   {mwt:5.0} MWt"),
            RectF::new(body.x + 16.0, body.y + 12.0, 380.0, 20.0),
            11.0,
            600,
            Color::rgba(120, 210, 255, 0.78),
            TextAlign::Left,
            true,
        );
        r.draw_text(
            "控制棒视图 · 点击画面切换立体发光堆芯",
            RectF::new(body.x + 16.0, body.bottom() - 26.0, 260.0, 18.0),
            11.0,
            500,
            Color::rgba(120, 210, 255, 0.46),
            TextAlign::Left,
            true,
            false,
        );

        r.pop_clip();
    }

    // ========================================================================
    // Style 3: Windows XP Windows Media Player Retro Visualizer (XP 复古)
    // ========================================================================
    fn draw_winxp_retro_style(
        &mut self,
        r: &mut D2DRenderer,
        body: RectF,
        snap: &LiveVisualizerSnapshot,
        hovered_pt: (f32, f32),
        mouse_down: bool,
        reduce_transparency: bool,
    ) {
        // Pure pitch-black CRT WMP stage
        r.fill_rounded_rect(body, 14.0, Color::rgb(0, 0, 0));
        r.stroke_rounded_rect(body, 14.0, Color::rgb(38, 56, 92), 1.5);
        r.push_clip(body);

        let preset_name = match self.xp_sub_preset % 3 {
            0 => "条形和波浪: 烈焰 (Fire)",
            1 => "条形和波浪: 翠绿电光 (Emerald)",
            _ => "迷幻极光 (WMP Alchemy)",
        };

        let btn = RectF::new(body.right() - 212.0, body.y + 11.0, 198.0, 28.0);
        let bhov = btn.contains(hovered_pt.0, hovered_pt.1);
        r.draw_liquid_glass_button(
            btn,
            14.0,
            GlassBtnVariant::Tinted(Color::rgb(35, 84, 178)),
            bhov,
            mouse_down,
            hovered_pt,
            reduce_transparency,
        );
        r.draw_text(
            preset_name,
            btn,
            11.5,
            500,
            Color::rgb(215, 235, 255),
            TextAlign::Center,
            true,
            false,
        );
        self.hit_zones.push(VizHitZone {
            rect: btn,
            action: VizHitAction::CycleXpSubPreset,
        });

        r.draw_mono_text(
            "Windows Media Player 9 Series",
            RectF::new(body.x + 16.0, body.y + 14.0, 240.0, 20.0),
            11.0,
            600,
            Color::rgba(115, 168, 245, 0.72),
            TextAlign::Left,
            true,
        );

        let pad_x = 28.0;
        let inner = RectF::new(
            body.x + pad_x,
            body.y + 48.0,
            (body.w - pad_x * 2.0).max(80.0),
            (body.h - 68.0).max(80.0),
        );

        if self.xp_sub_preset % 3 == 2 {
            // Classic WMP "Alchemy / Battery" rotating neon spirograph ribbons!
            let cx = inner.center_x();
            let cy = inner.center_y();
            let rx = inner.w * 0.38;
            let ry = inner.h * 0.42;
            let t = self.time_sec;

            for layer in 0..4 {
                let lf = layer as f32;
                let mut pts = Vec::with_capacity(129);
                for i in 0..=128 {
                    let u = (i as f32) / 128.0;
                    let ang = u * 2.0 * PI * (2.0 + lf) + t * (0.9 + lf * 0.32);
                    let w_idx = (i * snap.wave_l.len() / 129).min(snap.wave_l.len() - 1);
                    let mod_r = 0.42
                        + snap.bass_energy * 0.35
                        + snap.wave_l[w_idx] * 0.28
                        + (u * 12.0 + t * 2.1).sin() * 0.12;
                    let px = cx + rx * mod_r * (ang + lf * 0.7).cos();
                    let py = cy + ry * mod_r * (ang * 1.5 - t * 0.6).sin();
                    pts.push((px, py));
                }
                let col = match layer {
                    0 => Color::rgba(0, 255, 128, 0.78),
                    1 => Color::rgba(0, 195, 255, 0.72),
                    2 => Color::rgba(185, 80, 255, 0.65),
                    _ => Color::rgba(255, 230, 60, 0.58),
                };
                r.draw_polyline(&pts, col, 2.0, true);
            }
        } else {
            let is_fire = (self.xp_sub_preset % 3) == 0;
            // Upper WMP Stereo Scope Ribbon
            let scope_cy = inner.y + inner.h * 0.20;
            let scope_amp = inner.h * 0.16;
            if snap.wave_l.len() >= 16 {
                let n = snap.wave_l.len();
                let mut wl = Vec::with_capacity(n);
                let mut wr = Vec::with_capacity(n);
                for i in 0..n {
                    let x = inner.x + (i as f32 / (n - 1) as f32) * inner.w;
                    wl.push((x, scope_cy - snap.wave_l[i] * scope_amp));
                    wr.push((x, scope_cy - snap.wave_r[i] * scope_amp * 0.85 + 3.0));
                }
                let w_col1 = if is_fire {
                    Color::rgba(0, 225, 255, 0.85)
                } else {
                    Color::rgba(150, 255, 255, 0.85)
                };
                let w_col2 = if is_fire {
                    Color::rgba(255, 230, 80, 0.55)
                } else {
                    Color::rgba(0, 255, 110, 0.55)
                };
                r.draw_polyline(&wr, w_col2, 1.4, false);
                r.draw_polyline(&wl, w_col1, 1.9, false);
            }

            // Classic 48-bar WMP gradient bars + slow-falling white peak dots
            let bars_top = inner.y + inner.h * 0.34;
            let bars_h = inner.bottom() - bars_top;
            let base_y = inner.bottom();
            let step = inner.w / 48.0;
            let bw = (step * 0.74).max(3.0);

            let (c_bot, c_top, cap_col) = if is_fire {
                (
                    Color::rgb(220, 28, 0),
                    Color::rgb(255, 238, 65),
                    Color::rgb(255, 255, 255),
                )
            } else {
                (
                    Color::rgb(0, 145, 25),
                    Color::rgb(195, 255, 65),
                    Color::rgb(180, 255, 255),
                )
            };

            for i in 0..48 {
                let src = (i * 64 / 48).min(63);
                let v = snap.spectrum_64[src].clamp(0.0, 1.0);
                let bh = (v * bars_h).max(2.0);
                let bx = inner.x + (i as f32) * step + (step - bw) * 0.5;
                let br = RectF::new(bx, base_y - bh, bw, bh);
                r.fill_linear_gradient_rect(br, 0.0, bx, base_y, bx, bars_top, c_bot, c_top);

                // Floating WMP peak-hold cap
                let pk = self.xp_peaks[i].clamp(0.0, 1.0);
                if pk > 0.015 {
                    let py = (base_y - pk * bars_h - 3.0).max(bars_top);
                    r.fill_rect(RectF::new(bx, py, bw, 2.4), cap_col);
                }
            }
        }

        r.pop_clip();
    }

    // ========================================================================
    // Style 4: Sonic Silk Aurora Waves (流光丝绸)
    // ========================================================================
    fn draw_silk_aurora_style(
        &mut self,
        r: &mut D2DRenderer,
        body: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        r.fill_rounded_rect(body, 16.0, Color::rgb(9, 10, 16));
        r.stroke_rounded_rect(body, 16.0, Theme::SEPARATOR, 1.0);
        r.push_clip(body);

        let cx = body.center_x();
        let cy = body.center_y();
        let amp_max = body.h * 0.36;

        // Ambient silk aurora stage bloom
        r.fill_radial_glow(
            cx - body.w * 0.16,
            cy,
            body.w * 0.44,
            body.h * 0.42,
            Color::rgba(250, 45, 72, 0.10 + snap.bass_energy * 0.12),
            Color::rgba(9, 10, 16, 0.0),
        );
        r.fill_radial_glow(
            cx + body.w * 0.18,
            cy,
            body.w * 0.44,
            body.h * 0.42,
            Color::rgba(50, 173, 230, 0.10 + snap.high_energy * 0.14),
            Color::rgba(9, 10, 16, 0.0),
        );

        let colors = [
            Color::rgba(250, 45, 72, 0.84),
            Color::rgba(191, 90, 242, 0.74),
            Color::rgba(50, 173, 230, 0.78),
            Color::rgba(48, 209, 88, 0.62),
            Color::rgba(255, 214, 10, 0.54),
        ];

        for (layer, &col) in colors.iter().enumerate() {
            let lf = layer as f32;
            self.scratch_pts_a.clear();
            for i in 0..=112 {
                let u = (i as f32) / 112.0;
                let x = body.x + u * body.w;
                // Guard `.max(0.0)` before `.powf()` so `(1.0 * PI).sin() == -8.7e-8` NEVER yields `NaN`!
                let env = (u * PI).sin().max(0.0).powf(0.85);
                let b_idx = ((u * 63.0) as usize).min(63);
                let spec_e = snap.spectrum_64[b_idx].clamp(0.0, 1.0);
                let phase1 = u * (5.0 + lf * 1.4) * PI + self.time_sec * (1.8 + lf * 0.45);
                let phase2 = u * (9.0 - lf) * PI - self.time_sec * 1.25;
                let disp = (phase1.sin() * 0.62 + phase2.cos() * 0.38)
                    * env
                    * amp_max
                    * (0.14 + spec_e * 0.75 + snap.bass_energy * 0.35);
                let y = cy + if disp.is_finite() { disp } else { 0.0 };
                self.scratch_pts_a.push((x, y));
            }
            // Soft outer halo stroke + crisp core ribbon
            r.draw_polyline(&self.scratch_pts_a, col.with_alpha(col.a * 0.28), 6.0, false);
            r.draw_polyline(&self.scratch_pts_a, col, 2.4, false);
        }

        r.pop_clip();
    }
}

/// Multi-stop luminescent color ramp matching `media_1794910079586_825fccbd.jpg`:
/// Dark Slate (`#1A172B`) -> Deep Violet (`#3E1F70`) -> Electric Amethyst (`#7A2FE0`)
/// -> Neon Orchid (`#D64CF8`) -> Sakura Pink (`#FF8BC6`) -> Peach-Coral (`#FFB882`) -> Molten Gold (`#FFE48E`)
fn matrix_core_heat_color(t: f32) -> Color {
    const STOPS: &[(f32, Color)] = &[
        (0.00, Color::rgb(26, 23, 43)),
        (0.16, Color::rgb(58, 30, 104)),
        (0.36, Color::rgb(120, 46, 222)),
        (0.56, Color::rgb(208, 74, 246)),
        (0.75, Color::rgb(255, 138, 198)),
        (0.90, Color::rgb(255, 186, 132)),
        (1.00, Color::rgb(255, 228, 142)),
    ];
    let x = t.clamp(0.0, 1.0);
    for i in 0..(STOPS.len() - 1) {
        let (p0, c0) = STOPS[i];
        let (p1, c1) = STOPS[i + 1];
        if x >= p0 && x <= p1 {
            let u = (x - p0) / (p1 - p0).max(1e-5);
            return c0.lerp(c1, u);
        }
    }
    STOPS[STOPS.len() - 1].1
}

fn fmt_viz_mmss(sec: f64) -> String {
    let s = sec.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
