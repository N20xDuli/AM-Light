use base64::{engine::general_purpose, Engine as _};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOutputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub format_tag: String,
    #[serde(default)]
    pub spatial_audio_active: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAudioApp {
    pub pid: u32,
    pub exe_name: String,
    pub peak_db: f32,
    pub io_kbps: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HlsStreamVariant {
    pub group_id: String,
    pub codec: String,
    pub codec_label: String,
    pub tier_label: String,
    pub sample_rate_hz: Option<u32>,
    pub bit_depth: Option<u16>,
    pub channels: String,
    pub avg_bitrate_kbps: u32,
    pub peak_bitrate_kbps: u32,
    pub is_lossless: bool,
    pub is_hires: bool,
    pub is_atmos: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamAudioSpecs {
    pub codec: String,
    pub codec_label: String,
    pub tier_label: String,
    pub sample_rate_hz: u32,
    pub bit_depth: Option<u16>,
    pub channels: String,
    pub bitrate_kbps: u32,
    pub peak_bitrate_kbps: u32,
    pub uncompressed_pcm_kbps: Option<u32>,
    pub compression_ratio_pct: Option<f32>,
    #[serde(default)]
    pub atmos_fallback_reason: Option<String>,
    pub available_variants: Vec<HlsStreamVariant>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioVizFrame {
    pub active_signal: bool,
    pub waveform: Vec<f32>,
    pub spectrum: Vec<f32>,
    pub peak_db_l: f32,
    pub peak_db_r: f32,
    pub rms_db: f32,
    pub crest_db: f32,
    #[serde(default)]
    pub phase_correlation: f32,
    #[serde(default)]
    pub stereo_width_pct: f32,
    pub device_id: String,
    pub device_name: String,
    pub device_sample_rate: u32,
    pub device_bit_depth: u16,
    pub device_channels: u16,
}

impl Default for AudioVizFrame {
    fn default() -> Self {
        Self {
            active_signal: false,
            waveform: vec![0.0; 96],
            spectrum: vec![0.0; 32],
            peak_db_l: -90.0,
            peak_db_r: -90.0,
            rms_db: -90.0,
            crest_db: 0.0,
            phase_correlation: 1.0,
            stereo_width_pct: 0.0,
            device_id: String::new(),
            device_name: String::new(),
            device_sample_rate: 48_000,
            device_bit_depth: 32,
            device_channels: 2,
        }
    }
}

/// High-refresh (>=60Hz up to VSync) merged audio + forensic + soundstage + BPM frame
#[derive(Clone, Debug)]
pub struct LiveVisualizerSnapshot {
    pub active_signal: bool,
    pub paused: bool,
    pub device_id: String,
    pub device_name: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub nyquist_hz: f32,

    /// 32-band musical spectrum (0.0..1.0) + peak hold
    pub spectrum_32: [f32; 32],
    pub peaks_32: [f32; 32],
    /// 64-band log spectrum (0.0..1.0) + slow-falling XP-style peak hold
    pub spectrum_64: [f32; 64],
    pub peaks_64: [f32; 64],
    /// 120-band radial spectrum (0.0..1.0) for osu! circular visualizer
    pub radial_120: [f32; 120],

    /// 192-point stereo waveforms (-1.0..1.0)
    pub wave_l: Vec<f32>,
    pub wave_r: Vec<f32>,
    /// 256-point stereo (L, R) pairs for Lissajous / Goniometer Soundstage (声场图)
    pub soundstage_lr: Vec<[f32; 2]>,

    /// 256 linear-frequency FFT bins (-110..0 dBFS) for Spek waterfall & brickwall curve
    pub inst_spectrum_db: Vec<f32>,
    pub peak_envelope_db: Vec<f32>,

    /// Forensic audio quality inspection results
    pub tier_code: String,
    pub verdict_title: String,
    pub estimated_bitrate: String,
    pub confidence_pct: u8,
    pub forensic_notes: String,
    pub cutoff_hz: f32,
    pub inst_cutoff_hz: f32,
    pub brickwall_drop_db: f32,
    pub has_brickwall_cut: bool,
    pub mdct_hole_pct: f32,
    pub detected_bit_grid: String,
    pub clip_count: u64,
    pub analyzed_frames: u64,
    pub auto_reset_silence: bool,
    pub active_apps: Vec<ActiveAudioApp>,

    /// Loudness, dynamics, stereo phase, and real-time beat/BPM telemetry
    pub peak_db_l: f32,
    pub peak_db_r: f32,
    pub rms_db: f32,
    pub dynamic_range_db: f32,
    pub phase_correlation: f32,
    pub stereo_width_pct: f32,
    pub bass_energy: f32,
    pub mid_energy: f32,
    pub high_energy: f32,
    pub beat_pulse: f32,
    pub beat_phase: f32,
    pub beat_triggered: bool,
    pub estimated_bpm: f32,
}

impl Default for LiveVisualizerSnapshot {
    fn default() -> Self {
        Self {
            active_signal: false,
            paused: false,
            device_id: String::new(),
            device_name: "默认输出设备".to_string(),
            sample_rate: 48_000,
            bit_depth: 24,
            channels: 2,
            nyquist_hz: 24_000.0,
            spectrum_32: [0.0; 32],
            peaks_32: [0.0; 32],
            spectrum_64: [0.0; 64],
            peaks_64: [0.0; 64],
            radial_120: [0.0; 120],
            wave_l: vec![0.0; 192],
            wave_r: vec![0.0; 192],
            soundstage_lr: vec![[0.0, 0.0]; 256],
            inst_spectrum_db: vec![-110.0; 256],
            peak_envelope_db: vec![-110.0; 256],
            tier_code: "waiting".to_string(),
            verdict_title: "等待音频输出...".to_string(),
            estimated_bitrate: "-".to_string(),
            confidence_pct: 0,
            forensic_notes: "开始播放后将实时采样输出信号，分析有效频谱上限、MDCT 子带连续度与量化位深特征。".to_string(),
            cutoff_hz: 0.0,
            inst_cutoff_hz: 0.0,
            brickwall_drop_db: 0.0,
            has_brickwall_cut: false,
            mdct_hole_pct: 0.0,
            detected_bit_grid: "-".to_string(),
            clip_count: 0,
            analyzed_frames: 0,
            auto_reset_silence: true,
            active_apps: Vec::new(),
            peak_db_l: -96.0,
            peak_db_r: -96.0,
            rms_db: -96.0,
            dynamic_range_db: 0.0,
            phase_correlation: 1.0,
            stereo_width_pct: 0.0,
            bass_energy: 0.0,
            mid_energy: 0.0,
            high_energy: 0.0,
            beat_pulse: 0.0,
            beat_phase: 0.0,
            beat_triggered: false,
            estimated_bpm: 120.0,
        }
    }
}

pub const SPECTRUM_FREQS: [f32; 32] = [
    31.0, 39.0, 50.0, 63.0, 79.0, 100.0, 126.0, 158.0, 200.0, 251.0, 316.0, 398.0, 501.0, 631.0,
    794.0, 1000.0, 1259.0, 1585.0, 1995.0, 2512.0, 3162.0, 3981.0, 5012.0, 6310.0, 7943.0, 10000.0,
    11800.0, 13500.0, 15200.0, 17000.0, 18500.0, 20000.0,
];

/// Radix-2 Cooley-Tukey in-place complex FFT (`n` must be a power of two).
pub fn fft_radix2_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert_eq!(n, im.len());
    debug_assert!(n.is_power_of_two());

    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while (j & bit) != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let half = len >> 1;
        let ang = -2.0 * PI / (len as f32);
        let (wlen_sin, wlen_cos) = ang.sin_cos();
        let mut i = 0usize;
        while i < n {
            let mut wr = 1.0f32;
            let mut wi = 0.0f32;
            for k in 0..half {
                let u_r = re[i + k];
                let u_i = im[i + k];
                let v_r = re[i + k + half] * wr - im[i + k + half] * wi;
                let v_i = re[i + k + half] * wi + im[i + k + half] * wr;
                re[i + k] = u_r + v_r;
                im[i + k] = u_i + v_i;
                re[i + k + half] = u_r - v_r;
                im[i + k + half] = u_i - v_i;

                let next_wr = wr * wlen_cos - wi * wlen_sin;
                wi = wr * wlen_sin + wi * wlen_cos;
                wr = next_wr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// 4-term Blackman-Harris window (-92 dB sidelobe suppression)
pub fn build_blackman_harris_window(n: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![1.0; n];
    }
    let denom = (n - 1) as f32;
    (0..n)
        .map(|i| {
            let x = (i as f32) / denom;
            0.35875 - 0.48829 * (2.0 * PI * x).cos() + 0.14128 * (4.0 * PI * x).cos()
                - 0.01168 * (6.0 * PI * x).cos()
        })
        .collect()
}

pub fn compute_fft_dbfs(samples: &[f32], window: &[f32], re: &mut [f32], im: &mut [f32]) -> Vec<f32> {
    let n = window.len();
    if n < 2 || !n.is_power_of_two() || samples.len() < n || re.len() < n || im.len() < n {
        return vec![-110.0; (n / 2).max(1)];
    }
    for i in 0..n {
        re[i] = samples[i] * window[i];
        im[i] = 0.0;
    }
    fft_radix2_in_place(&mut re[..n], &mut im[..n]);

    let half = n / 2;
    let norm = 1.0 / (0.179_375 * (n as f32));
    let mut out = Vec::with_capacity(half);
    for k in 0..half {
        let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() * norm;
        let db = 20.0 * mag.max(3.162e-6).log10();
        out.push(db.clamp(-110.0, 0.0));
    }
    out
}

pub fn estimate_cutoff_from_bins(bins_db: &[f32], nyquist_hz: f32) -> (f32, f32, bool) {
    let n = bins_db.len();
    if n < 32 || nyquist_hz <= 1000.0 {
        return (0.0, 0.0, false);
    }
    let hz_per_bin = nyquist_hz / (n as f32);
    let r0 = ((5500.0 / hz_per_bin) as usize).min(n - 4);
    let r1 = ((10_500.0 / hz_per_bin) as usize).clamp(r0 + 1, n);

    // Sub-band peak envelope across 5.5–10.5 kHz so both broadband mixes and sparse harmonic
    // instruments yield an accurate upper-mid reference level (while silence stays -110 dB).
    let span = r1 - r0;
    let sub_cnt = 8usize.min(span).max(1);
    let mut env_sum = 0.0f32;
    for s in 0..sub_cnt {
        let s0 = r0 + (s * span) / sub_cnt;
        let s1 = (r0 + ((s + 1) * span) / sub_cnt).clamp(s0 + 1, r1);
        let sub_max = bins_db[s0..s1].iter().copied().fold(-110.0f32, f32::max);
        env_sum += sub_max;
    }
    let ref_avg = env_sum / (sub_cnt as f32);

    if ref_avg < -88.0 {
        return (0.0, 0.0, false);
    }

    let active_thresh = (ref_avg - 38.0).clamp(-84.0, -50.0);
    let min_bin = ((4000.0 / hz_per_bin) as usize).min(n - 3);
    let mut cutoff_bin = min_bin;
    for b in (min_bin..(n - 1)).rev() {
        let neigh = bins_db[b.saturating_sub(1)].max(bins_db[(b + 1).min(n - 1)]);
        let local_support = 0.65 * bins_db[b] + 0.35 * neigh;
        if bins_db[b] >= active_thresh && local_support >= active_thresh - 14.0 {
            cutoff_bin = b;
            break;
        }
    }

    let cutoff_hz = ((cutoff_bin + 1) as f32) * hz_per_bin;
    let below_span = ((1050.0 / hz_per_bin).ceil() as usize).max(2);
    let below_0 = cutoff_bin.saturating_sub(below_span);
    let below_1 = (cutoff_bin + 1).min(n);
    let pre_span = ((1350.0 / hz_per_bin).ceil() as usize).max(3);
    let pre_0 = below_0.saturating_sub(pre_span);
    let pre_1 = below_0;
    let above_0 = (cutoff_bin + 2).min(n);
    // Span ~1.4 kHz above cutoff so sparse high-pitched acoustic harmonics (e.g. C6–E6 piano notes
    // spaced 1.0–1.3 kHz apart) are included in the upper window rather than mistaking an inter-harmonic
    // valley for a codec lowpass stopband, while genuine 128k–320k lowpass stopbands stay flat at floor.
    let above_1 = (cutoff_bin + 2 + ((1400.0 / hz_per_bin).ceil() as usize).max(3)).min(n);

    let mut below_peak = ref_avg;
    let below_lvl = if below_1 > below_0 {
        let slice = &bins_db[below_0..below_1];
        let mean = slice.iter().sum::<f32>() / (slice.len() as f32);
        let peak = slice.iter().copied().fold(-110.0f32, f32::max);
        below_peak = peak;
        0.55 * peak + 0.45 * mean
    } else {
        ref_avg
    };
    let pre_lvl = if pre_1 > pre_0 {
        let slice = &bins_db[pre_0..pre_1];
        let mean = slice.iter().sum::<f32>() / (slice.len() as f32);
        let peak = slice.iter().copied().fold(-110.0f32, f32::max);
        0.55 * peak + 0.45 * mean
    } else {
        below_lvl
    };
    let above_lvl = if above_1 > above_0 {
        let slice = &bins_db[above_0..above_1];
        let mean = slice.iter().sum::<f32>() / (slice.len() as f32);
        let peak = slice.iter().copied().fold(-110.0f32, f32::max);
        0.55 * peak + 0.45 * mean
    } else {
        below_lvl
    };

    let pre_drop = (pre_lvl - below_lvl).max(0.0);
    let drop_db = (below_lvl - above_lvl).max(0.0);
    // True codec lowpass brickwall requires:
    // 1. Sharp >=14.5 dB stopband cliff below Nyquist with a clear filter knee (`drop_db >= pre_drop * 1.55 + 8.5`)
    //    so real-world pink-tilted (-6 dB/oct) 128k–320k music is caught accurately while steady natural acoustic
    //    slopes do not false-trigger.
    // 2. Passband corner clearance: either standard level (`below_peak >= -64.0`, `below_lvl >= -69.5`) so faint
    //    `-72..-76 dBFS` acoustic/piano overtones decaying into a `-95 dBFS` 16-bit dither floor (`~21 dB` drop)
    //    never false-trigger, OR digitally attenuated stream (`above_lvl <= -102.0` & `drop_db >= 28.5` where
    //    both passband and stopband were scaled down together by a volume slider).
    // 3. Cutoff in codec lowpass range (`>= 13.2 kHz` for 96–320 kbps codecs, or `10.8..13.2 kHz` only when
    //    a hard `>= 18 dB` lowpass shelf is present).
    let standard_corner_ok = below_peak >= -64.0 && below_lvl >= -69.5;
    let attenuated_corner_ok = below_peak >= (ref_avg - 18.0).max(-76.0)
        && below_lvl >= (ref_avg - 24.0).max(-80.0)
        && above_lvl <= -102.0
        && drop_db >= 28.5;
    let has_brickwall = drop_db >= 14.5
        && drop_db >= pre_drop * 1.55 + 8.5
        && cutoff_hz >= 10_800.0
        && cutoff_hz < (nyquist_hz - 420.0)
        && (standard_corner_ok || attenuated_corner_ok)
        && (cutoff_hz >= 13_200.0
            || (drop_db >= 18.0 && (below_lvl >= -60.0 || attenuated_corner_ok)));

    (
        (cutoff_hz * 10.0).round() / 10.0,
        (drop_db * 10.0).round() / 10.0,
        has_brickwall,
    )
}

pub fn measure_mdct_subband_holes(raw_fft_db: &[f32], nyquist_hz: f32) -> f32 {
    let n = raw_fft_db.len();
    if n < 512 || nyquist_hz < 20_000.0 {
        return 0.0;
    }
    let hz_per_bin = nyquist_hz / (n as f32);
    let start_bin = ((13_000.0 / hz_per_bin) as usize).min(n - 32);
    let end_bin = ((19_000.0 / hz_per_bin) as usize).clamp(start_bin + 24, n);
    let span = end_bin - start_bin;
    let num_bands = 24usize;
    let step = (span / num_bands).max(1);

    let mut band_avg = [0.0f32; 24];
    for m in 0..num_bands {
        let b0 = start_bin + m * step;
        let b1 = (b0 + step).min(end_bin);
        let s: f32 = raw_fft_db[b0..b1].iter().sum();
        band_avg[m] = s / ((b1 - b0).max(1) as f32);
    }

    let mut holes = 0usize;
    let mut evaluated = 0usize;
    for m in 1..(num_bands - 1) {
        let neigh_max = band_avg[m - 1].max(band_avg[m + 1]);
        if neigh_max > -76.0 {
            evaluated += 1;
            if (neigh_max - band_avg[m]) > 18.5 {
                holes += 1;
            }
        }
    }

    if evaluated == 0 {
        0.0
    } else {
        ((holes as f32) / (evaluated as f32) * 100.0).clamp(0.0, 100.0)
    }
}

pub struct ForensicAccumulator {
    pub sample_rate: u32,
    fft_size: usize,
    window: Vec<f32>,
    fft_re: Vec<f32>,
    fft_im: Vec<f32>,
    pub peak_env_db: Vec<f32>,
    pub smooth_inst_db: Vec<f32>,
    pub analyzed_frames: u64,
    pub mdct_hole_ema: f32,
    pub mdct_frames: u64,
    pub clip_count: u64,
    grid16_hits: u64,
    grid24_hits: u64,
    grid_total: u64,
}

impl ForensicAccumulator {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8_000);
        let fft_size = if sr > 50_000 { 8192 } else { 4096 };
        let window = build_blackman_harris_window(fft_size);
        Self {
            sample_rate: sr,
            fft_size,
            window,
            fft_re: vec![0.0; fft_size],
            fft_im: vec![0.0; fft_size],
            peak_env_db: vec![-110.0; 256],
            smooth_inst_db: vec![-110.0; 256],
            analyzed_frames: 0,
            mdct_hole_ema: 0.0,
            mdct_frames: 0,
            clip_count: 0,
            grid16_hits: 0,
            grid24_hits: 0,
            grid_total: 0,
        }
    }

    pub fn fft_len(&self) -> usize {
        self.fft_size
    }

    pub fn reset(&mut self) {
        self.peak_env_db.fill(-110.0);
        self.smooth_inst_db.fill(-110.0);
        self.analyzed_frames = 0;
        self.mdct_hole_ema = 0.0;
        self.mdct_frames = 0;
        self.clip_count = 0;
        self.grid16_hits = 0;
        self.grid24_hits = 0;
        self.grid_total = 0;
    }

    pub fn observe_pcm_samples(&mut self, samples_lr: &[[f32; 2]]) {
        for &[l, r] in samples_lr.iter().step_by(3) {
            if l.abs() >= 0.9995 || r.abs() >= 0.9995 {
                self.clip_count += 1;
            }
            let x = l.abs();
            if x > 0.002 && x < 0.98 {
                self.grid_total += 1;
                let s16 = x * 32_768.0;
                if (s16 - s16.round()).abs() < 1.2e-3 {
                    self.grid16_hits += 1;
                }
                let s24 = x * 8_388_608.0;
                if (s24 - s24.round()).abs() < 2.5e-2 {
                    self.grid24_hits += 1;
                }
            }
        }
    }

    pub fn integer_grid_tier(&self) -> Option<u8> {
        if self.grid_total < 200 {
            return None;
        }
        let r16 = (self.grid16_hits as f64) / (self.grid_total as f64);
        let r24 = (self.grid24_hits as f64) / (self.grid_total as f64);
        if r16 > 0.96 {
            Some(16)
        } else if r24 > 0.92 {
            Some(24)
        } else {
            None
        }
    }

    pub fn detected_bit_grid_label(&self) -> String {
        if self.grid_total < 200 {
            return "采样积累中...".to_string();
        }
        match self.integer_grid_tier() {
            Some(16) => "16-bit 整数无损网格".to_string(),
            Some(24) => "24-bit 高精度整数网格".to_string(),
            _ => "24/32-bit 浮点高精度".to_string(),
        }
    }

    pub fn process_fft_window(&mut self, mono_window: &[f32]) -> (Vec<f32>, f32) {
        let raw_bins = compute_fft_dbfs(
            mono_window,
            &self.window,
            &mut self.fft_re,
            &mut self.fft_im,
        );
        let half = raw_bins.len();
        let bins_per_bucket = (half / 256).max(1);
        let mut disp = vec![-110.0f32; 256];
        for b in 0..256 {
            let start = b * bins_per_bucket;
            let end = ((b + 1) * bins_per_bucket).min(half);
            let mut mx = -110.0f32;
            for &v in &raw_bins[start..end] {
                if v > mx {
                    mx = v;
                }
            }
            disp[b] = mx;
            if mx > self.smooth_inst_db[b] {
                self.smooth_inst_db[b] = self.smooth_inst_db[b] * 0.32 + mx * 0.68;
            } else {
                self.smooth_inst_db[b] = self.smooth_inst_db[b] * 0.74 + mx * 0.26;
            }
        }

        let nyquist = (self.sample_rate as f32) * 0.5;
        let hz_per_disp = nyquist / 256.0;

        let mid_b0 = ((600.0 / hz_per_disp) as usize).min(255);
        let mid_b1 = ((6500.0 / hz_per_disp) as usize).clamp(mid_b0 + 1, 256);
        let mid_max = disp[mid_b0..mid_b1].iter().copied().fold(-110.0f32, f32::max);

        let presence_b0 = ((4500.0 / hz_per_disp) as usize).min(255);
        let presence_b1 = ((11_500.0 / hz_per_disp) as usize).clamp(presence_b0 + 1, 256);
        let presence_max = disp[presence_b0..presence_b1]
            .iter()
            .copied()
            .fold(-110.0f32, f32::max);

        let inst_cutoff = estimate_cutoff_from_bins(&disp, nyquist).0;

        if mid_max > -62.0 && presence_max > -76.0 {
            self.analyzed_frames += 1;
            for b in 0..256 {
                let decayed = (self.peak_env_db[b] - 0.012).max(-110.0);
                self.peak_env_db[b] = decayed.max(disp[b]);
            }

            let hf_b0 = ((10_000.0 / hz_per_disp) as usize).min(255);
            let hf_b1 = ((16_000.0 / hz_per_disp) as usize).clamp(hf_b0 + 1, 256);
            let hf_max = disp[hf_b0..hf_b1].iter().copied().fold(-110.0f32, f32::max);

            if hf_max > -66.0 && nyquist >= 20_000.0 {
                let hole_ratio = measure_mdct_subband_holes(&raw_bins, nyquist);
                self.mdct_frames += 1;
                if self.mdct_frames == 1 {
                    self.mdct_hole_ema = hole_ratio;
                } else {
                    self.mdct_hole_ema = self.mdct_hole_ema * 0.90 + hole_ratio * 0.10;
                }
            }
        }

        (self.smooth_inst_db.clone(), inst_cutoff)
    }

    pub fn evaluate_verdict(&self) -> (String, String, String, u8, String, f32, f32, bool) {
        let nyquist = (self.sample_rate as f32) * 0.5;
        if self.analyzed_frames < 4 {
            return (
                "analyzing".to_string(),
                "正在采集高频瞬态包络...".to_string(),
                "采集中...".to_string(),
                ((self.analyzed_frames * 15) as u8).min(45),
                "播放包含鼓点镲片或弦乐的段落（约 2–5 秒即可锁定有效频谱截止上限）。".to_string(),
                0.0,
                0.0,
                false,
            );
        }

        let (cutoff_hz, cliff_drop_db, has_brickwall) =
            estimate_cutoff_from_bins(&self.peak_env_db, nyquist);
        let conf = (38.0 + (self.analyzed_frames as f32).sqrt() * 8.5)
            .round()
            .clamp(42.0, 98.0) as u8;
        let holes = self.mdct_hole_ema;

        if nyquist > 26_000.0 && cutoff_hz >= 25_200.0 && holes < 26.0 {
            let notes = format!(
                "实测有效频宽延伸至 {:.1} kHz（突破 24 kHz 界限），高频连续无 MDCT 空洞（{:.1}%），符合 Hi-Res 无损特征。",
                cutoff_hz / 1000.0,
                holes
            );
            return (
                "hires_lossless".to_string(),
                "Hi-Res 真无损音频 (24-bit / >48kHz)".to_string(),
                "2,200 – 4,800+ kbps".to_string(),
                conf,
                notes,
                cutoff_hz,
                cliff_drop_db,
                has_brickwall,
            );
        }

        let cd_lossless_floor = if nyquist <= 22_100.0 {
            21_350.0
        } else {
            21_750.0
        };

        if cutoff_hz >= cd_lossless_floor && holes < 28.0 {
            if cutoff_hz >= 23_150.0 {
                let extra = if self.sample_rate <= 48_000 {
                    "（当前声卡混音率为 48kHz，物理观测上限 24.0kHz）"
                } else {
                    ""
                };
                let notes = format!(
                    "实测频谱平滑延伸至 {:.2} kHz，高频连续无编码空洞（{:.1}%），符合 ≥48kHz 无损特征。{}",
                    cutoff_hz / 1000.0,
                    holes,
                    extra
                );
                return (
                    "lossless_48k".to_string(),
                    "真无损音频 (ALAC / FLAC ≥48kHz)".to_string(),
                    "1,150 – 2,300 kbps".to_string(),
                    conf,
                    notes,
                    cutoff_hz,
                    cliff_drop_db,
                    has_brickwall,
                );
            } else {
                let notes = format!(
                    "实测频谱完整覆盖至 {:.2} kHz（吻合 44.1kHz CD 无损 22.05kHz 奈奎斯特上限），空洞率 {:.1}%。",
                    cutoff_hz / 1000.0,
                    holes
                );
                return (
                    "lossless_cd".to_string(),
                    "CD 级真无损音频 (44.1kHz)".to_string(),
                    "800 – 1,411 kbps".to_string(),
                    conf,
                    notes,
                    cutoff_hz,
                    cliff_drop_db,
                    has_brickwall,
                );
            }
        }

        // Guard against false-positive lossy verdicts on soft piano / vocal / acoustic intros:
        // When there is NO steep codec lowpass brickwall (`!has_brickwall` & `holes < 18%`), or when
        // an exact integer lossless PCM grid is confirmed on a sub-10.8 kHz tonal passage, do NOT
        // misclassify natural instrument harmonic rolloff as low-bitrate lossy compression!
        let int_grid = self.integer_grid_tier();
        let is_gentle_acoustic_rolloff = (!has_brickwall && holes < 18.0)
            || (cutoff_hz < 10_800.0 && int_grid.is_some() && holes < 12.0);

        if is_gentle_acoustic_rolloff {
            // Near 20 kHz (19.65..21.75 kHz), only treat as AAC-256k/MP3-320k shelf if a clear >=11.5 dB
            // shelf exists without an exact integer PCM lossless grid.
            let likely_20k_lossy_shelf =
                cutoff_hz >= 19_650.0 && cliff_drop_db >= 11.5 && int_grid.is_none();
            if !likely_20k_lossy_shelf {
                if let Some(bits) = int_grid {
                    let est_br = if bits >= 24 {
                        "1,100 – 2,300 kbps".to_string()
                    } else {
                        "700 – 1,411 kbps".to_string()
                    };
                    let notes = format!(
                        "已检出 {bits}-bit 整数无损网格与 {:.1}% MDCT 空洞；当前钢琴/人声等声学段落呈自然谐波衰减（{:.2} kHz 缓降 {:.1} dB，无有损低通砖墙），待宽频打击乐进入后锁定全频上限。",
                        holes,
                        cutoff_hz / 1000.0,
                        cliff_drop_db
                    );
                    return (
                        "lossless_acoustic".to_string(),
                        format!("无损 PCM 音频 ({bits}-bit 声学段落)"),
                        est_br,
                        conf.clamp(56, 82),
                        notes,
                        cutoff_hz,
                        cliff_drop_db,
                        false,
                    );
                } else {
                    let notes = format!(
                        "当前段落高频呈自然谐波衰减（至 {:.2} kHz 缓降 {:.1} dB，未见编码器低通砖墙或 MDCT 空洞 {:.1}%），待鼓点镲片等宽频瞬态进入后完成定级。",
                        cutoff_hz / 1000.0,
                        cliff_drop_db,
                        holes
                    );
                    return (
                        "narrowband_analyzing".to_string(),
                        "窄带声学段落 · 待宽频瞬态确认".to_string(),
                        "待宽频段确认".to_string(),
                        conf.min(58),
                        notes,
                        cutoff_hz,
                        cliff_drop_db,
                        false,
                    );
                }
            }
        }

        if cutoff_hz >= 19_650.0 {
            let notes = if cutoff_hz < 21_750.0 {
                format!(
                    "在 {:.2} kHz 处检出高频滚降切线（落差 {:.1} dB），典型 AAC 256kbps / MP3 320kbps 特征。",
                    cutoff_hz / 1000.0,
                    cliff_drop_db
                )
            } else {
                format!(
                    "频宽达 {:.2} kHz，但 13–19 kHz 存在 MDCT 子带空洞（{:.1}%），典型高码率有损特征。",
                    cutoff_hz / 1000.0,
                    holes
                )
            };
            return (
                "lossy_high".to_string(),
                "高码率有损音频 (AAC 256k / MP3 320k)".to_string(),
                "256 – 320 kbps".to_string(),
                conf,
                notes,
                cutoff_hz,
                cliff_drop_db,
                has_brickwall,
            );
        }

        if cutoff_hz >= 18_300.0 {
            let notes = format!(
                "在 {:.2} kHz 检出低通截断（陡降 {:.1} dB），符合 192–256 kbps 有损流特征。",
                cutoff_hz / 1000.0,
                cliff_drop_db
            );
            return (
                "lossy_mid".to_string(),
                "中高码率有损 (AAC 192k / MP3 224k)".to_string(),
                "192 – 256 kbps".to_string(),
                conf,
                notes,
                cutoff_hz,
                cliff_drop_db,
                has_brickwall,
            );
        }

        if cutoff_hz >= 16_600.0 {
            let notes = format!(
                "在 {:.2} kHz 处出现砖墙截断（陡降 {:.1} dB），属于 128–192 kbps 标准有损音频。",
                cutoff_hz / 1000.0,
                cliff_drop_db
            );
            return (
                "lossy_mid".to_string(),
                "中等码率有损 (128 – 192 kbps)".to_string(),
                "128 – 192 kbps".to_string(),
                conf,
                notes,
                cutoff_hz,
                cliff_drop_db,
                has_brickwall,
            );
        }

        let notes = format!(
            "高频能量在 {:.2} kHz 提前截断（陡降 {:.1} dB），属于 ≤128 kbps 有损或转码音频。",
            cutoff_hz / 1000.0,
            cliff_drop_db
        );
        (
            "lossy_low".to_string(),
            "低码率有损音频 (≤128 kbps)".to_string(),
            "64 – 128 kbps".to_string(),
            conf,
            notes,
            cutoff_hz,
            cliff_drop_db,
            has_brickwall,
        )
    }
}

fn extract_m3u8_attr<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let quoted_pat = format!("{key}=\"");
    if let Some(pos) = line.find(&quoted_pat) {
        let rest = &line[pos + quoted_pat.len()..];
        let end = rest.find('"')?;
        return Some(&rest[..end]);
    }
    let bare_pat = format!("{key}=");
    if let Some(pos) = line.find(&bare_pat) {
        let rest = &line[pos + bare_pat.len()..];
        let end = rest.find(',').unwrap_or(rest.len());
        return Some(rest[..end].trim());
    }
    None
}

pub fn parse_enhanced_hls_variants(
    m3u8: &str,
    lossless_enabled: bool,
    streaming_quality: i64,
    atmos_setting: i64,
) -> Option<StreamAudioSpecs> {
    parse_enhanced_hls_variants_with_endpoint(
        m3u8,
        lossless_enabled,
        streaming_quality,
        atmos_setting,
        true,
    )
}

pub fn parse_enhanced_hls_variants_with_endpoint(
    m3u8: &str,
    lossless_enabled: bool,
    streaming_quality: i64,
    atmos_setting: i64,
    endpoint_spatial_ready: bool,
) -> Option<StreamAudioSpecs> {
    let mut asset_meta: HashMap<String, Value> = HashMap::new();
    for line in m3u8.lines() {
        if line.contains("com.apple.hls.audioAssetMetadata") {
            if let Some(b64) = extract_m3u8_attr(line, "VALUE") {
                if let Ok(decoded) = general_purpose::STANDARD.decode(b64) {
                    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&decoded) {
                        for (k, v) in map {
                            asset_meta.insert(k, v);
                        }
                    }
                }
            }
        }
    }

    let mut media_groups: HashMap<String, (Option<u32>, Option<u16>, String)> = HashMap::new();
    for line in m3u8.lines() {
        if line.starts_with("#EXT-X-MEDIA:") {
            if let Some(gid) = extract_m3u8_attr(line, "GROUP-ID") {
                let sr = extract_m3u8_attr(line, "SAMPLE-RATE").and_then(|s| s.parse::<u32>().ok());
                let bd = extract_m3u8_attr(line, "BIT-DEPTH").and_then(|s| s.parse::<u16>().ok());
                let ch = extract_m3u8_attr(line, "CHANNELS")
                    .unwrap_or("2")
                    .to_string();
                media_groups.insert(gid.to_string(), (sr, bd, ch));
            }
        }
    }

    let mut variants: Vec<HlsStreamVariant> = Vec::new();
    for line in m3u8.lines() {
        if !line.starts_with("#EXT-X-STREAM-INF:") {
            continue;
        }
        let group_id = extract_m3u8_attr(line, "AUDIO")
            .unwrap_or_default()
            .to_string();
        if group_id.ends_with("-binaural") || group_id.ends_with("-downmix") {
            continue;
        }

        let peak_bps = extract_m3u8_attr(line, "BANDWIDTH")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let avg_bps = extract_m3u8_attr(line, "AVERAGE-BANDWIDTH")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(peak_bps);
        let codecs_raw = extract_m3u8_attr(line, "CODECS").unwrap_or("mp4a.40.2");
        let variant_id = extract_m3u8_attr(line, "STABLE-VARIANT-ID").unwrap_or("");

        let (mut sr, mut bd, mut ch) = media_groups
            .get(&group_id)
            .cloned()
            .unwrap_or((None, None, "2".to_string()));

        if let Some(meta_entry) = asset_meta.get(variant_id) {
            if sr.is_none() {
                sr = meta_entry
                    .get("SAMPLE-RATE")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32);
            }
            if bd.is_none() {
                bd = meta_entry
                    .get("BIT-DEPTH")
                    .and_then(Value::as_u64)
                    .map(|n| n as u16);
            }
            if let Some(c_str) = meta_entry.get("CHANNEL-COUNT").and_then(Value::as_str) {
                if ch == "2" && c_str != "2" {
                    ch = c_str.to_string();
                }
            }
        }

        let is_alac = codecs_raw.eq_ignore_ascii_case("alac");
        let is_atmos = codecs_raw.contains("ec-3")
            || codecs_raw.contains("ac-4")
            || group_id.contains("atmos");

        let (codec_label, tier_label, default_sr) = if is_alac {
            let sr_val = sr.unwrap_or(44_100);
            if sr_val > 48_000 {
                (
                    "ALAC (Apple Lossless)".to_string(),
                    "Hi-Res 无损".to_string(),
                    sr_val,
                )
            } else {
                (
                    "ALAC (Apple Lossless)".to_string(),
                    "无损 Lossless".to_string(),
                    sr_val,
                )
            }
        } else if is_atmos {
            (
                "Dolby Atmos (E-AC-3 JOC)".to_string(),
                "杜比全景声".to_string(),
                sr.unwrap_or(48_000),
            )
        } else if codecs_raw.contains("40.5") {
            (
                "HE-AAC".to_string(),
                "高效省流 AAC".to_string(),
                sr.unwrap_or(44_100),
            )
        } else {
            (
                "AAC-LC".to_string(),
                "高质量 AAC".to_string(),
                sr.unwrap_or(44_100),
            )
        };

        let is_hires = is_alac && default_sr > 48_000;
        let channels_pretty = if is_atmos || ch.contains("JOC") || ch == "16" {
            "16ch JOC 空间音频".to_string()
        } else if ch == "2" {
            "2.0 立体声".to_string()
        } else {
            format!("{ch} 声道")
        };

        variants.push(HlsStreamVariant {
            group_id,
            codec: codecs_raw.to_uppercase(),
            codec_label,
            tier_label,
            sample_rate_hz: Some(default_sr),
            bit_depth: bd,
            channels: channels_pretty,
            avg_bitrate_kbps: ((avg_bps + 500) / 1000) as u32,
            peak_bitrate_kbps: ((peak_bps + 500) / 1000) as u32,
            is_lossless: is_alac,
            is_hires,
            is_atmos,
        });
    }

    if variants.is_empty() {
        return None;
    }

    variants.sort_by(|a, b| {
        let rank_a = (
            a.is_hires as u8,
            a.is_lossless as u8,
            a.is_atmos as u8,
            a.avg_bitrate_kbps,
        );
        let rank_b = (
            b.is_hires as u8,
            b.is_lossless as u8,
            b.is_atmos as u8,
            b.avg_bitrate_kbps,
        );
        rank_b.cmp(&rank_a)
    });

    let has_atmos_variant = variants.iter().any(|v| v.is_atmos);
    let wants_atmos = atmos_setting == 20;
    let atmos_fallback_reason = if wants_atmos && has_atmos_variant && !endpoint_spatial_ready {
        Some(
            "当前输出设备未开启系统「空间音效」，播放服务已自动回退至立体声无损/母带解码。"
                .to_string(),
        )
    } else {
        None
    };

    let chosen = if wants_atmos && endpoint_spatial_ready && has_atmos_variant {
        variants
            .iter()
            .filter(|v| v.is_atmos)
            .max_by_key(|v| v.avg_bitrate_kbps)
            .unwrap()
    } else if lossless_enabled && streaming_quality >= 20 && variants.iter().any(|v| v.is_lossless) {
        variants
            .iter()
            .filter(|v| v.is_lossless)
            .max_by_key(|v| (v.sample_rate_hz.unwrap_or(0), v.avg_bitrate_kbps))
            .unwrap()
    } else if lossless_enabled && streaming_quality >= 15 && variants.iter().any(|v| v.is_lossless) {
        variants
            .iter()
            .filter(|v| v.is_lossless && v.sample_rate_hz.unwrap_or(0) <= 48_000)
            .max_by_key(|v| v.avg_bitrate_kbps)
            .or_else(|| variants.iter().filter(|v| v.is_lossless).last())
            .unwrap()
    } else {
        variants
            .iter()
            .filter(|v| !v.is_lossless && !v.is_atmos)
            .max_by_key(|v| v.avg_bitrate_kbps)
            .unwrap_or(&variants[0])
    };

    let sr = chosen.sample_rate_hz.unwrap_or(44_100);
    let uncompressed_pcm_kbps = chosen.bit_depth.map(|bd| (sr * (bd as u32) * 2) / 1000);
    let compression_ratio_pct = uncompressed_pcm_kbps.and_then(|pcm| {
        if pcm > 0 && chosen.avg_bitrate_kbps > 0 {
            Some(((chosen.avg_bitrate_kbps as f32 / pcm as f32) * 1000.0).round() / 10.0)
        } else {
            None
        }
    });

    Some(StreamAudioSpecs {
        codec: chosen.codec.clone(),
        codec_label: chosen.codec_label.clone(),
        tier_label: chosen.tier_label.clone(),
        sample_rate_hz: sr,
        bit_depth: chosen.bit_depth,
        channels: chosen.channels.clone(),
        bitrate_kbps: chosen.avg_bitrate_kbps,
        peak_bitrate_kbps: chosen.peak_bitrate_kbps,
        uncompressed_pcm_kbps,
        compression_ratio_pct,
        atmos_fallback_reason,
        available_variants: variants,
    })
}

