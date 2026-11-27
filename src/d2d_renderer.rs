#![cfg(target_os = "windows")]

use crate::theme::{Color, RectF, Theme};
use std::cell::Cell;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::ffi::c_void;
use windows::core::{Interface, HSTRING};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_2DAFFINETRANSFORM_INTERPOLATION_MODE_LINEAR, D2D1_ALPHA_MODE_IGNORE,
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_BORDER_MODE_HARD,
    D2D1_BORDER_MODE_SOFT, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT, D2D_POINT_2U, D2D_RECT_F,
    D2D_RECT_U, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D12DAffineTransform, CLSID_D2D1Border, CLSID_D2D1ColorMatrix, CLSID_D2D1Crop,
    CLSID_D2D1DisplacementMap, CLSID_D2D1GaussianBlur, D2D1CreateFactory, ID2D1Bitmap1,
    ID2D1BitmapBrush1, ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory, ID2D1Factory1,
    ID2D1Geometry, ID2D1Image, ID2D1ImageBrush, ID2D1LinearGradientBrush, ID2D1PathGeometry,
    ID2D1RadialGradientBrush, ID2D1RenderTarget, ID2D1SolidColorBrush,
    D2D1_2DAFFINETRANSFORM_PROP_BORDER_MODE, D2D1_2DAFFINETRANSFORM_PROP_INTERPOLATION_MODE,
    D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_BRUSH_PROPERTIES1,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_BORDER_EDGE_MODE_CLAMP, D2D1_BORDER_PROP_EDGE_MODE_X,
    D2D1_BORDER_PROP_EDGE_MODE_Y, D2D1_CAP_STYLE_ROUND, D2D1_CHANNEL_SELECTOR_G,
    D2D1_CHANNEL_SELECTOR_R, D2D1_COLORMATRIX_PROP_COLOR_MATRIX, D2D1_COMBINE_MODE_EXCLUDE,
    D2D1_CROP_PROP_RECT, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DISPLACEMENTMAP_PROP_SCALE,
    D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT, D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_DRAW_TEXT_OPTIONS_NO_SNAP, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2, D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED,
    D2D1_GAUSSIANBLUR_OPTIMIZATION_SPEED, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_IMAGE_BRUSH_PROPERTIES, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LAYER_OPTIONS1_NONE,
    D2D1_LAYER_PARAMETERS1, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_LINE_JOIN_ROUND,
    D2D1_PRIMITIVE_BLEND_COPY, D2D1_PRIMITIVE_BLEND_SOURCE_OVER, D2D1_PROPERTY_TYPE_BLOB,
    D2D1_PROPERTY_TYPE_ENUM, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_MATRIX_3X2,
    D2D1_PROPERTY_TYPE_UINT32, D2D1_PROPERTY_TYPE_VECTOR4, D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Multithread, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFactory3, IDWriteFactory5, IDWriteFontCollection,
    IDWriteFontFallback, IDWriteInMemoryFontFileLoader, IDWriteInlineObject,
    IDWriteRenderingParams, IDWriteTextFormat, IDWriteTextFormat1, IDWriteTextFormat3,
    IDWriteTextLayout, IDWriteTypography, DWRITE_AUTOMATIC_FONT_AXES_OPTICAL_SIZE,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FEATURE, DWRITE_FONT_FEATURE_TAG_SLASHED_ZERO,
    DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_HIT_TEST_METRICS, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIDevice1, IDXGIDevice3, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
    DXGI_PRESENT, DXGI_PRESENT_PARAMETERS, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Media::MediaFoundation::{IMFDXGIDeviceManager, MFCreateDXGIDeviceManager};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows_numerics::{Matrix3x2, Vector2};

/// Official open-source Inter v4.1 Variable Font (SIL OFL 1.1 — `assets/fonts/OFL.txt`).
pub const INTER_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");
pub const INTER_FONT_FAMILY: &str = "Inter Variable Text";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfIcon {
    House,
    Grid,
    Radio,
    Visualizer,
    Clock,
    Mic,
    Albums,
    MusicNote,
    PersonSquare,
    DownloadCircle,
    PlaylistGrid,
    Star,
    Search,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    SidebarLeft,
    Play,
    Pause,
    Prev,
    Next,
    Shuffle,
    Repeat,
    Translate,
    LyricsBubble,
    QueueList,
    MoreDots,
    Plus,
    Check,
    Gear,
    Speaker,
    Folder,
    Trash,
    Refresh,
    Sparkles,
    WinMinimize,
    WinMaximize,
    WinRestore,
    WinClose,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlassBtnVariant {
    Regular,
    Subtle,
    Tinted(Color),
    ProminentLight,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    size_tenths: u16,
    weight: u16,
    align: TextAlign,
    vcenter: bool,
    wrap: bool,
    mono: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtworkSizeTier {
    /// Small list rows (42–48 DIP), sidebar (24 DIP), and queue rows (42 DIP) -> 128×128
    SmallThumb,
    /// Editorial shelves and album/playlist grid cards -> 384×384 (256 in aggressive mode)
    Card,
    /// Detail header hero, Lyrics full cover, and Cover Flow center -> 720×720 (512 in aggressive mode)
    Hero,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum IconGeomKind {
    RadioArcs,
    MicArc,
    PersonShoulders,
    DownloadArrow,
    StarFill,
    PlayFill,
    PrevFill,
    NextFill,
    ShuffleStrokes,
    RepeatArrow,
    TranslateStrokes,
    CheckStroke,
    SpeakerBodyFill,
    SpeakerWaveStroke,
    FolderStroke,
    RefreshArc,
    SparklesFill,
    WinRestoreBack,
}

struct LruNode<K, V> {
    key: K,
    val: V,
    prev: Option<usize>,
    next: Option<usize>,
}

/// O(1) indexed doubly-linked LRU cache (zero `unsafe`, zero duplicate deque keys, O(1) hit/insert/evict).
struct IndexLru<K, V> {
    map: HashMap<K, usize>,
    nodes: Vec<Option<LruNode<K, V>>>,
    free: Vec<usize>,
    head: Option<usize>, // Most recently used
    tail: Option<usize>, // Least recently used
}

impl<K: Eq + std::hash::Hash + Clone, V> IndexLru<K, V> {
    fn with_capacity(cap: usize) -> Self {
        Self {
            map: HashMap::with_capacity(cap),
            nodes: Vec::with_capacity(cap),
            free: Vec::new(),
            head: None,
            tail: None,
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.map.len()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn unlink(&mut self, idx: usize) {
        let (prev, next) = {
            let n = self.nodes[idx].as_mut().unwrap();
            (n.prev.take(), n.next.take())
        };
        if let Some(p) = prev {
            self.nodes[p].as_mut().unwrap().next = next;
        } else {
            self.head = next;
        }
        if let Some(nx) = next {
            self.nodes[nx].as_mut().unwrap().prev = prev;
        } else {
            self.tail = prev;
        }
    }

    fn push_front(&mut self, idx: usize) {
        let old_head = self.head;
        {
            let n = self.nodes[idx].as_mut().unwrap();
            n.prev = None;
            n.next = old_head;
        }
        if let Some(oh) = old_head {
            self.nodes[oh].as_mut().unwrap().prev = Some(idx);
        } else {
            self.tail = Some(idx);
        }
        self.head = Some(idx);
    }

    #[inline]
    fn move_to_front(&mut self, idx: usize) {
        if self.head == Some(idx) {
            return;
        }
        self.unlink(idx);
        self.push_front(idx);
    }

    #[inline]
    fn get_touch(&mut self, key: &K) -> Option<&V> {
        let idx = *self.map.get(key)?;
        self.move_to_front(idx);
        self.nodes[idx].as_ref().map(|n| &n.val)
    }

    #[inline]
    fn get_peek(&self, key: &K) -> Option<&V> {
        let idx = *self.map.get(key)?;
        self.nodes[idx].as_ref().map(|n| &n.val)
    }

    #[inline]
    fn get_mut_peek(&mut self, key: &K) -> Option<&mut V> {
        let idx = *self.map.get(key)?;
        self.nodes[idx].as_mut().map(|n| &mut n.val)
    }

    fn pop_lru(&mut self) -> Option<(K, V)> {
        let tail_idx = self.tail?;
        self.unlink(tail_idx);
        let node = self.nodes[tail_idx].take()?;
        self.free.push(tail_idx);
        self.map.remove(&node.key);
        Some((node.key, node.val))
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        let idx = self.map.remove(key)?;
        self.unlink(idx);
        let node = self.nodes[idx].take()?;
        self.free.push(idx);
        Some(node.val)
    }

    fn insert(&mut self, key: K, val: V, max_cap: usize) {
        if let Some(&idx) = self.map.get(&key) {
            if let Some(n) = self.nodes[idx].as_mut() {
                n.val = val;
            }
            self.move_to_front(idx);
            return;
        }
        let cap = max_cap.max(1);
        while self.map.len() >= cap {
            if self.pop_lru().is_none() {
                break;
            }
        }
        let idx = if let Some(slot) = self.free.pop() {
            self.nodes[slot] = Some(LruNode {
                key: key.clone(),
                val,
                prev: None,
                next: None,
            });
            slot
        } else {
            let slot = self.nodes.len();
            self.nodes.push(Some(LruNode {
                key: key.clone(),
                val,
                prev: None,
                next: None,
            }));
            slot
        };
        self.map.insert(key, idx);
        self.push_front(idx);
    }

    fn trim_to(&mut self, max_cap: usize) {
        while self.map.len() > max_cap {
            if self.pop_lru().is_none() {
                break;
            }
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.nodes.clear();
        self.free.clear();
        self.head = None;
        self.tail = None;
    }

    fn keys_oldest_to_newest(&self) -> Vec<K> {
        let mut out = Vec::with_capacity(self.map.len());
        let mut cur = self.tail;
        while let Some(idx) = cur {
            if let Some(ref node) = self.nodes[idx] {
                out.push(node.key.clone());
                cur = node.prev;
            } else {
                break;
            }
        }
        out
    }
}

pub struct DecodedArtworkCpu {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub palette: [Color; 4],
}

pub struct CachedArtwork {
    pub bitmap: ID2D1Bitmap1,
    pub brush: ID2D1BitmapBrush1,
    pub width: u32,
    pub height: u32,
    pub palette: [Color; 4],
    /// `Some(Instant)` ONLY when this artwork stem was seen for the very first time (smooth 180ms fade-in);
    /// `None` when upgraded or already cached so re-renders NEVER flash black!
    pub fade_in_start: Option<std::time::Instant>,
    /// True LRU recency stamp touched on every `&self` render hit via interior mutability (`Cell<u64>`).
    pub last_used: Cell<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LyricsLineBmpKey {
    pub line_idx: usize,
    pub max_w_q: u16,
    pub show_trans: bool,
}

pub struct CachedLyricsLineBmp {
    pub bitmap: ID2D1Bitmap1,
    pub blurred_bmp: Option<ID2D1Bitmap1>,
    pub blurred_sigma_q: u16,
    pub dip_w: f32,
    pub dip_h: f32,
    pub pad_dip: f32,
    pub scale: f32,
    pub written_at_flush_seq: u64,
}

#[derive(Default)]
pub struct RowFxSlot {
    pub bitmap: Option<ID2D1Bitmap1>,
    pub alloc_w: u32,
    pub alloc_h: u32,
    pub used_dip_w: f32,
    pub used_dip_h: f32,
    pub pad_dip: f32,
}

pub struct ActiveGlowSlot {
    pub bitmap: Option<ID2D1Bitmap1>,
    pub baked_bitmap: Option<ID2D1Bitmap1>,
    pub alloc_w: u32,
    pub alloc_h: u32,
    pub used_dip_w: f32,
    pub used_dip_h: f32,
    pub pad_dip: f32,
    outer_blur_effect: Option<ID2D1Effect>,
    inner_blur_effect: Option<ID2D1Effect>,
    outer_alpha_effect: Option<ID2D1Effect>,
    inner_alpha_effect: Option<ID2D1Effect>,
}

impl Default for ActiveGlowSlot {
    fn default() -> Self {
        Self {
            bitmap: None,
            baked_bitmap: None,
            alloc_w: 0,
            alloc_h: 0,
            used_dip_w: 0.0,
            used_dip_h: 0.0,
            pad_dip: 56.0,
            outer_blur_effect: None,
            inner_blur_effect: None,
            outer_alpha_effect: None,
            inner_alpha_effect: None,
        }
    }
}

pub const BACKDROP_DIRTY_TOP: u32 = 1 << 0;
pub const BACKDROP_DIRTY_BOTTOM: u32 = 1 << 1;
pub const BACKDROP_DIRTY_ALL: u32 = BACKDROP_DIRTY_TOP | BACKDROP_DIRTY_BOTTOM;

const GLASS_EFFECT_POOL_SIZE: usize = 12;
const LINEAR_GRAD_CACHE_CAP: usize = 128;
const RADIAL_GLOW_CACHE_CAP: usize = 64;
const GLASS_SHADOW_CACHE_CAP: usize = 48;
pub const MOTION_TEXTURE_LRU_CAP: usize = 28;
pub const DIRTY_RECT_FULL_FALLBACK_RATIO: f32 = 0.55;
pub const DIRTY_RECT_RING_DEPTH: usize = 3;
const MAX_PRESENT1_DIRTY_RECTS: usize = 32;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DirtyFramePlan {
    pub dirty_rects_dip: Vec<RectF>,
    pub force_full: bool,
}

impl DirtyFramePlan {
    pub fn full() -> Self {
        Self {
            dirty_rects_dip: Vec::new(),
            force_full: true,
        }
    }

    pub fn idle() -> Self {
        Self {
            dirty_rects_dip: Vec::new(),
            force_full: false,
        }
    }

    pub fn with_rects(dirty_rects_dip: Vec<RectF>) -> Self {
        Self {
            dirty_rects_dip,
            force_full: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirtyRectPx {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl DirtyRectPx {
    #[inline]
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    #[inline]
    pub fn area(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            ((self.right - self.left) as u64) * ((self.bottom - self.top) as u64)
        }
    }

    #[inline]
    pub fn clamp_to_bounds(self, width_px: u32, height_px: u32) -> Option<Self> {
        let max_w = width_px.min(i32::MAX as u32) as i32;
        let max_h = height_px.min(i32::MAX as u32) as i32;
        let left = self.left.clamp(0, max_w);
        let top = self.top.clamp(0, max_h);
        let right = self.right.clamp(0, max_w);
        let bottom = self.bottom.clamp(0, max_h);
        if right > left && bottom > top {
            Some(Self {
                left,
                top,
                right,
                bottom,
            })
        } else {
            None
        }
    }

    #[inline]
    pub fn union_bbox(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    #[inline]
    pub fn overlaps_or_touches(self, other: Self) -> bool {
        self.left <= other.right
            && other.left <= self.right
            && self.top <= other.bottom
            && other.top <= self.bottom
    }
}

/// Quantizes DIP dirty rects outward (`floor`/`ceil`) to physical pixels, clamps strictly to
/// `[0..width_px, 0..height_px]`, drops empty/out-of-bounds rects, and coalesces overlapping rects.
pub fn quantize_and_clamp_dirty_rects(
    rects_dip: &[RectF],
    dpi_scale: f32,
    width_px: u32,
    height_px: u32,
) -> Vec<DirtyRectPx> {
    if width_px == 0 || height_px == 0 || rects_dip.is_empty() {
        return Vec::new();
    }
    let s = if dpi_scale.is_finite() && dpi_scale > 0.0 {
        dpi_scale
    } else {
        1.0
    };
    let mut raw = Vec::with_capacity(rects_dip.len());
    for r in rects_dip {
        if !r.x.is_finite()
            || !r.y.is_finite()
            || !r.w.is_finite()
            || !r.h.is_finite()
            || r.w <= 0.0
            || r.h <= 0.0
        {
            continue;
        }
        let l = (r.x * s).floor() as i32;
        let t = (r.y * s).floor() as i32;
        let ri = ((r.x + r.w) * s).ceil() as i32;
        let b = ((r.y + r.h) * s).ceil() as i32;
        if let Some(clamped) = DirtyRectPx::new(l, t, ri, b).clamp_to_bounds(width_px, height_px) {
            raw.push(clamped);
        }
    }
    coalesce_dirty_rects_px(&raw)
}

/// Coalesces overlapping/edge-adjacent physical-pixel dirty rects into a clean bounded list
/// (preserving 100% coverage of the input union).
pub fn coalesce_dirty_rects_px(rects: &[DirtyRectPx]) -> Vec<DirtyRectPx> {
    let mut out: Vec<DirtyRectPx> = rects.iter().copied().filter(|r| !r.is_empty()).collect();
    let mut changed = true;
    while changed && out.len() > 1 {
        changed = false;
        'outer: for i in 0..out.len() {
            for j in (i + 1)..out.len() {
                if out[i].overlaps_or_touches(out[j]) {
                    let merged = out[i].union_bbox(out[j]);
                    out[i] = merged;
                    out.swap_remove(j);
                    changed = true;
                    break 'outer;
                }
            }
        }
    }
    while out.len() > MAX_PRESENT1_DIRTY_RECTS {
        let mut best_pair = (0usize, 1usize);
        let mut best_waste = u64::MAX;
        for i in 0..out.len() {
            for j in (i + 1)..out.len() {
                let merged = out[i].union_bbox(out[j]);
                let waste = merged
                    .area()
                    .saturating_sub(out[i].area().saturating_add(out[j].area()));
                if waste < best_waste {
                    best_waste = waste;
                    best_pair = (i, j);
                }
            }
        }
        let (i, j) = best_pair;
        out[i] = out[i].union_bbox(out[j]);
        out.swap_remove(j);
    }
    out.sort_unstable_by_key(|r| (r.top, r.left, r.bottom, r.right));
    out
}

/// Computes the exact 2D union area (in physical pixels) of an arbitrary set of rectangles
/// via an X-coordinate sweep line (zero double-counting on overlapping rects).
pub fn compute_dirty_rects_union_area_px(rects: &[DirtyRectPx]) -> u64 {
    let valid: Vec<DirtyRectPx> = rects.iter().copied().filter(|r| !r.is_empty()).collect();
    if valid.is_empty() {
        return 0;
    }
    if valid.len() == 1 {
        return valid[0].area();
    }
    let mut xs = Vec::with_capacity(valid.len() * 2);
    for r in &valid {
        xs.push(r.left);
        xs.push(r.right);
    }
    xs.sort_unstable();
    xs.dedup();
    let mut total_area = 0u64;
    let mut y_spans: Vec<(i32, i32)> = Vec::with_capacity(valid.len());
    for pair in xs.windows(2) {
        let x0 = pair[0];
        let x1 = pair[1];
        if x1 <= x0 {
            continue;
        }
        y_spans.clear();
        for r in &valid {
            if r.left <= x0 && r.right >= x1 {
                y_spans.push((r.top, r.bottom));
            }
        }
        if y_spans.is_empty() {
            continue;
        }
        y_spans.sort_unstable_by_key(|&(t, b)| (t, b));
        let mut covered_y = 0u64;
        let mut cur_t = y_spans[0].0;
        let mut cur_b = y_spans[0].1;
        for &(t, b) in &y_spans[1..] {
            if t <= cur_b {
                cur_b = cur_b.max(b);
            } else {
                covered_y += (cur_b - cur_t) as u64;
                cur_t = t;
                cur_b = b;
            }
        }
        covered_y += (cur_b - cur_t) as u64;
        total_area = total_area.saturating_add(((x1 - x0) as u64).saturating_mul(covered_y));
    }
    total_area
}

/// Computes the Flip-Model (`BufferCount = 3`) 3-frame union `D_T ∪ D_{T-1} ∪ D_{T-2}`,
/// strictly clamped to `[0..width_px, 0..height_px]` and coalesced.
pub fn merge_dirty_rect_ring_union(
    current: &[DirtyRectPx],
    prev1: &[DirtyRectPx],
    prev2: &[DirtyRectPx],
    width_px: u32,
    height_px: u32,
) -> Vec<DirtyRectPx> {
    if width_px == 0 || height_px == 0 {
        return Vec::new();
    }
    let mut combined = Vec::with_capacity(current.len() + prev1.len() + prev2.len());
    for r in current.iter().chain(prev1.iter()).chain(prev2.iter()) {
        if let Some(clamped) = r.clamp_to_bounds(width_px, height_px) {
            combined.push(clamped);
        }
    }
    coalesce_dirty_rects_px(&combined)
}

/// Computes the tight axis-aligned bounding box covering all non-empty dirty rects.
pub fn dirty_rects_bounding_box(rects: &[DirtyRectPx]) -> Option<DirtyRectPx> {
    let mut iter = rects.iter().copied().filter(|r| !r.is_empty());
    let first = iter.next()?;
    Some(iter.fold(first, |acc, r| acc.union_bbox(r)))
}

/// Returns `true` when the exact union area of `rects` exceeds `55%` (`DIRTY_RECT_FULL_FALLBACK_RATIO`)
/// of the swapchain area, triggering a full-frame fallback.
pub fn dirty_area_exceeds_full_threshold(
    rects: &[DirtyRectPx],
    width_px: u32,
    height_px: u32,
) -> bool {
    if width_px == 0 || height_px == 0 {
        return true;
    }
    let total_px = (width_px as u64).saturating_mul(height_px as u64);
    if total_px == 0 {
        return true;
    }
    let dirty_px = compute_dirty_rects_union_area_px(rects);
    (dirty_px as f64) > (total_px as f64) * (DIRTY_RECT_FULL_FALLBACK_RATIO as f64)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderFrameStats {
    pub glass_push_layers: u32,
    pub glass_effect_rebinds: u32,
    pub glass_shadow_geom_allocs: u32,
    pub glass_snapshot_flushes: u32,
    pub glass_snapshot_blurs: u32,
    pub glass_solid_fastpath_draws: u32,
    pub glass_snapshot_cache_hits: u32,
    pub motion_copy_from_memory_bytes: u64,
    pub motion_dxva_frames: u64,
    pub motion_cpu_fallback_frames: u64,
    pub present1_dirty_frames: u64,
    pub present_full_frames: u64,
    pub skipped_idle_frames: u64,
}

struct MotionDxgiSlot {
    dxgi_tex_ptr: usize,
    width: u32,
    height: u32,
    _bitmap: ID2D1Bitmap1,
    brush: ID2D1BitmapBrush1,
    _dxgi_tex_keepalive: ID3D11Texture2D,
}

struct MotionTextureEntry {
    /// 2-slot ping-pong cache matching double-buffered DXVA `out_tex` (zero COM re-wrap on alternating frames!).
    dxgi_slots: [Option<MotionDxgiSlot>; 2],
    dxgi_next_replace: usize,
    active_dxgi_slot: Option<usize>,
    /// CPU fallback bitmap/brush when DXVA is unavailable.
    cpu_surface: Option<(ID2D1Bitmap1, ID2D1BitmapBrush1, u32, u32)>,
    cached_seq: u64,
    last_used_frame: u64,
    last_bgra_arc: Option<std::sync::Arc<Vec<u8>>>,
}

struct DirtyPresentationState {
    full_invalidation_frames: u8,
    history_prev1: Vec<DirtyRectPx>,
    history_prev2: Vec<DirtyRectPx>,
    pending_present_rects_px: Vec<DirtyRectPx>,
    last_redraw_rects_px: Vec<DirtyRectPx>,
    active_dirty_clip_px: Option<DirtyRectPx>,
    active_dirty_clip_pushed: bool,
    last_corner_key: Option<(u16, bool)>,
}

impl DirtyPresentationState {
    fn new() -> Self {
        Self {
            full_invalidation_frames: DIRTY_RECT_RING_DEPTH as u8,
            history_prev1: Vec::new(),
            history_prev2: Vec::new(),
            pending_present_rects_px: Vec::new(),
            last_redraw_rects_px: Vec::new(),
            active_dirty_clip_px: None,
            active_dirty_clip_pushed: false,
            last_corner_key: None,
        }
    }

    fn invalidate_full(&mut self) {
        self.full_invalidation_frames = DIRTY_RECT_RING_DEPTH as u8;
        self.history_prev1.clear();
        self.history_prev2.clear();
        self.pending_present_rects_px.clear();
        self.last_redraw_rects_px.clear();
        self.active_dirty_clip_px = None;
        self.active_dirty_clip_pushed = false;
    }
}

struct GlassEffectSlot {
    disp_xform: ID2D1Effect,
    disp_border: ID2D1Effect,
    disp_g: ID2D1Effect,
    output: ID2D1Image,
    bound_bg_ptr: usize,
    bound_disp_ptr: usize,
    last_xf_bytes: [u8; 24],
    last_scale_bytes: [u8; 4],
    state_valid: bool,
}

struct CachedShadowPair {
    far_geom: Option<ID2D1PathGeometry>,
    close_geom: Option<ID2D1PathGeometry>,
}

#[derive(Clone, Copy, Debug)]
struct GlassAnimState {
    hover_t: f32,
    hover_target: f32,
    press_t: f32,
    press_target: f32,
    scale: f32,
    scale_vel: f32,
    scale_target: f32,
    caustic_x: f32,
    caustic_y: f32,
    caustic_target_x: f32,
    caustic_target_y: f32,
    last_frame: u64,
}

pub struct D2DRenderer {
    pub d2d_factory: ID2D1Factory1,
    d3d_device: ID3D11Device,
    d3d_video_support: bool,
    mf_dxgi_device_manager: Option<IMFDXGIDeviceManager>,
    mf_dxgi_reset_token: u32,
    pub dxgi_device: IDXGIDevice,
    pub d2d_device: ID2D1Device,
    pub gpu_staging_dirty: bool,
    pub dc: ID2D1DeviceContext,
    pub swap_chain: IDXGISwapChain1,
    pub dcomp_device: Option<IDCompositionDevice>,
    pub dcomp_target: Option<IDCompositionTarget>,
    pub dcomp_visual: Option<IDCompositionVisual>,
    pub premultiplied_swapchain: bool,
    pub needs_dcomp_commit: bool,
    pub swap_buffer_count: u32,
    pub target_bitmap: Option<ID2D1Bitmap1>,
    pub dwrite: IDWriteFactory,
    custom_font_collection: Option<IDWriteFontCollection>,
    system_font_fallback: Option<IDWriteFontFallback>,
    mono_typography: Option<IDWriteTypography>,
    bundled_inter_family: String,
    bundled_inter_loaded: bool,
    in_memory_font_loader: Option<IDWriteInMemoryFontFileLoader>,
    lyrics_text_params: Option<IDWriteRenderingParams>,
    lyrics_subpixel_mode: bool,
    pub wic: IWICImagingFactory,
    pub solid_brush: ID2D1SolidColorBrush,
    pub round_stroke: windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle,
    pub blur_effect: Option<ID2D1Effect>,
    pub glass_crop_effect: Option<ID2D1Effect>,
    pub glass_vibrancy_effect: Option<ID2D1Effect>,
    glass_top_downscale_effect: Option<ID2D1Effect>,
    glass_bot_downscale_effect: Option<ID2D1Effect>,
    glass_local_downscale_effect: Option<ID2D1Effect>,
    glass_bot_blur_effect: Option<ID2D1Effect>,
    glass_bot_vibrancy_effect: Option<ID2D1Effect>,
    glass_border_effect: Option<ID2D1Effect>,
    glass_bot_bg_scale_effect: Option<ID2D1Effect>,
    glass_bot_border_effect: Option<ID2D1Effect>,
    glass_local_scale_effect: Option<ID2D1Effect>,
    glass_local_border_effect: Option<ID2D1Effect>,
    glass_bg_scale_effect: Option<ID2D1Effect>,
    glass_disp_xform_effect: Option<ID2D1Effect>,
    glass_disp_border_effect: Option<ID2D1Effect>,
    glass_disp_g_effect: Option<ID2D1Effect>,
    pub line_blur_effect: Option<ID2D1Effect>,
    line_blur_effect_2: Option<ID2D1Effect>,
    line_blur_slot_cursor: Cell<u8>,
    dc_flush_seq: Cell<u64>,
    pub trans_bg_blur_effect: Option<ID2D1Effect>,
    pub trans_fg_blur_effect: Option<ID2D1Effect>,
    pub glow_blur_effect: Option<ID2D1Effect>,
    pub glow_inner_blur_effect: Option<ID2D1Effect>,
    pub glow_outer_alpha_effect: Option<ID2D1Effect>,
    pub glow_inner_alpha_effect: Option<ID2D1Effect>,
    pub mesh_blur_effect: Option<ID2D1Effect>,
    mesh_vibrancy_effect: Option<ID2D1Effect>,
    mesh_feather_brush: Option<ID2D1RadialGradientBrush>,
    cherenkov_brush: Option<ID2D1RadialGradientBrush>,
    glass_caustic_brush: Option<ID2D1RadialGradientBrush>,
    glass_image_brush: Option<ID2D1ImageBrush>,
    window_rim_brush: Option<ID2D1LinearGradientBrush>,
    glass_vrim_brush: Option<ID2D1LinearGradientBrush>,
    glass_vrim_hover_brush: Option<ID2D1LinearGradientBrush>,
    glass_inner_gleam_brush: Option<ID2D1LinearGradientBrush>,
    linear_grad_cache: std::cell::RefCell<IndexLru<(u8, u64), ID2D1LinearGradientBrush>>,
    radial_glow_cache: std::cell::RefCell<IndexLru<u64, ID2D1RadialGradientBrush>>,
    glass_shadow_cache: std::cell::RefCell<IndexLru<(u16, u16, u16, u16), CachedShadowPair>>,
    icon_geom_cache: std::cell::RefCell<HashMap<IconGeomKind, ID2D1PathGeometry>>,

    // Cached exterior corner mask geometry for custom rounded window silhouette
    corner_cutout_key: (u32, u32, u16),
    corner_cutout_geom: Option<ID2D1PathGeometry>,

    // Reusable tight local 0.5x popup/overlay scratch bitmaps + strip-sized offscreen glass targets (7x VRAM cut!)
    glass_capture_bmp: Option<ID2D1Bitmap1>,
    glass_cap_w: u32,
    glass_cap_h: u32,
    glass_local_half_blurred: Option<ID2D1Bitmap1>,
    glass_local_half_w: u32,
    glass_local_half_h: u32,
    glass_local_fallback_pending_flush: Cell<bool>,
    // Full-window fallback snapshot (allocated ONLY when `!use_strips`)
    glass_frame_snapshot: Option<ID2D1Bitmap1>,
    glass_snap_w: u32,
    glass_snap_h: u32,
    glass_blurred_snapshot: Option<ID2D1Bitmap1>,
    glass_half_w: u32,
    glass_half_h: u32,
    // Strip-sized top & bottom glass targets (`W × copy_top_px` and `W × bot_copy_h` -> 20.4 MB -> ~2.7 MB!)
    glass_top_snap: Option<ID2D1Bitmap1>,
    glass_top_snap_w: u32,
    glass_top_snap_h: u32,
    glass_top_blurred: Option<ID2D1Bitmap1>,
    glass_top_half_w: u32,
    glass_top_half_h: u32,
    glass_bot_snap: Option<ID2D1Bitmap1>,
    glass_bot_snap_w: u32,
    glass_bot_snap_h: u32,
    glass_bot_blurred: Option<ID2D1Bitmap1>,
    glass_bot_half_w: u32,
    glass_bot_half_h: u32,
    glass_strip_top_px: u32,
    glass_strip_bot_px: u32,
    glass_copy_top_px: u32,
    glass_copy_bot_px: u32,
    glass_snapshot_active: bool,
    glass_snapshot_strip_mode: bool,
    glass_snapshot_write_gen: u64,
    glass_batch_bounded_bg: Option<ID2D1Image>,
    glass_batch_bg_key: Option<(usize, u32, u32, u32, u32)>,
    glass_batch_bounded_top: Option<ID2D1Image>,
    glass_batch_top_key: Option<(usize, u32, u32, u32, u32)>,
    glass_batch_bounded_bot: Option<ID2D1Image>,
    glass_batch_bot_key: Option<(usize, u32, u32, u32, u32, u32)>,
    glass_effect_pool: Vec<GlassEffectSlot>,
    glass_effect_cursor: usize,
    glass_effect_overflow_used: bool,
    glass_top_solid_bg: Option<Color>,
    glass_top_x_spans_px: [(u32, u32); 2],
    glass_top_x_span_count: u8,
    glass_bot_x_span_px: Option<(u32, u32)>,
    glass_cache_valid_top: bool,
    glass_cache_valid_bot: bool,
    glass_cache_w_px: u32,
    glass_cache_h_px: u32,
    glass_cache_dpi_q: u32,
    glass_cache_lyrics_sheet: bool,
    glass_cache_top_px: u32,
    glass_cache_bot_px: u32,
    glass_cache_top_x_spans_px: [(u32, u32); 2],
    glass_cache_top_x_span_count: u8,
    glass_cache_bot_x_span_px: Option<(u32, u32)>,
    frame_stats: std::cell::Cell<RenderFrameStats>,
    glass_disp_maps: IndexLru<(u16, u16, u16), (ID2D1Bitmap1, u32, u32)>,
    glass_anim_states: std::cell::RefCell<HashMap<u64, GlassAnimState>>,
    last_glass_surface_rects: std::cell::RefCell<Vec<RectF>>,
    frame_counter: u64,

    // Bounded O(1) LRU offscreen premultiplied bitmaps for true GPU Gaussian depth-of-field lyrics blur
    lyrics_bmp_track: String,
    lyrics_bmp_cache: IndexLru<LyricsLineBmpKey, CachedLyricsLineBmp>,

    // Reusable 2-slot active-line glyph bloom bitmaps + 320x320 rotating album-art mesh bitmaps + playlist diff row pool
    pub active_glow_slots: [ActiveGlowSlot; 2],
    pub row_fx_slots: Vec<RowFxSlot>,
    lyrics_mesh_bmp: Option<ID2D1Bitmap1>,
    lyrics_mesh_out_bmp: Option<ID2D1Bitmap1>,
    mesh_last_bake_at: Option<std::time::Instant>,
    mesh_last_art_stem: String,
    mesh_last_free_blur: bool,
    mesh_aurora_bake_seq: u64,
    fade_mask_key: Option<(i32, i32, i32, i32, i32, i32)>,
    fade_mask_brush: Option<ID2D1LinearGradientBrush>,
    motion_textures: HashMap<String, MotionTextureEntry>,
    motion_lru_clock: u64,

    // Reusable bitmap + ring-buffer cursor for Spek waterfall (O(1) single-column GPU uploads!)
    pub waterfall_bmp: Option<ID2D1Bitmap1>,
    pub waterfall_pixels: Vec<u8>,
    pub waterfall_col_buf: Vec<u8>,
    pub waterfall_w: u32,
    pub waterfall_h: u32,
    pub waterfall_head_col: u32,
    pub waterfall_scroll_acc: f32,
    pub waterfall_display_max_hz: f32,

    font_cache: HashMap<FontKey, IDWriteTextFormat>,
    ellipsis_cache: HashMap<FontKey, IDWriteInlineObject>,
    text_layout_cache: IndexLru<u64, IDWriteTextLayout>,
    lyrics_unit_layout_cache: IndexLru<u64, (IDWriteTextLayout, f32, f32, f32)>,

    thumb_cache: HashMap<String, CachedArtwork>,
    hero_cache: HashMap<String, CachedArtwork>,
    art_lru_clock: Cell<u64>,
    seen_art_stems: std::collections::HashSet<String>,
    last_art_fade_at: Option<std::time::Instant>,

    pub width_px: u32,
    pub height_px: u32,
    pub dpi_scale: f32,
    offscreen_base_scale: f32,
    pub lyrics_blur_downsample: bool,
    lyrics_blur_downsample_explicit_off: bool,
    pub aggressive_memory_release: bool,
    lyrics_trans_bg_96: Option<ID2D1Bitmap1>,
    lyrics_trans_fg_bmp: Option<ID2D1Bitmap1>,
    lyrics_trans_half_bg: Option<ID2D1Bitmap1>,
    lyrics_trans_half_fg: Option<ID2D1Bitmap1>,
    lyrics_trans_w: u32,
    lyrics_trans_h: u32,
    lyrics_trans_half_w: u32,
    lyrics_trans_half_h: u32,
    in_lyrics_transition_sheet: bool,
    pub pending_capture_path: Option<std::path::PathBuf>,
    backbuffer_rotated_after_present: std::cell::Cell<bool>,
    dirty_state: std::cell::RefCell<DirtyPresentationState>,
}

#[inline]
fn pt2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

pub fn to_d2d_color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

pub fn to_d2d_rect(r: RectF) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

#[inline]
fn mat5x4_bytes(m: [f32; 20]) -> [u8; 80] {
    let mut b = [0u8; 80];
    for (i, f) in m.iter().enumerate() {
        b[i * 4..(i + 1) * 4].copy_from_slice(&f.to_le_bytes());
    }
    b
}

#[inline]
fn mat3x2_bytes(m: Matrix3x2) -> [u8; 24] {
    let vals = [m.M11, m.M12, m.M21, m.M22, m.M31, m.M32];
    let mut b = [0u8; 24];
    for (i, f) in vals.iter().enumerate() {
        b[i * 4..(i + 1) * 4].copy_from_slice(&f.to_le_bytes());
    }
    b
}

#[inline]
fn vec4_bytes(v: [f32; 4]) -> [u8; 16] {
    let mut b = [0u8; 16];
    for (i, f) in v.iter().enumerate() {
        b[i * 4..(i + 1) * 4].copy_from_slice(&f.to_le_bytes());
    }
    b
}

/// Converts a normalized `f32` in `[0.0, 1.0]` to an IEEE-754 binary16 half-float (`u16`)
/// for `DXGI_FORMAT_R16G16B16A16_FLOAT` displacement maps (exact `0.5 -> 0x3800`, `1.0 -> 0x3C00`).
#[inline]
fn f32_to_f16_bits(x: f32) -> u16 {
    let v = x.clamp(0.0, 1.0);
    if v == 0.0 {
        return 0;
    }
    let bits = v.to_bits();
    let exp32 = ((bits >> 23) & 0xFF) as i32;
    let mant32 = bits & 0x007F_FFFF;
    if exp32 <= 112 {
        if exp32 < 102 {
            return 0;
        }
        let full_mant = 0x0080_0000u32 | mant32;
        let shift = (126 - exp32) as u32;
        let rounded = full_mant + (1u32 << (shift - 1));
        return (rounded >> shift) as u16;
    }
    let mut exp16 = exp32 - 127 + 15;
    let mant_round = mant32 + 0x1000;
    let mant16 = if mant_round >= 0x0080_0000 {
        exp16 += 1;
        0u32
    } else {
        (mant_round >> 13) & 0x03FF
    };
    (((exp16 as u16) & 0x1F) << 10) | (mant16 as u16)
}

/// Quantized stable animation key for a glass button/pill rectangle (8-DIP buckets so slight hover
/// scale inflation maps to the exact same elastic spring slot!).
#[inline]
fn glass_anim_key(r: RectF) -> u64 {
    let bx = (r.center_x() / 8.0).round() as i32 as u64 & 0xFFFF;
    let by = (r.center_y() / 8.0).round() as i32 as u64 & 0xFFFF;
    let bw = (r.w / 8.0).round() as i32 as u64 & 0xFFFF;
    let bh = (r.h / 8.0).round() as i32 as u64 & 0xFFFF;
    (bx << 48) | (by << 32) | (bw << 16) | bh
}

impl Drop for D2DRenderer {
    fn drop(&mut self) {
        self.unbind_glass_effect_inputs();
        self.motion_textures.clear();
        self.mf_dxgi_device_manager = None;
        unsafe {
            for slot in &mut self.glass_effect_pool {
                slot.disp_border.SetInput(0, None::<&ID2D1Image>, true);
                slot.disp_g.SetInput(1, None::<&ID2D1Image>, true);
            }
        }
        self.glass_effect_pool.clear();
        self.linear_grad_cache.borrow_mut().clear();
        self.radial_glow_cache.borrow_mut().clear();
        self.glass_shadow_cache.borrow_mut().clear();
        self.icon_geom_cache.borrow_mut().clear();
        self.glass_image_brush = None;
        self.glass_caustic_brush = None;
        for slot in &mut self.active_glow_slots {
            unsafe {
                for eff in [
                    &slot.outer_blur_effect,
                    &slot.inner_blur_effect,
                    &slot.outer_alpha_effect,
                    &slot.inner_alpha_effect,
                ] {
                    if let Some(ref e) = eff {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                }
            }
            slot.outer_blur_effect = None;
            slot.inner_blur_effect = None;
            slot.outer_alpha_effect = None;
            slot.inner_alpha_effect = None;
        }
        // Release all DirectWrite formats/layouts/collections referencing our in-memory font loader
        // before unregistering it from the shared DirectWrite factory!
        self.text_layout_cache.clear();
        self.lyrics_unit_layout_cache.clear();
        self.ellipsis_cache.clear();
        self.font_cache.clear();
        self.custom_font_collection = None;
        if let Some(loader) = self.in_memory_font_loader.take() {
            unsafe {
                let _ = self.dwrite.UnregisterFontFileLoader(&loader);
            }
        }
    }
}

impl D2DRenderer {
    pub fn new(hwnd: HWND, width_px: u32, height_px: u32, dpi_scale: f32) -> Result<Self, String> {
        unsafe {
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                    .map_err(|e| format!("D2D1CreateFactory 失败: {e}"))?;

            let feature_levels = [
                D3D_FEATURE_LEVEL_11_1,
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_10_0,
            ];
            // Enable BGRA + Video support (without SINGLETHREADED so MF DXVA worker threads can
            // safely share the D3D11 device under ID3D11Multithread protection), falling back
            // gracefully to BGRA-only hardware and then WARP if a GPU/driver refuses VIDEO_SUPPORT.
            let video_flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
            let bgra_flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;

            let mut d3d_device: Option<ID3D11Device> = None;
            let mut d3d_video_support = false;
            let hr_hw_vid = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                video_flags,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                None,
            );
            if hr_hw_vid.is_ok() && d3d_device.is_some() {
                d3d_video_support = true;
            } else {
                d3d_device = None;
                let hr_hw_bgra = D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    windows::Win32::Foundation::HMODULE::default(),
                    bgra_flags,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut d3d_device),
                    None,
                    None,
                );
                if hr_hw_bgra.is_err() || d3d_device.is_none() {
                    d3d_device = None;
                    D3D11CreateDevice(
                        None,
                        D3D_DRIVER_TYPE_WARP,
                        windows::Win32::Foundation::HMODULE::default(),
                        bgra_flags,
                        Some(&feature_levels),
                        D3D11_SDK_VERSION,
                        Some(&mut d3d_device),
                        None,
                        None,
                    )
                    .map_err(|e| format!("D3D11CreateDevice 失败: {e}"))?;
                }
            }
            let d3d_device = d3d_device.ok_or_else(|| "D3D11Device 为空".to_string())?;

            // Enable thread-safe immediate-context protection for shared MF DXVA video threads.
            if let Ok(mt) = d3d_device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            if let Ok(ctx) = d3d_device.GetImmediateContext() {
                if let Ok(mt) = ctx.cast::<ID3D11Multithread>() {
                    let _ = mt.SetMultithreadProtected(true);
                }
            }

            let mut mf_dxgi_device_manager: Option<IMFDXGIDeviceManager> = None;
            let mut mf_dxgi_reset_token: u32 = 0;
            if d3d_video_support {
                let mut mgr_opt: Option<IMFDXGIDeviceManager> = None;
                let mut token: u32 = 0;
                if MFCreateDXGIDeviceManager(&mut token, &mut mgr_opt).is_ok() {
                    if let Some(mgr) = mgr_opt {
                        if mgr.ResetDevice(&d3d_device, token).is_ok() {
                            mf_dxgi_device_manager = Some(mgr);
                            mf_dxgi_reset_token = token;
                        }
                    }
                }
            }

            let dxgi_device: IDXGIDevice = d3d_device
                .cast()
                .map_err(|e| format!("IDXGIDevice cast 失败: {e}"))?;
            // Eliminate DXGI's default 3-frame render-ahead queue -> 1-frame input/scroll latency!
            if let Ok(dxgi_dev1) = dxgi_device.cast::<IDXGIDevice1>() {
                let _ = dxgi_dev1.SetMaximumFrameLatency(2);
            }

            let d2d_device = d2d_factory
                .CreateDevice(&dxgi_device)
                .map_err(|e| format!("CreateDevice 失败: {e}"))?;
            let dc = d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .map_err(|e| format!("CreateDeviceContext 失败: {e}"))?;

            let adapter = dxgi_device
                .GetAdapter()
                .map_err(|e| format!("GetAdapter 失败: {e}"))?;
            let dxgi_factory: IDXGIFactory2 = adapter
                .GetParent()
                .map_err(|e| format!("GetParent IDXGIFactory2 失败: {e}"))?;

            let w = width_px.max(64);
            let h = height_px.max(64);

            // Try DirectComposition + Premultiplied Alpha SwapChain first for true per-pixel anti-aliased
            // custom window corner radius (0 px sharp right-angle up to 28 px smooth rounded rect!)
            let mut dcomp_device: Option<IDCompositionDevice> = None;
            let mut dcomp_target: Option<IDCompositionTarget> = None;
            let mut dcomp_visual: Option<IDCompositionVisual> = None;
            let mut premultiplied_swapchain = false;
            let mut swap_buffer_count = 3u32;
            let mut swap_chain_opt: Option<IDXGISwapChain1> = None;

            let desc_comp = DXGI_SWAP_CHAIN_DESC1 {
                Width: w,
                Height: h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 3,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };

            if let Ok(dc_dev) = DCompositionCreateDevice::<_, IDCompositionDevice>(&dxgi_device) {
                if let (Ok(sc), Ok(tgt), Ok(vis)) = (
                    dxgi_factory.CreateSwapChainForComposition(&d3d_device, &desc_comp, None),
                    dc_dev.CreateTargetForHwnd(hwnd, true),
                    dc_dev.CreateVisual(),
                ) {
                    if vis.SetContent(&sc).is_ok()
                        && tgt.SetRoot(&vis).is_ok()
                        && dc_dev.Commit().is_ok()
                    {
                        dcomp_device = Some(dc_dev);
                        dcomp_target = Some(tgt);
                        dcomp_visual = Some(vis);
                        swap_chain_opt = Some(sc);
                        premultiplied_swapchain = true;
                    }
                }
            }

            let swap_chain = if let Some(sc) = swap_chain_opt {
                sc
            } else {
                swap_buffer_count = 3;
                let desc_hwnd = DXGI_SWAP_CHAIN_DESC1 {
                    BufferCount: 3,
                    AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                    ..desc_comp
                };
                dxgi_factory
                    .CreateSwapChainForHwnd(&d3d_device, hwnd, &desc_hwnd, None, None)
                    .map_err(|e| format!("CreateSwapChainForHwnd 失败: {e}"))?
            };

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                .map_err(|e| format!("DWriteCreateFactory 失败: {e}"))?;
            let lyrics_text_params = dwrite.CreateRenderingParams().ok().and_then(|base| {
                dwrite
                    .CreateCustomRenderingParams(
                        base.GetGamma(),
                        base.GetEnhancedContrast(),
                        base.GetClearTypeLevel(),
                        base.GetPixelGeometry(),
                        DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                    )
                    .ok()
            });

            // Load bundled OFL Inter Variable font in-process via DirectWrite IDWriteFactory5
            // and merge it with Windows' SystemFontSet so Inter Latin/numerals + CJK/Emoji
            // fallback work seamlessly with zero temporary disk files!
            let mut custom_font_collection: Option<IDWriteFontCollection> = None;
            let mut system_font_fallback: Option<IDWriteFontFallback> = None;
            let mut in_memory_font_loader: Option<IDWriteInMemoryFontFileLoader> = None;
            let mut bundled_inter_family = String::from("Segoe UI Variable Display");
            let mut bundled_inter_loaded = false;

            if let Ok(factory3) = dwrite.cast::<IDWriteFactory3>() {
                system_font_fallback = factory3.GetSystemFontFallback().ok();
                if let Ok(factory5) = dwrite.cast::<IDWriteFactory5>() {
                    if let Ok(mem_loader) = factory5.CreateInMemoryFontFileLoader() {
                        if dwrite.RegisterFontFileLoader(&mem_loader).is_ok() {
                            let load_res = (|| -> windows::core::Result<(IDWriteFontCollection, String)> {
                                let font_file = mem_loader.CreateInMemoryFontFileReference(
                                    &dwrite,
                                    INTER_FONT_BYTES.as_ptr() as *const c_void,
                                    INTER_FONT_BYTES.len() as u32,
                                    None::<&windows::core::IUnknown>,
                                )?;
                                // Discover the exact DirectWrite WWS family name directly from an
                                // Inter-only font collection (InterVariable.ttf registers as
                                // "Inter Variable Text" under DirectWrite WWS with all 9 weights!).
                                let mut discovered_names: Vec<String> = Vec::new();
                                if let Ok(probe_builder) = factory5.CreateFontSetBuilder() {
                                    if probe_builder.AddFontFile(&font_file).is_ok() {
                                        if let Ok(inter_set) = probe_builder.CreateFontSet() {
                                            if let Ok(inter_col1) =
                                                factory3.CreateFontCollectionFromFontSet(&inter_set)
                                            {
                                                if let Ok(inter_col) =
                                                    inter_col1.cast::<IDWriteFontCollection>()
                                                {
                                                    let fam_cnt = inter_col.GetFontFamilyCount();
                                                    let en_us = HSTRING::from("en-us");
                                                    for fi in 0..fam_cnt {
                                                        if let Ok(ff) = inter_col.GetFontFamily(fi)
                                                        {
                                                            if let Ok(names) = ff.GetFamilyNames() {
                                                                let mut loc_idx = 0u32;
                                                                let mut found =
                                                                    windows::core::BOOL(0);
                                                                let idx = if names
                                                                    .FindLocaleName(
                                                                        &en_us,
                                                                        &mut loc_idx,
                                                                        &mut found,
                                                                    )
                                                                    .is_ok()
                                                                    && found.as_bool()
                                                                {
                                                                    loc_idx
                                                                } else {
                                                                    0
                                                                };
                                                                if let Ok(len) =
                                                                    names.GetStringLength(idx)
                                                                {
                                                                    if len > 0 {
                                                                        let mut buf = vec![
                                                                            0u16;
                                                                            (len as usize) + 1
                                                                        ];
                                                                        if names
                                                                            .GetString(
                                                                                idx, &mut buf,
                                                                            )
                                                                            .is_ok()
                                                                        {
                                                                            let s = String::from_utf16_lossy(
                                                                                &buf[..(len as usize)],
                                                                            );
                                                                            if !s.is_empty() {
                                                                                discovered_names
                                                                                    .push(s);
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                let builder = factory5.CreateFontSetBuilder()?;
                                builder.AddFontFile(&font_file)?;
                                if let Ok(sys_set) = factory3.GetSystemFontSet() {
                                    let _ = builder.AddFontSet(&sys_set);
                                }
                                let merged_set = builder.CreateFontSet()?;
                                let col1 = factory3.CreateFontCollectionFromFontSet(&merged_set)?;
                                let col: IDWriteFontCollection = col1.cast()?;
                                for cand in discovered_names
                                    .iter()
                                    .map(String::as_str)
                                    .chain([INTER_FONT_FAMILY, "Inter Variable", "Inter"])
                                {
                                    let mut idx = 0u32;
                                    let mut exists = windows::core::BOOL(0);
                                    let name_h = HSTRING::from(cand);
                                    if col.FindFamilyName(&name_h, &mut idx, &mut exists).is_ok()
                                        && exists.as_bool()
                                    {
                                        return Ok((col, cand.to_string()));
                                    }
                                }
                                Err(windows::core::Error::empty())
                            })();
                            match load_res {
                                Ok((col, fam)) => {
                                    custom_font_collection = Some(col);
                                    bundled_inter_family = fam;
                                    bundled_inter_loaded = true;
                                    in_memory_font_loader = Some(mem_loader);
                                }
                                Err(_) => {
                                    let _ = dwrite.UnregisterFontFileLoader(&mem_loader);
                                }
                            }
                        }
                    }
                }
            }

            // Tabular fixed-width numerals (`tnum = 1`) + unslashed zero (`zero = 0`) for timestamps/telemetry
            let mono_typography = dwrite.CreateTypography().ok().inspect(|typo| {
                let _ = typo.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES,
                    parameter: 1,
                });
                let _ = typo.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: DWRITE_FONT_FEATURE_TAG_SLASHED_ZERO,
                    parameter: 0,
                });
            });

            let _ = crate::am_agent::init_process_com_security();
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| format!("WICImagingFactory 失败: {e}"))?;

            let solid_brush = dc
                .CreateSolidColorBrush(&to_d2d_color(Theme::TEXT_PRIMARY), None)
                .map_err(|e| format!("CreateSolidColorBrush 失败: {e}"))?;

            let stroke_props = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 2.0,
                ..Default::default()
            };
            let round_stroke = ID2D1Factory::CreateStrokeStyle(&d2d_factory, &stroke_props, None)
                .map_err(|e| format!("CreateStrokeStyle 失败: {e}"))?;

            let blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let glass_crop_effect = dc.CreateEffect(&CLSID_D2D1Crop).ok();
            let glass_vibrancy_effect = dc.CreateEffect(&CLSID_D2D1ColorMatrix).ok();
            let glass_bot_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let glass_bot_vibrancy_effect = dc.CreateEffect(&CLSID_D2D1ColorMatrix).ok();
            let init_border_clamp = |eff: &ID2D1Effect| {
                let clamp_mode = (D2D1_BORDER_EDGE_MODE_CLAMP.0 as u32).to_le_bytes();
                let _ = eff.SetValue(
                    D2D1_BORDER_PROP_EDGE_MODE_X.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &clamp_mode,
                );
                let _ = eff.SetValue(
                    D2D1_BORDER_PROP_EDGE_MODE_Y.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &clamp_mode,
                );
            };
            let glass_border_effect = dc
                .CreateEffect(&CLSID_D2D1Border)
                .ok()
                .inspect(init_border_clamp);
            let glass_bot_border_effect = dc
                .CreateEffect(&CLSID_D2D1Border)
                .ok()
                .inspect(init_border_clamp);
            let glass_local_border_effect = dc
                .CreateEffect(&CLSID_D2D1Border)
                .ok()
                .inspect(init_border_clamp);
            let glass_disp_border_effect = dc
                .CreateEffect(&CLSID_D2D1Border)
                .ok()
                .inspect(init_border_clamp);
            let init_affine_linear = |eff: &ID2D1Effect| {
                let interp = (D2D1_2DAFFINETRANSFORM_INTERPOLATION_MODE_LINEAR.0 as u32).to_le_bytes();
                let bmode = (D2D1_BORDER_MODE_HARD.0 as u32).to_le_bytes();
                let _ = eff.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_INTERPOLATION_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &interp,
                );
                let _ = eff.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_BORDER_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &bmode,
                );
            };
            let glass_top_downscale_effect = dc
                .CreateEffect(&CLSID_D2D12DAffineTransform)
                .ok()
                .inspect(init_affine_linear);
            let glass_bot_downscale_effect = dc
                .CreateEffect(&CLSID_D2D12DAffineTransform)
                .ok()
                .inspect(init_affine_linear);
            let glass_local_downscale_effect = dc
                .CreateEffect(&CLSID_D2D12DAffineTransform)
                .ok()
                .inspect(init_affine_linear);
            let glass_bg_scale_effect = dc
                .CreateEffect(&CLSID_D2D12DAffineTransform)
                .ok()
                .inspect(init_affine_linear);
            let glass_bot_bg_scale_effect = dc
                .CreateEffect(&CLSID_D2D12DAffineTransform)
                .ok()
                .inspect(init_affine_linear);
            let glass_local_scale_effect = dc
                .CreateEffect(&CLSID_D2D12DAffineTransform)
                .ok()
                .inspect(init_affine_linear);
            let glass_disp_xform_effect = dc
                .CreateEffect(&CLSID_D2D12DAffineTransform)
                .ok()
                .inspect(init_affine_linear);
            let glass_disp_g_effect = dc.CreateEffect(&CLSID_D2D1DisplacementMap).ok().inspect(|eff| {
                let cx = (D2D1_CHANNEL_SELECTOR_R.0 as u32).to_le_bytes();
                let cy = (D2D1_CHANNEL_SELECTOR_G.0 as u32).to_le_bytes();
                let _ = eff.SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &cx,
                );
                let _ = eff.SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &cy,
                );
            });

            let line_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let line_blur_effect_2 = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let trans_bg_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let trans_fg_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let glow_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let glow_inner_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let glow_outer_alpha_effect = dc.CreateEffect(&CLSID_D2D1ColorMatrix).ok();
            let glow_inner_alpha_effect = dc.CreateEffect(&CLSID_D2D1ColorMatrix).ok();
            let make_glow_slot = || ActiveGlowSlot {
                outer_blur_effect: dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok(),
                inner_blur_effect: dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok(),
                outer_alpha_effect: dc.CreateEffect(&CLSID_D2D1ColorMatrix).ok(),
                inner_alpha_effect: dc.CreateEffect(&CLSID_D2D1ColorMatrix).ok(),
                ..ActiveGlowSlot::default()
            };
            let active_glow_slots = [make_glow_slot(), make_glow_slot()];
            let mesh_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let mesh_vibrancy_effect = dc.CreateEffect(&CLSID_D2D1ColorMatrix).ok();

            // Pre-cached iPadOS 26 strictly VERTICAL top-to-bottom specular rim brushes (ZERO per-frame COM alloc!)
            let vrim_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.00,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.44)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.18,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.14)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.50,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.06)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.82,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.14)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.00,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.34)),
                },
            ];
            let glass_vrim_brush = ID2D1RenderTarget::CreateGradientStopCollection(
                &dc,
                &vrim_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()
            .and_then(|coll| {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(0.0, 0.0),
                    endPoint: pt2(0.0, 64.0),
                };
                dc.CreateLinearGradientBrush(&props, None, &coll).ok()
            });

            let vrim_hover_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.00,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.58)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.18,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.20)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.50,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.09)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.82,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.20)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.00,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.46)),
                },
            ];
            let glass_vrim_hover_brush = ID2D1RenderTarget::CreateGradientStopCollection(
                &dc,
                &vrim_hover_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()
            .and_then(|coll| {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(0.0, 0.0),
                    endPoint: pt2(0.0, 64.0),
                };
                dc.CreateLinearGradientBrush(&props, None, &coll).ok()
            });

            let inner_gleam_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.00,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.22)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.15,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.0)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.85,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.0)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.00,
                    color: to_d2d_color(Color::rgba(244, 249, 255, 0.14)),
                },
            ];
            let glass_inner_gleam_brush = ID2D1RenderTarget::CreateGradientStopCollection(
                &dc,
                &inner_gleam_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()
            .and_then(|coll| {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(0.0, 0.0),
                    endPoint: pt2(0.0, 64.0),
                };
                dc.CreateLinearGradientBrush(&props, None, &coll).ok()
            });

            let feather_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.90 },
                },
                D2D1_GRADIENT_STOP {
                    position: 0.42,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.64 },
                },
                D2D1_GRADIENT_STOP {
                    position: 0.74,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.22 },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.0 },
                },
            ];
            let mesh_feather_brush = ID2D1RenderTarget::CreateGradientStopCollection(
                &dc,
                &feather_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()
            .and_then(|coll| {
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: pt2(0.0, 0.0),
                    gradientOriginOffset: pt2(0.0, 0.0),
                    radiusX: 1.0,
                    radiusY: 1.0,
                };
                dc.CreateRadialGradientBrush(&props, None, &coll).ok()
            });

            let cherenkov_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: to_d2d_color(Color::rgba(60, 225, 255, 1.0)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: to_d2d_color(Color::rgba(0, 110, 255, 0.0)),
                },
            ];
            let cherenkov_brush = ID2D1RenderTarget::CreateGradientStopCollection(
                &dc,
                &cherenkov_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()
            .and_then(|coll| {
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: pt2(0.0, 0.0),
                    gradientOriginOffset: pt2(0.0, 0.0),
                    radiusX: 1.0,
                    radiusY: 1.0,
                };
                dc.CreateRadialGradientBrush(&props, None, &coll).ok()
            });

            let caustic_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 1.0)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.48,
                    color: to_d2d_color(Color::rgba(240, 247, 255, 0.30)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.0)),
                },
            ];
            let glass_caustic_brush = ID2D1RenderTarget::CreateGradientStopCollection(
                &dc,
                &caustic_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()
            .and_then(|coll| {
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: pt2(0.0, 0.0),
                    gradientOriginOffset: pt2(0.0, 0.0),
                    radiusX: 1.0,
                    radiusY: 1.0,
                };
                dc.CreateRadialGradientBrush(&props, None, &coll).ok()
            });

            let rim_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.30)),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.48,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.09)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: to_d2d_color(Color::rgba(255, 255, 255, 0.20)),
                },
            ];
            let window_rim_brush = ID2D1RenderTarget::CreateGradientStopCollection(
                &dc,
                &rim_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()
            .and_then(|coll| {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(0.0, 0.0),
                    endPoint: pt2(1200.0, 800.0),
                };
                dc.CreateLinearGradientBrush(&props, None, &coll).ok()
            });

            let mut renderer = Self {
                d2d_factory,
                d3d_device,
                d3d_video_support,
                mf_dxgi_device_manager,
                mf_dxgi_reset_token,
                dxgi_device,
                d2d_device,
                gpu_staging_dirty: false,
                dc,
                swap_chain,
                dcomp_device,
                dcomp_target,
                dcomp_visual,
                premultiplied_swapchain,
                needs_dcomp_commit: false,
                swap_buffer_count,
                target_bitmap: None,
                dwrite,
                custom_font_collection,
                system_font_fallback,
                mono_typography,
                bundled_inter_family,
                bundled_inter_loaded,
                in_memory_font_loader,
                lyrics_text_params,
                lyrics_subpixel_mode: false,
                wic,
                solid_brush,
                round_stroke,
                blur_effect,
                glass_crop_effect,
                glass_vibrancy_effect,
                glass_top_downscale_effect,
                glass_bot_downscale_effect,
                glass_local_downscale_effect,
                glass_bot_blur_effect,
                glass_bot_vibrancy_effect,
                glass_border_effect,
                glass_bot_bg_scale_effect,
                glass_bot_border_effect,
                glass_local_scale_effect,
                glass_local_border_effect,
                glass_bg_scale_effect,
                glass_disp_xform_effect,
                glass_disp_border_effect,
                glass_disp_g_effect,
                line_blur_effect,
                line_blur_effect_2,
                line_blur_slot_cursor: Cell::new(0),
                dc_flush_seq: Cell::new(1),
                trans_bg_blur_effect,
                trans_fg_blur_effect,
                glow_blur_effect,
                glow_inner_blur_effect,
                glow_outer_alpha_effect,
                glow_inner_alpha_effect,
                mesh_blur_effect,
                mesh_vibrancy_effect,
                mesh_feather_brush,
                cherenkov_brush,
                glass_caustic_brush,
                glass_image_brush: None,
                window_rim_brush,
                glass_vrim_brush,
                glass_vrim_hover_brush,
                glass_inner_gleam_brush,
                linear_grad_cache: std::cell::RefCell::new(IndexLru::with_capacity(
                    LINEAR_GRAD_CACHE_CAP,
                )),
                radial_glow_cache: std::cell::RefCell::new(IndexLru::with_capacity(
                    RADIAL_GLOW_CACHE_CAP,
                )),
                glass_shadow_cache: std::cell::RefCell::new(IndexLru::with_capacity(
                    GLASS_SHADOW_CACHE_CAP,
                )),
                icon_geom_cache: std::cell::RefCell::new(HashMap::with_capacity(24)),
                corner_cutout_key: (0, 0, 0),
                corner_cutout_geom: None,
                glass_capture_bmp: None,
                glass_cap_w: 0,
                glass_cap_h: 0,
                glass_local_half_blurred: None,
                glass_local_half_w: 0,
                glass_local_half_h: 0,
                glass_local_fallback_pending_flush: Cell::new(false),
                glass_frame_snapshot: None,
                glass_snap_w: 0,
                glass_snap_h: 0,
                glass_blurred_snapshot: None,
                glass_half_w: 0,
                glass_half_h: 0,
                glass_top_snap: None,
                glass_top_snap_w: 0,
                glass_top_snap_h: 0,
                glass_top_blurred: None,
                glass_top_half_w: 0,
                glass_top_half_h: 0,
                glass_bot_snap: None,
                glass_bot_snap_w: 0,
                glass_bot_snap_h: 0,
                glass_bot_blurred: None,
                glass_bot_half_w: 0,
                glass_bot_half_h: 0,
                glass_strip_top_px: 0,
                glass_strip_bot_px: 0,
                glass_copy_top_px: 0,
                glass_copy_bot_px: 0,
                glass_snapshot_active: false,
                glass_snapshot_strip_mode: false,
                glass_snapshot_write_gen: 0,
                glass_batch_bounded_bg: None,
                glass_batch_bg_key: None,
                glass_batch_bounded_top: None,
                glass_batch_top_key: None,
                glass_batch_bounded_bot: None,
                glass_batch_bot_key: None,
                glass_effect_pool: Vec::with_capacity(GLASS_EFFECT_POOL_SIZE),
                glass_effect_cursor: 0,
                glass_effect_overflow_used: false,
                glass_top_solid_bg: None,
                glass_top_x_spans_px: [(0, 0); 2],
                glass_top_x_span_count: 0,
                glass_bot_x_span_px: None,
                glass_cache_valid_top: false,
                glass_cache_valid_bot: false,
                glass_cache_w_px: 0,
                glass_cache_h_px: 0,
                glass_cache_dpi_q: 0,
                glass_cache_lyrics_sheet: false,
                glass_cache_top_px: 0,
                glass_cache_bot_px: 0,
                glass_cache_top_x_spans_px: [(0, 0); 2],
                glass_cache_top_x_span_count: 0,
                glass_cache_bot_x_span_px: None,
                frame_stats: std::cell::Cell::new(RenderFrameStats::default()),
                glass_disp_maps: IndexLru::with_capacity(32),
                glass_anim_states: std::cell::RefCell::new(HashMap::new()),
                last_glass_surface_rects: std::cell::RefCell::new(Vec::with_capacity(24)),
                frame_counter: 0,
                lyrics_bmp_track: String::new(),
                lyrics_bmp_cache: IndexLru::with_capacity(44),
                active_glow_slots,
                row_fx_slots: Vec::with_capacity(24),
                lyrics_mesh_bmp: None,
                lyrics_mesh_out_bmp: None,
                mesh_last_bake_at: None,
                mesh_last_art_stem: String::new(),
                mesh_last_free_blur: false,
                mesh_aurora_bake_seq: 0,
                fade_mask_key: None,
                fade_mask_brush: None,
                motion_textures: HashMap::new(),
                motion_lru_clock: 0,
                waterfall_bmp: None,
                waterfall_pixels: Vec::new(),
                waterfall_col_buf: Vec::new(),
                waterfall_w: 0,
                waterfall_h: 0,
                waterfall_head_col: 0,
                waterfall_scroll_acc: 0.0,
                waterfall_display_max_hz: 24_000.0,
                font_cache: HashMap::new(),
                ellipsis_cache: HashMap::new(),
                text_layout_cache: IndexLru::with_capacity(1024),
                lyrics_unit_layout_cache: IndexLru::with_capacity(256),
                thumb_cache: HashMap::new(),
                hero_cache: HashMap::new(),
                art_lru_clock: Cell::new(0),
                seen_art_stems: std::collections::HashSet::new(),
                last_art_fade_at: None,
                width_px: w,
                height_px: h,
                dpi_scale: dpi_scale.max(1.0),
                offscreen_base_scale: 1.0,
                lyrics_blur_downsample: false,
                lyrics_blur_downsample_explicit_off: false,
                aggressive_memory_release: false,
                lyrics_trans_bg_96: None,
                lyrics_trans_fg_bmp: None,
                lyrics_trans_half_bg: None,
                lyrics_trans_half_fg: None,
                lyrics_trans_w: 0,
                lyrics_trans_h: 0,
                lyrics_trans_half_w: 0,
                lyrics_trans_half_h: 0,
                in_lyrics_transition_sheet: false,
                pending_capture_path: None,
                backbuffer_rotated_after_present: std::cell::Cell::new(false),
                dirty_state: std::cell::RefCell::new(DirtyPresentationState::new()),
            };
            renderer.recreate_backbuffer_target()?;
            Ok(renderer)
        }
    }

    pub fn mf_dxgi_device_manager(&self) -> Option<IMFDXGIDeviceManager> {
        self.mf_dxgi_device_manager.clone()
    }

    pub fn d3d_device(&self) -> Option<ID3D11Device> {
        Some(self.d3d_device.clone())
    }

    pub fn has_d3d_video_support(&self) -> bool {
        self.d3d_video_support
    }

    pub fn mf_dxgi_reset_token(&self) -> u32 {
        self.mf_dxgi_reset_token
    }

    pub fn motion_texture_cache_len(&self) -> usize {
        self.motion_textures.len()
    }

    pub fn full_invalidation_frames_remaining(&self) -> u8 {
        self.dirty_state.borrow().full_invalidation_frames
    }

    pub fn last_redraw_rects_px(&self) -> Vec<DirtyRectPx> {
        self.dirty_state.borrow().last_redraw_rects_px.clone()
    }

    #[inline]
    fn bump_stat(&self, f: impl FnOnce(&mut RenderFrameStats)) {
        let mut st = self.frame_stats.get();
        f(&mut st);
        self.frame_stats.set(st);
    }

    #[inline]
    fn reset_per_frame_glass_stats(&self) {
        let mut st = self.frame_stats.get();
        st.glass_push_layers = 0;
        st.glass_effect_rebinds = 0;
        st.glass_shadow_geom_allocs = 0;
        st.glass_snapshot_flushes = 0;
        st.glass_snapshot_blurs = 0;
        st.glass_solid_fastpath_draws = 0;
        st.glass_snapshot_cache_hits = 0;
        self.frame_stats.set(st);
    }

    fn invalidate_dirty_history_full(&self) {
        self.dirty_state.borrow_mut().invalidate_full();
    }

    fn suspend_dirty_clip(&self) {
        let mut ds = self.dirty_state.borrow_mut();
        if ds.active_dirty_clip_pushed {
            unsafe {
                self.dc.PopAxisAlignedClip();
            }
            ds.active_dirty_clip_pushed = false;
        }
    }

    fn resume_dirty_clip(&self) {
        let mut ds = self.dirty_state.borrow_mut();
        if !ds.active_dirty_clip_pushed {
            if let Some(clip_px) = ds.active_dirty_clip_px {
                unsafe {
                    let mut saved_xf = Matrix3x2::identity();
                    self.dc.GetTransform(&mut saved_xf);
                    let main_dpi = 96.0 * self.dpi_scale.max(1.0);
                    self.dc.SetDpi(96.0, 96.0);
                    self.dc.SetTransform(&Matrix3x2::identity());
                    let rc = D2D_RECT_F {
                        left: clip_px.left as f32,
                        top: clip_px.top as f32,
                        right: clip_px.right as f32,
                        bottom: clip_px.bottom as f32,
                    };
                    self.dc
                        .PushAxisAlignedClip(&rc, D2D1_ANTIALIAS_MODE_ALIASED);
                    self.dc.SetDpi(main_dpi, main_dpi);
                    self.dc.SetTransform(&saved_xf);
                }
                ds.active_dirty_clip_pushed = true;
            }
        }
    }

    pub fn frame_stats(&self) -> RenderFrameStats {
        self.frame_stats.get()
    }

    pub fn reset_frame_stats(&mut self) {
        self.frame_stats.set(RenderFrameStats::default());
    }

    pub fn lyrics_aurora_bake_seq(&self) -> u64 {
        self.mesh_aurora_bake_seq
    }

    pub fn glass_snapshot_write_gen(&self) -> u64 {
        self.glass_snapshot_write_gen
    }

    pub fn is_in_lyrics_transition_sheet(&self) -> bool {
        self.in_lyrics_transition_sheet
    }

    pub fn has_active_lyrics_glow_slot_baked(&self, slot: usize) -> bool {
        self.active_glow_slots
            .get(slot)
            .is_some_and(|s| s.baked_bitmap.is_some())
    }

    /// Returns total bytes currently allocated across glass backdrop snapshot textures (full or strip mode).
    pub fn glass_snapshot_allocated_bytes(&self) -> u64 {
        let mut total = 0u64;
        if self.glass_frame_snapshot.is_some() {
            total += (self.glass_snap_w as u64) * (self.glass_snap_h as u64) * 4;
        }
        if self.glass_blurred_snapshot.is_some() {
            total += (self.glass_half_w as u64) * (self.glass_half_h as u64) * 4;
        }
        if self.glass_top_snap.is_some() {
            total += (self.glass_top_snap_w as u64) * (self.glass_top_snap_h as u64) * 4;
        }
        if self.glass_top_blurred.is_some() {
            total += (self.glass_top_half_w as u64) * (self.glass_top_half_h as u64) * 4;
        }
        if self.glass_bot_snap.is_some() {
            total += (self.glass_bot_snap_w as u64) * (self.glass_bot_snap_h as u64) * 4;
        }
        if self.glass_bot_blurred.is_some() {
            total += (self.glass_bot_half_w as u64) * (self.glass_bot_half_h as u64) * 4;
        }
        total
    }

    pub fn glass_strip_snapshot_dims(&self) -> (u32, u32, u32, u32) {
        (
            self.glass_top_snap_w,
            self.glass_top_snap_h,
            self.glass_bot_snap_w,
            self.glass_bot_snap_h,
        )
    }

    pub fn text_layout_cache_len(&self) -> usize {
        self.text_layout_cache.len()
    }

    pub fn text_layout_cache_keys_by_recency(&self) -> Vec<u64> {
        self.text_layout_cache.keys_oldest_to_newest()
    }

    pub fn radial_glow_cache_len(&self) -> usize {
        self.radial_glow_cache.borrow().len()
    }

    pub fn linear_grad_cache_len(&self) -> usize {
        self.linear_grad_cache.borrow().len()
    }

    pub fn icon_geom_cache_len(&self) -> usize {
        self.icon_geom_cache.borrow().len()
    }

    pub fn lyrics_transition_blur_dims(&self) -> (u32, u32) {
        (self.lyrics_trans_half_w, self.lyrics_trans_half_h)
    }

    pub fn thumb_cache_keys_by_recency(&self) -> Vec<String> {
        let mut items: Vec<(&String, u64)> = self
            .thumb_cache
            .iter()
            .map(|(k, v)| (k, v.last_used.get()))
            .collect();
        items.sort_by_key(|&(_, ts)| ts);
        items.into_iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn hero_cache_keys_by_recency(&self) -> Vec<String> {
        let mut items: Vec<(&String, u64)> = self
            .hero_cache
            .iter()
            .map(|(k, v)| (k, v.last_used.get()))
            .collect();
        items.sort_by_key(|&(_, ts)| ts);
        items.into_iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn has_active_artwork_fade(&self) -> bool {
        self.has_fading_artwork()
    }

    #[inline]
    fn flush_dc(&self) {
        unsafe {
            let _ = self.dc.Flush(None, None);
        }
        self.dc_flush_seq.set(self.dc_flush_seq.get().wrapping_add(1));
    }

    fn unbind_glass_effect_inputs(&mut self) {
        self.glass_batch_bounded_bg = None;
        self.glass_batch_bg_key = None;
        self.glass_batch_bounded_top = None;
        self.glass_batch_top_key = None;
        self.glass_batch_bounded_bot = None;
        self.glass_batch_bot_key = None;
        self.glass_effect_cursor = 0;
        self.glass_effect_overflow_used = false;
        self.glass_local_fallback_pending_flush.set(false);
        unsafe {
            if let Some(ref b) = self.glass_image_brush {
                b.SetImage(None::<&ID2D1Image>);
            }
            for eff in [
                &self.blur_effect,
                &self.glass_crop_effect,
                &self.glass_vibrancy_effect,
                &self.glass_top_downscale_effect,
                &self.glass_bot_downscale_effect,
                &self.glass_local_downscale_effect,
                &self.glass_bot_blur_effect,
                &self.glass_bot_vibrancy_effect,
                &self.glass_border_effect,
                &self.glass_bot_bg_scale_effect,
                &self.glass_bot_border_effect,
                &self.glass_local_scale_effect,
                &self.glass_local_border_effect,
                &self.glass_bg_scale_effect,
                &self.glass_disp_xform_effect,
                &self.glass_disp_border_effect,
                &self.glass_disp_g_effect,
            ] {
                if let Some(ref e) = eff {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
            }
            if let Some(ref e) = self.glass_disp_g_effect {
                e.SetInput(1, None::<&ID2D1Image>, true);
            }
            for slot in &mut self.glass_effect_pool {
                slot.disp_xform.SetInput(0, None::<&ID2D1Image>, true);
                slot.disp_g.SetInput(0, None::<&ID2D1Image>, true);
                slot.bound_bg_ptr = 0;
                slot.bound_disp_ptr = 0;
                slot.state_valid = false;
            }
        }
    }

    fn invalidate_glass_snapshot_cache(&mut self) {
        self.glass_cache_valid_top = false;
        self.glass_cache_valid_bot = false;
        self.glass_cache_w_px = 0;
        self.glass_cache_h_px = 0;
        self.glass_cache_dpi_q = 0;
        self.glass_cache_top_px = 0;
        self.glass_cache_bot_px = 0;
        self.glass_cache_top_x_span_count = 0;
        self.glass_cache_bot_x_span_px = None;
        self.glass_batch_bounded_bg = None;
        self.glass_batch_bg_key = None;
        self.glass_batch_bounded_top = None;
        self.glass_batch_top_key = None;
        self.glass_batch_bounded_bot = None;
        self.glass_batch_bot_key = None;
        unsafe {
            for eff in [
                &self.glass_bg_scale_effect,
                &self.glass_border_effect,
                &self.glass_bot_bg_scale_effect,
                &self.glass_bot_border_effect,
            ] {
                if let Some(ref e) = eff {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
            }
            for slot in &mut self.glass_effect_pool {
                slot.disp_g.SetInput(0, None::<&ID2D1Image>, true);
                slot.bound_bg_ptr = 0;
                slot.state_valid = false;
            }
        }
    }

    fn recreate_backbuffer_target(&mut self) -> Result<(), String> {
        self.invalidate_glass_snapshot_cache();
        self.unbind_glass_effect_inputs();
        self.invalidate_dirty_history_full();
        self.backbuffer_rotated_after_present.set(false);
        unsafe {
            self.dc.SetTarget(None);
            self.target_bitmap = None;

            let surface: IDXGISurface = self
                .swap_chain
                .GetBuffer(0)
                .map_err(|e| format!("SwapChain GetBuffer(0) 失败: {e}"))?;

            let dpi = 96.0 * self.dpi_scale;
            let alpha_mode = if self.premultiplied_swapchain {
                D2D1_ALPHA_MODE_PREMULTIPLIED
            } else {
                D2D1_ALPHA_MODE_IGNORE
            };
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: alpha_mode,
                },
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };

            let bmp = self
                .dc
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))
                .map_err(|e| format!("CreateBitmapFromDxgiSurface 失败: {e}"))?;

            self.dc.SetTarget(&bmp);
            self.dc.SetDpi(dpi, dpi);
            self.dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.dc
                .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            self.target_bitmap = Some(bmp);
            Ok(())
        }
    }

    pub fn resize(&mut self, width_px: u32, height_px: u32, dpi_scale: f32) {
        let w = width_px.max(64);
        let h = height_px.max(64);
        let new_scale = dpi_scale.max(1.0);
        let dpi_changed = (new_scale - self.dpi_scale).abs() > 0.01;
        if dpi_changed {
            self.invalidate_glass_snapshot_cache();
            self.clear_lyrics_line_cache();
            self.text_layout_cache.clear();
            self.corner_cutout_geom = None;
            self.fade_mask_key = None;
            self.fade_mask_brush = None;
            self.glass_capture_bmp = None;
            self.glass_cap_w = 0;
            self.glass_cap_h = 0;
            self.glass_local_half_blurred = None;
            self.glass_local_half_w = 0;
            self.glass_local_half_h = 0;
            self.glass_disp_maps.clear();
            self.release_lyrics_transition_surfaces();
            for slot in &mut self.active_glow_slots {
                slot.bitmap = None;
                slot.baked_bitmap = None;
                slot.alloc_w = 0;
                slot.alloc_h = 0;
                slot.used_dip_w = 0.0;
                slot.used_dip_h = 0.0;
            }
        }
        self.dpi_scale = new_scale;
        self.offscreen_base_scale = 1.0;
        if w == self.width_px && h == self.height_px {
            if dpi_changed {
                let _ = self.recreate_backbuffer_target();
            } else {
                unsafe {
                    let dpi = 96.0 * self.dpi_scale;
                    self.dc.SetDpi(dpi, dpi);
                }
            }
            return;
        }
        self.width_px = w;
        self.height_px = h;
        self.corner_cutout_geom = None;
        self.glass_frame_snapshot = None;
        self.glass_snap_w = 0;
        self.glass_snap_h = 0;
        self.glass_blurred_snapshot = None;
        self.glass_half_w = 0;
        self.glass_half_h = 0;
        self.glass_top_snap = None;
        self.glass_top_snap_w = 0;
        self.glass_top_snap_h = 0;
        self.glass_top_blurred = None;
        self.glass_top_half_w = 0;
        self.glass_top_half_h = 0;
        self.glass_bot_snap = None;
        self.glass_bot_snap_w = 0;
        self.glass_bot_snap_h = 0;
        self.glass_bot_blurred = None;
        self.glass_bot_half_w = 0;
        self.glass_bot_half_h = 0;
        self.glass_local_half_blurred = None;
        self.glass_local_half_w = 0;
        self.glass_local_half_h = 0;
        self.glass_snapshot_active = false;
        self.glass_snapshot_strip_mode = false;
        self.invalidate_glass_snapshot_cache();
        self.unbind_glass_effect_inputs();
        self.release_lyrics_transition_surfaces();
        unsafe {
            self.dc.SetTarget(None);
            self.target_bitmap = None;
            for eff in [
                &self.line_blur_effect,
                &self.line_blur_effect_2,
                &self.trans_bg_blur_effect,
                &self.trans_fg_blur_effect,
                &self.glow_blur_effect,
                &self.glow_inner_blur_effect,
                &self.glow_outer_alpha_effect,
                &self.glow_inner_alpha_effect,
                &self.mesh_blur_effect,
                &self.mesh_vibrancy_effect,
            ] {
                if let Some(ref e) = eff {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
            }
            for slot in &self.active_glow_slots {
                for eff in [
                    &slot.outer_blur_effect,
                    &slot.inner_blur_effect,
                    &slot.outer_alpha_effect,
                    &slot.inner_alpha_effect,
                ] {
                    if let Some(ref e) = eff {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                }
            }
            let _ = self.swap_chain.ResizeBuffers(
                self.swap_buffer_count,
                w,
                h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                Default::default(),
            );
            if let Some(ref dcomp) = self.dcomp_device {
                let _ = dcomp.Commit();
            }
        }
        self.needs_dcomp_commit = true;
        let _ = self.recreate_backbuffer_target();
    }

    pub fn logical_width(&self) -> f32 {
        (self.width_px as f32) / self.dpi_scale
    }

    pub fn logical_height(&self) -> f32 {
        (self.height_px as f32) / self.dpi_scale
    }

    pub fn begin_draw(&self, clear_color: Color) {
        self.reset_per_frame_glass_stats();
        self.line_blur_slot_cursor.set(0);
        self.glass_local_fallback_pending_flush.set(false);
        self.backbuffer_rotated_after_present.set(false);
        self.last_glass_surface_rects.borrow_mut().clear();
        {
            let mut ds = self.dirty_state.borrow_mut();
            ds.invalidate_full();
            ds.last_redraw_rects_px.push(DirtyRectPx::new(
                0,
                0,
                self.width_px as i32,
                self.height_px as i32,
            ));
        }
        unsafe {
            self.dc.BeginDraw();
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(Some(&to_d2d_color(clear_color)));
        }
    }

    /// Starts a frame using `DirtyFramePlan` with 3-frame Flip-Model dirty-rect ring history
    /// (`D_T ∪ D_{T-1} ∪ D_{T-2}`) and automatic full-frame fallback (`force_full`, first 3 frames
    /// after init/resize/DPI change, or merged dirty area `> 55%`).
    ///
    /// Returns `false` ONLY on an idle skip (`full_invalidation_frames == 0`, `!plan.force_full`,
    /// and no non-empty clamped dirty rects): in that case `skipped_idle_frames` is incremented,
    /// `dc.BeginDraw()` is NOT called, and the caller MUST skip drawing and MUST NOT call
    /// `end_window_draw_and_present`.
    pub fn begin_draw_dirty(&mut self, clear_color: Color, plan: &DirtyFramePlan) -> bool {
        let d_t = quantize_and_clamp_dirty_rects(
            &plan.dirty_rects_dip,
            self.dpi_scale,
            self.width_px,
            self.height_px,
        );
        let full_px_rect = DirtyRectPx::new(0, 0, self.width_px as i32, self.height_px as i32);

        {
            let mut ds = self.dirty_state.borrow_mut();
            if ds.full_invalidation_frames == 0 && !plan.force_full && d_t.is_empty() {
                ds.pending_present_rects_px.clear();
                ds.last_redraw_rects_px.clear();
                ds.active_dirty_clip_px = None;
                ds.active_dirty_clip_pushed = false;
                drop(ds);
                self.bump_stat(|st| st.skipped_idle_frames += 1);
                return false;
            }
        }

        self.reset_per_frame_glass_stats();
        self.line_blur_slot_cursor.set(0);
        self.glass_local_fallback_pending_flush.set(false);
        self.backbuffer_rotated_after_present.set(false);
        self.last_glass_surface_rects.borrow_mut().clear();

        let dirty_clip_opt: Option<DirtyRectPx> = {
            let mut ds = self.dirty_state.borrow_mut();
            let dt_too_large =
                !d_t.is_empty() && dirty_area_exceeds_full_threshold(&d_t, self.width_px, self.height_px);

            if plan.force_full || dt_too_large {
                ds.full_invalidation_frames = (DIRTY_RECT_RING_DEPTH as u8).saturating_sub(1);
                ds.history_prev1.clear();
                ds.history_prev2.clear();
                ds.pending_present_rects_px.clear();
                ds.last_redraw_rects_px.clear();
                ds.last_redraw_rects_px.push(full_px_rect);
                ds.active_dirty_clip_px = None;
                ds.active_dirty_clip_pushed = false;
                None
            } else if ds.full_invalidation_frames > 0 {
                ds.full_invalidation_frames -= 1;
                ds.history_prev2 = std::mem::take(&mut ds.history_prev1);
                ds.history_prev1 = d_t;
                ds.pending_present_rects_px.clear();
                ds.last_redraw_rects_px.clear();
                ds.last_redraw_rects_px.push(full_px_rect);
                ds.active_dirty_clip_px = None;
                ds.active_dirty_clip_pushed = false;
                None
            } else {
                let redraw_rects = merge_dirty_rect_ring_union(
                    &d_t,
                    &ds.history_prev1,
                    &ds.history_prev2,
                    self.width_px,
                    self.height_px,
                );
                let redraw_bbox = dirty_rects_bounding_box(&redraw_rects);
                if redraw_bbox.is_none()
                    || dirty_area_exceeds_full_threshold(&redraw_rects, self.width_px, self.height_px)
                {
                    ds.full_invalidation_frames = (DIRTY_RECT_RING_DEPTH as u8).saturating_sub(1);
                    ds.history_prev1.clear();
                    ds.history_prev2.clear();
                    ds.pending_present_rects_px.clear();
                    ds.last_redraw_rects_px.clear();
                    ds.last_redraw_rects_px.push(full_px_rect);
                    ds.active_dirty_clip_px = None;
                    ds.active_dirty_clip_pushed = false;
                    None
                } else {
                    let bbox = redraw_bbox.unwrap();
                    ds.history_prev2 = std::mem::take(&mut ds.history_prev1);
                    ds.history_prev1 = d_t.clone();
                    ds.pending_present_rects_px = d_t;
                    ds.last_redraw_rects_px = redraw_rects;
                    ds.active_dirty_clip_px = Some(bbox);
                    ds.active_dirty_clip_pushed = true;
                    Some(bbox)
                }
            }
        };

        unsafe {
            self.dc.BeginDraw();
            if let Some(clip_px) = dirty_clip_opt {
                let main_dpi = 96.0 * self.dpi_scale.max(1.0);
                self.dc.SetDpi(96.0, 96.0);
                self.dc.SetTransform(&Matrix3x2::identity());
                let clip_rc = D2D_RECT_F {
                    left: clip_px.left as f32,
                    top: clip_px.top as f32,
                    right: clip_px.right as f32,
                    bottom: clip_px.bottom as f32,
                };
                self.dc
                    .PushAxisAlignedClip(&clip_rc, D2D1_ANTIALIAS_MODE_ALIASED);
                self.dc.Clear(Some(&to_d2d_color(clear_color)));
                self.dc.SetDpi(main_dpi, main_dpi);
                self.dc.SetTransform(&Matrix3x2::identity());
            } else {
                self.dc.SetTransform(&Matrix3x2::identity());
                self.dc.Clear(Some(&to_d2d_color(clear_color)));
            }
        }
        true
    }

    pub fn end_draw_and_present(&mut self, vsync: bool) {
        self.end_window_draw_and_present(0.0, true, if vsync { 1 } else { 0 });
    }

    /// Finishes the frame by applying sub-pixel anti-aliased rounded window corner masking
    /// (`D2D1_COMBINE_MODE_EXCLUDE` + `D2D1_PRIMITIVE_BLEND_COPY` -> zero `PushLayer` overhead!)
    /// and a crisp Liquid Glass refractive window border!
    pub fn end_window_draw_and_present(
        &mut self,
        corner_radius_dip: f32,
        is_maximized: bool,
        sync_interval: u32,
    ) {
        let lw = self.logical_width();
        let lh = self.logical_height();
        let rad = if is_maximized {
            0.0
        } else {
            corner_radius_dip.clamp(0.0, 36.0)
        };

        unsafe {
            self.dc.SetTransform(&Matrix3x2::identity());

            if !is_maximized {
                // 1. Sub-pixel anti-aliased exterior corner cutout into transparent desktop
                if self.premultiplied_swapchain && rad > 0.25 {
                    let r_key = (rad * 10.0).round() as u16;
                    let key = (self.width_px, self.height_px, r_key);
                    if self.corner_cutout_key != key || self.corner_cutout_geom.is_none() {
                        let outer_rc = D2D_RECT_F {
                            left: -4.0,
                            top: -4.0,
                            right: lw + 4.0,
                            bottom: lh + 4.0,
                        };
                        let inner_rr = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: 0.0,
                                top: 0.0,
                                right: lw,
                                bottom: lh,
                            },
                            radiusX: rad,
                            radiusY: rad,
                        };
                        if let (Ok(g_out), Ok(g_in), Ok(path)) = (
                            self.d2d_factory.CreateRectangleGeometry(&outer_rc),
                            self.d2d_factory.CreateRoundedRectangleGeometry(&inner_rr),
                            self.d2d_factory.CreatePathGeometry(),
                        ) {
                            if let Ok(sink) = path.Open() {
                                let g_in_base: ID2D1Geometry = g_in.cast().unwrap();
                                if g_out
                                    .CombineWithGeometry(
                                        &g_in_base,
                                        D2D1_COMBINE_MODE_EXCLUDE,
                                        None,
                                        0.20,
                                        &sink,
                                    )
                                    .is_ok()
                                    && sink.Close().is_ok()
                                {
                                    self.corner_cutout_key = key;
                                    self.corner_cutout_geom = Some(path.into());
                                }
                            }
                        }
                    }

                    if let Some(ref cutout) = self.corner_cutout_geom {
                        self.dc.SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_COPY);
                        self.dc.FillGeometry(
                            cutout,
                            self.brush(Color::rgba(0, 0, 0, 0.0)),
                            None,
                        );
                        self.dc.SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_SOURCE_OVER);
                    }
                }

                // 2. Crisp diagonal specular window frame border (`0px` sharp OR `rad` rounded — zero COM alloc!)
                let frame_r = RectF::new(0.55, 0.55, (lw - 1.1).max(1.0), (lh - 1.1).max(1.0));
                let rim_rad = (rad - 0.5).max(0.0);
                if let Some(ref grad) = self.window_rim_brush {
                    grad.SetEndPoint(pt2(lw, lh));
                    if rim_rad > 0.15 {
                        let rr = D2D1_ROUNDED_RECT {
                            rect: to_d2d_rect(frame_r),
                            radiusX: rim_rad,
                            radiusY: rim_rad,
                        };
                        self.dc
                            .DrawRoundedRectangle(&rr, grad, 1.1, &self.round_stroke);
                    } else {
                        self.dc
                            .DrawRectangle(&to_d2d_rect(frame_r), grad, 1.1, &self.round_stroke);
                    }
                }
            }

            // Pop any active R4 dirty-rect clip cleanly before finishing the Direct2D draw pass.
            self.suspend_dirty_clip();
            self.dirty_state.borrow_mut().active_dirty_clip_px = None;

            let hr_end = self.dc.EndDraw(None, None);
            self.dc_flush_seq.set(self.dc_flush_seq.get().wrapping_add(1));
            if hr_end.is_ok() {
                if let Some(cap_path) = self.pending_capture_path.take() {
                    let _ = self.save_backbuffer_to_png(&cap_path);
                }
            }
            self.glass_snapshot_active = false;
            self.glass_snapshot_strip_mode = false;
            self.glass_top_solid_bg = None;
            self.glass_effect_cursor = 0;
            self.glass_effect_overflow_used = false;
            self.glass_local_fallback_pending_flush.set(false);
            self.line_blur_slot_cursor.set(0);
            if let Some(ref b) = self.glass_image_brush {
                b.SetImage(None::<&ID2D1Image>);
            }
            self.frame_counter = self.frame_counter.wrapping_add(1);

            let corner_key = ((rad * 10.0).round() as u16, is_maximized);
            let mut present_rects = {
                let mut ds = self.dirty_state.borrow_mut();
                let corner_changed = ds
                    .last_corner_key
                    .map(|prev| prev != corner_key)
                    .unwrap_or(false);
                ds.last_corner_key = Some(corner_key);
                if corner_changed {
                    ds.invalidate_full();
                    Vec::new()
                } else {
                    std::mem::take(&mut ds.pending_present_rects_px)
                }
            };

            let clamped_sync = sync_interval.min(4);
            let hr_pres = if hr_end.is_ok() && !present_rects.is_empty() {
                let mut dxgi_rects: Vec<RECT> = present_rects
                    .drain(..)
                    .map(|r| RECT {
                        left: r.left,
                        top: r.top,
                        right: r.right,
                        bottom: r.bottom,
                    })
                    .collect();
                let params = DXGI_PRESENT_PARAMETERS {
                    DirtyRectsCount: dxgi_rects.len() as u32,
                    pDirtyRects: dxgi_rects.as_mut_ptr(),
                    pScrollRect: std::ptr::null_mut(),
                    pScrollOffset: std::ptr::null_mut(),
                };
                let hr1 = self
                    .swap_chain
                    .Present1(clamped_sync, DXGI_PRESENT(0), &params);
                if hr1.is_ok() {
                    self.bump_stat(|st| st.present1_dirty_frames += 1);
                    hr1
                } else {
                    crate::logger::warn(
                        "d2d",
                        &format!(
                            "Present1 dirty-rect error ({hr1:?}); falling back to full Present"
                        ),
                    );
                    self.invalidate_dirty_history_full();
                    let hr_fb = self.swap_chain.Present(clamped_sync, DXGI_PRESENT(0));
                    if hr_fb.is_ok() {
                        self.bump_stat(|st| st.present_full_frames += 1);
                    }
                    hr_fb
                }
            } else {
                let hr_full = self.swap_chain.Present(clamped_sync, DXGI_PRESENT(0));
                if hr_full.is_ok() {
                    self.bump_stat(|st| st.present_full_frames += 1);
                }
                hr_full
            };
            if hr_end.is_ok() && hr_pres.is_ok() {
                self.backbuffer_rotated_after_present.set(true);
            }
            if self.needs_dcomp_commit {
                self.needs_dcomp_commit = false;
                if let Some(ref dcomp) = self.dcomp_device {
                    let _ = dcomp.Commit();
                }
            }
            if hr_end.is_err() || hr_pres.is_err() {
                crate::logger::warn(
                    "d2d",
                    &format!(
                        "EndDraw/Present error (end={:?}, present={:?}); recreating backbuffer",
                        hr_end, hr_pres
                    ),
                );
                let _ = self.recreate_backbuffer_target();
            }
        }
    }

    /// Reads back the finished DXGI swapchain backbuffer via a D3D11 staging texture into an
    /// un-premultiplied 32-bit BGRA buffer `(width_px, height_px, bgra_bytes)`.
    pub fn readback_backbuffer_bgra(&self) -> Result<(u32, u32, Vec<u8>), String> {
        use windows::Win32::Graphics::Direct3D11::{
            ID3D11Resource, ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE,
            D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
        };

        unsafe {
            let buf_idx = if self.backbuffer_rotated_after_present.get() && self.swap_buffer_count > 1 {
                self.swap_buffer_count - 1
            } else {
                0
            };
            let backbuf: ID3D11Texture2D = self
                .swap_chain
                .GetBuffer(buf_idx)
                .map_err(|e| format!("GetBuffer({buf_idx}): {e}"))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            backbuf.GetDesc(&mut desc);
            let w = desc.Width;
            let h = desc.Height;
            if w == 0 || h == 0 {
                return Err("zero backbuffer size".to_string());
            }

            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;

            let d3d_dev: ID3D11Device = self
                .dxgi_device
                .cast()
                .map_err(|e| format!("dxgi->d3d11 cast: {e}"))?;
            let d3d_ctx = d3d_dev
                .GetImmediateContext()
                .map_err(|e| format!("GetImmediateContext: {e}"))?;
            let mut staging_opt: Option<ID3D11Texture2D> = None;
            d3d_dev
                .CreateTexture2D(&desc, None, Some(&mut staging_opt))
                .map_err(|e| format!("CreateTexture2D staging: {e}"))?;
            let staging = staging_opt.ok_or_else(|| "staging texture null".to_string())?;

            let dst_res: ID3D11Resource = staging.cast().map_err(|e| format!("dst cast: {e}"))?;
            let src_res: ID3D11Resource = backbuf.cast().map_err(|e| format!("src cast: {e}"))?;
            d3d_ctx.CopyResource(&dst_res, &src_res);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            d3d_ctx
                .Map(&dst_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| format!("Map staging: {e}"))?;

            let stride = (w as usize) * 4;
            let mut bgra = vec![0u8; stride * (h as usize)];
            let src_base = mapped.pData as *const u8;
            let row_pitch = mapped.RowPitch as usize;
            for y in 0..(h as usize) {
                let src_row = std::slice::from_raw_parts(src_base.add(y * row_pitch), stride);
                let dst_row = &mut bgra[y * stride..(y + 1) * stride];
                if self.premultiplied_swapchain {
                    for x in 0..(w as usize) {
                        let i = x * 4;
                        let a = src_row[i + 3];
                        if a == 255 {
                            dst_row[i] = src_row[i];
                            dst_row[i + 1] = src_row[i + 1];
                            dst_row[i + 2] = src_row[i + 2];
                            dst_row[i + 3] = 255;
                        } else if a == 0 {
                            dst_row[i] = 0;
                            dst_row[i + 1] = 0;
                            dst_row[i + 2] = 0;
                            dst_row[i + 3] = 0;
                        } else {
                            let a16 = a as u16;
                            dst_row[i] = (((src_row[i] as u16) * 255 + a16 / 2) / a16).min(255) as u8;
                            dst_row[i + 1] = (((src_row[i + 1] as u16) * 255 + a16 / 2) / a16).min(255) as u8;
                            dst_row[i + 2] = (((src_row[i + 2] as u16) * 255 + a16 / 2) / a16).min(255) as u8;
                            dst_row[i + 3] = a;
                        }
                    }
                } else {
                    dst_row.copy_from_slice(src_row);
                    for x in 0..(w as usize) {
                        dst_row[x * 4 + 3] = 255;
                    }
                }
            }
            d3d_ctx.Unmap(&dst_res, 0);
            Ok((w, h, bgra))
        }
    }

    /// Reads back the finished DXGI swapchain backbuffer via a D3D11 staging texture and writes a
    /// lossless 32-bit BGRA PNG (un-premultiplying alpha for crisp transparent rounded corners).
    pub fn save_backbuffer_to_png(&self, out_path: &std::path::Path) -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::GENERIC_WRITE;
        use windows::Win32::Graphics::Imaging::{GUID_ContainerFormatPng, WICBitmapEncoderNoCache};

        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let (w, h, bgra) = self.readback_backbuffer_bgra()?;
        let stride = (w as usize) * 4;
        unsafe {
            let stream = self
                .wic
                .CreateStream()
                .map_err(|e| format!("WIC CreateStream: {e}"))?;
            let wide: Vec<u16> = out_path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            stream
                .InitializeFromFilename(PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)
                .map_err(|e| format!("WIC InitializeFromFilename: {e}"))?;

            let encoder = self
                .wic
                .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
                .map_err(|e| format!("WIC CreateEncoder: {e}"))?;
            encoder
                .Initialize(&stream, WICBitmapEncoderNoCache)
                .map_err(|e| format!("WIC encoder.Initialize: {e}"))?;

            let mut frame_opt = None;
            let mut props_opt = None;
            encoder
                .CreateNewFrame(&mut frame_opt, &mut props_opt)
                .map_err(|e| format!("WIC CreateNewFrame: {e}"))?;
            let frame = frame_opt.ok_or_else(|| "WIC frame null".to_string())?;
            frame
                .Initialize(props_opt.as_ref())
                .map_err(|e| format!("WIC frame.Initialize: {e}"))?;
            frame
                .SetSize(w, h)
                .map_err(|e| format!("WIC frame.SetSize: {e}"))?;
            let mut pixel_fmt = GUID_WICPixelFormat32bppBGRA;
            frame
                .SetPixelFormat(&mut pixel_fmt)
                .map_err(|e| format!("WIC frame.SetPixelFormat: {e}"))?;
            frame
                .WritePixels(h, stride as u32, &bgra)
                .map_err(|e| format!("WIC WritePixels: {e}"))?;
            frame.Commit().map_err(|e| format!("WIC frame.Commit: {e}"))?;
            encoder
                .Commit()
                .map_err(|e| format!("WIC encoder.Commit: {e}"))?;
            Ok(())
        }
    }

    pub fn set_transform(&self, m: &Matrix3x2) {
        unsafe {
            let s = self.offscreen_base_scale;
            if (s - 1.0).abs() > 0.001 {
                let scaled = Matrix3x2 {
                    M11: m.M11 * s,
                    M12: m.M12 * s,
                    M21: m.M21 * s,
                    M22: m.M22 * s,
                    M31: m.M31 * s,
                    M32: m.M32 * s,
                };
                self.dc.SetTransform(&scaled);
            } else {
                self.dc.SetTransform(m);
            }
        }
    }

    pub fn reset_transform(&self) {
        unsafe {
            let s = self.offscreen_base_scale;
            if (s - 1.0).abs() > 0.001 {
                self.dc.SetTransform(&Matrix3x2 {
                    M11: s,
                    M12: 0.0,
                    M21: 0.0,
                    M22: s,
                    M31: 0.0,
                    M32: 0.0,
                });
            } else {
                self.dc.SetTransform(&Matrix3x2::identity());
            }
        }
    }

    pub fn push_clip(&self, r: RectF) {
        unsafe {
            self.dc
                .PushAxisAlignedClip(&to_d2d_rect(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
    }

    pub fn pop_clip(&self) {
        unsafe {
            self.dc.PopAxisAlignedClip();
        }
    }

    #[inline]
    fn brush(&self, color: Color) -> &ID2D1SolidColorBrush {
        unsafe {
            self.solid_brush.SetColor(&to_d2d_color(color));
        }
        &self.solid_brush
    }

    pub fn fill_rect(&self, r: RectF, color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 || color.a <= 0.001 {
            return;
        }
        unsafe {
            self.dc.FillRectangle(&to_d2d_rect(r), self.brush(color));
        }
    }

    pub fn fill_rounded_rect(&self, r: RectF, radius: f32, color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 || color.a <= 0.001 {
            return;
        }
        let rr = D2D1_ROUNDED_RECT {
            rect: to_d2d_rect(r),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.dc.FillRoundedRectangle(&rr, self.brush(color));
        }
    }

    pub fn stroke_rounded_rect(&self, r: RectF, radius: f32, color: Color, width: f32) {
        if r.w <= 0.0 || r.h <= 0.0 || color.a <= 0.001 {
            return;
        }
        let rr = D2D1_ROUNDED_RECT {
            rect: to_d2d_rect(r),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.dc
                .DrawRoundedRectangle(&rr, self.brush(color), width, &self.round_stroke);
        }
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, color: Color) {
        if radius <= 0.0 || color.a <= 0.001 {
            return;
        }
        let el = D2D1_ELLIPSE {
            point: pt2(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.dc.FillEllipse(&el, self.brush(color));
        }
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, radius: f32, color: Color, width: f32) {
        if radius <= 0.0 || color.a <= 0.001 {
            return;
        }
        let el = D2D1_ELLIPSE {
            point: pt2(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.dc
                .DrawEllipse(&el, self.brush(color), width, &self.round_stroke);
        }
    }

    pub fn draw_line(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color, width: f32) {
        if color.a <= 0.001 {
            return;
        }
        unsafe {
            self.dc.DrawLine(
                pt2(x0, y0),
                pt2(x1, y1),
                self.brush(color),
                width,
                &self.round_stroke,
            );
        }
    }

    pub fn draw_polyline(&self, pts: &[(f32, f32)], color: Color, width: f32, closed: bool) {
        if pts.len() < 2 || color.a <= 0.001 {
            return;
        }
        unsafe {
            let Ok(geom) = self.d2d_factory.CreatePathGeometry() else {
                return;
            };
            let Ok(sink) = geom.Open() else {
                return;
            };
            sink.BeginFigure(pt2(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_HOLLOW);
            for &(x, y) in &pts[1..] {
                sink.AddLine(pt2(x, y));
            }
            sink.EndFigure(if closed {
                D2D1_FIGURE_END_CLOSED
            } else {
                D2D1_FIGURE_END_OPEN
            });
            if sink.Close().is_ok() {
                self.dc
                    .DrawGeometry(&geom, self.brush(color), width, &self.round_stroke);
            }
        }
    }

    pub fn fill_polygon(&self, pts: &[(f32, f32)], color: Color) {
        if pts.len() < 3 || color.a <= 0.001 {
            return;
        }
        unsafe {
            let Ok(geom) = self.d2d_factory.CreatePathGeometry() else {
                return;
            };
            let Ok(sink) = geom.Open() else {
                return;
            };
            sink.BeginFigure(pt2(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_FILLED);
            for &(x, y) in &pts[1..] {
                sink.AddLine(pt2(x, y));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            if sink.Close().is_ok() {
                self.dc.FillGeometry(&geom, self.brush(color), None);
            }
        }
    }

    fn get_or_create_linear_grad_2stop(
        &self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        c0: Color,
        c1: Color,
    ) -> Option<ID2D1LinearGradientBrush> {
        let q_ch = |v: f32| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
        let key = (
            0u8,
            u64::from_le_bytes([
                q_ch(c0.r),
                q_ch(c0.g),
                q_ch(c0.b),
                q_ch(c0.a),
                q_ch(c1.r),
                q_ch(c1.g),
                q_ch(c1.b),
                q_ch(c1.a),
            ]),
        );
        unsafe {
            if let Some(brush) = self.linear_grad_cache.borrow_mut().get_touch(&key).cloned() {
                brush.SetStartPoint(pt2(x0, y0));
                brush.SetEndPoint(pt2(x1, y1));
                return Some(brush);
            }
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: to_d2d_color(c0),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: to_d2d_color(c1),
                },
            ];
            let coll = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.dc,
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()?;
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: pt2(x0, y0),
                endPoint: pt2(x1, y1),
            };
            let grad = self.dc.CreateLinearGradientBrush(&props, None, &coll).ok()?;
            self.linear_grad_cache
                .borrow_mut()
                .insert(key, grad.clone(), LINEAR_GRAD_CACHE_CAP);
            Some(grad)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn get_or_create_frontier_glow_grad(
        &self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        body_a: f32,
        peak_a: f32,
        front_a: f32,
        peak_pos: f32,
    ) -> Option<ID2D1LinearGradientBrush> {
        let qa = |a: f32| -> u8 { (a.clamp(0.0, 1.0) * 63.0).round() as u8 };
        let qp = (peak_pos.clamp(0.15, 0.90) * 31.0).round() as u8;
        let bq = qa(body_a);
        let pq = qa(peak_a);
        let fq = qa(front_a);
        let packed: u64 = ((bq as u64) << 24) | ((pq as u64) << 16) | ((fq as u64) << 8) | (qp as u64);
        let key = (1u8, packed);
        unsafe {
            if let Some(brush) = self.linear_grad_cache.borrow_mut().get_touch(&key).cloned() {
                brush.SetStartPoint(pt2(x0, y0));
                brush.SetEndPoint(pt2(x1, y1));
                return Some(brush);
            }
            let ba = (bq as f32) / 63.0;
            let pa = (pq as f32) / 63.0;
            let fa = (fq as f32) / 63.0;
            let mid_a = ba + (pa - ba) * 0.58;
            let t_peak = ((qp as f32) / 31.0).clamp(0.15, 0.90);
            let t_mid = (t_peak * 0.65).clamp(0.05, t_peak - 0.02);
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: ba },
                },
                D2D1_GRADIENT_STOP {
                    position: t_mid,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: mid_a },
                },
                D2D1_GRADIENT_STOP {
                    position: t_peak,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: pa },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: fa },
                },
            ];
            let coll = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.dc,
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
            .ok()?;
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: pt2(x0, y0),
                endPoint: pt2(x1, y1),
            };
            let grad = self.dc.CreateLinearGradientBrush(&props, None, &coll).ok()?;
            self.linear_grad_cache
                .borrow_mut()
                .insert(key, grad.clone(), LINEAR_GRAD_CACHE_CAP);
            Some(grad)
        }
    }

    pub fn fill_linear_gradient_rect(
        &self,
        r: RectF,
        radius: f32,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        c0: Color,
        c1: Color,
    ) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let Some(grad) = self.get_or_create_linear_grad_2stop(x0, y0, x1, y1, c0, c1) else {
            return;
        };
        unsafe {
            if radius > 0.1 {
                let rr = D2D1_ROUNDED_RECT {
                    rect: to_d2d_rect(r),
                    radiusX: radius,
                    radiusY: radius,
                };
                self.dc.FillRoundedRectangle(&rr, &grad);
            } else {
                self.dc.FillRectangle(&to_d2d_rect(r), &grad);
            }
        }
    }

    pub fn fill_radial_glow(&self, cx: f32, cy: f32, rx: f32, ry: f32, inner: Color, outer: Color) {
        if rx <= 0.5 || ry <= 0.5 || inner.a <= 0.001 {
            return;
        }
        let q_ch = |v: f32| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
        let key = u64::from_le_bytes([
            q_ch(inner.r),
            q_ch(inner.g),
            q_ch(inner.b),
            q_ch(inner.a),
            q_ch(outer.r),
            q_ch(outer.g),
            q_ch(outer.b),
            q_ch(outer.a),
        ]);
        unsafe {
            let cached_opt = self.radial_glow_cache.borrow_mut().get_touch(&key).cloned();
            let rad = if let Some(cached) = cached_opt {
                cached.SetCenter(pt2(cx, cy));
                cached.SetRadiusX(rx);
                cached.SetRadiusY(ry);
                cached
            } else {
                let stops = [
                    D2D1_GRADIENT_STOP {
                        position: 0.0,
                        color: to_d2d_color(inner),
                    },
                    D2D1_GRADIENT_STOP {
                        position: 1.0,
                        color: to_d2d_color(outer),
                    },
                ];
                let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                    &self.dc,
                    &stops,
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                ) else {
                    return;
                };
                let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: pt2(cx, cy),
                    gradientOriginOffset: pt2(0.0, 0.0),
                    radiusX: rx,
                    radiusY: ry,
                };
                let Ok(created) = self.dc.CreateRadialGradientBrush(&props, None, &coll) else {
                    return;
                };
                self.radial_glow_cache.borrow_mut().insert(
                    key,
                    created.clone(),
                    RADIAL_GLOW_CACHE_CAP,
                );
                created
            };
            let el = D2D1_ELLIPSE {
                point: pt2(cx, cy),
                radiusX: rx,
                radiusY: ry,
            };
            self.dc.FillEllipse(&el, &rad);
        }
    }

    /// Zero-allocation GPU isometric prism + Cherenkov glow renderer for RBMK-1000 reactor channels.
    /// Uses Direct2D affine `Matrix3x2` transforms over unit `[0,0,1,1]` rectangles + a cached unit radial brush,
    /// eliminating ~840 COM geometry/brush allocations per frame so `反应堆芯` runs at 165–240 Hz!
    #[allow(clippy::too_many_arguments)]
    pub fn draw_iso_reactor_rod(
        &self,
        iso_x: f32,
        iso_y_base: f32,
        lift: f32,
        half_tw: f32,
        half_th: f32,
        wall_h: f32,
        glow_alpha: f32,
        left_col: Color,
        right_col: Color,
        cap_col: Color,
        rim_col: Color,
    ) {
        let iso_y = iso_y_base - lift;
        let unit_rc = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        };
        unsafe {
            // 1. Open shaft Cherenkov radiation glow (reuses cached radial brush with brush transform!)
            if glow_alpha > 0.01 {
                if let Some(ref cb) = self.cherenkov_brush {
                    let gx = half_tw * 1.35;
                    let gy = half_th * 1.35;
                    cb.SetOpacity(glow_alpha.clamp(0.0, 1.0));
                    cb.SetTransform(&Matrix3x2 {
                        M11: gx,
                        M12: 0.0,
                        M21: 0.0,
                        M22: gy,
                        M31: iso_x,
                        M32: iso_y_base,
                    });
                    let el = D2D1_ELLIPSE {
                        point: pt2(iso_x, iso_y_base),
                        radiusX: gx,
                        radiusY: gy,
                    };
                    self.dc.FillEllipse(&el, cb);
                }
            }

            // 2. Left 3D extruded wall: (0,0)->left, (1,0)->bottom, (1,1)->bottom_low, (0,1)->left_low
            self.dc.SetTransform(&Matrix3x2 {
                M11: half_tw,
                M12: half_th,
                M21: 0.0,
                M22: wall_h,
                M31: iso_x - half_tw,
                M32: iso_y,
            });
            self.dc.FillRectangle(&unit_rc, self.brush(left_col));

            // 3. Right 3D extruded wall: (0,0)->bottom, (1,0)->right, (1,1)->right_low, (0,1)->bottom_low
            self.dc.SetTransform(&Matrix3x2 {
                M11: half_tw,
                M12: -half_th,
                M21: 0.0,
                M22: wall_h,
                M31: iso_x,
                M32: iso_y + half_th,
            });
            self.dc.FillRectangle(&unit_rc, self.brush(right_col));

            // 4. Top steel rhombus cap + specular rim: (0,0)->top, (1,0)->right, (0,1)->left, (1,1)->bottom
            self.dc.SetTransform(&Matrix3x2 {
                M11: half_tw,
                M12: half_th,
                M21: -half_tw,
                M22: half_th,
                M31: iso_x,
                M32: iso_y - half_th,
            });
            self.dc.FillRectangle(&unit_rc, self.brush(cap_col));
            let rim_unit_w = (0.85 / half_tw.hypot(half_th).max(1.0)).clamp(0.02, 0.12);
            self.dc
                .DrawRectangle(&unit_rc, self.brush(rim_col), rim_unit_w, &self.round_stroke);

            // 5. Restore identity & draw center stainless plug socket
            self.dc.SetTransform(&Matrix3x2::identity());
            let plug = D2D1_ELLIPSE {
                point: pt2(iso_x, iso_y),
                radiusX: half_th * 0.24,
                radiusY: half_th * 0.24,
            };
            self.dc
                .FillEllipse(&plug, self.brush(Color::rgba(16, 20, 26, 0.65)));
        }
    }

    /// Zero-allocation GPU isometric 3D luminescent cuboid renderer for the default Reactor Matrix Core
    /// (`media_1794910079586_825fccbd.jpg` — crisp dark grid grooves, warm-lit left face, deep-shaded right face,
    /// glowing violet/magenta/peach/gold top bevel!).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_iso_matrix_cuboid(
        &self,
        iso_x: f32,
        iso_y_base: f32,
        height: f32,
        half_tw: f32,
        half_th: f32,
        left_col: Color,
        right_col: Color,
        top_col: Color,
        bevel_col: Color,
    ) {
        let iso_y = iso_y_base - height;
        let wall_h = height + 3.2;
        let unit_rc = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        };
        unsafe {
            // 1. Left extruded vertical wall
            self.dc.SetTransform(&Matrix3x2 {
                M11: half_tw,
                M12: half_th,
                M21: 0.0,
                M22: wall_h,
                M31: iso_x - half_tw,
                M32: iso_y,
            });
            self.dc.FillRectangle(&unit_rc, self.brush(left_col));

            // 2. Right extruded vertical wall
            self.dc.SetTransform(&Matrix3x2 {
                M11: half_tw,
                M12: -half_th,
                M21: 0.0,
                M22: wall_h,
                M31: iso_x,
                M32: iso_y + half_th,
            });
            self.dc.FillRectangle(&unit_rc, self.brush(right_col));

            // 3. Top rhombus cap + luminous inner bevel outline
            self.dc.SetTransform(&Matrix3x2 {
                M11: half_tw,
                M12: half_th,
                M21: -half_tw,
                M22: half_th,
                M31: iso_x,
                M32: iso_y - half_th,
            });
            self.dc.FillRectangle(&unit_rc, self.brush(top_col));
            if bevel_col.a > 0.08 {
                let inset_rc = D2D_RECT_F {
                    left: 0.04,
                    top: 0.04,
                    right: 0.96,
                    bottom: 0.96,
                };
                let rim_w = (0.95 / half_tw.hypot(half_th).max(1.0)).clamp(0.03, 0.14);
                self.dc
                    .DrawRectangle(&inset_rc, self.brush(bevel_col), rim_w, None);
            }
            self.dc.SetTransform(&Matrix3x2::identity());
        }
    }

    /// Builds a Direct2D 5×4 Rec.709 color vibrancy matrix (saturation boost + gentle contrast + lift)
    /// matching Apple iOS 26 / macOS Tahoe Liquid Glass optical saturation so colors under frosted glass
    /// glow richly instead of washing out to muddy gray.
    #[inline]
    pub fn build_glass_vibrancy_matrix(saturation: f32, contrast: f32, lift: f32) -> [f32; 20] {
        let s = saturation.clamp(0.5, 2.2);
        let c = contrast.clamp(0.8, 1.4);
        let l = lift.clamp(-0.2, 0.25);
        let inv_s = 1.0 - s;
        let sr = inv_s * 0.2126 * c;
        let sg = inv_s * 0.7152 * c;
        let sb = inv_s * 0.0722 * c;
        let sc = s * c;
        [
            sr + sc, sr,      sr,      0.0,
            sg,      sg + sc, sg,      0.0,
            sb,      sb,      sb + sc, 0.0,
            0.0,     0.0,     0.0,     1.0,
            l,       l,       l,       0.0,
        ]
    }

    /// Steps all active Liquid Glass hover/press elastic springs & cursor caustic tracking.
    /// Call once per frame before drawing; settles cleanly to exact targets for 0% idle CPU.
    pub fn step_glass_animations(&mut self, dt: f32) {
        let dt = dt.clamp(0.001, 0.05);
        let a_h = 1.0 - (-18.0 * dt).exp();
        let a_p = 1.0 - (-26.0 * dt).exp();
        let a_c = 1.0 - (-22.0 * dt).exp();
        let cur_frame = self.frame_counter;
        let mut map = self.glass_anim_states.borrow_mut();
        map.retain(|_, st| cur_frame.saturating_sub(st.last_frame) <= 180);
        for st in map.values_mut() {
            // If a button wasn't drawn in recent frames, relax smoothly back to idle
            if cur_frame.saturating_sub(st.last_frame) > 2 {
                st.hover_target = 0.0;
                st.press_target = 0.0;
                st.scale_target = 1.0;
            }
            st.hover_t += (st.hover_target - st.hover_t) * a_h;
            if (st.hover_target - st.hover_t).abs() < 0.001 {
                st.hover_t = st.hover_target;
            }
            st.press_t += (st.press_target - st.press_t) * a_p;
            if (st.press_target - st.press_t).abs() < 0.001 {
                st.press_t = st.press_target;
            }
            // Crisp underdamped elastic spring (omega = 24 rad/s, zeta = 0.72) for tactile Apple bounce
            let acc = (st.scale_target - st.scale) * 576.0 - st.scale_vel * 34.56;
            st.scale_vel += acc * dt;
            st.scale += st.scale_vel * dt;
            if (st.scale_target - st.scale).abs() < 0.0008 && st.scale_vel.abs() < 0.002 {
                st.scale = st.scale_target;
                st.scale_vel = 0.0;
            }
            st.caustic_x += (st.caustic_target_x - st.caustic_x) * a_c;
            st.caustic_y += (st.caustic_target_y - st.caustic_y) * a_c;
            if (st.caustic_target_x - st.caustic_x).hypot(st.caustic_target_y - st.caustic_y) < 0.25 {
                st.caustic_x = st.caustic_target_x;
                st.caustic_y = st.caustic_target_y;
            }
        }
    }

    /// Returns `true` while any Liquid Glass hover/press spring is actively moving; returns `false`
    /// as soon as springs settle so the app drops immediately to 0% idle CPU.
    pub fn has_active_glass_motion(&self) -> bool {
        let cur_frame = self.frame_counter;
        let map = self.glass_anim_states.borrow();
        map.values().any(|st| {
            cur_frame.saturating_sub(st.last_frame) <= 4
                && ((st.hover_t - st.hover_target).abs() > 0.001
                    || (st.press_t - st.press_target).abs() > 0.001
                    || (st.scale - st.scale_target).abs() > 0.0008
                    || st.scale_vel.abs() > 0.002)
        })
    }

    /// Updates targets and returns the current animated `(scale, hover_t, press_t, (caustic_x, caustic_y))`
    /// for a Liquid Glass interactive surface.
    fn sample_glass_anim(
        &self,
        rect: RectF,
        hovered: bool,
        pressed: bool,
        mouse_pos: (f32, f32),
    ) -> (f32, f32, f32, (f32, f32)) {
        let key = glass_anim_key(rect);
        let mut map = self.glass_anim_states.borrow_mut();
        if map.len() > 64 {
            let cur_f = self.frame_counter;
            map.retain(|_, v| cur_f.saturating_sub(v.last_frame) <= 60);
        }
        let st = map.entry(key).or_insert(GlassAnimState {
            hover_t: 0.0,
            hover_target: 0.0,
            press_t: 0.0,
            press_target: 0.0,
            scale: 1.0,
            scale_vel: 0.0,
            scale_target: 1.0,
            caustic_x: mouse_pos.0,
            caustic_y: mouse_pos.1,
            caustic_target_x: mouse_pos.0,
            caustic_target_y: mouse_pos.1,
            last_frame: self.frame_counter,
        });
        st.hover_target = if hovered { 1.0 } else { 0.0 };
        st.press_target = if pressed && hovered { 1.0 } else { 0.0 };
        st.scale_target = if pressed && hovered {
            0.945
        } else if hovered {
            1.045
        } else {
            1.0
        };
        st.caustic_target_x = mouse_pos.0;
        st.caustic_target_y = mouse_pos.1;
        st.last_frame = self.frame_counter;
        (st.scale, st.hover_t, st.press_t, (st.caustic_x, st.caustic_y))
    }

    #[inline]
    fn record_live_glass_rect(&self, r: RectF) {
        let mut rects = self.last_glass_surface_rects.borrow_mut();
        if rects.len() < 48 {
            rects.push(r);
        }
    }

    #[inline]
    fn is_point_on_live_glass(&self, px: f32, py: f32) -> bool {
        let rects = self.last_glass_surface_rects.borrow();
        rects.iter().any(|r| r.contains(px, py))
    }

    /// Draws authentic iPadOS 26 strictly VERTICAL top-to-bottom specular rim highlights
    /// (`top 0.44 -> sides 0.06 -> bottom 0.34`, brightening smoothly on hover to `0.58 / 0.46`)
    /// plus a razor-thin inner top gleam & subtle bottom internal bounce with ZERO per-frame COM alloc!
    fn draw_vertical_glass_rim(
        &self,
        r: RectF,
        rad: f32,
        hover_t: f32,
        opacity_mul: f32,
        dark_sep_alpha: f32,
    ) {
        let op = opacity_mul.clamp(0.0, 1.5);
        let ht = hover_t.clamp(0.0, 1.0);
        // Subtle outer hairline dark separation keeps crystal capsules distinct over white covers
        if dark_sep_alpha > 0.01 {
            self.stroke_rounded_rect(
                r.inset(-0.45, -0.45),
                (rad + 0.45).min(r.h * 0.5 + 0.45),
                Color::rgba(0, 0, 0, dark_sep_alpha),
                0.85,
            );
        }
        unsafe {
            let cx = r.center_x();
            let top_pt = pt2(cx, r.y);
            let bot_pt = pt2(cx, r.bottom());
            let rr = D2D1_ROUNDED_RECT {
                rect: to_d2d_rect(r),
                radiusX: rad,
                radiusY: rad,
            };
            let stroke_w = 1.02 + ht * 0.14;
            if let Some(ref base_rim) = self.glass_vrim_brush {
                base_rim.SetStartPoint(top_pt);
                base_rim.SetEndPoint(bot_pt);
                base_rim.SetOpacity(((1.0 - ht) * op).clamp(0.0, 1.0));
                if (1.0 - ht) * op > 0.01 {
                    self.dc
                        .DrawRoundedRectangle(&rr, base_rim, stroke_w, &self.round_stroke);
                }
            }
            if ht > 0.01 {
                if let Some(ref hov_rim) = self.glass_vrim_hover_brush {
                    hov_rim.SetStartPoint(top_pt);
                    hov_rim.SetEndPoint(bot_pt);
                    hov_rim.SetOpacity((ht * op).clamp(0.0, 1.0));
                    self.dc
                        .DrawRoundedRectangle(&rr, hov_rim, stroke_w, &self.round_stroke);
                }
            }
            // Razor-thin inner top gleam + subtle bottom internal reflection (zero heavy white bar!)
            let ir = r.inset(1.0, 1.0);
            if ir.w > 8.0 && ir.h > 8.0 {
                if let Some(ref gleam) = self.glass_inner_gleam_brush {
                    gleam.SetStartPoint(pt2(ir.center_x(), ir.y));
                    gleam.SetEndPoint(pt2(ir.center_x(), ir.bottom()));
                    gleam.SetOpacity(((0.85 + ht * 0.25) * op).clamp(0.0, 1.0));
                    let irad = (rad - 1.0).max(0.0);
                    let irr = D2D1_ROUNDED_RECT {
                        rect: to_d2d_rect(ir),
                        radiusX: irad,
                        radiusY: irad,
                    };
                    self.dc
                        .DrawRoundedRectangle(&irr, gleam, 0.80, &self.round_stroke);
                }
            }
        }
    }

    /// Draws a lightweight two-tier floating ambient + contact drop shadow STRICTLY OUTSIDE `rect`
    /// using `D2D1_COMBINE_MODE_EXCLUDE` so shadow pixels NEVER darken the live Liquid Glass backdrop!
    /// Caches quantized local `(0, 0, qw, qh)` geometry pairs in an LRU cache (`0` steady-state COM allocs).
    fn draw_exterior_glass_shadow(&self, rect: RectF, radius: f32, close_a: f32, far_a: f32, dy: f32) {
        if rect.w <= 2.0 || rect.h <= 2.0 || (close_a <= 0.005 && far_a <= 0.005) {
            return;
        }
        let rad = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
        let w_q = (rect.w * 2.0).round().clamp(4.0, 8192.0) as u16;
        let h_q = (rect.h * 2.0).round().clamp(4.0, 8192.0) as u16;
        let rad_q = (rad * 4.0).round().clamp(0.0, 4096.0) as u16;
        let dy_q = (dy * 4.0).round().clamp(0.0, 1024.0) as u16;
        let key = (w_q, h_q, rad_q, dy_q);

        unsafe {
            let cached_pair = self
                .glass_shadow_cache
                .borrow_mut()
                .get_touch(&key)
                .map(|pair| (pair.far_geom.clone(), pair.close_geom.clone()));

            let (far_geom, close_geom) = if let Some(pair) = cached_pair {
                pair
            } else {
                let qw = (w_q as f32) * 0.5;
                let qh = (h_q as f32) * 0.5;
                let qrad = ((rad_q as f32) * 0.25).min(qw * 0.5).min(qh * 0.5).max(0.0);
                let qdy = (dy_q as f32) * 0.25;
                let inner_rr = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: qw,
                        bottom: qh,
                    },
                    radiusX: qrad,
                    radiusY: qrad,
                };
                let Ok(inner_geom) = self.d2d_factory.CreateRoundedRectangleGeometry(&inner_rr) else {
                    return;
                };
                self.bump_stat(|st| st.glass_shadow_geom_allocs += 1);
                let inner_base: ID2D1Geometry = inner_geom.cast().unwrap();

                let build_ring = |sh_rect: RectF, sh_rad: f32| -> Option<ID2D1PathGeometry> {
                    let sr = D2D1_ROUNDED_RECT {
                        rect: to_d2d_rect(sh_rect),
                        radiusX: sh_rad.min(sh_rect.w * 0.5).min(sh_rect.h * 0.5).max(0.0),
                        radiusY: sh_rad.min(sh_rect.w * 0.5).min(sh_rect.h * 0.5).max(0.0),
                    };
                    let Ok(sh_geom) = self.d2d_factory.CreateRoundedRectangleGeometry(&sr) else {
                        return None;
                    };
                    self.bump_stat(|st| st.glass_shadow_geom_allocs += 1);
                    let Ok(path) = self.d2d_factory.CreatePathGeometry() else {
                        return None;
                    };
                    self.bump_stat(|st| st.glass_shadow_geom_allocs += 1);
                    let ok = if let Ok(sink) = path.Open() {
                        let res = sh_geom
                            .CombineWithGeometry(
                                &inner_base,
                                D2D1_COMBINE_MODE_EXCLUDE,
                                None,
                                0.30,
                                &sink,
                            )
                            .is_ok();
                        let _ = sink.Close();
                        res
                    } else {
                        false
                    };
                    if ok {
                        Some(path.into())
                    } else {
                        None
                    }
                };

                let fg = build_ring(
                    RectF::new(-2.5, qdy * 1.45, qw + 5.0, qh + 3.0),
                    qrad + 2.5,
                );
                let cg = build_ring(
                    RectF::new(0.8, (qdy * 0.50).max(1.0), (qw - 1.6).max(1.0), qh),
                    qrad,
                );
                self.glass_shadow_cache.borrow_mut().insert(
                    key,
                    CachedShadowPair {
                        far_geom: fg.clone(),
                        close_geom: cg.clone(),
                    },
                    GLASS_SHADOW_CACHE_CAP,
                );
                (fg, cg)
            };

            let mut saved_xf = Matrix3x2::identity();
            self.dc.GetTransform(&mut saved_xf);
            let sh_xf = Matrix3x2 {
                M11: saved_xf.M11,
                M12: saved_xf.M12,
                M21: saved_xf.M21,
                M22: saved_xf.M22,
                M31: rect.x * saved_xf.M11 + rect.y * saved_xf.M21 + saved_xf.M31,
                M32: rect.x * saved_xf.M12 + rect.y * saved_xf.M22 + saved_xf.M32,
            };
            self.dc.SetTransform(&sh_xf);
            if far_a > 0.004 {
                if let Some(ref fg) = far_geom {
                    self.dc
                        .FillGeometry(fg, self.brush(Color::rgba(0, 0, 0, far_a)), None);
                }
            }
            if close_a > 0.004 {
                if let Some(ref cg) = close_geom {
                    self.dc
                        .FillGeometry(cg, self.brush(Color::rgba(0, 0, 0, close_a)), None);
                }
            }
            self.dc.SetTransform(&saved_xf);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_liquid_glass_button(
        &self,
        rect: RectF,
        radius: f32,
        variant: GlassBtnVariant,
        hovered: bool,
        pressed: bool,
        mouse_pos: (f32, f32),
        reduce_transparency: bool,
    ) {
        self.draw_liquid_glass_button_impl(
            rect,
            radius,
            variant,
            hovered,
            pressed,
            mouse_pos,
            reduce_transparency,
            true,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_liquid_glass_button_impl(
        &self,
        rect: RectF,
        radius: f32,
        variant: GlassBtnVariant,
        hovered: bool,
        pressed: bool,
        mouse_pos: (f32, f32),
        reduce_transparency: bool,
        draw_shadow: bool,
        pre_sampled_anim: Option<(f32, f32, f32, (f32, f32))>,
    ) {
        if rect.w <= 1.0 || rect.h <= 1.0 {
            return;
        }
        let (scale, hover_t, press_t, caustic_pt) = if reduce_transparency {
            let sc = if pressed && hovered {
                0.96
            } else if hovered {
                1.015
            } else {
                1.0
            };
            (sc, if hovered { 1.0 } else { 0.0 }, if pressed && hovered { 1.0 } else { 0.0 }, mouse_pos)
        } else {
            pre_sampled_anim.unwrap_or_else(|| self.sample_glass_anim(rect, hovered, pressed, mouse_pos))
        };
        let dx = rect.w * (scale - 1.0) * 0.5;
        let dy = rect.h * (scale - 1.0) * 0.5;
        let r = RectF::new(
            rect.x - dx,
            rect.y - dy,
            (rect.w + dx * 2.0).max(2.0),
            (rect.h + dy * 2.0).max(2.0),
        );
        let rad = (radius * scale).min(r.w * 0.5).min(r.h * 0.5).max(0.0);

        if reduce_transparency {
            let is_down = pressed && hovered;
            let (flat, rim_a) = match variant {
                GlassBtnVariant::ProminentLight => (
                    if is_down {
                        Color::rgb(224, 226, 234)
                    } else if hovered {
                        Color::rgb(252, 252, 255)
                    } else {
                        Color::rgb(242, 242, 247)
                    },
                    if hovered { 0.24 } else { 0.12 },
                ),
                GlassBtnVariant::Tinted(t) => (
                    if is_down {
                        t.lerp(Color::rgb(0, 0, 0), 0.14).with_alpha(0.95)
                    } else if hovered {
                        t.lerp(Color::rgb(255, 255, 255), 0.14).with_alpha(0.96)
                    } else {
                        t.with_alpha(0.92)
                    },
                    if hovered { 0.24 } else { 0.14 },
                ),
                GlassBtnVariant::Regular => (
                    if is_down {
                        Color::rgb(32, 32, 38)
                    } else if hovered {
                        Color::rgb(54, 54, 64)
                    } else {
                        Color::rgb(40, 40, 47)
                    },
                    if hovered { 0.18 } else { 0.09 },
                ),
                GlassBtnVariant::Subtle => (
                    if is_down {
                        Color::rgb(28, 28, 34)
                    } else if hovered {
                        Color::rgb(46, 46, 56)
                    } else {
                        Color::rgb(33, 33, 40)
                    },
                    if hovered { 0.15 } else { 0.06 },
                ),
            };
            self.fill_rounded_rect(r, rad, flat);
            self.stroke_rounded_rect(r, rad, Color::rgba(255, 255, 255, rim_a), 1.0);
            return;
        }

        // 1. Lightweight contact + ambient shadow (skipped when floating button already drew exterior shadow)
        if draw_shadow {
            let sh_close = (0.16 + hover_t * 0.10 - press_t * 0.06).clamp(0.08, 0.30);
            let sh_far = (0.07 + hover_t * 0.05 - press_t * 0.03).clamp(0.03, 0.14);
            let sh_dy = 1.6 + hover_t * 1.0 - press_t * 0.6;
            self.fill_rounded_rect(
                RectF::new(r.x - 0.8, r.y + sh_dy, r.w + 1.6, r.h + 1.2),
                (rad + 0.8).min(r.h * 0.5 + 0.8),
                Color::rgba(0, 0, 0, sh_far),
            );
            self.fill_rounded_rect(
                RectF::new(r.x + 0.4, r.y + sh_dy * 0.55, r.w - 0.8, r.h),
                rad,
                Color::rgba(0, 0, 0, sh_close),
            );
        }

        // 2. Crystal-Clear iPadOS 26 Translucent Optical Body (alpha ~0.20..0.28 on floating buttons!)
        let (c_top, c_bot, rim_op, dark_sep) = match variant {
            GlassBtnVariant::ProminentLight => {
                let boost = hover_t * 0.04;
                (
                    Color::rgba(255, 255, 255, (0.94 + boost).min(0.99)),
                    Color::rgba(238, 240, 246, (0.89 + boost).min(0.96)),
                    1.25,
                    0.18,
                )
            }
            GlassBtnVariant::Tinted(tint) => {
                let h_add = hover_t * 0.10;
                let top_c = tint
                    .lerp(Color::rgb(255, 255, 255), 0.10 + hover_t * 0.10)
                    .with_alpha((0.58 + h_add).min(0.82));
                let bot_c = tint
                    .lerp(Color::rgb(10, 10, 14), 0.10)
                    .with_alpha((0.66 + h_add).min(0.86));
                (top_c, bot_c, 1.05, 0.24)
            }
            GlassBtnVariant::Regular => {
                if !draw_shadow {
                    // Standalone floating crystal button (e.g. top-bar controls) — truly transparent!
                    (
                        Color::rgba(18, 20, 26, 0.21 + hover_t * 0.06),
                        Color::rgba(12, 14, 20, 0.27 + hover_t * 0.06),
                        1.05,
                        0.28,
                    )
                } else {
                    (
                        Color::rgba(28, 30, 38, 0.24 + hover_t * 0.08),
                        Color::rgba(16, 18, 24, 0.30 + hover_t * 0.08),
                        0.96,
                        0.22,
                    )
                }
            }
            GlassBtnVariant::Subtle => {
                if !draw_shadow {
                    (
                        Color::rgba(18, 20, 26, 0.19 + hover_t * 0.06),
                        Color::rgba(12, 14, 20, 0.25 + hover_t * 0.06),
                        0.95,
                        0.26,
                    )
                } else {
                    (
                        Color::rgba(255, 255, 255, 0.06 + hover_t * 0.09),
                        Color::rgba(255, 255, 255, 0.03 + hover_t * 0.06),
                        0.72 + hover_t * 0.28,
                        0.16,
                    )
                }
            }
        };

        self.fill_linear_gradient_rect(r, rad, r.x, r.y, r.x, r.bottom(), c_top, c_bot);

        // 3. Smooth Cursor-Tracking Luminous Optical Caustic Glow (cached unit-alpha brush!)
        let near_box = r.inset(-40.0, -28.0);
        if near_box.contains(mouse_pos.0, mouse_pos.1) || hover_t > 0.02 {
            let caustic_a = (0.055 + hover_t * 0.16 + press_t * 0.06).clamp(0.0, 0.28);
            if caustic_a > 0.01 {
                if let Some(ref cb) = self.glass_caustic_brush {
                    let rad_glow = r.w.max(r.h) * 1.05 + 12.0;
                    unsafe {
                        cb.SetCenter(pt2(caustic_pt.0, caustic_pt.1));
                        cb.SetRadiusX(rad_glow);
                        cb.SetRadiusY(rad_glow);
                        cb.SetOpacity(caustic_a);
                        let rr = D2D1_ROUNDED_RECT {
                            rect: to_d2d_rect(r),
                            radiusX: rad,
                            radiusY: rad,
                        };
                        self.dc.FillRoundedRectangle(&rr, cb);
                    }
                }
            }
        }

        // 4. Strictly VERTICAL Top & Bottom iPadOS 26 Specular Rim Highlights (cached brushes!)
        self.draw_vertical_glass_rim(r, rad, hover_t, rim_op, dark_sep);
    }

    /// Standalone Floating Liquid Glass Button (Top Bar controls & Lyrics controls):
    /// samples the single per-frame half-res pre-blurred snapshot through one fast HiDPI-exact
    /// FP16 SDF displacement pass, animates with elastic hover/press spring dynamics, and records
    /// its rect so white glyphs rendered on top receive a crisp 1px optical drop shadow!
    #[allow(clippy::too_many_arguments)]
    pub fn draw_liquid_glass_floating_button(
        &mut self,
        rect: RectF,
        radius: f32,
        variant: GlassBtnVariant,
        hovered: bool,
        pressed: bool,
        mouse_pos: (f32, f32),
        reduce_transparency: bool,
    ) {
        let mut used_live_backdrop = false;
        let mut anim_sample = None;
        if !reduce_transparency && rect.w > 2.0 && rect.h > 2.0 {
            let anim = self.sample_glass_anim(rect, hovered, pressed, mouse_pos);
            let (scale, hover_t, press_t, _) = anim;
            anim_sample = Some(anim);
            let dx = rect.w * (scale - 1.0) * 0.5;
            let dy = rect.h * (scale - 1.0) * 0.5;
            let r = RectF::new(
                rect.x - dx,
                rect.y - dy,
                (rect.w + dx * 2.0).max(2.0),
                (rect.h + dy * 2.0).max(2.0),
            );
            let rad = (radius * scale).min(r.w * 0.5).min(r.h * 0.5).max(0.0);
            used_live_backdrop = self.draw_liquid_glass_backdrop_only(r, rad, 14.0, 1.0, false);
            if used_live_backdrop {
                let sc = (0.18 + hover_t * 0.08 - press_t * 0.06).clamp(0.10, 0.28);
                let sf = (0.09 + hover_t * 0.04 - press_t * 0.03).clamp(0.05, 0.14);
                let sh_dy = 1.6 + hover_t * 0.8 - press_t * 0.5;
                self.draw_exterior_glass_shadow(r, rad, sc, sf, sh_dy);
                if !matches!(variant, GlassBtnVariant::ProminentLight) {
                    self.record_live_glass_rect(r);
                }
            }
        }
        self.draw_liquid_glass_button_impl(
            rect,
            radius,
            variant,
            hovered,
            pressed,
            mouse_pos,
            reduce_transparency,
            !used_live_backdrop,
            anim_sample,
        );
    }

    /// Apple / Liquid Glass 10-spoke progressive luminance spinner + subtle sweeping orbital arc
    /// for track/album covers and bottom MiniPlayer buffering state!
    pub fn draw_loading_spinner(&self, cx: f32, cy: f32, radius: f32, phase: f32, tint: Color) {
        if radius <= 2.0 {
            return;
        }
        // Subtle outer orbital ring + sweeping arc
        self.stroke_circle(cx, cy, radius, tint.with_alpha(0.14), (radius * 0.14).clamp(1.1, 2.2));

        let spokes = 10usize;
        let r_in = radius * 0.44;
        let r_out = radius * 0.88;
        let sw = (radius * 0.17).clamp(1.4, 2.6);
        let head = phase.rem_euclid(1.0) * (spokes as f32);

        for i in 0..spokes {
            let dist = (head - (i as f32)).rem_euclid(spokes as f32);
            let trail = (1.0 - dist / (spokes as f32)).powf(1.85);
            let alpha = (0.14 + 0.86 * trail) * tint.a;
            let ang = -PI * 0.5 + (i as f32 / spokes as f32) * 2.0 * PI;
            let (sa, ca) = ang.sin_cos();
            self.draw_line(
                cx + r_in * ca,
                cy + r_in * sa,
                cx + r_out * ca,
                cy + r_out * sa,
                tint.with_alpha(alpha),
                sw,
            );
        }
    }

    pub fn set_glass_snapshot_x_rois(
        &mut self,
        top_spans_dip: [(f32, f32); 2],
        top_span_count: u8,
        bot_span_dip: Option<(f32, f32)>,
    ) {
        let s = self.dpi_scale.max(1.0);
        let merge_gap_px = (32.0 * s).ceil() as u32;
        let to_px_span = |a: f32, b: f32| -> Option<(u32, u32)> {
            let l = ((a * s).floor() as i32).clamp(0, self.width_px as i32) as u32;
            let r = ((b * s).ceil() as i32).clamp(0, self.width_px as i32) as u32;
            if r > l + 1 {
                Some((l, r))
            } else {
                None
            }
        };

        let mut spans = [(0u32, 0u32); 2];
        let mut cnt = 0u8;
        let max_in = (top_span_count as usize).min(2);
        for &(a, b) in &top_spans_dip[..max_in] {
            if let Some(sp) = to_px_span(a, b) {
                spans[cnt as usize] = sp;
                cnt += 1;
            }
        }
        if cnt == 2 {
            if spans[0].0 > spans[1].0 {
                spans.swap(0, 1);
            }
            if spans[0].1.saturating_add(merge_gap_px) >= spans[1].0 {
                spans[0] = (spans[0].0, spans[0].1.max(spans[1].1));
                spans[1] = (0, 0);
                cnt = 1;
            }
        }
        self.glass_top_x_spans_px = spans;
        self.glass_top_x_span_count = cnt;
        self.glass_bot_x_span_px = bot_span_dip.and_then(|(a, b)| to_px_span(a, b));
    }

    fn ensure_glass_batch_bounded_top(&mut self, force_reattach: bool) {
        let (Some(ref blurred_top), Some(ref bg_scale)) =
            (&self.glass_top_blurred, &self.glass_bg_scale_effect)
        else {
            return;
        };
        if self.glass_top_half_w == 0 || self.glass_top_half_h == 0 || self.glass_top_snap_h == 0 {
            return;
        }
        let cur_key = (
            blurred_top.as_raw() as usize,
            self.width_px,
            self.glass_top_snap_h,
            self.glass_top_half_w,
            self.glass_top_half_h,
        );
        if !force_reattach
            && self.glass_batch_top_key == Some(cur_key)
            && self.glass_batch_bounded_top.is_some()
        {
            return;
        }
        unsafe {
            if self.glass_batch_top_key == Some(cur_key) && self.glass_batch_bounded_top.is_some() {
                bg_scale.SetInput(0, blurred_top, true);
                if let (Some(ref brd), Ok(scaled_img)) =
                    (&self.glass_border_effect, bg_scale.GetOutput())
                {
                    brd.SetInput(0, &scaled_img, true);
                }
                return;
            }
            let sx = (self.width_px as f32) / (self.glass_top_half_w as f32);
            let sy = (self.glass_top_snap_h as f32) / (self.glass_top_half_h as f32);
            let up_xf = Matrix3x2 {
                M11: sx,
                M12: 0.0,
                M21: 0.0,
                M22: sy,
                M31: 0.0,
                M32: 0.0,
            };
            bg_scale.SetInput(0, blurred_top, true);
            let xf_b = mat3x2_bytes(up_xf);
            let _ = bg_scale.SetValue(
                D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                D2D1_PROPERTY_TYPE_MATRIX_3X2,
                &xf_b,
            );
            self.bump_stat(|st| st.glass_effect_rebinds += 1);
            let Ok(scaled_img) = bg_scale.GetOutput() else {
                self.glass_batch_bounded_top = None;
                self.glass_batch_top_key = None;
                return;
            };
            let bounded: ID2D1Image = if let Some(ref brd) = self.glass_border_effect {
                brd.SetInput(0, &scaled_img, true);
                self.bump_stat(|st| st.glass_effect_rebinds += 1);
                brd.GetOutput().unwrap_or(scaled_img)
            } else {
                scaled_img
            };
            self.glass_batch_bounded_top = Some(bounded);
            self.glass_batch_top_key = Some(cur_key);
        }
    }

    fn ensure_glass_batch_bounded_bot(&mut self, force_reattach: bool) {
        let (Some(ref blurred_bot), Some(ref bg_scale)) =
            (&self.glass_bot_blurred, &self.glass_bot_bg_scale_effect)
        else {
            return;
        };
        if self.glass_bot_half_w == 0 || self.glass_bot_half_h == 0 || self.glass_bot_snap_h == 0 {
            return;
        }
        let cur_key = (
            blurred_bot.as_raw() as usize,
            self.width_px,
            self.glass_bot_snap_h,
            self.glass_copy_bot_px,
            self.glass_bot_half_w,
            self.glass_bot_half_h,
        );
        if !force_reattach
            && self.glass_batch_bot_key == Some(cur_key)
            && self.glass_batch_bounded_bot.is_some()
        {
            return;
        }
        unsafe {
            if self.glass_batch_bot_key == Some(cur_key) && self.glass_batch_bounded_bot.is_some() {
                bg_scale.SetInput(0, blurred_bot, true);
                if let (Some(ref brd), Ok(scaled_img)) =
                    (&self.glass_bot_border_effect, bg_scale.GetOutput())
                {
                    brd.SetInput(0, &scaled_img, true);
                }
                return;
            }
            let sx = (self.width_px as f32) / (self.glass_bot_half_w as f32);
            let sy = (self.glass_bot_snap_h as f32) / (self.glass_bot_half_h as f32);
            // Maps strip-local [0..half_w, 0..half_bot_h] directly into exact screen pixels
            // [0..width_px, copy_bot_px..height_px] before CLAMP border so displacement coords match 1:1!
            let up_xf = Matrix3x2 {
                M11: sx,
                M12: 0.0,
                M21: 0.0,
                M22: sy,
                M31: 0.0,
                M32: self.glass_copy_bot_px as f32,
            };
            bg_scale.SetInput(0, blurred_bot, true);
            let xf_b = mat3x2_bytes(up_xf);
            let _ = bg_scale.SetValue(
                D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                D2D1_PROPERTY_TYPE_MATRIX_3X2,
                &xf_b,
            );
            self.bump_stat(|st| st.glass_effect_rebinds += 1);
            let Ok(scaled_img) = bg_scale.GetOutput() else {
                self.glass_batch_bounded_bot = None;
                self.glass_batch_bot_key = None;
                return;
            };
            let bounded: ID2D1Image = if let Some(ref brd) = self.glass_bot_border_effect {
                brd.SetInput(0, &scaled_img, true);
                self.bump_stat(|st| st.glass_effect_rebinds += 1);
                brd.GetOutput().unwrap_or(scaled_img)
            } else {
                scaled_img
            };
            self.glass_batch_bounded_bot = Some(bounded);
            self.glass_batch_bot_key = Some(cur_key);
        }
    }

    fn ensure_glass_batch_bounded_bg(&mut self, force_reattach: bool) {
        let (Some(ref blurred_snap), Some(ref bg_scale)) =
            (&self.glass_blurred_snapshot, &self.glass_bg_scale_effect)
        else {
            return;
        };
        if self.glass_half_w == 0 || self.glass_half_h == 0 {
            return;
        }
        let cur_key = (
            blurred_snap.as_raw() as usize,
            self.width_px,
            self.height_px,
            self.glass_half_w,
            self.glass_half_h,
        );
        if !force_reattach
            && self.glass_batch_bg_key == Some(cur_key)
            && self.glass_batch_bounded_bg.is_some()
        {
            return;
        }
        unsafe {
            if self.glass_batch_bg_key == Some(cur_key) && self.glass_batch_bounded_bg.is_some() {
                bg_scale.SetInput(0, blurred_snap, true);
                if let (Some(ref brd), Ok(scaled_img)) =
                    (&self.glass_border_effect, bg_scale.GetOutput())
                {
                    brd.SetInput(0, &scaled_img, true);
                }
                return;
            }
            let sx_snap = (self.width_px as f32) / (self.glass_half_w as f32);
            let sy_snap = (self.height_px as f32) / (self.glass_half_h as f32);
            let up_xf = Matrix3x2 {
                M11: sx_snap,
                M12: 0.0,
                M21: 0.0,
                M22: sy_snap,
                M31: 0.0,
                M32: 0.0,
            };
            bg_scale.SetInput(0, blurred_snap, true);
            let xf_b = mat3x2_bytes(up_xf);
            let _ = bg_scale.SetValue(
                D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                D2D1_PROPERTY_TYPE_MATRIX_3X2,
                &xf_b,
            );
            self.bump_stat(|st| st.glass_effect_rebinds += 1);
            let Ok(scaled_img) = bg_scale.GetOutput() else {
                self.glass_batch_bounded_bg = None;
                self.glass_batch_bg_key = None;
                return;
            };
            let bounded: ID2D1Image = if let Some(ref brd) = self.glass_border_effect {
                brd.SetInput(0, &scaled_img, true);
                self.bump_stat(|st| st.glass_effect_rebinds += 1);
                brd.GetOutput().unwrap_or(scaled_img)
            } else {
                scaled_img
            };
            self.glass_batch_bounded_bg = Some(bounded);
            self.glass_batch_bg_key = Some(cur_key);
        }
    }

    /// Captures ONLY the floating Liquid Glass horizontal ROI strips (`top_dip` + `bottom_dip`) ONCE per frame
    /// into strip-sized half-resolution offscreen targets (`W × top_px` + `W × bot_px` -> 5x VRAM reduction!)
    /// and executes ONE batched crystal-clear `GaussianBlur` + `1.26x` vibrancy pass strictly over dirty ROIs.
    pub fn begin_glass_backdrop_snapshot(&mut self, reduce_transparency: bool) {
        self.begin_glass_backdrop_snapshot_strips(reduce_transparency, 118.0, 144.0);
    }

    pub fn begin_glass_backdrop_snapshot_strips(
        &mut self,
        reduce_transparency: bool,
        top_dip: f32,
        bottom_dip: f32,
    ) {
        self.glass_top_x_spans_px = [(0, 0); 2];
        self.glass_top_x_span_count = 0;
        self.glass_bot_x_span_px = None;
        self.begin_glass_backdrop_snapshot_strips_ex(
            reduce_transparency,
            top_dip,
            bottom_dip,
            None,
            BACKDROP_DIRTY_ALL,
        );
    }

    pub fn begin_glass_backdrop_snapshot_strips_ex(
        &mut self,
        reduce_transparency: bool,
        top_dip: f32,
        bottom_dip: f32,
        top_bar_solid_bg: Option<Color>,
        backdrop_dirty_mask: u32,
    ) {
        self.glass_snapshot_active = false;
        self.glass_snapshot_strip_mode = false;
        self.glass_strip_top_px = 0;
        self.glass_strip_bot_px = 0;
        self.glass_top_solid_bg = None;
        self.glass_effect_cursor = 0;
        self.glass_effect_overflow_used = false;
        if reduce_transparency
            || self.blur_effect.is_none()
            || self.width_px < 4
            || self.height_px < 4
        {
            self.invalidate_glass_snapshot_cache();
            return;
        }
        let Some(active_tgt) = self.active_draw_target().cloned() else {
            return;
        };
        let half_w = (self.width_px / 2).max(32);
        let half_h = (self.height_px / 2).max(32);
        let s = self.dpi_scale.max(1.0);
        let dpi_q = (s * 100.0).round() as u32;
        let pad_px = (24.0 * s).ceil() as u32;
        let blur_margin_half = (6.0 * s).ceil();
        let raw_top_px = ((top_dip.max(0.0) * s).ceil() as u32).min(self.height_px);
        let raw_bot_h = ((bottom_dip.max(0.0) * s).ceil() as u32).min(self.height_px);
        let raw_bot_px = self.height_px.saturating_sub(raw_bot_h);
        if raw_top_px == 0 && raw_bot_h == 0 {
            return;
        }
        let copy_top_px = if raw_top_px > 0 {
            ((raw_top_px + pad_px + 1) & !1).min(self.height_px).max(2)
        } else {
            0
        };
        let copy_bot_px = if raw_bot_h > 0 {
            raw_bot_px.saturating_sub(pad_px) & !1
        } else {
            self.height_px
        };
        let bot_copy_h = self.height_px.saturating_sub(copy_bot_px);
        let use_strips = if raw_top_px > 0 && raw_bot_h > 0 {
            copy_top_px + 32 < copy_bot_px
        } else if raw_top_px > 0 {
            copy_top_px + 32 < self.height_px
        } else {
            copy_bot_px > 32
        };

        self.glass_top_solid_bg = if use_strips && raw_top_px > 0 {
            top_bar_solid_bg
        } else {
            None
        };

        let global_mismatch = !use_strips
            || self.glass_cache_w_px != self.width_px
            || self.glass_cache_h_px != self.height_px
            || self.glass_cache_dpi_q != dpi_q
            || self.glass_cache_lyrics_sheet != self.in_lyrics_transition_sheet;
        if global_mismatch {
            self.glass_cache_valid_top = false;
            self.glass_cache_valid_bot = false;
        }

        let top_half_h = ((copy_top_px + 1) / 2).max(16);
        let bot_half_h = ((bot_copy_h + 1) / 2).max(16);

        if use_strips {
            // Release any stale full-window snapshot textures so strip mode stays at ~2.7 MB VRAM!
            if self.glass_frame_snapshot.is_some()
                || self.glass_blurred_snapshot.is_some()
            {
                self.glass_batch_bounded_bg = None;
                self.glass_batch_bg_key = None;
                self.glass_frame_snapshot = None;
                self.glass_snap_w = 0;
                self.glass_snap_h = 0;
                self.glass_blurred_snapshot = None;
                self.glass_half_w = 0;
                self.glass_half_h = 0;
                self.gpu_staging_dirty = true;
            }
            if self.glass_top_snap.is_none()
                || self.glass_top_blurred.is_none()
                || self.glass_top_snap_w != self.width_px
                || self.glass_top_snap_h != copy_top_px
                || self.glass_top_half_w != half_w
                || self.glass_top_half_h != top_half_h
            {
                self.glass_cache_valid_top = false;
            }
            if self.glass_bot_snap.is_none()
                || self.glass_bot_blurred.is_none()
                || self.glass_bot_snap_w != self.width_px
                || self.glass_bot_snap_h != bot_copy_h
                || self.glass_bot_half_w != half_w
                || self.glass_bot_half_h != bot_half_h
            {
                self.glass_cache_valid_bot = false;
            }
        }

        let top_geom_match = self.glass_cache_top_px == raw_top_px
            && self.glass_cache_top_x_span_count == self.glass_top_x_span_count
            && self.glass_cache_top_x_spans_px == self.glass_top_x_spans_px;
        let bot_geom_match = self.glass_cache_bot_px == raw_bot_px
            && self.glass_cache_bot_x_span_px == self.glass_bot_x_span_px;

        if (backdrop_dirty_mask & BACKDROP_DIRTY_TOP) != 0 {
            self.glass_cache_valid_top = false;
        }
        if (backdrop_dirty_mask & BACKDROP_DIRTY_BOTTOM) != 0 {
            self.glass_cache_valid_bot = false;
        }

        let need_top_gpu = use_strips && raw_top_px > 0 && self.glass_top_solid_bg.is_none();
        let need_bot_gpu = use_strips && raw_bot_h > 0;
        let top_cache_hit = need_top_gpu
            && self.glass_cache_valid_top
            && top_geom_match
            && (backdrop_dirty_mask & BACKDROP_DIRTY_TOP) == 0;
        let bot_cache_hit = need_bot_gpu
            && self.glass_cache_valid_bot
            && bot_geom_match
            && (backdrop_dirty_mask & BACKDROP_DIRTY_BOTTOM) == 0;
        if top_cache_hit {
            self.bump_stat(|st| st.glass_snapshot_cache_hits += 1);
        }
        if bot_cache_hit {
            self.bump_stat(|st| st.glass_snapshot_cache_hits += 1);
        }

        let draw_top = need_top_gpu && !top_cache_hit;
        let draw_bot = need_bot_gpu && !bot_cache_hit;
        let draw_full = !use_strips;

        if !draw_top && !draw_bot && !draw_full {
            self.glass_strip_top_px = if raw_top_px > 0 { raw_top_px } else { 0 };
            self.glass_strip_bot_px = if raw_bot_h > 0 { raw_bot_px } else { self.height_px };
            self.glass_copy_top_px = copy_top_px;
            self.glass_copy_bot_px = copy_bot_px;
            self.glass_snapshot_strip_mode = true;
            self.glass_snapshot_active =
                self.glass_top_solid_bg.is_some() || top_cache_hit || bot_cache_hit;
            if top_cache_hit {
                self.ensure_glass_batch_bounded_top(false);
            }
            if bot_cache_hit {
                self.ensure_glass_batch_bounded_bot(false);
            }
            return;
        }

        unsafe {
            let alpha_mode = if self.premultiplied_swapchain {
                D2D1_ALPHA_MODE_PREMULTIPLIED
            } else {
                D2D1_ALPHA_MODE_IGNORE
            };
            let snap_props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: alpha_mode,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let half_props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: std::mem::ManuallyDrop::new(None),
            };

            if use_strips {
                if draw_top
                    && (self.glass_top_snap.is_none()
                        || self.glass_top_snap_w != self.width_px
                        || self.glass_top_snap_h != copy_top_px
                        || self.glass_top_blurred.is_none()
                        || self.glass_top_half_w != half_w
                        || self.glass_top_half_h != top_half_h)
                {
                    self.glass_batch_bounded_top = None;
                    self.glass_batch_top_key = None;
                    let snap_sz = D2D_SIZE_U {
                        width: self.width_px,
                        height: copy_top_px,
                    };
                    let half_sz = D2D_SIZE_U {
                        width: half_w,
                        height: top_half_h,
                    };
                    if let (Ok(tsnap), Ok(thblur)) = (
                        self.dc.CreateBitmap(snap_sz, None, 0, &snap_props),
                        self.dc.CreateBitmap(half_sz, None, 0, &half_props),
                    ) {
                        self.glass_top_snap_w = self.width_px;
                        self.glass_top_snap_h = copy_top_px;
                        self.glass_top_half_w = half_w;
                        self.glass_top_half_h = top_half_h;
                        self.glass_top_snap = Some(tsnap);
                        self.glass_top_blurred = Some(thblur);
                        self.gpu_staging_dirty = true;
                    }
                }
                if draw_bot
                    && (self.glass_bot_snap.is_none()
                        || self.glass_bot_snap_w != self.width_px
                        || self.glass_bot_snap_h != bot_copy_h
                        || self.glass_bot_blurred.is_none()
                        || self.glass_bot_half_w != half_w
                        || self.glass_bot_half_h != bot_half_h)
                {
                    self.glass_batch_bounded_bot = None;
                    self.glass_batch_bot_key = None;
                    let snap_sz = D2D_SIZE_U {
                        width: self.width_px,
                        height: bot_copy_h,
                    };
                    let half_sz = D2D_SIZE_U {
                        width: half_w,
                        height: bot_half_h,
                    };
                    if let (Ok(bsnap), Ok(bhblur)) = (
                        self.dc.CreateBitmap(snap_sz, None, 0, &snap_props),
                        self.dc.CreateBitmap(half_sz, None, 0, &half_props),
                    ) {
                        self.glass_bot_snap_w = self.width_px;
                        self.glass_bot_snap_h = bot_copy_h;
                        self.glass_bot_half_w = half_w;
                        self.glass_bot_half_h = bot_half_h;
                        self.glass_bot_snap = Some(bsnap);
                        self.glass_bot_blurred = Some(bhblur);
                        self.gpu_staging_dirty = true;
                    }
                }
            } else {
                if self.glass_frame_snapshot.is_none()
                    || self.glass_snap_w != self.width_px
                    || self.glass_snap_h != self.height_px
                {
                    if let Ok(bmp) = self.dc.CreateBitmap(
                        D2D_SIZE_U {
                            width: self.width_px,
                            height: self.height_px,
                        },
                        None,
                        0,
                        &snap_props,
                    ) {
                        self.glass_snap_w = self.width_px;
                        self.glass_snap_h = self.height_px;
                        self.glass_frame_snapshot = Some(bmp);
                        self.gpu_staging_dirty = true;
                    }
                }
                if self.glass_blurred_snapshot.is_none()
                    || self.glass_half_w != half_w
                    || self.glass_half_h != half_h
                {
                    self.invalidate_glass_snapshot_cache();
                    let half_sz = D2D_SIZE_U {
                        width: half_w,
                        height: half_h,
                    };
                    if let Ok(hblur) = self.dc.CreateBitmap(half_sz, None, 0, &half_props) {
                        self.glass_half_w = half_w;
                        self.glass_half_h = half_h;
                        self.glass_blurred_snapshot = Some(hblur);
                        self.gpu_staging_dirty = true;
                    }
                }
            }

            let mut top_spans = [(0u32, self.width_px); 2];
            let top_n: usize = if self.glass_top_x_span_count > 0 {
                let n = (self.glass_top_x_span_count as usize).min(2);
                top_spans[..n].copy_from_slice(&self.glass_top_x_spans_px[..n]);
                n
            } else {
                1
            };
            let bot_span = self.glass_bot_x_span_px.unwrap_or((0, self.width_px));

            self.flush_dc();
            self.glass_local_fallback_pending_flush.set(false);
            self.bump_stat(|st| st.glass_snapshot_flushes += 1);

            if use_strips {
                if draw_top {
                    let Some(ref tsnap) = self.glass_top_snap else {
                        return;
                    };
                    for &(xl, xr) in &top_spans[..top_n] {
                        let copy_l = xl.saturating_sub(pad_px) & !1;
                        let copy_r = ((xr + pad_px + 1) & !1).min(self.width_px);
                        if copy_r <= copy_l + 1 {
                            continue;
                        }
                        let top_u = D2D_RECT_U {
                            left: copy_l,
                            top: 0,
                            right: copy_r,
                            bottom: copy_top_px,
                        };
                        let dst_top = D2D_POINT_2U { x: copy_l, y: 0 };
                        if tsnap
                            .CopyFromBitmap(Some(&dst_top), &active_tgt, Some(&top_u))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                if draw_bot {
                    let Some(ref bsnap) = self.glass_bot_snap else {
                        return;
                    };
                    let (xl, xr) = bot_span;
                    let copy_l = xl.saturating_sub(pad_px) & !1;
                    let copy_r = ((xr + pad_px + 1) & !1).min(self.width_px);
                    if copy_r > copy_l + 1 {
                        let bot_u = D2D_RECT_U {
                            left: copy_l,
                            top: copy_bot_px,
                            right: copy_r,
                            bottom: self.height_px,
                        };
                        let dst_bot = D2D_POINT_2U { x: copy_l, y: 0 };
                        if bsnap
                            .CopyFromBitmap(Some(&dst_bot), &active_tgt, Some(&bot_u))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            } else {
                let Some(ref snap_bmp) = self.glass_frame_snapshot else {
                    return;
                };
                let src_u = D2D_RECT_U {
                    left: 0,
                    top: 0,
                    right: self.width_px,
                    bottom: self.height_px,
                };
                let dst_pt = D2D_POINT_2U { x: 0, y: 0 };
                if snap_bmp
                    .CopyFromBitmap(Some(&dst_pt), &active_tgt, Some(&src_u))
                    .is_err()
                {
                    return;
                }
            }

            let mut saved_xf = Matrix3x2::identity();
            self.dc.GetTransform(&mut saved_xf);
            let sx_to_half = (half_w as f32) / (self.width_px as f32);
            let sigma_half = (3.8 * self.dpi_scale * 0.5).clamp(1.6, 5.5);
            let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_SPEED.0 as u32;
            let border_mode = D2D1_BORDER_MODE_HARD.0 as u32;
            let vib_mat = mat5x4_bytes(Self::build_glass_vibrancy_matrix(1.26, 1.03, 0.012));

            // Detach upstream/downstream effect inputs BEFORE binding `thblur`/`bhblur`/`blurred_snap`
            // as render targets so D2D1/D3D11 never sees RTV/SRV aliasing.
            if use_strips {
                if draw_top {
                    if let Some(ref e) = self.glass_top_downscale_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                    if let Some(ref e) = self.blur_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                    if let Some(ref e) = self.glass_vibrancy_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                    if let Some(ref e) = self.glass_bg_scale_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                }
                if draw_bot {
                    if let Some(ref e) = self.glass_bot_downscale_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                    if let Some(ref e) = self.glass_bot_blur_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                    if let Some(ref e) = self.glass_bot_vibrancy_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                    if let Some(ref e) = self.glass_bot_bg_scale_effect {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                }
            } else {
                if let Some(ref e) = self.glass_top_downscale_effect {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
                if let Some(ref e) = self.blur_effect {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
                if let Some(ref e) = self.glass_vibrancy_effect {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
                if let Some(ref e) = self.glass_bg_scale_effect {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
            }

            // Suspend any active R4 dirty-rect clip before switching D2D render targets (`SetTarget`).
            self.suspend_dirty_clip();

            let mut did_blur_pass = false;
            if use_strips {
                // Top strip: single-pass GPU chain `tsnap (already materialized via CopyFromBitmap)
                // -> 0.5x AffineTransform -> GaussianBlur -> ColorMatrix -> thblur` (zero intermediate RT!)
                if draw_top {
                    if let (Some(ref tsnap), Some(ref thblur), Some(ref down), Some(ref blur)) = (
                        &self.glass_top_snap,
                        &self.glass_top_blurred,
                        &self.glass_top_downscale_effect,
                        &self.blur_effect,
                    ) {
                        let sy_top = (top_half_h as f32) / (copy_top_px.max(1) as f32);
                        let down_xf = mat3x2_bytes(Matrix3x2 {
                            M11: sx_to_half,
                            M12: 0.0,
                            M21: 0.0,
                            M22: sy_top,
                            M31: 0.0,
                            M32: 0.0,
                        });
                        down.SetInput(0, tsnap, true);
                        let _ = down.SetValue(
                            D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                            D2D1_PROPERTY_TYPE_MATRIX_3X2,
                            &down_xf,
                        );

                        self.dc.SetTarget(thblur);
                        self.dc.SetDpi(96.0, 96.0);
                        self.dc.SetTransform(&Matrix3x2::identity());
                        if let Ok(down_out) = down.GetOutput() {
                            blur.SetInput(0, &down_out, true);
                            let _ = blur.SetValue(
                                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                                D2D1_PROPERTY_TYPE_FLOAT,
                                &sigma_half.to_le_bytes(),
                            );
                            let _ = blur.SetValue(
                                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                                D2D1_PROPERTY_TYPE_UINT32,
                                &opt.to_le_bytes(),
                            );
                            let _ = blur.SetValue(
                                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                                D2D1_PROPERTY_TYPE_UINT32,
                                &border_mode.to_le_bytes(),
                            );
                            let final_top = if let (Some(ref vib), Ok(b_out)) =
                                (&self.glass_vibrancy_effect, blur.GetOutput())
                            {
                                vib.SetInput(0, &b_out, true);
                                let _ = vib.SetValue(
                                    D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                                    D2D1_PROPERTY_TYPE_BLOB,
                                    &vib_mat,
                                );
                                vib.GetOutput().ok().or(Some(b_out))
                            } else {
                                blur.GetOutput().ok()
                            };
                            if let Some(img) = final_top {
                                for &(xl, xr) in &top_spans[..top_n] {
                                    let blur_l = ((xl as f32) * sx_to_half - blur_margin_half)
                                        .floor()
                                        .max(0.0);
                                    let blur_r = ((xr as f32) * sx_to_half + blur_margin_half)
                                        .ceil()
                                        .min(half_w as f32);
                                    if blur_r <= blur_l + 1.0 {
                                        continue;
                                    }
                                    let top_img_rc = D2D_RECT_F {
                                        left: blur_l,
                                        top: 0.0,
                                        right: blur_r,
                                        bottom: top_half_h as f32,
                                    };
                                    self.dc.DrawImage(
                                        &img,
                                        Some(&pt2(blur_l, 0.0)),
                                        Some(&top_img_rc),
                                        D2D1_INTERPOLATION_MODE_LINEAR,
                                        windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_COPY,
                                    );
                                }
                                did_blur_pass = true;
                            }
                        }
                    }
                }

                // Bottom strip: single-pass GPU chain `bsnap -> 0.5x AffineTransform -> GaussianBlur
                // -> ColorMatrix -> bhblur` using independent effect instances (0 intermediate flush!).
                if draw_bot {
                    let shares_top_eff = self.glass_bot_blur_effect.is_none()
                        || self.glass_bot_downscale_effect.is_none();
                    if draw_top && shares_top_eff {
                        self.flush_dc();
                    }
                    let bot_down_eff = self
                        .glass_bot_downscale_effect
                        .as_ref()
                        .or(self.glass_top_downscale_effect.as_ref());
                    let bot_blur_eff = self
                        .glass_bot_blur_effect
                        .as_ref()
                        .or(self.blur_effect.as_ref());
                    let bot_vib_eff = self
                        .glass_bot_vibrancy_effect
                        .as_ref()
                        .or(self.glass_vibrancy_effect.as_ref());
                    if let (Some(ref bsnap), Some(ref bhblur), Some(down), Some(blur)) = (
                        &self.glass_bot_snap,
                        &self.glass_bot_blurred,
                        bot_down_eff,
                        bot_blur_eff,
                    ) {
                        let sy_bot = (bot_half_h as f32) / (bot_copy_h.max(1) as f32);
                        let down_xf = mat3x2_bytes(Matrix3x2 {
                            M11: sx_to_half,
                            M12: 0.0,
                            M21: 0.0,
                            M22: sy_bot,
                            M31: 0.0,
                            M32: 0.0,
                        });
                        down.SetInput(0, bsnap, true);
                        let _ = down.SetValue(
                            D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                            D2D1_PROPERTY_TYPE_MATRIX_3X2,
                            &down_xf,
                        );

                        self.dc.SetTarget(bhblur);
                        self.dc.SetDpi(96.0, 96.0);
                        self.dc.SetTransform(&Matrix3x2::identity());
                        if let Ok(down_out) = down.GetOutput() {
                            blur.SetInput(0, &down_out, true);
                            let _ = blur.SetValue(
                                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                                D2D1_PROPERTY_TYPE_FLOAT,
                                &sigma_half.to_le_bytes(),
                            );
                            let _ = blur.SetValue(
                                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                                D2D1_PROPERTY_TYPE_UINT32,
                                &opt.to_le_bytes(),
                            );
                            let _ = blur.SetValue(
                                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                                D2D1_PROPERTY_TYPE_UINT32,
                                &border_mode.to_le_bytes(),
                            );
                            let final_bot = if let (Some(vib), Ok(b_out)) =
                                (bot_vib_eff, blur.GetOutput())
                            {
                                vib.SetInput(0, &b_out, true);
                                let _ = vib.SetValue(
                                    D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                                    D2D1_PROPERTY_TYPE_BLOB,
                                    &vib_mat,
                                );
                                vib.GetOutput().ok().or(Some(b_out))
                            } else {
                                blur.GetOutput().ok()
                            };
                            if let Some(img) = final_bot {
                                let (xl, xr) = bot_span;
                                let blur_l = ((xl as f32) * sx_to_half - blur_margin_half)
                                    .floor()
                                    .max(0.0);
                                let blur_r = ((xr as f32) * sx_to_half + blur_margin_half)
                                    .ceil()
                                    .min(half_w as f32);
                                if blur_r > blur_l + 1.0 {
                                    let bot_img_rc = D2D_RECT_F {
                                        left: blur_l,
                                        top: 0.0,
                                        right: blur_r,
                                        bottom: bot_half_h as f32,
                                    };
                                    self.dc.DrawImage(
                                        &img,
                                        Some(&pt2(blur_l, 0.0)),
                                        Some(&bot_img_rc),
                                        D2D1_INTERPOLATION_MODE_LINEAR,
                                        windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_COPY,
                                    );
                                }
                                did_blur_pass = true;
                            }
                        }
                    }
                }

                if did_blur_pass {
                    self.bump_stat(|st| st.glass_snapshot_blurs += 1);
                    self.glass_snapshot_write_gen = self.glass_snapshot_write_gen.wrapping_add(1);
                    self.glass_strip_top_px = if raw_top_px > 0 { raw_top_px } else { 0 };
                    self.glass_strip_bot_px = if raw_bot_h > 0 { raw_bot_px } else { self.height_px };
                    self.glass_copy_top_px = copy_top_px;
                    self.glass_copy_bot_px = copy_bot_px;
                    self.glass_cache_w_px = self.width_px;
                    self.glass_cache_h_px = self.height_px;
                    self.glass_cache_dpi_q = dpi_q;
                    self.glass_cache_lyrics_sheet = self.in_lyrics_transition_sheet;
                    if draw_top {
                        self.glass_cache_valid_top = true;
                        self.glass_cache_top_px = raw_top_px;
                        self.glass_cache_top_x_spans_px = self.glass_top_x_spans_px;
                        self.glass_cache_top_x_span_count = self.glass_top_x_span_count;
                    }
                    if draw_bot {
                        self.glass_cache_valid_bot = true;
                        self.glass_cache_bot_px = raw_bot_px;
                        self.glass_cache_bot_x_span_px = self.glass_bot_x_span_px;
                    }
                    self.glass_snapshot_strip_mode = true;
                    self.glass_snapshot_active = true;
                }
            } else if let (
                Some(ref snap_bmp),
                Some(ref blurred_snap),
                Some(ref down),
                Some(ref blur),
            ) = (
                &self.glass_frame_snapshot,
                &self.glass_blurred_snapshot,
                &self.glass_top_downscale_effect,
                &self.blur_effect,
            ) {
                let half_rc = D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: half_w as f32,
                    bottom: half_h as f32,
                };
                let sy_full = (half_h as f32) / (self.height_px.max(1) as f32);
                let down_xf = mat3x2_bytes(Matrix3x2 {
                    M11: sx_to_half,
                    M12: 0.0,
                    M21: 0.0,
                    M22: sy_full,
                    M31: 0.0,
                    M32: 0.0,
                });
                down.SetInput(0, snap_bmp, true);
                let _ = down.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                    D2D1_PROPERTY_TYPE_MATRIX_3X2,
                    &down_xf,
                );

                self.dc.SetTarget(blurred_snap);
                self.dc.SetDpi(96.0, 96.0);
                self.dc.SetTransform(&Matrix3x2::identity());
                if let Ok(down_out) = down.GetOutput() {
                    blur.SetInput(0, &down_out, true);
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                        D2D1_PROPERTY_TYPE_FLOAT,
                        &sigma_half.to_le_bytes(),
                    );
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                        D2D1_PROPERTY_TYPE_UINT32,
                        &opt.to_le_bytes(),
                    );
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                        D2D1_PROPERTY_TYPE_UINT32,
                        &border_mode.to_le_bytes(),
                    );
                    let final_blur = if let (Some(ref vib), Ok(b_out)) =
                        (&self.glass_vibrancy_effect, blur.GetOutput())
                    {
                        vib.SetInput(0, &b_out, true);
                        let _ = vib.SetValue(
                            D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                            D2D1_PROPERTY_TYPE_BLOB,
                            &vib_mat,
                        );
                        vib.GetOutput().ok().or(Some(b_out))
                    } else {
                        blur.GetOutput().ok()
                    };
                    if let Some(img) = final_blur {
                        self.bump_stat(|st| st.glass_snapshot_blurs += 1);
                        self.glass_snapshot_write_gen = self.glass_snapshot_write_gen.wrapping_add(1);
                        self.dc.DrawImage(
                            &img,
                            Some(&pt2(0.0, 0.0)),
                            Some(&half_rc),
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_COPY,
                        );
                        self.glass_snapshot_strip_mode = false;
                        self.glass_snapshot_active = true;
                        did_blur_pass = true;
                    }
                }
            }

            // Direct2D 1.1 requires a Flush barrier between drawing into an offscreen target
            // (`thblur`/`bhblur`/`blurred_snap`) and reading it back via an `ID2D1Effect`
            // (`glass_bg_scale_effect -> DisplacementMap -> ImageBrush`) within the same frame.
            // One single Flush here covers BOTH top and bottom strips with zero frame lag!
            if did_blur_pass {
                self.flush_dc();
            }

            // Restore active render target, DPI, transform, and any suspended R4 dirty clip
            self.dc.SetTarget(&active_tgt);
            let main_dpi = 96.0 * self.dpi_scale;
            self.dc.SetDpi(main_dpi, main_dpi);
            self.dc.SetTransform(&saved_xf);
            self.resume_dirty_clip();
        }

        if self.glass_snapshot_active {
            if self.glass_snapshot_strip_mode {
                if need_top_gpu {
                    self.ensure_glass_batch_bounded_top(draw_top);
                }
                if need_bot_gpu {
                    self.ensure_glass_batch_bounded_bot(draw_bot);
                }
            } else {
                self.ensure_glass_batch_bounded_bg(draw_full);
            }
        }
    }

    pub fn end_glass_backdrop_snapshot(&mut self) {
        self.glass_snapshot_active = false;
        self.glass_snapshot_strip_mode = false;
        self.glass_strip_top_px = 0;
        self.glass_strip_bot_px = 0;
        self.glass_top_solid_bg = None;
        self.glass_top_x_spans_px = [(0, 0); 2];
        self.glass_top_x_span_count = 0;
        self.glass_bot_x_span_px = None;
        self.glass_effect_cursor = 0;
        self.glass_effect_overflow_used = false;
        unsafe {
            if let Some(ref b) = self.glass_image_brush {
                b.SetImage(None::<&ID2D1Image>);
            }
        }
    }

    /// Evaluates the C2-continuous quintic Hermite (`6u^5 - 15u^4 + 10u^3`) SDF convex glass lens
    /// normalized displacement `(dx, dy)` in `[-1.0, 1.0]` at centered coordinates `(px, py)`.
    #[inline]
    fn eval_sdf_glass_lens_offset(
        px: f32,
        py: f32,
        bx: f32,
        by: f32,
        r: f32,
        bezel_w: f32,
        inv_bezel: f32,
    ) -> (f32, f32) {
        let sx = if px < 0.0 { -1.0f32 } else { 1.0f32 };
        let sy = if py < 0.0 { -1.0f32 } else { 1.0f32 };
        let qx = px.abs() - bx;
        let qy = py.abs() - by;
        let ox = qx.max(0.0);
        let oy = qy.max(0.0);

        // Exact rounded-rect SDF (<= 0 inside, 0 on outer perimeter)
        let sdf = ox.hypot(oy) + qx.max(qy).min(0.0) - r;
        let d_edge = (-sdf).max(0.0);
        if d_edge >= bezel_w {
            return (0.0, 0.0);
        }

        let (nx, ny) = if qx > 0.0 && qy > 0.0 {
            let len = ox.hypot(oy).max(1e-5);
            ((ox / len) * sx, (oy / len) * sy)
        } else {
            // C-infinity normal transition across medial axis (qx == qy) -> zero diagonal crease
            let diff = (qx - qy).clamp(-16.0, 16.0);
            let wx = 1.0 / (1.0 + (-diff * 1.4).exp());
            let wy = 1.0 - wx;
            let gx = wx * sx;
            let gy = wy * sy;
            let len = gx.hypot(gy).max(1e-5);
            (gx / len, gy / len)
        };

        let u = (1.0 - d_edge * inv_bezel).clamp(0.0, 1.0);
        // C2 quintic smootherstep (zero 1st & 2nd derivative at core boundary -> zero seam on 4K HiDPI!)
        let quintic = u * u * u * (u * (u * 6.0 - 15.0) + 10.0);
        let mag = quintic * (0.24 + 0.76 * u * u);
        // Convex perimeter bends viewing rays INWARD along -normal
        (-mag * nx, -mag * ny)
    }

    /// Generates an IEEE-754 16-bit float (`DXGI_FORMAT_R16G16B16A16_FLOAT`, `[R=dx, G=dy, B=0.5, A=1.0]`)
    /// SDF convex glass lens displacement map with zero 8-bit staircase banding even on 4K 200% HiDPI!
    pub fn generate_liquid_glass_sdf_displacement_map_f16(
        map_w: u32,
        map_h: u32,
        rect_w: f32,
        rect_h: f32,
        radius: f32,
    ) -> Vec<u16> {
        let mw = map_w.max(2);
        let mh = map_h.max(2);
        let rw = rect_w.max(2.0);
        let rh = rect_h.max(2.0);
        let hx = rw * 0.5;
        let hy = rh * 0.5;
        let r = radius.clamp(0.0, hx.min(hy));
        let bx = (hx - r).max(0.0);
        let by = (hy - r).max(0.0);
        let bezel_w = (rw.min(rh) * 0.24).clamp(6.0, 42.0).min(hx.min(hy).max(1.0));
        let inv_bezel = 1.0 / bezel_w.max(1.0);

        const HALF_05: u16 = 0x3800; // exact 0.5 in IEEE-754 binary16
        const HALF_10: u16 = 0x3C00; // exact 1.0 in IEEE-754 binary16

        let mut data = vec![0u16; (mw * mh * 4) as usize];
        for iy in 0..mh {
            let py = ((iy as f32 + 0.5) / (mh as f32)) * rh - hy;
            for ix in 0..mw {
                let px = ((ix as f32 + 0.5) / (mw as f32)) * rw - hx;
                let (dx_norm, dy_norm) =
                    Self::eval_sdf_glass_lens_offset(px, py, bx, by, r, bezel_w, inv_bezel);
                let idx = ((iy * mw + ix) * 4) as usize;
                data[idx] = f32_to_f16_bits(0.5 + dx_norm * 0.5);
                data[idx + 1] = f32_to_f16_bits(0.5 + dy_norm * 0.5);
                data[idx + 2] = HALF_05;
                data[idx + 3] = HALF_10;
            }
        }
        data
    }

    /// Generates an analytical C2 quintic SDF convex glass lens displacement map in BGRA8 layout
    /// (`[B, G=dy, R=dx, A=255]`) with 4×4 Bayer dithering on the bezel (fallback when FP16 is unavailable).
    pub fn generate_liquid_glass_sdf_displacement_map(
        map_w: u32,
        map_h: u32,
        rect_w: f32,
        rect_h: f32,
        radius: f32,
    ) -> Vec<u8> {
        const BAYER4: [f32; 16] = [
            0.0 / 16.0, 8.0 / 16.0, 2.0 / 16.0, 10.0 / 16.0,
            12.0 / 16.0, 4.0 / 16.0, 14.0 / 16.0, 6.0 / 16.0,
            3.0 / 16.0, 11.0 / 16.0, 1.0 / 16.0, 9.0 / 16.0,
            15.0 / 16.0, 7.0 / 16.0, 13.0 / 16.0, 5.0 / 16.0,
        ];
        let mw = map_w.max(2);
        let mh = map_h.max(2);
        let rw = rect_w.max(2.0);
        let rh = rect_h.max(2.0);
        let hx = rw * 0.5;
        let hy = rh * 0.5;
        let r = radius.clamp(0.0, hx.min(hy));
        let bx = (hx - r).max(0.0);
        let by = (hy - r).max(0.0);
        let bezel_w = (rw.min(rh) * 0.24).clamp(6.0, 42.0).min(hx.min(hy).max(1.0));
        let inv_bezel = 1.0 / bezel_w.max(1.0);

        let mut pixels = vec![0u8; (mw * mh * 4) as usize];
        for iy in 0..mh {
            let py = ((iy as f32 + 0.5) / (mh as f32)) * rh - hy;
            let by_idx = ((iy & 3) * 4) as usize;
            for ix in 0..mw {
                let px = ((ix as f32 + 0.5) / (mw as f32)) * rw - hx;
                let (dx_norm, dy_norm) =
                    Self::eval_sdf_glass_lens_offset(px, py, bx, by, r, bezel_w, inv_bezel);
                let dither = if dx_norm != 0.0 || dy_norm != 0.0 {
                    (BAYER4[by_idx + ((ix & 3) as usize)] - 0.5) / 255.0
                } else {
                    0.0
                };
                let r_u8 = ((0.5 + dx_norm * 0.5 + dither) * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let g_u8 = ((0.5 + dy_norm * 0.5 + dither) * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let idx = ((iy * mw + ix) * 4) as usize;
                pixels[idx] = 128;
                pixels[idx + 1] = g_u8;
                pixels[idx + 2] = r_u8;
                pixels[idx + 3] = 255;
            }
        }
        pixels
    }

    /// Returns a cached GPU `ID2D1Bitmap1` SDF displacement map in unified `96.0 DPI` physical-pixel
    /// space (prefers 16-bit float `DXGI_FORMAT_R16G16B16A16_FLOAT` with automatic BGRA8 fallback,
    /// LRU-bounded at 32 entries, keyed by quantized physical pixel `(w_px, h_px, rad_px)`).
    fn ensure_glass_displacement_map(
        &mut self,
        rw_px: f32,
        rh_px: f32,
        rad_px: f32,
    ) -> Option<(ID2D1Bitmap1, u32, u32)> {
        let w_q = ((rw_px / 4.0).round() as u16).clamp(8, 2048);
        let h_q = ((rh_px / 4.0).round() as u16).clamp(8, 2048);
        let r_q = ((rad_px / 2.0).round() as u16).clamp(0, 1024);
        let key = (w_q, h_q, r_q);

        if let Some(hit) = self.glass_disp_maps.get_touch(&key).cloned() {
            return Some(hit);
        }

        let map_w = (rw_px.round() as u32).clamp(64, 512);
        let map_h = (rh_px.round() as u32).clamp(64, 320);
        let q_rw = (w_q as f32) * 4.0;
        let q_rh = (h_q as f32) * 4.0;
        let q_rad = (r_q as f32) * 2.0;

        unsafe {
            // Try 16-bit half-float displacement map first (zero 8-bit banding on 4K 200% HiDPI!)
            let f16_data = Self::generate_liquid_glass_sdf_displacement_map_f16(
                map_w, map_h, q_rw, q_rh, q_rad,
            );
            let props_f16 = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let bmp = self
                .dc
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: map_w,
                        height: map_h,
                    },
                    Some(f16_data.as_ptr() as *const c_void),
                    map_w * 8,
                    &props_f16,
                )
                .or_else(|_| {
                    // Fallback: smooth Bayer-dithered BGRA8
                    let u8_data = Self::generate_liquid_glass_sdf_displacement_map(
                        map_w, map_h, q_rw, q_rh, q_rad,
                    );
                    let props_u8 = D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    };
                    self.dc.CreateBitmap(
                        D2D_SIZE_U {
                            width: map_w,
                            height: map_h,
                        },
                        Some(u8_data.as_ptr() as *const c_void),
                        map_w * 4,
                        &props_u8,
                    )
                })
                .ok()?;

            let entry = (bmp, map_w, map_h);
            self.glass_disp_maps.insert(key, entry.clone(), 32);
            self.gpu_staging_dirty = true;
            Some(entry)
        }
    }

    /// Ultra-Fast HiDPI-Exact iPadOS 26 Liquid Glass Optical Refraction Pipeline:
    /// - Executes 100% in unified `96.0 DPI` physical-pixel space (Input 0, Input 1, AND `dc` DPI
    ///   all match identically -> ZERO HiDPI tear seam or discontinuity on 125%–300% displays!).
    /// - When `begin_glass_backdrop_snapshot` is active, samples the single pre-blurred half-res
    ///   snapshot with ZERO Gaussian blurs, ZERO ColorMatrix passes, and ZERO flushes — just ONE
    ///   fast `CLSID_D2D1DisplacementMap` pass using our FP16 C2 quintic SDF lens!
    /// - When called outside a batch snapshot (popovers/context menus), captures a tight local
    ///   region in `96.0 DPI` physical pixels with single-pass crystal blur + single-pass lens.
    fn create_glass_effect_slot(&self) -> Option<GlassEffectSlot> {
        unsafe {
            let disp_xform = self.dc.CreateEffect(&CLSID_D2D12DAffineTransform).ok()?;
            let interp = (D2D1_2DAFFINETRANSFORM_INTERPOLATION_MODE_LINEAR.0 as u32).to_le_bytes();
            let bmode = (D2D1_BORDER_MODE_HARD.0 as u32).to_le_bytes();
            let _ = disp_xform.SetValue(
                D2D1_2DAFFINETRANSFORM_PROP_INTERPOLATION_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &interp,
            );
            let _ = disp_xform.SetValue(
                D2D1_2DAFFINETRANSFORM_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &bmode,
            );

            let disp_border = self.dc.CreateEffect(&CLSID_D2D1Border).ok()?;
            let clamp_mode = (D2D1_BORDER_EDGE_MODE_CLAMP.0 as u32).to_le_bytes();
            let _ = disp_border.SetValue(
                D2D1_BORDER_PROP_EDGE_MODE_X.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &clamp_mode,
            );
            let _ = disp_border.SetValue(
                D2D1_BORDER_PROP_EDGE_MODE_Y.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &clamp_mode,
            );
            let xform_out = disp_xform.GetOutput().ok()?;
            disp_border.SetInput(0, &xform_out, true);

            let disp_g = self.dc.CreateEffect(&CLSID_D2D1DisplacementMap).ok()?;
            let cx = (D2D1_CHANNEL_SELECTOR_R.0 as u32).to_le_bytes();
            let cy = (D2D1_CHANNEL_SELECTOR_G.0 as u32).to_le_bytes();
            let _ = disp_g.SetValue(
                D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &cx,
            );
            let _ = disp_g.SetValue(
                D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &cy,
            );
            let border_out = disp_border.GetOutput().ok()?;
            disp_g.SetInput(1, &border_out, true);
            let output = disp_g.GetOutput().ok()?;

            Some(GlassEffectSlot {
                disp_xform,
                disp_border,
                disp_g,
                output,
                bound_bg_ptr: 0,
                bound_disp_ptr: 0,
                last_xf_bytes: [0u8; 24],
                last_scale_bytes: [0u8; 4],
                state_valid: false,
            })
        }
    }

    fn draw_liquid_glass_backdrop_only(
        &mut self,
        rect: RectF,
        radius: f32,
        sigma: f32,
        _core_zoom: f32,
        _unused_dispersion: bool,
    ) -> bool {
        if rect.w <= 2.0 || rect.h <= 2.0 || self.blur_effect.is_none() {
            return false;
        }
        if self.width_px < 4 || self.height_px < 4 {
            return false;
        }

        let s = self.dpi_scale.max(1.0);
        let rad = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
        let rx_px = rect.x * s;
        let ry_px = rect.y * s;
        let rw_px = (rect.w * s).max(2.0);
        let rh_px = (rect.h * s).max(2.0);
        let rad_px = rad * s;

        // Solid-BG_PRIMARY (+ tb_guard_a scrim) top-bar fast path: exact CPU vibrancy (`sat=1.26, con=1.03, lift=0.012`),
        // skipping snapshot copy, blur, displacement, and ImageBrush/PushLayer completely!
        if self.glass_snapshot_active && (ry_px + rh_px) <= (self.glass_strip_top_px as f32) + 2.0 {
            if let Some(solid_bg) = self.glass_top_solid_bg {
                let lum = 0.2126 * solid_bg.r + 0.7152 * solid_bg.g + 0.0722 * solid_bg.b;
                let vib_ch = |c: f32| -> f32 {
                    ((c * 1.26 + (1.0 - 1.26) * lum) * 1.03 + 0.012).clamp(0.0, 1.0)
                };
                let vib_col = Color {
                    r: vib_ch(solid_bg.r),
                    g: vib_ch(solid_bg.g),
                    b: vib_ch(solid_bg.b),
                    a: solid_bg.a,
                };
                if rad >= rect.w.min(rect.h) * 0.49 && (rect.w - rect.h).abs() < 2.0 {
                    self.fill_circle(rect.center_x(), rect.center_y(), rect.w * 0.5, vib_col);
                } else {
                    self.fill_rounded_rect(rect, rad, vib_col);
                }
                self.bump_stat(|st| st.glass_solid_fastpath_draws += 1);
                return true;
            }
        }

        let disp_map = self.ensure_glass_displacement_map(rw_px, rh_px, rad_px);

        // Identify which pre-blurred batch snapshot image covers `rect` (0 = none, 1 = top strip, 2 = bot strip, 3 = full)
        let batch_region: u8 = if !self.glass_snapshot_active {
            0
        } else if self.glass_snapshot_strip_mode {
            if self.glass_strip_top_px > 0 && (ry_px + rh_px) <= (self.glass_strip_top_px as f32) + 2.0 {
                let x_ok = if self.glass_top_x_span_count > 0 {
                    let n = (self.glass_top_x_span_count as usize).min(2);
                    self.glass_top_x_spans_px[..n]
                        .iter()
                        .any(|&(l, r)| rx_px >= (l as f32) - 2.0 && (rx_px + rw_px) <= (r as f32) + 2.0)
                } else {
                    true
                };
                if x_ok && self.glass_top_blurred.is_some() && self.glass_bg_scale_effect.is_some() {
                    1
                } else {
                    0
                }
            } else if self.glass_strip_bot_px < self.height_px
                && ry_px >= (self.glass_strip_bot_px as f32) - 2.0
            {
                let x_ok = if let Some((bl, br)) = self.glass_bot_x_span_px {
                    rx_px >= (bl as f32) - 2.0 && (rx_px + rw_px) <= (br as f32) + 2.0
                } else {
                    true
                };
                if x_ok
                    && self.glass_bot_blurred.is_some()
                    && self.glass_bot_bg_scale_effect.is_some()
                {
                    2
                } else {
                    0
                }
            } else {
                0
            }
        } else if self.glass_blurred_snapshot.is_some()
            && self.glass_bg_scale_effect.is_some()
            && self.glass_half_w > 0
            && self.glass_half_h > 0
        {
            3
        } else {
            0
        };
        let use_batch_snapshot = batch_region != 0;

        match batch_region {
            1 if self.glass_batch_bounded_top.is_none() => self.ensure_glass_batch_bounded_top(false),
            2 if self.glass_batch_bounded_bot.is_none() => self.ensure_glass_batch_bounded_bot(false),
            3 if self.glass_batch_bounded_bg.is_none() => self.ensure_glass_batch_bounded_bg(false),
            _ => {}
        }

        unsafe {
            let (bounded_bg_img, disp_ox_px, disp_oy_px) = if use_batch_snapshot {
                let bounded_opt = match batch_region {
                    1 => self.glass_batch_bounded_top.clone(),
                    2 => self.glass_batch_bounded_bot.clone(),
                    _ => self.glass_batch_bounded_bg.clone(),
                };
                let Some(bounded) = bounded_opt else {
                    return false;
                };
                (bounded, rx_px, ry_px)
            } else {
                // Tight local 0.5x downsampled capture for popovers / context menus drawn outside snapshot
                // (4x fewer blur pixels & 1 flush instead of 2!)
                let pad_px = (20.0 * s).ceil();
                let px_l = ((rx_px - pad_px).floor() as i32)
                    .clamp(0, self.width_px as i32 - 1) as u32;
                let px_t = ((ry_px - pad_px).floor() as i32)
                    .clamp(0, self.height_px as i32 - 1) as u32;
                let px_r = ((rx_px + rw_px + pad_px).ceil() as i32)
                    .clamp(px_l as i32, self.width_px as i32) as u32;
                let px_b = ((ry_px + rh_px + pad_px).ceil() as i32)
                    .clamp(px_t as i32, self.height_px as i32) as u32;
                if px_r <= px_l + 1 || px_b <= px_t + 1 {
                    return false;
                }
                let cap_w = px_r - px_l;
                let cap_h = px_b - px_t;
                let loc_half_w = (cap_w / 2).max(16);
                let loc_half_h = (cap_h / 2).max(16);

                let Some(active_tgt) = self.active_draw_target().cloned() else {
                    return false;
                };
                if self.glass_capture_bmp.is_none()
                    || cap_w > self.glass_cap_w
                    || cap_h > self.glass_cap_h
                {
                    let alloc_w = ((cap_w + 63) & !63).clamp(128, self.width_px.max(128));
                    let alloc_h = ((cap_h + 63) & !63).clamp(64, self.height_px.max(64));
                    let alpha_mode = if self.premultiplied_swapchain {
                        D2D1_ALPHA_MODE_PREMULTIPLIED
                    } else {
                        D2D1_ALPHA_MODE_IGNORE
                    };
                    let props = D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: alpha_mode,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    };
                    if let Ok(bmp) = self.dc.CreateBitmap(
                        D2D_SIZE_U {
                            width: alloc_w,
                            height: alloc_h,
                        },
                        None,
                        0,
                        &props,
                    ) {
                        self.glass_cap_w = alloc_w;
                        self.glass_cap_h = alloc_h;
                        self.glass_capture_bmp = Some(bmp);
                        self.gpu_staging_dirty = true;
                    }
                }
                if self.glass_local_half_blurred.is_none()
                    || self.glass_local_half_w != loc_half_w
                    || self.glass_local_half_h != loc_half_h
                {
                    let half_props = D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    };
                    let hsz = D2D_SIZE_U {
                        width: loc_half_w,
                        height: loc_half_h,
                    };
                    if let Ok(hblur) = self.dc.CreateBitmap(hsz, None, 0, &half_props) {
                        self.glass_local_half_w = loc_half_w;
                        self.glass_local_half_h = loc_half_h;
                        self.glass_local_half_blurred = Some(hblur);
                        self.gpu_staging_dirty = true;
                    }
                }
                let Some(ref cap_bmp) = self.glass_capture_bmp else {
                    return false;
                };
                if cap_w > self.glass_cap_w || cap_h > self.glass_cap_h {
                    return false;
                }
                self.flush_dc();
                self.glass_local_fallback_pending_flush.set(false);
                self.bump_stat(|st| st.glass_snapshot_flushes += 1);
                let src_u = D2D_RECT_U {
                    left: px_l,
                    top: px_t,
                    right: px_r,
                    bottom: px_b,
                };
                let dst_pt = D2D_POINT_2U { x: 0, y: 0 };
                if cap_bmp
                    .CopyFromBitmap(Some(&dst_pt), &active_tgt, Some(&src_u))
                    .is_err()
                {
                    return false;
                }

                let Some(ref blur) = self.blur_effect else {
                    return false;
                };
                let sigma_px = ((sigma * 0.22).clamp(3.2, 4.8) * s).clamp(3.2, 16.0);
                let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_SPEED.0 as u32;
                let border_mode = D2D1_BORDER_MODE_HARD.0 as u32;
                let vib_mat = mat5x4_bytes(Self::build_glass_vibrancy_matrix(1.26, 1.03, 0.012));

                let core_img: Option<ID2D1Image> = if let (
                    Some(ref hblur),
                    Some(ref loc_scale),
                    Some(ref loc_down),
                    Some(ref crop),
                ) = (
                    &self.glass_local_half_blurred,
                    &self.glass_local_scale_effect,
                    &self.glass_local_downscale_effect,
                    &self.glass_crop_effect,
                ) {
                    let mut saved_xf_loc = Matrix3x2::identity();
                    self.dc.GetTransform(&mut saved_xf_loc);
                    self.suspend_dirty_clip();

                    // Detach downstream upscale/border before binding `hblur` as render target
                    loc_scale.SetInput(0, None::<&ID2D1Image>, true);
                    if let Some(ref brd) = self.glass_local_border_effect {
                        brd.SetInput(0, None::<&ID2D1Image>, true);
                    }

                    // 1. Single-pass 0.5x crop -> downscale -> GaussianBlur -> vibrancy into `hblur`
                    // (4x fewer blur pixels, zero intermediate RT!)
                    crop.SetInput(0, cap_bmp, true);
                    let cb = vec4_bytes([0.0, 0.0, cap_w as f32, cap_h as f32]);
                    let _ = crop.SetValue(
                        D2D1_CROP_PROP_RECT.0 as u32,
                        D2D1_PROPERTY_TYPE_VECTOR4,
                        &cb,
                    );
                    let down_xf = mat3x2_bytes(Matrix3x2 {
                        M11: (loc_half_w as f32) / (cap_w as f32),
                        M12: 0.0,
                        M21: 0.0,
                        M22: (loc_half_h as f32) / (cap_h as f32),
                        M31: 0.0,
                        M32: 0.0,
                    });
                    if let Ok(cropped_img) = crop.GetOutput() {
                        loc_down.SetInput(0, &cropped_img, true);
                    } else {
                        loc_down.SetInput(0, cap_bmp, true);
                    }
                    let _ = loc_down.SetValue(
                        D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                        D2D1_PROPERTY_TYPE_MATRIX_3X2,
                        &down_xf,
                    );

                    let half_rc = D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: loc_half_w as f32,
                        bottom: loc_half_h as f32,
                    };
                    self.dc.SetTarget(hblur);
                    self.dc.SetDpi(96.0, 96.0);
                    self.dc.SetTransform(&Matrix3x2::identity());
                    let sigma_half = (sigma_px * 0.5).clamp(1.6, 8.0);
                    if let Ok(down_out) = loc_down.GetOutput() {
                        blur.SetInput(0, &down_out, true);
                        let _ = blur.SetValue(
                            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                            D2D1_PROPERTY_TYPE_FLOAT,
                            &sigma_half.to_le_bytes(),
                        );
                        let _ = blur.SetValue(
                            D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                            D2D1_PROPERTY_TYPE_UINT32,
                            &opt.to_le_bytes(),
                        );
                        let _ = blur.SetValue(
                            D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                            D2D1_PROPERTY_TYPE_UINT32,
                            &border_mode.to_le_bytes(),
                        );
                        self.bump_stat(|st| st.glass_snapshot_blurs += 1);
                        let blurred_half = if let (Some(ref vib), Ok(b_out)) =
                            (&self.glass_vibrancy_effect, blur.GetOutput())
                        {
                            vib.SetInput(0, &b_out, true);
                            let _ = vib.SetValue(
                                D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                                D2D1_PROPERTY_TYPE_BLOB,
                                &vib_mat,
                            );
                            vib.GetOutput().ok().or(Some(b_out))
                        } else {
                            blur.GetOutput().ok()
                        };
                        if let Some(ref b_img) = blurred_half {
                            self.dc.DrawImage(
                                b_img,
                                Some(&pt2(0.0, 0.0)),
                                Some(&half_rc),
                                D2D1_INTERPOLATION_MODE_LINEAR,
                                windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_COPY,
                            );
                        }
                    }
                    // Commit `hblur` before `loc_scale` (ID2D1Effect) reads it on `active_tgt`!
                    self.flush_dc();

                    self.dc.SetTarget(&active_tgt);
                    let main_dpi = 96.0 * s;
                    self.dc.SetDpi(main_dpi, main_dpi);
                    self.dc.SetTransform(&saved_xf_loc);
                    self.resume_dirty_clip();

                    // 2. Hardware bilinear upscale back to `(cap_w, cap_h)` via isolated `glass_local_scale_effect`
                    let up_xf = Matrix3x2 {
                        M11: (cap_w as f32) / (loc_half_w as f32),
                        M12: 0.0,
                        M21: 0.0,
                        M22: (cap_h as f32) / (loc_half_h as f32),
                        M31: 0.0,
                        M32: 0.0,
                    };
                    loc_scale.SetInput(0, hblur, true);
                    let xf_b = mat3x2_bytes(up_xf);
                    let _ = loc_scale.SetValue(
                        D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                        D2D1_PROPERTY_TYPE_MATRIX_3X2,
                        &xf_b,
                    );
                    loc_scale.GetOutput().ok()
                } else {
                    // Fallback full-res path if local half targets couldn't be allocated
                    let blur_input_img: ID2D1Image = if let Some(ref crop) = self.glass_crop_effect {
                        crop.SetInput(0, cap_bmp, true);
                        let cb = vec4_bytes([0.0, 0.0, cap_w as f32, cap_h as f32]);
                        let _ = crop.SetValue(
                            D2D1_CROP_PROP_RECT.0 as u32,
                            D2D1_PROPERTY_TYPE_VECTOR4,
                            &cb,
                        );
                        crop.GetOutput().unwrap_or_else(|_| cap_bmp.cast().unwrap())
                    } else {
                        cap_bmp.cast().unwrap()
                    };
                    blur.SetInput(0, &blur_input_img, true);
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                        D2D1_PROPERTY_TYPE_FLOAT,
                        &sigma_px.to_le_bytes(),
                    );
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                        D2D1_PROPERTY_TYPE_UINT32,
                        &opt.to_le_bytes(),
                    );
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                        D2D1_PROPERTY_TYPE_UINT32,
                        &border_mode.to_le_bytes(),
                    );
                    self.bump_stat(|st| st.glass_snapshot_blurs += 1);
                    if let (Some(ref vib), Ok(blur_out)) =
                        (&self.glass_vibrancy_effect, blur.GetOutput())
                    {
                        vib.SetInput(0, &blur_out, true);
                        let _ = vib.SetValue(
                            D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                            D2D1_PROPERTY_TYPE_BLOB,
                            &vib_mat,
                        );
                        vib.GetOutput().ok().or(Some(blur_out))
                    } else {
                        blur.GetOutput().ok()
                    }
                };
                let Some(core_img) = core_img else {
                    return false;
                };
                // Isolate fallback border effect from batch `self.glass_border_effect`!
                let bounded: ID2D1Image = if let Some(ref brd) = self.glass_local_border_effect {
                    brd.SetInput(0, &core_img, true);
                    brd.GetOutput().unwrap_or(core_img)
                } else {
                    core_img
                };
                (bounded, rx_px - (px_l as f32), ry_px - (px_t as f32))
            };

            // Single ultra-fast FP16 SDF displacement pass (all in 96.0 DPI physical pixels!)
            let final_img: ID2D1Image = if let Some((ref disp_bmp, map_w, map_h)) = disp_map {
                let map_xf = Matrix3x2 {
                    M11: rw_px / (map_w as f32).max(1.0),
                    M12: 0.0,
                    M21: 0.0,
                    M22: rh_px / (map_h as f32).max(1.0),
                    M31: disp_ox_px,
                    M32: disp_oy_px,
                };
                let xf_bytes = mat3x2_bytes(map_xf);
                let bezel_w_dip = (rect.w.min(rect.h) * 0.24).clamp(6.0, 20.0);
                let max_deflect_px = (bezel_w_dip * 0.90).clamp(5.5, 16.0) * s;
                let disp_scale_px = max_deflect_px * 2.0;
                let scale_bytes = disp_scale_px.to_le_bytes();

                let mut pooled_out: Option<ID2D1Image> = None;
                if use_batch_snapshot {
                    if self.glass_effect_cursor == self.glass_effect_pool.len()
                        && self.glass_effect_pool.len() < GLASS_EFFECT_POOL_SIZE
                    {
                        if let Some(new_slot) = self.create_glass_effect_slot() {
                            self.glass_effect_pool.push(new_slot);
                        }
                    }
                    if self.glass_effect_cursor < self.glass_effect_pool.len() {
                        let slot = &mut self.glass_effect_pool[self.glass_effect_cursor];
                        self.glass_effect_cursor += 1;
                        let bg_ptr = bounded_bg_img.as_raw() as usize;
                        let disp_ptr = disp_bmp.as_raw() as usize;
                        let mut rebinds = 0u32;
                        if !slot.state_valid || slot.bound_disp_ptr != disp_ptr {
                            slot.disp_xform.SetInput(0, disp_bmp, true);
                            slot.bound_disp_ptr = disp_ptr;
                            rebinds += 1;
                        }
                        if !slot.state_valid || slot.last_xf_bytes != xf_bytes {
                            let _ = slot.disp_xform.SetValue(
                                D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                                D2D1_PROPERTY_TYPE_MATRIX_3X2,
                                &xf_bytes,
                            );
                            slot.last_xf_bytes = xf_bytes;
                            rebinds += 1;
                        }
                        if !slot.state_valid || slot.bound_bg_ptr != bg_ptr {
                            slot.disp_g.SetInput(0, &bounded_bg_img, true);
                            slot.bound_bg_ptr = bg_ptr;
                            rebinds += 1;
                        }
                        if !slot.state_valid || slot.last_scale_bytes != scale_bytes {
                            let _ = slot.disp_g.SetValue(
                                D2D1_DISPLACEMENTMAP_PROP_SCALE.0 as u32,
                                D2D1_PROPERTY_TYPE_FLOAT,
                                &scale_bytes,
                            );
                            slot.last_scale_bytes = scale_bytes;
                            rebinds += 1;
                        }
                        slot.state_valid = true;
                        pooled_out = Some(slot.output.clone());
                        if rebinds > 0 {
                            self.bump_stat(|st| st.glass_effect_rebinds += rebinds);
                        }
                    }
                }

                if let Some(out) = pooled_out {
                    out
                } else if let (Some(ref xform), Some(ref disp_g)) =
                    (&self.glass_disp_xform_effect, &self.glass_disp_g_effect)
                {
                    if use_batch_snapshot {
                        if self.glass_effect_overflow_used || self.glass_local_fallback_pending_flush.get() {
                            let _ = self.dc.Flush(None, None);
                            self.glass_local_fallback_pending_flush.set(false);
                            self.bump_stat(|st| st.glass_snapshot_flushes += 1);
                        }
                        self.glass_effect_overflow_used = true;
                    }
                    xform.SetInput(0, disp_bmp, true);
                    let _ = xform.SetValue(
                        D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                        D2D1_PROPERTY_TYPE_MATRIX_3X2,
                        &xf_bytes,
                    );
                    if let Ok(disp_aligned) = xform.GetOutput() {
                        // Edge-clamp Input 1 so sub-pixel perimeter sampling NEVER sees (0,0,0,0) tear!
                        let disp_clamped: ID2D1Image =
                            if let Some(ref d_brd) = self.glass_disp_border_effect {
                                d_brd.SetInput(0, &disp_aligned, true);
                                d_brd.GetOutput().unwrap_or(disp_aligned)
                            } else {
                                disp_aligned
                            };
                        disp_g.SetInput(0, &bounded_bg_img, true);
                        disp_g.SetInput(1, &disp_clamped, true);
                        let _ = disp_g.SetValue(
                            D2D1_DISPLACEMENTMAP_PROP_SCALE.0 as u32,
                            D2D1_PROPERTY_TYPE_FLOAT,
                            &scale_bytes,
                        );
                        self.bump_stat(|st| st.glass_effect_rebinds += 3);
                        disp_g.GetOutput().unwrap_or(bounded_bg_img)
                    } else {
                        bounded_bg_img
                    }
                } else {
                    bounded_bg_img
                }
            } else {
                bounded_bg_img
            };

            let rect_px = D2D_RECT_F {
                left: rx_px,
                top: ry_px,
                right: rx_px + rw_px,
                bottom: ry_px + rh_px,
            };
            let rr_px = D2D1_ROUNDED_RECT {
                rect: rect_px,
                radiusX: rad_px,
                radiusY: rad_px,
            };
            let src_disp_rc = D2D_RECT_F {
                left: disp_ox_px,
                top: disp_oy_px,
                right: disp_ox_px + rw_px,
                bottom: disp_oy_px + rh_px,
            };

            if self.glass_image_brush.is_none() {
                let ib_props = D2D1_IMAGE_BRUSH_PROPERTIES {
                    sourceRectangle: src_disp_rc,
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
                };
                if let Ok(ib) = self.dc.CreateImageBrush(&final_img, &ib_props, None) {
                    self.glass_image_brush = Some(ib);
                }
            }

            // Switch temporarily to 96.0 DPI physical pixel space (preserving any active transform!)
            let mut saved_xf = Matrix3x2::identity();
            self.dc.GetTransform(&mut saved_xf);
            let xf_px = Matrix3x2 {
                M11: saved_xf.M11,
                M12: saved_xf.M12,
                M21: saved_xf.M21,
                M22: saved_xf.M22,
                M31: saved_xf.M31 * s,
                M32: saved_xf.M32 * s,
            };
            self.dc.SetDpi(96.0, 96.0);
            self.dc.SetTransform(&xf_px);

            if let Some(ref img_brush) = self.glass_image_brush {
                img_brush.SetImage(&final_img);
                img_brush.SetSourceRectangle(&src_disp_rc);
                img_brush.SetExtendModeX(D2D1_EXTEND_MODE_CLAMP);
                img_brush.SetExtendModeY(D2D1_EXTEND_MODE_CLAMP);
                img_brush.SetInterpolationMode(D2D1_INTERPOLATION_MODE_LINEAR);
                img_brush.SetTransform(&Matrix3x2::translation(rx_px, ry_px));
                if rad_px >= rw_px.min(rh_px) * 0.49 && (rw_px - rh_px).abs() < 2.0 {
                    let el_px = D2D1_ELLIPSE {
                        point: pt2(rx_px + rw_px * 0.5, ry_px + rh_px * 0.5),
                        radiusX: rw_px * 0.5,
                        radiusY: rh_px * 0.5,
                    };
                    self.dc.FillEllipse(&el_px, img_brush);
                } else {
                    self.dc.FillRoundedRectangle(&rr_px, img_brush);
                }
            } else if let Ok(outer_geom) = self.d2d_factory.CreateRoundedRectangleGeometry(&rr_px) {
                let layer_params = D2D1_LAYER_PARAMETERS1 {
                    contentBounds: rect_px,
                    geometricMask: std::mem::ManuallyDrop::new(Some(outer_geom.cast().unwrap())),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2::identity(),
                    opacity: 1.0,
                    opacityBrush: std::mem::ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                };
                self.dc.PushLayer(&layer_params, None);
                self.dc.DrawImage(
                    &final_img,
                    Some(&pt2(rx_px, ry_px)),
                    Some(&src_disp_rc),
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
                self.dc.PopLayer();
                let _ = std::mem::ManuallyDrop::into_inner(layer_params.geometricMask);
                self.bump_stat(|st| st.glass_push_layers += 1);
            } else {
                let main_dpi = 96.0 * s;
                self.dc.SetDpi(main_dpi, main_dpi);
                self.dc.SetTransform(&saved_xf);
                return false;
            }

            let main_dpi = 96.0 * s;
            self.dc.SetDpi(main_dpi, main_dpi);
            self.dc.SetTransform(&saved_xf);

            if !use_batch_snapshot {
                // Defer flushing until another local fallback / overflow draw reuses the shared local effects
                // (or until EndDraw), cutting single-popover flushes from 2 down to 1!
                self.glass_local_fallback_pending_flush.set(true);
            }
            true
        }
    }

    /// Authentic iPadOS 26 Crystal-Clear Liquid Glass Surface:
    /// - HiDPI-exact `96 DPI` physical-pixel FP16 C2 quintic SDF optical lens refraction
    /// - Single per-frame half-res pre-blurred snapshot (< 1.0ms/frame across all glass elements!)
    /// - Crystal-clear translucent glaze (`alpha ~0.16..0.24` on floating capsules) + `1.26x` vibrancy
    /// - Strictly VERTICAL top & bottom specular rim highlights (`0.44` top / `0.06` sides / `0.34` bottom)
    /// - Lightweight 2-tier exterior-only drop shadow
    pub fn draw_liquid_glass(
        &mut self,
        rect: RectF,
        radius: f32,
        sigma: f32,
        tint: Color,
        rim_color: Color,
        reduce_transparency: bool,
    ) {
        if rect.w <= 1.0 || rect.h <= 1.0 {
            return;
        }
        let rad = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
        if reduce_transparency || self.blur_effect.is_none() || self.target_bitmap.is_none() {
            self.fill_rounded_rect(rect, rad, Color::rgba(30, 30, 36, 0.96));
            self.stroke_rounded_rect(rect, rad, Color::rgba(255, 255, 255, 0.10), 1.0);
            return;
        }

        let refracted_drawn = self.draw_liquid_glass_backdrop_only(rect, rad, sigma, 1.0, false);
        if refracted_drawn {
            self.record_live_glass_rect(rect);
            if rad > 2.0 {
                self.draw_exterior_glass_shadow(rect, rad, 0.28, 0.14, 4.0);
            }
        }

        if refracted_drawn {
            // Crystal-clear iPadOS 26 translucent glaze: honors restrained `tint.a` (~0.16..0.24
            // for MiniPlayer pill, ~0.40..0.46 for context menus/popovers, ~0.78 for modals)
            let base_a = tint.a.clamp(0.15, 0.78);
            let top_glaze = tint.with_alpha((base_a * 0.82).clamp(0.12, 0.70));
            let bot_glaze = tint.with_alpha((base_a * 1.12).clamp(0.18, 0.82));
            self.fill_linear_gradient_rect(
                rect,
                rad,
                rect.x,
                rect.y,
                rect.x,
                rect.bottom(),
                top_glaze,
                bot_glaze,
            );
        } else {
            self.fill_rounded_rect(rect, rad, Theme::GLASS_OPAQUE_FALLBACK);
        }

        // Strictly VERTICAL top & bottom iPadOS 26 specular rim highlights (cached brushes!)
        let rim_op = (rim_color.a / 0.44).clamp(0.75, 1.25);
        self.draw_vertical_glass_rim(rect, rad, 0.0, rim_op, 0.28);
    }

    // ========================================================================
    // Typography & Exact DirectWrite Layout / Syllable Hit-Testing
    // ========================================================================

    /// Validates the bundled OFL Inter Variable TTF magic header (`00 01 00 00`) and non-empty payload.
    pub fn verify_inter_font_bundle() -> bool {
        INTER_FONT_BYTES.len() > 100_000 && INTER_FONT_BYTES.starts_with(&[0x00, 0x01, 0x00, 0x00])
    }

    /// Returns `true` when the bundled OFL Inter Variable font was registered in-memory with DirectWrite.
    pub fn has_bundled_inter_font(&self) -> bool {
        self.bundled_inter_loaded
    }

    /// Active primary UI font family (`"Inter Variable Text"` when bundled Inter loaded, else `"Segoe UI Variable Display"`).
    pub fn active_ui_font_family(&self) -> &str {
        &self.bundled_inter_family
    }

    /// Font family used by `draw_mono_text` (`mono: true`) — uses Inter with OpenType `tnum=1` + `zero=0`
    /// tabular unslashed figures (NEVER `"Consolas"`!).
    pub fn mono_font_family(&self) -> &str {
        &self.bundled_inter_family
    }

    /// Verifies that DirectWrite creates Inter-backed `IDWriteTextFormat` instances across all
    /// standard UI weights (`400`, `500`, `600`, `700`) for both regular and `mono` text paths.
    pub fn verify_inter_formats_all_weights(&mut self) -> bool {
        if !self.bundled_inter_loaded {
            return false;
        }
        for weight in [400u16, 500, 600, 700] {
            for mono in [false, true] {
                let Some((fmt, _)) =
                    self.get_or_create_format(14.0, weight, TextAlign::Left, true, false, mono)
                else {
                    return false;
                };
                unsafe {
                    let len = fmt.GetFontFamilyNameLength();
                    if len == 0 {
                        return false;
                    }
                    let mut buf = vec![0u16; (len as usize) + 1];
                    if fmt.GetFontFamilyName(&mut buf).is_err() {
                        return false;
                    }
                    let fam = String::from_utf16_lossy(&buf[..(len as usize)]);
                    if !fam.contains("Inter") {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn get_or_create_format(
        &mut self,
        size: f32,
        weight: u16,
        align: TextAlign,
        vcenter: bool,
        wrap: bool,
        mono: bool,
    ) -> Option<(IDWriteTextFormat, IDWriteInlineObject)> {
        let key = FontKey {
            size_tenths: (size * 10.0).round().clamp(60.0, 1200.0) as u16,
            weight,
            align,
            vcenter,
            wrap,
            mono,
        };
        if let (Some(fmt), Some(ell)) = (self.font_cache.get(&key), self.ellipsis_cache.get(&key)) {
            return Some((fmt.clone(), ell.clone()));
        }

        unsafe {
            // Both regular UI text and `mono: true` timestamps use bundled OFL Inter (with OpenType
            // `tnum=1` + `zero=0` applied on `IDWriteTextLayout` for `mono: true`), NEVER `"Consolas"`!
            let primary_family = HSTRING::from(self.bundled_inter_family.as_str());
            let locale = HSTRING::from("zh-CN");
            let font_coll = self.custom_font_collection.as_ref();
            let fmt = self
                .dwrite
                .CreateTextFormat(
                    &primary_family,
                    font_coll,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    &locale,
                )
                .or_else(|_| {
                    let fb1 = HSTRING::from("Segoe UI Variable Display");
                    self.dwrite.CreateTextFormat(
                        &fb1,
                        None,
                        DWRITE_FONT_WEIGHT(weight as i32),
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        &locale,
                    )
                })
                .or_else(|_| {
                    let fb2 = HSTRING::from("Microsoft YaHei UI");
                    self.dwrite.CreateTextFormat(
                        &fb2,
                        None,
                        DWRITE_FONT_WEIGHT(weight as i32),
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        &locale,
                    )
                })
                .ok()?;

            // Guarantee seamless Windows CJK / Emoji fallback alongside our merged Inter font collection
            if let Some(ref sys_fb) = self.system_font_fallback {
                if let Ok(fmt1) = fmt.cast::<IDWriteTextFormat1>() {
                    let _ = fmt1.SetFontFallback(sys_fb);
                }
            }
            // Enable automatic optical sizing (`opsz` 14..32) on Inter Variable across UI labels & large titles
            if let Ok(fmt3) = fmt.cast::<IDWriteTextFormat3>() {
                let _ = fmt3.SetAutomaticFontAxes(DWRITE_AUTOMATIC_FONT_AXES_OPTICAL_SIZE);
            }

            let _ = fmt.SetTextAlignment(match align {
                TextAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                TextAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                TextAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = fmt.SetParagraphAlignment(if vcenter {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            });
            let _ = fmt.SetWordWrapping(if wrap {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            });

            let ell = self.dwrite.CreateEllipsisTrimmingSign(&fmt).ok()?;
            if !wrap {
                let trim = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = fmt.SetTrimming(&trim, &ell);
            }

            self.font_cache.insert(key.clone(), fmt.clone());
            self.ellipsis_cache.insert(key, ell.clone());
            Some((fmt, ell))
        }
    }

    pub fn draw_text(
        &mut self,
        text: &str,
        rect: RectF,
        size: f32,
        weight: u16,
        color: Color,
        align: TextAlign,
        vcenter: bool,
        wrap: bool,
    ) {
        self.draw_text_ex(text, rect, size, weight, color, align, vcenter, wrap, false);
    }

    /// Returns `true` ONLY for truly ticking numeric readouts (`-mm:ss` countdown timers and live
    /// telemetry readouts like `143.8 FPS`, `-12.4 dB`, `6.9 ms`, `-14.2 LUFS`) so static track
    /// durations (`"3:42"`), row indices (`"1"`), sample rates (`"44.1 kHz"`), and sync offsets
    /// (`"0.0s"`) remain cached in `text_layout_cache`!
    #[inline]
    pub fn is_volatile_telemetry_mono_text(text: &str) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.len() > 28 {
            return false;
        }
        let bytes = trimmed.as_bytes();
        // Remaining-time countdown readouts: "-03:45", "-1:02:03"
        if bytes[0] == b'-'
            && bytes.len() >= 4
            && bytes.contains(&b':')
            && bytes[1..].iter().all(|b| b.is_ascii_digit() || *b == b':')
        {
            return true;
        }
        // Live numeric telemetry units (FPS / dB / dBFS / LUFS / LKFS / ms)
        if bytes.iter().any(|b| b.is_ascii_digit()) {
            let is_live_unit = trimmed.ends_with("dB")
                || trimmed.ends_with("dBFS")
                || trimmed.ends_with("LUFS")
                || trimmed.ends_with("LKFS")
                || trimmed.ends_with("FPS")
                || trimmed.ends_with("fps")
                || trimmed.ends_with("ms")
                || trimmed.ends_with("MS");
            if is_live_unit {
                return true;
            }
        }
        false
    }

    pub fn draw_mono_text(
        &mut self,
        text: &str,
        rect: RectF,
        size: f32,
        weight: u16,
        color: Color,
        align: TextAlign,
        vcenter: bool,
    ) {
        self.draw_text_ex(text, rect, size, weight, color, align, vcenter, false, true);
    }

    /// Explicit cache-bypassing monospace draw for ticking playback timers (`01:23`, `-03:45`) and
    /// live numeric meters so they never allocate or evict entries in `text_layout_cache`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_mono_text_volatile(
        &mut self,
        text: &str,
        rect: RectF,
        size: f32,
        weight: u16,
        color: Color,
        align: TextAlign,
        vcenter: bool,
    ) {
        self.draw_text_ex_inner(text, rect, size, weight, color, align, vcenter, false, true, true);
    }

    pub fn draw_text_ex(
        &mut self,
        text: &str,
        rect: RectF,
        size: f32,
        weight: u16,
        color: Color,
        align: TextAlign,
        vcenter: bool,
        wrap: bool,
        mono: bool,
    ) {
        let bypass_cache = mono && Self::is_volatile_telemetry_mono_text(text);
        self.draw_text_ex_inner(
            text,
            rect,
            size,
            weight,
            color,
            align,
            vcenter,
            wrap,
            mono,
            bypass_cache,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text_ex_inner(
        &mut self,
        text: &str,
        rect: RectF,
        size: f32,
        weight: u16,
        color: Color,
        align: TextAlign,
        vcenter: bool,
        wrap: bool,
        mono: bool,
        bypass_cache: bool,
    ) {
        if text.is_empty() || rect.w <= 1.0 || rect.h <= 1.0 || color.a <= 0.001 {
            return;
        }
        let translated = crate::i18n::translate_ui_text(text);
        let text = translated.as_ref();
        let size_tenths = (size * 10.0).round().clamp(60.0, 1200.0) as u16;
        let w_q = (rect.w * 2.0).round().clamp(2.0, 16_000.0) as u32;
        let h_q = (rect.h * 2.0).round().clamp(2.0, 8_000.0) as u32;

        // Fast 64-bit FNV-1a key over text + typography + quantized layout box
        let mut h64: u64 = 0xcbf2_9ce4_8422_2325;
        if !bypass_cache {
            for &b in text.as_bytes() {
                h64 ^= b as u64;
                h64 = h64.wrapping_mul(0x0100_0000_01b3);
            }
            let meta_bits: u64 = ((size_tenths as u64) << 48)
                | ((weight as u64) << 32)
                | ((w_q as u64 & 0xFFFF) << 16)
                | (h_q as u64 & 0x0FFF)
                | ((align as u64) << 12)
                | ((vcenter as u64) << 14)
                | ((wrap as u64) << 15)
                | ((mono as u64) << 63);
            h64 ^= meta_bits;
            h64 = h64.wrapping_mul(0x0100_0000_01b3);
        }

        let on_live_glass = color.a > 0.45
            && (color.r + color.g + color.b) > 1.75
            && self.is_point_on_live_glass(rect.center_x(), rect.center_y());

        if !bypass_cache {
            if let Some(cached_layout) = self.text_layout_cache.get_touch(&h64).cloned() {
                unsafe {
                    if on_live_glass {
                        let sh_brush = self.brush(Color::rgba(0, 0, 0, 0.42 * color.a));
                        self.dc.DrawTextLayout(
                            pt2(rect.x, rect.y + 1.0),
                            &cached_layout,
                            sh_brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                        );
                    }
                    let brush = self.brush(color);
                    self.dc.DrawTextLayout(
                        pt2(rect.x, rect.y),
                        &cached_layout,
                        brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );
                }
                return;
            }
        }

        let Some((fmt, _)) = self.get_or_create_format(size, weight, align, vcenter, wrap, mono)
        else {
            return;
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            if let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&utf16, &fmt, (w_q as f32) * 0.5, (h_q as f32) * 0.5)
            {
                if mono {
                    if let Some(ref typo) = self.mono_typography {
                        let _ = layout.SetTypography(
                            typo,
                            DWRITE_TEXT_RANGE {
                                startPosition: 0,
                                length: utf16.len() as u32,
                            },
                        );
                    }
                }
                if on_live_glass {
                    let sh_brush = self.brush(Color::rgba(0, 0, 0, 0.42 * color.a));
                    self.dc.DrawTextLayout(
                        pt2(rect.x, rect.y + 1.0),
                        &layout,
                        sh_brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );
                }
                let brush = self.brush(color);
                self.dc.DrawTextLayout(
                    pt2(rect.x, rect.y),
                    &layout,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
                if !bypass_cache {
                    let max_layouts = if self.aggressive_memory_release { 320 } else { 1024 };
                    self.text_layout_cache.insert(h64, layout, max_layouts);
                }
            } else {
                let brush = self.brush(color);
                self.dc.DrawText(
                    &utf16,
                    &fmt,
                    &to_d2d_rect(rect),
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    Default::default(),
                );
            }
        }
    }

    /// Computes exact DirectWrite sub-pixel X positions (in DIPs) for every UTF-8 char boundary
    /// (`0..=text.len()`) of a single-line text string, preserving trailing whitespace width.
    pub fn compute_single_line_char_xs(
        &mut self,
        text: &str,
        size: f32,
        weight: u16,
    ) -> Vec<(usize, f32)> {
        if text.is_empty() {
            return vec![(0, 0.0)];
        }
        let Some((fmt, _)) = self.get_or_create_format(size, weight, TextAlign::Left, true, false, false)
        else {
            let mut out = Vec::new();
            let mut x = 0.0f32;
            for (b_idx, ch) in text.char_indices() {
                out.push((b_idx, x));
                x += if ch.is_ascii() { size * 0.56 } else { size };
            }
            out.push((text.len(), x));
            return out;
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&utf16, &fmt, 16_000.0, 64.0) else {
                return vec![(0, 0.0), (text.len(), (text.chars().count() as f32) * size * 0.7)];
            };
            let mut out = Vec::with_capacity(text.chars().count() + 1);
            let mut u16_pos = 0u32;
            for (b_idx, ch) in text.char_indices() {
                let mut px = 0.0f32;
                let mut py = 0.0f32;
                let mut htm = DWRITE_HIT_TEST_METRICS::default();
                let _ = layout.HitTestTextPosition(u16_pos, false, &mut px, &mut py, &mut htm);
                out.push((b_idx, px.max(0.0)));
                u16_pos += ch.len_utf16() as u32;
            }
            let mut metrics = DWRITE_TEXT_METRICS::default();
            let end_x = if layout.GetMetrics(&mut metrics).is_ok() {
                metrics.widthIncludingTrailingWhitespace.max(0.0)
            } else {
                out.last().map(|&(_, x)| x + size * 0.6).unwrap_or(0.0)
            };
            out.push((text.len(), end_x));
            out
        }
    }

    pub fn create_lyrics_text_layout(
        &mut self,
        text: &str,
        font_size: f32,
        weight: u16,
        max_width: f32,
        align: TextAlign,
    ) -> Option<(IDWriteTextLayout, f32)> {
        let (fmt, _) = self.get_or_create_format(font_size, weight, align, false, true, false)?;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self
                .dwrite
                .CreateTextLayout(&utf16, &fmt, max_width.max(40.0), 2400.0)
                .ok()?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut metrics);
            Some((layout, metrics.height.max(font_size * 1.18)))
        }
    }

    /// Queries DirectWrite `HitTestTextRange` for the exact bounding boxes of a UTF-16 syllable range!
    pub fn hit_test_syllable_rects(
        layout: &IDWriteTextLayout,
        utf16_start: u32,
        utf16_len: u32,
        origin_x: f32,
        origin_y: f32,
    ) -> Vec<RectF> {
        if utf16_len == 0 {
            return Vec::new();
        }
        unsafe {
            let mut buf = [DWRITE_HIT_TEST_METRICS::default(); 8];
            let mut actual = 0u32;
            let hr = layout.HitTestTextRange(
                utf16_start,
                utf16_len,
                origin_x,
                origin_y,
                Some(&mut buf),
                &mut actual,
            );
            let mut out = Vec::new();
            if hr.is_ok() {
                for m in &buf[..(actual as usize).min(buf.len())] {
                    if m.width > 0.1 && m.height > 0.1 {
                        out.push(RectF::new(m.left, m.top, m.width, m.height));
                    }
                }
            } else if actual > 8 && actual <= 64 {
                let mut dyn_buf = vec![DWRITE_HIT_TEST_METRICS::default(); actual as usize];
                if layout
                    .HitTestTextRange(
                        utf16_start,
                        utf16_len,
                        origin_x,
                        origin_y,
                        Some(&mut dyn_buf),
                        &mut actual,
                    )
                    .is_ok()
                {
                    for m in &dyn_buf[..(actual as usize).min(dyn_buf.len())] {
                        if m.width > 0.1 && m.height > 0.1 {
                            out.push(RectF::new(m.left, m.top, m.width, m.height));
                        }
                    }
                }
            }
            out
        }
    }

    pub fn set_lyrics_subpixel_text_mode(&mut self, enabled: bool) {
        self.lyrics_subpixel_mode = enabled;
        unsafe {
            if enabled {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            } else {
                self.dc
                    .SetTextRenderingParams(None::<&IDWriteRenderingParams>);
            }
        }
    }

    pub fn get_or_create_lyrics_unit_layout(
        &mut self,
        text: &str,
        font_size: f32,
        max_width: f32,
    ) -> Option<(IDWriteTextLayout, f32, f32, f32)> {
        let trimmed = text.trim();
        let trim_u16 = trimmed.encode_utf16().count() as u32;
        if trimmed.is_empty() || trim_u16 == 0 {
            return None;
        }
        let sz_tenths = (font_size * 10.0).round().clamp(60.0, 1200.0) as u64;
        let mut h64: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in trimmed.as_bytes() {
            h64 ^= b as u64;
            h64 = h64.wrapping_mul(0x0100_0000_01b3);
        }
        h64 ^= sz_tenths << 32;
        h64 = h64.wrapping_mul(0x0100_0000_01b3);
        if let Some(hit) = self.lyrics_unit_layout_cache.get_touch(&h64) {
            return Some(hit.clone());
        }
        let (unit_lay, _) =
            self.create_lyrics_text_layout(trimmed, font_size, 700, max_width, TextAlign::Left)?;
        let u_bounds = Self::hit_test_syllable_rects(&unit_lay, 0, trim_u16, 0.0, 0.0);
        let (ux, uy, uw) = u_bounds
            .first()
            .map(|b| (b.x, b.y, b.w))
            .unwrap_or((0.0, 0.0, font_size * 0.5));
        let entry = (unit_lay, ux, uy, uw);
        self.lyrics_unit_layout_cache.insert(h64, entry.clone(), 256);
        Some(entry)
    }

    pub fn draw_text_layout_solid(&self, layout: &IDWriteTextLayout, x: f32, y: f32, color: Color) {
        if color.a <= 0.001 {
            return;
        }
        unsafe {
            self.dc.DrawTextLayout(
                pt2(x, y),
                layout,
                self.brush(color),
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
    }

    /// Draws `IDWriteTextLayout` without vertical pixel-grid snapping (`D2D1_DRAW_TEXT_OPTIONS_NO_SNAP`)
    /// so sub-pixel vertical spring motion during character/syllable float-up renders fluidly without 1px jumps.
    pub fn draw_text_layout_subpixel(
        &self,
        layout: &IDWriteTextLayout,
        x: f32,
        y: f32,
        color: Color,
    ) {
        if color.a <= 0.001 {
            return;
        }
        unsafe {
            self.dc.DrawTextLayout(
                pt2(x, y),
                layout,
                self.brush(color),
                D2D1_DRAW_TEXT_OPTIONS_NO_SNAP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
    }

    /// Draws `layout` clipped to `clip_rect` with a horizontal feathered karaoke sweep gradient!
    /// Matches `SyncedLyricsViewController.Specs.lineProgressionGradientFeather = 40.0` and RTL direction!
    pub fn draw_text_layout_feathered_sweep(
        &self,
        layout: &IDWriteTextLayout,
        origin_x: f32,
        origin_y: f32,
        clip_rect: RectF,
        progress: f32,
        feather_px: f32,
        is_rtl: bool,
        sung_color: Color,
        upcoming_color: Color,
    ) {
        if clip_rect.w <= 0.2 || clip_rect.h <= 0.2 {
            return;
        }
        self.push_clip(clip_rect);
        if progress <= 0.001 {
            self.draw_text_layout_subpixel(layout, origin_x, origin_y, upcoming_color);
        } else if progress >= 0.999 {
            self.draw_text_layout_subpixel(layout, origin_x, origin_y, sung_color);
        } else {
            let feather = feather_px.min(clip_rect.w * 0.85).max(8.0);
            let total_travel = clip_rect.w + feather;
            let (start_pt, end_pt) = if is_rtl {
                // Right-to-Left karaoke sweep for Arabic / Hebrew lyrics
                let edge_center = (clip_rect.x + clip_rect.w + feather * 0.5) - progress * total_travel;
                let gx0 = edge_center + feather * 0.5;
                let gx1 = (edge_center - feather * 0.5).min(gx0 - 1.0);
                (pt2(gx0, clip_rect.y), pt2(gx1, clip_rect.y))
            } else {
                let edge_center = clip_rect.x - feather * 0.5 + progress * total_travel;
                let gx0 = edge_center - feather * 0.5;
                let gx1 = (edge_center + feather * 0.5).max(gx0 + 1.0);
                (pt2(gx0, clip_rect.y), pt2(gx1, clip_rect.y))
            };
            if let Some(grad) = self.get_or_create_linear_grad_2stop(
                start_pt.X,
                start_pt.Y,
                end_pt.X,
                end_pt.Y,
                sung_color,
                upcoming_color,
            ) {
                unsafe {
                    self.dc.DrawTextLayout(
                        pt2(origin_x, origin_y),
                        layout,
                        &grad,
                        D2D1_DRAW_TEXT_OPTIONS_NO_SNAP
                            | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );
                }
            }
        }
        self.pop_clip();
    }

    /// Draws glowing emission for an active lyrics word/unit where glow across the already-swept
    /// body is subdued (`body_alpha`), peaks strongly at the highlight progress frontier (`peak_alpha`),
    /// and cleanly fades to zero ahead of the frontier (in both LTR and RTL reading directions).
    /// Routed through `linear_grad_cache` via local frontier span `[frontier - back .. frontier + front]`
    /// with `CLAMP` extend mode -> ZERO per-char COM gradient creation!
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_layout_frontier_glow_sweep(
        &self,
        layout: &IDWriteTextLayout,
        origin_x: f32,
        origin_y: f32,
        clip_rect: RectF,
        progress: f32,
        feather_px: f32,
        is_rtl: bool,
        body_alpha: f32,
        peak_alpha: f32,
    ) {
        if clip_rect.w <= 0.2 || clip_rect.h <= 0.2 {
            return;
        }
        let body_a = body_alpha.clamp(0.0, 1.0);
        let peak_a = peak_alpha.clamp(0.0, 1.0);
        if body_a <= 0.005 && peak_a <= 0.005 {
            return;
        }

        self.push_clip(clip_rect);
        if progress <= 0.002 {
            self.pop_clip();
            return;
        }
        if progress >= 0.996 && (peak_a - body_a).abs() <= 0.015 {
            self.draw_text_layout_subpixel(
                layout,
                origin_x,
                origin_y,
                Color::rgba(255, 255, 255, peak_a),
            );
            self.pop_clip();
            return;
        }

        let p = progress.clamp(0.0, 1.0);
        let w = clip_rect.w.max(2.0);
        let band_back = (feather_px * 0.72 / w).clamp(0.12, 0.55);
        let band_front = (feather_px * 0.34 / w).clamp(0.06, 0.32);
        let p_back = (p - band_back).clamp(0.0, 1.0);
        let start_a = if p_back <= 0.001 { peak_a } else { body_a };
        let front_a = if p >= 0.996 { peak_a } else { 0.0 };

        let back_px = (band_back * w).max(1.0);
        let front_px = (band_front * w).max(1.0);
        let peak_pos = (back_px / (back_px + front_px)).clamp(0.15, 0.90);
        let (x_back, x_front) = if is_rtl {
            let fx = clip_rect.right() - p * w;
            (fx + back_px, fx - front_px)
        } else {
            let fx = clip_rect.x + p * w;
            (fx - back_px, fx + front_px)
        };

        if let Some(grad) = self.get_or_create_frontier_glow_grad(
            x_back,
            clip_rect.y,
            x_front,
            clip_rect.y,
            start_a,
            peak_a,
            front_a,
            peak_pos,
        ) {
            unsafe {
                self.dc.DrawTextLayout(
                    pt2(origin_x, origin_y),
                    layout,
                    &grad,
                    D2D1_DRAW_TEXT_OPTIONS_NO_SNAP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            }
        }
        self.pop_clip();
    }

    /// Pushes an `ID2D1Layer` with a cached smooth vertical alpha-gradient mask (`0 -> 1 -> 1 -> 0`)
    /// so scrolling lyrics smoothly dissolve into any animated background with zero per-frame brush allocation!
    pub fn push_vertical_fade_layer(
        &mut self,
        rect: RectF,
        top_fade_dip: f32,
        bot_fade_dip: f32,
    ) -> bool {
        if rect.w <= 2.0 || rect.h <= 8.0 {
            return false;
        }
        let key = (
            (rect.x * 2.0).round() as i32,
            (rect.y * 2.0).round() as i32,
            (rect.w * 2.0).round() as i32,
            (rect.h * 2.0).round() as i32,
            (top_fade_dip * 2.0).round() as i32,
            (bot_fade_dip * 2.0).round() as i32,
        );
        unsafe {
            if self.fade_mask_key != Some(key) || self.fade_mask_brush.is_none() {
                let t_stop = (top_fade_dip / rect.h).clamp(0.01, 0.35);
                let b_stop = (1.0 - (bot_fade_dip / rect.h)).clamp(t_stop + 0.1, 0.99);
                let stops = [
                    D2D1_GRADIENT_STOP {
                        position: 0.0,
                        color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.0 },
                    },
                    D2D1_GRADIENT_STOP {
                        position: t_stop,
                        color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
                    },
                    D2D1_GRADIENT_STOP {
                        position: b_stop,
                        color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
                    },
                    D2D1_GRADIENT_STOP {
                        position: 1.0,
                        color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.0 },
                    },
                ];
                let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                    &self.dc,
                    &stops,
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                ) else {
                    return false;
                };
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(rect.x, rect.y),
                    endPoint: pt2(rect.x, rect.bottom()),
                };
                let Ok(mask_brush) = self.dc.CreateLinearGradientBrush(&props, None, &coll) else {
                    return false;
                };
                self.fade_mask_brush = Some(mask_brush);
                self.fade_mask_key = Some(key);
            }

            let Some(ref mask_brush) = self.fade_mask_brush else {
                return false;
            };
            let layer_params = D2D1_LAYER_PARAMETERS1 {
                contentBounds: to_d2d_rect(rect),
                geometricMask: std::mem::ManuallyDrop::new(None),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: 1.0,
                opacityBrush: std::mem::ManuallyDrop::new(Some(mask_brush.cast().unwrap())),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            self.dc.PushLayer(&layer_params, None);
            let _ = std::mem::ManuallyDrop::into_inner(layer_params.opacityBrush);
            true
        }
    }

    pub fn push_opacity_layer(&self, bounds: RectF, opacity: f32) -> bool {
        let a = opacity.clamp(0.0, 1.0);
        if bounds.w <= 1.0 || bounds.h <= 1.0 {
            return false;
        }
        unsafe {
            let lp = D2D1_LAYER_PARAMETERS1 {
                contentBounds: to_d2d_rect(bounds),
                geometricMask: std::mem::ManuallyDrop::new(None),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: a,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            self.dc.PushLayer(&lp, None);
            true
        }
    }

    pub fn pop_layer(&self) {
        unsafe {
            self.dc.PopLayer();
        }
    }

    pub fn clear_lyrics_line_cache(&mut self) {
        self.lyrics_bmp_cache.clear();
        self.lyrics_unit_layout_cache.clear();
    }

    pub fn release_lyrics_gpu_surfaces(&mut self) {
        let had_any = !self.lyrics_bmp_cache.is_empty()
            || self.active_glow_slots.iter().any(|s| s.bitmap.is_some())
            || self.lyrics_mesh_bmp.is_some()
            || self.lyrics_mesh_out_bmp.is_some();
        if !had_any {
            return;
        }
        self.lyrics_bmp_track.clear();
        self.clear_lyrics_line_cache();
        for slot in &mut self.active_glow_slots {
            slot.bitmap = None;
            slot.baked_bitmap = None;
            slot.alloc_w = 0;
            slot.alloc_h = 0;
            slot.used_dip_w = 0.0;
            slot.used_dip_h = 0.0;
            for eff in [
                &slot.outer_blur_effect,
                &slot.inner_blur_effect,
                &slot.outer_alpha_effect,
                &slot.inner_alpha_effect,
            ] {
                if let Some(ref e) = eff {
                    unsafe {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                }
            }
        }
        self.lyrics_mesh_bmp = None;
        self.lyrics_mesh_out_bmp = None;
        for slot in &mut self.row_fx_slots {
            slot.bitmap = None;
            slot.alloc_w = 0;
            slot.alloc_h = 0;
            slot.used_dip_w = 0.0;
            slot.used_dip_h = 0.0;
        }
        for eff in [
            &self.blur_effect,
            &self.line_blur_effect,
            &self.line_blur_effect_2,
            &self.trans_bg_blur_effect,
            &self.trans_fg_blur_effect,
            &self.glow_blur_effect,
            &self.glow_inner_blur_effect,
            &self.glow_outer_alpha_effect,
            &self.glow_inner_alpha_effect,
            &self.mesh_blur_effect,
            &self.mesh_vibrancy_effect,
        ] {
            if let Some(ref blur) = eff {
                unsafe {
                    blur.SetInput(0, None::<&ID2D1Image>, true);
                }
            }
        }
        self.gpu_staging_dirty = true;
    }

    pub fn set_aggressive_memory_release(&mut self, enabled: bool) {
        if self.aggressive_memory_release != enabled {
            self.aggressive_memory_release = enabled;
            if enabled {
                self.perform_aggressive_memory_trim(true);
            }
        }
    }

    /// Reclaims excess artwork (true LRU), text layouts (O(1) LRU), D2D staging resources, DXGI GPU memory,
    /// and optionally trims the Win32 process working set when「增强内存释放」is enabled.
    pub fn perform_aggressive_memory_trim(&mut self, keep_lyrics_caches: bool) {
        let max_thumbs = if self.aggressive_memory_release { 84 } else { 220 };
        while self.thumb_cache.len() > max_thumbs {
            if let Some(old) = self
                .thumb_cache
                .iter()
                .min_by_key(|(_, v)| v.last_used.get())
                .map(|(k, _)| k.clone())
            {
                self.thumb_cache.remove(&old);
            } else {
                break;
            }
        }
        let max_heroes = if self.aggressive_memory_release { 8 } else { 24 };
        while self.hero_cache.len() > max_heroes {
            if let Some(old) = self
                .hero_cache
                .iter()
                .min_by_key(|(_, v)| v.last_used.get())
                .map(|(k, _)| k.clone())
            {
                self.hero_cache.remove(&old);
            } else {
                break;
            }
        }
        let max_layouts = if self.aggressive_memory_release { 220 } else { 1024 };
        self.text_layout_cache.trim_to(max_layouts);
        if !keep_lyrics_caches {
            self.release_lyrics_gpu_surfaces();
            self.release_lyrics_transition_surfaces();
            self.release_row_fx_surfaces();
        } else if self.aggressive_memory_release {
            self.lyrics_bmp_cache.trim_to(18);
        }
        if self.aggressive_memory_release {
            self.seen_art_stems.clear();
            self.glass_capture_bmp = None;
            self.glass_cap_w = 0;
            self.glass_cap_h = 0;
            self.glass_local_half_blurred = None;
            self.glass_local_half_w = 0;
            self.glass_local_half_h = 0;
            self.glass_frame_snapshot = None;
            self.glass_snap_w = 0;
            self.glass_snap_h = 0;
            self.glass_blurred_snapshot = None;
            self.glass_half_w = 0;
            self.glass_half_h = 0;
            self.glass_top_snap = None;
            self.glass_top_snap_w = 0;
            self.glass_top_snap_h = 0;
            self.glass_top_blurred = None;
            self.glass_top_half_w = 0;
            self.glass_top_half_h = 0;
            self.glass_bot_snap = None;
            self.glass_bot_snap_w = 0;
            self.glass_bot_snap_h = 0;
            self.glass_bot_blurred = None;
            self.glass_bot_half_w = 0;
            self.glass_bot_half_h = 0;
            self.glass_snapshot_active = false;
            self.glass_snapshot_strip_mode = false;
            self.invalidate_glass_snapshot_cache();
            self.glass_disp_maps.clear();
            self.linear_grad_cache.borrow_mut().clear();
            self.radial_glow_cache.borrow_mut().clear();
            self.glass_shadow_cache.borrow_mut().clear();
        }
        self.gpu_staging_dirty = true;
        self.trim_gpu_staging_if_dirty();
        if self.aggressive_memory_release {
            Self::trim_process_working_set();
        }
    }

    pub fn trim_process_working_set() {
        unsafe {
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }

    pub fn trim_gpu_staging_if_dirty(&mut self) {
        if !self.gpu_staging_dirty {
            return;
        }
        self.gpu_staging_dirty = false;
        self.unbind_glass_effect_inputs();
        unsafe {
            for eff in [
                &self.line_blur_effect,
                &self.line_blur_effect_2,
                &self.trans_bg_blur_effect,
                &self.trans_fg_blur_effect,
                &self.glow_blur_effect,
                &self.glow_inner_blur_effect,
                &self.glow_outer_alpha_effect,
                &self.glow_inner_alpha_effect,
                &self.mesh_blur_effect,
                &self.mesh_vibrancy_effect,
            ] {
                if let Some(ref e) = eff {
                    e.SetInput(0, None::<&ID2D1Image>, true);
                }
            }
            for slot in &self.active_glow_slots {
                for eff in [
                    &slot.outer_blur_effect,
                    &slot.inner_blur_effect,
                    &slot.outer_alpha_effect,
                    &slot.inner_alpha_effect,
                ] {
                    if let Some(ref e) = eff {
                        e.SetInput(0, None::<&ID2D1Image>, true);
                    }
                }
            }
            self.d2d_device.ClearResources(0);
            if let Ok(dxgi3) = self.dxgi_device.cast::<IDXGIDevice3>() {
                dxgi3.Trim();
            }
        }
    }

    pub fn set_lyrics_bmp_track(&mut self, track_key: &str) {
        if self.lyrics_bmp_track != track_key {
            self.lyrics_bmp_track = track_key.to_string();
            self.clear_lyrics_line_cache();
        }
    }

    pub fn set_lyrics_blur_downsample(&mut self, enabled: bool) {
        let prev_scale = self.effective_lyrics_blur_scale();
        if enabled {
            self.lyrics_blur_downsample_explicit_off = false;
        } else if self.lyrics_blur_downsample {
            self.lyrics_blur_downsample_explicit_off = true;
        }
        self.lyrics_blur_downsample = enabled;
        if (self.effective_lyrics_blur_scale() - prev_scale).abs() > 0.005 {
            self.clear_lyrics_line_cache();
        }
    }

    /// Opt-in explicit setter distinguishing a user disabling downsample (`user_explicit = true`,
    /// which locks `1.0x` even on wide `>1400px` windows) from default startup (`false`).
    pub fn set_lyrics_blur_downsample_explicit(&mut self, enabled: bool, user_explicit: bool) {
        let prev_scale = self.effective_lyrics_blur_scale();
        self.lyrics_blur_downsample = enabled;
        self.lyrics_blur_downsample_explicit_off = user_explicit && !enabled;
        if (self.effective_lyrics_blur_scale() - prev_scale).abs() > 0.005 {
            self.clear_lyrics_line_cache();
        }
    }

    /// Adaptive lyrics offscreen blur resolution scale:
    /// - Wide windows (`> 1400 px`, e.g. 1440p/4K) automatically use `0.67x` (~2.2x fewer blur pixels & VRAM)
    ///   unless the user explicitly disabled downsampling.
    /// - Compact windows (`<= 1400 px`) use `0.75x` when power-save downsample is enabled, else `1.0x`.
    #[inline]
    pub fn effective_lyrics_blur_scale(&self) -> f32 {
        if self.lyrics_blur_downsample {
            if self.width_px > 1400 {
                0.67
            } else {
                0.75
            }
        } else if !self.lyrics_blur_downsample_explicit_off && self.width_px > 1400 {
            0.67
        } else {
            1.0
        }
    }

    /// Pre-bakes a crisp premultiplied offscreen bitmap for an inactive lyric line (main + bg + translation).
    /// MUST be called when clip/layer depth on `self.dc` is 0!
    /// Uses `effective_lyrics_blur_scale()` (`0.67x` on wide windows >1400px unless explicitly disabled,
    /// `0.75x` in compact power-save mode, else `1.0x`) at `96.0 DPI` for 100% DPI-invariance.
    pub fn ensure_lyrics_line_bitmap(
        &mut self,
        key: LyricsLineBmpKey,
        vocal_max_w: f32,
        total_h: f32,
        align: TextAlign,
        main_layout: &IDWriteTextLayout,
        bg_layout: Option<(&IDWriteTextLayout, f32)>,
        trans_layout: Option<(&IDWriteTextLayout, f32)>,
    ) {
        let scale = self.effective_lyrics_blur_scale();
        if let Some(existing) = self.lyrics_bmp_cache.get_touch(&key) {
            if (existing.scale - scale).abs() < 0.01 {
                return;
            }
            self.lyrics_bmp_cache.remove(&key);
        }
        if self.target_bitmap.is_none() {
            return;
        }

        let pad_dip = 24.0f32;
        let dip_w = (vocal_max_w + pad_dip * 2.0).clamp(64.0, 2400.0);
        let dip_h = (total_h + pad_dip * 2.0).clamp(32.0, 680.0);
        let px_w = ((dip_w * scale).ceil() as u32).clamp(32, 2400);
        let px_h = ((dip_h * scale).ceil() as u32).clamp(24, 720);

        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let Ok(bmp) = self.dc.CreateBitmap(
                D2D_SIZE_U {
                    width: px_w,
                    height: px_h,
                },
                None,
                0,
                &props,
            ) else {
                return;
            };

            let scale_xf = Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: 0.0,
                M32: 0.0,
            };
            self.dc.SetTarget(&bmp);
            self.dc.SetDpi(96.0, 96.0);
            if self.lyrics_subpixel_mode {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            }
            self.dc.SetTransform(&scale_xf);
            self.dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            self.draw_text_layout_subpixel(
                main_layout,
                pad_dip,
                pad_dip,
                Color::rgba(255, 255, 255, 1.0),
            );

            if let Some((bg_lay, bg_dy)) = bg_layout {
                // `Specs.backgroundVocalsDeselectedTransform = scale(0.9, 0.9)`
                let bg_scale = 0.90f32;
                let bg_piv_x = if align == TextAlign::Right {
                    pad_dip + vocal_max_w
                } else {
                    pad_dip
                };
                let bg_piv_y = pad_dip + bg_dy;
                let bg_xf = Matrix3x2 {
                    M11: bg_scale * scale,
                    M12: 0.0,
                    M21: 0.0,
                    M22: bg_scale * scale,
                    M31: bg_piv_x * (1.0 - bg_scale) * scale,
                    M32: bg_piv_y * (1.0 - bg_scale) * scale,
                };
                self.dc.SetTransform(&bg_xf);
                self.draw_text_layout_subpixel(
                    bg_lay,
                    pad_dip,
                    pad_dip + bg_dy,
                    Color::rgba(255, 255, 255, 0.72),
                );
                self.dc.SetTransform(&scale_xf);
            }

            if let Some((tr_lay, tr_dy)) = trans_layout {
                self.draw_text_layout_subpixel(
                    tr_lay,
                    pad_dip,
                    pad_dip + tr_dy,
                    Color::rgba(255, 255, 255, 0.68),
                );
            }

            if let Some(main_tgt) = self.active_draw_target() {
                let main_dpi = 96.0 * self.dpi_scale;
                self.dc.SetTarget(main_tgt);
                self.dc.SetDpi(main_dpi, main_dpi);
            }
            if self.lyrics_subpixel_mode {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            }
            self.dc.SetTransform(&Matrix3x2::identity());

            let max_lines = if self.aggressive_memory_release { 18 } else { 44 };
            self.lyrics_bmp_cache.insert(
                key,
                CachedLyricsLineBmp {
                    bitmap: bmp,
                    blurred_bmp: None,
                    blurred_sigma_q: 0,
                    dip_w,
                    dip_h,
                    pad_dip,
                    scale,
                    written_at_flush_seq: self.dc_flush_seq.get(),
                },
                max_lines,
            );
            self.gpu_staging_dirty = true;
        }
    }

    /// Pre-bakes smooth GPU Gaussian depth-of-field blur (`0.5 DIP` increments, `BALANCED` kernel,
    /// `96 DPI` at adaptive scale) in Pass 1 using 2 ping-pong blur effect slots (`line_blur_effect` /
    /// `line_blur_effect_2`) so steady-state `blur_spring` transitions run with **0 `dc.Flush()` calls**!
    /// Returns `true` if a GPU blur pass ran (so caller can budget max bakes per frame).
    pub fn prepare_lyrics_line_blurred(&mut self, key: LyricsLineBmpKey, sigma_dip: f32) -> bool {
        if sigma_dip <= 0.04 || self.blur_effect.is_none() || self.target_bitmap.is_none() {
            return false;
        }
        // 0.5 DIP quantization bucket halves rebake churn during blur_spring transitions while remaining silky!
        let sigma_q = (sigma_dip.clamp(0.25, 24.0) * 2.0).round().max(1.0) as u16;
        let Some((src_bmp, existing_blurred, prev_q, dip_w, dip_h, scale, written_seq)) =
            self.lyrics_bmp_cache.get_touch(&key).map(|e| {
                (
                    e.bitmap.clone(),
                    e.blurred_bmp.clone(),
                    e.blurred_sigma_q,
                    e.dip_w,
                    e.dip_h,
                    e.scale,
                    e.written_at_flush_seq,
                )
            })
        else {
            return false;
        };
        if existing_blurred.is_some() && prev_q == sigma_q {
            return false;
        }

        let b_px_w = ((dip_w * scale).ceil() as u32).clamp(32, 2400);
        let b_px_h = ((dip_h * scale).ceil() as u32).clamp(24, 720);
        let sigma_px = ((sigma_q as f32) * 0.5) * scale;

        unsafe {
            let target_bmp = if let Some(prev_b) = existing_blurred {
                let sz = prev_b.GetPixelSize();
                if sz.width == b_px_w && sz.height == b_px_h {
                    Some(prev_b)
                } else {
                    None
                }
            } else {
                None
            };

            let b_bmp = match target_bmp {
                Some(b) => b,
                None => {
                    let props = D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    };
                    let Ok(b) = self.dc.CreateBitmap(
                        D2D_SIZE_U {
                            width: b_px_w,
                            height: b_px_h,
                        },
                        None,
                        0,
                        &props,
                    ) else {
                        return false;
                    };
                    self.gpu_staging_dirty = true;
                    b
                }
            };

            // Pick dedicated effect slot 0 or 1 so up to 2 steady-state line bakes/frame run with
            // ZERO `dc.Flush()`! Only flush if `src_bmp` was newly rasterized since the last Flush/EndDraw
            // (D2D1 Target->Effect barrier) or a 3rd+ bake reuses an already-queued effect instance.
            let slot_idx = self.line_blur_slot_cursor.get();
            let src_needs_flush = written_seq == self.dc_flush_seq.get();
            let need_flush_for_reuse =
                slot_idx >= 2 || (slot_idx == 1 && self.line_blur_effect_2.is_none());
            if src_needs_flush || need_flush_for_reuse {
                self.flush_dc();
            }
            let blur_opt = if slot_idx % 2 == 1 {
                self.line_blur_effect_2
                    .as_ref()
                    .or(self.line_blur_effect.as_ref())
                    .or(self.blur_effect.as_ref())
            } else {
                self.line_blur_effect.as_ref().or(self.blur_effect.as_ref())
            };
            let Some(ref blur) = blur_opt else {
                return false;
            };
            blur.SetInput(0, &src_bmp, true);
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &sigma_px.to_le_bytes(),
            );
            let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32;
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                D2D1_PROPERTY_TYPE_UINT32,
                &opt.to_le_bytes(),
            );
            let border = D2D1_BORDER_MODE_SOFT.0 as u32;
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_UINT32,
                &border.to_le_bytes(),
            );

            self.dc.SetTarget(&b_bmp);
            self.dc.SetDpi(96.0, 96.0);
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            if let Ok(out_img) = blur.GetOutput() {
                self.dc.DrawImage(
                    &out_img,
                    Some(&pt2(0.0, 0.0)),
                    None,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
            }
            self.line_blur_slot_cursor.set(slot_idx.saturating_add(1));

            if let Some(main_tgt) = self.active_draw_target() {
                self.dc.SetTarget(main_tgt);
                let dpi = 96.0 * self.dpi_scale;
                self.dc.SetDpi(dpi, dpi);
            }
            self.dc.SetTransform(&Matrix3x2::identity());

            if let Some(entry) = self.lyrics_bmp_cache.get_mut_peek(&key) {
                entry.blurred_bmp = Some(b_bmp);
                entry.blurred_sigma_q = sigma_q;
            }
        }
        true
    }

    /// Draws a pre-baked (`96 DPI`) blurred lyric line cleanly mapped onto exact DIP bounds
    /// with zero live blur and zero `PushLayer` in Pass 2!
    pub fn draw_cached_lyrics_line_blurred(
        &self,
        key: LyricsLineBmpKey,
        origin_x: f32,
        screen_y: f32,
        sigma_dip: f32,
        alpha: f32,
    ) -> bool {
        let Some(entry) = self.lyrics_bmp_cache.get_peek(&key) else {
            return false;
        };
        let a = alpha.clamp(0.0, 1.0);
        if a <= 0.004 {
            return true;
        }
        let dst = RectF::new(
            origin_x - entry.pad_dip,
            screen_y - entry.pad_dip,
            entry.dip_w,
            entry.dip_h,
        );
        unsafe {
            let bmp_ref = if sigma_dip > 0.04 {
                entry.blurred_bmp.as_ref().unwrap_or(&entry.bitmap)
            } else {
                &entry.bitmap
            };
            self.dc.DrawBitmap(
                bmp_ref,
                Some(&to_d2d_rect(dst)),
                a,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
            true
        }
    }

    /// Begins offscreen glyph emission capture into `active_glow_slots[slot]` (0 or 1).
    /// Uses native `1.0x` DIP resolution at `96.0 DPI` so emphasized glow has crisp optical smooth
    /// falloff with zero magnification mosaic across 100%..300% monitors.
    pub fn begin_active_lyrics_glow_slot(
        &mut self,
        slot: usize,
        vocal_max_w: f32,
        total_h: f32,
    ) -> Option<f32> {
        if slot >= 2 || self.target_bitmap.is_none() {
            return None;
        }
        let pad_dip = 56.0f32;
        let dip_w = (vocal_max_w + pad_dip * 2.0).clamp(64.0, 2600.0);
        let dip_h = (total_h + pad_dip * 2.0).clamp(48.0, 720.0);
        let scale = 1.0f32;
        let req_px_w = ((dip_w * scale).ceil() as u32).clamp(64, 2560);
        let req_px_h = ((dip_h * scale).ceil() as u32).clamp(48, 768);

        unsafe {
            let need_alloc = {
                let s = &self.active_glow_slots[slot];
                s.bitmap.is_none()
                    || s.baked_bitmap.is_none()
                    || req_px_w > s.alloc_w
                    || req_px_h > s.alloc_h
            };
            if need_alloc {
                let alloc_w = ((req_px_w + 127) & !127).clamp(128, 2560);
                let alloc_h = ((req_px_h + 63) & !63).clamp(64, 768);
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                    colorContext: std::mem::ManuallyDrop::new(None),
                };
                let bmp = self
                    .dc
                    .CreateBitmap(
                        D2D_SIZE_U {
                            width: alloc_w,
                            height: alloc_h,
                        },
                        None,
                        0,
                        &props,
                    )
                    .ok()?;
                let baked = self
                    .dc
                    .CreateBitmap(
                        D2D_SIZE_U {
                            width: alloc_w,
                            height: alloc_h,
                        },
                        None,
                        0,
                        &props,
                    )
                    .ok()?;
                self.active_glow_slots[slot].alloc_w = alloc_w;
                self.active_glow_slots[slot].alloc_h = alloc_h;
                self.active_glow_slots[slot].bitmap = Some(bmp);
                self.active_glow_slots[slot].baked_bitmap = Some(baked);
                self.gpu_staging_dirty = true;
            }

            self.active_glow_slots[slot].used_dip_w = dip_w;
            self.active_glow_slots[slot].used_dip_h = dip_h;
            self.active_glow_slots[slot].pad_dip = pad_dip;
            self.offscreen_base_scale = scale;

            let bmp_ref = self.active_glow_slots[slot].bitmap.as_ref()?;
            self.dc.SetTarget(bmp_ref);
            self.dc.SetDpi(96.0, 96.0);
            if self.lyrics_subpixel_mode {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            }
            self.reset_transform();
            self.dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
        }
        Some(pad_dip)
    }

    pub fn end_active_lyrics_glow_slot(&mut self) {
        self.offscreen_base_scale = 1.0;
        unsafe {
            if let Some(main_tgt) = self.active_draw_target() {
                self.dc.SetTarget(main_tgt);
                let dpi = 96.0 * self.dpi_scale;
                self.dc.SetDpi(dpi, dpi);
            }
            if self.lyrics_subpixel_mode {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            } else {
                self.dc
                    .SetTextRenderingParams(None::<&IDWriteRenderingParams>);
            }
            self.dc.SetTransform(&Matrix3x2::identity());
        }
    }

    /// Finishes Pass 1 emission capture and pre-bakes smooth dual-sigma `1.0x` (`96 DPI`) optical
    /// bloom using dedicated per-slot outer/inner blur + ColorMatrix opacity effects (`0 dc.Flush()`
    /// calls across both active glow slots; `SetTarget` provides the GPU RT barrier automatically!).
    pub fn finish_and_bake_active_lyrics_glow_slot(
        &mut self,
        slot: usize,
        inner_sigma: f32,
        inner_alpha: f32,
        outer_sigma: f32,
        outer_alpha: f32,
    ) -> bool {
        if slot >= 2 {
            self.end_active_lyrics_glow_slot();
            return false;
        }
        let s = &self.active_glow_slots[slot];
        let dedicated_outer = s.outer_blur_effect.is_some() && s.outer_alpha_effect.is_some();
        let dedicated_inner = s.inner_blur_effect.is_some() && s.inner_alpha_effect.is_some();
        let outer_blur_opt = s.outer_blur_effect.as_ref().or(self.glow_blur_effect.as_ref());
        let outer_alpha_opt = s
            .outer_alpha_effect
            .as_ref()
            .or(self.glow_outer_alpha_effect.as_ref());
        let inner_blur_opt = s
            .inner_blur_effect
            .as_ref()
            .or(self.glow_inner_blur_effect.as_ref());
        let inner_alpha_opt = s
            .inner_alpha_effect
            .as_ref()
            .or(self.glow_inner_alpha_effect.as_ref());
        let (Some(ref src_bmp), Some(ref baked_bmp), Some(outer_blur)) =
            (&s.bitmap, &s.baked_bitmap, outer_blur_opt)
        else {
            self.end_active_lyrics_glow_slot();
            return false;
        };

        let full_rc = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: s.used_dip_w.min(s.alloc_w as f32),
            bottom: s.used_dip_h.min(s.alloc_h as f32),
        };
        let outer_sig = outer_sigma.clamp(4.5, 22.0);
        let inner_sig = inner_sigma.clamp(2.4, 8.5);
        let oa = outer_alpha.clamp(0.0, 0.48);
        let ia = inner_alpha.clamp(0.0, 0.36);

        // Note: Direct2D `CLSID_D2D1ColorMatrix` defaults to `D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED`,
        // which un-premultiplies RGB, multiplies by the 5x4 matrix, and re-premultiplies by the new A.
        // Keeping RGB diagonals at 1.0 and scaling only M44 by `a` preserves pure white optical bloom
        // at exact linear intensity `a` (avoiding `a^2` darkening/graying).
        let build_alpha_mat_bytes = |a: f32| -> [u8; 80] {
            let m: [f32; 20] = [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, a,
                0.0, 0.0, 0.0, 0.0,
            ];
            let mut bytes = [0u8; 80];
            for (i, f) in m.iter().enumerate() {
                bytes[i * 4..(i + 1) * 4].copy_from_slice(&f.to_le_bytes());
            }
            bytes
        };

        unsafe {
            // Flush Pass 1 glyph emission commands into `src_bmp` before sampling it in GaussianBlur!
            self.flush_dc();
            self.dc.SetTarget(baked_bmp);
            self.dc.SetDpi(96.0, 96.0);
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32;
            let border = D2D1_BORDER_MODE_SOFT.0 as u32;

            // Pass A: Soft wide atmospheric bloom (per-slot `outer_blur_effect` + exact `oa` attenuation)
            if oa > 0.02 {
                outer_blur.SetInput(0, src_bmp, true);
                let _ = outer_blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                    D2D1_PROPERTY_TYPE_UINT32,
                    &opt.to_le_bytes(),
                );
                let _ = outer_blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_UINT32,
                    &border.to_le_bytes(),
                );
                let _ = outer_blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &outer_sig.to_le_bytes(),
                );
                if let (Some(alpha_eff), Ok(blur_out)) = (outer_alpha_opt, outer_blur.GetOutput()) {
                    alpha_eff.SetInput(0, &blur_out, true);
                    let mat_b = build_alpha_mat_bytes(oa);
                    let _ = alpha_eff.SetValue(
                        D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                        D2D1_PROPERTY_TYPE_BLOB,
                        &mat_b,
                    );
                    if let Ok(out_img) = alpha_eff.GetOutput() {
                        self.dc.DrawImage(
                            &out_img,
                            Some(&pt2(0.0, 0.0)),
                            Some(&full_rc),
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            D2D1_COMPOSITE_MODE_SOURCE_OVER,
                        );
                    }
                } else if let Ok(out_img) = outer_blur.GetOutput() {
                    let lp = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: full_rc,
                        geometricMask: std::mem::ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: Matrix3x2::identity(),
                        opacity: oa,
                        opacityBrush: std::mem::ManuallyDrop::new(None),
                        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                    };
                    self.dc.PushLayer(&lp, None);
                    self.dc.DrawImage(
                        &out_img,
                        Some(&pt2(0.0, 0.0)),
                        Some(&full_rc),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                    self.dc.PopLayer();
                }
            }

            // Pass B: Smooth tight inner halo (per-slot `inner_blur_effect` + exact `ia` attenuation!)
            if ia > 0.02 {
                let inner_blur = inner_blur_opt.unwrap_or(outer_blur);
                if inner_blur_opt.is_none() {
                    let _ = self.dc.Flush(None, None);
                }
                inner_blur.SetInput(0, src_bmp, true);
                let _ = inner_blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                    D2D1_PROPERTY_TYPE_UINT32,
                    &opt.to_le_bytes(),
                );
                let _ = inner_blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_UINT32,
                    &border.to_le_bytes(),
                );
                let _ = inner_blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &inner_sig.to_le_bytes(),
                );
                // Use SOURCE_OVER (never additive PLUS) so dense CJK Kanji/Hanzi inner strokes never blow out to solid white!
                let comp_mode = D2D1_COMPOSITE_MODE_SOURCE_OVER;
                if let (Some(alpha_eff), Ok(blur_out)) = (inner_alpha_opt, inner_blur.GetOutput()) {
                    alpha_eff.SetInput(0, &blur_out, true);
                    let mat_b = build_alpha_mat_bytes(ia);
                    let _ = alpha_eff.SetValue(
                        D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                        D2D1_PROPERTY_TYPE_BLOB,
                        &mat_b,
                    );
                    if let Ok(in_img) = alpha_eff.GetOutput() {
                        self.dc.DrawImage(
                            &in_img,
                            Some(&pt2(0.0, 0.0)),
                            Some(&full_rc),
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            comp_mode,
                        );
                    }
                } else if let Ok(in_img) = inner_blur.GetOutput() {
                    let lp = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: full_rc,
                        geometricMask: std::mem::ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: Matrix3x2::identity(),
                        opacity: ia,
                        opacityBrush: std::mem::ManuallyDrop::new(None),
                        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                    };
                    self.dc.PushLayer(&lp, None);
                    self.dc.DrawImage(
                        &in_img,
                        Some(&pt2(0.0, 0.0)),
                        Some(&full_rc),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        comp_mode,
                    );
                    self.dc.PopLayer();
                }
            }
            // Only flush when falling back to shared glow effects across slots!
            if !(dedicated_outer && dedicated_inner) {
                let _ = self.dc.Flush(None, None);
            }
        }
        self.end_active_lyrics_glow_slot();
        true
    }

    /// Composites a pre-baked Pass 1 active-line glyph glow bitmap (`96 DPI`, `1.0x`) in Pass 2
    /// with a single additive `DrawImage` call (`D2D1_COMPOSITE_MODE_PLUS`, `LINEAR`, zero live blur/PushLayer).
    #[allow(clippy::too_many_arguments)]
    pub fn composite_active_lyrics_glow_slot(
        &self,
        slot: usize,
        origin_x: f32,
        screen_y: f32,
        _inner_sigma: f32,
        _inner_alpha: f32,
        _outer_sigma: f32,
        _outer_alpha: f32,
    ) {
        if slot >= 2 {
            return;
        }
        let s = &self.active_glow_slots[slot];
        let Some(ref baked) = s.baked_bitmap else {
            return;
        };
        let dst_x = origin_x - s.pad_dip;
        let dst_y = screen_y - s.pad_dip;
        let full_rc = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: s.used_dip_w.min(s.alloc_w as f32),
            bottom: s.used_dip_h.min(s.alloc_h as f32),
        };
        unsafe {
            let mut cur_xf = Matrix3x2::identity();
            self.dc.GetTransform(&mut cur_xf);
            // Compose `Translate(dst_x, dst_y) * cur_xf` in D2D row-vector order (exact 1:1 DIP!)
            let glow_xf = Matrix3x2 {
                M11: cur_xf.M11,
                M12: cur_xf.M12,
                M21: cur_xf.M21,
                M22: cur_xf.M22,
                M31: dst_x * cur_xf.M11 + dst_y * cur_xf.M21 + cur_xf.M31,
                M32: dst_x * cur_xf.M12 + dst_y * cur_xf.M22 + cur_xf.M32,
            };
            self.dc.SetTransform(&glow_xf);
            // Use SOURCE_OVER behind foreground crisp glyphs so bright aurora covers + dense CJK
            // ideographs retain strong stroke contrast and never clip to unreadable pure white!
            self.dc.DrawImage(
                baked,
                Some(&pt2(0.0, 0.0)),
                Some(&full_rc),
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            self.dc.SetTransform(&cur_xf);
        }
    }

    #[inline]
    fn active_draw_target(&self) -> Option<&ID2D1Bitmap1> {
        if self.in_lyrics_transition_sheet {
            self.lyrics_trans_fg_bmp
                .as_ref()
                .or(self.target_bitmap.as_ref())
        } else {
            self.target_bitmap.as_ref()
        }
    }

    pub fn release_lyrics_transition_surfaces(&mut self) {
        self.in_lyrics_transition_sheet = false;
        unsafe {
            // CRITICAL: unbind effect graph inputs BEFORE dropping transition bitmaps so D2D COM
            // effects do not permanently pin full-screen VRAM surfaces!
            if let Some(ref eff) = self.trans_bg_blur_effect {
                eff.SetInput(0, None, true);
            }
            if let Some(ref eff) = self.trans_fg_blur_effect {
                eff.SetInput(0, None, true);
            }
        }
        if self.lyrics_trans_bg_96.is_none()
            && self.lyrics_trans_fg_bmp.is_none()
            && self.lyrics_trans_half_bg.is_none()
            && self.lyrics_trans_half_fg.is_none()
        {
            return;
        }
        self.lyrics_trans_bg_96 = None;
        self.lyrics_trans_fg_bmp = None;
        self.lyrics_trans_half_bg = None;
        self.lyrics_trans_half_fg = None;
        self.lyrics_trans_w = 0;
        self.lyrics_trans_h = 0;
        self.lyrics_trans_half_w = 0;
        self.lyrics_trans_half_h = 0;
        self.gpu_staging_dirty = true;
    }

    fn ensure_lyrics_transition_surfaces(&mut self) -> bool {
        let w = self.width_px.max(64);
        let h = self.height_px.max(64);
        let half_w = (w / 2).max(32);
        let half_h = (h / 2).max(32);
        if self.lyrics_trans_w == w
            && self.lyrics_trans_h == h
            && self.lyrics_trans_half_w == half_w
            && self.lyrics_trans_half_h == half_h
            && self.lyrics_trans_bg_96.is_some()
            && self.lyrics_trans_fg_bmp.is_some()
            && self.lyrics_trans_half_bg.is_some()
            && self.lyrics_trans_half_fg.is_some()
        {
            return true;
        }
        let main_dpi = 96.0 * self.dpi_scale;
        unsafe {
            let props_96 = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let props_fg = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: main_dpi,
                dpiY: main_dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let props_half = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let sz = D2D_SIZE_U { width: w, height: h };
            let hsz = D2D_SIZE_U {
                width: half_w,
                height: half_h,
            };
            let (Ok(bg96), Ok(fg), Ok(hbg), Ok(hfg)) = (
                self.dc.CreateBitmap(sz, None, 0, &props_96),
                self.dc.CreateBitmap(sz, None, 0, &props_fg),
                self.dc.CreateBitmap(hsz, None, 0, &props_half),
                self.dc.CreateBitmap(hsz, None, 0, &props_half),
            ) else {
                return false;
            };
            self.lyrics_trans_bg_96 = Some(bg96);
            self.lyrics_trans_fg_bmp = Some(fg);
            self.lyrics_trans_half_bg = Some(hbg);
            self.lyrics_trans_half_fg = Some(hfg);
            self.lyrics_trans_w = w;
            self.lyrics_trans_h = h;
            self.lyrics_trans_half_w = half_w;
            self.lyrics_trans_half_h = half_h;
            self.gpu_staging_dirty = true;
            true
        }
    }

    /// Smoothly blurs the already-rendered main UI on `target_bitmap` during Lyrics open/close
    /// transitions at **0.5x resolution** (4x fewer blur pixels, 1 flush instead of 2, `96 DPI -> 96 DPI`
    /// for 100% DPI-invariance, zero edge halo via `BORDER_MODE_HARD`).
    pub fn blur_current_backbuffer_in_place(&mut self, sigma_dip: f32) {
        if sigma_dip <= 0.25 || self.target_bitmap.is_none() {
            return;
        }
        if !self.ensure_lyrics_transition_surfaces() {
            return;
        }
        let Some(ref blur) = self
            .trans_bg_blur_effect
            .as_ref()
            .or(self.blur_effect.as_ref())
        else {
            return;
        };
        let (Some(ref main_tgt), Some(ref bg96), Some(ref half_bg)) = (
            &self.target_bitmap,
            &self.lyrics_trans_bg_96,
            &self.lyrics_trans_half_bg,
        ) else {
            return;
        };
        let main_dpi = 96.0 * self.dpi_scale;
        let w_px = self.width_px as f32;
        let h_px = self.height_px as f32;
        let half_w = self.lyrics_trans_half_w.max(1) as f32;
        let half_h = self.lyrics_trans_half_h.max(1) as f32;
        let sigma_half = (sigma_dip * self.dpi_scale * 0.5).clamp(0.4, 21.0);
        let full_px = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: w_px,
            bottom: h_px,
        };
        let half_rc = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: half_w,
            bottom: half_h,
        };
        unsafe {
            // Flush all queued main UI draws (sidebar, top nav, miniplayer) into `main_tgt`
            // BEFORE `CopyFromBitmap` executes on the GPU copy queue!
            self.flush_dc();
            if bg96.CopyFromBitmap(None, main_tgt, None).is_err() {
                return;
            }
            self.suspend_dirty_clip();

            // 1. Hardware 0.5x downsample into `half_bg`
            blur.SetInput(0, None::<&ID2D1Image>, true);
            self.dc.SetTarget(half_bg);
            self.dc.SetDpi(96.0, 96.0);
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.DrawBitmap(
                bg96,
                Some(&half_rc),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                Some(&full_px),
                None,
            );
            // Commit `half_bg` before `trans_bg_blur_effect` (ID2D1Effect) reads it!
            self.flush_dc();

            // 2. Blur at 0.5x (4x fewer pixels!) and upscale straight onto `main_tgt`
            self.dc.SetTarget(main_tgt);
            self.dc.SetDpi(96.0, 96.0);
            self.dc.SetTransform(&Matrix3x2 {
                M11: w_px / half_w,
                M12: 0.0,
                M21: 0.0,
                M22: h_px / half_h,
                M31: 0.0,
                M32: 0.0,
            });
            blur.SetInput(0, half_bg, true);
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &sigma_half.to_le_bytes(),
            );
            let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_SPEED.0 as u32;
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                D2D1_PROPERTY_TYPE_UINT32,
                &opt.to_le_bytes(),
            );
            let border = D2D1_BORDER_MODE_HARD.0 as u32;
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_UINT32,
                &border.to_le_bytes(),
            );
            if let Ok(out_img) = blur.GetOutput() {
                self.dc.DrawImage(
                    &out_img,
                    Some(&pt2(0.0, 0.0)),
                    Some(&half_rc),
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    windows::Win32::Graphics::Direct2D::Common::D2D1_COMPOSITE_MODE_SOURCE_COPY,
                );
            }
            self.dc.SetDpi(main_dpi, main_dpi);
            self.dc.SetTransform(&Matrix3x2::identity());
            self.resume_dirty_clip();
        }
    }

    /// Binds an offscreen native-DPI target so the entire Lyrics overlay can be rendered as one
    /// unified sheet (after Pass 1 caches are pre-baked at depth 0!) and composited with true
    /// opacity fade + 0.5x Gaussian blur.
    pub fn begin_lyrics_transition_sheet(&mut self) -> bool {
        if self.target_bitmap.is_none() || !self.ensure_lyrics_transition_surfaces() {
            return false;
        }
        let Some(ref fg) = self.lyrics_trans_fg_bmp else {
            return false;
        };
        let main_dpi = 96.0 * self.dpi_scale;
        unsafe {
            self.dc.SetTarget(fg);
            self.dc.SetDpi(main_dpi, main_dpi);
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
        }
        self.in_lyrics_transition_sheet = true;
        true
    }

    /// Restores `target_bitmap` at layer depth 0 and composites the captured Lyrics sheet with
    /// unified opacity fade (`alpha`), **0.5x Gaussian blur** (`blur_sigma_dip`), vertical slide
    /// (`slide_y_dip`), subtle centered scale, and rounded-corner geometric mask (zero `CopyFromBitmap`,
    /// zero `dc.Flush()`, 4x fewer blur pixels!).
    pub fn end_and_composite_lyrics_transition_sheet(
        &mut self,
        alpha: f32,
        blur_sigma_dip: f32,
        slide_y_dip: f32,
        scale: f32,
        corner_radius_dip: f32,
        is_maximized: bool,
    ) {
        self.in_lyrics_transition_sheet = false;
        let main_dpi = 96.0 * self.dpi_scale;
        let a = alpha.clamp(0.0, 1.0);
        let sc = scale.clamp(0.85, 1.08);
        let w_px = self.width_px as f32;
        let h_px = self.height_px as f32;
        let slide_px = slide_y_dip * self.dpi_scale;

        unsafe {
            let Some(ref main_tgt) = self.target_bitmap else {
                return;
            };
            let Some(ref fg_bmp) = self.lyrics_trans_fg_bmp else {
                self.dc.SetTarget(main_tgt);
                self.dc.SetDpi(main_dpi, main_dpi);
                self.dc.SetTransform(&Matrix3x2::identity());
                return;
            };
            if a <= 0.004 {
                self.dc.SetTarget(main_tgt);
                self.dc.SetDpi(main_dpi, main_dpi);
                self.dc.SetTransform(&Matrix3x2::identity());
                return;
            }

            let half_w = self.lyrics_trans_half_w.max(1) as f32;
            let half_h = self.lyrics_trans_half_h.max(1) as f32;
            let half_rc = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: half_w,
                bottom: half_h,
            };
            let full_px = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w_px,
                bottom: h_px,
            };

            let want_half_blur = blur_sigma_dip > 0.25
                && self.lyrics_trans_half_fg.is_some()
                && self
                    .trans_fg_blur_effect
                    .as_ref()
                    .or(self.blur_effect.as_ref())
                    .is_some();

            // 1. When blurring, hardware-downsample `fg_bmp` directly into `half_fg` and flush before Effect read
            if want_half_blur {
                if let Some(ref eff) = self.trans_fg_blur_effect {
                    eff.SetInput(0, None::<&ID2D1Image>, true);
                }
                if let Some(ref half_fg) = self.lyrics_trans_half_fg {
                    self.dc.SetTarget(half_fg);
                    self.dc.SetDpi(96.0, 96.0);
                    self.dc.SetTransform(&Matrix3x2::identity());
                    self.dc.Clear(Some(&D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }));
                    self.dc.DrawBitmap(
                        fg_bmp,
                        Some(&half_rc),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                    self.flush_dc();
                }
            }

            self.dc.SetTarget(main_tgt);
            self.dc.SetDpi(96.0, 96.0);
            self.dc.SetTransform(&Matrix3x2::identity());

            let cx = w_px * 0.5;
            let cy = h_px * 0.5;
            let sheet_xf = Matrix3x2 {
                M11: sc,
                M12: 0.0,
                M21: 0.0,
                M22: sc,
                M31: cx * (1.0 - sc),
                M32: cy * (1.0 - sc) + slide_px,
            };

            // Rounded-corner geometric mask matching window silhouette so the scaling/sliding
            // transition sheet NEVER exposes sharp 90° rectangular corners!
            let sheet_rad_dip = if is_maximized {
                (14.0 * (1.0 - a)).max(0.0)
            } else {
                corner_radius_dip.clamp(0.0, 36.0).max(10.0 * (1.0 - a))
            };
            let sheet_rad_px = sheet_rad_dip * self.dpi_scale;
            let mask_geom: Option<ID2D1Geometry> = if sheet_rad_px > 0.5 {
                let rr = D2D1_ROUNDED_RECT {
                    rect: full_px,
                    radiusX: sheet_rad_px,
                    radiusY: sheet_rad_px,
                };
                self.d2d_factory
                    .CreateRoundedRectangleGeometry(&rr)
                    .ok()
                    .and_then(|g| g.cast().ok())
            } else {
                None
            };

            let params = D2D1_LAYER_PARAMETERS1 {
                contentBounds: full_px,
                geometricMask: std::mem::ManuallyDrop::new(mask_geom),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: sheet_xf,
                opacity: a,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            self.dc.PushLayer(&params, None);

            let mut drew_blurred = false;
            if want_half_blur {
                if let (Some(ref half_fg), Some(ref blur)) = (
                    &self.lyrics_trans_half_fg,
                    self.trans_fg_blur_effect
                        .as_ref()
                        .or(self.blur_effect.as_ref()),
                ) {
                    let ups_sheet_xf = Matrix3x2 {
                        M11: (w_px / half_w) * sc,
                        M12: 0.0,
                        M21: 0.0,
                        M22: (h_px / half_h) * sc,
                        M31: sheet_xf.M31,
                        M32: sheet_xf.M32,
                    };
                    self.dc.SetTransform(&ups_sheet_xf);
                    let sigma_half = (blur_sigma_dip * self.dpi_scale * 0.5).clamp(0.3, 18.0);
                    blur.SetInput(0, half_fg, true);
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                        D2D1_PROPERTY_TYPE_FLOAT,
                        &sigma_half.to_le_bytes(),
                    );
                    let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_SPEED.0 as u32;
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                        D2D1_PROPERTY_TYPE_UINT32,
                        &opt.to_le_bytes(),
                    );
                    let border = D2D1_BORDER_MODE_HARD.0 as u32;
                    let _ = blur.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                        D2D1_PROPERTY_TYPE_UINT32,
                        &border.to_le_bytes(),
                    );
                    if let Ok(out_img) = blur.GetOutput() {
                        self.dc.DrawImage(
                            &out_img,
                            Some(&pt2(0.0, 0.0)),
                            Some(&half_rc),
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            D2D1_COMPOSITE_MODE_SOURCE_OVER,
                        );
                        drew_blurred = true;
                    }
                }
            }
            if !drew_blurred {
                self.dc.SetTransform(&sheet_xf);
                self.dc.DrawBitmap(
                    fg_bmp,
                    Some(&full_px),
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
            }

            self.dc.PopLayer();
            let _ = std::mem::ManuallyDrop::into_inner(params.geometricMask);
            self.dc.SetDpi(main_dpi, main_dpi);
            self.dc.SetTransform(&Matrix3x2::identity());
        }
    }

    pub fn release_row_fx_surfaces(&mut self) {
        if self.row_fx_slots.iter().all(|s| s.bitmap.is_none()) {
            return;
        }
        for s in &mut self.row_fx_slots {
            s.bitmap = None;
            s.alloc_w = 0;
            s.alloc_h = 0;
            s.used_dip_w = 0.0;
            s.used_dip_h = 0.0;
        }
        self.gpu_staging_dirty = true;
    }

    /// Begins offscreen playlist row capture for diff fade/blur animation at clip/layer depth == 0.
    /// Draws inside `(pad_dip, pad_dip)`..`(pad_dip + dip_w, pad_dip + dip_h)`.
    pub fn begin_row_fx_bake(&mut self, slot: usize, dip_w: f32, dip_h: f32) -> Option<f32> {
        if slot >= 24 || self.target_bitmap.is_none() {
            return None;
        }
        while self.row_fx_slots.len() <= slot {
            self.row_fx_slots.push(RowFxSlot::default());
        }
        let pad_dip = 18.0f32;
        let total_dip_w = (dip_w + pad_dip * 2.0).clamp(64.0, 2600.0);
        let total_dip_h = (dip_h + pad_dip * 2.0).clamp(32.0, 220.0);
        let scale = self.dpi_scale.max(1.0);
        let req_px_w = ((total_dip_w * scale).ceil() as u32).clamp(64, 4096);
        let req_px_h = ((total_dip_h * scale).ceil() as u32).clamp(32, 512);
        let dpi = 96.0 * scale;

        unsafe {
            let need_alloc = {
                let s = &self.row_fx_slots[slot];
                s.bitmap.is_none() || req_px_w > s.alloc_w || req_px_h > s.alloc_h
            };
            if need_alloc {
                let alloc_w = ((req_px_w + 255) & !255).clamp(256, 4096);
                let alloc_h = ((req_px_h + 63) & !63).clamp(64, 512);
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: dpi,
                    dpiY: dpi,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                    colorContext: std::mem::ManuallyDrop::new(None),
                };
                let bmp = self
                    .dc
                    .CreateBitmap(
                        D2D_SIZE_U {
                            width: alloc_w,
                            height: alloc_h,
                        },
                        None,
                        0,
                        &props,
                    )
                    .ok()?;
                self.row_fx_slots[slot].alloc_w = alloc_w;
                self.row_fx_slots[slot].alloc_h = alloc_h;
                self.row_fx_slots[slot].bitmap = Some(bmp);
                self.gpu_staging_dirty = true;
            }
            self.row_fx_slots[slot].used_dip_w = total_dip_w;
            self.row_fx_slots[slot].used_dip_h = total_dip_h;
            self.row_fx_slots[slot].pad_dip = pad_dip;

            let bmp_ref = self.row_fx_slots[slot].bitmap.as_ref()?;
            self.dc.SetTarget(bmp_ref);
            self.dc.SetDpi(dpi, dpi);
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
        }
        Some(pad_dip)
    }

    pub fn end_row_fx_bake(&mut self) {
        self.end_active_lyrics_glow_slot();
    }

    pub fn draw_row_fx_blurred(&self, slot: usize, row_x: f32, row_y: f32, sigma: f32, alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        if a <= 0.008 || slot >= self.row_fx_slots.len() {
            return;
        }
        let s = &self.row_fx_slots[slot];
        let Some(ref bmp) = s.bitmap else {
            return;
        };
        let dst_x = row_x - s.pad_dip;
        let dst_y = row_y - s.pad_dip;
        let src_rc = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: s.used_dip_w,
            bottom: s.used_dip_h,
        };
        unsafe {
            if sigma <= 0.25 || self.blur_effect.is_none() {
                let dst_rc = D2D_RECT_F {
                    left: dst_x,
                    top: dst_y,
                    right: dst_x + s.used_dip_w,
                    bottom: dst_y + s.used_dip_h,
                };
                self.dc.DrawBitmap(
                    bmp,
                    Some(&dst_rc),
                    a,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    Some(&src_rc),
                    None,
                );
                return;
            }
            let Some(ref blur) = self.blur_effect else {
                return;
            };
            blur.SetInput(0, bmp, true);
            let sig = sigma.clamp(0.25, 24.0);
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &sig.to_le_bytes(),
            );
            let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_SPEED.0 as u32;
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                D2D1_PROPERTY_TYPE_UINT32,
                &opt.to_le_bytes(),
            );
            let border = D2D1_BORDER_MODE_SOFT.0 as u32;
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_UINT32,
                &border.to_le_bytes(),
            );
            if let Ok(out_img) = blur.GetOutput() {
                if a < 0.995 {
                    let bounds = RectF::new(
                        dst_x - 24.0,
                        dst_y - 24.0,
                        s.used_dip_w + 48.0,
                        s.used_dip_h + 48.0,
                    );
                    let lp = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: to_d2d_rect(bounds),
                        geometricMask: std::mem::ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: Matrix3x2::identity(),
                        opacity: a,
                        opacityBrush: std::mem::ManuallyDrop::new(None),
                        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                    };
                    self.dc.PushLayer(&lp, None);
                    self.dc.DrawImage(
                        &out_img,
                        Some(&pt2(dst_x, dst_y)),
                        Some(&src_rc),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                    self.dc.PopLayer();
                } else {
                    self.dc.DrawImage(
                        &out_img,
                        Some(&pt2(dst_x, dst_y)),
                        Some(&src_rc),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                }
            }
        }
    }

    /// Rebuilt Apple Music dynamic flowing aurora ("流光", `free_blur = true`) and deep atmospheric
    /// creamy bokeh ("柔焦", `free_blur = false`) background renderer on a `320x320` GPU surface.
    /// Uses a full-bleed overscaled anchor cover + 3 radial-feathered orbiting cover currents (ZERO
    /// visible rectangular cover corners!) + 4 organic drifting palette orbs + Rec.709 vibrancy boost
    /// + balanced Gaussian blur (`sigma = 18.0` for 流光, `sigma = 38.0` for 柔焦). Must be called at layer depth 0!
    pub fn draw_lyrics_artwork_mesh_bg(
        &mut self,
        win_rect: RectF,
        artwork_key: Option<&str>,
        phase: f32,
        bass_energy: f32,
        palette: &[Color; 4],
        free_blur: bool,
    ) -> bool {
        if self.target_bitmap.is_none() || self.mesh_blur_effect.is_none() {
            return false;
        }
        let art_bmp = artwork_key.and_then(|k| {
            let stem = crate::am_api::artwork_canonical_stem(k);
            let art = self
                .hero_cache
                .get(&stem)
                .or_else(|| self.thumb_cache.get(&stem))?;
            self.touch_art_entry(art);
            Some((art.bitmap.clone(), art.width as f32, art.height as f32))
        });
        let Some((cov_bmp, aw, ah)) = art_bmp else {
            return false;
        };
        let stem = artwork_key
            .map(crate::am_api::artwork_canonical_stem)
            .unwrap_or_default();

        const MESH_DIM: u32 = 320;
        const MESH_F: f32 = MESH_DIM as f32;
        const HALF_F: f32 = MESH_F * 0.5;

        unsafe {
            let need_alloc = self.lyrics_mesh_bmp.is_none()
                || self.lyrics_mesh_out_bmp.is_none()
                || self
                    .lyrics_mesh_bmp
                    .as_ref()
                    .is_some_and(|b| b.GetPixelSize().width != MESH_DIM);
            if need_alloc {
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                    colorContext: std::mem::ManuallyDrop::new(None),
                };
                let sz = D2D_SIZE_U {
                    width: MESH_DIM,
                    height: MESH_DIM,
                };
                self.lyrics_mesh_bmp = self.dc.CreateBitmap(sz, None, 0, &props).ok();
                self.lyrics_mesh_out_bmp = self.dc.CreateBitmap(sz, None, 0, &props).ok();
                self.gpu_staging_dirty = true;
                self.mesh_last_bake_at = None;
            }

            let need_rebake = need_alloc
                || self.mesh_last_art_stem != stem
                || self.mesh_last_free_blur != free_blur
                || self
                    .mesh_last_bake_at
                    .map(|t| t.elapsed() >= std::time::Duration::from_millis(33))
                    .unwrap_or(true);

            let (Some(ref src_bmp), Some(ref out_bmp), Some(ref blur)) = (
                &self.lyrics_mesh_bmp,
                &self.lyrics_mesh_out_bmp,
                &self.mesh_blur_effect,
            ) else {
                return false;
            };

            if need_rebake {
                self.dc.SetTarget(src_bmp);
                self.dc.SetDpi(96.0, 96.0);
                self.dc.SetTransform(&Matrix3x2::identity());

                let base = palette[0]
                    .lerp(palette[2], 0.40)
                    .lerp(Color::rgb(10, 10, 16), 0.44);
                self.dc.Clear(Some(&to_d2d_color(base)));

                let full_mesh_rc = D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: MESH_F,
                    bottom: MESH_F,
                };
                let cov_dst = D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: aw,
                    bottom: ah,
                };
                let hw = aw * 0.5;
                let hh = ah * 0.5;
                let breath = 1.0 + bass_energy * 0.10;

                // Layer 1: Full-bleed overscaled anchor cover (560px > 320*sqrt(2) = 452.5px) so its
                // 4 square edges ALWAYS stay completely outside the 320x320 canvas at every angle!
                {
                    let ang = phase * 0.14;
                    let (sa, ca) = ang.sin_cos();
                    let draw_sz = 560.0 * breath;
                    let sx = draw_sz / aw.max(1.0);
                    let sy = draw_sz / ah.max(1.0);
                    let m11 = sx * ca;
                    let m12 = sx * sa;
                    let m21 = -sy * sa;
                    let m22 = sy * ca;
                    let cx = HALF_F + (phase * 0.21).sin() * 24.0;
                    let cy = HALF_F + (phase * 0.17).cos() * 20.0;
                    self.dc.SetTransform(&Matrix3x2 {
                        M11: m11,
                        M12: m12,
                        M21: m21,
                        M22: m22,
                        M31: cx - hw * m11 - hh * m21,
                        M32: cy - hw * m12 - hh * m22,
                    });
                    self.dc.DrawBitmap(
                        &cov_bmp,
                        Some(&cov_dst),
                        0.92,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                }

                // Layer 2: 3 counter-orbiting cover currents masked by a cached smooth radial
                // opacity gradient (`mesh_feather_brush`) that fades to 0 well inside the cover
                // bounds — eliminating EVERY possible rectangular edge or corner!
                if let Some(ref feather) = self.mesh_feather_brush {
                    let currents = [
                        (
                            HALF_F + (phase * 0.28).cos() * 76.0,
                            HALF_F + (phase * 0.22).sin() * 68.0,
                            -phase * 0.25 + 1.1,
                            370.0 * breath,
                            148.0f32,
                            132.0f32,
                            if free_blur { 0.86f32 } else { 0.78f32 },
                        ),
                        (
                            HALF_F + (phase * 0.23 + 2.1).sin() * 82.0,
                            HALF_F + (phase * 0.31 + 0.8).cos() * 74.0,
                            phase * 0.21 + 2.6,
                            356.0,
                            142.0,
                            154.0,
                            if free_blur { 0.80 } else { 0.72 },
                        ),
                        (
                            HALF_F + (phase * 0.19 + 4.2).cos() * 68.0,
                            HALF_F + (phase * 0.26 + 3.4).sin() * 76.0,
                            -phase * 0.18 + 4.3,
                            384.0 * breath,
                            152.0,
                            138.0,
                            if free_blur { 0.74 } else { 0.66 },
                        ),
                    ];
                    for (ox, oy, ang, draw_sz, rx, ry, alpha) in currents {
                        self.dc.SetTransform(&Matrix3x2::identity());
                        feather.SetOpacity(alpha);
                        feather.SetTransform(&Matrix3x2 {
                            M11: rx,
                            M12: 0.0,
                            M21: 0.0,
                            M22: ry,
                            M31: ox,
                            M32: oy,
                        });
                        let lp = D2D1_LAYER_PARAMETERS1 {
                            contentBounds: full_mesh_rc,
                            geometricMask: std::mem::ManuallyDrop::new(None),
                            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                            maskTransform: Matrix3x2::identity(),
                            opacity: 1.0,
                            opacityBrush: std::mem::ManuallyDrop::new(Some(
                                feather.cast().unwrap(),
                            )),
                            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                        };
                        self.dc.PushLayer(&lp, None);

                        let (sa, ca) = ang.sin_cos();
                        let sx = draw_sz / aw.max(1.0);
                        let sy = draw_sz / ah.max(1.0);
                        let m11 = sx * ca;
                        let m12 = sx * sa;
                        let m21 = -sy * sa;
                        let m22 = sy * ca;
                        self.dc.SetTransform(&Matrix3x2 {
                            M11: m11,
                            M12: m12,
                            M21: m21,
                            M22: m22,
                            M31: ox - hw * m11 - hh * m21,
                            M32: oy - hw * m12 - hh * m22,
                        });
                        self.dc.DrawBitmap(
                            &cov_bmp,
                            Some(&cov_dst),
                            1.0,
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            None,
                            None,
                        );
                        self.dc.SetTransform(&Matrix3x2::identity());
                        self.dc.PopLayer();
                        let _ = std::mem::ManuallyDrop::into_inner(lp.opacityBrush);
                    }
                }

                // Layer 3: 4 organic drifting palette color orbs for rich liquid aurora streams
                self.dc.SetTransform(&Matrix3x2::identity());
                let orb_a = if free_blur { 0.46f32 } else { 0.38f32 };
                let orbs = [
                    (
                        HALF_F + (phase * 0.31).sin() * 112.0,
                        HALF_F + (phase * 0.23).cos() * 98.0,
                        182.0 * breath,
                        152.0,
                        palette[0],
                        orb_a,
                    ),
                    (
                        HALF_F + (phase * 0.27 + 1.8).cos() * 118.0,
                        HALF_F + (phase * 0.34 + 0.9).sin() * 104.0,
                        164.0,
                        190.0 * breath,
                        palette[1],
                        orb_a * 0.92,
                    ),
                    (
                        HALF_F + (phase * 0.21 + 3.4).sin() * 106.0,
                        HALF_F + (phase * 0.29 + 2.5).cos() * 114.0,
                        196.0,
                        162.0,
                        palette[2],
                        orb_a * 0.88,
                    ),
                    (
                        HALF_F + (phase * 0.35 + 4.9).cos() * 96.0,
                        HALF_F + (phase * 0.19 + 4.1).sin() * 108.0,
                        174.0 * breath,
                        178.0,
                        palette[3],
                        orb_a * 0.84,
                    ),
                ];
                for (ox, oy, rx, ry, col, a) in orbs {
                    self.fill_radial_glow(ox, oy, rx, ry, col.with_alpha(a), col.with_alpha(0.0));
                }

                // Step B: Commit `src_bmp` (30 Hz only) before Rec.709 vibrancy + GaussianBlur reads it!
                self.flush_dc();
                self.dc.SetTarget(out_bmp);
                self.dc.SetDpi(96.0, 96.0);
                self.dc.SetTransform(&Matrix3x2::identity());
                self.dc.Clear(Some(&to_d2d_color(base)));

                let blur_input: ID2D1Image = if let Some(ref vib) = self.mesh_vibrancy_effect {
                    vib.SetInput(0, src_bmp, true);
                    let mb = if free_blur {
                        mat5x4_bytes(Self::build_glass_vibrancy_matrix(1.44, 1.06, 0.010))
                    } else {
                        mat5x4_bytes(Self::build_glass_vibrancy_matrix(1.38, 1.03, 0.005))
                    };
                    let _ = vib.SetValue(
                        D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                        D2D1_PROPERTY_TYPE_BLOB,
                        &mb,
                    );
                    vib.GetOutput()
                        .unwrap_or_else(|_| src_bmp.cast().unwrap())
                } else {
                    src_bmp.cast().unwrap()
                };

                // "流光" (`free_blur = true`): silky flowing aurora (`sigma = 18.0`, zero corners!)
                // "柔焦" (`free_blur = false`): deep atmospheric creamy bokeh (`sigma = 38.0`)
                let sigma = if free_blur { 18.0f32 } else { 38.0f32 };
                blur.SetInput(0, &blur_input, true);
                let _ = blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &sigma.to_le_bytes(),
                );
                let opt = D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32;
                let _ = blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                    D2D1_PROPERTY_TYPE_UINT32,
                    &opt.to_le_bytes(),
                );
                let border = D2D1_BORDER_MODE_HARD.0 as u32;
                let _ = blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_UINT32,
                    &border.to_le_bytes(),
                );
                if let Ok(blurred_img) = blur.GetOutput() {
                    self.dc.DrawImage(
                        &blurred_img,
                        Some(&pt2(0.0, 0.0)),
                        Some(&full_mesh_rc),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                }

                // Restore active draw target (main backbuffer or lyrics transition sheet)
                if let Some(main_tgt) = self.active_draw_target() {
                    self.dc.SetTarget(main_tgt);
                    let dpi = 96.0 * self.dpi_scale;
                    self.dc.SetDpi(dpi, dpi);
                }
                self.dc.SetTransform(&Matrix3x2::identity());
                self.mesh_last_bake_at = Some(std::time::Instant::now());
                self.mesh_last_art_stem = stem;
                self.mesh_last_free_blur = free_blur;
                self.mesh_aurora_bake_seq = self.mesh_aurora_bake_seq.wrapping_add(1);
            }

            // Step C: Blit silky 320x320 aurora mesh across window with inner crop & LINEAR filtering
            let crop_src = D2D_RECT_F {
                left: 16.0,
                top: 16.0,
                right: MESH_F - 16.0,
                bottom: MESH_F - 16.0,
            };
            self.dc.DrawBitmap(
                out_bmp,
                Some(&to_d2d_rect(win_rect)),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                Some(&crop_src),
                None,
            );
        }
        true
    }

    // ========================================================================
    // Bounded Off-Thread WIC Artwork Decoder, Canonical Stem Cache & Fade-In
    // ========================================================================

    /// Right-sized decode ceiling for an artwork usage tier (`SmallThumb=128`, `Card=384/256`,
    /// `Hero=720/512`). Using `720` for Hero matches Apple Music's native `720x720bb.jpg` 1:1 so
    /// WIC skips the Fant scaler completely!
    #[inline]
    pub fn decode_dim_for_tier(tier: ArtworkSizeTier, aggressive_mem: bool) -> u32 {
        match tier {
            ArtworkSizeTier::SmallThumb => 128,
            ArtworkSizeTier::Card => {
                if aggressive_mem {
                    256
                } else {
                    384
                }
            }
            ArtworkSizeTier::Hero => {
                if aggressive_mem {
                    512
                } else {
                    720
                }
            }
        }
    }

    /// Computes a right-sized physical decode dimension (`128` / `256` / `384` / `512` / `720`)
    /// from the logical DIP slot size and current monitor DPI scale.
    #[inline]
    pub fn recommended_artwork_decode_dim(&self, target_dip: f32, hero_res: bool) -> u32 {
        if hero_res {
            Self::decode_dim_for_tier(ArtworkSizeTier::Hero, self.aggressive_memory_release)
        } else {
            let px = (target_dip.max(16.0) * self.dpi_scale.max(1.0)).ceil() as u32;
            if px <= 128 {
                128
            } else if px <= 256 || self.aggressive_memory_release {
                256
            } else {
                384
            }
        }
    }

    /// Thread-safe background CPU WIC decode using a named [`ArtworkSizeTier`].
    pub fn decode_artwork_pixels_for_tier(
        bytes: &[u8],
        tier: ArtworkSizeTier,
        aggressive_mem: bool,
    ) -> Option<DecodedArtworkCpu> {
        Self::decode_artwork_pixels_cpu(bytes, Self::decode_dim_for_tier(tier, aggressive_mem))
    }

    /// Thread-safe background CPU WIC decode + palette extraction (keeps UI thread 100% stutter-free!).
    /// Reuses a per-worker `thread_local!` `IWICImagingFactory` paired with an RAII `WicComGuard`
    /// that balances `CoInitializeEx` with `CoUninitialize` cleanly on worker thread exit (after
    /// `IWICImagingFactory` releases), skips Fant scaler when dimensions already fit, and omits
    /// dead alpha overwrite (`D2D1_ALPHA_MODE_IGNORE` ignores alpha on upload).
    pub fn decode_artwork_pixels_cpu(bytes: &[u8], max_dim: u32) -> Option<DecodedArtworkCpu> {
        if bytes.len() < 16 {
            return None;
        }
        struct WicComGuard {
            should_uninit: bool,
        }
        impl Drop for WicComGuard {
            fn drop(&mut self) {
                if self.should_uninit {
                    unsafe {
                        CoUninitialize();
                    }
                }
            }
        }
        struct ThreadLocalWic {
            // Field declaration order guarantees `factory` releases BEFORE `_com_guard` calls `CoUninitialize()`!
            factory: IWICImagingFactory,
            _com_guard: WicComGuard,
        }
        thread_local! {
            static WORKER_WIC: std::cell::RefCell<Option<ThreadLocalWic>> =
                const { std::cell::RefCell::new(None) };
        }
        unsafe {
            let wic: IWICImagingFactory = WORKER_WIC.with(|cell| {
                if let Some(ref existing) = *cell.borrow() {
                    return Some(existing.factory.clone());
                }
                let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
                let guard = WicComGuard {
                    should_uninit: hr.is_ok(),
                };
                let created: IWICImagingFactory =
                    CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
                let cloned = created.clone();
                *cell.borrow_mut() = Some(ThreadLocalWic {
                    factory: created,
                    _com_guard: guard,
                });
                Some(cloned)
            })?;
            let stream = SHCreateMemStream(Some(bytes))?;
            let decoder = wic
                .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let mut src_w = 0u32;
            let mut src_h = 0u32;
            frame.GetSize(&mut src_w, &mut src_h).ok()?;
            if src_w == 0 || src_h == 0 {
                return None;
            }

            let scale = ((max_dim as f32) / (src_w.max(src_h) as f32)).min(1.0);
            let dst_w = ((src_w as f32 * scale).round() as u32).max(1);
            let dst_h = ((src_h as f32 * scale).round() as u32).max(1);

            let conv = wic.CreateFormatConverter().ok()?;
            conv.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;

            let stride = dst_w * 4;
            let mut pixels = vec![0u8; (stride * dst_h) as usize];
            if dst_w == src_w && dst_h == src_h {
                conv.CopyPixels(std::ptr::null(), stride, &mut pixels).ok()?;
            } else {
                let scaler = wic.CreateBitmapScaler().ok()?;
                scaler
                    .Initialize(&conv, dst_w, dst_h, WICBitmapInterpolationModeFant)
                    .ok()?;
                scaler
                    .CopyPixels(std::ptr::null(), stride, &mut pixels)
                    .ok()?;
            }
            let palette = extract_quadrant_palette(&pixels, dst_w, dst_h);

            Some(DecodedArtworkCpu {
                pixels,
                width: dst_w,
                height: dst_h,
                palette,
            })
        }
    }

    #[inline]
    fn touch_art_entry(&self, art: &CachedArtwork) {
        let next = self.art_lru_clock.get().wrapping_add(1);
        self.art_lru_clock.set(next);
        art.last_used.set(next);
    }

    pub fn has_artwork(&self, key: &str, hero_res: bool) -> bool {
        let stem = crate::am_api::artwork_canonical_stem(key);
        if hero_res {
            self.hero_cache.contains_key(&stem)
        } else {
            self.thumb_cache.contains_key(&stem) || self.hero_cache.contains_key(&stem)
        }
    }

    /// Returns `true` when `key` is already cached with resolution `>= min_dim` pixels.
    pub fn has_artwork_min_dim(&self, key: &str, hero_res: bool, min_dim: u32) -> bool {
        let stem = crate::am_api::artwork_canonical_stem(key);
        let entry = if hero_res {
            self.hero_cache.get(&stem)
        } else {
            self.hero_cache
                .get(&stem)
                .or_else(|| self.thumb_cache.get(&stem))
        };
        entry.is_some_and(|c| c.width.max(c.height) >= min_dim)
    }

    pub fn has_fading_artwork(&self) -> bool {
        self.last_art_fade_at
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(200))
    }

    pub fn get_artwork_palette(&self, key: &str) -> Option<[Color; 4]> {
        let stem = crate::am_api::artwork_canonical_stem(key);
        self.hero_cache
            .get(&stem)
            .or_else(|| self.thumb_cache.get(&stem))
            .map(|c| c.palette)
    }

    pub fn upload_decoded_artwork(
        &mut self,
        key: String,
        decoded_cpu: DecodedArtworkCpu,
        hero_res: bool,
    ) -> bool {
        let stem = crate::am_api::artwork_canonical_stem(&key);
        let incoming_dim = decoded_cpu.width.max(decoded_cpu.height);

        // No-downgrade guard: never replace a sharper already-cached bitmap (e.g. 384px card) with
        // a smaller tier decode (e.g. 128px list thumb) for the same canonical artwork stem!
        if !hero_res {
            if let Some(existing) = self.thumb_cache.get(&stem) {
                if existing.width.max(existing.height) > incoming_dim {
                    self.touch_art_entry(existing);
                    return true;
                }
            }
        } else if let Some(existing) = self.hero_cache.get(&stem) {
            if existing.width.max(existing.height) > incoming_dim {
                self.touch_art_entry(existing);
                return true;
            }
        }

        let already_visible =
            self.thumb_cache.contains_key(&stem) || self.hero_cache.contains_key(&stem);
        let is_brand_new = self.seen_art_stems.insert(stem.clone()) && !already_visible;
        if self.seen_art_stems.len() > 2048 {
            self.seen_art_stems.clear();
        }

        let stride = decoded_cpu.width * 4;
        let (bitmap, brush) = unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let Ok(bmp) = self.dc.CreateBitmap(
                D2D_SIZE_U {
                    width: decoded_cpu.width,
                    height: decoded_cpu.height,
                },
                Some(decoded_cpu.pixels.as_ptr() as *const c_void),
                stride,
                &props,
            ) else {
                return false;
            };
            let b_props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
            };
            let Ok(bbrush) = self.dc.CreateBitmapBrush(&bmp, Some(&b_props), None) else {
                return false;
            };
            (bmp, bbrush)
        };

        let now = std::time::Instant::now();
        if is_brand_new {
            self.last_art_fade_at = Some(now);
        }
        let stamp = self.art_lru_clock.get().wrapping_add(1);
        self.art_lru_clock.set(stamp);
        let cached = CachedArtwork {
            bitmap,
            brush,
            width: decoded_cpu.width,
            height: decoded_cpu.height,
            palette: decoded_cpu.palette,
            fade_in_start: is_brand_new.then_some(now),
            last_used: Cell::new(stamp),
        };

        if hero_res {
            let max_heroes = if self.aggressive_memory_release { 8 } else { 24 };
            if !self.hero_cache.contains_key(&stem) {
                while self.hero_cache.len() >= max_heroes {
                    if let Some(old) = self
                        .hero_cache
                        .iter()
                        .min_by_key(|(_, v)| v.last_used.get())
                        .map(|(k, _)| k.clone())
                    {
                        self.hero_cache.remove(&old);
                    } else {
                        break;
                    }
                }
            }
            if self.mesh_last_art_stem == stem {
                self.mesh_last_bake_at = None;
            }
            self.hero_cache.insert(stem, cached);
        } else {
            let max_thumbs = if self.aggressive_memory_release { 84 } else { 220 };
            if !self.thumb_cache.contains_key(&stem) {
                while self.thumb_cache.len() >= max_thumbs {
                    if let Some(old) = self
                        .thumb_cache
                        .iter()
                        .min_by_key(|(_, v)| v.last_used.get())
                        .map(|(k, _)| k.clone())
                    {
                        self.thumb_cache.remove(&old);
                    } else {
                        break;
                    }
                }
            }
            if self.mesh_last_art_stem == stem {
                self.mesh_last_bake_at = None;
            }
            self.thumb_cache.insert(stem, cached);
        }
        self.gpu_staging_dirty = true;
        true
    }

    pub fn insert_artwork_bytes(&mut self, key: String, bytes: &[u8], hero_res: bool) -> bool {
        let tier = if hero_res {
            ArtworkSizeTier::Hero
        } else {
            ArtworkSizeTier::Card
        };
        let max_dim = Self::decode_dim_for_tier(tier, self.aggressive_memory_release);
        self.insert_artwork_bytes_for_dim(key, bytes, hero_res, max_dim)
    }

    pub fn insert_artwork_bytes_for_dim(
        &mut self,
        key: String,
        bytes: &[u8],
        hero_res: bool,
        max_dim: u32,
    ) -> bool {
        let Some(decoded) = Self::decode_artwork_pixels_cpu(bytes, max_dim.clamp(64, 1024)) else {
            return false;
        };
        self.upload_decoded_artwork(key, decoded, hero_res)
    }

    /// Draws a classic Apple Cover Flow 2.5D tilted album sleeve with subtle side shading
    /// and a glossy inverted mirror reflection fading cleanly into the dark floor.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_cover_flow_sleeve(
        &self,
        art_url: Option<&str>,
        rect: RectF,
        tilt_skew_y: f32,
        scale_x: f32,
        dim_alpha: f32,
        reflection_alpha: f32,
        is_center: bool,
    ) {
        if rect.w <= 2.0 || rect.h <= 2.0 {
            return;
        }
        let cx = rect.center_x();
        let cy = rect.center_y();
        let sx = scale_x.clamp(0.35, 1.25);
        let sk = tilt_skew_y.clamp(-0.42, 0.42);
        let sleeve_xf = Matrix3x2 {
            M11: sx,
            M12: sk,
            M21: 0.0,
            M22: 1.0,
            M31: cx * (1.0 - sx),
            M32: -cx * sk,
        };
        let radius = if is_center { 12.0 } else { 9.0 };

        // 1. Glossy inverted mirror reflection beneath the sleeve (exact parallel tilt with bottom edge)
        if reflection_alpha > 0.02 {
            let refl_h = (rect.h * 0.46).max(24.0);
            let floor_y = rect.bottom() + 2.0;
            // Flip vertically around `floor_y` while preserving identical `+sk` vertical shear so
            // the reflected top edge stays parallel and 4 DIP below the tilted sleeve bottom edge.
            let refl_xf = Matrix3x2 {
                M11: sx,
                M12: sk,
                M21: 0.0,
                M22: -1.0,
                M31: cx * (1.0 - sx),
                M32: floor_y * 2.0 - cx * sk,
            };
            self.set_transform(&refl_xf);
            let refl_clip = RectF::new(
                rect.x - 2.0,
                rect.bottom() - refl_h,
                rect.w + 4.0,
                refl_h + 2.0,
            );
            self.push_clip(refl_clip);
            let drew_ref = art_url
                .filter(|u| !u.is_empty())
                .is_some_and(|u| self.draw_rounded_artwork(u, is_center, rect, radius, reflection_alpha * 0.42));
            if !drew_ref {
                self.fill_rounded_rect(
                    rect,
                    radius,
                    Theme::BG_ELEVATED.with_alpha(reflection_alpha * 0.35),
                );
            }
            self.pop_clip();
            self.set_transform(&sleeve_xf);
            let fade_r = RectF::new(rect.x - 4.0, floor_y, rect.w + 8.0, refl_h + 10.0);
            self.fill_linear_gradient_rect(
                fade_r,
                0.0,
                fade_r.x,
                fade_r.y,
                fade_r.x,
                fade_r.bottom(),
                Color::rgba(14, 14, 18, 0.30),
                Color::rgba(14, 14, 18, 1.0),
            );
        }

        // 2. Main 2.5D Album Sleeve
        self.set_transform(&sleeve_xf);
        let shadow_off = if is_center { 12.0 } else { 6.0 };
        self.fill_rounded_rect(
            RectF::new(rect.x + 2.0, rect.y + shadow_off, rect.w - 4.0, rect.h),
            radius,
            Color::rgba(0, 0, 0, if is_center { 0.52 } else { 0.36 }),
        );
        let drew = art_url
            .filter(|u| !u.is_empty())
            .is_some_and(|u| self.draw_rounded_artwork(u, is_center, rect, radius, 1.0));
        if !drew {
            self.fill_rounded_rect(rect, radius, Theme::BG_ELEVATED);
            self.draw_icon(
                SfIcon::Albums,
                rect.inset(rect.w * 0.32, rect.h * 0.32),
                Theme::TEXT_MUTED,
            );
        }
        if dim_alpha > 0.01 {
            self.fill_rounded_rect(rect, radius, Color::rgba(6, 6, 10, dim_alpha.clamp(0.0, 0.85)));
        }
        self.stroke_rounded_rect(
            rect,
            radius,
            if is_center {
                Color::rgba(255, 255, 255, 0.28)
            } else {
                Color::rgba(255, 255, 255, 0.12)
            },
            if is_center { 1.3 } else { 1.0 },
        );
        let _ = cy;
        self.reset_transform();
    }

    pub fn draw_rounded_artwork(
        &self,
        key: &str,
        hero_res: bool,
        rect: RectF,
        radius: f32,
        opacity: f32,
    ) -> bool {
        let stem = crate::am_api::artwork_canonical_stem(key);
        let entry = if hero_res {
            self.hero_cache
                .get(&stem)
                .or_else(|| self.thumb_cache.get(&stem))
        } else {
            self.thumb_cache
                .get(&stem)
                .or_else(|| self.hero_cache.get(&stem))
        };
        let Some(art) = entry else {
            return false;
        };
        self.touch_art_entry(art);
        if rect.w <= 1.0 || rect.h <= 1.0 || opacity <= 0.001 {
            return true;
        }

        // Smooth 180ms cubic ease-out fade-in ONLY on brand-new unseen covers;
        // already-cached or resolution-upgraded covers render at full opacity with ZERO black frame!
        let fade_mul = if let Some(t0) = art.fade_in_start {
            let u = (t0.elapsed().as_secs_f32() / 0.18).clamp(0.0, 1.0);
            if u < 0.999 {
                self.fill_rounded_rect(rect, radius, Theme::BG_ELEVATED);
                let inv = 1.0 - u;
                0.08 + 0.92 * (1.0 - inv * inv * inv)
            } else {
                1.0
            }
        } else {
            1.0
        };
        let final_opacity = (opacity * fade_mul).clamp(0.0, 1.0);

        unsafe {
            // Uniform aspect-preserving center-crop cover fill (prevents stretching 1:1 covers into 16:9 heroes!)
            let fw = (art.width as f32).max(1.0);
            let fh = (art.height as f32).max(1.0);
            let scale = (rect.w / fw).max(rect.h / fh);
            let draw_w = fw * scale;
            let draw_h = fh * scale;
            let tx = rect.x + (rect.w - draw_w) * 0.5;
            let ty = rect.y + (rect.h - draw_h) * 0.5;
            art.brush.SetOpacity(final_opacity);
            art.brush.SetTransform(&Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: tx,
                M32: ty,
            });

            if radius >= rect.w * 0.49 && (rect.w - rect.h).abs() < 2.0 {
                let el = D2D1_ELLIPSE {
                    point: pt2(rect.center_x(), rect.center_y()),
                    radiusX: rect.w * 0.5,
                    radiusY: rect.h * 0.5,
                };
                self.dc.FillEllipse(&el, &art.brush);
            } else {
                let rr = D2D1_ROUNDED_RECT {
                    rect: to_d2d_rect(rect),
                    radiusX: radius,
                    radiusY: radius,
                };
                self.dc.FillRoundedRectangle(&rr, &art.brush);
            }
        }
        true
    }

    pub fn invalidate_motion_texture(&mut self, key: &str) {
        if self.motion_textures.remove(key).is_some() {
            self.gpu_staging_dirty = true;
        }
    }

    fn evict_motion_textures_if_full(&mut self, incoming_key: &str) {
        if self.motion_textures.contains_key(incoming_key) {
            return;
        }
        while self.motion_textures.len() >= MOTION_TEXTURE_LRU_CAP {
            if let Some(evict_k) = self
                .motion_textures
                .iter()
                .min_by_key(|(_, v)| v.last_used_frame)
                .map(|(k, _)| k.clone())
            {
                self.motion_textures.remove(&evict_k);
            } else {
                break;
            }
        }
    }

    /// Opacity-aware alias for [`D2DRenderer::draw_motion_artwork`] for smooth static-to-motion
    /// crossfades.
    pub fn draw_motion_artwork_alpha(
        &mut self,
        frame: &crate::motion_art::MotionFrameSnapshot,
        rect: RectF,
        radius: f32,
        opacity: f32,
    ) -> bool {
        self.draw_motion_artwork(frame, rect, radius, opacity)
    }

    /// Returns how many DXVA ping-pong surface slots (0..=2) are currently cached for `key`.
    pub fn motion_dxgi_cached_slot_count(&self, key: &str) -> usize {
        self.motion_textures
            .get(key)
            .map(|e| e.dxgi_slots.iter().filter(|s| s.is_some()).count())
            .unwrap_or(0)
    }

    /// Zero-copy DXGI (`CreateBitmapFromDxgiSurface`, 2-slot ping-pong cache matching double-buffered
    /// DXVA `out_tex`) or target-resolution CPU-fallback (`CopyFromMemory`) renderer for
    /// `MotionFrameSnapshot` with a bounded 28-entry LRU cache, `LINEAR` aspect-fill scaling, and zero `PushLayer`.
    pub fn draw_motion_artwork(
        &mut self,
        frame: &crate::motion_art::MotionFrameSnapshot,
        rect: RectF,
        radius: f32,
        opacity: f32,
    ) -> bool {
        let a = opacity.clamp(0.0, 1.0);
        if a <= 0.005 || rect.w <= 1.0 || rect.h <= 1.0 || frame.width == 0 || frame.height == 0 {
            return false;
        }

        self.motion_lru_clock = self.motion_lru_clock.wrapping_add(1);
        let touch_stamp = self.motion_lru_clock;
        let mut used_dxva = false;

        // 1. Zero-copy DXVA GPU surface path (check BEFORE `frame.bgra.len()` because DXVA frames
        //    intentionally leave `bgra` empty to avoid CPU RAM waste!).
        if let Some(ref dxgi_tex) = frame.dxgi_texture {
            let tex_ptr = dxgi_tex.as_raw() as usize;
            let mut hit_new_seq = false;
            let cache_hit = if let Some(entry) = self.motion_textures.get_mut(&frame.key) {
                let matched_idx = entry.dxgi_slots.iter().position(|slot_opt| {
                    slot_opt.as_ref().is_some_and(|s| {
                        s.dxgi_tex_ptr == tex_ptr
                            && s.width == frame.width
                            && s.height == frame.height
                    })
                });
                if let Some(idx) = matched_idx {
                    entry.active_dxgi_slot = Some(idx);
                    entry.dxgi_next_replace = 1 - idx;
                    entry.last_used_frame = touch_stamp;
                    if entry.cached_seq != frame.frame_seq {
                        entry.cached_seq = frame.frame_seq;
                        hit_new_seq = true;
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if cache_hit {
                if hit_new_seq {
                    self.bump_stat(|st| st.motion_dxva_frames += 1);
                }
                used_dxva = true;
            } else {
                let wrapped = unsafe {
                    dxgi_tex.cast::<IDXGISurface>().ok().and_then(|surface| {
                        let make_props = |alpha_mode| D2D1_BITMAP_PROPERTIES1 {
                            pixelFormat: D2D1_PIXEL_FORMAT {
                                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                alphaMode: alpha_mode,
                            },
                            dpiX: 96.0,
                            dpiY: 96.0,
                            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                            colorContext: std::mem::ManuallyDrop::new(None),
                        };
                        let bmp = self
                            .dc
                            .CreateBitmapFromDxgiSurface(
                                &surface,
                                Some(&make_props(D2D1_ALPHA_MODE_IGNORE)),
                            )
                            .or_else(|_| {
                                self.dc.CreateBitmapFromDxgiSurface(
                                    &surface,
                                    Some(&make_props(D2D1_ALPHA_MODE_PREMULTIPLIED)),
                                )
                            })
                            .ok()?;
                        let b_props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                            extendModeX: D2D1_EXTEND_MODE_CLAMP,
                            extendModeY: D2D1_EXTEND_MODE_CLAMP,
                            interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
                        };
                        let bbrush = self.dc.CreateBitmapBrush(&bmp, Some(&b_props), None).ok()?;
                        Some((bmp, bbrush))
                    })
                };
                if let Some((bmp, bbrush)) = wrapped {
                    let new_slot = MotionDxgiSlot {
                        dxgi_tex_ptr: tex_ptr,
                        width: frame.width,
                        height: frame.height,
                        _bitmap: bmp,
                        brush: bbrush,
                        _dxgi_tex_keepalive: dxgi_tex.clone(),
                    };
                    if let Some(entry) = self.motion_textures.get_mut(&frame.key) {
                        // Prefer an empty slot first (for 2-slot ping-pong warmup), else replace the LRU slot
                        let target_idx = if entry.dxgi_slots[0].is_none() {
                            0
                        } else if entry.dxgi_slots[1].is_none() {
                            1
                        } else {
                            entry.dxgi_next_replace & 1
                        };
                        entry.dxgi_slots[target_idx] = Some(new_slot);
                        entry.active_dxgi_slot = Some(target_idx);
                        entry.dxgi_next_replace = 1 - target_idx;
                        entry.cpu_surface = None;
                        entry.cached_seq = frame.frame_seq;
                        entry.last_used_frame = touch_stamp;
                        entry.last_bgra_arc = None;
                    } else {
                        self.evict_motion_textures_if_full(&frame.key);
                        self.motion_textures.insert(
                            frame.key.clone(),
                            MotionTextureEntry {
                                dxgi_slots: [Some(new_slot), None],
                                dxgi_next_replace: 1,
                                active_dxgi_slot: Some(0),
                                cpu_surface: None,
                                cached_seq: frame.frame_seq,
                                last_used_frame: touch_stamp,
                                last_bgra_arc: None,
                            },
                        );
                    }
                    self.gpu_staging_dirty = true;
                    self.bump_stat(|st| st.motion_dxva_frames += 1);
                    used_dxva = true;
                }
            }
        }

        // 2. Target-resolution CPU fallback path (`CopyFromMemory` / initial `CreateBitmap` upload).
        if !used_dxva {
            let need_bytes = (frame.width as usize)
                .saturating_mul(frame.height as usize)
                .saturating_mul(4);
            if frame.bgra.len() < need_bytes {
                return false;
            }

            let need_recreate = self
                .motion_textures
                .get(&frame.key)
                .map(|entry| match entry.cpu_surface {
                    Some((_, _, w, h)) => w != frame.width || h != frame.height,
                    None => true,
                })
                .unwrap_or(true);

            if need_recreate {
                self.evict_motion_textures_if_full(&frame.key);
                unsafe {
                    let props = D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_IGNORE,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    };
                    let Ok(bmp) = self.dc.CreateBitmap(
                        D2D_SIZE_U {
                            width: frame.width,
                            height: frame.height,
                        },
                        Some(frame.bgra.as_ptr() as *const c_void),
                        frame.width * 4,
                        &props,
                    ) else {
                        return false;
                    };
                    let b_props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                        extendModeX: D2D1_EXTEND_MODE_CLAMP,
                        extendModeY: D2D1_EXTEND_MODE_CLAMP,
                        interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
                    };
                    let Ok(bbrush) = self.dc.CreateBitmapBrush(&bmp, Some(&b_props), None) else {
                        return false;
                    };
                    self.motion_textures.insert(
                        frame.key.clone(),
                        MotionTextureEntry {
                            dxgi_slots: [None, None],
                            dxgi_next_replace: 0,
                            active_dxgi_slot: None,
                            cpu_surface: Some((bmp, bbrush, frame.width, frame.height)),
                            cached_seq: frame.frame_seq,
                            last_used_frame: touch_stamp,
                            last_bgra_arc: Some(std::sync::Arc::clone(&frame.bgra)),
                        },
                    );
                    self.gpu_staging_dirty = true;
                    self.bump_stat(|st| {
                        st.motion_copy_from_memory_bytes += need_bytes as u64;
                        st.motion_cpu_fallback_frames += 1;
                    });
                }
            } else if let Some(entry) = self.motion_textures.get_mut(&frame.key) {
                entry.last_used_frame = touch_stamp;
                entry.dxgi_slots = [None, None];
                entry.active_dxgi_slot = None;
                let bgra_changed = !entry
                    .last_bgra_arc
                    .as_ref()
                    .is_some_and(|prev| std::sync::Arc::ptr_eq(prev, &frame.bgra));
                if entry.cached_seq != frame.frame_seq || bgra_changed {
                    if let Some((ref bmp, _, _, _)) = entry.cpu_surface {
                        unsafe {
                            let dst_u = D2D_RECT_U {
                                left: 0,
                                top: 0,
                                right: frame.width,
                                bottom: frame.height,
                            };
                            let _ = bmp.CopyFromMemory(
                                Some(&dst_u),
                                frame.bgra.as_ptr() as *const c_void,
                                frame.width * 4,
                            );
                        }
                    }
                    entry.cached_seq = frame.frame_seq;
                    entry.last_bgra_arc = Some(std::sync::Arc::clone(&frame.bgra));
                    self.bump_stat(|st| {
                        st.motion_copy_from_memory_bytes += need_bytes as u64;
                        st.motion_cpu_fallback_frames += 1;
                    });
                }
            }
        }

        let Some(entry) = self.motion_textures.get(&frame.key) else {
            return false;
        };
        let Some((brush, fw, fh)) = entry
            .active_dxgi_slot
            .and_then(|idx| entry.dxgi_slots[idx].as_ref())
            .map(|s| (&s.brush, (s.width as f32).max(1.0), (s.height as f32).max(1.0)))
            .or_else(|| {
                entry
                    .cpu_surface
                    .as_ref()
                    .map(|(_, b, w, h)| (b, (*w as f32).max(1.0), (*h as f32).max(1.0)))
            })
        else {
            return false;
        };
        let scale = (rect.w / fw).max(rect.h / fh);
        let draw_w = fw * scale;
        let draw_h = fh * scale;
        let tx = rect.x + (rect.w - draw_w) * 0.5;
        let ty = rect.y + (rect.h - draw_h) * 0.5;

        unsafe {
            brush.SetOpacity(a);
            brush.SetTransform(&Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: tx,
                M32: ty,
            });
            if radius <= 0.5 {
                self.dc.FillRectangle(&to_d2d_rect(rect), brush);
            } else {
                let rr = D2D1_ROUNDED_RECT {
                    rect: to_d2d_rect(rect),
                    radiusX: radius,
                    radiusY: radius,
                };
                self.dc.FillRoundedRectangle(&rr, brush);
            }
        }
        true
    }

    /// Real-time audio-reactive vertical equalizer bars (unified 5-bar symmetric style)
    /// for playing track covers and left sidebar `可视化` icon. Settles cleanly when silent/paused!
    pub fn draw_live_equalizer_bars(
        &self,
        rect: RectF,
        color: Color,
        levels: &[f32],
        symmetric_waveform: bool,
    ) {
        let n = levels.len();
        if n == 0 || rect.w <= 2.0 || rect.h <= 2.0 {
            return;
        }
        let cx = rect.center_x();
        let cy = rect.center_y();
        let s = rect.w.min(rect.h) * 0.5;
        let sw = (s * if n <= 4 { 0.28 } else { 0.21 }).clamp(1.5, 2.7);
        let span = s * if n <= 4 { 1.32 } else { 1.38 };
        let x0 = cx - span * 0.5;
        let step = if n > 1 { span / ((n - 1) as f32) } else { 0.0 };

        for (i, &lvl) in levels.iter().enumerate() {
            let bx = x0 + (i as f32) * step;
            let v = lvl.clamp(0.14, 1.0);
            if symmetric_waveform {
                // Sidebar `可视化` waveform glyph: symmetric around vertical center
                let hh = (s * 0.84 * v).max(1.1);
                self.draw_line(bx, cy - hh, bx, cy + hh, color, sw);
            } else {
                // Track cover playing indicator: anchored at bottom baseline, bouncing upward!
                let base_y = cy + s * 0.72;
                let bar_h = (s * 1.44 * v).max(2.2);
                self.draw_line(bx, base_y, bx, base_y - bar_h, color, sw);
            }
        }
    }

    // ========================================================================
    // Crisp Apple SF-Symbol-Inspired Vector Icon Primitives
    // ========================================================================

    /// Smoothly rotates a crisp disclosure chevron between right-pointing (`open_t = 0.0`)
    /// and down-pointing (`open_t = 1.0`) with zero COM geometry allocations!
    pub fn draw_chevron_morph(&self, rect: RectF, open_t: f32, color: Color) {
        let cx = rect.center_x();
        let cy = rect.center_y();
        let s = rect.w.min(rect.h) * 0.5;
        let sw = (s * 0.175).clamp(1.3, 2.0);
        let ang = open_t.clamp(0.0, 1.0) * (PI * 0.5);
        let (sa, ca) = ang.sin_cos();
        let rot = |lx: f32, ly: f32| -> (f32, f32) {
            (cx + lx * ca - ly * sa, cy + lx * sa + ly * ca)
        };
        let p0 = rot(-s * 0.22, -s * 0.46);
        let p1 = rot(s * 0.26, 0.0);
        let p2 = rot(-s * 0.22, s * 0.46);
        self.draw_line(p0.0, p0.1, p1.0, p1.1, color, sw);
        self.draw_line(p1.0, p1.1, p2.0, p2.1, color, sw);
    }

    pub fn draw_icon(&self, icon: SfIcon, rect: RectF, color: Color) {
        // Subtle 1px optical drop shadow behind bright glyphs over crystal-clear Liquid Glass
        // keeps icons sharp even over pure white album covers without muddying the glass!
        if color.a > 0.45
            && (color.r + color.g + color.b) > 1.75
            && self.is_point_on_live_glass(rect.center_x(), rect.center_y())
        {
            self.draw_icon_geom(
                icon,
                RectF::new(rect.x, rect.y + 1.0, rect.w, rect.h),
                Color::rgba(0, 0, 0, 0.45 * color.a),
            );
        }
        self.draw_icon_geom(icon, rect, color);
    }

    fn build_icon_geom(&self, kind: IconGeomKind) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geom = self.d2d_factory.CreatePathGeometry().ok()?;
            let sink = geom.Open().ok()?;
            let add_poly = |pts: &[(f32, f32)], filled: bool, closed: bool| {
                if pts.len() < 2 {
                    return;
                }
                sink.BeginFigure(
                    pt2(pts[0].0, pts[0].1),
                    if filled {
                        D2D1_FIGURE_BEGIN_FILLED
                    } else {
                        D2D1_FIGURE_BEGIN_HOLLOW
                    },
                );
                for &(x, y) in &pts[1..] {
                    sink.AddLine(pt2(x, y));
                }
                sink.EndFigure(if closed {
                    D2D1_FIGURE_END_CLOSED
                } else {
                    D2D1_FIGURE_END_OPEN
                });
            };

            match kind {
                IconGeomKind::RadioArcs => {
                    for r in [0.46f32, 0.78f32] {
                        for side in [-1.0f32, 1.0f32] {
                            let mut pts = [(0.0f32, 0.0f32); 9];
                            for (k, slot) in pts.iter_mut().enumerate() {
                                let ang = -0.68 + (k as f32 / 8.0) * 1.36;
                                let a = if side < 0.0 { PI - ang } else { ang };
                                *slot = (r * a.cos(), r * a.sin());
                            }
                            add_poly(&pts, false, false);
                        }
                    }
                }
                IconGeomKind::MicArc => {
                    let mut pts = [(0.0f32, 0.0f32); 9];
                    for (k, slot) in pts.iter_mut().enumerate() {
                        let a = (k as f32 / 8.0) * PI;
                        *slot = (0.50 * a.cos(), -0.10 + 0.50 * a.sin());
                    }
                    add_poly(&pts, false, false);
                }
                IconGeomKind::PersonShoulders => {
                    let mut pts = [(0.0f32, 0.0f32); 7];
                    for (k, slot) in pts.iter_mut().enumerate() {
                        let a = PI + (k as f32 / 6.0) * PI;
                        *slot = (0.42 * a.cos(), 0.54 + 0.34 * a.sin());
                    }
                    add_poly(&pts, false, false);
                }
                IconGeomKind::DownloadArrow => {
                    add_poly(&[(-0.26, 0.06), (0.0, 0.34), (0.26, 0.06)], false, false);
                }
                IconGeomKind::StarFill => {
                    let mut pts = [(0.0f32, 0.0f32); 10];
                    for (i, slot) in pts.iter_mut().enumerate() {
                        let r = if i % 2 == 0 { 0.72 } else { 0.30 };
                        let a = -PI * 0.5 + (i as f32) * (PI / 5.0);
                        *slot = (r * a.cos(), r * a.sin());
                    }
                    add_poly(&pts, true, true);
                }
                IconGeomKind::PlayFill => {
                    add_poly(&[(-0.42, -0.66), (0.66, 0.0), (-0.42, 0.66)], true, true);
                }
                IconGeomKind::PrevFill => {
                    add_poly(&[(-0.02, -0.52), (-0.76, 0.0), (-0.02, 0.52)], true, true);
                    add_poly(&[(0.72, -0.52), (-0.02, 0.0), (0.72, 0.52)], true, true);
                }
                IconGeomKind::NextFill => {
                    add_poly(&[(-0.72, -0.52), (0.02, 0.0), (-0.72, 0.52)], true, true);
                    add_poly(&[(0.02, -0.52), (0.76, 0.0), (0.02, 0.52)], true, true);
                }
                IconGeomKind::ShuffleStrokes => {
                    add_poly(
                        &[(-0.68, -0.36), (-0.18, -0.36), (0.22, 0.36), (0.66, 0.36)],
                        false,
                        false,
                    );
                    add_poly(
                        &[(-0.68, 0.36), (-0.18, 0.36), (0.22, -0.36), (0.66, -0.36)],
                        false,
                        false,
                    );
                    add_poly(&[(0.46, -0.56), (0.68, -0.36), (0.46, -0.16)], false, false);
                    add_poly(&[(0.46, 0.16), (0.68, 0.36), (0.46, 0.56)], false, false);
                }
                IconGeomKind::RepeatArrow => {
                    add_poly(&[(0.16, -0.60), (0.40, -0.40), (0.16, -0.20)], false, false);
                }
                IconGeomKind::TranslateStrokes => {
                    add_poly(&[(-0.36, -0.74), (-0.36, -0.56)], false, false);
                    add_poly(&[(-0.72, -0.50), (0.02, -0.50)], false, false);
                    add_poly(&[(-0.14, -0.48), (-0.34, -0.14), (-0.68, 0.10)], false, false);
                    add_poly(&[(-0.54, -0.38), (-0.32, -0.12), (-0.06, 0.06)], false, false);
                    add_poly(&[(0.02, 0.70), (0.36, -0.08), (0.70, 0.70)], false, false);
                    add_poly(&[(0.15, 0.42), (0.57, 0.42)], false, false);
                }
                IconGeomKind::CheckStroke => {
                    add_poly(&[(-0.52, 0.04), (-0.12, 0.46), (0.56, -0.42)], false, false);
                }
                IconGeomKind::SpeakerBodyFill => {
                    add_poly(
                        &[
                            (-0.64, -0.24),
                            (-0.32, -0.24),
                            (0.06, -0.58),
                            (0.06, 0.58),
                            (-0.32, 0.24),
                            (-0.64, 0.24),
                        ],
                        true,
                        true,
                    );
                }
                IconGeomKind::SpeakerWaveStroke => {
                    let mut pts = [(0.0f32, 0.0f32); 7];
                    for (k, slot) in pts.iter_mut().enumerate() {
                        let a = -0.65 + (k as f32 / 6.0) * 1.30;
                        *slot = (0.16 + 0.42 * a.cos(), 0.42 * a.sin());
                    }
                    add_poly(&pts, false, false);
                }
                IconGeomKind::FolderStroke => {
                    add_poly(
                        &[
                            (-0.72, -0.52),
                            (-0.18, -0.52),
                            (0.02, -0.30),
                            (0.72, -0.30),
                            (0.72, 0.56),
                            (-0.72, 0.56),
                        ],
                        false,
                        true,
                    );
                }
                IconGeomKind::RefreshArc => {
                    let mut pts = [(0.0f32, 0.0f32); 13];
                    for (k, slot) in pts.iter_mut().enumerate() {
                        let a = -PI * 0.75 + (k as f32 / 12.0) * (PI * 1.55);
                        *slot = (0.62 * a.cos(), 0.62 * a.sin());
                    }
                    add_poly(&pts, false, false);
                }
                IconGeomKind::SparklesFill => {
                    add_poly(
                        &[
                            (0.0, -0.74),
                            (0.18, -0.18),
                            (0.74, 0.0),
                            (0.18, 0.18),
                            (0.0, 0.74),
                            (-0.18, 0.18),
                            (-0.74, 0.0),
                            (-0.18, -0.18),
                        ],
                        true,
                        true,
                    );
                }
                IconGeomKind::WinRestoreBack => {
                    // Matches `back = (-0.32, -0.54, 0.86, 0.86)` and `front = (-0.54, -0.30, 0.86, 0.86)`
                    add_poly(
                        &[
                            (-0.32, -0.30),
                            (-0.32, -0.54),
                            (0.54, -0.54),
                            (0.54, 0.32),
                            (0.32, 0.32),
                        ],
                        false,
                        false,
                    );
                }
            }

            sink.Close().ok()?;
            Some(geom.into())
        }
    }

    fn get_or_create_icon_geom(&self, kind: IconGeomKind) -> Option<ID2D1PathGeometry> {
        if let Some(cached) = self.icon_geom_cache.borrow().get(&kind) {
            return Some(cached.clone());
        }
        let created = self.build_icon_geom(kind)?;
        self.icon_geom_cache
            .borrow_mut()
            .insert(kind, created.clone());
        Some(created)
    }

    fn fill_cached_icon_geom(&self, kind: IconGeomKind, cx: f32, cy: f32, s: f32, color: Color) {
        if s <= 0.1 || color.a <= 0.001 {
            return;
        }
        let Some(geom) = self.get_or_create_icon_geom(kind) else {
            return;
        };
        unsafe {
            let mut saved_xf = Matrix3x2::identity();
            self.dc.GetTransform(&mut saved_xf);
            let xf = Matrix3x2 {
                M11: s * saved_xf.M11,
                M12: s * saved_xf.M12,
                M21: s * saved_xf.M21,
                M22: s * saved_xf.M22,
                M31: cx * saved_xf.M11 + cy * saved_xf.M21 + saved_xf.M31,
                M32: cx * saved_xf.M12 + cy * saved_xf.M22 + saved_xf.M32,
            };
            self.dc.SetTransform(&xf);
            self.dc.FillGeometry(&geom, self.brush(color), None);
            self.dc.SetTransform(&saved_xf);
        }
    }

    fn stroke_cached_icon_geom(
        &self,
        kind: IconGeomKind,
        cx: f32,
        cy: f32,
        s: f32,
        color: Color,
        width: f32,
    ) {
        if s <= 0.1 || color.a <= 0.001 {
            return;
        }
        let Some(geom) = self.get_or_create_icon_geom(kind) else {
            return;
        };
        let unit_w = width / s.max(0.5);
        unsafe {
            let mut saved_xf = Matrix3x2::identity();
            self.dc.GetTransform(&mut saved_xf);
            let xf = Matrix3x2 {
                M11: s * saved_xf.M11,
                M12: s * saved_xf.M12,
                M21: s * saved_xf.M21,
                M22: s * saved_xf.M22,
                M31: cx * saved_xf.M11 + cy * saved_xf.M21 + saved_xf.M31,
                M32: cx * saved_xf.M12 + cy * saved_xf.M22 + saved_xf.M32,
            };
            self.dc.SetTransform(&xf);
            self.dc
                .DrawGeometry(&geom, self.brush(color), unit_w, &self.round_stroke);
            self.dc.SetTransform(&saved_xf);
        }
    }

    fn draw_icon_geom(&self, icon: SfIcon, rect: RectF, color: Color) {
        let cx = rect.center_x();
        let cy = rect.center_y();
        let s = rect.w.min(rect.h) * 0.5;
        let sw = (s * 0.17).clamp(1.35, 2.1);

        match icon {
            SfIcon::House => {
                self.draw_line(cx - s * 0.78, cy - s * 0.02, cx, cy - s * 0.76, color, sw);
                self.draw_line(cx, cy - s * 0.76, cx + s * 0.78, cy - s * 0.02, color, sw);
                self.draw_line(cx - s * 0.56, cy - s * 0.10, cx - s * 0.56, cy + s * 0.72, color, sw);
                self.draw_line(cx - s * 0.56, cy + s * 0.72, cx + s * 0.56, cy + s * 0.72, color, sw);
                self.draw_line(cx + s * 0.56, cy + s * 0.72, cx + s * 0.56, cy - s * 0.10, color, sw);
            }
            SfIcon::Grid | SfIcon::PlaylistGrid => {
                let g = s * 0.68;
                let pad = s * 0.13;
                let cell = g - pad;
                for &(ox, oy) in &[
                    (cx - g, cy - g),
                    (cx + pad, cy - g),
                    (cx - g, cy + pad),
                    (cx + pad, cy + pad),
                ] {
                    self.stroke_rounded_rect(RectF::new(ox, oy, cell, cell), cell * 0.25, color, sw);
                }
            }
            SfIcon::Radio => {
                self.fill_circle(cx, cy, s * 0.18, color);
                self.stroke_cached_icon_geom(IconGeomKind::RadioArcs, cx, cy, s, color, sw);
            }
            SfIcon::Visualizer => {
                // Unified 5-bar symmetric Apple Music waveform glyph
                let bars = [0.38f32, 0.76, 1.0, 0.72, 0.42];
                let span = s * 1.38;
                let x0 = cx - span * 0.5;
                for (i, &h_mul) in bars.iter().enumerate() {
                    let bx = x0 + (i as f32 / 4.0) * span;
                    let hh = s * 0.78 * h_mul;
                    self.draw_line(bx, cy - hh, bx, cy + hh, color, sw * 1.15);
                }
            }
            SfIcon::Clock => {
                self.stroke_circle(cx, cy, s * 0.74, color, sw);
                self.draw_line(cx, cy - s * 0.42, cx, cy, color, sw);
                self.draw_line(cx, cy, cx + s * 0.30, cy + s * 0.18, color, sw);
            }
            SfIcon::Mic => {
                self.stroke_rounded_rect(
                    RectF::new(cx - s * 0.26, cy - s * 0.76, s * 0.52, s * 0.88),
                    s * 0.26,
                    color,
                    sw,
                );
                self.stroke_cached_icon_geom(IconGeomKind::MicArc, cx, cy, s, color, sw);
                self.draw_line(cx, cy + s * 0.40, cx, cy + s * 0.76, color, sw);
                self.draw_line(cx - s * 0.28, cy + s * 0.76, cx + s * 0.28, cy + s * 0.76, color, sw);
            }
            SfIcon::Albums => {
                self.stroke_rounded_rect(
                    RectF::new(cx - s * 0.62, cy - s * 0.44, s * 1.24, s * 1.18),
                    s * 0.20,
                    color,
                    sw,
                );
                self.draw_line(cx - s * 0.44, cy - s * 0.66, cx + s * 0.44, cy - s * 0.66, color, sw);
            }
            SfIcon::MusicNote => {
                let x1 = cx - s * 0.22;
                let x2 = cx + s * 0.44;
                self.draw_line(x1, cy + s * 0.46, x1, cy - s * 0.54, color, sw);
                self.draw_line(x2, cy + s * 0.32, x2, cy - s * 0.68, color, sw);
                self.draw_line(x1, cy - s * 0.54, x2, cy - s * 0.68, color, sw * 1.25);
                self.fill_circle(x1 - s * 0.16, cy + s * 0.48, s * 0.22, color);
                self.fill_circle(x2 - s * 0.16, cy + s * 0.34, s * 0.22, color);
            }
            SfIcon::PersonSquare => {
                self.stroke_rounded_rect(
                    RectF::new(cx - s * 0.68, cy - s * 0.68, s * 1.36, s * 1.36),
                    s * 0.26,
                    color,
                    sw,
                );
                self.fill_circle(cx, cy - s * 0.16, s * 0.24, color);
                self.stroke_cached_icon_geom(IconGeomKind::PersonShoulders, cx, cy, s, color, sw);
            }
            SfIcon::DownloadCircle => {
                self.stroke_circle(cx, cy, s * 0.74, color, sw);
                self.draw_line(cx, cy - s * 0.36, cx, cy + s * 0.32, color, sw);
                self.stroke_cached_icon_geom(IconGeomKind::DownloadArrow, cx, cy, s, color, sw);
            }
            SfIcon::Star => {
                self.fill_cached_icon_geom(IconGeomKind::StarFill, cx, cy, s, color);
            }
            SfIcon::Search => {
                let rc = s * 0.48;
                self.stroke_circle(cx - s * 0.12, cy - s * 0.12, rc, color, sw * 1.05);
                self.draw_line(
                    cx + s * 0.22,
                    cy + s * 0.22,
                    cx + s * 0.68,
                    cy + s * 0.68,
                    color,
                    sw * 1.15,
                );
            }
            SfIcon::ChevronLeft => {
                self.draw_line(cx + s * 0.24, cy - s * 0.54, cx - s * 0.28, cy, color, sw * 1.15);
                self.draw_line(cx - s * 0.28, cy, cx + s * 0.24, cy + s * 0.54, color, sw * 1.15);
            }
            SfIcon::ChevronRight => {
                self.draw_line(cx - s * 0.24, cy - s * 0.54, cx + s * 0.28, cy, color, sw * 1.15);
                self.draw_line(cx + s * 0.28, cy, cx - s * 0.24, cy + s * 0.54, color, sw * 1.15);
            }
            SfIcon::ChevronDown => {
                self.draw_line(cx - s * 0.46, cy - s * 0.18, cx, cy + s * 0.26, color, sw);
                self.draw_line(cx, cy + s * 0.26, cx + s * 0.46, cy - s * 0.18, color, sw);
            }
            SfIcon::SidebarLeft => {
                let r = RectF::new(cx - s * 0.72, cy - s * 0.56, s * 1.44, s * 1.12);
                self.stroke_rounded_rect(r, s * 0.22, color, sw);
                self.draw_line(r.x + r.w * 0.34, r.y, r.x + r.w * 0.34, r.bottom(), color, sw);
            }
            SfIcon::Play => {
                self.fill_cached_icon_geom(IconGeomKind::PlayFill, cx, cy, s, color);
            }
            SfIcon::Pause => {
                let bw = s * 0.34;
                let bh = s * 1.28;
                self.fill_rounded_rect(
                    RectF::new(cx - s * 0.48, cy - bh * 0.5, bw, bh),
                    bw * 0.32,
                    color,
                );
                self.fill_rounded_rect(
                    RectF::new(cx + s * 0.14, cy - bh * 0.5, bw, bh),
                    bw * 0.32,
                    color,
                );
            }
            SfIcon::Prev => {
                self.fill_cached_icon_geom(IconGeomKind::PrevFill, cx, cy, s, color);
            }
            SfIcon::Next => {
                self.fill_cached_icon_geom(IconGeomKind::NextFill, cx, cy, s, color);
            }
            SfIcon::Shuffle => {
                self.stroke_cached_icon_geom(IconGeomKind::ShuffleStrokes, cx, cy, s, color, sw);
            }
            SfIcon::Repeat => {
                let r = RectF::new(cx - s * 0.66, cy - s * 0.40, s * 1.32, s * 0.80);
                self.stroke_rounded_rect(r, s * 0.34, color, sw);
                self.stroke_cached_icon_geom(IconGeomKind::RepeatArrow, cx, cy, s, color, sw);
            }
            SfIcon::Translate => {
                // Crisp Apple SF Symbols-style dual `文 / A` translation glyph (cached PathGeometry!)
                let sw_t = (sw * 0.92).clamp(1.25, 1.85);
                self.stroke_cached_icon_geom(IconGeomKind::TranslateStrokes, cx, cy, s, color, sw_t);
            }
            SfIcon::LyricsBubble => {
                let r = RectF::new(cx - s * 0.72, cy - s * 0.64, s * 1.44, s * 1.12);
                self.stroke_rounded_rect(r, s * 0.28, color, sw);
                self.fill_circle(cx - s * 0.22, cy - s * 0.08, s * 0.12, color);
                self.fill_circle(cx + s * 0.22, cy - s * 0.08, s * 0.12, color);
                self.draw_line(
                    cx - s * 0.24,
                    cy + s * 0.48,
                    cx - s * 0.44,
                    cy + s * 0.74,
                    color,
                    sw,
                );
            }
            SfIcon::QueueList => {
                for (i, y_off) in [-0.44f32, 0.0, 0.44].iter().enumerate() {
                    let y = cy + s * y_off;
                    self.fill_circle(cx - s * 0.58, y, s * 0.11, color);
                    let w_end = if i == 0 { 0.66 } else { 0.58 };
                    self.draw_line(cx - s * 0.28, y, cx + s * w_end, y, color, sw);
                }
            }
            SfIcon::MoreDots => {
                for x_off in [-0.52f32, 0.0, 0.52] {
                    self.fill_circle(cx + s * x_off, cy, s * 0.15, color);
                }
            }
            SfIcon::Plus => {
                self.draw_line(cx - s * 0.56, cy, cx + s * 0.56, cy, color, sw * 1.15);
                self.draw_line(cx, cy - s * 0.56, cx, cy + s * 0.56, color, sw * 1.15);
            }
            SfIcon::Check => {
                self.stroke_cached_icon_geom(IconGeomKind::CheckStroke, cx, cy, s, color, sw * 1.2);
            }
            SfIcon::Gear => {
                self.stroke_circle(cx, cy, s * 0.34, color, sw);
                for i in 0..8 {
                    let a = (i as f32) * (PI * 0.25);
                    self.draw_line(
                        cx + s * 0.48 * a.cos(),
                        cy + s * 0.48 * a.sin(),
                        cx + s * 0.74 * a.cos(),
                        cy + s * 0.74 * a.sin(),
                        color,
                        sw * 1.15,
                    );
                }
            }
            SfIcon::Speaker => {
                self.fill_cached_icon_geom(IconGeomKind::SpeakerBodyFill, cx, cy, s, color);
                self.stroke_cached_icon_geom(IconGeomKind::SpeakerWaveStroke, cx, cy, s, color, sw);
            }
            SfIcon::Folder => {
                self.stroke_cached_icon_geom(IconGeomKind::FolderStroke, cx, cy, s, color, sw);
            }
            SfIcon::Trash => {
                self.draw_line(
                    cx - s * 0.62,
                    cy - s * 0.44,
                    cx + s * 0.62,
                    cy - s * 0.44,
                    color,
                    sw,
                );
                self.stroke_rounded_rect(
                    RectF::new(cx - s * 0.46, cy - s * 0.40, s * 0.92, s * 1.06),
                    s * 0.14,
                    color,
                    sw,
                );
            }
            SfIcon::Refresh => {
                self.stroke_cached_icon_geom(IconGeomKind::RefreshArc, cx, cy, s, color, sw);
            }
            SfIcon::Sparkles => {
                self.fill_cached_icon_geom(IconGeomKind::SparklesFill, cx, cy, s, color);
            }
            SfIcon::WinMinimize => {
                self.draw_line(cx - s * 0.54, cy, cx + s * 0.54, cy, color, 1.35);
            }
            SfIcon::WinMaximize => {
                let b = RectF::new(cx - s * 0.50, cy - s * 0.50, s * 1.00, s * 1.00);
                self.stroke_rounded_rect(b, 2.0, color, 1.30);
            }
            SfIcon::WinRestore => {
                let front = RectF::new(cx - s * 0.54, cy - s * 0.30, s * 0.86, s * 0.86);
                self.stroke_cached_icon_geom(IconGeomKind::WinRestoreBack, cx, cy, s, color, 1.25);
                self.stroke_rounded_rect(front, 1.8, color, 1.25);
            }
            SfIcon::WinClose => {
                self.draw_line(
                    cx - s * 0.50,
                    cy - s * 0.50,
                    cx + s * 0.50,
                    cy + s * 0.50,
                    color,
                    1.40,
                );
                self.draw_line(
                    cx + s * 0.50,
                    cy - s * 0.50,
                    cx - s * 0.50,
                    cy + s * 0.50,
                    color,
                    1.40,
                );
            }
        }
        let _ = D2D1_BEZIER_SEGMENT::default();
        let _ = D2D_SIZE_F::default();
    }

    // ========================================================================
    // GPU-Uploaded Dynamic Waterfall Bitmap (Spek Spectrogram)
    // ========================================================================

    #[allow(clippy::too_many_arguments)]
    pub fn update_and_draw_waterfall(
        &mut self,
        plot_rect: RectF,
        bins_db: &[f32],
        low24_bins_db: Option<&[f32]>,
        nyquist_hz: f32,
        display_max_hz: f32,
        advance_col: bool,
        dt_sec: f32,
    ) {
        let w = (plot_rect.w.round() as u32).clamp(120, 960);
        let h = 256u32;
        let nyq = nyquist_hz.max(8_000.0);
        let disp_max = display_max_hz.clamp(8_000.0, nyq);

        if self.waterfall_bmp.is_none() || self.waterfall_w != w || self.waterfall_h != h {
            self.waterfall_w = w;
            self.waterfall_h = h;
            self.waterfall_head_col = 0;
            self.waterfall_scroll_acc = 0.0;
            self.waterfall_display_max_hz = disp_max;
            self.waterfall_pixels = vec![0u8; (w * h * 4) as usize];
            self.waterfall_col_buf = vec![0u8; (h * 4) as usize];
            // Initialize dark background `#06070C`
            for px in self.waterfall_pixels.chunks_exact_mut(4) {
                px[0] = 12; // B
                px[1] = 7;  // G
                px[2] = 6;  // R
                px[3] = 255;
            }
            unsafe {
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_IGNORE,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                    colorContext: std::mem::ManuallyDrop::new(None),
                };
                self.waterfall_bmp = self
                    .dc
                    .CreateBitmap(
                        D2D_SIZE_U { width: w, height: h },
                        Some(self.waterfall_pixels.as_ptr() as *const c_void),
                        w * 4,
                        &props,
                    )
                    .ok();
            }
        } else if (self.waterfall_display_max_hz - disp_max).abs() > 120.0 {
            // Smoothly remap existing ring-buffer waterfall rows when frequency axis auto-expands (24k -> 48k -> 96k)
            let old_max = self.waterfall_display_max_hz.max(8_000.0);
            let stride = (w * 4) as usize;
            let mut remapped = vec![0u8; (w * h * 4) as usize];
            for px in remapped.chunks_exact_mut(4) {
                px[0] = 12;
                px[1] = 7;
                px[2] = 6;
                px[3] = 255;
            }
            if disp_max > old_max {
                let ratio = (old_max / disp_max).clamp(0.05, 1.0);
                for new_row in 0..(h as usize) {
                    let freq_norm_new = 1.0 - (new_row as f32) / ((h - 1) as f32);
                    if freq_norm_new <= ratio {
                        let freq_norm_old = (freq_norm_new / ratio).clamp(0.0, 1.0);
                        let old_row =
                            (((1.0 - freq_norm_old) * ((h - 1) as f32)).round() as usize)
                                .min((h - 1) as usize);
                        let src0 = old_row * stride;
                        let dst0 = new_row * stride;
                        remapped[dst0..dst0 + stride]
                            .copy_from_slice(&self.waterfall_pixels[src0..src0 + stride]);
                    }
                }
            }
            self.waterfall_pixels = remapped;
            self.waterfall_display_max_hz = disp_max;
            if let Some(ref bmp) = self.waterfall_bmp {
                unsafe {
                    let dst = D2D_RECT_U {
                        left: 0,
                        top: 0,
                        right: w,
                        bottom: h,
                    };
                    let _ = bmp.CopyFromMemory(
                        Some(&dst),
                        self.waterfall_pixels.as_ptr() as *const c_void,
                        w * 4,
                    );
                }
            }
        }

        if advance_col && !bins_db.is_empty() {
            // O(1) Ring-buffer column upload (~1 KB per column instead of 983 KB full-frame upload = 960x lighter!)
            self.waterfall_scroll_acc += dt_sec.clamp(0.001, 0.05) * 92.0;
            let shift_cols = (self.waterfall_scroll_acc.floor() as usize)
                .min(6)
                .min(w as usize - 1);
            if shift_cols >= 1 {
                self.waterfall_scroll_acc -= shift_cols as f32;
                let stride = (w * 4) as usize;
                let use_low24 = disp_max <= 24_050.0
                    && low24_bins_db.is_some_and(|l| l.len() >= 64);
                let low_cap_hz = nyq.min(24_000.0);
                if self.waterfall_col_buf.len() != (h * 4) as usize {
                    self.waterfall_col_buf.resize((h * 4) as usize, 0);
                }
                for row in 0..(h as usize) {
                    let freq_hz = (1.0 - (row as f32) / ((h - 1) as f32)) * disp_max;
                    let db = if use_low24 {
                        let low = low24_bins_db.unwrap();
                        let pos = ((freq_hz / low_cap_hz.max(1.0)) * ((low.len() - 1) as f32))
                            .clamp(0.0, (low.len() - 1) as f32);
                        let i0 = pos.floor() as usize;
                        let i1 = (i0 + 1).min(low.len() - 1);
                        let frac = pos - (i0 as f32);
                        low[i0] + (low[i1] - low[i0]) * frac
                    } else {
                        let pos = ((freq_hz / nyq) * ((bins_db.len() - 1) as f32))
                            .clamp(0.0, (bins_db.len() - 1) as f32);
                        let i0 = pos.floor() as usize;
                        let i1 = (i0 + 1).min(bins_db.len() - 1);
                        let frac = pos - (i0 as f32);
                        bins_db[i0] + (bins_db[i1] - bins_db[i0]) * frac
                    };
                    let (r, g, b) = spek_thermal_rgb(db);
                    let cp = row * 4;
                    self.waterfall_col_buf[cp] = b;
                    self.waterfall_col_buf[cp + 1] = g;
                    self.waterfall_col_buf[cp + 2] = r;
                    self.waterfall_col_buf[cp + 3] = 255;
                }
                for _ in 0..shift_cols {
                    let col = self.waterfall_head_col % w;
                    let col_off = (col * 4) as usize;
                    for row in 0..(h as usize) {
                        let cp = row * 4;
                        let p = row * stride + col_off;
                        self.waterfall_pixels[p..p + 4]
                            .copy_from_slice(&self.waterfall_col_buf[cp..cp + 4]);
                    }
                    if let Some(ref bmp) = self.waterfall_bmp {
                        unsafe {
                            let dst = D2D_RECT_U {
                                left: col,
                                top: 0,
                                right: col + 1,
                                bottom: h,
                            };
                            let _ = bmp.CopyFromMemory(
                                Some(&dst),
                                self.waterfall_col_buf.as_ptr() as *const c_void,
                                4,
                            );
                        }
                    }
                    self.waterfall_head_col = (col + 1) % w;
                }
            }
        }

        if let Some(ref bmp) = self.waterfall_bmp {
            let head = self.waterfall_head_col % w;
            unsafe {
                if head == 0 {
                    self.dc.DrawBitmap(
                        bmp,
                        Some(&to_d2d_rect(plot_rect)),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                } else {
                    // Draw ring buffer in 2 seamless slices: oldest `[head..w]` on the left, newest `[0..head]` on the right!
                    let right_cols = (w - head) as f32;
                    let split_x = plot_rect.x + plot_rect.w * (right_cols / (w as f32));
                    let dst_l = D2D_RECT_F {
                        left: plot_rect.x,
                        top: plot_rect.y,
                        right: split_x,
                        bottom: plot_rect.bottom(),
                    };
                    let src_l = D2D_RECT_F {
                        left: head as f32,
                        top: 0.0,
                        right: w as f32,
                        bottom: h as f32,
                    };
                    let dst_r = D2D_RECT_F {
                        left: split_x,
                        top: plot_rect.y,
                        right: plot_rect.right(),
                        bottom: plot_rect.bottom(),
                    };
                    let src_r = D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: head as f32,
                        bottom: h as f32,
                    };
                    self.dc.DrawBitmap(
                        bmp,
                        Some(&dst_l),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        Some(&src_l),
                        None,
                    );
                    self.dc.DrawBitmap(
                        bmp,
                        Some(&dst_r),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        Some(&src_r),
                        None,
                    );
                }
            }
        }
    }
}

/// Spek high-contrast thermal LUT (-108 dBFS -> 0 dBFS)
fn spek_thermal_rgb(db: f32) -> (u8, u8, u8) {
    const STOPS: &[(f32, f32, f32, f32)] = &[
        (0.00, 6.0, 7.0, 12.0),
        (0.16, 24.0, 14.0, 58.0),
        (0.34, 88.0, 19.0, 112.0),
        (0.54, 190.0, 34.0, 76.0),
        (0.74, 246.0, 126.0, 29.0),
        (0.90, 253.0, 224.0, 72.0),
        (1.00, 255.0, 255.0, 255.0),
    ];
    let x = ((db + 106.0) / 104.0).clamp(0.0, 1.0);
    for k in 0..(STOPS.len() - 1) {
        let s0 = STOPS[k];
        let s1 = STOPS[k + 1];
        if x >= s0.0 && x <= s1.0 {
            let u = (x - s0.0) / (s1.0 - s0.0).max(1e-5);
            return (
                (s0.1 + (s1.1 - s0.1) * u) as u8,
                (s0.2 + (s1.2 - s0.2) * u) as u8,
                (s0.3 + (s1.3 - s0.3) * u) as u8,
            );
        }
    }
    (255, 255, 255)
}

/// Extracts 4 rich, vibrant colors from the 4 quadrants of an album cover for LyricsBlossom's flowing mesh!
fn extract_quadrant_palette(bgra: &[u8], w: u32, h: u32) -> [Color; 4] {
    let default_pal = [
        Color::rgb(138, 36, 68),
        Color::rgb(42, 66, 128),
        Color::rgb(96, 38, 114),
        Color::rgb(28, 78, 96),
    ];
    if w < 4 || h < 4 || bgra.len() < (w * h * 4) as usize {
        return default_pal;
    }

    let hw = w / 2;
    let hh = h / 2;
    let quads = [(0, 0, hw, hh), (hw, 0, w, hh), (0, hh, hw, h), (hw, hh, w, h)];
    let mut out = default_pal;

    for (qi, &(x0, y0, x1, y1)) in quads.iter().enumerate() {
        let mut r_sum = 0.0f32;
        let mut g_sum = 0.0f32;
        let mut b_sum = 0.0f32;
        let mut w_sum = 0.0f32;

        let step_x = ((x1 - x0) / 8).max(1);
        let step_y = ((y1 - y0) / 8).max(1);
        let mut y = y0;
        while y < y1 {
            let mut x = x0;
            while x < x1 {
                let idx = ((y * w + x) * 4) as usize;
                let b = (bgra[idx] as f32) / 255.0;
                let g = (bgra[idx + 1] as f32) / 255.0;
                let r = (bgra[idx + 2] as f32) / 255.0;
                let mx = r.max(g).max(b);
                let mn = r.min(g).min(b);
                let sat = if mx > 0.02 { (mx - mn) / mx } else { 0.0 };
                let weight = 0.25 + sat * 2.2 + (1.0 - (mx - 0.55).abs()) * 0.5;
                r_sum += r * weight;
                g_sum += g * weight;
                b_sum += b * weight;
                w_sum += weight;
                x += step_x;
            }
            y += step_y;
        }
        if w_sum > 0.01 {
            let mut r = r_sum / w_sum;
            let mut g = g_sum / w_sum;
            let mut b = b_sum / w_sum;
            // Boost saturation mildly and keep luminance comfortable for white lyrics legibility (4.5:1+)
            let avg = (r + g + b) / 3.0;
            r = (avg + (r - avg) * 1.32).clamp(0.06, 0.72);
            g = (avg + (g - avg) * 1.32).clamp(0.06, 0.72);
            b = (avg + (b - avg) * 1.32).clamp(0.06, 0.72);
            out[qi] = Color { r, g, b, a: 1.0 };
        }
    }
    out
}
