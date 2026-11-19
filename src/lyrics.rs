use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LyricSyllable {
    pub text: String,
    pub start_sec: f64,
    pub end_sec: f64,
    /// UTF-16 code unit start index within parent line's `text` (for DirectWrite HitTestTextRange)
    pub utf16_start: u32,
    /// UTF-16 code unit length
    pub utf16_len: u32,
    /// True if this syllable is a sustained/emphasized vocal note (auto or explicit)
    pub is_sustained: bool,
    /// True ONLY when TTML/source explicitly marked emphasis (italic/bold/emphasis)
    #[serde(default)]
    pub is_explicit_emphasis: bool,
}

/// Piecewise syllable timing keyframe within a merged multi-syllable `LyricCharUnit`
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct LyricSubSpan {
    pub frac_start: f32,
    pub frac_end: f32,
    pub start_sec: f64,
    pub end_sec: f64,
}

/// How non-emphasized lyrics (normal words/characters without TTML emphasis or sustained duration)
/// behave while active: by default they lift gently upward and sweep cleanly without glow or zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NonEmphasisLyricsEffect {
    #[default]
    None,
    GlowOnly,
    ScaleOnly,
    Both,
}

impl NonEmphasisLyricsEffect {
    #[inline]
    pub fn glow(self) -> bool {
        matches!(self, Self::GlowOnly | Self::Both)
    }

    #[inline]
    pub fn scale(self) -> bool {
        matches!(self, Self::ScaleOnly | Self::Both)
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Self::None => "关闭（默认）",
            Self::GlowOnly => "仅发光",
            Self::ScaleOnly => "仅放大",
            Self::Both => "发光 + 放大",
        }
    }
}

/// Per-character (`字`) or per-word timing & UTF-16 glyph slice subdivided from `LyricSyllable`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LyricCharUnit {
    pub ch_str: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub word_dur_sec: f64,
    pub utf16_start: u32,
    pub utf16_len: u32,
    pub is_sustained: bool,
    #[serde(default)]
    pub is_explicit_emphasis: bool,
    #[serde(default)]
    pub sub_spans: Vec<LyricSubSpan>,
}

pub const SLOW_NON_EMPH_MIN_DUR_SEC: f64 = 0.55;
pub const SLOW_NON_EMPH_MAX_SPEED_DIP_PER_SEC: f32 = 160.0;

#[inline]
pub fn should_use_frontier_lift(is_emphasized: bool, dur_sec: f64, unit_width_dip: f32) -> bool {
    if is_emphasized {
        return true;
    }
    if dur_sec < SLOW_NON_EMPH_MIN_DUR_SEC {
        return false;
    }
    let speed = unit_width_dip.max(1.0) / (dur_sec as f32).max(0.05);
    speed <= SLOW_NON_EMPH_MAX_SPEED_DIP_PER_SEC
}

#[inline]
pub fn eval_frontier_lift_weight(local_u: f32, prog: f32, is_rtl: bool) -> f32 {
    if prog <= 0.002 {
        return 0.0;
    }
    if prog >= 0.996 {
        return 1.0;
    }
    let u = if is_rtl { 1.0 - local_u } else { local_u }.clamp(0.0, 1.0);
    let p = prog.clamp(0.0, 1.0);
    let half_w = 0.22f32;
    let f = p * 1.24 - 0.12 * (1.0 - p);
    let x = ((f - u + half_w) / (half_w * 2.0)).clamp(0.0, 1.0);
    let s = x * x * (3.0 - 2.0 * x);
    let onset = (p / 0.08).clamp(0.0, 1.0);
    (s * onset).clamp(0.0, 1.0)
}

/// Computes continuous zero-tear elastic vertical lift & shear `(center_lift, shear_m12)` for a
/// single glyph (e.g. a Chinese/CJK character) sweeping through the highlight frontier so the
/// leading edge lifts smoothly ahead of the trailing edge without ever slicing the glyph strokes.
#[inline]
pub fn eval_frontier_wave_shear(lift_y: f32, glyph_w: f32, prog: f32, is_rtl: bool) -> (f32, f32) {
    if prog <= 0.002 {
        return (0.0, 0.0);
    }
    if prog >= 0.996 {
        return (lift_y, 0.0);
    }
    let w_l = eval_frontier_lift_weight(0.0, prog, is_rtl);
    let w_r = eval_frontier_lift_weight(1.0, prog, is_rtl);
    let y_l = lift_y * w_l;
    let y_r = lift_y * w_r;
    let center_lift = 0.5 * (y_l + y_r);
    let shear_m12 = ((y_r - y_l) / glyph_w.max(12.0)).clamp(-0.14, 0.14);
    (center_lift, shear_m12)
}

#[inline]
pub fn eval_frontier_scale_delta(local_u: f32, prog: f32, max_delta: f32, is_rtl: bool) -> f32 {
    if prog <= 0.002 || prog >= 0.998 || max_delta <= 0.0001 {
        return 0.0;
    }
    let u = if is_rtl { 1.0 - local_u } else { local_u }.clamp(0.0, 1.0);
    let p = prog.clamp(0.0, 1.0);
    let dist = u - p;
    let sigma = 0.26f32;
    let bump = (-0.5 * (dist / sigma) * (dist / sigma)).exp();
    let entry_env = (p / 0.08).clamp(0.0, 1.0);
    let exit_env = ((1.0 - p) / 0.10).clamp(0.0, 1.0);
    max_delta * bump * entry_env.min(exit_env)
}

/// Computes `(body_alpha, frontier_peak_alpha, tail_decay)` for a glowing unit:
/// - During sweep (`prog < 0.996`), swept body glow is weakened (`body_base`) while the highlight
///   frontier peaks sharply (`peak_base`).
/// - After the unit finishes sweeping and rests at word end (`is_holding_at_word_end`), body glow
///   gradually rises toward `frontier_peak_alpha`.
/// - Once moving past the word hold window, `decay_sec` smoothly attenuates glow to `0.0`.
#[inline]
pub fn eval_frontier_glow_profile(
    prog: f32,
    hold_after_end_sec: f32,
    decay_sec: f32,
    is_holding_at_word_end: bool,
    is_emphasized: bool,
    dur_factor: f32,
) -> (f32, f32, f32) {
    if prog <= 0.002 {
        return (0.0, 0.0, 0.0);
    }
    let df = dur_factor.clamp(0.0, 1.0);
    let peak_base = if is_emphasized {
        0.58 + 0.20 * df
    } else {
        0.40
    };
    let body_base = if is_emphasized {
        0.34 + 0.14 * df
    } else {
        0.24
    };
    // Target body glow reached seamlessly by the time the frontier arrives at word end (~78% of frontier peak)
    let body_at_end = (peak_base * 0.78).max(body_base);

    if prog < 0.996 {
        let entry = (prog / 0.08).clamp(0.0, 1.0);
        let bell = 0.86 + 0.14 * (prog * std::f32::consts::PI).sin().max(0.0);
        let env = entry * bell;
        let ramp_t = ((prog - 0.25) / 0.75).clamp(0.0, 1.0);
        let ramp = ramp_t * ramp_t * (3.0 - 2.0 * ramp_t);
        let swept_body = body_base + (body_at_end - body_base) * ramp;
        return (
            (swept_body * env).clamp(0.0, 1.0),
            (peak_base * env).clamp(0.0, 1.0),
            1.0,
        );
    }

    // Gentle, organic final rise from `body_at_end` (~80% peak) up to `peak_base` when resting at word end
    let rise_t = (hold_after_end_sec.max(0.0) / 0.42).clamp(0.0, 1.0);
    let rise = rise_t * rise_t * (3.0 - 2.0 * rise_t);
    let body_held = body_at_end + (peak_base - body_at_end) * rise;
    if is_holding_at_word_end {
        (body_held.clamp(0.0, 1.0), peak_base.clamp(0.0, 1.0), 1.0)
    } else {
        let after = (1.0 - decay_sec.max(0.0) / 0.34).clamp(0.0, 1.0);
        let tail_decay = after * after;
        (
            (body_held * tail_decay).clamp(0.0, 1.0),
            (peak_base * tail_decay).clamp(0.0, 1.0),
            tail_decay,
        )
    }
}

impl LyricCharUnit {
    #[inline]
    pub fn is_emphasized(&self, auto_detect: bool) -> bool {
        if auto_detect {
            self.is_sustained
        } else {
            self.is_explicit_emphasis
        }
    }