pub fn compute_spectrum_32(samples: &[f32], sample_rate: f32) -> [f32; 32] {
    let mut out = [0.0f32; 32];
    let n = samples.len();
    if n < 64 || sample_rate <= 0.0 {
        return out;
    }

    let mut windowed = vec![0.0f32; n];
    let denom = (n - 1) as f32;
    for (i, &s) in samples.iter().enumerate() {
        let w = 0.5 * (1.0 - (2.0 * PI * (i as f32) / denom).cos());
        windowed[i] = s * w;
    }

    let n_f = n as f32;
    for (band_idx, &freq) in SPECTRUM_FREQS.iter().enumerate() {
        if freq >= sample_rate * 0.49 {
            out[band_idx] = 0.0;
            continue;
        }
        let omega = 2.0 * PI * freq / sample_rate;
        let coeff = 2.0 * omega.cos();
        let mut s_prev = 0.0f32;
        let mut s_prev2 = 0.0f32;
        for &x in &windowed {
            let s = x + coeff * s_prev - s_prev2;
            s_prev2 = s_prev;
            s_prev = s;
        }
        let power = (s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2).max(0.0);
        let mag = (power.sqrt() * 2.0) / n_f;
        let tilt = 1.0 + (band_idx as f32) * 0.055;
        let db = 20.0 * ((mag * tilt).max(1e-5)).log10();
        let norm = ((db + 58.0) / 52.0).clamp(0.0, 1.0);
        out[band_idx] = norm;
    }
    out
}

