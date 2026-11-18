#![cfg(target_os = "windows")]

use crate::am_agent::AudioPrefs;
use crate::audio_viz::{AudioOutputDevice, LiveVisualizerSnapshot, StreamAudioSpecs};
use crate::d2d_renderer::{D2DRenderer, GlassBtnVariant, SfIcon, TextAlign};
use crate::smtc::SmtcState;
use crate::theme::{Color, RectF, Theme};
use std::collections::VecDeque;
use std::f32::consts::PI;

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
    pan_t: f32,
    life: f32,
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
    soundstage_gain: f32,
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
            soundstage_phosphor: VecDeque::with_capacity(1664),
            soundstage_gain: 1.35,
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
            hit_zones: Vec::with_capacity(16),
        }
    }
}

pub const INSPECTOR_HEADER_FONT_SIZE: f32 = 11.2;

pub fn inspector_detected_header_avail_w(det_card_w: f32) -> f32 {
    (det_card_w - 166.0 - 12.0 - 6.0).max(56.0)
}

pub fn format_inspector_detected_header(
    active_signal: bool,
    confidence_pct: u8,
    avail_width_dip: f32,
) -> String {
    if !active_signal {
        return "实测音质 · 静音待命".to_string();
    }
    if avail_width_dip >= 246.0 {
        format!("实测音质 · WASAPI 实时鉴定 (置信度 {confidence_pct}%)")
    } else if avail_width_dip >= 216.0 {
        format!("实测音质 · WASAPI 鉴定 (置信度 {confidence_pct}%)")
    } else if avail_width_dip >= 166.0 {
        format!("实测音质 · 鉴定 (置信度 {confidence_pct}%)")
    } else if avail_width_dip >= 94.0 {
        format!("实测音质 · {confidence_pct}%")
    } else {
        format!("实测 {confidence_pct}%")
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

    pub fn step(&mut self, snap: &LiveVisualizerSnapshot, dt_sec: f32, reduce_motion: bool) {
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

        // 0. True Fixed-Coordinate CRT Phosphor Persistence (`声场图`)
        // Existing dots stay strictly frozen at their birth `(sx, sy)` coordinates while decaying exponentially!
        let phos_decay = (-2.25 * dt).exp();
        for dot in &mut self.soundstage_phosphor {
            dot.life *= phos_decay;
        }
        self.soundstage_phosphor.retain(|d| d.life > 0.018);

        if is_audible && !snap.soundstage_lr.is_empty() {
            let mut sum_l2 = 0.0f32;
            let mut sum_r2 = 0.0f32;
            let mut max_amp = 0.16f32;
            for &[l, rr] in &snap.soundstage_lr {
                sum_l2 += l * l;
                sum_r2 += rr * rr;
                max_amp = max_amp.max(l.abs()).max(rr.abs());
            }
            let n_inv = 1.0 / (snap.soundstage_lr.len() as f32);
            let rms_l_db = 20.0 * (sum_l2 * n_inv).sqrt().max(1.58e-5).log10();
            let rms_r_db = 20.0 * (sum_r2 * n_inv).sqrt().max(1.58e-5).log10();
            let eff_l_db = rms_l_db.max(snap.peak_db_l - 6.0);
            let eff_r_db = rms_r_db.max(snap.peak_db_r - 6.0);
            let tgt_l = ((eff_l_db + 58.0) / 56.0).clamp(0.0, 1.0);
            let tgt_r = ((eff_r_db + 58.0) / 56.0).clamp(0.0, 1.0);
            let k_bar = (1.0 - (-18.0 * dt).exp()).clamp(0.05, 1.0);
            self.smooth_lvl_l += (tgt_l - self.smooth_lvl_l) * k_bar;
            self.smooth_lvl_r += (tgt_r - self.smooth_lvl_r) * k_bar;

            let target_gain = (0.88 / max_amp).clamp(0.75, 3.5);
            let gain_rate = if target_gain < self.soundstage_gain {
                (8.0 * dt).min(1.0)
            } else {
                (1.8 * dt).min(1.0)
            };
            self.soundstage_gain += (target_gain - self.soundstage_gain) * gain_rate;
            let gain = self.soundstage_gain;

            let stride = if snap.soundstage_lr.len() >= 192 { 3 } else { 2 };
            for &[l, rr] in snap.soundstage_lr.iter().step_by(stride) {
                let sx = ((rr - l) * 0.7071 * gain).clamp(-1.0, 1.0);
                let sy = ((l + rr) * 0.7071 * gain).clamp(-1.0, 1.0);
                let pan_t = (sx * 0.88 + 0.5).clamp(0.0, 1.0);
                if self.soundstage_phosphor.len() >= 1600 {
                    self.soundstage_phosphor.pop_front();
                }
                self.soundstage_phosphor.push_back(PhosphorDot {
                    sx,
                    sy,
                    pan_t,
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

        if reduce_motion {
            self.emblem_scale = 1.0 + snap.bass_energy * 0.03;
            for rod in &mut self.rods {
                rod.vz = 0.0;
                rod.z = if is_audible {
                    let b = (11 + ((rod.dist_norm * 32.0) as usize)).min(58);
                    (snap.spectrum_64[b] * 34.0).clamp(0.0, 38.0)
                } else {
                    0.0
                };
            }
            for p in &mut self.matrix_pillars {
                p.vz = 0.0;
                p.z = if is_audible {
                    let dome = (-2.6 * p.dist_norm * p.dist_norm).exp();
                    let b = ((p.dist_norm * 46.0) as usize).min(60);
                    ((snap.spectrum_64[b] * 0.85 + snap.bass_energy * dome) * 52.0).clamp(0.0, 56.0)
                } else {
                    0.0
                };
            }
            return;
        }

        // 1. osu! circular emblem phase-locked BPM bounce & particles
        let bpm_norm = ((snap.estimated_bpm - 70.0) / 110.0).clamp(0.0, 1.2);
        self.osu_rot =
            (self.osu_rot + dt * (0.11 + snap.bass_energy * 0.22 + bpm_norm * 0.06)) % (2.0 * PI);
        if snap.beat_triggered && is_audible {
            self.emblem_vel += 2.65 + snap.bass_energy * 1.45;
            if self.ripples.len() < 7 {
                self.ripples.push(ShockwaveRing {
                    radius_extra: 0.0,
                    alpha: 0.64,
                });
            }
        }
        // Couple emblem resting target to the PLL beat_phase envelope so every tracked beat visibly pulses!
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

        // 2. High-Sensitivity Multi-Band Reactor Physics (RBMK Control Rods + 37x37 Luminescent Matrix Core)
        if is_audible {
            let target_agc = spec_peak.clamp(0.09, 0.85);
            let agc_rate = if target_agc > self.reactor_agc {
                (13.0 * dt).min(1.0)
            } else {
                (2.5 * dt).min(1.0)
            };
            self.reactor_agc += (target_agc - self.reactor_agc) * agc_rate;

            let effective_db = if snap.rms_db > -90.0 {
                snap.rms_db.max(peak_db - 5.5)
            } else {
                -58.0 + spec_peak * 52.0
            };
            let loud_scale = ((effective_db + 58.0) / 38.0).clamp(0.30, 1.32);
            let agc_inv = (1.0 / self.reactor_agc.max(0.095)).clamp(1.10, 4.8);

            // Per-band transient spectral flux (sub-bass, vocal/snare mids, hi-hat highs)
            let bass_flux = (snap.bass_energy - self.bass_ema).max(0.0);
            let mid_flux = (snap.mid_energy - self.mid_ema).max(0.0);
            let high_flux = (snap.high_energy - self.high_ema).max(0.0);
            let band_ema_k = (1.0 - (-7.5 * dt).exp()).clamp(0.01, 1.0);
            self.bass_ema += (snap.bass_energy - self.bass_ema) * band_ema_k;
            self.mid_ema += (snap.mid_energy - self.mid_ema) * band_ema_k;
            self.high_ema += (snap.high_energy - self.high_ema) * band_ema_k;

            let broadband_now =
                snap.bass_energy * 0.95 + snap.mid_energy * 1.25 + snap.high_energy * 0.85;
            let bb_rise = (broadband_now - self.broadband_ema).max(0.0);
            self.broadband_ema += (broadband_now - self.broadband_ema) * band_ema_k;
            self.flux_cooldown_sec = (self.flux_cooldown_sec - dt).max(0.0);
            self.matrix_kick_cd = (self.matrix_kick_cd - dt).max(0.0);

            let flux_onset = (bb_rise > 0.022 || bass_flux > 0.032 || mid_flux > 0.035)
                && self.flux_cooldown_sec <= 0.0;
            let transient_hit = snap.beat_triggered || flux_onset;
            if transient_hit {
                self.onset_counter = self.onset_counter.wrapping_add(1);
                if !snap.beat_triggered {
                    self.flux_cooldown_sec = 0.075;
                }
            }

            // Launch radial shockwave across the 37x37 matrix on beats & strong transients!
            if (snap.beat_triggered || bass_flux > 0.042 || mid_flux > 0.048)
                && self.matrix_kick_cd <= 0.0
            {
                self.shockwave_radius = 0.04;
                self.shockwave_strength = (0.62
                    + snap.bass_energy * 0.65
                    + bass_flux * 2.8
                    + mid_flux * 2.2)
                    .clamp(0.45, 1.38);
                self.matrix_kick_cd = 0.085;
            } else {
                self.shockwave_radius += (2.45 + snap.bass_energy * 1.2) * dt;
                self.shockwave_strength *= (-2.8 * dt).exp();
            }

            let onset_group = (self.onset_counter as usize) % 3;
            let wave_phase = self.time_sec * 8.2;

            // 2A. RBMK-1000 Control Rods (High sensitivity & dynamic vertical range)
            for (idx, rod) in self.rods.iter_mut().enumerate() {
                let azim_slot = (((rod.angle + PI) / (2.0 * PI)) * 14.0).floor() as usize % 14;
                let musical_b = (10 + ((rod.dist_norm * 30.0) as usize) + azim_slot).min(58);
                let upper_b = (musical_b + 5).min(62);
                let raw_band = snap.spectrum_64[musical_b]
                    .max(snap.spectrum_64[upper_b] * 0.88)
                    .max(snap.mid_energy * 0.72 + snap.high_energy * 0.45 * rod.dist_norm);
                let bass_punch = (1.0 - rod.dist_norm * 0.70).max(0.0) * snap.bass_energy * 1.05;
                let flux_boost = bass_flux * 1.3 + mid_flux * 1.4 + high_flux * 1.1;
                let norm_exc =
                    ((raw_band + bass_punch + flux_boost) * agc_inv).clamp(0.0, 1.35) * loud_scale;

                let radial_wave =
                    (rod.dist_norm * 7.2 - wave_phase + rod.angle * 2.0).sin() * 0.5 + 0.5;
                let phase_i = (idx as f32) * 2.399_963;
                let chatter_freq = 11.8 + ((idx % 5) as f32) * 1.55;
                let chatter_osc = (self.time_sec * chatter_freq + phase_i).sin() * 0.5 + 0.5;
                let kind_gain = if rod.rod_kind != 0 { 1.22 } else { 0.98 };
                let core_surge =
                    (1.0 - rod.dist_norm * 0.62).max(0.0) * snap.beat_pulse * 0.65 * loud_scale;

                let drive =
                    norm_exc * (0.42 + 0.36 * radial_wave + 0.40 * chatter_osc) * kind_gain
                        + core_surge;
                let target_z = (drive * 40.0).clamp(0.0, 58.0);

                if snap.beat_triggered && rod.dist_norm < 0.68 {
                    rod.vz += (1.0 - rod.dist_norm * 0.72) * 235.0 * loud_scale;
                } else if transient_hit && (idx % 3 == onset_group) {
                    let flux_kick = (bb_rise * agc_inv * 210.0).clamp(48.0, 185.0) * loud_scale;
                    rod.vz += flux_kick * kind_gain;
                }

                let gravity = if rod.z > target_z { -108.0 } else { 0.0 };
                let accel = (target_z - rod.z) * 180.0 - rod.vz * 12.6 + gravity;
                rod.vz += accel * dt;
                rod.z = (rod.z + rod.vz * dt).min(60.0);
                if rod.z < 0.0 {
                    rod.z = 0.0;
                    if rod.vz < -12.0 {
                        rod.vz = -rod.vz * 0.28;
                    } else {
                        rod.vz = 0.0;
                    }
                }
            }

            // 2B. High-Density 37x37 Luminescent Cuboid Matrix Core (Full-spectrum + Shockwave sensitivity!)
            let radial_phase = self.time_sec * 5.8;
            for p in &mut self.matrix_pillars {
                let d = p.dist_norm;
                let core_dome = (-2.65 * d * d).exp();
                let mid_ring = (-((d - 0.52) * 3.1).powi(2)).exp();
                let outer_skirt = ((d - 0.55) * 1.4).clamp(0.0, 1.0) * (1.38 - d).clamp(0.0, 1.0);

                let azim = (((p.angle + PI) / (2.0 * PI)) * 20.0).floor() as usize % 20;
                let spec_bin = ((d * 38.0) as usize + azim).min(61);
                let hi_bin = (36 + ((p.seed * 24.0) as usize)).min(62);
                let band_e = (snap.spectrum_64[spec_bin] * agc_inv).clamp(0.0, 1.45);
                let hi_e = (snap.spectrum_64[hi_bin] * agc_inv).clamp(0.0, 1.35);

                let ripple =
                    (d * 10.5 - radial_phase + p.seed * 4.2 + p.angle * 2.0).sin() * 0.5 + 0.5;
                let micro =
                    (self.time_sec * (6.4 + p.seed * 5.6) + p.seed * 23.0).sin() * 0.5 + 0.5;

                // Expanding transient shockwave crest across all 37x37 pillars
                let sw_dist = d - self.shockwave_radius;
                let shockwave_lift =
                    (-sw_dist * sw_dist * 38.0).exp() * self.shockwave_strength * 26.0;

                let core_drive = core_dome
                    * (0.28
                        + snap.bass_energy * 1.45
                        + snap.mid_energy * 0.98
                        + snap.beat_pulse * 0.88
                        + bass_flux * 2.6)
                    * (0.64 + 0.42 * micro + 0.34 * p.seed);
                let mid_drive = (mid_ring * 0.85 + (1.0 - (d * 0.65).min(0.92)) * 0.55)
                    * (band_e * 1.15 + mid_flux * 2.4)
                    * (0.34 + 0.46 * ripple + 0.28 * micro);
                let high_drive = outer_skirt
                    * (hi_e * 0.95 + snap.high_energy * 1.30 + high_flux * 3.4)
                    * (0.35 + 0.65 * micro);

                let target_z = ((core_drive * 48.0
                    + mid_drive * 32.0
                    + high_drive * 24.0
                    + shockwave_lift)
                    * loud_scale)
                    .clamp(0.0, 84.0);

                if snap.beat_triggered && d < 0.65 {
                    p.vz += core_dome * (165.0 + p.seed * 95.0) * loud_scale;
                } else if flux_onset && p.seed > 0.52 {
                    p.vz += (mid_flux + high_flux) * 240.0 * (0.4 + 0.6 * p.seed) * loud_scale;
                }

                let accel = (target_z - p.z) * 175.0 - p.vz * 13.8;
                p.vz += accel * dt;
                p.z = (p.z + p.vz * dt).clamp(0.0, 88.0);
            }
        } else {
            self.broadband_ema = 0.0;
            self.bass_ema = 0.0;
            self.mid_ema = 0.0;
            self.high_ema = 0.0;
            self.shockwave_strength = 0.0;
            self.matrix_kick_cd = 0.0;
            self.flux_cooldown_sec = 0.0;
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

        // 3. Windows XP WMP gravity peak-hold caps
        for i in 0..48 {
            let src_idx = (i * 64 / 48).min(63);
            let val = if is_audible {
                snap.spectrum_64[src_idx]
            } else {
                0.0
            };
            if val >= self.xp_peaks[i] && val > 0.0 {
                self.xp_peaks[i] = val;
                self.xp_peak_vel[i] = -0.04; // brief hang at apex!
            } else if self.xp_peaks[i] > 0.0 {
                let grav = if is_audible { 0.62 } else { 1.35 };
                self.xp_peak_vel[i] += grav * dt; // classic WMP gravity acceleration
                self.xp_peaks[i] =
                    (self.xp_peaks[i] - self.xp_peak_vel[i].max(0.0) * dt).max(0.0);
                if self.xp_peaks[i] == 0.0 {
                    self.xp_peak_vel[i] = 0.0;
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
        let top_cards_h = 128.0f32.min(body.h * 0.30).max(108.0);
        let half_w = (body.w - gap) * 0.5;

        // --- Card A: Expected Stream Audio Quality (应有音质) ---
        let exp_card = RectF::new(body.x, body.y, half_w, top_cards_h);
        r.fill_rounded_rect(exp_card, 12.0, Theme::BG_ELEVATED);
        r.stroke_rounded_rect(exp_card, 12.0, Theme::SEPARATOR, 1.0);

        r.draw_text(
            "应有音质 · 源流规格",
            RectF::new(exp_card.x + 14.0, exp_card.y + 10.0, exp_card.w - 28.0, 18.0),
            12.0,
            600,
            Theme::TEXT_MUTED,
            TextAlign::Left,
            true,
            false,
        );

        let (exp_headline, exp_sub, exp_detail) = if let Some(sp) = expected_specs {
            let bd_str = sp
                .bit_depth
                .map(|b| format!("{b}-bit / "))
                .unwrap_or_default();
            let sr_khz = (sp.sample_rate_hz as f32) / 1000.0;
            let sr_str = if (sr_khz - sr_khz.round()).abs() < 0.05 {
                format!("{:.0} kHz", sr_khz)
            } else {
                format!("{:.1} kHz", sr_khz)
            };
            (
                format!("{} · {}", sp.tier_label, sp.codec_label),
                format!("{bd_str}{sr_str} · {}", sp.channels),
                format!(
                    "目标码率 {} kbps (峰值 {} kbps)",
                    sp.bitrate_kbps, sp.peak_bitrate_kbps
                ),
            )
        } else {
            let cfg_tier = if prefs.dolby_atmos_setting == 20 {
                "杜比全景声 (优先)"
            } else if !prefs.lossless_enabled {
                "高质量 AAC (256 kbps)"
            } else if prefs.streaming_audio_quality >= 20 {
                "Hi-Res 无损 (最高 24-bit / 192 kHz)"
            } else {
                "无损 ALAC (最高 24-bit / 48 kHz)"
            };
            let badge = expected_badge.unwrap_or("待播放曲目获取源流清单");
            (
                cfg_tier.to_string(),
                format!("曲目标记：{badge}"),
                format!(
                    "终端输出：{} ({:.1} kHz / {}-bit)",
                    snap.device_name,
                    snap.sample_rate as f32 / 1000.0,
                    snap.bit_depth
                ),
            )
        };

        r.draw_text(
            &exp_headline,
            RectF::new(exp_card.x + 14.0, exp_card.y + 32.0, exp_card.w - 28.0, 24.0),
            15.5,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            &exp_sub,
            RectF::new(exp_card.x + 14.0, exp_card.y + 57.0, exp_card.w - 28.0, 18.0),
            12.5,
            500,
            Theme::SIGNAL_CYAN,
            TextAlign::Left,
            true,
            false,
        );
        r.draw_text(
            &exp_detail,
            RectF::new(exp_card.x + 14.0, exp_card.y + 76.0, exp_card.w - 28.0, 16.0),
            11.5,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
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
            let pr = RectF::new(pill_x, pill_y, 54.0, 21.0);
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
            pill_x += 59.0;
        }

        if devices.len() > 1 {
            let dev_btn = RectF::new(exp_card.right() - 98.0, pill_y, 84.0, 21.0);
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
                    "切换输出设备",
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

        // --- Card B: Actual Detected System Audio Quality (实测音质) ---
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

        // Action buttons: 自动清零 / 冻结 / 清空重测 (sized cleanly so full header fits at 1185 DIP without ellipsis)
        let rst_btn = RectF::new(det_card.right() - 66.0, det_card.y + 8.0, 54.0, 22.0);
        let frz_btn = RectF::new(rst_btn.x - 42.0, det_card.y + 8.0, 38.0, 22.0);
        let auto_btn = RectF::new(frz_btn.x - 58.0, det_card.y + 8.0, 54.0, 22.0);

        let hdr_avail_w = inspector_detected_header_avail_w(det_card.w);
        let status_hdr =
            format_inspector_detected_header(snap.active_signal, snap.confidence_pct, hdr_avail_w);
        r.draw_text(
            &status_hdr,
            RectF::new(det_card.x + 12.0, det_card.y + 10.0, hdr_avail_w, 18.0),
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
                    "保持包络"
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
            (rst_btn, "清空重测", false, VizHitAction::InspectorReset),
        ] {
            let bhov = br.contains(hovered_pt.0, hovered_pt.1);
            r.draw_liquid_glass_button(
                br,
                11.0,
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
            RectF::new(det_card.x + 14.0, det_card.y + 32.0, det_card.w - 28.0, 24.0),
            15.5,
            700,
            Theme::TEXT_PRIMARY,
            TextAlign::Left,
            true,
            false,
        );

        let cut_str = if snap.cutoff_hz > 1000.0 {
            format!("{:.2} kHz", snap.cutoff_hz / 1000.0)
        } else {
            "-".to_string()
        };
        let det_metrics = format!(
            "截止上限 {cut_str} (落差 {:.1}dB) · MDCT 空洞 {:.1}% · {}",
            snap.brickwall_drop_db, snap.mdct_hole_pct, snap.detected_bit_grid
        );
        r.draw_text(
            &det_metrics,
            RectF::new(det_card.x + 14.0, det_card.y + 57.0, det_card.w - 28.0, 18.0),
            12.2,
            500,
            verdict_accent,
            TextAlign::Left,
            true,
            false,
        );

        let apps_str = if let Some(top_app) = snap.active_apps.first() {
            format!(
                "估算码率 {} · 活跃发声：{} ({:.1} dB) · 电平 {:.1} dBFS",
                snap.estimated_bitrate, top_app.exe_name, top_app.peak_db, snap.rms_db
            )
        } else {
            format!(
                "估算码率 {} · {}",
                snap.estimated_bitrate, snap.forensic_notes
            )
        };
        r.draw_text(
            &apps_str,
            RectF::new(det_card.x + 14.0, det_card.y + 79.0, det_card.w - 28.0, 38.0),
            11.3,
            400,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            false,
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

        let nyq_khz = (snap.nyquist_hz / 1000.0).max(20.0);
        r.draw_text(
            &format!("声谱瀑布图 (0 – {nyq_khz:.1} kHz 线性频率)"),
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
            !snap.paused && snap.active_signal,
            self.last_dt_sec,
        );

        let step_k = if nyq_khz > 30.0 { 8.0 } else { 4.0 };
        let mut khz = 0.0f32;
        while khz <= nyq_khz + 0.05 {
            let y = plot.y + plot.h * (1.0 - (khz / nyq_khz).clamp(0.0, 1.0));
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
            let ratio = (snap.cutoff_hz / snap.nyquist_hz.max(1.0)).clamp(0.0, 1.0);
            let cy = plot.y + plot.h * (1.0 - ratio);
            let is_lossy_cut = snap.tier_code.starts_with("lossy");
            let line_col = if is_lossy_cut {
                Theme::ACCENT_RED
            } else {
                Theme::SIGNAL_CYAN
            };
            r.draw_line(plot.x, cy, plot.right(), cy, line_col, 1.5);
            let tag_rect = RectF::new(plot.x + 6.0, (cy - 18.0).max(plot.y + 2.0), 124.0, 16.0);
            r.fill_rounded_rect(tag_rect, 4.0, Color::rgba(8, 10, 16, 0.84));
            let tag_lbl = if snap.tier_code == "narrowband_analyzing"
                || snap.tier_code == "lossless_acoustic"
            {
                format!("谐波 {:.2} kHz", snap.cutoff_hz / 1000.0)
            } else {
                format!("实测 {:.2} kHz", snap.cutoff_hz / 1000.0)
            };
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
        &self,
        r: &mut D2DRenderer,
        panel: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        r.fill_rounded_rect(panel, 12.0, Color::rgb(8, 9, 14));
        r.stroke_rounded_rect(panel, 12.0, Theme::SEPARATOR, 1.0);

        r.draw_text(
            "4096/8192点 FFT 峰值包络与高频切线检测",
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

        let nyq = snap.nyquist_hz.max(20_000.0);
        for &(hz, col) in &[
            (16_000.0f32, Color::rgba(250, 45, 72, 0.25)),
            (20_000.0f32, Color::rgba(191, 90, 242, 0.28)),
            (22_050.0f32, Color::rgba(48, 209, 88, 0.28)),
        ] {
            if hz < nyq {
                let x = plot.x + plot.w * (hz / nyq);
                r.draw_line(x, plot.y, x, plot.bottom(), col, 1.0);
            }
        }

        let nyq_khz = nyq / 1000.0;
        let step_k = if nyq_khz > 30.0 { 8.0 } else { 4.0 };
        let mut k = 0.0f32;
        while k <= nyq_khz + 0.05 {
            let x = plot.x + plot.w * (k / nyq_khz).clamp(0.0, 1.0);
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

        if snap.peak_envelope_db.len() == 256 {
            let mut env_pts = Vec::with_capacity(256);
            for i in 0..256 {
                let x = plot.x + (i as f32 / 255.0) * plot.w;
                let db = snap.peak_envelope_db[i].clamp(min_db, max_db);
                let y = plot.y + plot.h * ((max_db - db) / (max_db - min_db));
                env_pts.push((x, y));
            }
            r.draw_polyline(&env_pts, Theme::SIGNAL_CYAN.with_alpha(0.92), 1.6, false);
        }

        if snap.inst_spectrum_db.len() == 256 {
            let mut inst_pts = Vec::with_capacity(256);
            for i in 0..256 {
                let x = plot.x + (i as f32 / 255.0) * plot.w;
                let db = snap.inst_spectrum_db[i].clamp(min_db, max_db);
                let y = plot.y + plot.h * ((max_db - db) / (max_db - min_db));
                inst_pts.push((x, y));
            }
            r.draw_polyline(&inst_pts, Theme::ACCENT_RED.with_alpha(0.84), 1.25, false);
        }

        if snap.cutoff_hz > 4000.0 && snap.analyzed_frames >= 4 {
            let cx = plot.x + (snap.cutoff_hz / nyq).clamp(0.0, 1.0) * plot.w;
            r.draw_line(cx, plot.y, cx, plot.bottom(), Theme::SIGNAL_AMBER, 1.6);
        }
    }

    /// Panel 3 (BL): Stereo Soundstage / Goniometer / Vectorscope (`声场图`)
    fn draw_soundstage_panel(
        &self,
        r: &mut D2DRenderer,
        panel: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        r.fill_rounded_rect(panel, 12.0, Color::rgb(8, 9, 14));
        r.stroke_rounded_rect(panel, 12.0, Theme::SEPARATOR, 1.0);

        r.draw_text(
            "立体声声场图 (Goniometer / 相位场)",
            RectF::new(panel.x + 12.0, panel.y + 6.0, panel.w - 135.0, 18.0),
            11.5,
            600,
            Theme::TEXT_SECONDARY,
            TextAlign::Left,
            true,
            false,
        );
        let stage_hdr_metrics = if snap.active_signal || snap.paused {
            format!(
                "宽度 {:.0}% · 相位 {:+.2}",
                snap.stereo_width_pct, snap.phase_correlation
            )
        } else {
            "宽度 -- · 相位 --".to_string()
        };
        r.draw_mono_text(
            &stage_hdr_metrics,
            RectF::new(panel.right() - 150.0, panel.y + 6.0, 138.0, 18.0),
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
            (panel.h - 50.0).max(40.0),
        );
        let cx = stage_rect.center_x();
        let cy = stage_rect.center_y();
        let rad = (stage_rect.w.min(stage_rect.h) * 0.46).max(24.0);

        // Polar circles & 45-degree rotated L/R + Mid/Side axes
        r.stroke_circle(cx, cy, rad, Color::rgba(255, 255, 255, 0.10), 1.0);
        r.stroke_circle(cx, cy, rad * 0.55, Color::rgba(255, 255, 255, 0.06), 1.0);
        // Vertical Mid (Mono Center) & Horizontal Side axes
        r.draw_line(cx, cy - rad, cx, cy + rad, Color::rgba(255, 255, 255, 0.11), 1.0);
        r.draw_line(cx - rad, cy, cx + rad, cy, Color::rgba(255, 255, 255, 0.09), 1.0);
        // Diagonal Left (-45 deg) & Right (+45 deg) axes
        let diag = rad * 0.7071;
        r.draw_line(
            cx - diag,
            cy - diag,
            cx + diag,
            cy + diag,
            Color::rgba(50, 173, 230, 0.16),
            1.0,
        );
        r.draw_line(
            cx + diag,
            cy - diag,
            cx - diag,
            cy + diag,
            Color::rgba(250, 45, 72, 0.16),
            1.0,
        );

        // Stage labels: M (Center), L, R, S
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
            RectF::new(cx - diag - 16.0, cy - diag - 10.0, 16.0, 12.0),
            9.5,
            600,
            Theme::SIGNAL_CYAN,
            TextAlign::Center,
            true,
        );
        r.draw_mono_text(
            "R",
            RectF::new(cx + diag, cy - diag - 10.0, 16.0, 12.0),
            9.5,
            600,
            Theme::ACCENT_RED,
            TextAlign::Center,
            true,
        );

        // Plot stereo (L, R) pairs as a fine CRT Phosphor Scatter Plot (`散点图` with true fixed-coordinate persistence):
        // X = (R - L) * 0.7071 (Left is negative X, Right is positive X)
        // Y = (L + R) * 0.7071 (In-phase Mid extends vertically)
        if !self.soundstage_phosphor.is_empty()
            || (snap.active_signal && snap.soundstage_lr.len() >= 16)
        {
            r.fill_radial_glow(
                cx,
                cy,
                rad * (0.18 + snap.stereo_width_pct * 0.0038),
                rad * 0.34,
                Color::rgba(50, 173, 230, 0.11),
                Color::rgba(50, 173, 230, 0.0),
            );

            // 1. Fixed-coordinate Phosphor Persistence Cloud:
            // Every dot stays anchored at its exact birth `(sx, sy)` while its luminescence decays smoothly over time!
            for dot in &self.soundstage_phosphor {
                let px = cx + dot.sx * rad;
                let py = cy - dot.sy * rad;
                let glow_a = (dot.life.powf(1.48) * 0.46).clamp(0.014, 0.50);
                let dot_col = if dot.pan_t < 0.5 {
                    Theme::SIGNAL_CYAN.lerp(Color::rgb(176, 240, 255), dot.pan_t * 2.0)
                } else {
                    Color::rgb(176, 240, 255).lerp(Theme::ACCENT_RED, (dot.pan_t - 0.5) * 2.0)
                }
                .with_alpha(glow_a);
                r.fill_circle(px, py, 0.82, dot_col);
            }

            // 2. Live-frame fine electron-beam excitations (`1.00 px` radius)
            if snap.active_signal && snap.soundstage_lr.len() >= 16 {
                let gain = self.soundstage_gain;
                let n = snap.soundstage_lr.len().max(1) as f32;
                for (i, &[l, rr]) in snap.soundstage_lr.iter().enumerate() {
                    let sx = ((rr - l) * 0.7071 * gain).clamp(-1.0, 1.0);
                    let sy = ((l + rr) * 0.7071 * gain).clamp(-1.0, 1.0);
                    let px = cx + sx * rad;
                    let py = cy - sy * rad;
                    let fresh = (i as f32) / n;
                    let pan_t = (sx * 0.90 + 0.5).clamp(0.0, 1.0);
                    let edge_tint = if pan_t < 0.5 {
                        Theme::SIGNAL_CYAN.lerp(Color::rgb(232, 250, 255), pan_t * 2.0)
                    } else {
                        Color::rgb(232, 250, 255).lerp(Theme::ACCENT_RED, (pan_t - 0.5) * 2.0)
                    };
                    if i % 8 == 0 && (sx.abs() + sy.abs()) > 0.28 {
                        r.fill_circle(px, py, 1.85, edge_tint.with_alpha(0.11 * fresh));
                    }
                    r.fill_circle(px, py, 1.00, edge_tint.with_alpha(0.38 + 0.54 * fresh));
                }
            }
        } else {
            r.fill_circle(cx, cy, 1.8, Theme::SIGNAL_CYAN.with_alpha(0.58));
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
        &self,
        r: &mut D2DRenderer,
        panel: RectF,
        snap: &LiveVisualizerSnapshot,
    ) {
        r.fill_rounded_rect(panel, 12.0, Color::rgb(8, 9, 14));
        r.stroke_rounded_rect(panel, 12.0, Theme::SEPARATOR, 1.0);

        r.draw_text(
            "32段对数频谱与双声道示波器",
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
                "Peak {:.1} / DR {:.1} dB",
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
            let mut pts_l = Vec::with_capacity(n);
            let mut pts_r = Vec::with_capacity(n);
            for i in 0..n {
                let x = inner.x + (i as f32 / (n - 1) as f32) * inner.w;
                pts_l.push((x, mid_y - snap.wave_l[i] * wave_h * 0.45));
                pts_r.push((x, mid_y - snap.wave_r[i] * wave_h * 0.45));
            }
            r.draw_polyline(&pts_r, Theme::SIGNAL_CYAN.with_alpha(0.68), 1.3, false);
            r.draw_polyline(&pts_l, Theme::ACCENT_RED.with_alpha(0.90), 1.6, false);
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

        // Render 37x37 (1,369!) foreshortened 3D cuboids from back to front
        for p in &self.matrix_pillars {
            let fx = p.gx as f32;
            let fy = p.gy as f32;
            let depth_row = (fx + fy) / 36.0; // -1.0 (back) .. +1.0 (front)
            let persp = 1.0 + depth_row * 0.17;

            let step_x = base_tile_w * 0.50 * persp;
            let step_y = base_tile_h * 0.50 * (0.94 + depth_row * 0.11);
            let iso_x = cx + (fx - fy) * step_x;
            let iso_y_base = cy + (fx + fy) * step_y;

            // Skip tiles completely outside viewport
            if iso_x < body.x - 42.0
                || iso_x > body.right() + 42.0
                || iso_y_base > body.bottom() + 42.0
                || iso_y_base < body.y - 24.0
            {
                continue;
            }

            let d = p.dist_norm;
            let rest_dome = (-2.9 * d * d).exp() * (2.6 + p.seed * 2.4);
            let h = (p.z * persp + rest_dome).clamp(0.5, 92.0);
            let h_norm = (h / 54.0).clamp(0.0, 1.0);
            let core_prox = (1.0 - (d / 0.88).clamp(0.0, 1.0)).powf(1.55);

            // Multi-stop thermal luminescence palette matching screenshot:
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

            let half_tw = step_x * 0.88;
            let half_th = step_y * 0.88;
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
        }

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

        // Outer steel biological shield vessel rim ("Elena" ring)
        let mut ring_pts = Vec::with_capacity(49);
        for i in 0..=48 {
            let a = (i as f32 / 48.0) * 2.0 * PI;
            ring_pts.push((cx + core_rx * 1.06 * a.cos(), cy + core_ry * 1.06 * a.sin()));
        }
        r.draw_polyline(&ring_pts, Color::rgba(80, 110, 145, 0.35), 2.0, true);

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
        &self,
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
            let mut pts = Vec::with_capacity(145);
            for i in 0..=144 {
                let u = (i as f32) / 144.0;
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
                pts.push((x, y));
            }
            // Soft outer halo stroke + crisp core ribbon
            r.draw_polyline(&pts, col.with_alpha(col.a * 0.28), 6.0, false);
            r.draw_polyline(&pts, col, 2.4, false);
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