    /// Computes `[0.0 .. 1.0]` karaoke sweep progress at `t_sec`, preserving exact piecewise
    /// syllable timing inside merged multi-syllable English words!
    #[inline]
    pub fn sweep_progress(&self, t_sec: f64) -> f32 {
        if t_sec <= self.start_sec {
            return 0.0;
        }
        if t_sec >= self.end_sec {
            return 1.0;
        }
        if self.sub_spans.len() > 1 {
            for seg in &self.sub_spans {
                if t_sec < seg.start_sec {
                    return seg.frac_start.clamp(0.0, 1.0);
                }
                if t_sec <= seg.end_sec {
                    let dur = (seg.end_sec - seg.start_sec).max(0.012);
                    let local = ((t_sec - seg.start_sec) / dur).clamp(0.0, 1.0) as f32;
                    return (seg.frac_start + (seg.frac_end - seg.frac_start) * local)
                        .clamp(0.0, 1.0);
                }
            }
            return 1.0;
        }
        let dur = (self.end_sec - self.start_sec).max(0.025);
        ((t_sec - self.start_sec) / dur).clamp(0.0, 1.0) as f32
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LyricLine {
    pub start_sec: f64,
    pub end_sec: f64,
    pub text: String,
    pub syllables: Vec<LyricSyllable>,
    /// CJK per-character (`字`) + Latin/English per-word (`word`) units (default mode!)
    #[serde(default)]
    pub word_cjk_units: Vec<LyricCharUnit>,
    /// All-scripts per-character units (used when English per-word switch is turned OFF)
    #[serde(default)]
    pub char_units: Vec<LyricCharUnit>,
    pub has_word_sync: bool,
    pub bg_text: Option<String>,
    pub bg_syllables: Vec<LyricSyllable>,
    #[serde(default)]
    pub bg_word_cjk_units: Vec<LyricCharUnit>,
    #[serde(default)]
    pub bg_char_units: Vec<LyricCharUnit>,
    pub translation: Option<String>,
    pub is_duet_secondary: bool,
    pub is_rtl: bool,
}

impl LyricLine {
    #[inline]
    pub fn units_for_mode(&self, latin_by_word: bool) -> &[LyricCharUnit] {
        if latin_by_word && !self.word_cjk_units.is_empty() {
            &self.word_cjk_units
        } else {
            &self.char_units
        }
    }

    #[inline]
    pub fn bg_units_for_mode(&self, latin_by_word: bool) -> &[LyricCharUnit] {
        if latin_by_word && !self.bg_word_cjk_units.is_empty() {
            &self.bg_word_cjk_units
        } else {
            &self.bg_char_units
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ParsedLyrics {
    pub source_name: String,
    pub is_syllable_synced: bool,
    pub lines: Vec<LyricLine>,
    pub songwriters: Vec<String>,
}

impl ParsedLyrics {
    /// Deterministic 64-bit FNV-1a fingerprint covering source, sync granularity, all line texts,
    /// translations, backing vocals, and millisecond timestamps so source switches & edits
    /// always invalidate layout/bitmap/animator caches cleanly.
    pub fn content_hash(&self) -> u64 {
        #[inline]
        fn mix_bytes(h: &mut u64, bytes: &[u8]) {
            for &b in bytes {
                *h ^= b as u64;
                *h = h.wrapping_mul(0x0100_0000_01b3);
            }
            *h ^= 0xff;
            *h = h.wrapping_mul(0x0100_0000_01b3);
        }
        #[inline]
        fn mix_u64(h: &mut u64, v: u64) {
            mix_bytes(h, &v.to_le_bytes());
        }

        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        mix_bytes(&mut h, self.source_name.as_bytes());
        mix_u64(&mut h, self.is_syllable_synced as u64);
        mix_u64(&mut h, self.lines.len() as u64);
        for line in &self.lines {
            mix_u64(&mut h, (line.start_sec * 1000.0).round() as i64 as u64);
            mix_u64(&mut h, (line.end_sec * 1000.0).round() as i64 as u64);
            let flags = (line.has_word_sync as u64)
                | ((line.is_duet_secondary as u64) << 1)
                | ((line.is_rtl as u64) << 2);
            mix_u64(&mut h, flags);
            mix_bytes(&mut h, line.text.as_bytes());
            if let Some(ref bg) = line.bg_text {
                mix_bytes(&mut h, bg.as_bytes());
            }
            if let Some(ref tr) = line.translation {
                mix_bytes(&mut h, tr.as_bytes());
            }
            mix_u64(&mut h, line.syllables.len() as u64);
            if let Some(first_s) = line.syllables.first() {
                mix_u64(&mut h, (first_s.start_sec * 1000.0).round() as i64 as u64);
            }
            if let Some(last_s) = line.syllables.last() {
                mix_u64(&mut h, (last_s.end_sec * 1000.0).round() as i64 as u64);
            }
        }
        h
    }
}

/// Exact 2nd-order mass-spring-damper ODE matching `UISpringTimingParameters(mass:stiffness:damping:)`
#[derive(Clone, Copy, Debug)]
pub struct Spring1D {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub mass: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl Spring1D {
    pub const fn new(initial: f32, mass: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
            mass,
            stiffness,
            damping,
        }
    }

    /// `SyncedLyricsViewController.Specs.lineChangeSpringTimingParameters()` -> (1.0, 100.0, 18.0)
    pub const fn line_change(initial: f32) -> Self {
        Self::new(initial, 1.0, 100.0, 18.0)
    }

    /// `SyncedLyricsViewController.Specs.liftSpringTimingParameters()` -> (1.0, 14.0, 7.0)
    pub const fn syllable_lift(initial: f32) -> Self {
        Self::new(initial, 1.0, 14.0, 7.0)
    }

    /// Crisp per-character (`字`) float spring (critically damped, smooth organic wave)
    pub const fn char_lift(initial: f32) -> Self {
        Self::new(initial, 1.0, 30.0, 11.2)
    }

    /// `SyncedLyricsViewController.Specs.highlightSpringTimingParameters(highlighted:)`
    pub fn set_highlight_pressed(&mut self, pressed: bool) {
        if pressed {
            self.mass = 1.0;
            self.stiffness = 322.0;
            self.damping = 24.0;
            self.target = 0.95; // Specs.touchDownTransform = scale(0.95, 0.95)
        } else {
            self.mass = 2.0;
            self.stiffness = 300.0;
            self.damping = 50.0;
            self.target = 1.0;
        }
    }

    pub fn snap(&mut self, val: f32) {
        self.value = val;
        self.target = val;
        self.velocity = 0.0;
    }

    pub fn step(&mut self, dt_sec: f32) {
        let dt_total = dt_sec.clamp(0.0, 0.08);
        if (self.value - self.target).abs() < 1e-4 && self.velocity.abs() < 1e-3 {
            self.value = self.target;
            self.velocity = 0.0;
            return;
        }
        // Sub-step at <= 4ms for unconditional stability even on stiff highlight springs (k=322)
        let steps = ((dt_total / 0.004).ceil() as usize).clamp(1, 20);
        let h = dt_total / (steps as f32);
        let inv_m = 1.0 / self.mass.max(0.05);
        for _ in 0..steps {
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * inv_m * h;
            self.value += self.velocity * h;
        }
        if (self.value - self.target).abs() < 1e-4 && self.velocity.abs() < 1e-3 {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.05 && self.velocity.abs() < 0.15
    }

    #[inline]
    pub fn is_settled_norm(&self) -> bool {
        (self.value - self.target).abs() < 0.002 && self.velocity.abs() < 0.012
    }
}

/// Per-line runtime physics state implementing `SyncedLyricsViewController.Specs`
#[derive(Clone, Debug)]
pub struct LineAnimState {
    pub y_spring: Spring1D,
    pub pending_target_y: f32,
    pub stagger_delay_rem: f32,
    pub scale_spring: Spring1D,
    pub press_spring: Spring1D,
    pub active_blend: Spring1D,
    pub blur_spring: Spring1D,
    pub syllable_lifts: Vec<Spring1D>,
    pub bg_syllable_lifts: Vec<Spring1D>,
    pub char_lifts: Vec<Spring1D>,
    pub bg_char_lifts: Vec<Spring1D>,
    pub measured_height: f32,
    /// Per-line monotonic playhead high-water mark (`ProcessedLine` `< 0.50s` anti-jitter clamp)
    pub max_word_t: f64,
}

impl LineAnimState {
    pub fn new(
        initial_y: f32,
        syllable_count: usize,
        bg_syllable_count: usize,
        char_count: usize,
        bg_char_count: usize,
    ) -> Self {
        Self {
            y_spring: Spring1D::line_change(initial_y),
            pending_target_y: initial_y,
            stagger_delay_rem: 0.0,
            scale_spring: Spring1D::new(0.98, 1.0, 115.0, 21.6),
            press_spring: Spring1D::new(1.0, 2.0, 300.0, 50.0),
            // Critically damped so active_blend and blur_spring ramp promptly & smoothly without ringing
            active_blend: Spring1D::new(0.0, 1.0, 128.0, 22.8),
            blur_spring: Spring1D::new(0.0, 1.0, 138.0, 23.5),
            syllable_lifts: vec![Spring1D::syllable_lift(0.0); syllable_count],
            bg_syllable_lifts: vec![Spring1D::syllable_lift(0.0); bg_syllable_count],
            char_lifts: vec![Spring1D::char_lift(0.0); char_count],
            bg_char_lifts: vec![Spring1D::char_lift(0.0); bg_char_count],
            measured_height: 64.0,
            max_word_t: -1.0,
        }
    }

    #[inline]
    pub fn clamped_word_time(&self, candidate_t: f64) -> f64 {
        if self.max_word_t >= 0.0
            && candidate_t < self.max_word_t
            && (self.max_word_t - candidate_t) < 0.18
        {
            self.max_word_t
        } else {
            candidate_t
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InterludeDotsState {
    pub visible: bool,
    pub after_line_idx: Option<usize>, // None = intro before line 0
    pub progress: f32,                 // 0.0 .. 1.0
    pub scale_alpha: f32,              // 0.0 .. 1.0 entrance/exit envelope
}

impl Default for InterludeDotsState {
    fn default() -> Self {
        Self {
            visible: false,
            after_line_idx: None,
            progress: 0.0,
            scale_alpha: 0.0,
        }
    }
}

/// Complete `SyncedLyricsViewController.Specs` physics coordinator
pub struct LyricsAnimator {
    pub track_key: String,
    pub bound_content_hash: u64,
    pub line_states: Vec<LineAnimState>,
    pub primary_active_idx: Option<usize>,
    pub secondary_active_idx: Option<usize>,
    pub interlude: InterludeDotsState,
    pub interlude_height_spring: Spring1D,
    pub user_scroll_offset: f32,
    pub user_scroll_target: f32,
    pub user_scroll_vel: f32,
    pub user_scroll_until: Option<Instant>,
    pub pressed_line: Option<usize>,
    pub hovered_line: Option<usize>,
    /// When `true` (default), CJK scripts animate per `字` while English/Latin scripts animate per `word`.
    pub latin_by_word: bool,
    interlude_latched: Option<Option<usize>>,
    layout_initialized: bool,
    last_playhead_sec: f64,
}

impl Default for LyricsAnimator {
    fn default() -> Self {
        Self {
            track_key: String::new(),
            bound_content_hash: 0,
            line_states: Vec::new(),
            primary_active_idx: None,
            secondary_active_idx: None,
            interlude: InterludeDotsState::default(),
            interlude_height_spring: Spring1D::line_change(0.0),
            user_scroll_offset: 0.0,
            user_scroll_target: 0.0,
            user_scroll_vel: 0.0,
            user_scroll_until: None,
            pressed_line: None,
            hovered_line: None,
            latin_by_word: true,
            interlude_latched: None,
            layout_initialized: false,
            last_playhead_sec: -1.0,
        }
    }
}

impl LyricsAnimator {
    /// Authoritative Specs constants from iPadOS 16.4 `SyncedLyricsViewController.Specs` (`0xAD56F8`)
    pub const LINE_DELAY_SEC: f32 = 0.05; // Specs.lineDelay = 0.05 (+144)
    pub const MAX_END_TIME_OFFSET_SEC: f64 = 0.50; // Specs.maxEndTimeOffset = 0.5 (+152)
    pub const ANIMATION_HEADSTART_SEC: f64 = 0.10; // Specs.animationHeadstart = 0.10 (+336)
    pub const SWEEP_HEADSTART_SEC: f64 = 0.10; // Unified +0.10s headstart shared across lift, glow & sweep!
    pub const GRADIENT_FEATHER_PX: f32 = 40.0; // Specs.lineProgressionGradientFeather = 40.0 (+376)
    pub const SYLLABLE_LIFT_PX: f32 = 2.0; // Specs.syllableLift = 2.0 (+488)
    pub const CHAR_LIFT_PX: f32 = 3.2; // Per-character (`字`) vertical float distance
    pub const EMPHASIZING_SCALE_DELTA: f32 = 0.14; // Specs.emphasizingScaleRange = 1.0...1.14 (+216)
    pub const DESELECTED_SCALE: f32 = 0.98; // Specs.deselectedTransform = 0.98 (+280)
    pub const BG_VOCALS_TOP_SPACING: f32 = 15.0; // Specs.backgroundVocalsTopSpacing = 15.0 (+88)
    pub const INTERLUDE_MIN_SEC: f64 = 7.0; // Specs.instrumentalBreakVisualizationMinSeconds = 7.0 (+464)
    pub const INTERLUDE_HEIGHT: f32 = 40.0; // Specs.instrumentalBreakViewHeight = 40.0 (+480)
    pub const LINE_SPACING_IPAD: f32 = 28.0; // Specs.lineSpacing = 25.0+ (+80)

    pub fn bind_lyrics(&mut self, key: &str, lyrics: &ParsedLyrics) {
        let chash = lyrics.content_hash();
        if self.track_key == key
            && self.bound_content_hash == chash
            && self.line_states.len() == lyrics.lines.len()
        {
            return;
        }
        let same_track_rebind = self.track_key == key && !key.is_empty();
        self.track_key = key.to_string();
        self.bound_content_hash = chash;
        self.primary_active_idx = None;
        self.secondary_active_idx = None;
        if !same_track_rebind {
            self.user_scroll_offset = 0.0;
            self.user_scroll_target = 0.0;
            self.user_scroll_vel = 0.0;
            self.user_scroll_until = None;
        }
        self.pressed_line = None;
        self.hovered_line = None;
        self.interlude_latched = None;
        self.layout_initialized = false;
        self.last_playhead_sec = -1.0;
        self.interlude = InterludeDotsState::default();
        self.interlude_height_spring.snap(0.0);

        let mut y_cursor = 60.0f32;
        let by_word = self.latin_by_word;
        self.line_states = lyrics
            .lines
            .iter()
            .map(|l| {
                let st = LineAnimState::new(
                    y_cursor,
                    l.syllables.len(),
                    l.bg_syllables.len(),
                    l.units_for_mode(by_word).len(),
                    l.bg_units_for_mode(by_word).len(),
                );
                y_cursor += 72.0;
                st
            })
            .collect();
    }

    pub fn reset_word_clamps(&mut self) {
        for st in &mut self.line_states {
            st.max_word_t = -1.0;
        }
        self.last_playhead_sec = -1.0;
    }

    pub fn reset_layout_snap(&mut self) {
        self.layout_initialized = false;
    }

    pub fn on_user_wheel(&mut self, delta_px: f32) {
        self.user_scroll_target = (self.user_scroll_target - delta_px).clamp(-12000.0, 12000.0);
        self.user_scroll_until = Some(Instant::now() + Duration::from_millis(3200));
    }

    pub fn is_user_scrolling(&self) -> bool {
        self.user_scroll_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    pub fn set_pressed_line(&mut self, idx: Option<usize>) {
        if self.pressed_line == idx {
            return;
        }
        if let Some(prev) = self.pressed_line {
            if let Some(st) = self.line_states.get_mut(prev) {
                st.press_spring.set_highlight_pressed(false);
            }
        }
        self.pressed_line = idx;
        if let Some(cur) = idx {
            if let Some(st) = self.line_states.get_mut(cur) {
                st.press_spring.set_highlight_pressed(true);
            }
        }
    }

    /// Updates active line selection, cascade stagger delays, interlude dots, and steps all springs!
    pub fn step(
        &mut self,
        lyrics: &ParsedLyrics,
        playhead_sec: f64,
        viewport_h: f32,
        dt_sec: f32,
    ) {
        if lyrics.lines.is_empty() || self.line_states.len() != lyrics.lines.len() {
            return;
        }

        // Reset per-line monotonic sweep clamps on backward seek >= 0.18s (or track loop)
        if self.last_playhead_sec >= 0.0 && playhead_sec + 0.18 < self.last_playhead_sec {
            for st in &mut self.line_states {
                st.max_word_t = -1.0;
            }
        }
        self.last_playhead_sec = playhead_sec;

        // Smooth 2nd-order critically-damped manual scroll + auto-return to synced anchor
        if let Some(until) = self.user_scroll_until {
            if Instant::now() >= until {
                self.user_scroll_until = None;
                self.user_scroll_target = 0.0;
            }
        } else {
            self.user_scroll_target = 0.0;
        }
        {
            let dt_s = dt_sec.clamp(0.001, 0.05);
            let sub = ((dt_s / 0.004).ceil() as usize).clamp(1, 16);
            let h = dt_s / (sub as f32);
            let k = if self.user_scroll_until.is_some() { 320.0f32 } else { 90.0f32 };
            let c = if self.user_scroll_until.is_some() { 35.8f32 } else { 19.2f32 };
            for _ in 0..sub {
                let err = self.user_scroll_offset - self.user_scroll_target;
                let acc = -k * err - c * self.user_scroll_vel;
                self.user_scroll_vel += acc * h;
                self.user_scroll_offset += self.user_scroll_vel * h;
            }
            if (self.user_scroll_offset - self.user_scroll_target).abs() < 0.08
                && self.user_scroll_vel.abs() < 0.25
            {
                self.user_scroll_offset = self.user_scroll_target;
                self.user_scroll_vel = 0.0;
            }
        }

        let t_line = playhead_sec + Self::ANIMATION_HEADSTART_SEC;

        // 1. Detect >= 7.0s instrumental break (`Specs.instrumentalBreakVisualizationMinSeconds`)
        // Exits 0.45s before `next_start` so dot height & line unblur settle cleanly ON vocal onset!
        let mut interlude_vis = false;
        let mut interlude_after: Option<usize> = None;
        let mut interlude_prog = 0.0f32;
        let mut interlude_env = 0.0f32;

        let first_start = lyrics.lines[0].start_sec;
        let intro_latched = self.interlude_latched == Some(None);
        let intro_exit = if intro_latched {
            first_start - 0.45
        } else {
            first_start - 0.55
        };
        if first_start >= Self::INTERLUDE_MIN_SEC && playhead_sec < intro_exit {
            interlude_vis = true;
            interlude_after = None;
            let span = intro_exit.max(0.1);
            interlude_prog = (playhead_sec / span).clamp(0.0, 1.0) as f32;
            let rem = (intro_exit - playhead_sec).max(0.0) as f32;
            interlude_env = if rem < 0.35 {
                (rem / 0.35).clamp(0.0, 1.0)
            } else {
                1.0
            };
        } else {
            // Locate gap purely from monotonic line intervals `(line[k].end_sec .. line[k+1].start_sec)`
            for k in 0..lyrics.lines.len().saturating_sub(1) {
                let prev_end = lyrics.lines[k].end_sec;
                let next_start = lyrics.lines[k + 1].start_sec;
                let gap = next_start - prev_end;
                if gap < Self::INTERLUDE_MIN_SEC {
                    continue;
                }
                let was_this = self.interlude_latched == Some(Some(k));
                let enter_t = if was_this {
                    prev_end + 0.10
                } else {
                    prev_end + 0.22
                };
                let exit_t = if was_this {
                    next_start - 0.45
                } else {
                    next_start - 0.55
                };
                if playhead_sec >= enter_t && playhead_sec < exit_t {
                    interlude_vis = true;
                    interlude_after = Some(k);
                    let eff_span = (exit_t - prev_end).max(0.1);
                    interlude_prog = ((playhead_sec - prev_end) / eff_span).clamp(0.0, 1.0) as f32;
                    let into = (playhead_sec - prev_end) as f32;
                    let rem = (exit_t - playhead_sec).max(0.0) as f32;
                    interlude_env = (into / 0.42).min(rem / 0.35).clamp(0.0, 1.0);
                    break;
                }
            }
        }
        self.interlude_latched = if interlude_vis {
            Some(interlude_after)
        } else {
            None
        };

        self.interlude = InterludeDotsState {
            visible: interlude_vis,
            after_line_idx: interlude_after,
            progress: interlude_prog,
            scale_alpha: interlude_env,
        };
        self.interlude_height_spring.target = if interlude_vis {
            Self::INTERLUDE_HEIGHT + 16.0
        } else {
            0.0
        };
        self.interlude_height_spring.step(dt_sec);

        // 2. Find up to `Specs.maxSelectedLines = 2` active lines:
        //    CRITICAL (`0x32271f: subsd 0.5`): `Specs.maxEndTimeOffset = 0.50` is SUBTRACTED from
        //    an overlapping line's `end_sec` for early handoff — NEVER added past `next_line.start_sec`!
        let mut active_1: Option<usize> = None;
        let mut active_2: Option<usize> = None;
        for (i, line) in lyrics.lines.iter().enumerate() {
            if t_line < line.start_sec {
                break;
            }
            let next_start_opt = lyrics.lines.get(i + 1).map(|n| n.start_sec);
            let effective_end = match next_start_opt {
                Some(ns) if line.end_sec <= ns + 0.05 => {
                    let gap = ns - line.end_sec;
                    if gap < Self::INTERLUDE_MIN_SEC {
                        // Brief afterglow up to `line.end_sec + 0.32` (never past `ns`), then smoothly
                        // hand off to `line[i+1]` for `[handoff_t .. ns)` so `<7s` gaps never flash blurry!
                        (line.end_sec + 0.32).min(ns)
                    } else {
                        (line.end_sec + 0.32).min(ns)
                    }
                }
                Some(ns) => {
                    // Genuine overlapping duet (`line.end_sec > ns + 0.05`): allow early release up to
                    // `MAX_END_TIME_OFFSET_SEC` before `line.end_sec` once a 3rd line also enters,
                    // and never hold past `line.end_sec`.
                    let third_entered = lyrics
                        .lines
                        .get(i + 2)
                        .is_some_and(|l3| t_line >= l3.start_sec);
                    if third_entered {
                        (line.end_sec - Self::MAX_END_TIME_OFFSET_SEC).max(ns)
                    } else {
                        line.end_sec
                    }
                }
                None => line.end_sec + 0.85,
            };

            if t_line < effective_end {
                if active_1.is_none() {
                    active_1 = Some(i);
                } else if active_2.is_none() {
                    active_2 = Some(i);
                } else {
                    active_1 = active_2;
                    active_2 = Some(i);
                }
            }
        }

        // Smooth handoff during non-interlude gaps `<7.0s` + pre-warm right after >=7.0s interlude exit:
        // prevents lines from ever flashing dimmed/shrunk/blurred during normal breaths between verses!
        if active_1.is_none() && !interlude_vis {
            if t_line < first_start {
                active_1 = Some(0);
            } else {
                for k in 0..lyrics.lines.len().saturating_sub(1) {
                    let prev_end = lyrics.lines[k].end_sec;
                    let next_start = lyrics.lines[k + 1].start_sec;
                    if t_line >= prev_end && t_line < next_start {
                        let gap = next_start - prev_end;
                        if gap < Self::INTERLUDE_MIN_SEC || playhead_sec >= next_start - 0.45 {
                            active_1 = Some(k + 1);
                        }
                        break;
                    }
                }
            }
        }

        // 3. Anchor line index (`Specs.selectedLinePosition` at ~24% of viewport height on iPad)
        let anchor_idx = if interlude_vis {
            match interlude_after {
                None => 0,
                Some(i) => (i + 1).min(lyrics.lines.len() - 1),
            }
        } else if let (Some(i1), Some(i2)) = (active_1, active_2) {
            // In overlapping duets, advance scroll anchor to `i2` once `i1` enters its final `0.50s` (`subsd 0.5`)
            if t_line >= (lyrics.lines[i1].end_sec - Self::MAX_END_TIME_OFFSET_SEC) {
                i2
            } else {
                i1
            }
        } else if let Some(i1) = active_1 {
            i1
        } else {
            // Unselected gap (< 7s) or post-outro: rest calmly at the most recently sung line (or 0 before intro)
            let mut last_past: Option<usize> = None;
            for (i, line) in lyrics.lines.iter().enumerate() {
                if t_line >= line.start_sec {
                    last_past = Some(i);
                } else {
                    break;
                }
            }
            last_past.unwrap_or(0)
        };

        let scroll_ref_idx = active_1.unwrap_or(anchor_idx);
        let line_changed = self.primary_active_idx != active_1;
        self.primary_active_idx = active_1;
        self.secondary_active_idx = active_2;

        // Responsive vertical rhythm for large/fullscreen windows
        let vp_scale = (viewport_h / 720.0).clamp(0.92, 1.55);
        let dyn_line_spacing = Self::LINE_SPACING_IPAD * vp_scale;

        // Compute cumulative vertical layout coordinates
        let mut rel_y = vec![0.0f32; lyrics.lines.len()];
        let mut cursor = 0.0f32;
        if interlude_after.is_none() && self.interlude_height_spring.value > 0.5 {
            cursor += self.interlude_height_spring.value;
        }
        for i in 0..lyrics.lines.len() {
            rel_y[i] = cursor;
            cursor += self.line_states[i].measured_height + dyn_line_spacing;
            if interlude_after == Some(i) && self.interlude_height_spring.value > 0.5 {
                cursor += self.interlude_height_spring.value;
            }
        }

        let selected_screen_y = (viewport_h * 0.24).clamp(68.0, 380.0);
        let anchor_rel = rel_y.get(anchor_idx).copied().unwrap_or(0.0);

        for i in 0..lyrics.lines.len() {
            let desired_y = selected_screen_y + (rel_y[i] - anchor_rel);
            let st = &mut self.line_states[i];

            if !self.layout_initialized {
                st.y_spring.snap(desired_y);
                st.pending_target_y = desired_y;
                st.stagger_delay_rem = 0.0;
            } else if (st.pending_target_y - desired_y).abs() > 0.5 {
                st.pending_target_y = desired_y;
                if line_changed && self.user_scroll_until.is_none() && active_1.is_some() {
                    // Cascade wave stagger: lines from anchor downward stagger by 50ms each (`Specs.lineDelay = 0.05`)
                    let rank = if i >= scroll_ref_idx {
                        (i - scroll_ref_idx).min(9)
                    } else {
                        0
                    };
                    st.stagger_delay_rem = (rank as f32) * Self::LINE_DELAY_SEC;
                } else if st.stagger_delay_rem <= 0.0 {
                    st.y_spring.target = desired_y;
                }
            }

            if st.stagger_delay_rem > 0.0 {
                st.stagger_delay_rem = (st.stagger_delay_rem - dt_sec).max(0.0);
                if st.stagger_delay_rem <= 0.0 {
                    st.y_spring.target = st.pending_target_y;
                }
            } else {
                st.y_spring.target = st.pending_target_y;
            }

            let is_sel = !interlude_vis && (active_1 == Some(i) || active_2 == Some(i));
            st.active_blend.target = if is_sel { 1.0 } else { 0.0 };
            st.scale_spring.target = if is_sel { 1.0 } else { Self::DESELECTED_SCALE };

            // Optical depth-of-field blur target (0.0 when selected, hovered, or manually scrolling)
            let dist = (i as isize - anchor_idx as isize).unsigned_abs();
            let is_hov = self.hovered_line == Some(i);
            let target_sigma = if is_sel || is_hov || self.user_scroll_until.is_some() {
                0.0
            } else {
                match dist {
                    0 => 1.35,
                    1 => 1.85,
                    2 => 3.15,
                    3 => 4.45,
                    _ => 5.65,
                }
            };
            if !self.layout_initialized {
                st.blur_spring.snap(target_sigma);
            } else {
                st.blur_spring.target = target_sigma;
            }

            st.y_spring.step(dt_sec);
            st.scale_spring.step(dt_sec);
            st.press_spring.step(dt_sec);
            st.active_blend.step(dt_sec);
            st.blur_spring.step(dt_sec);

            let line = &lyrics.lines[i];
            // Unified +0.10s word time (`synced_playhead + 0.10`, identical to sweep!) with per-line `<0.50s` monotonic clamp
            let t_word = if is_sel {
                let raw_tw = playhead_sec + Self::SWEEP_HEADSTART_SEC;
                let clamped = st.clamped_word_time(raw_tw);
                st.max_word_t = clamped;
                clamped
            } else {
                if playhead_sec + 0.50 < line.start_sec {
                    st.max_word_t = -1.0;
                }
                playhead_sec + Self::SWEEP_HEADSTART_SEC
            };

            // Step per-syllable lift springs ONLY when `line.has_word_sync` is true!
            // When `!is_sel`, return to baseline briskly so post-line blur handoff happens smoothly and promptly.
            let lift_dt = if is_sel { dt_sec } else { (dt_sec * 2.5).min(0.05) };
            if st.syllable_lifts.len() != line.syllables.len() {
                st.syllable_lifts = vec![Spring1D::syllable_lift(0.0); line.syllables.len()];
            }
            for (s_idx, syl) in line.syllables.iter().enumerate() {
                let lift_target = if is_sel && line.has_word_sync && t_word >= syl.start_sec {
                    -Self::SYLLABLE_LIFT_PX
                } else {
                    0.0
                };
                st.syllable_lifts[s_idx].target = lift_target;
                st.syllable_lifts[s_idx].step(lift_dt);
            }

            if st.bg_syllable_lifts.len() != line.bg_syllables.len() {
                st.bg_syllable_lifts = vec![Spring1D::syllable_lift(0.0); line.bg_syllables.len()];
            }
            for (b_idx, bsyl) in line.bg_syllables.iter().enumerate() {
                let lift_target = if is_sel && line.has_word_sync && t_word >= bsyl.start_sec {
                    -(Self::SYLLABLE_LIFT_PX * 0.8)
                } else {
                    0.0
                };
                st.bg_syllable_lifts[b_idx].target = lift_target;
                st.bg_syllable_lifts[b_idx].step(lift_dt);
            }

            // Step per-unit (`字` for CJK; `word` or `char` for Latin per user switch!) lift springs!
            let char_lift_dist = Self::CHAR_LIFT_PX * vp_scale.clamp(0.95, 1.35);
            let active_units = line.units_for_mode(self.latin_by_word);
            if st.char_lifts.len() != active_units.len() {
                st.char_lifts = vec![Spring1D::char_lift(0.0); active_units.len()];
            }
            for (c_idx, cu) in active_units.iter().enumerate() {
                let lift_target = if is_sel && line.has_word_sync && t_word > cu.start_sec {
                    let ramp = (cu.end_sec - cu.start_sec).clamp(0.035, 0.065);
                    let u = ((t_word - cu.start_sec) / ramp).clamp(0.0, 1.0) as f32;
                    -char_lift_dist * (u * u * (3.0 - 2.0 * u))
                } else {
                    0.0
                };
                st.char_lifts[c_idx].target = lift_target;
                st.char_lifts[c_idx].step(lift_dt);
            }

            let active_bg_units = line.bg_units_for_mode(self.latin_by_word);
            if st.bg_char_lifts.len() != active_bg_units.len() {
                st.bg_char_lifts = vec![Spring1D::char_lift(0.0); active_bg_units.len()];
            }
            for (bc_idx, bcu) in active_bg_units.iter().enumerate() {
                let lift_target = if is_sel && line.has_word_sync && t_word > bcu.start_sec {
                    let ramp = (bcu.end_sec - bcu.start_sec).clamp(0.035, 0.065);
                    let u = ((t_word - bcu.start_sec) / ramp).clamp(0.0, 1.0) as f32;
                    -(char_lift_dist * 0.80) * (u * u * (3.0 - 2.0 * u))
                } else {
                    0.0
                };
                st.bg_char_lifts[bc_idx].target = lift_target;
                st.bg_char_lifts[bc_idx].step(lift_dt);
            }
        }

        self.layout_initialized = true;
    }
}

// ============================================================================
// Parsers: TTML (Apple Music & AMLL), YRC (NetEase Word-by-Word), LRC
// ============================================================================

pub fn parse_timestamp_str(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(ms_str) = s.strip_suffix("ms") {
        return ms_str.trim().parse::<f64>().ok().map(|v| v / 1000.0);
    }
    if let Some(sec_str) = s.strip_suffix('s') {
        return sec_str.trim().parse::<f64>().ok();
    }
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        1 => parts[0].parse::<f64>().ok(),
        2 => {
            let m = parts[0].parse::<f64>().ok()?;
            let sec = parts[1].parse::<f64>().ok()?;
            Some(m * 60.0 + sec)
        }
        3 => {
            let h = parts[0].parse::<f64>().ok()?;
            let m = parts[1].parse::<f64>().ok()?;
            let sec = parts[2].parse::<f64>().ok()?;
            Some(h * 3600.0 + m * 60.0 + sec)
        }
        4 => {
            // W3C TTML `HH:MM:SS:FF` frame-qualified clock time (30fps nominal)
            let h = parts[0].parse::<f64>().ok()?;
            let m = parts[1].parse::<f64>().ok()?;
            let sec = parts[2].parse::<f64>().ok()?;
            let frames = parts[3].parse::<f64>().ok()?;
            Some(h * 3600.0 + m * 60.0 + sec + frames / 30.0)
        }
        _ => None,
    }
}

fn unescape_xml_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn contains_rtl_chars(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

fn extract_xml_attr<'a>(tag_head: &'a str, attr_name: &str) -> Option<&'a str> {
    let bytes = tag_head.as_bytes();
    let name_b = attr_name.as_bytes();
    if name_b.is_empty() || bytes.len() <= name_b.len() {
        return None;
    }
    let mut search_from = 0usize;
    while let Some(rel) = tag_head[search_from..].find(attr_name) {
        let idx = search_from + rel;
        let boundary_ok = idx == 0
            || matches!(
                bytes[idx - 1],
                b' ' | b'\t' | b'\r' | b'\n' | b'<' | b'"' | b'\'' | b'/'
            );
        let after_name = idx + name_b.len();
        search_from = after_name;
        if !boundary_ok {
            continue;
        }
        let mut cur = after_name;
        while cur < bytes.len() && bytes[cur].is_ascii_whitespace() {
            cur += 1;
        }
        if cur >= bytes.len() || bytes[cur] != b'=' {
            continue;
        }
        cur += 1;
        while cur < bytes.len() && bytes[cur].is_ascii_whitespace() {
            cur += 1;
        }
        if cur >= bytes.len() {
            return None;
        }
        let q = bytes[cur];
        if q != b'"' && q != b'\'' {
            continue;
        }
        let val_start = cur + 1;
        let quote_ch = q as char;
        if let Some(end_rel) = tag_head[val_start..].find(quote_ch) {
            return Some(&tag_head[val_start..val_start + end_rel]);
        }
    }
    None
}

fn strip_all_xml_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    unescape_xml_entities(&out)
}

fn is_combining_or_modifier(c: char) -> bool {
    matches!(
        c as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0x200D
    )
}

/// Returns `true` for Chinese Hanzi / CJK Ideographs, Japanese Hiragana / Katakana,
/// Korean Hangul / Jamo, and Bopomofo characters (which always animate per-`字`).
pub fn is_cjk_char(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x11FF // Hangul Jamo
            | 0x2E80..=0x2FDF // CJK Radicals / Kangxi
            | 0x3005..=0x3007 // Ideographic iteration / zero (`々`, `〆`, `〇`)
            | 0x3040..=0x30FF // Hiragana & Katakana
            | 0x3100..=0x312F // Bopomofo
            | 0x3130..=0x318F // Hangul Compatibility Jamo
            | 0x31F0..=0x31FF // Katakana Phonetic Extensions
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xA960..=0xA97F // Hangul Jamo Extended-A
            | 0xAC00..=0xD7FF // Hangul Syllables & Extended-B
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFF65..=0xFF9F // Halfwidth Katakana
            | 0x20000..=0x323AF // CJK Unified Ideographs Extensions B..G
    )
}

/// Subdivides multi-character syllables/words into individual character (`字`) units across ALL scripts
/// (used when the user explicitly disables English per-word mode).
pub fn build_char_units_from_syllables(syllables: &[LyricSyllable]) -> Vec<LyricCharUnit> {
    let mut out = Vec::new();
    for syl in syllables {
        if syl.utf16_len == 0 || syl.text.is_empty() {
            continue;
        }
        let mut clusters: Vec<(u32, u32, String, f64)> = Vec::new();
        let mut rel_u16 = 0u32;
        for ch in syl.text.chars() {
            let cu16 = ch.len_utf16() as u32;
            if ch.is_whitespace() {
                rel_u16 += cu16;
                continue;
            }
            if is_combining_or_modifier(ch) {
                if let Some(last) = clusters.last_mut() {
                    last.1 += cu16;
                    last.2.push(ch);
                    rel_u16 += cu16;
                    continue;
                }
            }
            let w = if ch.is_alphanumeric() || is_cjk_char(ch) {
                1.0
            } else {
                0.35
            };
            clusters.push((rel_u16, cu16, ch.to_string(), w));
            rel_u16 += cu16;
        }
        if clusters.is_empty() {
            continue;
        }
        let word_dur = (syl.end_sec - syl.start_sec).max(0.03);
        let total_w: f64 = clusters.iter().map(|c| c.3).sum::<f64>().max(0.01);
        let mut acc_t = syl.start_sec;
        for (rel_s, u16_len, ch_str, w) in clusters {
            let dt = word_dur * (w / total_w);
            let c_start = acc_t;
            let c_end = (acc_t + dt).max(c_start + 0.015);
            acc_t += dt;
            out.push(LyricCharUnit {
                ch_str,
                start_sec: c_start,
                end_sec: c_end,
                word_dur_sec: word_dur,
                utf16_start: syl.utf16_start + rel_s,
                utf16_len: u16_len,
                is_sustained: syl.is_sustained,
                is_explicit_emphasis: syl.is_explicit_emphasis,
                sub_spans: Vec::new(),
            });
        }
    }
    out
}

/// Default hybrid unit builder:
/// - CJK ideographic/syllabic scripts (Chinese, Japanese, Korean) subdivide into individual `字` units;
/// - English / Latin / alphabetic scripts stay grouped as whole `word` units (even across multi-syllable
///   TTML spans like `"beau"+"ti"+"ful"`, while splitting multi-word spans like `"In the night"` at whitespace!),
///   preserving piecewise syllable timing keyframes (`sub_spans`) and per-syllable `is_sustained`!
pub fn build_word_cjk_units_from_syllables(syllables: &[LyricSyllable]) -> Vec<LyricCharUnit> {
    struct TimedAtom {
        u16_start: u32,
        u16_len: u32,
        txt: String,
        weight: f64,
        start_sec: f64,
        end_sec: f64,
        word_dur_sec: f64,
        is_sustained: bool,
        is_explicit_emphasis: bool,
        preceded_by_ws: bool,
        is_cjk: bool,
        is_punct_only: bool,
    }

    let mut atoms: Vec<TimedAtom> = Vec::new();
    let mut saw_ws = true;

    for syl in syllables {
        if syl.utf16_len == 0 || syl.text.is_empty() {
            continue;
        }
        let mut clusters: Vec<(u32, u32, String, f64, bool, bool, bool)> = Vec::new();
        let mut rel_u16 = 0u32;
        for ch in syl.text.chars() {
            let cu16 = ch.len_utf16() as u32;
            if ch.is_whitespace() {
                saw_ws = true;
                rel_u16 += cu16;
                continue;
            }
            if is_combining_or_modifier(ch) {
                if let Some(last) = clusters.last_mut() {
                    last.1 += cu16;
                    last.2.push(ch);
                    rel_u16 += cu16;
                    continue;
                }
            }
            let cjk = is_cjk_char(ch);
            let punct = !cjk && !ch.is_alphanumeric();
            let w = if cjk || ch.is_alphanumeric() { 1.0 } else { 0.25 };
            clusters.push((rel_u16, cu16, ch.to_string(), w, saw_ws, cjk, punct));
            saw_ws = false;
            rel_u16 += cu16;
        }
        if clusters.is_empty() {
            continue;
        }
        let span_dur = (syl.end_sec - syl.start_sec).max(0.03);
        let total_w: f64 = clusters.iter().map(|c| c.3).sum::<f64>().max(0.01);
        let mut acc_t = syl.start_sec;
        for (rel_s, u16_len, txt, w, prec_ws, cjk, punct) in clusters {
            let dt = span_dur * (w / total_w);
            let c_start = acc_t;
            let c_end = (acc_t + dt).max(c_start + 0.012);
            acc_t += dt;
            atoms.push(TimedAtom {
                u16_start: syl.utf16_start + rel_s,
                u16_len,
                txt,
                weight: w,
                start_sec: c_start,
                end_sec: c_end,
                word_dur_sec: span_dur,
                is_sustained: syl.is_sustained,
                is_explicit_emphasis: syl.is_explicit_emphasis,
                preceded_by_ws: prec_ws,
                is_cjk: cjk,
                is_punct_only: punct,
            });
        }
    }

    let mut out: Vec<LyricCharUnit> = Vec::with_capacity(atoms.len());
    let mut unit_atom_spans: Vec<Vec<(f64, f64, f64)>> = Vec::with_capacity(atoms.len());
    let mut last_was_cjk = false;

    for a in atoms {
        if a.is_cjk {
            // If immediately preceded by opening quote/bracket with no space (e.g. `「温`), attach prefix cleanly
            let attach_open_punct = if let Some(prev) = out.last_mut() {
                !last_was_cjk
                    && !a.preceded_by_ws
                    && prev.utf16_start + prev.utf16_len == a.u16_start
                    && prev.ch_str.chars().all(|c| !c.is_alphanumeric() && !is_cjk_char(c))
            } else {
                false
            };
            if attach_open_punct {
                let prev = out.last_mut().unwrap();
                prev.ch_str.push_str(&a.txt);
                prev.utf16_len += a.u16_len;
                prev.end_sec = a.end_sec;
                prev.word_dur_sec = a.word_dur_sec;
                prev.is_sustained |= a.is_sustained;
                prev.is_explicit_emphasis |= a.is_explicit_emphasis;
                if let Some(sp) = unit_atom_spans.last_mut() {
                    sp.push((a.weight, a.start_sec, a.end_sec));
                }
            } else {
                out.push(LyricCharUnit {
                    ch_str: a.txt,
                    start_sec: a.start_sec,
                    end_sec: a.end_sec,
                    word_dur_sec: a.word_dur_sec,
                    utf16_start: a.u16_start,
                    utf16_len: a.u16_len,
                    is_sustained: a.is_sustained,
                    is_explicit_emphasis: a.is_explicit_emphasis,
                    sub_spans: Vec::new(),
                });
                unit_atom_spans.push(vec![(a.weight, a.start_sec, a.end_sec)]);
            }
            last_was_cjk = true;
        } else if a.is_punct_only {
            // Attach apostrophes/hyphens/trailing punctuation (`don't`, `Hello,`, `星空！`) to contiguous preceding unit
            let can_attach = out.last().is_some_and(|prev| {
                !a.preceded_by_ws && prev.utf16_start + prev.utf16_len == a.u16_start
            });
            if can_attach {
                let prev = out.last_mut().unwrap();
                prev.ch_str.push_str(&a.txt);
                prev.utf16_len += a.u16_len;
                prev.end_sec = prev.end_sec.max(a.end_sec);
                prev.is_sustained |= a.is_sustained;
                prev.is_explicit_emphasis |= a.is_explicit_emphasis;
                if !last_was_cjk {
                    prev.word_dur_sec = (prev.end_sec - prev.start_sec).max(prev.word_dur_sec);
                }
                if let Some(sp) = unit_atom_spans.last_mut() {
                    sp.push((a.weight, a.start_sec, a.end_sec));
                }
            } else {
                out.push(LyricCharUnit {
                    ch_str: a.txt,
                    start_sec: a.start_sec,
                    end_sec: a.end_sec,
                    word_dur_sec: a.word_dur_sec,
                    utf16_start: a.u16_start,
                    utf16_len: a.u16_len,
                    is_sustained: a.is_sustained,
                    is_explicit_emphasis: a.is_explicit_emphasis,
                    sub_spans: Vec::new(),
                });
                unit_atom_spans.push(vec![(a.weight, a.start_sec, a.end_sec)]);
                last_was_cjk = false;
            }
        } else {
            // Latin / alphabetic / numeric character: merge into current non-CJK word if no whitespace separates them!
            let can_merge_word = out.last().is_some_and(|prev| {
                !last_was_cjk
                    && !a.preceded_by_ws
                    && prev.utf16_start + prev.utf16_len == a.u16_start
            });
            if can_merge_word {
                let prev = out.last_mut().unwrap();
                prev.ch_str.push_str(&a.txt);
                prev.utf16_len += a.u16_len;
                prev.end_sec = a.end_sec.max(prev.end_sec);
                let dur = (prev.end_sec - prev.start_sec).max(0.03);
                prev.word_dur_sec = dur;
                // Strictly preserve per-syllable sustained/explicit emphasis flags
                prev.is_sustained |= a.is_sustained;
                prev.is_explicit_emphasis |= a.is_explicit_emphasis;
                if let Some(sp) = unit_atom_spans.last_mut() {
                    sp.push((a.weight, a.start_sec, a.end_sec));
                }
            } else {
                out.push(LyricCharUnit {
                    ch_str: a.txt,
                    start_sec: a.start_sec,
                    end_sec: a.end_sec,
                    word_dur_sec: a.word_dur_sec,
                    utf16_start: a.u16_start,
                    utf16_len: a.u16_len,
                    is_sustained: a.is_sustained,
                    is_explicit_emphasis: a.is_explicit_emphasis,
                    sub_spans: Vec::new(),
                });
                unit_atom_spans.push(vec![(a.weight, a.start_sec, a.end_sec)]);
            }
            last_was_cjk = false;
        }
    }

    for (unit, spans) in out.iter_mut().zip(unit_atom_spans.into_iter()) {
        if spans.len() > 1 {
            let tot_w: f64 = spans.iter().map(|s| s.0).sum::<f64>().max(0.01);
            let mut acc_w = 0.0f64;
            let mut sub = Vec::with_capacity(spans.len());
            for (w, s0, s1) in spans {
                let f0 = (acc_w / tot_w) as f32;
                acc_w += w;
                let f1 = (acc_w / tot_w) as f32;
                sub.push(LyricSubSpan {
                    frac_start: f0,
                    frac_end: f1,
                    start_sec: s0,
                    end_sec: s1,
                });
            }
            unit.sub_spans = sub;
        }
    }

    out
}

fn recompute_utf16_offsets(text: &mut String, syllables: &mut [LyricSyllable]) {
    let mut rebuilt = String::new();
    let mut u16_cursor = 0u32;
    for syl in syllables.iter_mut() {
        let u16_len = syl.text.encode_utf16().count() as u32;
        syl.utf16_start = u16_cursor;
        syl.utf16_len = u16_len;
        let dur = syl.end_sec - syl.start_sec;
        let trimmed_chars = syl.text.trim().chars().count();
        let avg_char_dur = dur / (trimmed_chars.max(1) as f64);
        // Preserve explicit TTML emphasis markup; otherwise detect genuinely sustained vocal notes:
        // - Long held note >= 1.00s on a concise word/syllable (<= 8 chars), OR
        // - Clearly drawn-out vowel/syllable >= 0.85s with >= 0.42s per character
        let sustained_by_dur = trimmed_chars > 0
            && (((1.00..=30.0).contains(&dur) && trimmed_chars <= 8)
                || ((0.85..=30.0).contains(&dur)
                    && trimmed_chars <= 6
                    && avg_char_dur >= 0.42));
        syl.is_sustained = syl.is_explicit_emphasis || syl.is_sustained || sustained_by_dur;
        rebuilt.push_str(&syl.text);
        u16_cursor += u16_len;
    }
    *text = rebuilt;
}

/// Subdivides a line `[start_sec, end_sec]` into natural grapheme/word syllables when explicitly requested
pub fn synthesize_natural_syllables(text: &str, start_sec: f64, end_sec: f64) -> Vec<LyricSyllable> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();

    for ch in text.chars() {
        let is_cjk = matches!(
            ch as u32,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF
        );
        if is_cjk {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
            tokens.push(ch.to_string());
        } else if ch.is_whitespace() {
            cur.push(ch);
            tokens.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    if tokens.is_empty() {
        return Vec::new();
    }

    let weights: Vec<f64> = tokens
        .iter()
        .map(|tok| {
            let trimmed = tok.trim();
            if trimmed.is_empty() {
                0.15
            } else {
                let c_len = trimmed.chars().count();
                if c_len == 1
                    && trimmed.chars().all(|c| {
                        matches!(
                            c as u32,
                            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF
                        )
                    })
                {
                    1.0
                } else {
                    0.65 + (c_len as f64) * 0.14
                }
            }
        })
        .collect();

    let total_w: f64 = weights.iter().sum::<f64>().max(0.01);
    let active_span = ((end_sec - start_sec) * 0.90).clamp(0.25, 12.0);
    let mut acc_t = start_sec;
    let mut u16_pos = 0u32;
    let mut out = Vec::with_capacity(tokens.len());

    for (tok, w) in tokens.into_iter().zip(weights.into_iter()) {
        let dur = active_span * (w / total_w);
        let u16_len = tok.encode_utf16().count() as u32;
        out.push(LyricSyllable {
            text: tok,
            start_sec: acc_t,
            end_sec: acc_t + dur,
            utf16_start: u16_pos,
            utf16_len: u16_len,
            is_sustained: false,
            is_explicit_emphasis: false,
        });
        acc_t += dur;
        u16_pos += u16_len;
    }
    out
}

/// Strips nested `<span ttm:role="x-translation">` and `<span ttm:role="x-roman">` blocks from a TTML fragment.
fn strip_ttml_aux_role_spans(fragment: &str) -> String {
    let mut cleaned = String::with_capacity(fragment.len());
    let mut cursor = 0usize;
    while let Some(rel_open) = fragment[cursor..].find("<span") {
        let open_idx = cursor + rel_open;
        cleaned.push_str(&fragment[cursor..open_idx]);
        let Some(rel_gt) = fragment[open_idx..].find('>') else {
            cleaned.push_str(&fragment[open_idx..]);
            cursor = fragment.len();
            break;
        };
        let head = &fragment[open_idx..open_idx + rel_gt + 1];
        let role = extract_xml_attr(head, "ttm:role").unwrap_or("");
        if role == "x-translation" || role == "x-roman" {
            if head.ends_with("/>") {
                cursor = open_idx + rel_gt + 1;
                continue;
            }
            let mut depth = 1i32;
            let mut scan = open_idx + rel_gt + 1;
            while scan < fragment.len() {
                if fragment[scan..].starts_with("<span") {
                    let sub_self_close = fragment[scan..]
                        .find('>')
                        .is_some_and(|g| fragment[scan..scan + g + 1].ends_with("/>"));
                    if !sub_self_close {
                        depth += 1;
                    }
                    scan += 5;
                } else if fragment[scan..].starts_with("</span>") {
                    depth -= 1;
                    scan += 7;
                    if depth == 0 {
                        break;
                    }
                } else {
                    let ch_len = fragment[scan..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(1);
                    scan += ch_len;
                }
            }
            cursor = scan;
        } else {
            cleaned.push_str(head);
            cursor = open_idx + rel_gt + 1;
        }
    }
    if cursor < fragment.len() {
        cleaned.push_str(&fragment[cursor..]);
    }
    cleaned
}

/// Parses Apple Music / AMLL TTML XML into `ParsedLyrics` (supporting syllable spans, x-bg backing vocals, x-translation, x-roman stripping, duet agents, and songwriters)
pub fn parse_ttml_lyrics(ttml: &str, source_label: &str) -> Option<ParsedLyrics> {
    static P_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<p\b([^>]*)>(.*?)</p>").expect("valid p regex"));
    static SPAN_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<span\b([^>]*)>(.*?)</span>").expect("valid span regex"));
    static SW_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?s)<songwriter\b[^>]*>(.*?)</songwriter>").expect("valid songwriter regex")
    });
    static TRANSLATIONS_CONTAINER_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?s)<translations(?:\s[^>]*?[^/])?>(.*?)</translations>")
            .expect("valid translations container regex")
    });
    static TRANSLATION_LANG_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?s)<translation(?:\s([^>]*?[^/]))?>(.*?)</translation>")
            .expect("valid translation regex")
    });
    static TRANS_BLOCK_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"(?s)<text\b[^>]*\bfor=["']([^"']+)["'][^>]*>(.*?)</text>"#)
            .expect("valid trans text regex")
    });

    let ttml = ttml.trim_start_matches('\u{feff}');

    // Reject unsynced static TTML (`itunes:timing="None"`) when no timed `<p>` or `<span>` exists
    if let Some(tt_open) = ttml.find("<tt") {
        if let Some(tt_gt) = ttml[tt_open..].find('>') {
            let tt_head = &ttml[tt_open..tt_open + tt_gt + 1];
            if extract_xml_attr(tt_head, "itunes:timing")
                .is_some_and(|v| v.eq_ignore_ascii_case("none"))
            {
                return None;
            }
        }
    }

    let mut songwriters = Vec::new();
    for cap in SW_RE.captures_iter(ttml) {
        let name = strip_all_xml_tags(&cap[1]).trim().to_string();
        if !name.is_empty() && !songwriters.contains(&name) {
            songwriters.push(name);
        }
    }

    // Header `<translations><translation><text for="L1">...` map by `itunes:key`:
    // STRICTLY scoped inside `<translations>...</translations>` so `<transliterations>` Romaji
    // (`muteki no egao de...`) NEVER leaks into `header_translations`!
    let mut zh_header_translations: HashMap<String, String> = HashMap::new();
    let mut any_header_translations: HashMap<String, String> = HashMap::new();
    for c_cap in TRANSLATIONS_CONTAINER_RE.captures_iter(ttml) {
        let tr_container = &c_cap[1];
        let mut had_lang_blocks = false;
        for t_cap in TRANSLATION_LANG_RE.captures_iter(tr_container) {
            had_lang_blocks = true;
            let t_attrs = t_cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let t_body = &t_cap[2];
            let is_zh = extract_xml_attr(t_attrs, "xml:lang")
                .or_else(|| extract_xml_attr(t_attrs, "lang"))
                .is_some_and(|l| l.trim().to_ascii_lowercase().starts_with("zh"));
            for cap in TRANS_BLOCK_RE.captures_iter(t_body) {
                let key = cap[1].trim().to_string();
                let val = strip_all_xml_tags(&cap[2]).trim().to_string();
                if !key.is_empty() && !val.is_empty() {
                    if is_zh {
                        zh_header_translations.entry(key.clone()).or_insert(val.clone());
                    }
                    any_header_translations.entry(key).or_insert(val);
                }
            }
        }
        if !had_lang_blocks {
            for cap in TRANS_BLOCK_RE.captures_iter(tr_container) {
                let key = cap[1].trim().to_string();
                let val = strip_all_xml_tags(&cap[2]).trim().to_string();
                if !key.is_empty() && !val.is_empty() {
                    any_header_translations.entry(key).or_insert(val);
                }
            }
        }
    }
    let header_translations = if !zh_header_translations.is_empty() {
        zh_header_translations
    } else {
        any_header_translations
    };

    let mut primary_agent: Option<String> = None;
    let mut any_word_sync = false;
    let mut lines = Vec::new();

    for cap in P_RE.captures_iter(ttml) {
        let p_attrs = &cap[1];
        let mut p_body = cap[2].to_string();

        let p_begin = extract_xml_attr(p_attrs, "begin").and_then(parse_timestamp_str);
        let p_dur = extract_xml_attr(p_attrs, "dur").and_then(parse_timestamp_str);
        let p_end = extract_xml_attr(p_attrs, "end")
            .and_then(parse_timestamp_str)
            .or_else(|| p_begin.zip(p_dur).map(|(b, d)| b + d));
        let itunes_key = extract_xml_attr(p_attrs, "itunes:key").map(|s| s.to_string());
        let agent = extract_xml_attr(p_attrs, "ttm:agent").map(|s| s.to_string());

        let is_duet_secondary = if let Some(ref ag) = agent {
            if ag == "v2" || ag == "v2000" {
                true
            } else if let Some(ref prim) = primary_agent {
                prim != ag
            } else {
                primary_agent = Some(ag.clone());
                false
            }
        } else {
            false
        };

        let mut inline_translation: Option<String> = None;
        let mut bg_syllables: Vec<LyricSyllable> = Vec::new();
        let mut bg_text_opt: Option<String> = None;

        // Extract nested `<span ttm:role="x-translation">`, `<span ttm:role="x-bg">`, and strip `<span ttm:role="x-roman">`
        let mut cleaned_body = String::with_capacity(p_body.len());
        let mut cursor = 0usize;
        while let Some(rel_open) = p_body[cursor..].find("<span") {
            let open_idx = cursor + rel_open;
            cleaned_body.push_str(&p_body[cursor..open_idx]);
            let Some(rel_gt) = p_body[open_idx..].find('>') else {
                cleaned_body.push_str(&p_body[open_idx..]);
                cursor = p_body.len();
                break;
            };
            let head = &p_body[open_idx..open_idx + rel_gt + 1];
            let role = extract_xml_attr(head, "ttm:role").unwrap_or("");
            if role == "x-translation" || role == "x-bg" || role == "x-roman" {
                if head.ends_with("/>") {
                    cursor = open_idx + rel_gt + 1;
                    continue;
                }
                // Find matching `</span>` accounting for nested syllable `<span>` inside `x-bg` / `x-roman`!
                let mut depth = 1i32;
                let mut scan = open_idx + rel_gt + 1;
                let inner_start = scan;
                let mut inner_end = p_body.len();
                while scan < p_body.len() {
                    if p_body[scan..].starts_with("<span") {
                        let sub_self_close = p_body[scan..]
                            .find('>')
                            .is_some_and(|g| p_body[scan..scan + g + 1].ends_with("/>"));
                        if !sub_self_close {
                            depth += 1;
                        }
                        scan += 5;
                    } else if p_body[scan..].starts_with("</span>") {
                        depth -= 1;
                        if depth == 0 {
                            inner_end = scan;
                            scan += 7;
                            break;
                        }
                        scan += 7;
                    } else {
                        let ch_len = p_body[scan..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                        scan += ch_len;
                    }
                }
                let inner = &p_body[inner_start..inner_end];
                if role == "x-translation" {
                    let tr = strip_all_xml_tags(inner).trim().to_string();
                    if !tr.is_empty() {
                        inline_translation = Some(tr);
                    }
                } else if role == "x-bg" {
                    let bg_vocal_only = strip_ttml_aux_role_spans(inner);
                    let mut bg_syls = parse_ttml_inline_spans(&bg_vocal_only, &SPAN_RE);
                    let mut bg_str = String::new();
                    if !bg_syls.is_empty() {
                        recompute_utf16_offsets(&mut bg_str, &mut bg_syls);
                        let trimmed = bg_str.trim();
                        if !trimmed.is_empty() {
                            bg_text_opt = Some(trimmed.to_string());
                            bg_syllables = bg_syls;
                        }
                    } else {
                        let raw_bg = strip_all_xml_tags(&bg_vocal_only).trim().to_string();
                        if !raw_bg.is_empty() {
                            bg_text_opt = Some(raw_bg);
                        }
                    }
                }
                cursor = scan;
            } else {
                cleaned_body.push_str(head);
                cursor = open_idx + rel_gt + 1;
            }
        }
        if cursor < p_body.len() {
            cleaned_body.push_str(&p_body[cursor..]);
        }
        p_body = cleaned_body;

        let mut syllables = parse_ttml_inline_spans(&p_body, &SPAN_RE);
        let has_word = !syllables.is_empty();
        if has_word {
            // Normalize `<p>`-relative span timestamps if authored relative to `p_begin`
            if let Some(pb) = p_begin.filter(|&v| v > 1.0) {
                let first_s = syllables[0].start_sec;
                let last_e = syllables.last().map(|s| s.end_sec).unwrap_or(first_s);
                let p_span = p_end.map(|pe| (pe - pb).max(0.5)).unwrap_or(15.0);
                if first_s + 0.5 < pb && last_e <= p_span + 0.5 {
                    for s in &mut syllables {
                        s.start_sec += pb;
                        s.end_sec += pb;
                    }
                    for bs in &mut bg_syllables {
                        if bs.start_sec + 0.5 < pb && bs.end_sec <= p_span + 0.5 {
                            bs.start_sec += pb;
                            bs.end_sec += pb;
                        }
                    }
                }
            }
            any_word_sync = true;
        }

        let mut main_text = String::new();
        let (start_sec, end_sec) = if has_word {
            recompute_utf16_offsets(&mut main_text, &mut syllables);
            let s0 = p_begin.unwrap_or_else(|| syllables.first().map(|s| s.start_sec).unwrap_or(0.0));
            let s1 = p_end.unwrap_or_else(|| syllables.last().map(|s| s.end_sec).unwrap_or(s0 + 3.0));
            (s0, s1.max(s0 + 0.1))
        } else {
            main_text = strip_all_xml_tags(&p_body).trim().to_string();
            let s0 = match p_begin {
                Some(v) => v,
                None => continue,
            };
            let s1 = p_end.unwrap_or(s0 + 4.0).max(s0 + 0.25);
            (s0, s1)
        };

        if main_text.trim().is_empty() {
            continue;
        }

        let translation = inline_translation.or_else(|| {
            itunes_key
                .as_ref()
                .and_then(|k| header_translations.get(k).cloned())
        });

        let is_rtl = contains_rtl_chars(&main_text);
        let (word_cjk_units, char_units) = if has_word {
            (
                build_word_cjk_units_from_syllables(&syllables),
                build_char_units_from_syllables(&syllables),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let bg_word_cjk_units = build_word_cjk_units_from_syllables(&bg_syllables);
        let bg_char_units = build_char_units_from_syllables(&bg_syllables);
        lines.push(LyricLine {
            start_sec,
            end_sec,
            text: main_text,
            syllables,
            word_cjk_units,
            char_units,
            has_word_sync: has_word,
            bg_text: bg_text_opt,
            bg_syllables,
            bg_word_cjk_units,
            bg_char_units,
            translation,
            is_duet_secondary,
            is_rtl,
        });
    }

    if lines.is_empty() {
        return None;
    }

    lines.sort_by(|a, b| {
        a.start_sec
            .partial_cmp(&b.start_sec)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Cap line-only TTML `end_sec` cleanly when no explicit `end` was authored
    for i in 0..lines.len().saturating_sub(1) {
        if !lines[i].has_word_sync {
            let ns = lines[i + 1].start_sec;
            if lines[i].end_sec > ns {
                lines[i].end_sec = ns.max(lines[i].start_sec + 0.25);
            }
        }
    }

    Some(ParsedLyrics {
        source_name: source_label.to_string(),
        is_syllable_synced: any_word_sync,
        lines,
        songwriters,
    })
}

fn parse_ttml_inline_spans(fragment: &str, span_re: &Regex) -> Vec<LyricSyllable> {
    let mut out: Vec<LyricSyllable> = Vec::new();
    let mut last_end_idx = 0usize;
    let mut pending_prefix = String::new();

    for cap in span_re.captures_iter(fragment) {
        let m = cap.get(0).unwrap();
        // Preserve inter-span spaces & punctuation (`<span>Hello</span>! <span>World</span>`)!
        let between_raw = strip_all_xml_tags(&fragment[last_end_idx..m.start()]);
        if !between_raw.is_empty() {
            // Collapse XML indentation newlines into single spaces while keeping spaces/punctuation intact
            let mut norm = String::with_capacity(between_raw.len());
            let mut prev_ws = false;
            for ch in between_raw.chars() {
                if ch == '\r' || ch == '\n' || ch == '\t' {
                    if !prev_ws {
                        norm.push(' ');
                        prev_ws = true;
                    }
                } else {
                    prev_ws = ch.is_whitespace();
                    norm.push(ch);
                }
            }
            if let Some(prev) = out.last_mut() {
                prev.text.push_str(&norm);
            } else {
                let lead = norm.trim_start();
                if !lead.is_empty() {
                    pending_prefix.push_str(lead);
                }
            }
        }
        last_end_idx = m.end();

        let attrs = &cap[1];
        let role = extract_xml_attr(attrs, "ttm:role").unwrap_or("");
        if role == "x-translation" || role == "x-bg" || role == "x-roman" {
            continue;
        }
        let lower_attrs = attrs.to_ascii_lowercase();
        let explicit_emph = lower_attrs.contains("emphasis")
            || extract_xml_attr(attrs, "tts:fontStyle")
                .is_some_and(|v| v.eq_ignore_ascii_case("italic"))
            || extract_xml_attr(attrs, "tts:fontWeight")
                .is_some_and(|v| v.eq_ignore_ascii_case("bold"));
        let Some(b) = extract_xml_attr(attrs, "begin").and_then(parse_timestamp_str) else {
            continue;
        };
        let e = extract_xml_attr(attrs, "end")
            .and_then(parse_timestamp_str)
            .or_else(|| {
                extract_xml_attr(attrs, "dur")
                    .and_then(parse_timestamp_str)
                    .map(|d| b + d)
            })
            .unwrap_or(b + 0.22);
        let mut txt = strip_all_xml_tags(&cap[2]);
        if !pending_prefix.is_empty() {
            pending_prefix.push_str(&txt);
            txt = std::mem::take(&mut pending_prefix);
        }
        if txt.is_empty() {
            continue;
        }
        out.push(LyricSyllable {
            text: txt,
            start_sec: b,
            end_sec: e.max(b + 0.03),
            utf16_start: 0,
            utf16_len: 0,
            is_sustained: explicit_emph,
            is_explicit_emphasis: explicit_emph,
        });
    }

    if last_end_idx < fragment.len() {
        let tail = strip_all_xml_tags(&fragment[last_end_idx..]).trim_end().to_string();
        if !tail.is_empty() {
            if let Some(last) = out.last_mut() {
                last.text.push_str(&tail);
            }
        }
    }
    out
}

/// Parses NetEase Cloud Music word-by-word YRC (`[lineStartMs,lineDurMs](wStartMs,wDurMs,0)字...`) + optional `tlyric`
pub fn parse_yrc_lyrics(yrc: &str, tlyric: Option<&str>) -> Option<ParsedLyrics> {
    static LINE_HDR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\[(\d+),(\d+)\](.*)$").expect("valid yrc line regex"));
    static WORD_TAG_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\((\d+),(\d+),[^\)]*\)").expect("valid yrc word tag regex"));

    let yrc = yrc.trim_start_matches('\u{feff}');
    // Intentionally do NOT apply community `[offset:...]` tags (`0x22b047` / `0x22f379`)
    let trans_map = tlyric.map(parse_lrc_timestamp_map).unwrap_or_default();
    let mut trans_used = vec![false; trans_map.len()];
    let mut lines = Vec::new();

    for raw_line in yrc.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('{') {
            continue;
        }
        let Some(cap) = LINE_HDR_RE.captures(trimmed) else {
            continue;
        };
        let line_start_ms: f64 = cap[1].parse().unwrap_or(0.0);
        let line_dur_ms: f64 = cap[2].parse().unwrap_or(2500.0);
        let rest = &cap[3];

        let tag_caps: Vec<_> = WORD_TAG_RE.captures_iter(rest).collect();
        let mut syllables = Vec::with_capacity(tag_caps.len());
        for idx in 0..tag_caps.len() {
            let full_m = tag_caps[idx].get(0).unwrap();
            let ws_ms: f64 = tag_caps[idx][1].parse().unwrap_or(line_start_ms);
            let wd_ms: f64 = tag_caps[idx][2].parse().unwrap_or(200.0);
            let txt_start = full_m.end();
            let txt_end = tag_caps
                .get(idx + 1)
                .map(|nc| nc.get(0).unwrap().start())
                .unwrap_or(rest.len());
            let w_txt = rest[txt_start..txt_end].to_string();
            if w_txt.is_empty() {
                continue;
            }
            let s0 = (ws_ms / 1000.0).max(0.0);
            let s1 = ((ws_ms + wd_ms.max(30.0)) / 1000.0).max(s0 + 0.03);
            syllables.push(LyricSyllable {
                text: w_txt,
                start_sec: s0,
                end_sec: s1,
                utf16_start: 0,
                utf16_len: 0,
                is_sustained: false,
                is_explicit_emphasis: false,
            });
        }

        if syllables.is_empty() {
            continue;
        }
        let mut main_text = String::new();
        recompute_utf16_offsets(&mut main_text, &mut syllables);
        if main_text.trim().is_empty() {
            continue;
        }
        let start_sec = (line_start_ms / 1000.0).max(0.0);
        let end_sec = ((line_start_ms + line_dur_ms.max(150.0)) / 1000.0).max(start_sec + 0.15);
        let translation =
            find_closest_translation_monotonic(&trans_map, &mut trans_used, start_sec, 1.35);
        let is_rtl = contains_rtl_chars(&main_text);
        let word_cjk_units = build_word_cjk_units_from_syllables(&syllables);
        let char_units = build_char_units_from_syllables(&syllables);

        lines.push(LyricLine {
            start_sec,
            end_sec,
            text: main_text,
            syllables,
            word_cjk_units,
            char_units,
            has_word_sync: true,
            bg_text: None,
            bg_syllables: Vec::new(),
            bg_word_cjk_units: Vec::new(),
            bg_char_units: Vec::new(),
            translation,
            is_duet_secondary: false,
            is_rtl,
        });
    }

    if lines.is_empty() {
        return None;
    }

    Some(ParsedLyrics {
        source_name: "逐字同步歌词".to_string(),
        is_syllable_synced: true,
        lines,
        songwriters: Vec::new(),
    })
}

fn parse_lrc_timestamp_map(lrc: &str) -> Vec<(f64, String)> {
    static TAG_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\[(\d{1,2}:\d{1,2}(?:[\.:]\d{1,3})?)\]").expect("lrc tag re"));

    let lrc = lrc.trim_start_matches('\u{feff}');
    // Intentionally do NOT apply community `[offset:...]` tags (`0x22b047` — prevents bogus uploader offsets!)
    let mut out = Vec::new();
    for raw in lrc.lines() {
        let mut times = Vec::new();
        let mut last_end = 0usize;
        for cap in TAG_RE.captures_iter(raw) {
            let m = cap.get(0).unwrap();
            last_end = m.end();
            let fixed = if cap[1].matches(':').count() == 2 {
                let p: Vec<&str> = cap[1].split(':').collect();
                format!("{}:{}.{}", p[0], p[1], p[2])
            } else {
                cap[1].to_string()
            };
            if let Some(t) = parse_timestamp_str(&fixed) {
                times.push(t.max(0.0));
            }
        }
        let body = raw[last_end..].trim();
        if body.is_empty() || body == "//" {
            continue;
        }
        for t in times {
            out.push((t, body.to_string()));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn find_closest_translation_monotonic(
    trans: &[(f64, String)],
    used: &mut [bool],
    target_sec: f64,
    max_diff_sec: f64,
) -> Option<String> {
    let mut best: Option<(f64, usize)> = None;
    for (idx, (t, _)) in trans.iter().enumerate() {
        if used.get(idx).copied().unwrap_or(false) {
            continue;
        }
        let diff = (t - target_sec).abs();
        if diff <= max_diff_sec && best.is_none_or(|(bd, _)| diff < bd) {
            best = Some((diff, idx));
        }
    }
    if let Some((_, idx)) = best {
        if let Some(u) = used.get_mut(idx) {
            *u = true;
        }
        return Some(trans[idx].1.clone());
    }
    None
}

/// Parses standard or enhanced LRC (`[mm:ss.xx]` + optional `<mm:ss.xx>` word tags) + optional `tlyric`.
/// Plain line-synced LRC sets `has_word_sync = false` (zero fake slow-motion karaoke!) and caps
/// `end_sec = next_start.min(start_sec + 4.0)` (`0x229cb4`) so >= 7.0s interludes trigger cleanly.
pub fn parse_lrc_lyrics(lrc: &str, tlyric: Option<&str>, source_label: &str) -> Option<ParsedLyrics> {
    let raw_pairs = parse_lrc_timestamp_map(lrc);
    if raw_pairs.is_empty() {
        return None;
    }
    let ext_trans = tlyric.map(parse_lrc_timestamp_map).unwrap_or_default();
    let mut ext_used = vec![false; ext_trans.len()];

    // Merge consecutive lines with identical timestamps as `(primary, translation)`
    let mut merged: Vec<(f64, String, Option<String>)> = Vec::with_capacity(raw_pairs.len());
    for (t, txt) in raw_pairs {
        if let Some(last) = merged.last_mut() {
            if (last.0 - t).abs() < 0.02 && last.2.is_none() {
                last.2 = Some(txt);
                continue;
            }
        }
        let tr = find_closest_translation_monotonic(&ext_trans, &mut ext_used, t, 1.35);
        merged.push((t, txt, tr));
    }

    static INLINE_TAG_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"<(\d{1,2}:\d{1,2}(?:[\.:]\d{1,3})?)>").expect("inline word lrc tag re")
    });

    let mut any_word = false;
    let mut lines = Vec::with_capacity(merged.len());
    for i in 0..merged.len() {
        let (start_sec, ref raw_txt, ref tr) = merged[i];
        let next_start = if i + 1 < merged.len() {
            merged[i + 1].0
        } else {
            start_sec + 4.0
        };
        let mut end_sec = next_start.min(start_sec + 4.0).max(start_sec + 0.25);

        let mut syllables = Vec::new();
        let mut main_text = String::new();
        let mut has_word = false;

        let tag_caps: Vec<_> = INLINE_TAG_RE.captures_iter(raw_txt).collect();
        if !tag_caps.is_empty() {
            let parse_inline_t = |raw_tag: &str| -> Option<f64> {
                let fixed = if raw_tag.matches(':').count() == 2 {
                    let p: Vec<&str> = raw_tag.split(':').collect();
                    format!("{}:{}.{}", p[0], p[1], p[2])
                } else {
                    raw_tag.to_string()
                };
                parse_timestamp_str(&fixed)
            };

            let first_m = tag_caps[0].get(0).unwrap();
            if first_m.start() > 0 {
                let lead_txt = &raw_txt[..first_m.start()];
                if !lead_txt.is_empty() {
                    let first_t = parse_inline_t(&tag_caps[0][1]).unwrap_or(start_sec + 0.25);
                    syllables.push(LyricSyllable {
                        text: lead_txt.to_string(),
                        start_sec,
                        end_sec: first_t.max(start_sec + 0.04),
                        utf16_start: 0,
                        utf16_len: 0,
                        is_sustained: false,
                        is_explicit_emphasis: false,
                    });
                }
            }

            for w_idx in 0..tag_caps.len() {
                let m_cur = tag_caps[w_idx].get(0).unwrap();
                let Some(ws) = parse_inline_t(&tag_caps[w_idx][1]) else {
                    continue;
                };
                let txt_end = tag_caps
                    .get(w_idx + 1)
                    .map(|nc| nc.get(0).unwrap().start())
                    .unwrap_or(raw_txt.len());
                let seg_txt = &raw_txt[m_cur.end()..txt_end];
                if seg_txt.is_empty() {
                    // Trailing `<mm:ss.xx>` end-marker bounds the line's final word cleanly!
                    end_sec = ws.max(start_sec + 0.20);
                    continue;
                }
                let we = if w_idx + 1 < tag_caps.len() {
                    parse_inline_t(&tag_caps[w_idx + 1][1]).unwrap_or(ws + 0.3)
                } else {
                    next_start.min(ws + 3.5).max(ws + 0.25)
                };
                syllables.push(LyricSyllable {
                    text: seg_txt.to_string(),
                    start_sec: ws,
                    end_sec: we.max(ws + 0.04),
                    utf16_start: 0,
                    utf16_len: 0,
                    is_sustained: false,
                    is_explicit_emphasis: false,
                });
            }

            if syllables.len() >= 2 {
                has_word = true;
                any_word = true;
                if let Some(last_s) = syllables.last() {
                    end_sec = end_sec.max(last_s.end_sec).min(next_start.max(start_sec + 0.25));
                }
                recompute_utf16_offsets(&mut main_text, &mut syllables);
            } else {
                syllables.clear();
            }
        }

        if !has_word {
            main_text = raw_txt.trim().to_string();
        }

        if main_text.trim().is_empty() {
            continue;
        }
        let is_rtl = contains_rtl_chars(&main_text);
        let (word_cjk_units, char_units) = if has_word {
            (
                build_word_cjk_units_from_syllables(&syllables),
                build_char_units_from_syllables(&syllables),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        lines.push(LyricLine {
            start_sec,
            end_sec,
            text: main_text,
            syllables,
            word_cjk_units,
            char_units,
            has_word_sync: has_word,
            bg_text: None,
            bg_syllables: Vec::new(),
            bg_word_cjk_units: Vec::new(),
            bg_char_units: Vec::new(),
            translation: tr.clone(),
            is_duet_secondary: false,
            is_rtl,
        });
    }

    if lines.is_empty() {
        return None;
    }

    Some(ParsedLyrics {
        source_name: source_label.to_string(),
        is_syllable_synced: any_word,
        lines,
        songwriters: Vec::new(),
    })
}

/// Auto-detects `.ttml`, `.yrc`, or `.lrc` from a local file string
pub fn parse_any_lyrics_file(content: &str) -> Option<ParsedLyrics> {
    let trimmed = content.trim_start_matches('\u{feff}').trim();
    if trimmed.contains("<tt") && trimmed.contains("<p") {
        return parse_ttml_lyrics(trimmed, "本地 TTML 逐词歌词");
    }
    if trimmed.lines().any(|l| l.starts_with('[') && l.contains("](")) {
        return parse_yrc_lyrics(trimmed, None).map(|mut p| {
            p.source_name = "本地 YRC 逐字歌词".to_string();
            p
        });
    }
    parse_lrc_lyrics(trimmed, None, "本地 LRC 歌词")
}

// ============================================================================
// Multi-Source Async Fetcher (Apple Music TTML -> AMLL TTML -> NetEase YRC -> LRCLIB)
// ============================================================================

static LYRICS_CACHE: Lazy<Mutex<HashMap<String, Option<ParsedLyrics>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static BUILTIN_AGENT_LYRICS_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_builtin_agent_lyrics_enabled(enabled: bool) {
    BUILTIN_AGENT_LYRICS_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

static LYRICS_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(9))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
        .expect("lyrics http client")
});

pub fn make_lyrics_cache_key(adam_id: Option<&str>, title: &str, artist: &str) -> String {
    if let Some(aid) = adam_id.filter(|s| !s.trim().is_empty()) {
        format!("id:{}", aid.trim())
    } else {
        format!(
            "meta:{}|{}",
            title.trim().to_lowercase(),
            artist.trim().to_lowercase()
        )
    }
}

pub fn put_custom_lyrics_in_cache(key: &str, parsed: ParsedLyrics) {
    if let Ok(mut g) = LYRICS_CACHE.lock() {
        g.insert(key.to_string(), Some(parsed));
    }
}

pub fn get_cached_lyrics(key: &str) -> Option<Option<ParsedLyrics>> {
    LYRICS_CACHE.lock().ok().and_then(|g| g.get(key).cloned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LyricsSourcePref {
    Auto,
    AppleMusic,
    AmllDb,
    NetEase,
    Lrclib,
}

impl LyricsSourcePref {
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::AppleMusic,
            Self::AppleMusic => Self::AmllDb,
            Self::AmllDb => Self::NetEase,
            Self::NetEase => Self::Lrclib,
            Self::Lrclib => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动词源",
            Self::AppleMusic => "Apple Music",
            Self::AmllDb => "AMLL 词库",
            Self::NetEase => "网易云逐字",
            Self::Lrclib => "LRCLIB",
        }
    }
}

pub async fn fetch_best_lyrics(
    adam_id: Option<String>,
    title: String,
    artist: String,
    album: String,
    duration_sec: f64,
    storefront: String,
    user_token: Option<String>,
) -> Option<ParsedLyrics> {
    fetch_lyrics_with_pref(
        LyricsSourcePref::Auto,
        adam_id,
        title,
        artist,
        album,
        duration_sec,
        storefront,
        user_token,
        false,
    )
    .await
}

pub async fn fetch_lyrics_with_pref(
    pref: LyricsSourcePref,
    adam_id: Option<String>,
    title: String,
    artist: String,
    album: String,
    duration_sec: f64,
    storefront: String,
    user_token: Option<String>,
    bypass_cache: bool,
) -> Option<ParsedLyrics> {
    let connect_agent = BUILTIN_AGENT_LYRICS_ENABLED.load(std::sync::atomic::Ordering::Relaxed);
    fetch_lyrics_with_pref_ex(
        pref,
        adam_id,
        title,
        artist,
        album,
        duration_sec,
        storefront,
        user_token,
        bypass_cache,
        connect_agent,
    )
    .await
}

fn extract_ttml_strings_from_amp_json(json: &Value) -> (Option<String>, Option<String>) {
    let base_ttml = json
        .pointer("/data/0/attributes/ttml")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    let loc_node = json.pointer("/data/0/attributes/ttmlLocalizations");
    let loc_ttml = match loc_node {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(Value::Object(map)) => {
            let preferred_keys = ["zh-Hans-CN", "zh-Hans", "zh-CN", "ja", "en-US"];
            let mut found = None;
            for pk in preferred_keys {
                if let Some(v) = map.get(pk) {
                    if let Some(s) = v
                        .as_str()
                        .or_else(|| v.get("ttml").and_then(Value::as_str))
                        .filter(|s| !s.trim().is_empty())
                    {
                        found = Some(s.to_string());
                        break;
                    }
                }
            }
            if found.is_none() {
                for v in map.values() {
                    if let Some(s) = v
                        .as_str()
                        .or_else(|| v.get("ttml").and_then(Value::as_str))
                        .filter(|s| !s.trim().is_empty())
                    {
                        found = Some(s.to_string());
                        break;
                    }
                }
            }
            found
        }
        _ => None,
    };

    (loc_ttml, base_ttml)
}

fn parse_best_amp_ttml_pair(
    loc_ttml: Option<&str>,
    base_ttml: Option<&str>,
    label: &str,
) -> Option<ParsedLyrics> {
    let p_loc = loc_ttml.and_then(|t| parse_ttml_lyrics(t, label));
    let p_base = base_ttml.and_then(|t| parse_ttml_lyrics(t, label));

    match (p_loc, p_base) {
        (Some(mut loc), Some(mut base)) => {
            if loc.is_syllable_synced {
                return Some(loc);
            }
            if base.is_syllable_synced {
                // Backfill localized translations from `loc` onto syllable-synced `base` within 1.0s
                let mut used = vec![false; loc.lines.len()];
                for bl in &mut base.lines {
                    if bl.translation.is_some() {
                        continue;
                    }
                    let mut best_tr: Option<(f64, usize)> = None;
                    for (idx, ll) in loc.lines.iter().enumerate() {
                        if used[idx] {
                            continue;
                        }
                        let diff = (ll.start_sec - bl.start_sec).abs();
                        if diff <= 1.0 && best_tr.is_none_or(|(bd, _)| diff < bd) {
                            best_tr = Some((diff, idx));
                        }
                    }
                    if let Some((_, idx)) = best_tr {
                        used[idx] = true;
                        bl.translation = loc.lines[idx]
                            .translation
                            .clone()
                            .or_else(|| {
                                let lt = loc.lines[idx].text.trim();
                                (!lt.is_empty() && lt != bl.text.trim()).then(|| lt.to_string())
                            });
                    }
                }
                return Some(base);
            }
            if loc.lines.iter().any(|l| l.translation.is_some()) {
                Some(std::mem::take(&mut loc))
            } else {
                Some(base)
            }
        }
        (Some(loc), None) => Some(loc),
        (None, Some(base)) => Some(base),
        (None, None) => None,
    }
}

async fn resolve_numeric_catalog_adam_id(
    title: &str,
    artist: &str,
    album: &str,
    duration_sec: f64,
    sf: &str,
    user_token: Option<String>,
) -> Option<String> {
    if title.trim().is_empty() {
        return None;
    }
    let q_raw = if artist.trim().is_empty() {
        title.trim().to_string()
    } else {
        format!("{} {}", title.trim(), artist.trim())
    };
    let enc: String = url::form_urlencoded::byte_serialize(q_raw.as_bytes()).collect();
    let search_path = format!("v1/catalog/{sf}/search?term={enc}&types=songs&limit=8");
    let json = crate::am_api::fetch_am_api(search_path, user_token, None, Some(sf.to_string()))
        .await
        .ok()?;
    let arr = json.pointer("/results/songs/data")?.as_array()?;

    let mut best: Option<(f64, String)> = None;
    for item in arr {
        let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        else {
            continue;
        };
        let attrs = item.get("attributes").unwrap_or(item);
        let c_title = attrs.get("name").and_then(Value::as_str).unwrap_or("");
        let c_artist = attrs.get("artistName").and_then(Value::as_str).unwrap_or("");
        let c_album = attrs.get("albumName").and_then(Value::as_str).unwrap_or("");
        let c_dur = attrs
            .get("durationInMillis")
            .and_then(Value::as_f64)
            .map(|ms| ms / 1000.0)
            .unwrap_or(0.0);

        if has_mismatched_edition_tag(title, album, &format!("{c_title} {c_album}")) {
            continue;
        }
        let Some(mut sc) =
            strict_catalog_match_score(title, artist, duration_sec, c_title, c_artist, c_dur)
        else {
            continue;
        };
        if !album.trim().is_empty() && !c_album.trim().is_empty() {
            let a_low = album.trim().to_lowercase();
            let ca_low = c_album.trim().to_lowercase();
            if a_low == ca_low || ca_low.contains(&a_low) {
                sc += 12.0;
            }
        }
        if best.as_ref().is_none_or(|(bs, _)| sc > *bs) {
            best = Some((sc, id.to_string()));
        }
    }
    best.map(|(_, id)| id)
}

pub async fn fetch_lyrics_with_pref_ex(
    pref: LyricsSourcePref,
    adam_id: Option<String>,
    title: String,
    artist: String,
    album: String,
    duration_sec: f64,
    storefront: String,
    user_token: Option<String>,
    bypass_cache: bool,
    connect_agent_if_needed: bool,
) -> Option<ParsedLyrics> {
    let base_key = make_lyrics_cache_key(adam_id.as_deref(), &title, &artist);
    let key = if pref == LyricsSourcePref::Auto {
        base_key
    } else {
        format!("{base_key}#src:{:?}", pref)
    };
    let can_cache = adam_id.as_deref().is_some_and(|s| !s.trim().is_empty()) || duration_sec > 5.0;
    if !bypass_cache && can_cache {
        if let Some(Some(cached_parsed)) = get_cached_lyrics(&key) {
            return Some(cached_parsed);
        }
    } else if !bypass_cache && pref == LyricsSourcePref::Auto {
        // Still honor user-imported local lyrics on `base_key` even when `duration_sec <= 5.0`
        if let Some(Some(cached_parsed)) = get_cached_lyrics(&key) {
            if cached_parsed.source_name.contains("本地") {
                return Some(cached_parsed);
            }
        }
    }

    let sf = if storefront.trim().is_empty() {
        "cn".to_string()
    } else {
        storefront.trim().to_ascii_lowercase()
    };

    let try_am = matches!(pref, LyricsSourcePref::Auto | LyricsSourcePref::AppleMusic);
    let try_amll = matches!(pref, LyricsSourcePref::Auto | LyricsSourcePref::AmllDb);
    let try_ne = matches!(pref, LyricsSourcePref::Auto | LyricsSourcePref::NetEase);
    let try_lrc = matches!(pref, LyricsSourcePref::Auto | LyricsSourcePref::Lrclib);

    let clean_utok_opt = user_token
        .as_deref()
        .map(crate::am_api::normalize_user_token)
        .filter(|s| !s.is_empty());

    // Format-Tier-First Acquisition: NEVER let line-only Command 5023 (`displayType=2`) block
    // official Apple Music `/syllable-lyrics`, `AMLL`, or `NetEase YRC` word-by-word lyrics!
    let mut line_fallback: Option<ParsedLyrics> = None;

    // Resolve numeric catalog `adam_id` up front when missing (SMTC) or non-numeric (`i.xxxx` library ID)
    let numeric_aid: Option<String> = match adam_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
    {
        Some(digits) => Some(digits.to_string()),
        None if try_am || try_amll => {
            resolve_numeric_catalog_adam_id(
                &title,
                &artist,
                &album,
                duration_sec,
                &sf,
                clean_utok_opt.clone(),
            )
            .await
        }
        None => None,
    };

    // Tier 1A: Official Apple Music REST `/syllable-lyrics` FIRST (word-by-word `displayType=3`)
    if try_am {
        if let (Some(ref aid), Some(ref clean_utok)) = (&numeric_aid, &clean_utok_opt) {
            // Storefront-safe paths (`jp` / `us` / `cn`): try `l[lyrics]=zh-Hans-CN` localization first,
            // then bare `/syllable-lyrics` (no invalid `l=` UI tag!).
            let syl_paths = [
                format!(
                    "v1/catalog/{sf}/songs/{aid}/syllable-lyrics?l%5Blyrics%5D=zh-Hans-CN&l%5Bscript%5D=en-Latn&extend=ttmlLocalizations"
                ),
                format!("v1/catalog/{sf}/songs/{aid}/syllable-lyrics"),
            ];
            for path in syl_paths {
                if let Ok(json) = crate::am_api::fetch_am_api(
                    path,
                    Some(clean_utok.clone()),
                    None,
                    Some(sf.clone()),
                )
                .await
                {
                    let (loc_ttml, base_ttml) = extract_ttml_strings_from_amp_json(&json);
                    if let Some(mut parsed) = parse_best_amp_ttml_pair(
                        loc_ttml.as_deref(),
                        base_ttml.as_deref(),
                        "Apple Music 逐词歌词",
                    ) {
                        if parsed.is_syllable_synced {
                            if parsed.lines.iter().all(|l| l.translation.is_none())
                                && !title.trim().is_empty()
                            {
                                backfill_netease_translations(
                                    &mut parsed,
                                    &title,
                                    &artist,
                                    &album,
                                    duration_sec,
                                )
                                .await;
                            }
                            if can_cache {
                                put_cache_bounded(&key, Some(parsed.clone()));
                            }
                            return Some(parsed);
                        }
                        parsed.source_name = "Apple Music 同步歌词".to_string();
                        if line_fallback.is_none() {
                            line_fallback = Some(parsed);
                        }
                    }
                }
            }
        }
    }

    // Tier 1B: `AMPLibraryAgent` Command 5023 (`GetLyrics` -> `bag://musicSubscription/ttmlLyrics`)
    // Only return immediately if it actually contains syllable/word spans OR user explicitly locked AppleMusic!
    if try_am {
        let mut agent_candidates: Vec<String> = Vec::new();
        if let Some(aid_raw) = adam_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            agent_candidates.push(aid_raw.to_string());
        }
        if let Some(ref naid) = numeric_aid {
            if !agent_candidates.iter().any(|x| x == naid) {
                agent_candidates.push(naid.clone());
            }
        }
        for aid_agent in agent_candidates {
            if let Ok(Ok(Some((raw_body, is_ttml)))) = tokio::task::spawn_blocking(move || {
                crate::am_agent::get_store_lyrics_via_agent(&aid_agent, connect_agent_if_needed)
            })
            .await
            {
                let parsed_opt = if is_ttml || raw_body.contains("<tt") {
                    parse_ttml_lyrics(&raw_body, "Apple Music 逐词歌词")
                } else {
                    parse_any_lyrics_file(&raw_body).map(|mut p| {
                        p.source_name = "Apple Music 同步歌词".to_string();
                        p
                    })
                };
                if let Some(mut parsed) = parsed_opt {
                    if !parsed.is_syllable_synced {
                        parsed.source_name = "Apple Music 同步歌词".to_string();
                    }
                    if parsed.is_syllable_synced || pref == LyricsSourcePref::AppleMusic {
                        if parsed.lines.iter().all(|l| l.translation.is_none())
                            && !title.trim().is_empty()
                        {
                            backfill_netease_translations(
                                &mut parsed,
                                &title,
                                &artist,
                                &album,
                                duration_sec,
                            )
                            .await;
                        }
                        if can_cache {
                            put_cache_bounded(&key, Some(parsed.clone()));
                        }
                        return Some(parsed);
                    }
                    if line_fallback.is_none() {
                        line_fallback = Some(parsed);
                    }
                }
            }
        }
    }

    // Tier 1C: AMLL Syllable TTML DB via fast CDN (`1.2s` crisp timeout, zero GFW hang!)
    if try_amll {
        if let Some(ref aid) = numeric_aid {
            let url = format!(
                "https://fastly.jsdelivr.net/gh/Steve-xmh/amll-ttml-db@main/am-lyrics/{aid}.ttml"
            );
            if let Ok(resp) = LYRICS_HTTP
                .get(&url)
                .timeout(Duration::from_millis(1200))
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(ttml) = resp.text().await {
                        if let Some(mut parsed) = parse_ttml_lyrics(&ttml, "AMLL 逐词歌词") {
                            if !parsed.is_syllable_synced {
                                parsed.source_name = "AMLL 同步歌词".to_string();
                            }
                            if parsed.is_syllable_synced || pref == LyricsSourcePref::AmllDb {
                                if parsed.lines.iter().all(|l| l.translation.is_none())
                                    && !title.trim().is_empty()
                                {
                                    backfill_netease_translations(
                                        &mut parsed,
                                        &title,
                                        &artist,
                                        &album,
                                        duration_sec,
                                    )
                                    .await;
                                }
                                if can_cache {
                                    put_cache_bounded(&key, Some(parsed.clone()));
                                }
                                return Some(parsed);
                            }
                            if line_fallback.is_none() {
                                line_fallback = Some(parsed);
                            }
                        }
                    }
                }
            }
        }
    }

    // Tier 1D: NetEase Cloud Music Word-by-Word YRC + Chinese Translation (strict duration & edition guard!)
    if try_ne && !title.trim().is_empty() {
        if let Some(parsed) = fetch_netease_yrc_or_lrc(&title, &artist, &album, duration_sec).await {
            if parsed.is_syllable_synced || pref == LyricsSourcePref::NetEase {
                if can_cache {
                    put_cache_bounded(&key, Some(parsed.clone()));
                }
                return Some(parsed);
            }
            if line_fallback.is_none() {
                line_fallback = Some(parsed);
            }
        }
    }

    // Tier 2: Line-Synced Fallbacks (Official Apple `/lyrics` -> collected `line_fallback` -> LRCLIB)
    if line_fallback.is_none() && try_am {
        if let (Some(ref aid), Some(ref clean_utok)) = (&numeric_aid, &clean_utok_opt) {
            let path = format!("v1/catalog/{sf}/songs/{aid}/lyrics");
            if let Ok(json) = crate::am_api::fetch_am_api(
                path,
                Some(clean_utok.clone()),
                None,
                Some(sf.clone()),
            )
            .await
            {
                let (loc_ttml, base_ttml) = extract_ttml_strings_from_amp_json(&json);
                if let Some(mut parsed) = parse_best_amp_ttml_pair(
                    loc_ttml.as_deref(),
                    base_ttml.as_deref(),
                    "Apple Music 同步歌词",
                ) {
                    if !parsed.is_syllable_synced {
                        parsed.source_name = "Apple Music 同步歌词".to_string();
                    }
                    line_fallback = Some(parsed);
                }
            }
        }
    }

    if let Some(mut fb) = line_fallback {
        if fb.lines.iter().all(|l| l.translation.is_none())
            && fb.source_name.starts_with("Apple Music")
            && !title.trim().is_empty()
        {
            backfill_netease_translations(&mut fb, &title, &artist, &album, duration_sec).await;
        }
        if can_cache {
            put_cache_bounded(&key, Some(fb.clone()));
        }
        return Some(fb);
    }

    if try_lrc && !title.trim().is_empty() {
        if let Some(parsed) = fetch_lrclib(&title, &artist, &album, duration_sec).await {
            if can_cache {
                put_cache_bounded(&key, Some(parsed.clone()));
            }
            return Some(parsed);
        }
    }

    // Never store `None` permanently in `LYRICS_CACHE` so transient misses or 0.0s early fetches can retry!
    None
}