/// Shared high-speed ring buffer written at ~160Hz by WASAPI and sampled at 60Hz..VSync by Direct2D
const RING_LEN: usize = 16_384;
const RING_MASK: usize = RING_LEN - 1;

struct SharedRingState {
    stereo: Vec<[f32; 2]>,
    write_pos: usize,
    active_signal: bool,
    device_id: String,
    device_name: String,
    sample_rate: u32,
    bit_depth: u16,
    channels: u16,
    peak_db_l: f32,
    peak_db_r: f32,
    rms_db: f32,
    dynamic_range_db: f32,
    phase_correlation: f32,
    stereo_width_pct: f32,
}

impl Default for SharedRingState {
    fn default() -> Self {
        Self {
            stereo: vec![[0.0, 0.0]; RING_LEN],
            write_pos: 0,
            active_signal: false,
            device_id: String::new(),
            device_name: "默认输出设备".to_string(),
            sample_rate: 48_000,
            bit_depth: 24,
            channels: 2,
            peak_db_l: -96.0,
            peak_db_r: -96.0,
            rms_db: -96.0,
            dynamic_range_db: 0.0,
            phase_correlation: 1.0,
            stereo_width_pct: 0.0,
        }
    }
}

#[derive(Clone)]
struct SharedForensicSummary {
    inst_spectrum_db: Vec<f32>,
    peak_envelope_db: Vec<f32>,
    tier_code: String,
    verdict_title: String,
    estimated_bitrate: String,
    confidence_pct: u8,
    forensic_notes: String,
    cutoff_hz: f32,
    inst_cutoff_hz: f32,
    brickwall_drop_db: f32,
    has_brickwall_cut: bool,
    mdct_hole_pct: f32,
    detected_bit_grid: String,
    clip_count: u64,
    analyzed_frames: u64,
    active_apps: Vec<ActiveAudioApp>,
}

impl Default for SharedForensicSummary {
    fn default() -> Self {
        let def = LiveVisualizerSnapshot::default();
        Self {
            inst_spectrum_db: def.inst_spectrum_db,
            peak_envelope_db: def.peak_envelope_db,
            tier_code: def.tier_code,
            verdict_title: def.verdict_title,
            estimated_bitrate: def.estimated_bitrate,
            confidence_pct: def.confidence_pct,
            forensic_notes: def.forensic_notes,
            cutoff_hz: 0.0,
            inst_cutoff_hz: 0.0,
            brickwall_drop_db: 0.0,
            has_brickwall_cut: false,
            mdct_hole_pct: 0.0,
            detected_bit_grid: def.detected_bit_grid,
            clip_count: 0,
            analyzed_frames: 0,
            active_apps: Vec::new(),
        }
    }
}

/// Studio-grade real-time Tempo & Beat Phase Tracker:
/// 1. Sample-clocked 1024-pt STFT (512/1024 hop -> ~90 Hz exact hop clock, immune to UI FPS jitter)
/// 2. 6-Band Log-Compressed Spectral Flux Onset Envelope + adaptive local threshold
/// 3. Normalized Autocorrelation + 4-Harmonic Resonant Comb-Filter Bank (66..196 BPM @ 0.5 BPM step)
///    with perceptual tempo prior, octave/half-time disambiguation, and parabolic peak refinement
/// 4. Phase-Locked Loop (PLL) beat phase oscillator (`0.0..1.0`) for crisp osu!-style emblem bounce!
pub struct TempoTracker {
    sample_rate: u32,
    hop_samples: usize,
    hop_hz: f32,
    pcm_ring: [f32; 1024],
    pcm_write: usize,
    samples_since_hop: usize,
    hann_1024: Vec<f32>,
    fft_re: Vec<f32>,
    fft_im: Vec<f32>,
    prev_log_bands: [f32; 6],
    local_flux_mean: f32,
    local_onset_peak: f32,
    onset_hist: VecDeque<f32>,
    hops_since_eval: usize,
    candidate_streak_bpm: f32,
    candidate_streak_cnt: u8,
    pub locked_bpm: f32,
    pub confidence: f32,
    pub beat_phase: f32,
    pub beat_seq: u64,
    refractory_hops: usize,
}

impl TempoTracker {
    const HIST_CAP: usize = 512;

    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.clamp(8_000, 384_000);
        let hop_samples = ((sr as f32 / 92.0).round() as usize).clamp(256, 4096);
        let hop_hz = (sr as f32) / (hop_samples as f32);
        let mut hann = vec![0.0f32; 1024];
        for (i, w) in hann.iter_mut().enumerate() {
            *w = 0.5 * (1.0 - (2.0 * PI * (i as f32) / 1023.0).cos());
        }
        Self {
            sample_rate: sr,
            hop_samples,
            hop_hz,
            pcm_ring: [0.0; 1024],
            pcm_write: 0,
            samples_since_hop: 0,
            hann_1024: hann,
            fft_re: vec![0.0; 1024],
            fft_im: vec![0.0; 1024],
            prev_log_bands: [0.0; 6],
            local_flux_mean: 0.0,
            local_onset_peak: 0.08,
            onset_hist: VecDeque::with_capacity(Self::HIST_CAP + 4),
            hops_since_eval: 0,
            candidate_streak_bpm: 120.0,
            candidate_streak_cnt: 0,
            locked_bpm: 120.0,
            confidence: 0.0,
            beat_phase: 0.0,
            beat_seq: 0,
            refractory_hops: 0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        let sr = sample_rate.clamp(8_000, 384_000);
        if self.sample_rate != sr {
            *self = Self::new(sr);
        }
    }

    pub fn reset(&mut self) {
        self.pcm_ring = [0.0; 1024];
        self.pcm_write = 0;
        self.samples_since_hop = 0;
        self.prev_log_bands = [0.0; 6];
        self.local_flux_mean = 0.0;
        self.local_onset_peak = 0.08;
        self.onset_hist.clear();
        self.hops_since_eval = 0;
        self.candidate_streak_cnt = 0;
        self.confidence = 0.0;
        self.refractory_hops = 0;
    }

    pub fn observe_stereo_pcm(&mut self, samples_lr: &[[f32; 2]]) {
        let decim = (self.hop_samples / 512).max(1);
        for (idx, &[l, r]) in samples_lr.iter().enumerate() {
            if idx % decim == 0 {
                self.pcm_ring[self.pcm_write] = 0.5 * (l + r);
                self.pcm_write = (self.pcm_write + 1) & 1023;
            }
            self.samples_since_hop += 1;
            if self.samples_since_hop >= self.hop_samples {
                self.samples_since_hop = 0;
                self.process_hop();
            }
        }
    }

