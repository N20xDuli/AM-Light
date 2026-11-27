use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinauralConfig {
    pub enabled: bool,
    #[serde(default)]
    pub bypass: bool,
    pub preset: String,
    pub spatial_width: f32,
    pub room_ambience: f32,
    pub crossfeed: f32,
    pub center_focus: f32,
    pub bass_warmth_db: f32,
    #[serde(default)]
    pub target_device_id: String,
    #[serde(default)]
    pub capture_bus_id: String,
}

impl Default for BinauralConfig {
    fn default() -> Self {
        Self::from_preset("apple_spatial", false, String::new(), String::new())
    }
}

impl BinauralConfig {
    pub fn from_preset(
        preset: &str,
        enabled: bool,
        target_device_id: String,
        capture_bus_id: String,
    ) -> Self {
        let (spatial_width, room_ambience, crossfeed, center_focus, bass_warmth_db, norm_name) =
            match preset {
                "studio_nearfield" => (0.85, 0.14, 0.42, 0.96, 0.6, "studio_nearfield"),
                "concert_hall" => (1.35, 0.46, 0.26, 0.84, 2.0, "concert_hall"),
                "intimate_vocal" => (0.78, 0.08, 0.48, 1.08, 0.4, "intimate_vocal"),
                "natural_crossfeed" => (0.76, 0.0, 0.50, 1.0, 0.0, "natural_crossfeed"),
                "custom" => (1.08, 0.28, 0.34, 0.90, 1.4, "custom"),
                _ => (1.08, 0.28, 0.34, 0.90, 1.4, "apple_spatial"),
            };
        Self {
            enabled,
            bypass: false,
            preset: norm_name.to_string(),
            spatial_width,
            room_ambience,
            crossfeed,
            center_focus,
            bass_warmth_db,
            target_device_id,
            capture_bus_id,
        }
    }

    pub fn sanitized(&self) -> Self {
        Self {
            enabled: self.enabled,
            bypass: self.bypass,
            preset: if self.preset.trim().is_empty() {
                "apple_spatial".to_string()
            } else {
                self.preset.trim().to_string()
            },
            spatial_width: self.spatial_width.clamp(0.0, 1.6),
            room_ambience: self.room_ambience.clamp(0.0, 0.85),
            crossfeed: self.crossfeed.clamp(0.0, 0.8),
            center_focus: self.center_focus.clamp(0.45, 1.35),
            bass_warmth_db: self.bass_warmth_db.clamp(-4.0, 6.0),
            target_device_id: self.target_device_id.trim().to_string(),
            capture_bus_id: self.capture_bus_id.trim().to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinauralStatus {
    pub active: bool,
    pub bypass: bool,
    pub config: BinauralConfig,
    pub target_device_name: String,
    pub capture_bus_name: String,
    pub sample_rate: u32,
    pub latency_ms: f32,
    pub phase_correlation: f32,
    pub spatial_width_pct: f32,
    pub can_enable: bool,
    pub message: Option<String>,
}

/// Transposed Direct Form II Biquad filter (zero heap allocations)
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biquad {
    fn reset_state(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        if y.abs() < 1e-20 {
            0.0
        } else {
            y
        }
    }

    fn set_peaking(&mut self, fc: f32, q: f32, gain_db: f32, fs: f32) {
        let fs_safe = fs.max(8_000.0);
        let f0 = fc.clamp(20.0, fs_safe * 0.46);
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0 / fs_safe;
        let (sin_w, cos_w) = w0.sin_cos();
        let alpha = sin_w / (2.0 * q.max(0.15));

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha / a;

        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }

    fn set_high_shelf(&mut self, fc: f32, slope: f32, gain_db: f32, fs: f32) {
        let fs_safe = fs.max(8_000.0);
        let f0 = fc.clamp(100.0, fs_safe * 0.46);
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0 / fs_safe;
        let (sin_w, cos_w) = w0.sin_cos();
        let s = slope.max(0.2);
        let alpha = (sin_w * 0.5) * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).max(0.01).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w - two_sqrt_a_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w - two_sqrt_a_alpha;

        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }

    fn set_low_shelf(&mut self, fc: f32, slope: f32, gain_db: f32, fs: f32) {
        let fs_safe = fs.max(8_000.0);
        let f0 = fc.clamp(25.0, fs_safe * 0.45);
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0 / fs_safe;
        let (sin_w, cos_w) = w0.sin_cos();
        let s = slope.max(0.2);
        let alpha = (sin_w * 0.5) * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).max(0.01).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w + two_sqrt_a_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w - two_sqrt_a_alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w + two_sqrt_a_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w - two_sqrt_a_alpha;

        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }

    fn set_bandpass(&mut self, fc: f32, q: f32, fs: f32) {
        let fs_safe = fs.max(8_000.0);
        let f0 = fc.clamp(60.0, fs_safe * 0.45);
        let w0 = 2.0 * PI * f0 / fs_safe;
        let (sin_w, cos_w) = w0.sin_cos();
        let alpha = sin_w / (2.0 * q.max(0.2));

        let b0 = alpha;
        let b1 = 0.0;
        let b2 = -alpha;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }
}

/// Power-of-two fractional delay line for Kemar ITD & studio early reflections
struct FracDelayLine {
    buf: Vec<f32>,
    mask: usize,
    write_idx: usize,
}

impl FracDelayLine {
    fn new(capacity_pow2: usize) -> Self {
        let cap = capacity_pow2.next_power_of_two().max(64);
        Self {
            buf: vec![0.0; cap],
            mask: cap - 1,
            write_idx: 0,
        }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.write_idx] = x;
        self.write_idx = (self.write_idx + 1) & self.mask;
    }

    #[inline]
    fn read_frac(&self, delay_samples: f32) -> f32 {
        let max_d = (self.mask - 2) as f32;
        let d = delay_samples.clamp(0.0, max_d);
        let d_int = d.floor() as usize;
        let frac = d - (d_int as f32);
        let cap = self.mask + 1;
        // Latest written sample is at write_idx - 1
        let idx0 = (self.write_idx + cap - 1 - d_int) & self.mask;
        let idx1 = (self.write_idx + cap - 2 - d_int) & self.mask;
        self.buf[idx0] * (1.0 - frac) + self.buf[idx1] * frac
    }
}

/// Schroeder all-pass phase diffuser for decorrelated surround spatialization
struct AllPassDiffuser {
    buf: Vec<f32>,
    idx: usize,
    g: f32,
}

impl AllPassDiffuser {
    fn new(delay_samples: usize, g: f32) -> Self {
        Self {
            buf: vec![0.0; delay_samples.max(4)],
            idx: 0,
            g,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.idx];
        let v = x + self.g * delayed;
        let y = delayed - self.g * v;
        self.buf[self.idx] = if v.abs() < 1e-20 { 0.0 } else { v };
        self.idx += 1;
        if self.idx >= self.buf.len() {
            self.idx = 0;
        }
        y
    }
}

/// Studio-grade Apple-Renderer-style Binaural Spatial Audio DSP Engine
/// Implements:
/// 1. 5-channel Virtual Spatial Matrix (0° Center, ±32° Front, ±112° Decorrelated Surround)
/// 2. Kemar/Woodworth Spherical-Head ITD Fractional Delays + Contralateral Head-Shadow & Pinna Cues
/// 3. 6-Tap Lateral Early-Reflection Studio Virtualizer for Out-of-Head Externalization
/// 4. Smooth Click-Free A/B Bypass Crossfade & Loudness-Matched True-Peak Soft Limiter
pub struct BinauralEngine {
    fs: f32,
    cfg: BinauralConfig,
    // Smoothed live parameters (zipper-noise free)
    sm_wet_mix: f32,
    sm_width: f32,
    sm_room: f32,
    sm_xfeed: f32,
    sm_center: f32,
    last_warmth_db: f32,

    // ITD & reflection delay lines
    itd_front_l: FracDelayLine,
    itd_front_r: FracDelayLine,
    surr_line_l: FracDelayLine,
    surr_line_r: FracDelayLine,
    room_line_l: FracDelayLine,
    room_line_r: FracDelayLine,

    // HRTF & acoustic filters
    center_concha: Biquad,
    shadow_front_l2r: Biquad,
    shadow_front_r2l: Biquad,
    surr_bp: Biquad,
    pinna_rear_l: Biquad,
    pinna_rear_r: Biquad,
    shadow_rear_l2r: Biquad,
    shadow_rear_r2l: Biquad,
    room_damp_l: f32,
    room_damp_r: f32,
    room_fb_l: f32,
    room_fb_r: f32,
    diff_l1: AllPassDiffuser,
    diff_r1: AllPassDiffuser,
    warmth_l: Biquad,
    warmth_r: Biquad,
}