fn put_cache_bounded(key: &str, val: Option<ParsedLyrics>) {
    let Some(parsed) = val else {
        return;
    };
    if let Ok(mut g) = LYRICS_CACHE.lock() {
        if g.len() >= 48 {
            g.clear();
        }
        g.insert(key.to_string(), Some(parsed));
    }
}

/// 14-keyword LyricsBlossom edition set + CJK/J-Pop/Anime timing-altering edition modifiers
const BRACKET_EDITION_KEYWORDS: &[&str] = &[
    "remix",
    "remaster",
    "version",
    "mix",
    "karaoke",
    "extended",
    "radio edit",
    "acoustic",
    "live",
    "demo",
    "original",
    "片段",
    "伴奏",
    "翻唱",
    "现场",
    "演唱会",
    "音乐节",
    "concert",
    "instrumental",
    "off vocal",
    "less vocal",
    "backing track",
    "cover",
    "unplugged",
    "tv size",
    "tv-size",
    "tv ver",
    "short ver",
    "movie ver",
    "sped up",
    "slowed",
    "加快",
    "慢速",
    "dj版",
    "dj ",
    "nightcore",
    "串烧",
    "铃声",
    "纯享",
];

const TIMING_SHIFT_EDITION_TAGS: &[&str] = &[
    "live",
    "现场",
    "演唱会",
    "音乐节",
    "concert",
    "remix",
    "伴奏",
    "instrumental",
    "karaoke",
    "off vocal",
    "less vocal",
    "backing track",
    "cover",
    "翻唱",
    "acoustic",
    "unplugged",
    "demo",
    "radio edit",
    "extended",
    "tv size",
    "tv-size",
    "tv ver",
    "short ver",
    "movie ver",
    "sped up",
    "slowed",
    "加快",
    "慢速",
    "dj版",
    "dj ",
    "nightcore",
    "1.2x",
    "0.8x",
    "片段",
    "串烧",
    "铃声",
    "纯享",
];