    fn process_hop(&mut self) {
        for i in 0..1024 {
            let s = self.pcm_ring[(self.pcm_write + i) & 1023];
            self.fft_re[i] = s * self.hann_1024[i];
            self.fft_im[i] = 0.0;
        }
        fft_radix2_in_place(&mut self.fft_re, &mut self.fft_im);

        let eff_sr = (self.sample_rate as f32) / ((self.hop_samples / 512).max(1) as f32);
        let bin_hz = eff_sr / 1024.0;
        let norm = 2.0 / 512.0;

        const BANDS: [(f32, f32, f32); 6] = [
            (35.0, 95.0, 1.55),     // Sub-Kick
            (95.0, 210.0, 1.32),    // Upper-Kick / Bass Punch
            (210.0, 560.0, 0.92),   // Snare Body / Low-Mid
            (560.0, 1850.0, 1.05),  // Vocal & Chord Transients
            (1850.0, 4800.0, 1.18), // Snare Crack / Percussive Presence
            (4800.0, 12500.0, 0.72), // Hi-Hat / Cymbal Attack
        ];

        let mut flux = 0.0f32;
        for (bi, &(f0, f1, w)) in BANDS.iter().enumerate() {
            let b0 = ((f0 / bin_hz).floor() as usize).clamp(1, 510);
            let b1 = ((f1 / bin_hz).ceil() as usize).clamp(b0 + 1, 511);
            let mut energy = 0.0f32;
            for k in b0..b1 {
                let m = (self.fft_re[k] * self.fft_re[k] + self.fft_im[k] * self.fft_im[k]).sqrt()
                    * norm;
                energy += m;
            }
            energy /= (b1 - b0) as f32;
            let log_e = (1.0 + 36.0 * energy).ln();
            let diff = (log_e - self.prev_log_bands[bi]).max(0.0);
            self.prev_log_bands[bi] = log_e;
            flux += w * diff;
        }

        // Adaptive local moving mean subtraction -> clean percussive onset spikes
        self.local_flux_mean = self.local_flux_mean * 0.90 + flux * 0.10;
        let onset = (flux - self.local_flux_mean * 1.12).max(0.0);
        self.local_onset_peak = (self.local_onset_peak * 0.985).max(onset).max(0.04);

        if self.onset_hist.len() >= Self::HIST_CAP {
            self.onset_hist.pop_front();
        }
        self.onset_hist.push_back(onset);

        // Advance PLL beat oscillator & phase-align to strong real musical onsets
        let hop_dt = 1.0 / self.hop_hz.max(20.0);
        let prev_phase = self.beat_phase;
        self.beat_phase += hop_dt * (self.locked_bpm / 60.0);
        self.refractory_hops = self.refractory_hops.saturating_sub(1);

        let min_refrac = (self.hop_hz * 0.22) as usize;
        let strong_hit = onset > self.local_onset_peak * 0.42 && onset > 0.035;
        if strong_hit {
            // Wrap phase error into [-0.5, +0.5] around beat boundary 0.0 / 1.0
            let frac = self.beat_phase.rem_euclid(1.0);
            let err = if frac > 0.5 { frac - 1.0 } else { frac };
            if err.abs() < 0.30 {
                self.beat_phase -= err * 0.34;
            }
        }

        let wrapped = self.beat_phase >= 1.0;
        if wrapped {
            self.beat_phase = self.beat_phase.rem_euclid(1.0);
        }

        let near_downbeat = self.beat_phase < 0.14 || self.beat_phase > 0.88;
        if self.refractory_hops == 0 && ((wrapped && self.local_onset_peak > 0.045) || (strong_hit && near_downbeat)) {
            self.beat_seq = self.beat_seq.wrapping_add(1);
            self.refractory_hops = min_refrac;
            if strong_hit && !wrapped {
                self.beat_phase = 0.02;
            }
        }
        let _ = prev_phase;

        self.hops_since_eval += 1;
        if self.hops_since_eval >= 14 && self.onset_hist.len() >= 128 {
            self.hops_since_eval = 0;
            self.evaluate_autocorr_comb_tempo();
        }
    }

