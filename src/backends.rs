use crate::theme::Color;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PlaybackBackend {
    /// Apple Music 原生内置播放服务 (`AMPLibraryAgent`) / 官方桌面端
    #[default]
    AppleMusicNative,
    /// 网页版 Apple Music (Edge MusicKit JS v3 引擎，免装商店版客户端)
    AppleMusicWeb,
    /// 网易云音乐 (原生 MediaFoundation + WASAPI 直出播放，支持扫码登录与 YRC 逐字歌词)
    NetEase,
    /// QQ 音乐 (原生 MediaFoundation + WASAPI 直出播放，支持巅峰榜/电台与 Cookie 高音质)
    QQMusic,
    /// 通用外置播放器接管 (Windows SMTC：兼容任意桌面/网页播放器 + 歌词与可视化)
    ExternalSmtc,
}

impl PlaybackBackend {
    pub const ALL: [PlaybackBackend; 5] = [
        PlaybackBackend::AppleMusicNative,
        PlaybackBackend::AppleMusicWeb,
        PlaybackBackend::NetEase,
        PlaybackBackend::QQMusic,
        PlaybackBackend::ExternalSmtc,
    ];

    pub fn short_label(self) -> &'static str {
        match self {
            Self::AppleMusicNative => "Apple Music 原生",
            Self::AppleMusicWeb => "Apple Music 网页",
            Self::NetEase => "网易云音乐",
            Self::QQMusic => "QQ 音乐",
            Self::ExternalSmtc => "外置 SMTC",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::AppleMusicNative => "Apple Music",
            Self::AppleMusicWeb => "Apple Music 网页版",
            Self::NetEase => "网易云音乐",
            Self::QQMusic => "QQ 音乐",
            Self::ExternalSmtc => "外置媒体接管",
        }
    }

    pub fn mode_tag(self) -> &'static str {
        match self {
            Self::AppleMusicNative => "agent",
            Self::AppleMusicWeb => "web-am",
            Self::NetEase => "netease",
            Self::QQMusic => "qqmusic",
            Self::ExternalSmtc => "smtc",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::AppleMusicNative => "通过本机 Apple Music 后台服务播放 Hi-Res / 无损 ALAC，主页与资料库同步官方目录",
            Self::AppleMusicWeb => "通过内置 Edge MusicKit JS 网页引擎播放 Apple Music 完整曲库与电台，无需微软商店原版客户端",
            Self::NetEase => "原生流媒体解码直出，主页/发现/电台/搜索全面切换为网易云曲库，支持内置扫码登录与 YRC 逐字歌词",
            Self::QQMusic => "原生流媒体解码直出，主页/发现/电台/搜索全面切换为 QQ 音乐巅峰榜与歌单，支持 Cookie 解锁高音质",
            Self::ExternalSmtc => "只读/控制当前系统正在播放的任意音乐软件（Spotify / 客户端 / 浏览器），专注歌词与音频可视化",
        }
    }

    pub fn accent_color(self) -> Color {
        match self {
            Self::AppleMusicNative | Self::AppleMusicWeb => Color::rgba(250, 45, 72, 1.0),
            Self::NetEase => Color::rgba(236, 65, 65, 1.0),
            Self::QQMusic => Color::rgba(34, 197, 94, 1.0),
            Self::ExternalSmtc => Color::rgba(56, 189, 248, 1.0),
        }
    }

    pub fn is_apple_music(self) -> bool {
        matches!(self, Self::AppleMusicNative | Self::AppleMusicWeb)
    }

    pub fn uses_native_stream_player(self) -> bool {
        matches!(self, Self::NetEase | Self::QQMusic)
    }

    pub fn supports_motion_artwork(self) -> bool {
        self.is_apple_music()
    }

    pub fn search_placeholder(self) -> &'static str {
        match self {
            Self::AppleMusicNative | Self::AppleMusicWeb => "搜索 Apple Music",
            Self::NetEase => "搜索网易云歌曲、专辑、歌单",
            Self::QQMusic => "搜索 QQ 音乐歌曲、专辑、歌单",
            Self::ExternalSmtc => "搜索曲库（切换对应后端播放）",
        }
    }

    pub fn home_heading(self) -> &'static str {
        match self {
            Self::AppleMusicNative | Self::AppleMusicWeb => "主页",
            Self::NetEase => "网易云 · 发现主页",
            Self::QQMusic => "QQ 音乐 · 推荐主页",
            Self::ExternalSmtc => "主页",
        }
    }

    pub fn browse_heading(self) -> &'static str {
        match self {
            Self::AppleMusicNative | Self::AppleMusicWeb => "新发现",
            Self::NetEase => "新碟与热歌广场",
            Self::QQMusic => "新碟首发与广场",
            Self::ExternalSmtc => "新发现",
        }
    }

    pub fn radio_heading(self) -> &'static str {
        match self {
            Self::AppleMusicNative | Self::AppleMusicWeb => "广播",
            Self::NetEase => "私人 FM 与电台",
            Self::QQMusic => "QQ 音乐电台",
            Self::ExternalSmtc => "广播",
        }
    }

    pub fn next_backend(self) -> Self {
        match self {
            Self::AppleMusicNative => Self::AppleMusicWeb,
            Self::AppleMusicWeb => Self::NetEase,
            Self::NetEase => Self::QQMusic,
            Self::QQMusic => Self::ExternalSmtc,
            Self::ExternalSmtc => Self::AppleMusicNative,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NetEaseQuality {
    Standard,
    #[default]
    ExHigh,
    Lossless,
    HiRes,
}

impl NetEaseQuality {
    pub const ALL: [NetEaseQuality; 4] = [
        NetEaseQuality::Standard,
        NetEaseQuality::ExHigh,
        NetEaseQuality::Lossless,
        NetEaseQuality::HiRes,
    ];

    pub fn api_level(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::ExHigh => "exhigh",
            Self::Lossless => "lossless",
            Self::HiRes => "hires",
        }
    }

    pub fn bitrate_param(self) -> u32 {
        match self {
            Self::Standard => 128_000,
            Self::ExHigh => 320_000,
            Self::Lossless => 999_000,
            Self::HiRes => 1_999_000,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "标准 128k",
            Self::ExHigh => "极高 320k",
            Self::Lossless => "无损 FLAC",
            Self::HiRes => "Hi-Res 无损",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum QQQuality {
    M4a,
    Mp3_128,
    #[default]
    Mp3_320,
    Flac,
}

impl QQQuality {
    pub const ALL: [QQQuality; 4] = [
        QQQuality::M4a,
        QQQuality::Mp3_128,
        QQQuality::Mp3_320,
        QQQuality::Flac,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::M4a => "M4A 标准",
            Self::Mp3_128 => "MP3 128k",
            Self::Mp3_320 => "HQ 320k",
            Self::Flac => "SQ 无损 FLAC",
        }
    }

    /// Ordered fallback chain `(prefix, ext, label, nominal_kbps)` so VIP-gated qualities degrade smoothly.
    pub fn fallback_chain(self) -> &'static [(&'static str, &'static str, &'static str, u32)] {
        match self {
            Self::Flac => &[
                ("F000", "flac", "SQ 无损 FLAC", 960),
                ("M800", "mp3", "HQ 320k MP3", 320),
                ("M500", "mp3", "标准 128k MP3", 128),
                ("C400", "m4a", "M4A 96k", 96),
            ],
            Self::Mp3_320 => &[
                ("M800", "mp3", "HQ 320k MP3", 320),
                ("M500", "mp3", "标准 128k MP3", 128),
                ("C400", "m4a", "M4A 96k", 96),
            ],
            Self::Mp3_128 => &[
                ("M500", "mp3", "标准 128k MP3", 128),
                ("C400", "m4a", "M4A 96k", 96),
            ],
            Self::M4a => &[
                ("C400", "m4a", "M4A 96k", 96),
                ("M500", "mp3", "标准 128k MP3", 128),
            ],
        }
    }
}

/// Resolved direct-playable audio stream metadata returned by NetEase / QQ Music / Preview resolvers.
#[derive(Clone, Debug)]
pub struct ResolvedAudioStream {
    pub url: String,
    pub referer: String,
    pub cookie: Option<String>,
    pub file_ext: String,
    pub quality_label: String,
    pub nominal_bitrate_kbps: u32,
    pub is_trial: bool,
}

#[derive(Clone, Debug, Default)]
pub struct NetEaseQrState {
    pub unikey: String,
    pub qr_url: String,
    /// Flattened `(width, modules_bool)` rendered natively via Direct2D rectangles!
    pub qr_width: usize,
    pub qr_modules: Vec<bool>,
    pub status_code: i64, // 800 expired, 801 waiting, 802 scanned, 803 logged in
    pub status_text: String,
}

#[derive(Clone, Debug, Default)]
pub struct WebAmStatus {
    pub running: bool,
    pub ready: bool,
    pub authorized: bool,
    pub window_visible: bool,
    pub storefront: String,
    pub status_text: String,
}