/// Word-boundary-aware edition keyword matcher so ASCII tags (`"demo"`, `"cover"`, `"live"`, `"mix"`)
/// never false-positive inside normal words (`"Demon Slayer"`, `"Discovery"`, `"Deliver"`, `"Alive"`),
/// while still matching `"(Live)"`, `"Live版"`, `"2021 Remastered"`, `"TV Version"`, and CJK tags (`"现场"`).
fn contains_edition_keyword(haystack_lower: &str, kw: &str) -> bool {
    if kw.is_empty() {
        return false;
    }
    let bytes = haystack_lower.as_bytes();
    let check_left = kw.as_bytes().first().is_some_and(|b| b.is_ascii_alphanumeric());
    let check_right = kw.as_bytes().last().is_some_and(|b| b.is_ascii_alphanumeric());

    for (start, _) in haystack_lower.match_indices(kw) {
        let end = start + kw.len();
        let left_ok = !check_left || start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        if !left_ok {
            continue;
        }
        let right_ok = if !check_right || end == bytes.len() || !bytes[end].is_ascii_alphanumeric() {
            true
        } else if kw == "remaster" && haystack_lower[end..].starts_with("ed") {
            end + 2 == bytes.len() || !bytes[end + 2].is_ascii_alphanumeric()
        } else if kw.ends_with(" ver") && haystack_lower[end..].starts_with("sion") {
            end + 4 == bytes.len() || !bytes[end + 4].is_ascii_alphanumeric()
        } else {
            false
        };
        if right_ok {
            return true;
        }
    }
    false
}