    fn evaluate_autocorr_comb_tempo(&mut self) {
        let m = self.onset_hist.len();
        if m < 128 {
            return;
        }
        let mut x = vec![0.0f32; m];
        let mut sum = 0.0f32;
        for (i, &v) in self.onset_hist.iter().enumerate() {
            x[i] = v;
            sum += v;
        }
        let mean = sum / (m as f32);
        let mut energy_0 = 0.0f32;
        for v in &mut x {
            *v -= mean;
            energy_0 += (*v) * (*v);
        }
        if energy_0 < 1e-5 {
            return;
        }

        let max_lag = (m - 16).min(((self.hop_hz * 60.0 / 32.0).ceil() as usize).max(64));
        let min_lag = ((self.hop_hz * 60.0 / 210.0).floor() as usize).clamp(8, max_lag - 4);
        let mut acf = vec![0.0f32; max_lag + 2];
        for lag in (min_lag / 2).max(4)..=max_lag {
            let cnt = m - lag;
            if cnt < 24 {
                break;
            }
            let mut acc = 0.0f32;
            let mut e_a = 0.0f32;
            let mut e_b = 0.0f32;
            for i in 0..cnt {
                let a = x[i];
                let b = x[i + lag];
                acc += a * b;
                e_a += a * a;
                e_b += b * b;
            }
            let denom = (e_a * e_b).sqrt();
            acf[lag] = if denom > 1e-6 {
                (acc / denom).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }

        let sample_acf = |lag_f: f32| -> f32 {
            let l0 = lag_f.floor() as isize;
            if l0 < 4 || (l0 as usize + 1) >= acf.len() {
                return 0.0;
            }
            let frac = lag_f - (l0 as f32);
            acf[l0 as usize] * (1.0 - frac) + acf[l0 as usize + 1] * frac
        };

        // Sweep 66.0 .. 196.0 BPM at 0.5 BPM resolution (261 candidates)
        const NUM_CANDS: usize = 261;
        let mut raw_comb = [0.0f32; NUM_CANDS];
        let mut weighted = [0.0f32; NUM_CANDS];

        for idx in 0..NUM_CANDS {
            let bpm = 66.0 + (idx as f32) * 0.5;
            let tau = self.hop_hz * 60.0 / bpm;

            // 4-Harmonic Pulse-Train Comb Filter + Eighth-Note Subdivision Support
            let mut comb_sum = 0.0f32;
            let mut w_sum = 0.0f32;
            for &(mult, hw) in &[(1.0f32, 1.00f32), (2.0, 0.58), (3.0, 0.36), (4.0, 0.22)] {
                let lt = tau * mult;
                if (lt as usize + 2) < acf.len() {
                    comb_sum += hw * sample_acf(lt).max(-0.25);
                    w_sum += hw;
                }
            }
            if w_sum <= 0.0 {
                continue;
            }
            let mut c_score = (comb_sum / w_sum).max(0.0);
            // Gentle sub-beat reinforcement without letting B/2 steal B's main beat energy
            if tau * 0.5 >= 5.0 {
                c_score += 0.07 * sample_acf(tau * 0.5).max(0.0);
            }
            raw_comb[idx] = c_score;

            // Perceptual Log-Gaussian Tempo Prior centered around 124 BPM (Ellis 2007 human tactus prior)
            let log_ratio = (bpm / 124.0).ln() / 0.35;
            let prior = (-0.5 * log_ratio * log_ratio).exp();
            // Gentle continuity lock around already-established tempo
            let cont = if self.confidence > 0.35 {
                let d = (bpm - self.locked_bpm).abs();
                1.0 + 0.14 * (-d * d / 18.0).exp()
            } else {
                1.0
            };
            weighted[idx] = c_score * (0.58 + 0.42 * prior) * cont;
        }

        let mut best_i = 0usize;
        let mut best_val = 0.0f32;
        for (i, &v) in weighted.iter().enumerate() {
            if v > best_val {
                best_val = v;
                best_i = i;
            }
        }
        if best_val < 0.08 {
            return;
        }

        // Parabolic peak interpolation for sub-0.1 BPM accuracy
        let mut refined_bpm = 66.0 + (best_i as f32) * 0.5;
        if best_i > 0 && best_i + 1 < NUM_CANDS {
            let y0 = weighted[best_i - 1];
            let y1 = weighted[best_i];
            let y2 = weighted[best_i + 1];
            let denom = y0 - 2.0 * y1 + y2;
            if denom.abs() > 1e-6 {
                let delta = (0.5 * (y0 - y2) / denom).clamp(-0.5, 0.5);
                refined_bpm += delta * 0.5;
            }
        }

        // Explicit Octave / Half-Time Backbeat Disambiguation:
        // In 4/4 backbeat grooves (e.g. 140-175 BPM), 2-beat lag (70-87.5 BPM) aligns Kick+Snare pairs.
        // If the 1-beat lag (`refined_bpm * 2`) also has strong autocorrelation, promote to true tactus!
        let raw_at_bpm = |b: f32| -> f32 {
            let f_idx = (b - 66.0) * 2.0;
            if f_idx < 0.0 || f_idx >= (NUM_CANDS - 1) as f32 {
                return 0.0;
            }
            let i0 = f_idx.floor() as usize;
            let fr = f_idx - (i0 as f32);
            raw_comb[i0] * (1.0 - fr) + raw_comb[i0 + 1] * fr
        };
        let cur_raw = raw_at_bpm(refined_bpm).max(1e-4);
        if refined_bpm < 88.0 && refined_bpm * 2.0 <= 186.0 {
            let dbl_bpm = refined_bpm * 2.0;
            let dbl = raw_at_bpm(dbl_bpm);
            let dbl_acf = sample_acf(self.hop_hz * 60.0 / dbl_bpm);
            if dbl >= cur_raw * 0.70 || dbl_acf >= 0.42 {
                refined_bpm = dbl_bpm;
            }
        } else if refined_bpm > 168.0 && refined_bpm * 0.5 >= 78.0 {
            let hlf = raw_at_bpm(refined_bpm * 0.5);
            if hlf >= cur_raw * 0.88 {
                refined_bpm *= 0.5;
            }
        }

        // Smooth steady-state tempo lock (prevents wild jitter while locking rapidly onto new songs)
        let diff = (refined_bpm - self.locked_bpm).abs();
        if self.confidence < 0.20 {
            self.locked_bpm = refined_bpm;
            self.confidence = (best_val * 1.4).clamp(0.25, 1.0);
        } else if diff <= 3.8 {
            self.locked_bpm = self.locked_bpm * 0.68 + refined_bpm * 0.32;
            self.confidence = (self.confidence * 0.8 + best_val * 0.45).clamp(0.25, 1.0);
            self.candidate_streak_cnt = 0;
        } else {
            if (refined_bpm - self.candidate_streak_bpm).abs() <= 2.8 {
                self.candidate_streak_cnt = self.candidate_streak_cnt.saturating_add(1);
            } else {
                self.candidate_streak_bpm = refined_bpm;
                self.candidate_streak_cnt = 1;
            }
            if self.candidate_streak_cnt >= 3 {
                self.locked_bpm = refined_bpm;
                self.candidate_streak_cnt = 0;
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct SharedTempoSnapshot {
    bpm: f32,
    beat_phase: f32,
    beat_seq: u64,
}

static SHARED_TEMPO: Lazy<RwLock<SharedTempoSnapshot>> = Lazy::new(|| {
    RwLock::new(SharedTempoSnapshot {
        bpm: 120.0,
        beat_phase: 0.0,
        beat_seq: 0,
    })
});

struct RenderTimeSmoother {
    fft_window_2048: Vec<f32>,
    fft_window_512: [f32; 512],
    mono_2048: Vec<f32>,
    fft_re: Vec<f32>,
    fft_im: Vec<f32>,
    fft_mag: Vec<f32>,
    bands_sr: u32,
    bands_64: [(usize, usize); 64],
    bands_120: [(usize, usize); 120],
    spec_32: [f32; 32],
    peaks_32: [f32; 32],
    spec_64: [f32; 64],
    peaks_64: [f32; 64],
    radial_120: [f32; 120],
    beat_pulse: f32,
    last_seen_beat_seq: u64,
    estimated_bpm: f32,
    beat_phase: f32,
    frozen_snapshot: Option<LiveVisualizerSnapshot>,
}

impl Default for RenderTimeSmoother {
    fn default() -> Self {
        let n = 2048;
        let mut win = vec![0.0f32; n];
        for i in 0..n {
            win[i] = 0.5 * (1.0 - (2.0 * PI * (i as f32) / ((n - 1) as f32)).cos());
        }
        let mut win512 = [0.0f32; 512];
        for (i, w) in win512.iter_mut().enumerate() {
            *w = 0.5 * (1.0 - (2.0 * PI * (i as f32) / 511.0).cos());
        }
        Self {
            fft_window_2048: win,
            fft_window_512: win512,
            mono_2048: vec![0.0; n],
            fft_re: vec![0.0; n],
            fft_im: vec![0.0; n],
            fft_mag: vec![0.0; n / 2],
            bands_sr: 0,
            bands_64: [(1, 2); 64],
            bands_120: [(1, 2); 120],
            spec_32: [0.0; 32],
            peaks_32: [0.0; 32],
            spec_64: [0.0; 64],
            peaks_64: [0.0; 64],
            radial_120: [0.0; 120],
            beat_pulse: 0.0,
            last_seen_beat_seq: 0,
            estimated_bpm: 120.0,
            beat_phase: 0.0,
            frozen_snapshot: None,
        }
    }
}

impl RenderTimeSmoother {
    fn ensure_band_tables(&mut self, sr: u32) {
        if self.bands_sr == sr {
            return;
        }
        self.bands_sr = sr;
        let bin_hz = (sr.max(8_000) as f32) / 2048.0;
        for i in 0..64 {
            let u0 = (i as f32) / 64.0;
            let u1 = ((i + 1) as f32) / 64.0;
            let f0 = 28.0 * (20_000.0f32 / 28.0).powf(u0);
            let f1 = 28.0 * (20_000.0f32 / 28.0).powf(u1);
            let b0 = ((f0 / bin_hz).floor() as usize).clamp(1, 1022);
            let b1 = ((f1 / bin_hz).ceil() as usize).clamp(b0 + 1, 1023);
            self.bands_64[i] = (b0, b1);
        }
        for i in 0..120 {
            let u0 = (i as f32) / 120.0;
            let u1 = ((i + 1) as f32) / 120.0;
            let f0 = 32.0 * (18_500.0f32 / 32.0).powf(u0);
            let f1 = 32.0 * (18_500.0f32 / 32.0).powf(u1);
            let b0 = ((f0 / bin_hz).floor() as usize).clamp(1, 1022);
            let b1 = ((f1 / bin_hz).ceil() as usize).clamp(b0 + 1, 1023);
            self.bands_120[i] = (b0, b1);
        }
    }
}

static RING_STATE: Lazy<RwLock<SharedRingState>> =
    Lazy::new(|| RwLock::new(SharedRingState::default()));
static FORENSIC_SUMMARY: Lazy<RwLock<SharedForensicSummary>> =
    Lazy::new(|| RwLock::new(SharedForensicSummary::default()));
static RENDER_SMOOTHER: Lazy<Mutex<RenderTimeSmoother>> =
    Lazy::new(|| Mutex::new(RenderTimeSmoother::default()));
static LATEST_VIZ: Lazy<RwLock<AudioVizFrame>> =
    Lazy::new(|| RwLock::new(AudioVizFrame::default()));

static VIZ_SUBSCRIBED: AtomicBool = AtomicBool::new(false);
static INSPECTOR_RESET: AtomicBool = AtomicBool::new(false);
static INSPECTOR_PAUSED: AtomicBool = AtomicBool::new(false);
static AUTO_RESET_SILENCE: AtomicBool = AtomicBool::new(true);
static FORCE_REBIND_CAPTURE: AtomicBool = AtomicBool::new(false);
static CAPTURE_TARGET_OVERRIDE: Lazy<RwLock<Option<String>>> = Lazy::new(|| RwLock::new(None));
static WAKE_COND: Lazy<(Mutex<bool>, Condvar)> = Lazy::new(|| (Mutex::new(false), Condvar::new()));
static WAKE_HWND: AtomicUsize = AtomicUsize::new(0);

pub const WM_APP_AUDIO_WAKE: u32 = 0x8000 + 41;

pub fn register_wake_hwnd(hwnd_val: usize) {
    WAKE_HWND.store(hwnd_val, Ordering::Release);
}

fn notify_worker_wake() {
    let (lock, cvar) = &*WAKE_COND;
    if let Ok(mut g) = lock.lock() {
        *g = true;
        cvar.notify_all();
    }
}

pub fn set_viz_target_override(device_id: Option<String>) {
    if let Ok(mut g) = CAPTURE_TARGET_OVERRIDE.write() {
        *g = device_id.filter(|s| !s.trim().is_empty());
    }
    INSPECTOR_RESET.store(true, Ordering::Release);
    FORCE_REBIND_CAPTURE.store(true, Ordering::Release);
    notify_worker_wake();
}

pub fn trigger_inspector_reset() {
    INSPECTOR_RESET.store(true, Ordering::Release);
    notify_worker_wake();
}

pub fn set_inspector_paused(paused: bool) {
    if paused && !INSPECTOR_PAUSED.load(Ordering::Acquire) {
        let snap = sample_live_visualizer(1.0 / 60.0);
        if let Ok(mut sm) = RENDER_SMOOTHER.lock() {
            sm.frozen_snapshot = Some(snap);
        }
    }
    INSPECTOR_PAUSED.store(paused, Ordering::Release);
    if !paused {
        if let Ok(mut sm) = RENDER_SMOOTHER.lock() {
            sm.frozen_snapshot = None;
        }
        notify_worker_wake();
    }
}

pub fn is_inspector_paused() -> bool {
    INSPECTOR_PAUSED.load(Ordering::Acquire)
}

pub fn set_auto_reset_silence(enabled: bool) {
    AUTO_RESET_SILENCE.store(enabled, Ordering::Release);
}

pub fn is_auto_reset_silence() -> bool {
    AUTO_RESET_SILENCE.load(Ordering::Acquire)
}

pub fn set_viz_subscribed(enabled: bool) {
    let prev = VIZ_SUBSCRIBED.swap(enabled, Ordering::AcqRel);
    if enabled {
        ensure_loopback_worker();
        if !prev {
            notify_worker_wake();
        }
    }
}

pub fn is_viz_subscribed() -> bool {
    VIZ_SUBSCRIBED.load(Ordering::Acquire)
}

static AMP_AGENT_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static AMP_AWAITING_ONSET: AtomicBool = AtomicBool::new(false);
static AMP_ONSET_EPOCH: AtomicU64 = AtomicU64::new(0);
static AMP_ONSET_DETECTED_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Resets stale `AMPLibraryAgent.exe` session activity and arms a 25ms `IAudioMeterInformation`
/// onset watch on the default render endpoint for the newly starting track.
pub fn reset_amp_agent_audio_onset() -> u64 {
    let ep = AMP_ONSET_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    AMP_AGENT_SESSION_ACTIVE.store(false, Ordering::Release);
    AMP_AWAITING_ONSET.store(true, Ordering::Release);
    ensure_loopback_worker();
    notify_worker_wake();
    ep
}

pub fn clear_amp_agent_audio_onset_watch() {
    AMP_AWAITING_ONSET.store(false, Ordering::Release);
}

#[inline]
pub fn has_amp_agent_audio_onset_since(min_epoch: u64) -> bool {
    min_epoch > 0
        && AMP_ONSET_DETECTED_EPOCH.load(Ordering::Acquire) >= min_epoch
        && AMP_AGENT_SESSION_ACTIVE.load(Ordering::Acquire)
}

pub fn is_audio_currently_active() -> bool {
    RING_STATE.read().map(|r| r.active_signal).unwrap_or(false)
}

/// Returns `true` specifically when `AMPLibraryAgent.exe` is actively emitting non-silent WASAPI audio.
pub fn is_amp_agent_audio_active() -> bool {
    if !AMP_AGENT_SESSION_ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    if VIZ_SUBSCRIBED.load(Ordering::Acquire) {
        is_audio_currently_active()
    } else {
        true
    }
}

pub fn get_latest_viz_frame() -> AudioVizFrame {
    ensure_loopback_worker();
    LATEST_VIZ.read().map(|f| f.clone()).unwrap_or_default()
}

/// Ultra-lightweight zero-heap-allocation rhythm sampler (<0.015ms CPU) for non-Visualizer pages
/// (drives sidebar `可视化` rhythm icon, track-row equalizer bars, and Lyrics bass breathing).
pub fn sample_rhythm_levels_fast(dt_sec: f32) -> (bool, f32, f32, f32, f32, [f32; 32]) {
    ensure_loopback_worker();
    let dt = dt_sec.clamp(1.0 / 360.0, 0.08);
    let Ok(mut sm) = RENDER_SMOOTHER.lock() else {
        return (false, 0.0, 0.0, 0.0, 0.0, [0.0; 32]);
    };

    let mut re = [0.0f32; 512];
    let mut im = [0.0f32; 512];
    let (active_signal, sr) = {
        let Ok(ring) = RING_STATE.read() else {
            return (false, 0.0, 0.0, 0.0, 0.0, sm.spec_32);
        };
        if ring.active_signal {
            let start = (ring.write_pos + RING_LEN - 512) & RING_MASK;
            for i in 0..512 {
                let lr = ring.stereo[(start + i) & RING_MASK];
                re[i] = 0.5 * (lr[0] + lr[1]) * sm.fft_window_512[i];
            }
        }
        (ring.active_signal, ring.sample_rate.max(8_000))
    };

    let attack = 1.0 - (-28.0 * dt).exp();
    let release = 1.0 - (-9.5 * dt).exp();

    if active_signal {
        fft_radix2_in_place(&mut re, &mut im);
        let sr_f = sr as f32;
        let nyq = sr_f * 0.5;
        let bin_hz = sr_f / 512.0;
        let norm = 2.0 / 256.0;
        let mut mag = [0.0f32; 256];
        for k in 1..256 {
            mag[k] = (re[k] * re[k] + im[k] * im[k]).sqrt() * norm;
        }
        for i in 0..32 {
            let fc = SPECTRUM_FREQS[i];
            let raw = if fc < nyq * 0.98 {
                let b0 = (((fc * 0.86) / bin_hz).floor() as usize).clamp(1, 254);
                let b1 = (((fc * 1.16) / bin_hz).ceil() as usize).clamp(b0 + 1, 255);
                let mut pk = 0.0f32;
                for &m in &mag[b0..b1] {
                    if m > pk {
                        pk = m;
                    }
                }
                let tilt = 1.0 + (i as f32) * 0.055;
                let db = 20.0 * ((pk * tilt).max(1e-5)).log10();
                ((db + 58.0) / 52.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let k = if raw > sm.spec_32[i] { attack } else { release };
            sm.spec_32[i] += (raw - sm.spec_32[i]) * k;
        }
        if let Ok(ts) = SHARED_TEMPO.read() {
            if ts.beat_seq != sm.last_seen_beat_seq {
                sm.last_seen_beat_seq = ts.beat_seq;
                sm.beat_pulse = 1.0;
            } else {
                sm.beat_pulse *= (-8.5 * dt).exp();
            }
        } else {
            sm.beat_pulse *= (-8.5 * dt).exp();
        }
    } else {
        let decay = (-9.0 * dt).exp();
        for v in &mut sm.spec_32 {
            *v *= decay;
        }
        sm.beat_pulse *= decay;
    }

    let bass = sm.spec_32[0..7].iter().sum::<f32>() / 7.0;
    let mid = sm.spec_32[8..20].iter().sum::<f32>() / 12.0;
    let high = sm.spec_32[20..32].iter().sum::<f32>() / 12.0;
    (active_signal, bass, mid, high, sm.beat_pulse, sm.spec_32)
}

/// Samples the high-speed stereo ring buffer at the exact render frame tick (60Hz .. VSync)
pub fn sample_live_visualizer(dt_sec: f32) -> LiveVisualizerSnapshot {
    ensure_loopback_worker();
    let dt = dt_sec.clamp(1.0 / 360.0, 0.08);
    let paused = INSPECTOR_PAUSED.load(Ordering::Acquire);

    let Ok(mut sm) = RENDER_SMOOTHER.lock() else {
        return LiveVisualizerSnapshot::default();
    };

    if paused {
        if let Some(ref frozen) = sm.frozen_snapshot {
            let mut copy = frozen.clone();
            copy.paused = true;
            return copy;
        }
    }

    let (
        active_signal,
        dev_id,
        dev_name,
        sr,
        bd,
        ch,
        pk_l,
        pk_r,
        rms_db,
        dr_db,
        corr,
        width_pct,
        wave_l,
        wave_r,
        stage_lr,
    ) = {
        let Ok(ring) = RING_STATE.read() else {
            return LiveVisualizerSnapshot::default();
        };
        let wpos = ring.write_pos;
        if ring.active_signal {
            let start = (wpos + RING_LEN - 2048) & RING_MASK;
            for i in 0..2048 {
                let lr = ring.stereo[(start + i) & RING_MASK];
                sm.mono_2048[i] = 0.5 * (lr[0] + lr[1]);
            }
        }

        let mut wl = vec![0.0f32; 192];
        let mut wr = vec![0.0f32; 192];
        let mut st_lr = vec![[0.0f32; 2]; 256];
        if ring.active_signal {
            let w_start = (wpos + RING_LEN - 768) & RING_MASK;
            for i in 0..192 {
                let mut bl = 0.0f32;
                let mut br = 0.0f32;
                for k in 0..4 {
                    let lr = ring.stereo[(w_start + i * 4 + k) & RING_MASK];
                    if lr[0].abs() > bl.abs() {
                        bl = lr[0];
                    }
                    if lr[1].abs() > br.abs() {
                        br = lr[1];
                    }
                }
                wl[i] = bl.clamp(-1.0, 1.0);
                wr[i] = br.clamp(-1.0, 1.0);
            }
            let s_start = (wpos + RING_LEN - 512) & RING_MASK;
            for i in 0..256 {
                let lr = ring.stereo[(s_start + i * 2) & RING_MASK];
                st_lr[i] = [lr[0].clamp(-1.0, 1.0), lr[1].clamp(-1.0, 1.0)];
            }
        }

        (
            ring.active_signal,
            ring.device_id.clone(),
            ring.device_name.clone(),
            ring.sample_rate.max(8_000),
            ring.bit_depth,
            ring.channels,
            ring.peak_db_l,
            ring.peak_db_r,
            ring.rms_db,
            ring.dynamic_range_db,
            ring.phase_correlation,
            ring.stereo_width_pct,
            wl,
            wr,
            st_lr,
        )
    };

    // Frame-rate independent exponential smoothing constants
    let attack = 1.0 - (-28.0 * dt).exp();
    let release = 1.0 - (-9.5 * dt).exp();
    let peak_fall = 0.52 * dt;
    let mut beat_triggered = false;

    if active_signal {
        sm.ensure_band_tables(sr);
        for i in 0..2048 {
            sm.fft_re[i] = sm.mono_2048[i] * sm.fft_window_2048[i];
            sm.fft_im[i] = 0.0;
        }
        let RenderTimeSmoother {
            ref mut fft_re,
            ref mut fft_im,
            ref mut fft_mag,
            ..
        } = *sm;
        fft_radix2_in_place(fft_re, fft_im);

        let sr_f = sr as f32;
        let nyq = sr_f * 0.5;
        let bin_hz = sr_f / 2048.0;
        let norm = 2.0 / 1024.0;
        fft_mag[0] = 0.0;
        for k in 1..1024 {
            fft_mag[k] = (fft_re[k] * fft_re[k] + fft_im[k] * fft_im[k]).sqrt() * norm;
        }

        let peak_in_bins = |mag: &[f32], b0: usize, b1: usize, tilt: f32| -> f32 {
            let mut pk = 0.0f32;
            for &m in &mag[b0..b1] {
                if m > pk {
                    pk = m;
                }
            }
            let db = 20.0 * ((pk * tilt).max(1e-5)).log10();
            ((db + 58.0) / 52.0).clamp(0.0, 1.0)
        };

        // 32-band spectrum
        for i in 0..32 {
            let fc = SPECTRUM_FREQS[i];
            let raw = if fc < nyq * 0.98 {
                let b0 = (((fc * 0.86) / bin_hz).floor() as usize).clamp(1, 1022);
                let b1 = (((fc * 1.16) / bin_hz).ceil() as usize).clamp(b0 + 1, 1023);
                peak_in_bins(&sm.fft_mag, b0, b1, 1.0 + (i as f32) * 0.055)
            } else {
                0.0
            };
            let k = if raw > sm.spec_32[i] { attack } else { release };
            sm.spec_32[i] += (raw - sm.spec_32[i]) * k;
            if sm.spec_32[i] >= sm.peaks_32[i] {
                sm.peaks_32[i] = sm.spec_32[i];
            } else {
                sm.peaks_32[i] = (sm.peaks_32[i] - peak_fall).max(0.0);
            }
        }

        // 64-band log spectrum (precomputed bins!)
        for i in 0..64 {
            let (b0, b1) = sm.bands_64[i];
            let raw = if (b0 as f32) * bin_hz < nyq * 0.98 {
                peak_in_bins(&sm.fft_mag, b0, b1, 1.0 + (i as f32) * 0.032)
            } else {
                0.0
            };
            let k = if raw > sm.spec_64[i] { attack } else { release };
            sm.spec_64[i] += (raw - sm.spec_64[i]) * k;
            if sm.spec_64[i] >= sm.peaks_64[i] {
                sm.peaks_64[i] = sm.spec_64[i];
            } else {
                sm.peaks_64[i] = (sm.peaks_64[i] - peak_fall * 0.82).max(0.0);
            }
        }

        // 120-band radial spectrum (precomputed bins!)
        for i in 0..120 {
            let (b0, b1) = sm.bands_120[i];
            let raw = peak_in_bins(&sm.fft_mag, b0, b1, 1.0 + (i as f32) * 0.022);
            let k = if raw > sm.radial_120[i] {
                (attack * 1.15).min(1.0)
            } else {
                release
            };
            sm.radial_120[i] += (raw - sm.radial_120[i]) * k;
        }

        // Read sample-clocked Autocorrelation + Comb-Filter TempoTracker state
        if let Ok(ts) = SHARED_TEMPO.read() {
            sm.estimated_bpm = ts.bpm;
            sm.beat_phase = ts.beat_phase;
            if ts.beat_seq != sm.last_seen_beat_seq {
                sm.last_seen_beat_seq = ts.beat_seq;
                beat_triggered = true;
                sm.beat_pulse = 1.0;
            } else {
                sm.beat_pulse *= (-8.5 * dt).exp();
            }
        } else {
            sm.beat_pulse *= (-8.5 * dt).exp();
        }
    } else {
        let decay = (-9.0 * dt).exp();
        for v in &mut sm.spec_32 {
            *v *= decay;
        }
        for v in &mut sm.peaks_32 {
            *v = (*v - peak_fall).max(0.0);
        }
        for v in &mut sm.spec_64 {
            *v *= decay;
        }
        for v in &mut sm.peaks_64 {
            *v = (*v - peak_fall).max(0.0);
        }
        for v in &mut sm.radial_120 {
            *v *= decay;
        }
        sm.beat_pulse *= decay;
    }

    let bass_energy = sm.spec_32[0..7].iter().sum::<f32>() / 7.0;
    let mid_energy = sm.spec_32[8..20].iter().sum::<f32>() / 12.0;
    let high_energy = sm.spec_32[20..32].iter().sum::<f32>() / 12.0;

    let forensic = FORENSIC_SUMMARY.read().map(|f| f.clone()).unwrap_or_default();

    let snap = LiveVisualizerSnapshot {
        active_signal,
        paused,
        device_id: dev_id,
        device_name: dev_name,
        sample_rate: sr,
        bit_depth: bd,
        channels: ch,
        nyquist_hz: (sr as f32) * 0.5,
        spectrum_32: sm.spec_32,
        peaks_32: sm.peaks_32,
        spectrum_64: sm.spec_64,
        peaks_64: sm.peaks_64,
        radial_120: sm.radial_120,
        wave_l,
        wave_r,
        soundstage_lr: stage_lr,
        inst_spectrum_db: forensic.inst_spectrum_db,
        peak_envelope_db: forensic.peak_envelope_db,
        tier_code: forensic.tier_code,
        verdict_title: forensic.verdict_title,
        estimated_bitrate: forensic.estimated_bitrate,
        confidence_pct: forensic.confidence_pct,
        forensic_notes: forensic.forensic_notes,
        cutoff_hz: forensic.cutoff_hz,
        inst_cutoff_hz: forensic.inst_cutoff_hz,
        brickwall_drop_db: forensic.brickwall_drop_db,
        has_brickwall_cut: forensic.has_brickwall_cut,
        mdct_hole_pct: forensic.mdct_hole_pct,
        detected_bit_grid: forensic.detected_bit_grid,
        clip_count: forensic.clip_count,
        analyzed_frames: forensic.analyzed_frames,
        auto_reset_silence: AUTO_RESET_SILENCE.load(Ordering::Acquire),
        active_apps: forensic.active_apps,
        peak_db_l: pk_l,
        peak_db_r: pk_r,
        rms_db,
        dynamic_range_db: dr_db,
        phase_correlation: corr,
        stereo_width_pct: width_pct,
        bass_energy,
        mid_energy,
        high_energy,
        beat_pulse: sm.beat_pulse,
        beat_phase: sm.beat_phase,
        beat_triggered,
        estimated_bpm: sm.estimated_bpm,
    };

    if paused {
        let mut copy = snap.clone();
        copy.paused = true;
        sm.frozen_snapshot = Some(snap);
        return copy;
    }
    snap
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};

    type HRESULT = i32;
    type ULONG = u32;
    type HANDLE = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct GUID {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

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
    const IID_ISPATIAL_AUDIO_CLIENT: GUID = GUID {
        data1: 0xBBF8_E066,
        data2: 0xAAAA,
        data3: 0x49BE,
        data4: [0x9A, 0x4D, 0xFD, 0x2A, 0x85, 0x8E, 0xA2, 0x7F],
    };
    const IID_IAUDIO_SESSION_MANAGER2: GUID = GUID {
        data1: 0x77AA_99A0,
        data2: 0x1BD6,
        data3: 0x484F,
        data4: [0x8B, 0xC7, 0x2C, 0x65, 0x4C, 0x9A, 0x9B, 0x6F],
    };
    const IID_IAUDIO_SESSION_CONTROL2: GUID = GUID {
        data1: 0xBFB7_FF88,
        data2: 0x7239,
        data3: 0x4FC9,
        data4: [0x8F, 0xA2, 0x07, 0xC9, 0x50, 0xBE, 0x9C, 0x6D],
    };
    const IID_IAUDIO_METER_INFORMATION: GUID = GUID {
        data1: 0xC022_16F6,
        data2: 0x8C67,
        data3: 0x4B5B,
        data4: [0x9D, 0x00, 0xD0, 0x08, 0xE7, 0x3E, 0x00, 0x64],
    };
    const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID {
        data1: 0x870A_F99C,
        data2: 0x171D,
        data3: 0x4F9E,
        data4: [0xAF, 0x0D, 0xE6, 0x3D, 0xF4, 0x0C, 0x2B, 0xC9],
    };
    const IID_IPOLICY_CONFIG: GUID = GUID {
        data1: 0xF867_9F50,
        data2: 0x850A,
        data3: 0x41CF,
        data4: [0x9C, 0x72, 0x43, 0x0F, 0x29, 0x02, 0x90, 0xC8],
    };
    const IID_IPOLICY_CONFIG_VISTA: GUID = GUID {
        data1: 0x568B_9108,
        data2: 0x44BF,
        data3: 0x40B4,
        data4: [0x90, 0x06, 0x86, 0xAF, 0xE5, 0xB5, 0xA6, 0x20],
    };
    const SUBTYPE_IEEE_FLOAT: GUID = GUID {
        data1: 0x0000_0003,
        data2: 0x0000,
        data3: 0x0010,
        data4: [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    };

    #[repr(C)]
    struct PROPERTYKEY {
        fmtid: GUID,
        pid: u32,
    }

    const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID {
            data1: 0xA45C_254E,
            data2: 0xDF1C,
            data3: 0x4EFD,
            data4: [0x80, 0x20, 0x67, 0xD1, 0x46, 0xA8, 0x50, E0],
        },
        pid: 14,
    };

    const E0: u8 = 0xE0;

    #[repr(C)]
    struct PROPVARIANT {
        vt: u16,
        w_reserved1: u16,
        w_reserved2: u16,
        w_reserved3: u16,
        data1: usize,
        data2: usize,
    }

    #[repr(C, packed(1))]
    #[derive(Clone, Copy)]
    struct WAVEFORMATEX {
        w_format_tag: u16,
        n_channels: u16,
        n_samples_per_sec: u32,
        n_avg_bytes_per_sec: u32,
        n_block_align: u16,
        w_bits_per_sample: u16,
        cb_size: u16,
    }

    #[repr(C, packed(1))]
    #[derive(Clone, Copy)]
    struct WAVEFORMATEXTENSIBLE {
        format: WAVEFORMATEX,
        w_valid_bits_per_sample: u16,
        dw_channel_mask: u32,
        sub_format: GUID,
    }

    #[repr(C)]
    struct IO_COUNTERS {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_coinit: u32) -> HRESULT;
        fn CoCreateInstance(
            rclsid: *const GUID,
            p_unk_outer: *mut c_void,
            dw_cls_context: u32,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> HRESULT;
        fn CoTaskMemFree(pv: *mut c_void);
        fn PropVariantClear(pvar: *mut PROPVARIANT) -> HRESULT;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> HANDLE;
        fn CloseHandle(h_object: HANDLE) -> i32;
        fn QueryFullProcessImageNameW(
            h_process: HANDLE,
            dw_flags: u32,
            lp_exe_name: *mut u16,
            lpdw_size: *mut u32,
        ) -> i32;
        fn GetProcessIoCounters(h_process: HANDLE, lp_io_counters: *mut IO_COUNTERS) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn PostMessageW(h_wnd: *mut c_void, msg: u32, w_param: usize, l_param: isize) -> i32;
    }

    unsafe fn pwstr_to_string(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }

    unsafe fn release_com(unk: *mut c_void) {
        if !unk.is_null() {
            let vtbl = *(unk as *const *const *const c_void);
            let rel: unsafe extern "system" fn(*mut c_void) -> ULONG =
                std::mem::transmute(*(vtbl.add(2)));
            let _ = rel(unk);
        }
    }

    unsafe fn read_device_id(dev: *mut c_void) -> String {
        let vtbl = *(dev as *const *const *const c_void);
        let get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> HRESULT =
            std::mem::transmute(*(vtbl.add(5)));
        let mut raw_id: *mut u16 = null_mut();
        if get_id(dev, &mut raw_id) >= 0 && !raw_id.is_null() {
            let s = pwstr_to_string(raw_id);
            CoTaskMemFree(raw_id as *mut c_void);
            s
        } else {
            String::new()
        }
    }

    unsafe fn read_device_friendly_name(dev: *mut c_void) -> String {
        let vtbl = *(dev as *const *const *const c_void);
        let open_props: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(*(vtbl.add(4)));
        let mut store: *mut c_void = null_mut();
        if open_props(dev, 0, &mut store) < 0 || store.is_null() {
            return "音频输出设备".to_string();
        }
        let s_vtbl = *(store as *const *const *const c_void);
        let get_val: unsafe extern "system" fn(
            *mut c_void,
            *const PROPERTYKEY,
            *mut PROPVARIANT,
        ) -> HRESULT = std::mem::transmute(*(s_vtbl.add(5)));
        let mut pv: PROPVARIANT = zeroed();
        let mut name = "音频输出设备".to_string();
        if get_val(store, &PKEY_DEVICE_FRIENDLY_NAME, &mut pv) >= 0 && pv.vt == 31 && pv.data1 != 0
        {
            name = pwstr_to_string(pv.data1 as *const u16);
        }
        let _ = PropVariantClear(&mut pv);
        release_com(store);
        name
    }

    unsafe fn read_device_mix_format(dev: *mut c_void) -> (u32, u16, u16, bool) {
        let vtbl = *(dev as *const *const *const c_void);
        let activate: unsafe extern "system" fn(
            *mut c_void,
            *const GUID,
            u32,
            *mut c_void,
            *mut *mut c_void,
        ) -> HRESULT = std::mem::transmute(*(vtbl.add(3)));
        let mut client: *mut c_void = null_mut();
        if activate(dev, &IID_IAUDIO_CLIENT, 0x17, null_mut(), &mut client) < 0 || client.is_null()
        {
            return (48_000, 24, 2, true);
        }
        let c_vtbl = *(client as *const *const *const c_void);
        let get_mix: unsafe extern "system" fn(*mut c_void, *mut *mut WAVEFORMATEX) -> HRESULT =
            std::mem::transmute(*(c_vtbl.add(8)));
        let mut fmt_ptr: *mut WAVEFORMATEX = null_mut();
        let mut out = (48_000u32, 32u16, 2u16, true);
        if get_mix(client, &mut fmt_ptr) >= 0 && !fmt_ptr.is_null() {
            let wf = std::ptr::read_unaligned(fmt_ptr);
            let mut bits = wf.w_bits_per_sample;
            let mut is_float = wf.w_format_tag == 3;
            if wf.w_format_tag == 0xFFFE && wf.cb_size >= 22 {
                let ext = std::ptr::read_unaligned(fmt_ptr as *const WAVEFORMATEXTENSIBLE);
                if ext.w_valid_bits_per_sample > 0 {
                    bits = ext.w_valid_bits_per_sample;
                }
                let sub = ext.sub_format;
                is_float = sub == SUBTYPE_IEEE_FLOAT;
            }
            out = (wf.n_samples_per_sec, bits, wf.n_channels, is_float);
            CoTaskMemFree(fmt_ptr as *mut c_void);
        }
        release_com(client);
        out
    }

    unsafe fn read_device_spatial_support(dev: *mut c_void, mix_channels: u16) -> bool {
        if mix_channels > 2 {
            return true;
        }
        let vtbl = *(dev as *const *const *const c_void);
        let activate: unsafe extern "system" fn(
            *mut c_void,
            *const GUID,
            u32,
            *mut c_void,
            *mut *mut c_void,
        ) -> HRESULT = std::mem::transmute(*(vtbl.add(3)));
        let mut sa_client: *mut c_void = null_mut();
        if activate(dev, &IID_ISPATIAL_AUDIO_CLIENT, 0x17, null_mut(), &mut sa_client) < 0
            || sa_client.is_null()
        {
            return false;
        }
        let sa_vtbl = *(sa_client as *const *const *const c_void);
        let get_max_dyn: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
            std::mem::transmute(*(sa_vtbl.add(5)));
        let mut max_dyn: u32 = 0;
        let ok = get_max_dyn(sa_client, &mut max_dyn) >= 0 && max_dyn > 0;
        release_com(sa_client);
        ok
    }

    pub fn is_default_endpoint_spatial_ready() -> bool {
        unsafe {
            let _ = CoInitializeEx(null_mut(), 0);
            let _ = crate::am_agent::init_process_com_security();
            let mut enumerator: *mut c_void = null_mut();
            if CoCreateInstance(
                &CLSID_MMDEVICE_ENUMERATOR,
                null_mut(),
                0x17,
                &IID_IMMDEVICE_ENUMERATOR,
                &mut enumerator,
            ) < 0
                || enumerator.is_null()
            {
                return false;
            }
            let e_vtbl = *(enumerator as *const *const *const c_void);
            let get_default: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(e_vtbl.add(4)));
            let mut def_dev: *mut c_void = null_mut();
            let mut ready = false;
            if get_default(enumerator, 0, 1, &mut def_dev) >= 0 && !def_dev.is_null() {
                let (_, _, ch, _) = read_device_mix_format(def_dev);
                ready = read_device_spatial_support(def_dev, ch);
                release_com(def_dev);
            }
            release_com(enumerator);
            ready
        }
    }

    pub fn list_output_devices() -> Result<Vec<AudioOutputDevice>, String> {
        unsafe {
            let _ = CoInitializeEx(null_mut(), 0);
            let _ = crate::am_agent::init_process_com_security();
            let mut enumerator: *mut c_void = null_mut();
            let hr = CoCreateInstance(
                &CLSID_MMDEVICE_ENUMERATOR,
                null_mut(),
                0x17,
                &IID_IMMDEVICE_ENUMERATOR,
                &mut enumerator,
            );
            if hr < 0 || enumerator.is_null() {
                return Err(format!("初始化音频设备枚举器失败: 0x{:08X}", hr as u32));
            }

            let e_vtbl = *(enumerator as *const *const *const c_void);
            let get_default: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(e_vtbl.add(4)));
            let enum_eps: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(e_vtbl.add(3)));

            let mut def_dev: *mut c_void = null_mut();
            let mut default_id = String::new();
            if get_default(enumerator, 0, 1, &mut def_dev) >= 0 && !def_dev.is_null() {
                default_id = read_device_id(def_dev);
                release_com(def_dev);
            }

            let mut collection: *mut c_void = null_mut();
            let hr_list = enum_eps(enumerator, 0, 1, &mut collection);
            if hr_list < 0 || collection.is_null() {
                release_com(enumerator);
                return Err(format!("枚举输出设备失败: 0x{:08X}", hr_list as u32));
            }

            let c_vtbl = *(collection as *const *const *const c_void);
            let get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(3)));
            let get_item: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(4)));

            let mut count: u32 = 0;
            let _ = get_count(collection, &mut count);

            let mut devices = Vec::with_capacity(count as usize);
            for idx in 0..count {
                let mut dev: *mut c_void = null_mut();
                if get_item(collection, idx, &mut dev) >= 0 && !dev.is_null() {
                    let id = read_device_id(dev);
                    let name = read_device_friendly_name(dev);
                    let (sr, bits, ch, is_float) = read_device_mix_format(dev);
                    let spatial_audio_active = read_device_spatial_support(dev, ch);
                    let is_default = !id.is_empty() && id == default_id;
                    devices.push(AudioOutputDevice {
                        id,
                        name,
                        is_default,
                        sample_rate: sr,
                        bit_depth: bits,
                        channels: ch,
                        format_tag: if is_float {
                            "IEEE Float".to_string()
                        } else {
                            "PCM".to_string()
                        },
                        spatial_audio_active,
                    });
                    release_com(dev);
                }
            }

            release_com(collection);
            release_com(enumerator);
            Ok(devices)
        }
    }