impl BinauralEngine {
    pub fn new(sample_rate: u32, cfg: BinauralConfig) -> Self {
        let fs = (sample_rate.max(8_000)) as f32;
        let delay_cap = ((fs * 0.12) as usize).next_power_of_two().max(2048);
        let clean = cfg.sanitized();

        let mut center_concha = Biquad::default();
        // Subtle 2.55 kHz frontal concha resonance helps project vocals forward out of the head
        center_concha.set_peaking(2550.0, 1.35, 1.3, fs);

        let mut shadow_front_l2r = Biquad::default();
        let mut shadow_front_r2l = Biquad::default();
        // Kemar ±32° contralateral head-shadow shelf (-4.2 dB above 1.85 kHz)
        shadow_front_l2r.set_high_shelf(1850.0, 0.75, -4.2, fs);
        shadow_front_r2l.set_high_shelf(1850.0, 0.75, -4.2, fs);

        let mut surr_bp = Biquad::default();
        // Surround bed focused on spatial 250 Hz - 6.8 kHz band (keeps sub-bass tight & mono-centered)
        surr_bp.set_bandpass(1350.0, 0.42, fs);

        let mut pinna_rear_l = Biquad::default();
        let mut pinna_rear_r = Biquad::default();
        // ±112° rear/side pinna spectral notch @ 7.6 kHz gives rear-hemisphere depth cue
        pinna_rear_l.set_peaking(7600.0, 2.2, -3.4, fs);
        pinna_rear_r.set_peaking(7600.0, 2.2, -3.4, fs);

        let mut shadow_rear_l2r = Biquad::default();
        let mut shadow_rear_r2l = Biquad::default();
        shadow_rear_l2r.set_high_shelf(1350.0, 0.7, -6.8, fs);
        shadow_rear_r2l.set_high_shelf(1350.0, 0.7, -6.8, fs);

        let mut warmth_l = Biquad::default();
        let mut warmth_r = Biquad::default();
        warmth_l.set_low_shelf(95.0, 0.75, clean.bass_warmth_db, fs);
        warmth_r.set_low_shelf(95.0, 0.75, clean.bass_warmth_db, fs);

        let d_l = ((0.0031 * fs) as usize).max(8);
        let d_r = ((0.0047 * fs) as usize).max(11);

        let initial_wet = if clean.bypass { 0.0 } else { 1.0 };

        Self {
            fs,
            sm_wet_mix: initial_wet,
            sm_width: clean.spatial_width,
            sm_room: clean.room_ambience,
            sm_xfeed: clean.crossfeed,
            sm_center: clean.center_focus,
            last_warmth_db: clean.bass_warmth_db,
            cfg: clean,
            itd_front_l: FracDelayLine::new(512),
            itd_front_r: FracDelayLine::new(512),
            surr_line_l: FracDelayLine::new(delay_cap),
            surr_line_r: FracDelayLine::new(delay_cap),
            room_line_l: FracDelayLine::new(delay_cap),
            room_line_r: FracDelayLine::new(delay_cap),
            center_concha,
            shadow_front_l2r,
            shadow_front_r2l,
            surr_bp,
            pinna_rear_l,
            pinna_rear_r,
            shadow_rear_l2r,
            shadow_rear_r2l,
            room_damp_l: 0.0,
            room_damp_r: 0.0,
            room_fb_l: 0.0,
            room_fb_r: 0.0,
            diff_l1: AllPassDiffuser::new(d_l, 0.52),
            diff_r1: AllPassDiffuser::new(d_r, -0.52),
            warmth_l,
            warmth_r,
        }
    }

    pub fn update_config(&mut self, cfg: BinauralConfig) {
        let clean = cfg.sanitized();
        if (clean.bass_warmth_db - self.last_warmth_db).abs() > 0.02 {
            self.warmth_l
                .set_low_shelf(95.0, 0.75, clean.bass_warmth_db, self.fs);
            self.warmth_r
                .set_low_shelf(95.0, 0.75, clean.bass_warmth_db, self.fs);
            self.last_warmth_db = clean.bass_warmth_db;
        }
        self.cfg = clean;
    }