/// Returns `true` if `candidate_meta` carries a live/remix/cover/instrumental/TV-size/sped-up edition tag
/// that is NOT present on the playing track's own title or album (which would cause intro/tempo desync!).
pub fn has_mismatched_edition_tag(track_title: &str, track_album: &str, candidate_meta: &str) -> bool {
    let ref_low = format!("{} {}", track_title.to_lowercase(), track_album.to_lowercase());
    let cand_low = candidate_meta.to_lowercase();
    TIMING_SHIFT_EDITION_TAGS
        .iter()
        .any(|tag| contains_edition_keyword(&cand_low, tag) && !contains_edition_keyword(&ref_low, tag))
}

/// Strips balanced `(...)`, `[...]`, `【...】`, `（...）`, `［...］` non-edition noise (e.g. `(feat. X)`, `[Explicit]`),
/// while preserving any bracketed segment containing one of the 14+ edition keywords (`0x240e50`).
fn strip_non_edition_brackets(raw: &str) -> String {
    let chars: Vec<char> = raw.trim().to_lowercase().chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    while i < chars.len() {
        let close_opt = match chars[i] {
            '(' => Some(')'),
            '[' => Some(']'),
            '【' => Some('】'),
            '（' => Some('）'),
            '［' => Some('］'),
            _ => None,
        };
        if let Some(close_ch) = close_opt {
            let open_ch = chars[i];
            let mut depth = 1i32;
            let mut j = i + 1;
            let mut matched = None;
            while j < chars.len() {
                if chars[j] == open_ch {
                    depth += 1;
                } else if chars[j] == close_ch {
                    depth -= 1;
                    if depth == 0 {
                        matched = Some(j);
                        break;
                    }
                }
                j += 1;
            }
            if let Some(end_j) = matched {
                let interior: String = chars[i + 1..end_j].iter().collect();
                let is_edition = BRACKET_EDITION_KEYWORDS
                    .iter()
                    .any(|kw| contains_edition_keyword(&interior, kw));
                if is_edition {
                    for &c in &chars[i..=end_j] {
                        out.push(c);
                    }
                }
                i = end_j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_title_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || is_cjk_char(*c))
        .collect()
}

fn split_artist_tokens(raw: &str) -> Vec<String> {
    let mut norm = raw.trim().to_lowercase();
    for kw in [" feat. ", " feat ", " ft. ", " ft ", " with ", " × ", " x "] {
        norm = norm.replace(kw, ",");
    }
    norm.split(|c| matches!(c, ',' | '&' | '/' | '、' | ';' | '|' | '+' | '·'))
        .map(|part| strip_non_edition_brackets(part))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Authoritative cross-provider candidate scorer enforcing:
/// 1. `|query_dur_sec - cand_dur_sec| <= 2.2s` hard gate whenever both durations > 5.0s;
/// 2. Bidirectional edition-modifier mismatch hard rejection (`0x240e50` / `0x1e2cb0`);
/// 3. Normalized title equality or clean non-edition substring match (`0x241870`);
/// 4. Hard artist token/substring overlap gate when `query_artist` is non-empty (`0x2483c4`).
pub fn strict_catalog_match_score(
    query_title: &str,
    query_artist: &str,
    query_dur_sec: f64,
    cand_title: &str,
    cand_artist: &str,
    cand_dur_sec: f64,
) -> Option<f64> {
    // 1. Strict duration hard gate (`<= 2.2s` blocks MV intros, TV-Size, Radio Edits & covers)
    if query_dur_sec > 5.0 && cand_dur_sec > 5.0 && (query_dur_sec - cand_dur_sec).abs() > 2.2 {
        return None;
    }

    // 2. Bidirectional timing-shift edition mismatch hard rejection
    let q_low = query_title.trim().to_lowercase();
    let c_low = cand_title.trim().to_lowercase();
    for tag in TIMING_SHIFT_EDITION_TAGS {
        if contains_edition_keyword(&c_low, tag) != contains_edition_keyword(&q_low, tag) {
            return None;
        }
    }

    // 3. Edition-preserving bracket strip & title match gate
    let q_norm = strip_non_edition_brackets(query_title);
    let c_norm = strip_non_edition_brackets(cand_title);
    let q_comp = compact_title_key(&q_norm);
    let c_comp = compact_title_key(&c_norm);
    if q_comp.is_empty() || c_comp.is_empty() {
        return None;
    }

    let mut score = if q_norm == c_norm || q_comp == c_comp {
        60.0
    } else if c_norm.contains(&q_norm) || q_norm.contains(&c_norm) {
        let residual = if c_norm.len() >= q_norm.len() {
            c_norm.replace(&q_norm, " ")
        } else {
            q_norm.replace(&c_norm, " ")
        };
        if BRACKET_EDITION_KEYWORDS
            .iter()
            .filter(|&&kw| kw != "remaster" && kw != "original")
            .any(|kw| contains_edition_keyword(&residual, kw))
        {
            return None;
        }
        let min_chars = q_comp.chars().count().min(c_comp.chars().count());
        let max_chars = q_comp.chars().count().max(c_comp.chars().count()).max(1);
        if min_chars < 2 && max_chars > 2 {
            return None;
        }
        32.0
    } else {
        return None;
    };

    // 4. Hard artist overlap gate (`0x2483c4` / `0x2a5eb2`)
    if !query_artist.trim().is_empty() {
        if cand_artist.trim().is_empty() {
            return None;
        }
        let qa_full = query_artist.trim().to_lowercase();
        let ca_full = cand_artist.trim().to_lowercase();
        if qa_full == ca_full {
            score += 45.0;
        } else {
            let q_toks = split_artist_tokens(query_artist);
            let c_toks = split_artist_tokens(cand_artist);
            let matched = (qa_full.chars().count() >= 2
                && ca_full.chars().count() >= 2
                && (qa_full.contains(&ca_full) || ca_full.contains(&qa_full)))
                || q_toks.iter().any(|qt| {
                    c_toks.iter().any(|ct| {
                        qt == ct
                            || (qt.chars().count() >= 2
                                && ct.chars().count() >= 2
                                && (qt.contains(ct) || ct.contains(qt)))
                    })
                });
            if !matched {
                return None;
            }
            score += 30.0;
        }
    }

    // 5. Sub-second duration closeness reward
    if query_dur_sec > 5.0 && cand_dur_sec > 5.0 {
        let diff = (query_dur_sec - cand_dur_sec).abs();
        score += (2.2 - diff).max(0.0) * 20.0;
    }

    Some(score)
}

async fn fetch_netease_matched_song_json(
    title: &str,
    artist: &str,
    album: &str,
    duration_sec: f64,
    timeout_ms: u64,
) -> Option<Value> {
    let query = if artist.trim().is_empty() {
        title.trim().to_string()
    } else {
        format!("{} {}", title.trim(), artist.trim())
    };
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let search_url =
        format!("https://music.163.com/api/search/get/?s={encoded}&type=1&limit=20&offset=0");

    let resp = LYRICS_HTTP
        .post(&search_url)
        .timeout(Duration::from_millis(timeout_ms))
        .header("Referer", "https://music.163.com/")
        .header("Origin", "https://music.163.com")
        .header("Cookie", "os=pc; appver=3.0.18.203152")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().await.ok()?;
    let songs = json.pointer("/result/songs")?.as_array()?;
    if songs.is_empty() {
        return None;
    }

    let album_low = album.trim().to_lowercase();
    let mut best_id: Option<(f64, i64)> = None;

    for s in songs {
        let Some(sid) = s.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let s_name = s
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let s_album = s
            .pointer("/album/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let s_alias = s
            .get("alias")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .unwrap_or_default();
        let s_dur = s.get("duration").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;

        let cand_full = format!("{s_name} {s_album} {s_alias}");
        if has_mismatched_edition_tag(title, album, &cand_full) {
            continue;
        }

        let cand_artists = s
            .get("artists")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.get("name").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();

        let Some(mut score) =
            strict_catalog_match_score(title, artist, duration_sec, s_name, &cand_artists, s_dur)
        else {
            continue;
        };

        let s_alb_low = s_album.to_lowercase();
        if !album_low.is_empty()
            && !s_alb_low.is_empty()
            && (s_alb_low == album_low || s_alb_low.contains(&album_low))
        {
            score += 15.0;
        }

        if best_id.is_none_or(|(bs, _)| score > bs) {
            best_id = Some((score, sid));
        }
    }

    let (_, song_id) = best_id?;

    let lyric_url = format!(
        "https://music.163.com/api/song/lyric?os=pc&id={song_id}&lv=-1&kv=-1&tv=-1&yv=-1"
    );
    let lresp = LYRICS_HTTP
        .get(&lyric_url)
        .timeout(Duration::from_millis(timeout_ms))
        .header("Referer", "https://music.163.com/")
        .header("Cookie", "os=pc; appver=3.0.18.203152")
        .send()
        .await
        .ok()?;
    lresp.json().await.ok()
}

async fn backfill_netease_translations(
    parsed: &mut ParsedLyrics,
    title: &str,
    artist: &str,
    album: &str,
    duration_sec: f64,
) {
    if parsed.lines.is_empty() || parsed.lines.iter().any(|l| l.translation.is_some()) {
        return;
    }
    let Some(ljson) =
        fetch_netease_matched_song_json(title, artist, album, duration_sec, 1500).await
    else {
        return;
    };
    let Some(tlyric) = ljson
        .pointer("/tlyric/lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return;
    };
    let trans_map = parse_lrc_timestamp_map(tlyric);
    if trans_map.is_empty() {
        return;
    }
    let mut used = vec![false; trans_map.len()];
    for line in &mut parsed.lines {
        if line.translation.is_some() {
            continue;
        }
        line.translation =
            find_closest_translation_monotonic(&trans_map, &mut used, line.start_sec, 1.35);
    }
}

async fn fetch_netease_yrc_or_lrc(
    title: &str,
    artist: &str,
    album: &str,
    duration_sec: f64,
) -> Option<ParsedLyrics> {
    let ljson = fetch_netease_matched_song_json(title, artist, album, duration_sec, 6500).await?;
    let tlyric = ljson
        .pointer("/tlyric/lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());

    if let Some(yrc_raw) = ljson
        .pointer("/yrc/lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        if let Some(parsed) = parse_yrc_lyrics(yrc_raw, tlyric) {
            return Some(parsed);
        }
    }

    if let Some(lrc_raw) = ljson
        .pointer("/lrc/lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        if let Some(parsed) = parse_lrc_lyrics(lrc_raw, tlyric, "同步歌词") {
            return Some(parsed);
        }
    }
    None
}

async fn fetch_lrclib(
    title: &str,
    artist: &str,
    album: &str,
    duration_sec: f64,
) -> Option<ParsedLyrics> {
    let mut u = url::Url::parse("https://lrclib.net/api/search").ok()?;
    {
        let mut qp = u.query_pairs_mut();
        qp.append_pair("track_name", title.trim());
        if !artist.trim().is_empty() {
            qp.append_pair("artist_name", artist.trim());
        }
        if !album.trim().is_empty() {
            qp.append_pair("album_name", album.trim());
        }
    }

    let resp = LYRICS_HTTP.get(u).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let arr: Vec<Value> = resp.json().await.ok()?;
    let album_low = album.trim().to_lowercase();
    let mut best: Option<(f64, &str)> = None;

    for item in &arr {
        let Some(synced) = item
            .get("syncedLyrics")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        else {
            continue;
        };
        let d = item.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
        let cand_name = item
            .get("trackName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let cand_artist = item
            .get("artistName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let cand_alb = item
            .get("albumName")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if has_mismatched_edition_tag(title, album, &format!("{cand_name} {cand_alb}")) {
            continue;
        }
        let Some(mut score) =
            strict_catalog_match_score(title, artist, duration_sec, cand_name, cand_artist, d)
        else {
            continue;
        };
        let c_alb_low = cand_alb.to_lowercase();
        if !album_low.is_empty()
            && !c_alb_low.is_empty()
            && (c_alb_low == album_low || c_alb_low.contains(&album_low))
        {
            score += 12.0;
        }

        if best.is_none_or(|(bs, _)| score > bs) {
            best = Some((score, synced));
        }
    }

    let (_, synced_lrc) = best?;
    parse_lrc_lyrics(synced_lrc, None, "LRCLIB 同步歌词")
}

// ============================================================================
// Smart Instrumental / No-Lyrics Classifier
// ============================================================================

fn is_instrumental_marker_line(raw: &str) -> bool {
    let low = raw.trim().to_lowercase();
    if low.is_empty() {
        return false;
    }
    let compact: String = low
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '，' | ',' | '。' | '.' | '！' | '!' | '（' | '）' | '(' | ')' | '[' | ']' | '【' | '】' | '-' | '—' | '~' | '·'))
        .collect();
    if compact.is_empty() {
        return false;
    }
    compact.contains("纯音乐")
        || compact.contains("纯伴奏")
        || compact.contains("纯器乐")
        || compact.contains("没有填词")
        || compact.contains("暂无歌词")
        || compact.contains("没有歌词")
        || compact == "伴奏"
        || compact == "请欣赏"
        || (compact.contains("请欣赏") && compact.chars().count() <= 16)
        || compact == "instrumental"
        || compact == "musiconly"
        || compact == "nolyrics"
        || low.contains("[instrumental]")
        || low.contains("(instrumental)")
        || (low.contains("instrumental") && low.chars().count() <= 28)
}

fn is_credit_or_meta_line(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() {
        return true;
    }
    let low = t.to_lowercase();
    // Pure musical note / ellipsis / separator symbols
    if low
        .chars()
        .all(|c| c.is_whitespace() || matches!(c, '♪' | '♫' | '♬' | '.' | '…' | '-' | '~' | '/' | '*' | '—'))
    {
        return true;
    }
    const CREDIT_PREFIXES: &[&str] = &[
        "作词", "作詞", "作曲", "编曲", "編曲", "制作人", "製作人", "制作", "混音", "母带",
        "录音", "錄音", "监制", "出品", "统筹", "企划", "演唱", "原唱", "和声", "吉他",
        "贝斯", "弦乐", "鼓", "词", "曲", "编", "lyricist", "lyrics by", "composer",
        "composed by", "arranged by", "arranger", "produced by", "producer", "written by",
        "mixed by", "mastered by",
    ];
    if let Some(colon_pos) = low.find(':').or_else(|| low.find('：')) {
        let head = low[..colon_pos].trim();
        if head.chars().count() <= 14
            && CREDIT_PREFIXES
                .iter()
                .any(|kw| head == *kw || head.ends_with(kw) || head.starts_with(kw))
        {
            return true;
        }
    }
    false
}

/// Returns `true` when the track has no lyrics, contains explicit instrumental prompts
/// (`"纯音乐 请欣赏"`, `"纯音乐，请欣赏"`, `"Instrumental"`, etc.), or contains only 1-3
/// songwriter/production credit lines with no real vocal lyrics.
pub fn is_effectively_instrumental_or_empty(lyrics: Option<&ParsedLyrics>) -> bool {
    let Some(lyr) = lyrics else {
        return true;
    };
    if lyr.lines.is_empty() {
        return true;
    }

    let mut inst_markers = 0usize;
    let mut credit_lines = 0usize;
    let mut vocal_lines = 0usize;

    for line in &lyr.lines {
        let s = line.text.trim();
        if s.is_empty() {
            continue;
        }
        if is_instrumental_marker_line(s) {
            inst_markers += 1;
        } else if is_credit_or_meta_line(s) {
            credit_lines += 1;
        } else {
            vocal_lines += 1;
        }
        if let Some(ref tr) = line.translation {
            if is_instrumental_marker_line(tr) {
                inst_markers += 1;
            }
        }
    }

    if vocal_lines == 0 {
        return true;
    }
    if inst_markers > 0 && vocal_lines <= 2 {
        return true;
    }
    if lyr.lines.len() <= 3 && credit_lines >= 1 && vocal_lines <= 1 {
        return true;
    }
    false
}