    pub fn set_output_device(device_id: &str) -> Result<Vec<AudioOutputDevice>, String> {
        let trimmed = device_id.trim();
        if trimmed.is_empty() {
            return Err("设备 ID 不能为空".to_string());
        }
        unsafe {
            let _ = CoInitializeEx(null_mut(), 0);
            let _ = crate::am_agent::init_process_com_security();
            let mut policy: *mut c_void = null_mut();
            let mut hr = CoCreateInstance(
                &CLSID_POLICY_CONFIG_CLIENT,
                null_mut(),
                0x17,
                &IID_IPOLICY_CONFIG,
                &mut policy,
            );
            if hr < 0 || policy.is_null() {
                hr = CoCreateInstance(
                    &CLSID_POLICY_CONFIG_CLIENT,
                    null_mut(),
                    0x17,
                    &IID_IPOLICY_CONFIG_VISTA,
                    &mut policy,
                );
            }
            if hr < 0 || policy.is_null() {
                return Err(format!("创建系统音频路由控制器失败: 0x{:08X}", hr as u32));
            }

            let p_vtbl = *(policy as *const *const *const c_void);
            let set_def: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> HRESULT =
                std::mem::transmute(*(p_vtbl.add(13)));

            let mut wide: Vec<u16> = trimmed.encode_utf16().collect();
            wide.push(0);

            let hr_console = set_def(policy, wide.as_ptr(), 0);
            let hr_media = set_def(policy, wide.as_ptr(), 1);
            release_com(policy);

            if hr_console < 0 && hr_media < 0 {
                return Err(format!("切换音频播放设备失败: 0x{:08X}", hr_media as u32));
            }
        }

        INSPECTOR_RESET.store(true, Ordering::Release);
        FORCE_REBIND_CAPTURE.store(true, Ordering::Release);
        notify_worker_wake();
        list_output_devices()
    }

    unsafe fn query_process_name_and_io(
        pid: u32,
        io_hist: &mut HashMap<u32, (u64, Instant)>,
    ) -> (String, u32) {
        if pid == 0 {
            return ("系统声音 (System)".to_string(), 0);
        }
        let h = OpenProcess(0x1000, 0, pid);
        if h.is_null() {
            return (format!("PID {pid}"), 0);
        }
        let mut buf = [0u16; 260];
        let mut sz = buf.len() as u32;
        let exe_name = if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut sz) != 0 && sz > 0
        {
            let full = String::from_utf16_lossy(&buf[..(sz as usize)]);
            full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string()
        } else {
            format!("PID {pid}")
        };