    /// Processes stereo frames in-place and returns `(phase_correlation, spatial_width_pct)`.
    pub fn process_stereo_block(&mut self, frames: &mut [[f32; 2]]) -> (f32, f32) {
        let fs = self.fs;
        // Woodworth Kemar spherical-head ITD delays in samples:
        // Front ±32° -> ~0.255 ms; Surround ±112° -> ~0.630 ms
        let d_itd_front = 0.000_255 * fs;
        let d_surr_haas_l = 0.011_3 * fs;
        let d_surr_haas_r = 0.014_9 * fs;
        let d_surr_contra = 0.000_630 * fs;

        // 6-tap Apple-studio control-room early lateral reflections (7ms .. 39ms)
        let r_t1 = 0.007_1 * fs;
        let r_t2 = 0.012_4 * fs;
        let r_t3 = 0.018_7 * fs;
        let r_t4 = 0.024_9 * fs;
        let r_t5 = 0.031_8 * fs;
        let r_t6 = 0.039_2 * fs;

        let target_wet = if self.cfg.bypass { 0.0 } else { 1.0 };
        let target_width = self.cfg.spatial_width;
        let target_room = self.cfg.room_ambience;
        let target_xfeed = self.cfg.crossfeed;
        let target_center = self.cfg.center_focus;

        // Smooth 1-pole coefficient (~6 ms time constant)
        let smooth_a = (1.0 - (-1.0 / (0.006 * fs)).exp()).clamp(0.0005, 0.02);
        // 4.2 kHz one-pole air absorption in room reflections
        let damp_a = (1.0 - (-2.0 * PI * 4200.0 / fs).exp()).clamp(0.05, 0.85);

        let mut sum_l2 = 0.0f32;
        let mut sum_r2 = 0.0f32;
        let mut sum_lr = 0.0f32;
        let mut sum_m2 = 0.0f32;
        let mut sum_s2 = 0.0f32;

        for frame in frames.iter_mut() {
            let dry_l = frame[0];
            let dry_r = frame[1];

            self.sm_wet_mix += smooth_a * (target_wet - self.sm_wet_mix);
            self.sm_width += smooth_a * (target_width - self.sm_width);
            self.sm_room += smooth_a * (target_room - self.sm_room);
            self.sm_xfeed += smooth_a * (target_xfeed - self.sm_xfeed);
            self.sm_center += smooth_a * (target_center - self.sm_center);

            // 1. Mid / Side decomposition
            let mid = 0.5 * (dry_l + dry_r);
            let side = 0.5 * (dry_l - dry_r);

            // 2. Frontal 0° Vocal Center + ±32° Direct Stereo Pair
            let center_proj = self.center_concha.process(mid) * self.sm_center;
            let front_l = center_proj + side * (0.82 + 0.28 * self.sm_width);
            let front_r = center_proj - side * (0.82 + 0.28 * self.sm_width);

            self.itd_front_l.push(front_l);
            self.itd_front_r.push(front_r);

            let xfeed_from_l = self
                .shadow_front_l2r
                .process(self.itd_front_l.read_frac(d_itd_front));
            let xfeed_from_r = self
                .shadow_front_r2l
                .process(self.itd_front_r.read_frac(d_itd_front));

            let stage_l = front_l + self.sm_xfeed * 0.48 * xfeed_from_r;
            let stage_r = front_r + self.sm_xfeed * 0.48 * xfeed_from_l;

            // 3. ±112° Decorrelated Surround Bed
            let surr_src = self.surr_bp.process(side);
            let diff_l = self.diff_l1.process(surr_src);
            let diff_r = self.diff_r1.process(-surr_src);
            self.surr_line_l.push(diff_l);
            self.surr_line_r.push(diff_r);

            let sl_ipsi = self
                .pinna_rear_l
                .process(self.surr_line_l.read_frac(d_surr_haas_l));
            let sr_ipsi = self
                .pinna_rear_r
                .process(self.surr_line_r.read_frac(d_surr_haas_r));
            let sl_contra = self
                .shadow_rear_l2r
                .process(self.surr_line_l.read_frac(d_surr_haas_l + d_surr_contra));
            let sr_contra = self
                .shadow_rear_r2l
                .process(self.surr_line_r.read_frac(d_surr_haas_r + d_surr_contra));

            let surr_gain = self.sm_width * 0.34;
            let bed_l = stage_l + surr_gain * (sl_ipsi + 0.36 * sr_contra);
            let bed_r = stage_r + surr_gain * (sr_ipsi + 0.36 * sl_contra);

            // 4. 6-Tap Lateral Early-Reflection Room Virtualizer (Out-of-Head Externalizer)
            self.room_line_l.push(front_l + 0.24 * self.room_fb_r);
            self.room_line_r.push(front_r + 0.24 * self.room_fb_l);

            let er_l = 0.36 * self.room_line_r.read_frac(r_t1)
                - 0.27 * self.room_line_l.read_frac(r_t2)
                + 0.22 * self.room_line_r.read_frac(r_t3)
                + 0.16 * self.room_line_l.read_frac(r_t5);
            let er_r = 0.36 * self.room_line_l.read_frac(r_t2 * 0.91)
                - 0.27 * self.room_line_r.read_frac(r_t3 * 0.94)
                + 0.22 * self.room_line_l.read_frac(r_t4)
                + 0.16 * self.room_line_r.read_frac(r_t6);

            self.room_damp_l += damp_a * (er_l - self.room_damp_l);
            self.room_damp_r += damp_a * (er_r - self.room_damp_r);
            self.room_fb_l = self.room_damp_l;
            self.room_fb_r = self.room_damp_r;

            let room_gain = self.sm_room * 0.44;
            let raw_wet_l = self.warmth_l.process(bed_l + room_gain * self.room_damp_l);
            let raw_wet_r = self.warmth_r.process(bed_r + room_gain * self.room_damp_r);

            // Loudness-matching normalization so A/B bypass keeps perceived loudness consistent
            let norm = 1.0
                / (1.0 + 0.20 * self.sm_xfeed + 0.14 * self.sm_width + 0.12 * self.sm_room);
            let wet_l = soft_limit(raw_wet_l * norm);
            let wet_r = soft_limit(raw_wet_r * norm);

            let out_l = dry_l * (1.0 - self.sm_wet_mix) + wet_l * self.sm_wet_mix;
            let out_r = dry_r * (1.0 - self.sm_wet_mix) + wet_r * self.sm_wet_mix;

            frame[0] = out_l;
            frame[1] = out_r;

            sum_l2 += out_l * out_l;
            sum_r2 += out_r * out_r;
            sum_lr += out_l * out_r;
            let om = 0.5 * (out_l + out_r);
            let os = 0.5 * (out_l - out_r);
            sum_m2 += om * om;
            sum_s2 += os * os;
        }

        let denom = (sum_l2 * sum_r2).sqrt();
        let corr = if denom > 1e-9 {
            (sum_lr / denom).clamp(-1.0, 1.0)
        } else {
            1.0
        };
        let ms_tot = sum_m2 + sum_s2;
        let width_pct = if ms_tot > 1e-9 {
            ((sum_s2 / ms_tot).sqrt() * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        (corr, width_pct)
    }

    pub fn reset_states(&mut self) {
        self.center_concha.reset_state();
        self.shadow_front_l2r.reset_state();
        self.shadow_front_r2l.reset_state();
        self.surr_bp.reset_state();
        self.pinna_rear_l.reset_state();
        self.pinna_rear_r.reset_state();
        self.shadow_rear_l2r.reset_state();
        self.shadow_rear_r2l.reset_state();
        self.warmth_l.reset_state();
        self.warmth_r.reset_state();
        self.room_damp_l = 0.0;
        self.room_damp_r = 0.0;
        self.room_fb_l = 0.0;
        self.room_fb_r = 0.0;
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    let ax = x.abs();
    if ax <= 0.88 {
        x
    } else {
        let excess = ax - 0.88;
        let compressed = 0.88 + 0.115 * (excess / 0.115).tanh();
        compressed.copysign(x)
    }
}

/// 4-point Hermite cubic interpolation for smooth clock-drift-compensated resampling
#[inline]
pub fn hermite_interp(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let c0 = p1;
    let c1 = 0.5 * (p2 - p0);
    let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);
    ((c3 * t + c2) * t + c1) * t + c0
}

static SHARED_CONFIG: Lazy<RwLock<BinauralConfig>> =
    Lazy::new(|| RwLock::new(BinauralConfig::default()));
static SHARED_STATUS: Lazy<RwLock<BinauralStatus>> = Lazy::new(|| {
    RwLock::new(BinauralStatus {
        active: false,
        bypass: false,
        config: BinauralConfig::default(),
        target_device_name: String::new(),
        capture_bus_name: String::new(),
        sample_rate: 48_000,
        latency_ms: 0.0,
        phase_correlation: 1.0,
        spatial_width_pct: 0.0,
        can_enable: true,
        message: None,
    })
});
static CONFIG_GENERATION: AtomicU64 = AtomicU64::new(1);
static BRIDGE_STOP_FLAG: AtomicBool = AtomicBool::new(false);
static BINAURAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static BINAURAL_BYPASS: AtomicBool = AtomicBool::new(false);
static BINAURAL_LATENCY_US: AtomicU32 = AtomicU32::new(0);
static CACHED_CAN_ENABLE: AtomicBool = AtomicBool::new(true);

/// Lock-free, zero-COM per-frame query for Binaural bridge acoustic latency in seconds.
/// Returns `0.0` when inactive or bypassed, or the measured ring + WASAPI render latency (~0.034s) when active.
#[inline]
pub fn fast_binaural_latency_sec() -> f64 {
    if !BINAURAL_ACTIVE.load(Ordering::Relaxed) || BINAURAL_BYPASS.load(Ordering::Relaxed) {
        return 0.0;
    }
    let us = BINAURAL_LATENCY_US.load(Ordering::Relaxed);
    if us == 0 {
        0.034
    } else {
        (us as f64) * 1.0e-6
    }
}

/// Explicitly refreshes cached multi-endpoint capability off the render hot-path.
pub fn refresh_binaural_can_enable() -> bool {
    let can_enable = crate::audio_viz::list_output_devices()
        .map(|d| d.len() >= 2)
        .unwrap_or(false);
    CACHED_CAN_ENABLE.store(can_enable, Ordering::Release);
    if let Ok(mut st) = SHARED_STATUS.write() {
        st.can_enable = can_enable;
    }
    can_enable
}

pub fn restore_marker_path_in_base(local_app_data: &std::path::Path) -> std::path::PathBuf {
    local_app_data
        .join("AMLight")
        .join("binaural_restore_device.txt")
}

pub fn legacy_restore_marker_path_in_base(local_app_data: &std::path::Path) -> std::path::PathBuf {
    local_app_data
        .join("com.local.amlight")
        .join("binaural_restore_device.txt")
}

pub fn save_restore_marker_in_base(local_app_data: &std::path::Path, orig_device_id: &str) {
    let trimmed = orig_device_id.trim();
    if trimmed.is_empty() {
        return;
    }
    let p = restore_marker_path_in_base(local_app_data);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p, trimmed.as_bytes());
    let legacy = legacy_restore_marker_path_in_base(local_app_data);
    if legacy.exists() {
        let _ = std::fs::remove_file(&legacy);
    }
}

/// Reads and consumes any stale Binaural restore marker from the canonical `%LOCALAPPDATA%\AMLight\`
/// path first, falling back to legacy `%LOCALAPPDATA%\com.local.amlight\` for seamless migration.
pub fn pop_restore_marker_in_base(local_app_data: &std::path::Path) -> Option<String> {
    let primary = restore_marker_path_in_base(local_app_data);
    let legacy = legacy_restore_marker_path_in_base(local_app_data);
    let mut restored: Option<String> = None;
    for path in [&primary, &legacy] {
        if let Ok(saved_id) = std::fs::read_to_string(path) {
            let trimmed = saved_id.trim();
            if restored.is_none() && !trimmed.is_empty() {
                restored = Some(trimmed.to_string());
            }
            let _ = std::fs::remove_file(path);
        }
    }
    restored
}

fn local_app_data_root() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
}

fn save_restore_marker(orig_device_id: &str) {
    if let Some(base) = local_app_data_root() {
        save_restore_marker_in_base(&base, orig_device_id);
    }
}

fn clear_restore_marker() {
    if let Some(base) = local_app_data_root() {
        let _ = std::fs::remove_file(restore_marker_path_in_base(&base));
        let _ = std::fs::remove_file(legacy_restore_marker_path_in_base(&base));
    }
}

/// Restores the user's physical audio device if a previous session terminated while Binaural routing was active.
pub fn restore_stale_routing_on_startup() {
    let Some(base) = local_app_data_root() else {
        return;
    };
    if let Some(saved_id) = pop_restore_marker_in_base(&base) {
        let _ = crate::audio_viz::set_output_device(&saved_id);
    }
}

fn is_likely_auxiliary_bus(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("pico")
        || lower.contains("virtual")
        || lower.contains("vb-audio")
        || lower.contains("cable")
        || lower.contains("steam")
        || lower.contains("nvidia")
        || lower.contains("amd high definition")
        || lower.contains("hdmi")
        || lower.contains("spdif")
        || lower.contains("digital output")
}

