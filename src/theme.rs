//! Apple Design Skill Token System (`references/hig/color.md`, `liquid-glass.md`,
//! `typography.md`, `layout.md`, `accessibility.md`, `sidebars.md`, `buttons.md`)

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self {
            r: (r as f32) / 255.0,
            g: (g as f32) / 255.0,
            b: (b as f32) / 255.0,
            a,
        }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 1.0)
    }

    pub const fn with_alpha(self, a: f32) -> Self {
        Self {
            r: self.r,
            g: self.g,
            b: self.b,
            a,
        }
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let u = t.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * u,
            g: self.g + (other.g - self.g) * u,
            b: self.b + (other.b - self.b) * u,
            a: self.a + (other.a - self.a) * u,
        }
    }

    /// Relative luminance per WCAG 2.1 (`accessibility.md › Vision`)
    pub fn relative_luminance(self) -> f32 {
        fn lin(c: f32) -> f32 {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio against `bg` (must be >= 4.5:1 for <=17pt text, >= 3.0:1 for >=18pt/bold)
    pub fn contrast_ratio(self, bg: Self) -> f32 {
        let l1 = self.relative_luminance();
        let l2 = bg.relative_luminance();
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn center_x(self) -> f32 {
        self.x + self.w * 0.5
    }

    pub fn center_y(self) -> f32 {
        self.y + self.h * 0.5
    }

    pub fn contains(self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    pub fn inset(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            w: (self.w - dx * 2.0).max(0.0),
            h: (self.h - dy * 2.0).max(0.0),
        }
    }
}

/// Semantic Palette grounded in iPadOS 26 Apple Music Dark Mode & `apple-design-skill`:
/// - Strict two-layer rule (`liquid-glass.md`): Opaque semantic surfaces in Content Layer;
///   Liquid Glass ONLY on Functional Layer (Sidebar, Floating Top Nav, Floating MiniPlayer Capsule, Popovers).
/// - All text tokens verified >= 4.5:1 WCAG AA contrast on `#101013` / `#1A1A1F`.
pub struct Theme;

impl Theme {
    // --- Content Layer Semantic Surfaces (`color.md › Mobile`, `dark-mode.md`) ---
    /// Primary content canvas (`#101013` obsidian base matching iPad Apple Music screenshot)
    pub const BG_PRIMARY: Color = Color::rgb(16, 16, 19);
    /// Secondary grouped card / settings surface (`#1A1A1F`, opaque — never glass in content layer!)
    pub const BG_ELEVATED: Color = Color::rgb(26, 26, 31);
    /// Tertiary nested surface (`#24242B`)
    pub const BG_TERTIARY: Color = Color::rgb(36, 36, 43);
    /// Subtle row hover tint in content lists
    pub const ROW_HOVER: Color = Color::rgba(255, 255, 255, 0.048);
    /// Active/playing row highlight
    pub const ROW_ACTIVE: Color = Color::rgba(250, 45, 72, 0.11);

    // --- Functional Layer Liquid Glass Materials (`liquid-glass.md`) ---
    /// Sidebar warm-dark regular material (`#1B191E` @ 88% over extended backdrop; 100% when Reduce Transparency)
    pub const SIDEBAR_GLASS: Color = Color::rgba(27, 25, 30, 0.90);
    pub const SIDEBAR_OPAQUE: Color = Color::rgb(26, 24, 29);
    /// Sidebar search & top pill fill (`#2B2930`)
    pub const SIDEBAR_PILL: Color = Color::rgb(43, 41, 48);
    /// Selected sidebar item capsule (`rgba(255,255,255,0.095)`)
    pub const SIDEBAR_SELECTED: Color = Color::rgba(255, 255, 255, 0.095);

    /// Floating MiniPlayer & Floating Toolbar Liquid Glass tint (`liquid-glass.md › Regular`)
    /// High-transmittance crystal optical glaze — avoids muddy gray opacity!
    pub const GLASS_REGULAR_FILL: Color = Color::rgba(24, 26, 34, 0.38);
    pub const GLASS_OPAQUE_FALLBACK: Color = Color::rgb(34, 34, 40);
    /// Specular 1px top/outer glass highlight rim
    pub const GLASS_RIM_LIGHT: Color = Color::rgba(255, 255, 255, 0.30);
    pub const GLASS_RIM_SUBTLE: Color = Color::rgba(255, 255, 255, 0.12);
    /// Floating button circle (`<`, `Shuffle`, `+`, `•••`)
    pub const GLASS_BTN_FILL: Color = Color::rgba(34, 36, 46, 0.44);
    pub const GLASS_BTN_HOVER: Color = Color::rgba(58, 62, 76, 0.58);

    // --- Foreground Labels & Separators (`color.md`, `accessibility.md` >= 4.5:1) ---
    /// Primary Label (`#F5F5F7` -> 17.3:1 on BG_PRIMARY)
    pub const TEXT_PRIMARY: Color = Color::rgb(245, 245, 247);
    /// Secondary Label (`#9A9AA2` -> 6.8:1 on BG_PRIMARY, passes WCAG AA 4.5:1 at 12pt+)
    pub const TEXT_SECONDARY: Color = Color::rgb(154, 154, 162);
    /// Muted / Caption Label (`#8A8A93` -> 5.5:1 on BG_PRIMARY, passes WCAG AA 4.5:1)
    pub const TEXT_MUTED: Color = Color::rgb(138, 138, 147);
    /// Hairline inset separator (`rgba(255, 255, 255, 0.075)`)
    pub const SEPARATOR: Color = Color::rgba(255, 255, 255, 0.075);

    // --- Brand & Semantic Signals (`branding.md`, `buttons.md`) ---
    /// Apple Music Signature Crimson (`#FA2D48`) — used sparingly for sidebar glyphs & active accents
    pub const ACCENT_RED: Color = Color::rgb(250, 45, 72);
    pub const ACCENT_RED_HOVER: Color = Color::rgb(255, 69, 93);
    /// Hero Primary Call-to-Action Pill (`#FFFFFF` bg + `#0A0A0D` text = 19.4:1 contrast, matching screenshot `▶ 播放`)
    pub const HERO_PLAY_BG: Color = Color::rgb(255, 255, 255);
    pub const HERO_PLAY_FG: Color = Color::rgb(10, 10, 13);

    /// Semantic audio telemetry signals (paired with text/shape labels, never color-only per `accessibility.md`)
    pub const SIGNAL_GREEN: Color = Color::rgb(48, 209, 88);
    pub const SIGNAL_CYAN: Color = Color::rgb(50, 173, 230);
    pub const SIGNAL_AMBER: Color = Color::rgb(255, 214, 10);
    pub const SIGNAL_PURPLE: Color = Color::rgb(191, 90, 242);

    // --- Layout & Sizing Constants (`layout.md`, `sidebars.md`, `accessibility.md`) ---
    pub const SIDEBAR_WIDTH: f32 = 264.0;
    pub const SIDEBAR_AUTO_COLLAPSE_WIDTH: f32 = 860.0;
    pub const TRACK_ROW_HEIGHT: f32 = 56.0;
    pub const MINI_PLAYER_HEIGHT: f32 = 62.0;
    pub const MINI_PLAYER_BOTTOM_MARGIN: f32 = 14.0;
    pub const MINI_PLAYER_MAX_WIDTH: f32 = 760.0;

    /// Unified Top Bar Geometry: strict horizontal centerline at `Y = 31.0` across Sidebar, Content & Lyrics!
    pub const TOP_BAR_Y: f32 = 14.0;
    pub const TOP_BAR_H: f32 = 34.0;
    pub const TOP_BAR_RADIUS: f32 = 17.0;
}