        let mut io: IO_COUNTERS = zeroed();
        let mut kbps = 0u32;
        if GetProcessIoCounters(h, &mut io) != 0 {
            let total_bytes = io.read_transfer_count.saturating_add(io.other_transfer_count);
            let now = Instant::now();
            if let Some((prev_bytes, prev_at)) = io_hist.insert(pid, (total_bytes, now)) {
                let dt = now.duration_since(prev_at).as_secs_f64();
                if dt > 0.15 && total_bytes >= prev_bytes {
                    let bits_per_sec = ((total_bytes - prev_bytes) as f64 * 8.0) / dt;
                    kbps = ((bits_per_sec / 1000.0).round() as u32).min(99_999);
                }
            }
        }
        let _ = CloseHandle(h);
        (exe_name, kbps)
    }

    unsafe fn query_active_audio_apps_on_device(
        dev_id: &str,
        io_hist: &mut HashMap<u32, (u64, Instant)>,
    ) -> Vec<ActiveAudioApp> {
        let mut out = Vec::new();
        let mut enumerator: *mut c_void = null_mut();
        if CoCreateInstance(
            &CLSID_MMDEVICE_ENUMERATOR,
            null_mut(),
            0x17,
            &IID_IMMDEVICE_ENUMERATOR,
            &mut enumerator,
        ) < 0
            || enumerator.is_null()
        {
            return out;
        }
        let e_vtbl = *(enumerator as *const *const *const c_void);
        let get_device: unsafe extern "system" fn(
            *mut c_void,
            *const u16,
            *mut *mut c_void,
        ) -> HRESULT = std::mem::transmute(*(e_vtbl.add(5)));
        let mut wide: Vec<u16> = dev_id.encode_utf16().collect();
        wide.push(0);
        let mut dev: *mut c_void = null_mut();
        if get_device(enumerator, wide.as_ptr(), &mut dev) < 0 || dev.is_null() {
            release_com(enumerator);
            return out;
        }
        release_com(enumerator);

        let d_vtbl = *(dev as *const *const *const c_void);
        let activate: unsafe extern "system" fn(
            *mut c_void,
            *const GUID,
            u32,
            *mut c_void,
            *mut *mut c_void,
        ) -> HRESULT = std::mem::transmute(*(d_vtbl.add(3)));
        let mut mgr2: *mut c_void = null_mut();
        let hr_m = activate(dev, &IID_IAUDIO_SESSION_MANAGER2, 0x17, null_mut(), &mut mgr2);
        release_com(dev);
        if hr_m < 0 || mgr2.is_null() {
            return out;
        }

        let m_vtbl = *(mgr2 as *const *const *const c_void);
        let get_enum: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(*(m_vtbl.add(5)));
        let mut sess_enum: *mut c_void = null_mut();
        if get_enum(mgr2, &mut sess_enum) < 0 || sess_enum.is_null() {
            release_com(mgr2);
            return out;
        }

        let se_vtbl = *(sess_enum as *const *const *const c_void);
        let get_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT =
            std::mem::transmute(*(se_vtbl.add(3)));
        let get_sess: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(*(se_vtbl.add(4)));

        let mut count: i32 = 0;
        let _ = get_count(sess_enum, &mut count);
        let my_pid = GetCurrentProcessId();

        for i in 0..count {
            let mut ctrl: *mut c_void = null_mut();
            if get_sess(sess_enum, i, &mut ctrl) < 0 || ctrl.is_null() {
                continue;
            }
            let c_vtbl = *(ctrl as *const *const *const c_void);
            let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
                std::mem::transmute(*c_vtbl);
            let get_state: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT =
                std::mem::transmute(*(c_vtbl.add(3)));

            let mut state: i32 = 0;
            let _ = get_state(ctrl, &mut state);

            let mut pid: u32 = 0;
            let mut ctrl2: *mut c_void = null_mut();
            if qi(ctrl, &IID_IAUDIO_SESSION_CONTROL2, &mut ctrl2) >= 0 && !ctrl2.is_null() {
                let c2_vtbl = *(ctrl2 as *const *const *const c_void);
                let get_pid: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                    std::mem::transmute(*(c2_vtbl.add(14)));
                let _ = get_pid(ctrl2, &mut pid);
                release_com(ctrl2);
            }

            let mut peak: f32 = 0.0;
            let mut meter: *mut c_void = null_mut();
            if qi(ctrl, &IID_IAUDIO_METER_INFORMATION, &mut meter) >= 0 && !meter.is_null() {
                let mt_vtbl = *(meter as *const *const *const c_void);
                let get_peak: unsafe extern "system" fn(*mut c_void, *mut f32) -> HRESULT =
                    std::mem::transmute(*(mt_vtbl.add(3)));
                let _ = get_peak(meter, &mut peak);
                release_com(meter);
            }
            release_com(ctrl);

            if pid != my_pid && (state == 1 || peak > 0.0005) {
                let peak_db = (20.0 * peak.max(1e-5).log10()).clamp(-96.0, 0.0);
                let (exe_name, io_kbps) = query_process_name_and_io(pid, io_hist);
                if let Some(existing) = out.iter_mut().find(|a: &&mut ActiveAudioApp| a.pid == pid) {
                    if peak_db > existing.peak_db {
                        existing.peak_db = (peak_db * 10.0).round() / 10.0;
                    }
                } else {
                    out.push(ActiveAudioApp {
                        pid,
                        exe_name,
                        peak_db: (peak_db * 10.0).round() / 10.0,
                        io_kbps,
                    });
                }
            }
        }

        release_com(sess_enum);
        release_com(mgr2);
        out.sort_by(|a, b| b.peak_db.partial_cmp(&a.peak_db).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    unsafe fn query_amp_agent_meter_peak_default_endpoint(
        pid_is_amp: &mut HashMap<u32, bool>,
    ) -> f32 {
        let mut enumerator: *mut c_void = null_mut();
        if CoCreateInstance(
            &CLSID_MMDEVICE_ENUMERATOR,
            null_mut(),
            0x17,
            &IID_IMMDEVICE_ENUMERATOR,
            &mut enumerator,
        ) < 0
            || enumerator.is_null()
        {
            return 0.0;
        }
        let e_vtbl = *(enumerator as *const *const *const c_void);
        let get_default: unsafe extern "system" fn(
            *mut c_void,
            u32,
            u32,
            *mut *mut c_void,
        ) -> HRESULT = std::mem::transmute(*(e_vtbl.add(4)));

        let mut dev: *mut c_void = null_mut();
        if get_default(enumerator, 0, 1, &mut dev) < 0 || dev.is_null() {
            release_com(enumerator);
            return 0.0;
        }
        release_com(enumerator);

        let d_vtbl = *(dev as *const *const *const c_void);
        let activate: unsafe extern "system" fn(
            *mut c_void,
            *const GUID,
            u32,
            *mut c_void,
            *mut *mut c_void,
        ) -> HRESULT = std::mem::transmute(*(d_vtbl.add(3)));
        let mut mgr2: *mut c_void = null_mut();
        let hr_m = activate(dev, &IID_IAUDIO_SESSION_MANAGER2, 0x17, null_mut(), &mut mgr2);
        release_com(dev);
        if hr_m < 0 || mgr2.is_null() {
            return 0.0;
        }

        let m_vtbl = *(mgr2 as *const *const *const c_void);
        let get_enum: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(*(m_vtbl.add(5)));
        let mut sess_enum: *mut c_void = null_mut();
        if get_enum(mgr2, &mut sess_enum) < 0 || sess_enum.is_null() {
            release_com(mgr2);
            return 0.0;
        }

        let se_vtbl = *(sess_enum as *const *const *const c_void);
        let get_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT =
            std::mem::transmute(*(se_vtbl.add(3)));
        let get_sess: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(*(se_vtbl.add(4)));

        let mut count: i32 = 0;
        let _ = get_count(sess_enum, &mut count);
        if pid_is_amp.len() > 256 {
            pid_is_amp.clear();
        }

        let mut max_peak: f32 = 0.0;
        for i in 0..count {
            let mut ctrl: *mut c_void = null_mut();
            if get_sess(sess_enum, i, &mut ctrl) < 0 || ctrl.is_null() {
                continue;
            }
            let c_vtbl = *(ctrl as *const *const *const c_void);
            let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
                std::mem::transmute(*c_vtbl);

            let mut pid: u32 = 0;
            let mut ctrl2: *mut c_void = null_mut();
            if qi(ctrl, &IID_IAUDIO_SESSION_CONTROL2, &mut ctrl2) >= 0 && !ctrl2.is_null() {
                let c2_vtbl = *(ctrl2 as *const *const *const c_void);
                let get_pid: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT =
                    std::mem::transmute(*(c2_vtbl.add(14)));
                let _ = get_pid(ctrl2, &mut pid);
                release_com(ctrl2);
            }

            let is_amp = if pid == 0 {
                false
            } else if let Some(&cached) = pid_is_amp.get(&pid) {
                cached
            } else {
                let h = OpenProcess(0x1000, 0, pid);
                let matched = if h.is_null() {
                    false
                } else {
                    let mut buf = [0u16; 260];
                    let mut sz = buf.len() as u32;
                    let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut sz) != 0 && sz > 0;
                    let _ = CloseHandle(h);
                    if ok {
                        let full = String::from_utf16_lossy(&buf[..(sz as usize)]);
                        full.rsplit(['\\', '/'])
                            .next()
                            .is_some_and(|n| n.eq_ignore_ascii_case("AMPLibraryAgent.exe"))
                    } else {
                        false
                    }
                };
                pid_is_amp.insert(pid, matched);
                matched
            };

            if is_amp {
                let mut peak: f32 = 0.0;
                let mut meter: *mut c_void = null_mut();
                if qi(ctrl, &IID_IAUDIO_METER_INFORMATION, &mut meter) >= 0 && !meter.is_null() {
                    let mt_vtbl = *(meter as *const *const *const c_void);
                    let get_peak: unsafe extern "system" fn(*mut c_void, *mut f32) -> HRESULT =
                        std::mem::transmute(*(mt_vtbl.add(3)));
                    let _ = get_peak(meter, &mut peak);
                    release_com(meter);
                }
                if peak.is_finite() && peak > max_peak {
                    max_peak = peak;
                }
            }
            release_com(ctrl);
        }

        release_com(sess_enum);
        release_com(mgr2);
        max_peak
    }

    struct LoopbackCapture {
        dev_id: String,
        dev_name: String,
        sample_rate: u32,
        bit_depth: u16,
        channels: u16,
        fmt: crate::binaural_dsp::ParsedWasapiWaveFormat,
        audio_client: *mut c_void,
        capture_client: *mut c_void,
    }

    impl LoopbackCapture {
        unsafe fn open_target(override_id: Option<&str>) -> Option<Self> {
            let mut enumerator: *mut c_void = null_mut();
            if CoCreateInstance(
                &CLSID_MMDEVICE_ENUMERATOR,
                null_mut(),
                0x17,
                &IID_IMMDEVICE_ENUMERATOR,
                &mut enumerator,
            ) < 0
                || enumerator.is_null()
            {
                return None;
            }
            let e_vtbl = *(enumerator as *const *const *const c_void);
            let get_default: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(e_vtbl.add(4)));
            let get_device: unsafe extern "system" fn(
                *mut c_void,
                *const u16,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(e_vtbl.add(5)));

            let mut dev: *mut c_void = null_mut();
            if let Some(oid) = override_id.filter(|s| !s.is_empty()) {
                let mut wide: Vec<u16> = oid.encode_utf16().collect();
                wide.push(0);
                let _ = get_device(enumerator, wide.as_ptr(), &mut dev);
            }
            if dev.is_null() && (get_default(enumerator, 0, 1, &mut dev) < 0 || dev.is_null()) {
                release_com(enumerator);
                return None;
            }
            release_com(enumerator);

            let dev_id = read_device_id(dev);
            let dev_name = read_device_friendly_name(dev);

            let d_vtbl = *(dev as *const *const *const c_void);
            let activate: unsafe extern "system" fn(
                *mut c_void,
                *const GUID,
                u32,
                *mut c_void,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(*(d_vtbl.add(3)));

            let mut audio_client: *mut c_void = null_mut();
            let hr_act = activate(dev, &IID_IAUDIO_CLIENT, 0x17, null_mut(), &mut audio_client);
            release_com(dev);
            if hr_act < 0 || audio_client.is_null() {
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

            let wf = std::ptr::read_unaligned(fmt_ptr);
            let raw_len = if wf.w_format_tag == 0xFFFE && wf.cb_size >= 22 {
                std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
            } else {
                std::mem::size_of::<WAVEFORMATEX>()
            };
            let raw_fmt = std::slice::from_raw_parts(fmt_ptr as *const u8, raw_len);
            let parsed_fmt = crate::binaural_dsp::parse_wasapi_waveformat_bytes(raw_fmt).unwrap_or(
                crate::binaural_dsp::ParsedWasapiWaveFormat {
                    sample_rate: wf.n_samples_per_sec.max(8_000),
                    channels: wf.n_channels.max(1),
                    bits_per_sample: wf.w_bits_per_sample.max(16),
                    valid_bits_per_sample: wf.w_bits_per_sample.max(16),
                    bytes_per_sample: (wf.w_bits_per_sample as usize / 8).max(2),
                    block_align: (wf.n_channels.max(1) as usize)
                        * (wf.w_bits_per_sample as usize / 8).max(2),
                    channel_mask: 0x3,
                    is_float: wf.w_format_tag == 3,
                },
            );
            let sample_rate = parsed_fmt.sample_rate.max(8_000);
            let channels = parsed_fmt.channels.max(1);
            let valid_bits = parsed_fmt.valid_bits_per_sample;

            // 25ms low-latency shared loopback buffer for >=60Hz..240Hz visual fluidity
            let hr_init = init_fn(
                audio_client,
                0,
                0x0002_0000,
                250_000,
                0,
                fmt_ptr,
                null(),
            );
            CoTaskMemFree(fmt_ptr as *mut c_void);
            if hr_init < 0 {
                release_com(audio_client);
                return None;
            }

            let mut capture_client: *mut c_void = null_mut();
            if get_service(audio_client, &IID_IAUDIO_CAPTURE_CLIENT, &mut capture_client) < 0
                || capture_client.is_null()
            {
                release_com(audio_client);
                return None;
            }

            if start_fn(audio_client) < 0 {
                release_com(capture_client);
                release_com(audio_client);
                return None;
            }

            Some(Self {
                dev_id,
                dev_name,
                sample_rate,
                bit_depth: valid_bits,
                channels,
                fmt: parsed_fmt,
                audio_client,
                capture_client,
            })
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

    static WORKER_STARTED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

    pub fn ensure_loopback_worker() {
        let Ok(mut started) = WORKER_STARTED.lock() else {
            return;
        };
        if *started {
            return;
        }
        *started = true;

        let _ = std::thread::Builder::new()
            .name("am-wasapi-viz".to_string())
            .stack_size(512 * 1024)
            .spawn(move || unsafe {
                let _ = CoInitializeEx(null_mut(), 0);
                let _ = crate::am_agent::init_process_com_security();
                let mut cap: Option<LoopbackCapture> = None;
                let mut accum = ForensicAccumulator::new(48_000);
                let mut tempo = TempoTracker::new(48_000);
                let mut fft_win_buf: Vec<f32> = vec![0.0; accum.fft_len()];
                let mut pcm_scratch: Vec<[f32; 2]> = Vec::with_capacity(2048);

                let mut last_packet_at = Instant::now() - Duration::from_secs(10);
                let mut was_silent_long = false;
                let mut was_live_prev = false;
                let mut last_dev_check = Instant::now();
                let mut last_forensic_fft = Instant::now() - Duration::from_secs(1);
                let mut last_session_poll = Instant::now() - Duration::from_secs(2);
                let mut cached_apps: Vec<ActiveAudioApp> = Vec::new();
                let mut io_hist: HashMap<u32, (u64, Instant)> = HashMap::new();
                let mut pid_is_amp: HashMap<u32, bool> = HashMap::new();
                let mut onset_watch_epoch_seen: u64 = 0;
                let mut onset_watch_started = Instant::now();
                let mut last_onset_meter_poll = Instant::now() - Duration::from_secs(1);

                loop {
                    let mut awaiting_onset = AMP_AWAITING_ONSET.load(Ordering::Acquire);
                    let viz_sub = VIZ_SUBSCRIBED.load(Ordering::Acquire);

                    // Zero-CPU Condvar sleep when UI is NOT subscribed to visualizer AND not awaiting onset
                    if !viz_sub && !awaiting_onset {
                        if let Some(old) = cap.take() {
                            old.close();
                        }
                        if let Ok(mut rs) = RING_STATE.write() {
                            rs.active_signal = false;
                        }
                        was_live_prev = false;
                        let (lock, cvar) = &*WAKE_COND;
                        if let Ok(mut g) = lock.lock() {
                            while !VIZ_SUBSCRIBED.load(Ordering::Acquire)
                                && !AMP_AWAITING_ONSET.load(Ordering::Acquire)
                            {
                                let res = cvar.wait_timeout(g, Duration::from_secs(5)).unwrap();
                                g = res.0;
                            }
                            *g = false;
                        }
                        continue;
                    }

                    if awaiting_onset {
                        let cur_ep = AMP_ONSET_EPOCH.load(Ordering::Acquire);
                        if cur_ep != onset_watch_epoch_seen {
                            onset_watch_epoch_seen = cur_ep;
                            onset_watch_started = Instant::now();
                        }
                        if onset_watch_started.elapsed() > Duration::from_secs(10) {
                            AMP_AWAITING_ONSET.store(false, Ordering::Release);
                            awaiting_onset = false;
                        } else if last_onset_meter_poll.elapsed() >= Duration::from_millis(24) {
                            last_onset_meter_poll = Instant::now();
                            let peak = query_amp_agent_meter_peak_default_endpoint(&mut pid_is_amp);
                            if peak > 1.0e-4 {
                                AMP_AGENT_SESSION_ACTIVE.store(true, Ordering::Release);
                                AMP_ONSET_DETECTED_EPOCH.store(cur_ep, Ordering::Release);
                                AMP_AWAITING_ONSET.store(false, Ordering::Release);
                                awaiting_onset = false;
                                wake_registered_ui_window();
                            } else {
                                AMP_AGENT_SESSION_ACTIVE.store(false, Ordering::Release);
                            }
                        }
                        if !viz_sub {
                            std::thread::sleep(Duration::from_millis(25));
                            continue;
                        }
                    }

                    if INSPECTOR_PAUSED.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(80));
                        continue;
                    }

                    if FORCE_REBIND_CAPTURE.swap(false, Ordering::AcqRel) {
                        if let Some(old) = cap.take() {
                            old.close();
                        }
                    }

                    if last_dev_check.elapsed() > Duration::from_millis(2500) {
                        last_dev_check = Instant::now();
                        let ovr = CAPTURE_TARGET_OVERRIDE
                            .read()
                            .ok()
                            .and_then(|g| g.clone());
                        if let Some(ref cur) = cap {
                            if let Some(ref oid) = ovr {
                                if &cur.dev_id != oid {
                                    if let Some(old) = cap.take() {
                                        old.close();
                                    }
                                }
                            } else if let Ok(devs) = list_output_devices() {
                                if let Some(def) = devs.iter().find(|d| d.is_default) {
                                    if def.id != cur.dev_id {
                                        if let Some(old) = cap.take() {
                                            old.close();
                                        }
                                        INSPECTOR_RESET.store(true, Ordering::Release);
                                    }
                                }
                            }
                        }
                    }

                    if cap.is_none() {
                        let ovr = CAPTURE_TARGET_OVERRIDE
                            .read()
                            .ok()
                            .and_then(|g| g.clone());
                        cap = LoopbackCapture::open_target(ovr.as_deref());
                        if let Some(ref c) = cap {
                            accum = ForensicAccumulator::new(c.sample_rate);
                            tempo.set_sample_rate(c.sample_rate);
                            fft_win_buf.resize(accum.fft_len(), 0.0);
                        } else {
                            std::thread::sleep(Duration::from_millis(450));
                            continue;
                        }
                    }

                    if INSPECTOR_RESET.swap(false, Ordering::AcqRel) {
                        accum.reset();
                        tempo.reset();
                    }

                    let mut peak_l = 0.0f32;
                    let mut peak_r = 0.0f32;
                    let mut sum_sq = 0.0f32;
                    let mut sum_l2 = 0.0f32;
                    let mut sum_r2 = 0.0f32;
                    let mut sum_lr = 0.0f32;
                    let mut sum_mid2 = 0.0f32;
                    let mut sum_side2 = 0.0f32;
                    let mut total_frames = 0usize;
                    let mut broken = false;
                    pcm_scratch.clear();

                    if let Some(ref c) = cap {
                        let cap_vtbl = *(c.capture_client as *const *const *const c_void);
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

                        let mut packet_frames: u32 = 0;
                        if get_next_size(c.capture_client, &mut packet_frames) < 0 {
                            broken = true;
                        } else {
                            while packet_frames > 0 {
                                let mut data_ptr: *mut u8 = null_mut();
                                let mut num_frames: u32 = 0;
                                let mut flags: u32 = 0;
                                let mut dev_pos: u64 = 0;
                                let mut qpc_pos: u64 = 0;
                                if get_buf(
                                    c.capture_client,
                                    &mut data_ptr,
                                    &mut num_frames,
                                    &mut flags,
                                    &mut dev_pos,
                                    &mut qpc_pos,
                                ) < 0
                                {
                                    broken = true;
                                    break;
                                }

                                let silent = (flags & 0x2) != 0 || data_ptr.is_null();
                                let stride = c.fmt.block_align.max(2);

                                for f in 0..(num_frames as usize) {
                                    let (sl, sr) = if silent {
                                        (0.0f32, 0.0f32)
                                    } else {
                                        let frame_slice = std::slice::from_raw_parts(
                                            data_ptr.add(f * stride),
                                            stride,
                                        );
                                        let [l, r] =
                                            crate::binaural_dsp::decode_wasapi_frame_to_stereo(
                                                frame_slice,
                                                &c.fmt,
                                            );
                                        (l.clamp(-1.5, 1.5), r.clamp(-1.5, 1.5))
                                    };

                                    peak_l = peak_l.max(sl.abs());
                                    peak_r = peak_r.max(sr.abs());
                                    let mono = 0.5 * (sl + sr);
                                    let side = 0.5 * (sl - sr);
                                    sum_sq += mono * mono;
                                    sum_l2 += sl * sl;
                                    sum_r2 += sr * sr;
                                    sum_lr += sl * sr;
                                    sum_mid2 += mono * mono;
                                    sum_side2 += side * side;
                                    pcm_scratch.push([sl, sr]);
                                }

                                total_frames += num_frames as usize;
                                let _ = rel_buf(c.capture_client, num_frames);
                                if get_next_size(c.capture_client, &mut packet_frames) < 0 {
                                    broken = true;
                                    break;
                                }
                            }
                        }
                    }

                    if broken {
                        if let Some(old) = cap.take() {
                            old.close();
                        }
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }

                    let has_signal = total_frames > 0 && (peak_l > 1.2e-4 || peak_r > 1.2e-4);
                    if has_signal {
                        if was_silent_long && AUTO_RESET_SILENCE.load(Ordering::Acquire) {
                            accum.reset();
                            tempo.reset();
                        }
                        was_silent_long = false;
                        last_packet_at = Instant::now();
                        accum.observe_pcm_samples(&pcm_scratch);
                        tempo.observe_stereo_pcm(&pcm_scratch);
                        if let Ok(mut ts) = SHARED_TEMPO.write() {
                            ts.bpm = tempo.locked_bpm;
                            ts.beat_phase = tempo.beat_phase;
                            ts.beat_seq = tempo.beat_seq;
                        }
                    } else if last_packet_at.elapsed() >= Duration::from_millis(2200) {
                        was_silent_long = true;
                    }

                    let is_live = last_packet_at.elapsed() < Duration::from_millis(200);

                    if let Some(ref c) = cap {
                        // Commit new stereo samples into the shared high-speed ring buffer
                        let mut mono_tail_for_fft = false;
                        if let Ok(mut rs) = RING_STATE.write() {
                            rs.active_signal = is_live;
                            rs.device_id = c.dev_id.clone();
                            rs.device_name = c.dev_name.clone();
                            rs.sample_rate = c.sample_rate;
                            rs.bit_depth = c.bit_depth;
                            rs.channels = c.channels;

                            for &lr in &pcm_scratch {
                                let wp = rs.write_pos;
                                rs.stereo[wp] = lr;
                                rs.write_pos = (wp + 1) & RING_MASK;
                            }

                            if is_live && total_frames > 0 {
                                let rms = (sum_sq / (total_frames as f32)).sqrt();
                                let dl = (20.0 * peak_l.max(1.58e-5).log10()).clamp(-96.0, 0.0);
                                let dr = (20.0 * peak_r.max(1.58e-5).log10()).clamp(-96.0, 0.0);
                                let drms = (20.0 * rms.max(1.58e-5).log10()).clamp(-96.0, 0.0);
                                let crest = (dl.max(dr) - drms).max(0.0);
                                let denom = (sum_l2 * sum_r2).sqrt();
                                let corr = if denom > 1e-9 {
                                    (sum_lr / denom).clamp(-1.0, 1.0)
                                } else {
                                    1.0
                                };
                                let tot_ms = sum_mid2 + sum_side2;
                                let width = if tot_ms > 1e-9 {
                                    ((sum_side2 / tot_ms).sqrt() * 140.0).clamp(0.0, 100.0)
                                } else {
                                    0.0
                                };
                                rs.peak_db_l = rs.peak_db_l * 0.45 + dl * 0.55;
                                rs.peak_db_r = rs.peak_db_r * 0.45 + dr * 0.55;
                                rs.rms_db = rs.rms_db * 0.55 + drms * 0.45;
                                rs.dynamic_range_db = rs.dynamic_range_db * 0.65 + crest * 0.35;
                                rs.phase_correlation = rs.phase_correlation * 0.6 + corr * 0.4;
                                rs.stereo_width_pct = rs.stereo_width_pct * 0.6 + width * 0.4;
                            } else if !is_live {
                                rs.peak_db_l = -96.0;
                                rs.peak_db_r = -96.0;
                                rs.rms_db = -96.0;
                                rs.dynamic_range_db = 0.0;
                            }

                            if is_live && last_forensic_fft.elapsed() >= Duration::from_millis(14) {
                                last_forensic_fft = Instant::now();
                                let flen = accum.fft_len();
                                let start = (rs.write_pos + RING_LEN - flen) & RING_MASK;
                                for i in 0..flen {
                                    let lr = rs.stereo[(start + i) & RING_MASK];
                                    fft_win_buf[i] = 0.5 * (lr[0] + lr[1]);
                                }
                                mono_tail_for_fft = true;
                            }
                        }

                        // Wake idle UI window immediately when audio transitions from silence -> active
                        let onset_now = is_live && !was_live_prev;
                        if onset_now {
                            cached_apps =
                                query_active_audio_apps_on_device(&c.dev_id, &mut io_hist);
                            last_session_poll = Instant::now();
                            if !AMP_AWAITING_ONSET.load(Ordering::Acquire) {
                                let amp_on = cached_apps.iter().any(|a| {
                                    a.exe_name.eq_ignore_ascii_case("AMPLibraryAgent.exe")
                                        && a.peak_db > -80.0
                                });
                                AMP_AGENT_SESSION_ACTIVE.store(amp_on, Ordering::Release);
                            }
                            wake_registered_ui_window();
                        } else if !is_live && !AMP_AWAITING_ONSET.load(Ordering::Acquire) {
                            AMP_AGENT_SESSION_ACTIVE.store(false, Ordering::Release);
                        }
                        was_live_prev = is_live;

                        // Update forensic 4096/8192-pt FFT & Spek column at >=60Hz when live
                        if mono_tail_for_fft {
                            if last_session_poll.elapsed() >= Duration::from_millis(850) {
                                last_session_poll = Instant::now();
                                cached_apps =
                                    query_active_audio_apps_on_device(&c.dev_id, &mut io_hist);
                                if !AMP_AWAITING_ONSET.load(Ordering::Acquire) {
                                    let amp_on = cached_apps.iter().any(|a| {
                                        a.exe_name.eq_ignore_ascii_case("AMPLibraryAgent.exe")
                                            && a.peak_db > -80.0
                                    });
                                    AMP_AGENT_SESSION_ACTIVE.store(amp_on, Ordering::Release);
                                }
                            }
                            let (inst_spec, inst_cut) = accum.process_fft_window(&fft_win_buf);
                            let (
                                tier_code,
                                verdict_title,
                                estimated_bitrate,
                                confidence_pct,
                                forensic_notes,
                                cutoff_hz,
                                brickwall_drop_db,
                                has_brickwall_cut,
                            ) = accum.evaluate_verdict();

                            if let Ok(mut fs) = FORENSIC_SUMMARY.write() {
                                fs.inst_spectrum_db = inst_spec;
                                fs.peak_envelope_db = accum.peak_env_db.clone();
                                fs.tier_code = tier_code;
                                fs.verdict_title = verdict_title;
                                fs.estimated_bitrate = estimated_bitrate;
                                fs.confidence_pct = confidence_pct;
                                fs.forensic_notes = forensic_notes;
                                fs.cutoff_hz = cutoff_hz;
                                fs.inst_cutoff_hz = inst_cut;
                                fs.brickwall_drop_db = brickwall_drop_db;
                                fs.has_brickwall_cut = has_brickwall_cut;
                                fs.mdct_hole_pct =
                                    (accum.mdct_hole_ema * 10.0).round() / 10.0;
                                fs.detected_bit_grid = accum.detected_bit_grid_label();
                                fs.clip_count = accum.clip_count;
                                fs.analyzed_frames = accum.analyzed_frames;
                                fs.active_apps = cached_apps.clone();
                            }

                            // Also keep legacy LATEST_VIZ snapshot fresh for tests/diagnostics
                            if let Ok(mut lv) = LATEST_VIZ.write() {
                                lv.active_signal = is_live;
                                lv.device_id = c.dev_id.clone();
                                lv.device_name = c.dev_name.clone();
                                lv.device_sample_rate = c.sample_rate;
                                lv.device_bit_depth = c.bit_depth;
                                lv.device_channels = c.channels;
                                lv.spectrum = compute_spectrum_32(
                                    &fft_win_buf[fft_win_buf.len() - 512..],
                                    c.sample_rate as f32,
                                )
                                .to_vec();
                                if let Ok(rs) = RING_STATE.read() {
                                    lv.peak_db_l = rs.peak_db_l;
                                    lv.peak_db_r = rs.peak_db_r;
                                    lv.rms_db = rs.rms_db;
                                    lv.crest_db = rs.dynamic_range_db;
                                    lv.phase_correlation = rs.phase_correlation;
                                    lv.stereo_width_pct = rs.stereo_width_pct;
                                }
                            }
                        } else if !is_live {
                            // During silence on Visualizer page, decay spectrum once and sleep deeply (~60ms) so CPU < 0.1%
                            if let Ok(mut fs) = FORENSIC_SUMMARY.write() {
                                let mut any_above = false;
                                for v in &mut fs.inst_spectrum_db {
                                    if *v > -109.5 {
                                        *v = (*v - 4.0).max(-110.0);
                                        any_above = true;
                                    }
                                }
                                if !any_above {
                                    fs.active_apps.clear();
                                }
                            }
                        }
                    }

                    if is_live {
                        // ~6ms capture cadence (~166Hz) keeps the ring buffer silky-smooth for 60Hz..240Hz VSync rendering
                        std::thread::sleep(Duration::from_millis(6));
                    } else if awaiting_onset {
                        // 25ms cadence while awaiting AMPLibraryAgent audio onset
                        std::thread::sleep(Duration::from_millis(25));
                    } else {
                        // Deep sleep during silence (~0.02% CPU) while waking within ~55ms of audio start
                        std::thread::sleep(Duration::from_millis(55));
                    }
                }
            });
    }

    pub fn wake_registered_ui_window() {
        let hwnd = WAKE_HWND.load(Ordering::Acquire);
        if hwnd != 0 {
            unsafe {
                let _ = PostMessageW(hwnd as *mut c_void, WM_APP_AUDIO_WAKE, 0, 0);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod win {
    use super::*;
    pub fn is_default_endpoint_spatial_ready() -> bool {
        true
    }
    pub fn list_output_devices() -> Result<Vec<AudioOutputDevice>, String> {
        Ok(Vec::new())
    }
    pub fn set_output_device(_device_id: &str) -> Result<Vec<AudioOutputDevice>, String> {
        Err("仅支持 Windows 平台".to_string())
    }
    pub fn ensure_loopback_worker() {}
    pub fn wake_registered_ui_window() {}
}

pub use win::{
    ensure_loopback_worker, is_default_endpoint_spatial_ready, list_output_devices,
    set_output_device, wake_registered_ui_window,
};