fn is_likely_headphone(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.contains("耳机")
        || lower.contains("headphone")
        || lower.contains("headset")
        || lower.contains("airpods")
        || lower.contains("earphone")
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WasapiGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub const WASAPI_SUBTYPE_IEEE_FLOAT: WasapiGuid = WasapiGuid {
    data1: 0x0000_0003,
    data2: 0x0000,
    data3: 0x0010,
    data4: [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
};

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct PackedWaveFormatEx {
    pub w_format_tag: u16,
    pub n_channels: u16,
    pub n_samples_per_sec: u32,
    pub n_avg_bytes_per_sec: u32,
    pub n_block_align: u16,
    pub w_bits_per_sample: u16,
    pub cb_size: u16,
}

#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct PackedWaveFormatExtensible {
    pub format: PackedWaveFormatEx,
    pub w_valid_bits_per_sample: u16,
    pub dw_channel_mask: u32,
    pub sub_format: WasapiGuid,
}

const _: [(); 18] = [(); std::mem::size_of::<PackedWaveFormatEx>()];
const _: [(); 40] = [(); std::mem::size_of::<PackedWaveFormatExtensible>()];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedWasapiWaveFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub valid_bits_per_sample: u16,
    pub bytes_per_sample: usize,
    pub block_align: usize,
    pub channel_mask: u32,
    pub is_float: bool,
}

/// Parses a raw WASAPI `WAVEFORMATEX` / `WAVEFORMATEXTENSIBLE` buffer using unaligned packed reads.
pub fn parse_wasapi_waveformat_bytes(raw: &[u8]) -> Option<ParsedWasapiWaveFormat> {
    if raw.len() < std::mem::size_of::<PackedWaveFormatEx>() {
        return None;
    }
    let wf = unsafe { std::ptr::read_unaligned(raw.as_ptr() as *const PackedWaveFormatEx) };
    let sample_rate = wf.n_samples_per_sec;
    let channels = wf.n_channels;
    if sample_rate == 0 || channels == 0 {
        return None;
    }
    let ch_usize = channels as usize;
    let bits_per_sample = wf.w_bits_per_sample;
    let bps_bits = (bits_per_sample as usize) / 8;
    let bps_align = if wf.n_block_align > 0 && (wf.n_block_align as usize) % ch_usize == 0 {
        (wf.n_block_align as usize) / ch_usize
    } else {
        0
    };
    let bytes_per_sample = if (1..=8).contains(&bps_align) {
        bps_align
    } else {
        bps_bits.clamp(1, 8).max(2)
    };
    let block_align = if (wf.n_block_align as usize) >= ch_usize * bytes_per_sample {
        wf.n_block_align as usize
    } else {
        ch_usize * bytes_per_sample
    };

    let mut valid_bits = bits_per_sample;
    let mut channel_mask = 0u32;
    let mut is_float = wf.w_format_tag == 3;
    if wf.w_format_tag == 0xFFFE {
        if wf.cb_size < 22 || raw.len() < std::mem::size_of::<PackedWaveFormatExtensible>() {
            return None;
        }
        let ext =
            unsafe { std::ptr::read_unaligned(raw.as_ptr() as *const PackedWaveFormatExtensible) };
        if ext.w_valid_bits_per_sample > 0 {
            valid_bits = ext.w_valid_bits_per_sample;
        }
        channel_mask = ext.dw_channel_mask;
        let sub = ext.sub_format;
        is_float = sub == WASAPI_SUBTYPE_IEEE_FLOAT;
    }
    Some(ParsedWasapiWaveFormat {
        sample_rate,
        channels,
        bits_per_sample,
        valid_bits_per_sample: valid_bits,
        bytes_per_sample,
        block_align,
        channel_mask,
        is_float,
    })
}

#[inline]
fn read_wasapi_channel_sample(frame: &[u8], ch_idx: usize, fmt: &ParsedWasapiWaveFormat) -> f32 {
    let bps = fmt.bytes_per_sample;
    let off = ch_idx * bps;
    if off + bps > frame.len() {
        return 0.0;
    }
    unsafe {
        let p = frame.as_ptr().add(off);
        if fmt.is_float && bps == 4 {
            let v = std::ptr::read_unaligned(p as *const f32);
            if v.is_finite() { v } else { 0.0 }
        } else if fmt.is_float && bps == 8 {
            let v = std::ptr::read_unaligned(p as *const f64) as f32;
            if v.is_finite() { v } else { 0.0 }
        } else if !fmt.is_float && bps == 2 {
            (std::ptr::read_unaligned(p as *const i16) as f32) / 32_768.0
        } else if !fmt.is_float && bps == 3 {
            let b0 = *p as i32;
            let b1 = (*p.add(1) as i32) << 8;
            let b2 = (*p.add(2) as i8 as i32) << 16;
            ((b0 | b1 | b2) as f32) / 8_388_608.0
        } else if !fmt.is_float && bps == 4 {
            (std::ptr::read_unaligned(p as *const i32) as f32) / 2_147_483_648.0
        } else if !fmt.is_float && bps == 1 {
            ((*p as f32) - 128.0) / 128.0
        } else {
            0.0
        }
    }
}

/// Decodes a single interleaved WASAPI frame (`1..=N` channels, float32/64 or 16/24/24-in-32/32-bit PCM)
/// into stereo `[L, R]`. Preserves bit-identical `[L, R]` on stereo streams (or silent surround channels)
/// while applying ITU-R BS.775 downmix when 5.1/7.1 Center/Surround/LFE channels carry signal.
#[inline]
pub fn decode_wasapi_frame_to_stereo(frame: &[u8], fmt: &ParsedWasapiWaveFormat) -> [f32; 2] {
    let ch = fmt.channels as usize;
    if ch == 0 || frame.len() < ch * fmt.bytes_per_sample {
        return [0.0, 0.0];
    }
    let c0 = read_wasapi_channel_sample(frame, 0, fmt);
    if ch == 1 {
        return [c0, c0];
    }
    let c1 = read_wasapi_channel_sample(frame, 1, fmt);
    if ch == 2 {
        return [c0, c1];
    }
    const K_SURR: f32 = 0.707_106_77;
    const K_LFE: f32 = 0.316_227_76;
    if ch == 3 {
        let fc = read_wasapi_channel_sample(frame, 2, fmt);
        return if fc == 0.0 {
            [c0, c1]
        } else {
            [c0 + K_SURR * fc, c1 + K_SURR * fc]
        };
    }
    if ch == 4 {
        let bl = read_wasapi_channel_sample(frame, 2, fmt);
        let br = read_wasapi_channel_sample(frame, 3, fmt);
        return [
            if bl == 0.0 { c0 } else { c0 + K_SURR * bl },
            if br == 0.0 { c1 } else { c1 + K_SURR * br },
        ];
    }
    // 5.0 / 5.1 / 7.1 SMPTE speaker order: 0=FL, 1=FR, 2=FC, 3=LFE, 4=BL, 5=BR, 6=SL, 7=SR
    let fc = read_wasapi_channel_sample(frame, 2, fmt);
    let lfe = read_wasapi_channel_sample(frame, 3, fmt);
    let bl = read_wasapi_channel_sample(frame, 4, fmt);
    let br = if ch >= 6 {
        read_wasapi_channel_sample(frame, 5, fmt)
    } else {
        0.0
    };
    let sl = if ch >= 7 {
        read_wasapi_channel_sample(frame, 6, fmt)
    } else {
        0.0
    };
    let sr = if ch >= 8 {
        read_wasapi_channel_sample(frame, 7, fmt)
    } else {
        0.0
    };
    let add_l = K_SURR * (fc + bl + sl) + K_LFE * lfe;
    let add_r = K_SURR * (fc + br + sr) + K_LFE * lfe;
    [
        if add_l == 0.0 { c0 } else { c0 + add_l },
        if add_r == 0.0 { c1 } else { c1 + add_r },
    ]
}

/// Encodes a stereo `[L, R]` frame into a single WASAPI output frame buffer using unaligned writes.
#[inline]
pub fn encode_stereo_frame_to_wasapi(
    lr: [f32; 2],
    fmt: &ParsedWasapiWaveFormat,
    dst_frame: &mut [u8],
) {
    let ch = fmt.channels as usize;
    let bps = fmt.bytes_per_sample;
    if ch == 0 || dst_frame.len() < ch * bps {
        return;
    }
    dst_frame.fill(0);
    let cl = if lr[0].is_finite() {
        lr[0].clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let cr = if lr[1].is_finite() {
        lr[1].clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let out_l = if ch == 1 { 0.5 * (cl + cr) } else { cl };
    unsafe {
        let base = dst_frame.as_mut_ptr();
        if fmt.is_float && bps == 4 {
            std::ptr::write_unaligned(base as *mut f32, out_l);
            if ch > 1 {
                std::ptr::write_unaligned((base as *mut f32).add(1), cr);
            }
        } else if fmt.is_float && bps == 8 {
            std::ptr::write_unaligned(base as *mut f64, out_l as f64);
            if ch > 1 {
                std::ptr::write_unaligned((base as *mut f64).add(1), cr as f64);
            }
        } else if !fmt.is_float && bps == 2 {
            std::ptr::write_unaligned(base as *mut i16, (out_l * 32_767.0).round() as i16);
            if ch > 1 {
                std::ptr::write_unaligned((base as *mut i16).add(1), (cr * 32_767.0).round() as i16);
            }
        } else if !fmt.is_float && bps == 3 {
            let write_i24 = |p: *mut u8, v: f32| {
                let s = (v * 8_388_607.0).round() as i32;
                *p = (s & 0xFF) as u8;
                *p.add(1) = ((s >> 8) & 0xFF) as u8;
                *p.add(2) = ((s >> 16) & 0xFF) as u8;
            };
            write_i24(base, out_l);
            if ch > 1 {
                write_i24(base.add(3), cr);
            }
        } else if !fmt.is_float && bps == 4 {
            let mut sl = (out_l * 2_147_483_000.0) as i32;
            let mut sr = (cr * 2_147_483_000.0) as i32;
            if fmt.valid_bits_per_sample == 24 {
                sl &= !0xFF;
                sr &= !0xFF;
            }
            std::ptr::write_unaligned(base as *mut i32, sl);
            if ch > 1 {
                std::ptr::write_unaligned((base as *mut i32).add(1), sr);
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    type HRESULT = i32;
    type ULONG = u32;
    type GUID = WasapiGuid;
    type WAVEFORMATEX = PackedWaveFormatEx;

    const CLSID_MMDEVICE_ENUMERATOR: GUID = GUID {
        data1: 0xBCDE_0395,
        data2: 0xE52F,
        data3: 0x467C,
        data4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
    };
    const IID_IMMDEVICE_ENUMERATOR: GUID = GUID {
        data1: 0xA956_64D2,
        data2: 0x9614,
        data3: 0x4F35,
        data4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
    };
    const IID_IAUDIO_CLIENT: GUID = GUID {
        data1: 0x1CB9_AD4C,
        data2: 0xDBFA,
        data3: 0x4C32,
        data4: [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2],
    };
    const IID_IAUDIO_CAPTURE_CLIENT: GUID = GUID {
        data1: 0xC8AD_BD64,
        data2: 0xE71E,
        data3: 0x48A0,
        data4: [0xA4, 0xDE, 0x18, 0x5C, 0x39, 0x5C, 0xD3, 0x17],
    };
    const IID_IAUDIO_RENDER_CLIENT: GUID = GUID {
        data1: 0xF294_ACFC,
        data2: 0x3146,
        data3: 0x4483,
        data4: [0xA7, 0xBF, 0xAD, 0xDC, 0xA7, 0xC2, 0x60, 0xE2],
    };

    unsafe fn parse_mix_format_ptr(fmt_ptr: *const WAVEFORMATEX) -> ParsedWasapiWaveFormat {
        let wf = std::ptr::read_unaligned(fmt_ptr);
        let byte_len = if wf.w_format_tag == 0xFFFE && wf.cb_size >= 22 {
            std::mem::size_of::<PackedWaveFormatExtensible>()
        } else {
            std::mem::size_of::<PackedWaveFormatEx>()
        };
        let raw = std::slice::from_raw_parts(fmt_ptr as *const u8, byte_len);
        parse_wasapi_waveformat_bytes(raw).unwrap_or(ParsedWasapiWaveFormat {
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
            valid_bits_per_sample: 32,
            bytes_per_sample: 4,
            block_align: 8,
            channel_mask: 0x3,
            is_float: true,
        })
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_coinit: u32) -> HRESULT;
        fn CoUninitialize();
        fn CoCreateInstance(
            rclsid: *const GUID,
            p_unk_outer: *mut c_void,
            dw_cls_context: u32,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> HRESULT;
        fn CoTaskMemFree(pv: *mut c_void);
    }

    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    unsafe fn release_com(unk: *mut c_void) {
        if !unk.is_null() {
            let vtbl = *(unk as *const *const *const c_void);
            let rel: unsafe extern "system" fn(*mut c_void) -> ULONG =
                std::mem::transmute(*(vtbl.add(2)));
            let _ = rel(unk);
        }
    }

    struct WasapiCaptureEndpoint {
        sample_rate: u32,
        fmt: ParsedWasapiWaveFormat,
        audio_client: *mut c_void,
        capture_client: *mut c_void,
    }

    struct WasapiRenderEndpoint {
        sample_rate: u32,
        fmt: ParsedWasapiWaveFormat,
        buffer_frames: u32,
        audio_client: *mut c_void,
        render_client: *mut c_void,
    }

    unsafe fn open_mmdevice_by_id(device_id: &str) -> Result<*mut c_void, String> {
        let mut enumerator: *mut c_void = null_mut();
        let hr = CoCreateInstance(
            &CLSID_MMDEVICE_ENUMERATOR,
            null_mut(),
            0x17,
            &IID_IMMDEVICE_ENUMERATOR,
            &mut enumerator,
        );
        if hr < 0 || enumerator.is_null() {
            return Err(format!("创建音频枚举器失败: 0x{:08X}", hr as u32));
        }
        let e_vtbl = *(enumerator as *const *const *const c_void);
        let get_device: unsafe extern "system" fn(*mut c_void, *const u16, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(*(e_vtbl.add(5)));
        let mut wide: Vec<u16> = device_id.encode_utf16().collect();
        wide.push(0);
        let mut dev: *mut c_void = null_mut();
        let hr_dev = get_device(enumerator, wide.as_ptr(), &mut dev);
        release_com(enumerator);
        if hr_dev < 0 || dev.is_null() {
            return Err(format!("打开指定音频端点失败: 0x{:08X}", hr_dev as u32));
        }
        Ok(dev)
    }

    impl WasapiCaptureEndpoint {
        unsafe fn open_loopback(device_id: &str) -> Result<Self, String> {
            let dev = open_mmdevice_by_id(device_id)?;
            let d_vtbl = *(dev as *const *const *const c_void);
            let activate: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                u32,
                *mut c_void,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(d_vtbl.add(3)));

            let mut audio_client: *mut c_void = null_mut();
            let hr = activate(dev, &IID_IAUDIO_CLIENT, 0x17, null_mut(), &mut audio_client);
            release_com(dev);
            if hr < 0 || audio_client.is_null() {
                return Err(format!("激活捕获总线 IAudioClient 失败: 0x{:08X}", hr as u32));
            }

            let c_vtbl = *(audio_client as *const *const *const c_void);
            let get_mix: unsafe extern "system" fn(*mut c_void, *mut *mut WAVEFORMATEX) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(8)));
            let init_fn: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                i64,
                i64,
                *const WAVEFORMATEX,
                *const GUID,
            ) -> HRESULT = std::mem::transmute(*(c_vtbl.add(3)));
            let start_fn: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(10)));
            let get_service: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(c_vtbl.add(14)));

            let mut fmt_ptr: *mut WAVEFORMATEX = null_mut();
            if get_mix(audio_client, &mut fmt_ptr) < 0 || fmt_ptr.is_null() {
                release_com(audio_client);
                return Err("读取捕获总线混音格式失败".to_string());
            }

            let parsed_fmt = parse_mix_format_ptr(fmt_ptr);
            let sample_rate = parsed_fmt.sample_rate;

            // 28ms low-latency loopback buffer (AUDCLNT_STREAMFLAGS_LOOPBACK = 0x00020000)
            let hr_init = init_fn(
                audio_client,
                0,
                0x0002_0000,
                280_000,
                0,
                fmt_ptr,
                null(),
            );
            CoTaskMemFree(fmt_ptr as *mut c_void);
            if hr_init < 0 {
                release_com(audio_client);
                return Err(format!("初始化捕获总线 Loopback 失败: 0x{:08X}", hr_init as u32));
            }

            let mut capture_client: *mut c_void = null_mut();
            if get_service(audio_client, &IID_IAUDIO_CAPTURE_CLIENT, &mut capture_client) < 0
                || capture_client.is_null()
            {
                release_com(audio_client);
                return Err("获取 IAudioCaptureClient 失败".to_string());
            }

            if start_fn(audio_client) < 0 {
                release_com(capture_client);
                release_com(audio_client);
                return Err("启动捕获总线流失败".to_string());
            }

            Ok(Self {
                sample_rate,
                fmt: parsed_fmt,
                audio_client,
                capture_client,
            })
        }

        unsafe fn drain_stereo(&self, ring: &mut VecDeque<[f32; 2]>, max_ring: usize) -> bool {
            let cap_vtbl = *(self.capture_client as *const *const *const c_void);
            let get_next_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*(cap_vtbl.add(5)));
            let get_buf: unsafe extern "system" fn(
                *mut c_void,
                *mut *mut u8,
                *mut u32,
                *mut u32,
                *mut u64,
                *mut u64,
            ) -> HRESULT = std::mem::transmute(*(cap_vtbl.add(3)));
            let rel_buf: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT =
                std::mem::transmute(*(cap_vtbl.add(4)));

            let mut pkt_frames: u32 = 0;
            if get_next_size(self.capture_client, &mut pkt_frames) < 0 {
                return false;
            }
            while pkt_frames > 0 {
                let mut data_ptr: *mut u8 = null_mut();
                let mut num_frames: u32 = 0;
                let mut flags: u32 = 0;
                let mut dev_pos: u64 = 0;
                let mut qpc_pos: u64 = 0;
                if get_buf(
                    self.capture_client,
                    &mut data_ptr,
                    &mut num_frames,
                    &mut flags,
                    &mut dev_pos,
                    &mut qpc_pos,
                ) < 0
                {
                    return false;
                }
                let silent = (flags & 0x2) != 0 || data_ptr.is_null();
                let stride = self.fmt.block_align.max(2);
                for f in 0..(num_frames as usize) {
                    let pair = if silent {
                        [0.0, 0.0]
                    } else {
                        let frame_slice = std::slice::from_raw_parts(data_ptr.add(f * stride), stride);
                        decode_wasapi_frame_to_stereo(frame_slice, &self.fmt)
                    };
                    if ring.len() >= max_ring {
                        ring.pop_front();
                    }
                    ring.push_back(pair);
                }
                let _ = rel_buf(self.capture_client, num_frames);
                if get_next_size(self.capture_client, &mut pkt_frames) < 0 {
                    return false;
                }
            }
            true
        }

        unsafe fn close(self) {
            if !self.audio_client.is_null() {
                let c_vtbl = *(self.audio_client as *const *const *const c_void);
                let stop_fn: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                    std::mem::transmute(*(c_vtbl.add(11)));
                let _ = stop_fn(self.audio_client);
            }
            release_com(self.capture_client);
            release_com(self.audio_client);
        }
    }

    /// Note: Keep a silent dummy render stream open on the auxiliary capture bus so Windows Audio Engine
    /// continuously clocks loopback packets even during silent passages!
    struct SilentClockKeeper {
        audio_client: *mut c_void,
        render_client: *mut c_void,
    }

    impl SilentClockKeeper {
        unsafe fn open(device_id: &str) -> Option<Self> {
            let dev = open_mmdevice_by_id(device_id).ok()?;
            let d_vtbl = *(dev as *const *const *const c_void);
            let activate: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                u32,
                *mut c_void,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(d_vtbl.add(3)));
            let mut audio_client: *mut c_void = null_mut();
            let hr = activate(dev, &IID_IAUDIO_CLIENT, 0x17, null_mut(), &mut audio_client);
            release_com(dev);
            if hr < 0 || audio_client.is_null() {
                return None;
            }

            let c_vtbl = *(audio_client as *const *const *const c_void);
            let get_mix: unsafe extern "system" fn(*mut c_void, *mut *mut WAVEFORMATEX) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(8)));
            let init_fn: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                i64,
                i64,
                *const WAVEFORMATEX,
                *const GUID,
            ) -> HRESULT = std::mem::transmute(*(c_vtbl.add(3)));
            let get_buf_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(4)));
            let start_fn: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(10)));
            let get_service: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(c_vtbl.add(14)));

            let mut fmt_ptr: *mut WAVEFORMATEX = null_mut();
            if get_mix(audio_client, &mut fmt_ptr) < 0 || fmt_ptr.is_null() {
                release_com(audio_client);
                return None;
            }
            let hr_init = init_fn(audio_client, 0, 0, 500_000, 0, fmt_ptr, null());
            CoTaskMemFree(fmt_ptr as *mut c_void);
            if hr_init < 0 {
                release_com(audio_client);
                return None;
            }
            let mut render_client: *mut c_void = null_mut();
            if get_service(audio_client, &IID_IAUDIO_RENDER_CLIENT, &mut render_client) < 0
                || render_client.is_null()
            {
                release_com(audio_client);
                return None;
            }
            let mut buf_sz: u32 = 0;
            let _ = get_buf_size(audio_client, &mut buf_sz);
            if buf_sz > 0 {
                let r_vtbl = *(render_client as *const *const *const c_void);
                let get_buf: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u8) -> HRESULT =
                    std::mem::transmute(*(r_vtbl.add(3)));
                let rel_buf: unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT =
                    std::mem::transmute(*(r_vtbl.add(4)));
                let mut p: *mut u8 = null_mut();
                if get_buf(render_client, buf_sz, &mut p) >= 0 {
                    let _ = rel_buf(render_client, buf_sz, 0x2 /* AUDCLNT_BUFFERFLAGS_SILENT */);
                }
            }
            let _ = start_fn(audio_client);
            Some(Self {
                audio_client,
                render_client,
            })
        }

        unsafe fn tick_silence(&self) {
            let c_vtbl = *(self.audio_client as *const *const *const c_void);
            let get_buf_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(4)));
            let get_pad: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(6)));
            let mut bsz: u32 = 0;
            let mut pad: u32 = 0;
            if get_buf_size(self.audio_client, &mut bsz) >= 0
                && get_pad(self.audio_client, &mut pad) >= 0
            {
                let avail = bsz.saturating_sub(pad);
                if avail > 0 {
                    let r_vtbl = *(self.render_client as *const *const *const c_void);
                    let get_buf: unsafe extern "system" fn(
                        *mut c_void,
                        u32,
                        *mut *mut u8,
                    ) -> HRESULT = std::mem::transmute(*(r_vtbl.add(3)));
                    let rel_buf: unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT =
                        std::mem::transmute(*(r_vtbl.add(4)));
                    let mut p: *mut u8 = null_mut();
                    if get_buf(self.render_client, avail, &mut p) >= 0 {
                        let _ = rel_buf(self.render_client, avail, 0x2);
                    }
                }
            }
        }

        unsafe fn close(self) {
            if !self.audio_client.is_null() {
                let c_vtbl = *(self.audio_client as *const *const *const c_void);
                let stop_fn: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                    std::mem::transmute(*(c_vtbl.add(11)));
                let _ = stop_fn(self.audio_client);
            }
            release_com(self.render_client);
            release_com(self.audio_client);
        }
    }

    impl WasapiRenderEndpoint {
        unsafe fn open_render(device_id: &str) -> Result<Self, String> {
            let dev = open_mmdevice_by_id(device_id)?;
            let d_vtbl = *(dev as *const *const *const c_void);
            let activate: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                u32,
                *mut c_void,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(d_vtbl.add(3)));

            let mut audio_client: *mut c_void = null_mut();
            let hr = activate(dev, &IID_IAUDIO_CLIENT, 0x17, null_mut(), &mut audio_client);
            release_com(dev);
            if hr < 0 || audio_client.is_null() {
                return Err(format!("激活目标监听设备 IAudioClient 失败: 0x{:08X}", hr as u32));
            }

            let c_vtbl = *(audio_client as *const *const *const c_void);
            let get_mix: unsafe extern "system" fn(*mut c_void, *mut *mut WAVEFORMATEX) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(8)));
            let init_fn: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                i64,
                i64,
                *const WAVEFORMATEX,
                *const GUID,
            ) -> HRESULT = std::mem::transmute(*(c_vtbl.add(3)));
            let get_buf_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(4)));
            let start_fn: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(10)));
            let get_service: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(c_vtbl.add(14)));

            let mut fmt_ptr: *mut WAVEFORMATEX = null_mut();
            if get_mix(audio_client, &mut fmt_ptr) < 0 || fmt_ptr.is_null() {
                release_com(audio_client);
                return Err("读取目标监听设备混音格式失败".to_string());
            }

            let parsed_fmt = parse_mix_format_ptr(fmt_ptr);
            let sample_rate = parsed_fmt.sample_rate;

            // 32ms shared render buffer
            let hr_init = init_fn(audio_client, 0, 0, 320_000, 0, fmt_ptr, null());
            CoTaskMemFree(fmt_ptr as *mut c_void);
            if hr_init < 0 {
                release_com(audio_client);
                return Err(format!("初始化目标监听设备流失败: 0x{:08X}", hr_init as u32));
            }

            let mut buffer_frames: u32 = 0;
            let _ = get_buf_size(audio_client, &mut buffer_frames);

            let mut render_client: *mut c_void = null_mut();
            if get_service(audio_client, &IID_IAUDIO_RENDER_CLIENT, &mut render_client) < 0
                || render_client.is_null()
            {
                release_com(audio_client);
                return Err("获取 IAudioRenderClient 失败".to_string());
            }

            if start_fn(audio_client) < 0 {
                release_com(render_client);
                release_com(audio_client);
                return Err("启动目标监听设备音频输出失败".to_string());
            }

            Ok(Self {
                sample_rate,
                fmt: parsed_fmt,
                buffer_frames: buffer_frames.max(256),
                audio_client,
                render_client,
            })
        }

        unsafe fn available_frames(&self) -> Option<u32> {
            let c_vtbl = *(self.audio_client as *const *const *const c_void);
            let get_pad: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(6)));
            let mut pad: u32 = 0;
            if get_pad(self.audio_client, &mut pad) < 0 {
                return None;
            }
            Some(self.buffer_frames.saturating_sub(pad))
        }

        unsafe fn write_stereo(&self, frames: &[[f32; 2]]) -> bool {
            let n = frames.len() as u32;
            if n == 0 {
                return true;
            }
            let r_vtbl = *(self.render_client as *const *const *const c_void);
            let get_buf: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u8) -> HRESULT =
                std::mem::transmute(*(r_vtbl.add(3)));
            let rel_buf: unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT =
                std::mem::transmute(*(r_vtbl.add(4)));

            let mut data_ptr: *mut u8 = null_mut();
            if get_buf(self.render_client, n, &mut data_ptr) < 0 || data_ptr.is_null() {
                return false;
            }

            let stride = self.fmt.block_align.max(2);
            for (i, &lr) in frames.iter().enumerate() {
                let dst_slice = std::slice::from_raw_parts_mut(data_ptr.add(i * stride), stride);
                encode_stereo_frame_to_wasapi(lr, &self.fmt, dst_slice);
            }

            rel_buf(self.render_client, n, 0) >= 0
        }

        unsafe fn close(self) {
            if !self.audio_client.is_null() {
                let c_vtbl = *(self.audio_client as *const *const *const c_void);
                let stop_fn: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                    std::mem::transmute(*(c_vtbl.add(11)));
                let _ = stop_fn(self.audio_client);
            }
            release_com(self.render_client);
            release_com(self.audio_client);
        }
    }

    struct ActiveBridge {
        original_default_id: String,
        target_id: String,
        capture_id: String,
        join_handle: Option<JoinHandle<()>>,
    }

    static ACTIVE_BRIDGE: Lazy<Mutex<Option<ActiveBridge>>> = Lazy::new(|| Mutex::new(None));

    fn stop_active_bridge_locked(guard: &mut Option<ActiveBridge>, restore_default: bool) {
        BINAURAL_ACTIVE.store(false, Ordering::Release);
        BINAURAL_LATENCY_US.store(0, Ordering::Release);
        if let Some(mut br) = guard.take() {
            BRIDGE_STOP_FLAG.store(true, Ordering::Release);
            if let Some(jh) = br.join_handle.take() {
                let _ = jh.join();
            }
            crate::audio_viz::set_viz_target_override(None);
            if restore_default && !br.original_default_id.is_empty() {
                let _ = crate::audio_viz::set_output_device(&br.original_default_id);
            }
            clear_restore_marker();
        }
        if let Ok(mut st) = SHARED_STATUS.write() {
            st.active = false;
            st.latency_ms = 0.0;
        }
    }

    pub fn apply_binaural_config(req: BinauralConfig) -> Result<BinauralStatus, String> {
        let mut cfg = req.sanitized();
        let mut bridge_guard = ACTIVE_BRIDGE
            .lock()
            .map_err(|_| "双耳渲染引擎锁异常".to_string())?;

        let devices = crate::audio_viz::list_output_devices()?;
        let can_enable = devices.len() >= 2;
        CACHED_CAN_ENABLE.store(can_enable, Ordering::Release);

        if !cfg.enabled {
            BINAURAL_BYPASS.store(cfg.bypass, Ordering::Release);
            stop_active_bridge_locked(&mut bridge_guard, true);
            if let Ok(mut sc) = SHARED_CONFIG.write() {
                *sc = cfg.clone();
            }
            let st = BinauralStatus {
                active: false,
                bypass: cfg.bypass,
                config: cfg,
                target_device_name: String::new(),
                capture_bus_name: String::new(),
                sample_rate: 48_000,
                latency_ms: 0.0,
                phase_correlation: 1.0,
                spatial_width_pct: 0.0,
                can_enable,
                message: Some("本地双耳空间渲染器已关闭，已恢复原物理设备直出".to_string()),
            };
            if let Ok(mut ss) = SHARED_STATUS.write() {
                *ss = st.clone();
            }
            return Ok(st);
        }

        if !can_enable {
            cfg.enabled = false;
            return Err(
                "开启本地双耳渲染需要系统至少存在 2 个可用音频输出端点（一个作为内部静音捕获总线，一个作为物理监听耳机/音箱）。"
                    .to_string(),
            );
        }

        // Determine original physical default device (if bridge is already running, preserve its saved original_default_id)
        let current_default_id = devices
            .iter()
            .find(|d| d.is_default)
            .map(|d| d.id.clone())
            .unwrap_or_else(|| devices[0].id.clone());

        let orig_default_id = bridge_guard
            .as_ref()
            .map(|b| b.original_default_id.clone())
            .unwrap_or_else(|| current_default_id.clone());

        // Auto-select target physical headphone/speaker if unset or missing
        if cfg.target_device_id.is_empty() || !devices.iter().any(|d| d.id == cfg.target_device_id)
        {
            if let Some(orig_dev) = devices.iter().find(|d| d.id == orig_default_id) {
                if !is_likely_auxiliary_bus(&orig_dev.name) {
                    cfg.target_device_id = orig_dev.id.clone();
                }
            }
            if cfg.target_device_id.is_empty() {
                cfg.target_device_id = devices
                    .iter()
                    .find(|d| is_likely_headphone(&d.name))
                    .or_else(|| devices.iter().find(|d| !is_likely_auxiliary_bus(&d.name)))
                    .unwrap_or(&devices[0])
                    .id
                    .clone();
            }
        }

        // Auto-select silent/auxiliary capture bus != target_device_id
        if cfg.capture_bus_id.is_empty()
            || cfg.capture_bus_id == cfg.target_device_id
            || !devices.iter().any(|d| d.id == cfg.capture_bus_id)
        {
            cfg.capture_bus_id = devices
                .iter()
                .filter(|d| d.id != cfg.target_device_id)
                .find(|d| is_likely_auxiliary_bus(&d.name))
                .or_else(|| devices.iter().find(|d| d.id != cfg.target_device_id))
                .ok_or_else(|| "未找到可用于内部音频捕获的辅助总线端点".to_string())?
                .id
                .clone();
        }

        let target_name = devices
            .iter()
            .find(|d| d.id == cfg.target_device_id)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let capture_name = devices
            .iter()
            .find(|d| d.id == cfg.capture_bus_id)
            .map(|d| d.name.clone())
            .unwrap_or_default();

        // Hot-update DSP parameters in-place with ZERO gap if routing endpoints haven't changed!
        let same_endpoints = bridge_guard
            .as_ref()
            .map(|b| b.target_id == cfg.target_device_id && b.capture_id == cfg.capture_bus_id)
            .unwrap_or(false);

        if let Ok(mut sc) = SHARED_CONFIG.write() {
            *sc = cfg.clone();
        }
        CONFIG_GENERATION.fetch_add(1, Ordering::AcqRel);

        if same_endpoints {
            BINAURAL_BYPASS.store(cfg.bypass, Ordering::Release);
            if let Ok(mut ss) = SHARED_STATUS.write() {
                ss.bypass = cfg.bypass;
                ss.config = cfg.clone();
                ss.message = Some(if cfg.bypass {
                    "已切入原声直通对比 (A/B Bypass)，0ms 无缝监听干声".to_string()
                } else {
                    format!("Apple 级双耳空间渲染实时生效中 → {target_name}")
                });
                return Ok(ss.clone());
            }
        }

        // Restart bridge if endpoints changed or bridge wasn't active yet
        stop_active_bridge_locked(&mut bridge_guard, false);

        // Save crash-recovery marker BEFORE switching Windows default endpoint to the auxiliary capture bus
        save_restore_marker(&orig_default_id);
        crate::audio_viz::set_output_device(&cfg.capture_bus_id)?;
        crate::audio_viz::set_viz_target_override(Some(cfg.target_device_id.clone()));

        BRIDGE_STOP_FLAG.store(false, Ordering::Release);
        let cap_id_thread = cfg.capture_bus_id.clone();
        let dst_id_thread = cfg.target_device_id.clone();
        let tgt_name_thread = target_name.clone();
        let cap_name_thread = capture_name.clone();
        let init_cfg = cfg.clone();

        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<u32, String>>(1);

        let jh = std::thread::Builder::new()
            .name("am-binaural-renderer".to_string())
            .spawn(move || unsafe {
                let _com_guard = ComGuard(CoInitializeEx(null_mut(), 0) >= 0);
                let _ = crate::am_agent::init_process_com_security();
                let keeper = SilentClockKeeper::open(&cap_id_thread);
                let cap = match WasapiCaptureEndpoint::open_loopback(&cap_id_thread) {
                    Ok(c) => c,
                    Err(e) => {
                        if let Some(k) = keeper {
                            k.close();
                        }
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let ren = match WasapiRenderEndpoint::open_render(&dst_id_thread) {
                    Ok(r) => r,
                    Err(e) => {
                        cap.close();
                        if let Some(k) = keeper {
                            k.close();
                        }
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };

                let dst_sr = ren.sample_rate.max(8_000);
                let src_sr = cap.sample_rate.max(8_000);
                BINAURAL_BYPASS.store(init_cfg.bypass, Ordering::Release);
                BINAURAL_LATENCY_US.store(34_000, Ordering::Release);
                BINAURAL_ACTIVE.store(true, Ordering::Release);
                let _ = ready_tx.send(Ok(dst_sr));

                let mut engine = BinauralEngine::new(dst_sr, init_cfg);
                let mut seen_gen = CONFIG_GENERATION.load(Ordering::Acquire);

                let target_ring_frames = ((src_sr as usize) * 18) / 1000; // ~18ms jitter buffer
                let max_ring_frames = ((src_sr as usize) * 120) / 1000;
                let mut ring: VecDeque<[f32; 2]> = VecDeque::with_capacity(max_ring_frames + 512);
                let mut phase: f64 = 0.0;
                let base_ratio = (src_sr as f64) / (dst_sr as f64);
                let mut work_buf: Vec<[f32; 2]> = Vec::with_capacity(2048);
                let mut last_stat_update = Instant::now();

                while !BRIDGE_STOP_FLAG.load(Ordering::Acquire) {
                    let g = CONFIG_GENERATION.load(Ordering::Acquire);
                    if g != seen_gen {
                        seen_gen = g;
                        if let Ok(latest) = SHARED_CONFIG.read() {
                            BINAURAL_BYPASS.store(latest.bypass, Ordering::Release);
                            engine.update_config(latest.clone());
                        }
                    }

                    if let Some(ref k) = keeper {
                        k.tick_silence();
                    }

                    if !cap.drain_stereo(&mut ring, max_ring_frames) {
                        break;
                    }

                    let Some(avail_out) = ren.available_frames() else {
                        break;
                    };
                    let max_chunk = (dst_sr / 50).max(128); // max 20ms per write
                    let to_write = avail_out.min(max_chunk) as usize;

                    if to_write >= 32 {
                        work_buf.clear();
                        if ring.len() >= 8 {
                            let err_frames = (ring.len() as f64) - (target_ring_frames as f64);
                            let drift = (err_frames * 0.000_004).clamp(-0.0015, 0.0015);
                            let step = base_ratio * (1.0 + drift);

                            for _ in 0..to_write {
                                let idx = phase.floor() as usize;
                                if idx + 3 >= ring.len() {
                                    work_buf.push([0.0, 0.0]);
                                } else {
                                    let t = (phase - (idx as f64)) as f32;
                                    let p0 = ring[idx];
                                    let p1 = ring[idx + 1];
                                    let p2 = ring[idx + 2];
                                    let p3 = ring[idx + 3];
                                    let l = hermite_interp(p0[0], p1[0], p2[0], p3[0], t);
                                    let r = hermite_interp(p0[1], p1[1], p2[1], p3[1], t);
                                    work_buf.push([l, r]);
                                    phase += step;
                                }
                            }
                            let consumed = phase.floor() as usize;
                            if consumed > 0 {
                                let drop_n = consumed.min(ring.len().saturating_sub(4));
                                ring.drain(..drop_n);
                                phase -= drop_n as f64;
                            }
                        } else {
                            work_buf.resize(to_write, [0.0, 0.0]);
                        }

                        let (corr, width_pct) = engine.process_stereo_block(&mut work_buf);
                        if !ren.write_stereo(&work_buf) {
                            break;
                        }

                        if last_stat_update.elapsed() >= Duration::from_millis(120) {
                            last_stat_update = Instant::now();
                            let ren_pad_frames =
                                ren.buffer_frames.saturating_sub(avail_out) as usize + to_write;
                            let total_lat_sec = ((ring.len() as f32) / (src_sr as f32)
                                + (ren_pad_frames as f32) / (dst_sr as f32))
                                .clamp(0.012, 0.160);
                            let lat_ms = total_lat_sec * 1000.0;
                            BINAURAL_LATENCY_US.store(
                                (total_lat_sec * 1_000_000.0).round() as u32,
                                Ordering::Release,
                            );
                            BINAURAL_ACTIVE.store(true, Ordering::Release);
                            if let Ok(mut ss) = SHARED_STATUS.write() {
                                ss.active = true;
                                ss.sample_rate = dst_sr;
                                ss.latency_ms = (lat_ms * 10.0).round() / 10.0;
                                ss.phase_correlation = (corr * 100.0).round() / 100.0;
                                ss.spatial_width_pct = (width_pct * 10.0).round() / 10.0;
                                ss.target_device_name = tgt_name_thread.clone();
                                ss.capture_bus_name = cap_name_thread.clone();
                            }
                        }
                    }

                    std::thread::sleep(Duration::from_millis(5));
                }

                BINAURAL_ACTIVE.store(false, Ordering::Release);
                BINAURAL_LATENCY_US.store(0, Ordering::Release);
                if let Ok(mut ss) = SHARED_STATUS.write() {
                    ss.active = false;
                    ss.latency_ms = 0.0;
                }
                ren.close();
                cap.close();
                if let Some(k) = keeper {
                    k.close();
                }
            })
            .map_err(|e| format!("启动双耳渲染实时音频线程失败: {e}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(4)) {
            Ok(Ok(sr)) => {
                *bridge_guard = Some(ActiveBridge {
                    original_default_id: orig_default_id.clone(),
                    target_id: cfg.target_device_id.clone(),
                    capture_id: cfg.capture_bus_id.clone(),
                    join_handle: Some(jh),
                });
                let st = BinauralStatus {
                    active: true,
                    bypass: cfg.bypass,
                    config: cfg,
                    target_device_name: target_name.clone(),
                    capture_bus_name: capture_name.clone(),
                    sample_rate: sr,
                    latency_ms: 34.0,
                    phase_correlation: 0.65,
                    spatial_width_pct: 48.0,
                    can_enable: true,
                    message: Some(format!(
                        "Apple 级本地双耳渲染已激活：{capture_name} (捕获) → HRTF 空间引擎 → {target_name}"
                    )),
                };
                if let Ok(mut ss) = SHARED_STATUS.write() {
                    *ss = st.clone();
                }
                Ok(st)
            }
            Ok(Err(e)) => {
                BINAURAL_ACTIVE.store(false, Ordering::Release);
                BINAURAL_LATENCY_US.store(0, Ordering::Release);
                let _ = jh.join();
                crate::audio_viz::set_viz_target_override(None);
                let _ = crate::audio_viz::set_output_device(&orig_default_id);
                clear_restore_marker();
                Err(e)
            }
            Err(_) => {
                BRIDGE_STOP_FLAG.store(true, Ordering::Release);
                BINAURAL_ACTIVE.store(false, Ordering::Release);
                BINAURAL_LATENCY_US.store(0, Ordering::Release);
                let _ = jh.join();
                crate::audio_viz::set_viz_target_override(None);
                let _ = crate::audio_viz::set_output_device(&orig_default_id);
                clear_restore_marker();
                Err("初始化双耳空间渲染管线超时".to_string())
            }
        }
    }

    pub fn get_binaural_status() -> BinauralStatus {
        let can_enable = CACHED_CAN_ENABLE.load(Ordering::Relaxed);
        if let Ok(st) = SHARED_STATUS.read() {
            let mut out = st.clone();
            out.can_enable = can_enable;
            if let Ok(cfg) = SHARED_CONFIG.read() {
                out.config = cfg.clone();
                out.bypass = cfg.bypass;
            }
            return out;
        }
        BinauralStatus::default()
    }

    pub fn shutdown_binaural_cleanly() {
        if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
            stop_active_bridge_locked(&mut guard, true);
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod win {
    use super::*;
    pub fn apply_binaural_config(_req: BinauralConfig) -> Result<BinauralStatus, String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn get_binaural_status() -> BinauralStatus {
        BinauralStatus::default()
    }
    pub fn shutdown_binaural_cleanly() {}
}

pub use win::{apply_binaural_config, get_binaural_status, shutdown_binaural_cleanly};
