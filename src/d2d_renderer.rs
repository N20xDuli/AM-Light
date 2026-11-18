#![cfg(target_os = "windows")]

use crate::theme::{Color, RectF, Theme};
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;
use std::ffi::c_void;
use windows::core::{Interface, HSTRING};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT,
    D2D1_BORDER_MODE_HARD, D2D1_BORDER_MODE_SOFT, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_PLUS,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
    D2D_POINT_2U, D2D_RECT_F, D2D_RECT_U, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, D2D1CreateFactory, ID2D1Bitmap1, ID2D1BitmapBrush1, ID2D1Device,
    ID2D1DeviceContext, ID2D1Effect, ID2D1Factory, ID2D1Factory1, ID2D1Geometry, ID2D1Image,
    ID2D1LinearGradientBrush, ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_BRUSH_PROPERTIES1,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_CAP_STYLE_ROUND,
    D2D1_COMBINE_MODE_EXCLUDE, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_DRAW_TEXT_OPTIONS_NO_SNAP, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2, D2D1_GAUSSIANBLUR_OPTIMIZATION_SPEED,
    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE, D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_LINE_JOIN_ROUND, D2D1_PRIMITIVE_BLEND_COPY,
    D2D1_PRIMITIVE_BLEND_SOURCE_OVER, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_UINT32,
    D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_SINGLETHREADED, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteInlineObject, IDWriteRenderingParams,
    IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_HIT_TEST_METRICS,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIDevice1, IDXGIDevice3, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeLinear, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows_numerics::{Matrix3x2, Vector2};

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
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LyricsLineBmpKey {
    pub line_idx: usize,
    pub max_w_q: u16,
    pub show_trans: bool,
}

pub struct CachedLyricsLineBmp {
    pub bitmap: ID2D1Bitmap1,
    pub dip_w: f32,
    pub dip_h: f32,
    pub pad_dip: f32,
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
    pub alloc_w: u32,
    pub alloc_h: u32,
    pub used_dip_w: f32,
    pub used_dip_h: f32,
    pub pad_dip: f32,
}

impl Default for ActiveGlowSlot {
    fn default() -> Self {
        Self {
            bitmap: None,
            alloc_w: 0,
            alloc_h: 0,
            used_dip_w: 0.0,
            used_dip_h: 0.0,
            pad_dip: 36.0,
        }
    }
}

pub struct D2DRenderer {
    pub d2d_factory: ID2D1Factory1,
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
    lyrics_text_params: Option<IDWriteRenderingParams>,
    lyrics_subpixel_mode: bool,
    pub wic: IWICImagingFactory,
    pub solid_brush: ID2D1SolidColorBrush,
    pub round_stroke: windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle,
    pub blur_effect: Option<ID2D1Effect>,
    pub glow_blur_effect: Option<ID2D1Effect>,
    pub mesh_blur_effect: Option<ID2D1Effect>,
    cherenkov_brush: Option<windows::Win32::Graphics::Direct2D::ID2D1RadialGradientBrush>,
    window_rim_brush: Option<ID2D1LinearGradientBrush>,

    // Cached exterior corner mask geometry for custom rounded window silhouette
    corner_cutout_key: (u32, u32, u16),
    corner_cutout_geom: Option<ID2D1PathGeometry>,

    // Reusable scratch bitmap for Liquid Glass backdrop capture (max 960x120 px = ~460 KB!)
    glass_capture_bmp: Option<ID2D1Bitmap1>,
    glass_cap_w: u32,
    glass_cap_h: u32,

    // Bounded LRU offscreen premultiplied bitmaps for true GPU Gaussian depth-of-field lyrics blur
    lyrics_bmp_track: String,
    lyrics_bmp_cache: HashMap<LyricsLineBmpKey, CachedLyricsLineBmp>,
    lyrics_bmp_lru: VecDeque<LyricsLineBmpKey>,

    // Reusable 2-slot active-line glyph bloom bitmaps + 256x256 rotating album-art mesh bitmaps + playlist diff row pool
    pub active_glow_slots: [ActiveGlowSlot; 2],
    pub row_fx_slots: Vec<RowFxSlot>,
    lyrics_mesh_bmp: Option<ID2D1Bitmap1>,
    lyrics_mesh_out_bmp: Option<ID2D1Bitmap1>,

    // Reusable low-res 128x128 bitmap for Spek waterfall & dynamic lyrics mesh background
    pub waterfall_bmp: Option<ID2D1Bitmap1>,
    pub waterfall_pixels: Vec<u8>,
    pub waterfall_w: u32,
    pub waterfall_h: u32,
    pub waterfall_scroll_acc: f32,

    font_cache: HashMap<FontKey, IDWriteTextFormat>,
    ellipsis_cache: HashMap<FontKey, IDWriteInlineObject>,
    text_layout_cache: HashMap<u64, IDWriteTextLayout>,
    text_layout_fifo: VecDeque<u64>,
    lyrics_unit_layout_cache: HashMap<u64, (IDWriteTextLayout, f32, f32, f32)>,
    lyrics_unit_layout_fifo: VecDeque<u64>,

    thumb_cache: HashMap<String, CachedArtwork>,
    thumb_lru: VecDeque<String>,
    hero_cache: HashMap<String, CachedArtwork>,
    hero_lru: VecDeque<String>,
    seen_art_stems: std::collections::HashSet<String>,
    last_art_fade_at: Option<std::time::Instant>,

    pub width_px: u32,
    pub height_px: u32,
    pub dpi_scale: f32,
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
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED;

            let mut d3d_device: Option<ID3D11Device> = None;
            let hr_hw = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                flags,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                None,
            );
            if hr_hw.is_err() || d3d_device.is_none() {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    windows::Win32::Foundation::HMODULE::default(),
                    flags,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut d3d_device),
                    None,
                    None,
                )
                .map_err(|e| format!("D3D11CreateDevice 失败: {e}"))?;
            }
            let d3d_device = d3d_device.ok_or_else(|| "D3D11Device 为空".to_string())?;
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
            let glow_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();
            let mesh_blur_effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok();

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
                lyrics_text_params,
                lyrics_subpixel_mode: false,
                wic,
                solid_brush,
                round_stroke,
                blur_effect,
                glow_blur_effect,
                mesh_blur_effect,
                cherenkov_brush,
                window_rim_brush,
                corner_cutout_key: (0, 0, 0),
                corner_cutout_geom: None,
                glass_capture_bmp: None,
                glass_cap_w: 0,
                glass_cap_h: 0,
                lyrics_bmp_track: String::new(),
                lyrics_bmp_cache: HashMap::new(),
                lyrics_bmp_lru: VecDeque::new(),
                active_glow_slots: [ActiveGlowSlot::default(), ActiveGlowSlot::default()],
                row_fx_slots: Vec::with_capacity(24),
                lyrics_mesh_bmp: None,
                lyrics_mesh_out_bmp: None,
                waterfall_bmp: None,
                waterfall_pixels: Vec::new(),
                waterfall_w: 0,
                waterfall_h: 0,
                waterfall_scroll_acc: 0.0,
                font_cache: HashMap::new(),
                ellipsis_cache: HashMap::new(),
                text_layout_cache: HashMap::with_capacity(512),
                text_layout_fifo: VecDeque::with_capacity(512),
                lyrics_unit_layout_cache: HashMap::with_capacity(256),
                lyrics_unit_layout_fifo: VecDeque::with_capacity(256),
                thumb_cache: HashMap::new(),
                thumb_lru: VecDeque::new(),
                hero_cache: HashMap::new(),
                hero_lru: VecDeque::new(),
                seen_art_stems: std::collections::HashSet::new(),
                last_art_fade_at: None,
                width_px: w,
                height_px: h,
                dpi_scale: dpi_scale.max(1.0),
            };
            renderer.recreate_backbuffer_target()?;
            Ok(renderer)
        }
    }

    fn recreate_backbuffer_target(&mut self) -> Result<(), String> {
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
        if (new_scale - self.dpi_scale).abs() > 0.01 {
            self.clear_lyrics_line_cache();
            self.text_layout_cache.clear();
            self.text_layout_fifo.clear();
            self.corner_cutout_geom = None;
        }
        self.dpi_scale = new_scale;
        if w == self.width_px && h == self.height_px {
            unsafe {
                let dpi = 96.0 * self.dpi_scale;
                self.dc.SetDpi(dpi, dpi);
            }
            return;
        }
        self.width_px = w;
        self.height_px = h;
        self.corner_cutout_geom = None;
        unsafe {
            self.dc.SetTarget(None);
            self.target_bitmap = None;
            for eff in [&self.blur_effect, &self.glow_blur_effect, &self.mesh_blur_effect] {
                if let Some(ref blur) = eff {
                    blur.SetInput(0, None::<&ID2D1Image>, true);
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
        unsafe {
            self.dc.BeginDraw();
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(Some(&to_d2d_color(clear_color)));
        }
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

            let hr_end = self.dc.EndDraw(None, None);
            let clamped_sync = sync_interval.min(4);
            let hr_pres = self.swap_chain.Present(clamped_sync, DXGI_PRESENT(0));
            if self.needs_dcomp_commit {
                self.needs_dcomp_commit = false;
                if let Some(ref dcomp) = self.dcomp_device {
                    let _ = dcomp.Commit();
                }
            }
            if hr_end.is_err() || hr_pres.is_err() {
                let _ = self.recreate_backbuffer_target();
            }
        }
    }

    pub fn set_transform(&self, m: &Matrix3x2) {
        unsafe {
            self.dc.SetTransform(m);
        }
    }

    pub fn reset_transform(&self) {
        unsafe {
            self.dc.SetTransform(&Matrix3x2::identity());
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
        unsafe {
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
            let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.dc,
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: pt2(x0, y0),
                endPoint: pt2(x1, y1),
            };
            let Ok(grad) = self.dc.CreateLinearGradientBrush(&props, None, &coll) else {
                return;
            };
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
        unsafe {
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
            let Ok(rad) = self.dc.CreateRadialGradientBrush(&props, None, &coll) else {
                return;
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

    /// iOS 26 / macOS Tahoe Liquid Glass Interactive Button (`liquid-glass-design/SKILL.md`):
    /// - Multi-stop translucent body (never flat opaque unless `reduce_transparency` is enabled)
    /// - Top meniscus specular highlight
    /// - Pointer-tracking radial caustic refraction highlight (`mouse_pos`)
    /// - 3-stop diagonal specular refractive rim (`top-left bright -> equator subtle -> bottom-right internal reflection`)
    /// - Tactile press compression & hover lift
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
        if rect.w <= 1.0 || rect.h <= 1.0 {
            return;
        }
        let r = if pressed && hovered {
            rect.inset(0.8, 0.7)
        } else if hovered {
            rect.inset(-0.4, -0.35)
        } else {
            rect
        };
        let rad = radius.min(r.w * 0.5).min(r.h * 0.5).max(0.0);

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

        // 1. Soft ambient elevation shadow underneath glass button
        let sh_alpha = if pressed && hovered {
            0.14
        } else if hovered {
            0.34
        } else {
            0.20
        };
        self.fill_rounded_rect(
            RectF::new(r.x + 0.5, r.y + if hovered { 2.2 } else { 1.4 }, r.w - 1.0, r.h),
            rad,
            Color::rgba(0, 0, 0, sh_alpha),
        );

        // 2. Crystal-Clear Liquid Glass 3-Stop Body (clear optical waist avoids muddy gray opacity!)
        let (c_top, c_waist, c_bot, rim_tl, rim_mid1, rim_mid2, rim_br, sheen_a) = match variant {
            GlassBtnVariant::ProminentLight => {
                let boost: f32 = if hovered { 0.04 } else { 0.0 };
                (
                    Color::rgba(255, 255, 255, (0.95 + boost).min(0.99)),
                    Color::rgba(244, 246, 255, (0.86 + boost).min(0.97)),
                    Color::rgba(232, 238, 252, (0.90 + boost).min(0.98)),
                    Color::rgba(255, 255, 255, 0.98),
                    Color::rgba(215, 232, 255, 0.55),
                    Color::rgba(255, 225, 242, 0.45),
                    Color::rgba(255, 255, 255, 0.80),
                    0.44,
                )
            }
            GlassBtnVariant::Tinted(tint) => {
                let h_add: f32 = if hovered { 0.14 } else { 0.0 };
                let top_c = tint
                    .lerp(Color::rgb(255, 255, 255), if hovered { 0.34 } else { 0.22 })
                    .with_alpha((0.72 + h_add).min(0.92));
                let waist_c = tint
                    .lerp(Color::rgb(255, 255, 255), 0.06)
                    .with_alpha((0.50 + h_add).min(0.82));
                let bot_c = tint
                    .lerp(Color::rgb(225, 238, 255), 0.16)
                    .with_alpha((0.64 + h_add).min(0.88));
                (
                    top_c,
                    waist_c,
                    bot_c,
                    Color::rgba(255, 255, 255, if hovered { 0.88 } else { 0.62 }),
                    tint.lerp(Color::rgb(210, 235, 255), 0.55)
                        .with_alpha(if hovered { 0.36 } else { 0.20 }),
                    tint.lerp(Color::rgb(255, 220, 240), 0.45)
                        .with_alpha(if hovered { 0.32 } else { 0.18 }),
                    Color::rgba(255, 255, 255, if hovered { 0.54 } else { 0.34 }),
                    if hovered { 0.34 } else { 0.24 },
                )
            }
            GlassBtnVariant::Regular => (
                Color::rgba(255, 255, 255, if hovered { 0.17 } else { 0.095 }),
                Color::rgba(220, 232, 255, if hovered { 0.068 } else { 0.028 }),
                Color::rgba(215, 234, 255, if hovered { 0.135 } else { 0.068 }),
                Color::rgba(255, 255, 255, if hovered { 0.84 } else { 0.50 }),
                Color::rgba(190, 226, 255, if hovered { 0.28 } else { 0.11 }),
                Color::rgba(255, 210, 236, if hovered { 0.22 } else { 0.08 }),
                Color::rgba(240, 248, 255, if hovered { 0.52 } else { 0.28 }),
                if hovered { 0.28 } else { 0.16 },
            ),
            GlassBtnVariant::Subtle => (
                Color::rgba(255, 255, 255, if hovered { 0.14 } else { 0.062 }),
                Color::rgba(220, 232, 255, if hovered { 0.052 } else { 0.018 }),
                Color::rgba(215, 234, 255, if hovered { 0.105 } else { 0.045 }),
                Color::rgba(255, 255, 255, if hovered { 0.72 } else { 0.34 }),
                Color::rgba(190, 226, 255, if hovered { 0.22 } else { 0.07 }),
                Color::rgba(255, 210, 236, if hovered { 0.18 } else { 0.05 }),
                Color::rgba(240, 248, 255, if hovered { 0.42 } else { 0.18 }),
                if hovered { 0.22 } else { 0.11 },
            ),
        };

        unsafe {
            let body_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: to_d2d_color(c_top),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.54,
                    color: to_d2d_color(c_waist),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: to_d2d_color(c_bot),
                },
            ];
            if let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.dc,
                &body_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(r.x, r.y),
                    endPoint: pt2(r.x, r.bottom()),
                };
                if let Ok(grad) = self.dc.CreateLinearGradientBrush(&props, None, &coll) {
                    let rr = D2D1_ROUNDED_RECT {
                        rect: to_d2d_rect(r),
                        radiusX: rad,
                        radiusY: rad,
                    };
                    self.dc.FillRoundedRectangle(&rr, &grad);
                }
            }
        }

        // Curved Top Meniscus Lip Sheen (liquid surface tension crest along upper 36%)
        let sheen_h = (r.h * 0.36).max(2.0);
        let sheen_r = RectF::new(r.x + 1.1, r.y + 0.7, (r.w - 2.2).max(1.0), sheen_h);
        self.fill_linear_gradient_rect(
            sheen_r,
            (rad - 0.8).max(0.0),
            sheen_r.x,
            sheen_r.y,
            sheen_r.x,
            sheen_r.bottom(),
            Color::rgba(255, 255, 255, sheen_a),
            Color::rgba(255, 255, 255, 0.0),
        );

        // Bottom Internal Caustic Reflection (upward light bounce along lower 24%)
        let bot_h = (r.h * 0.24).max(1.8);
        let bot_r = RectF::new(
            r.x + 1.5,
            r.bottom() - bot_h - 0.6,
            (r.w - 3.0).max(1.0),
            bot_h,
        );
        self.fill_linear_gradient_rect(
            bot_r,
            (rad - 1.0).max(0.0),
            bot_r.x,
            bot_r.bottom(),
            bot_r.x,
            bot_r.y,
            Color::rgba(220, 240, 255, sheen_a * 0.42),
            Color::rgba(220, 240, 255, 0.0),
        );

        // 3. Wide Pointer-Tracking Liquid Caustic Glow (bends light smoothly as cursor approaches!)
        let near_box = r.inset(-44.0, -32.0);
        if near_box.contains(mouse_pos.0, mouse_pos.1) {
            unsafe {
                let caustic_a = if hovered { 0.24 } else { 0.095 };
                let stops = [
                    D2D1_GRADIENT_STOP {
                        position: 0.0,
                        color: to_d2d_color(Color::rgba(255, 255, 255, caustic_a)),
                    },
                    D2D1_GRADIENT_STOP {
                        position: 0.45,
                        color: to_d2d_color(Color::rgba(205, 232, 255, caustic_a * 0.42)),
                    },
                    D2D1_GRADIENT_STOP {
                        position: 1.0,
                        color: to_d2d_color(Color::rgba(255, 255, 255, 0.0)),
                    },
                ];
                if let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                    &self.dc,
                    &stops,
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                ) {
                    let rad_glow = r.w.max(r.h) * 1.15 + 16.0;
                    let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                        center: pt2(mouse_pos.0, mouse_pos.1),
                        gradientOriginOffset: pt2(0.0, 0.0),
                        radiusX: rad_glow,
                        radiusY: rad_glow,
                    };
                    if let Ok(rbrush) = self.dc.CreateRadialGradientBrush(&props, None, &coll) {
                        let rr = D2D1_ROUNDED_RECT {
                            rect: to_d2d_rect(r),
                            radiusX: rad,
                            radiusY: rad,
                        };
                        self.dc.FillRoundedRectangle(&rr, &rbrush);
                    }
                }
            }
        }

        // 4. 4-Stop Iridescent Diagonal Refractive Rim Stroke (signature Liquid Glass crystal edge!)
        unsafe {
            let rim_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: to_d2d_color(rim_tl),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.38,
                    color: to_d2d_color(rim_mid1),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.68,
                    color: to_d2d_color(rim_mid2),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: to_d2d_color(rim_br),
                },
            ];
            if let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.dc,
                &rim_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(r.x, r.y),
                    endPoint: pt2(r.right(), r.bottom()),
                };
                if let Ok(grad) = self.dc.CreateLinearGradientBrush(&props, None, &coll) {
                    let rr = D2D1_ROUNDED_RECT {
                        rect: to_d2d_rect(r),
                        radiusX: rad,
                        radiusY: rad,
                    };
                    let stroke_w = if hovered { 1.22 } else { 1.05 };
                    self.dc
                        .DrawRoundedRectangle(&rr, &grad, stroke_w, &self.round_stroke);
                }
            }
        }
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

    /// Real Liquid Glass (`liquid-glass.md`): captures live scrolled pixels underneath `rect`,
    /// blurs with GPU `CLSID_D2D1GaussianBlur`, clips to `radius`, overlays `tint`, and draws specular rim!
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
        if reduce_transparency || self.blur_effect.is_none() || self.target_bitmap.is_none() {
            self.fill_rounded_rect(rect, radius, Color::rgba(32, 32, 38, 0.96));
            self.stroke_rounded_rect(rect, radius, Color::rgba(255, 255, 255, 0.10), 1.0);
            return;
        }

        // Compute physical pixel coordinates clamped to backbuffer
        let px_l = ((rect.x * self.dpi_scale).floor() as i32).clamp(0, self.width_px as i32 - 1) as u32;
        let px_t = ((rect.y * self.dpi_scale).floor() as i32).clamp(0, self.height_px as i32 - 1) as u32;
        let px_r = (((rect.x + rect.w) * self.dpi_scale).ceil() as i32)
            .clamp(px_l as i32 + 1, self.width_px as i32) as u32;
        let px_b = (((rect.y + rect.h) * self.dpi_scale).ceil() as i32)
            .clamp(px_t as i32 + 1, self.height_px as i32) as u32;
        let cap_w = px_r - px_l;
        let cap_h = px_b - px_t;

        unsafe {
            // Ensure reusable scratch bitmap is large enough and matches target_bitmap's alphaMode!
            if self.glass_capture_bmp.is_none() || cap_w > self.glass_cap_w || cap_h > self.glass_cap_h
            {
                let alloc_w = cap_w.next_power_of_two().max(128).min(2048);
                let alloc_h = cap_h.next_power_of_two().max(64).min(512);
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
                }
            }

            let mut blurred_drawn = false;
            if let (Some(ref cap_bmp), Some(ref blur), Some(ref target_bmp)) =
                (&self.glass_capture_bmp, &self.blur_effect, &self.target_bitmap)
            {
                if cap_w <= self.glass_cap_w && cap_h <= self.glass_cap_h {
                    // Flush queued content draws before reading backbuffer subregion
                    let _ = self.dc.Flush(None, None);
                    let src_u = D2D_RECT_U {
                        left: px_l,
                        top: px_t,
                        right: px_r,
                        bottom: px_b,
                    };
                    let dst_pt = D2D_POINT_2U { x: 0, y: 0 };
                    if cap_bmp
                        .CopyFromBitmap(Some(&dst_pt), target_bmp, Some(&src_u))
                        .is_ok()
                    {
                        blur.SetInput(0, cap_bmp, true);
                        let _ = blur.SetValue(
                            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                            D2D1_PROPERTY_TYPE_FLOAT,
                            &sigma.to_le_bytes(),
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

                        let rr = D2D1_ROUNDED_RECT {
                            rect: to_d2d_rect(rect),
                            radiusX: radius,
                            radiusY: radius,
                        };
                        if let (Ok(geom), Ok(out_img)) = (
                            self.d2d_factory.CreateRoundedRectangleGeometry(&rr),
                            blur.GetOutput(),
                        ) {
                            let layer_params = D2D1_LAYER_PARAMETERS1 {
                                contentBounds: to_d2d_rect(rect),
                                geometricMask: std::mem::ManuallyDrop::new(Some(geom.cast().unwrap())),
                                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                                maskTransform: Matrix3x2::identity(),
                                opacity: 1.0,
                                opacityBrush: std::mem::ManuallyDrop::new(None),
                                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                            };
                            self.dc.PushLayer(&layer_params, None);
                            // Convex lens refraction: sample slightly inset region (3.2% magnification)
                            // so underlying colors visibly bend through the curved glass capsule!
                            let dip_w = (cap_w as f32) / self.dpi_scale;
                            let dip_h = (cap_h as f32) / self.dpi_scale;
                            let zx = dip_w * 0.028;
                            let zy = dip_h * 0.065;
                            let src_lens = D2D_RECT_F {
                                left: zx,
                                top: zy,
                                right: (dip_w - zx).max(zx + 1.0),
                                bottom: (dip_h - zy).max(zy + 1.0),
                            };
                            let dst_lens = D2D_RECT_F {
                                left: (px_l as f32) / self.dpi_scale,
                                top: (px_t as f32) / self.dpi_scale,
                                right: (px_l as f32) / self.dpi_scale + dip_w,
                                bottom: (px_t as f32) / self.dpi_scale + dip_h,
                            };
                            let target_off = Vector2 {
                                X: dst_lens.left - src_lens.left,
                                Y: dst_lens.top - src_lens.top,
                            };
                            let prev_xf = Matrix3x2 {
                                M11: dip_w / (src_lens.right - src_lens.left).max(1.0),
                                M12: 0.0,
                                M21: 0.0,
                                M22: dip_h / (src_lens.bottom - src_lens.top).max(1.0),
                                M31: dst_lens.left * (1.0 - dip_w / (src_lens.right - src_lens.left).max(1.0)),
                                M32: dst_lens.top * (1.0 - dip_h / (src_lens.bottom - src_lens.top).max(1.0)),
                            };
                            self.dc.SetTransform(&prev_xf);
                            self.dc.DrawImage(
                                &out_img,
                                Some(&target_off),
                                Some(&src_lens),
                                D2D1_INTERPOLATION_MODE_LINEAR,
                                D2D1_COMPOSITE_MODE_SOURCE_OVER,
                            );
                            self.dc.SetTransform(&Matrix3x2::identity());
                            self.dc.PopLayer();
                            let _ = std::mem::ManuallyDrop::into_inner(layer_params.geometricMask);
                            blurred_drawn = true;
                        }
                    }
                }
            }

            if blurred_drawn {
                // High-transmittance crystal glaze: subtle cool top sheen -> ultra-clear waist -> bottom internal reflection
                self.fill_rounded_rect(rect, radius, tint);
                let top_sheen = RectF::new(
                    rect.x + 1.5,
                    rect.y + 0.8,
                    (rect.w - 3.0).max(1.0),
                    (rect.h * 0.38).max(2.0),
                );
                self.fill_linear_gradient_rect(
                    top_sheen,
                    (radius - 1.0).max(0.0),
                    top_sheen.x,
                    top_sheen.y,
                    top_sheen.x,
                    top_sheen.bottom(),
                    Color::rgba(255, 255, 255, 0.16),
                    Color::rgba(255, 255, 255, 0.0),
                );
                let bot_caustic = RectF::new(
                    rect.x + 2.0,
                    rect.bottom() - (rect.h * 0.25).max(2.0) - 0.6,
                    (rect.w - 4.0).max(1.0),
                    (rect.h * 0.25).max(2.0),
                );
                self.fill_linear_gradient_rect(
                    bot_caustic,
                    (radius - 1.2).max(0.0),
                    bot_caustic.x,
                    bot_caustic.bottom(),
                    bot_caustic.x,
                    bot_caustic.y,
                    Color::rgba(205, 232, 255, 0.085),
                    Color::rgba(205, 232, 255, 0.0),
                );
            } else {
                self.fill_rounded_rect(rect, radius, Theme::GLASS_OPAQUE_FALLBACK);
            }

            // 4-stop iridescent diagonal refractive rim
            let rim_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: to_d2d_color(Color::rgba(255, 255, 255, (rim_color.a * 1.9).min(0.68))),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.42,
                    color: to_d2d_color(Color::rgba(190, 226, 255, (rim_color.a * 0.55).min(0.24))),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.72,
                    color: to_d2d_color(Color::rgba(255, 212, 238, (rim_color.a * 0.45).min(0.20))),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: to_d2d_color(Color::rgba(245, 250, 255, (rim_color.a * 1.25).min(0.46))),
                },
            ];
            if let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.dc,
                &rim_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: pt2(rect.x, rect.y),
                    endPoint: pt2(rect.right(), rect.bottom()),
                };
                if let Ok(grad) = self.dc.CreateLinearGradientBrush(&props, None, &coll) {
                    let rr = D2D1_ROUNDED_RECT {
                        rect: to_d2d_rect(rect),
                        radiusX: radius,
                        radiusY: radius,
                    };
                    self.dc
                        .DrawRoundedRectangle(&rr, &grad, 1.12, &self.round_stroke);
                } else {
                    self.stroke_rounded_rect(rect, radius, rim_color, 1.0);
                }
            } else {
                self.stroke_rounded_rect(rect, radius, rim_color, 1.0);
            }
        }
    }

    // ========================================================================
    // Typography & Exact DirectWrite Layout / Syllable Hit-Testing
    // ========================================================================

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
            let family = if mono {
                HSTRING::from("Consolas")
            } else {
                HSTRING::from("Segoe UI Variable Display")
            };
            let locale = HSTRING::from("zh-CN");
            let fmt = self
                .dwrite
                .CreateTextFormat(
                    &family,
                    None,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    &locale,
                )
                .or_else(|_| {
                    let fb = HSTRING::from("Microsoft YaHei UI");
                    self.dwrite.CreateTextFormat(
                        &fb,
                        None,
                        DWRITE_FONT_WEIGHT(weight as i32),
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        &locale,
                    )
                })
                .ok()?;

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
        if text.is_empty() || rect.w <= 1.0 || rect.h <= 1.0 || color.a <= 0.001 {
            return;
        }
        let size_tenths = (size * 10.0).round().clamp(60.0, 1200.0) as u16;
        let w_q = (rect.w * 2.0).round().clamp(2.0, 16_000.0) as u32;
        let h_q = (rect.h * 2.0).round().clamp(2.0, 8_000.0) as u32;

        // Fast 64-bit FNV-1a key over text + typography + quantized layout box
        let mut h64: u64 = 0xcbf2_9ce4_8422_2325;
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

        if let Some(cached_layout) = self.text_layout_cache.get(&h64).cloned() {
            unsafe {
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
                let brush = self.brush(color);
                self.dc.DrawTextLayout(
                    pt2(rect.x, rect.y),
                    &layout,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
                while self.text_layout_fifo.len() >= 480 {
                    if let Some(old_k) = self.text_layout_fifo.pop_front() {
                        self.text_layout_cache.remove(&old_k);
                    }
                }
                self.text_layout_fifo.push_back(h64);
                self.text_layout_cache.insert(h64, layout);
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
        if let Some(hit) = self.lyrics_unit_layout_cache.get(&h64) {
            return Some(hit.clone());
        }
        let (unit_lay, _) =
            self.create_lyrics_text_layout(trimmed, font_size, 700, max_width, TextAlign::Left)?;
        let u_bounds = Self::hit_test_syllable_rects(&unit_lay, 0, trim_u16, 0.0, 0.0);
        let (ux, uy, uw) = u_bounds
            .first()
            .map(|b| (b.x, b.y, b.w))
            .unwrap_or((0.0, 0.0, font_size * 0.5));
        while self.lyrics_unit_layout_fifo.len() >= 256 {
            if let Some(old_k) = self.lyrics_unit_layout_fifo.pop_front() {
                self.lyrics_unit_layout_cache.remove(&old_k);
            }
        }
        let entry = (unit_lay, ux, uy, uw);
        self.lyrics_unit_layout_fifo.push_back(h64);
        self.lyrics_unit_layout_cache.insert(h64, entry.clone());
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
            unsafe {
                let stops = [
                    D2D1_GRADIENT_STOP {
                        position: 0.0,
                        color: to_d2d_color(sung_color),
                    },
                    D2D1_GRADIENT_STOP {
                        position: 1.0,
                        color: to_d2d_color(upcoming_color),
                    },
                ];
                if let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                    &self.dc,
                    &stops,
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                ) {
                    let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: start_pt,
                        endPoint: end_pt,
                    };
                    if let Ok(grad) = self.dc.CreateLinearGradientBrush(&props, None, &coll) {
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
        }
        self.pop_clip();
    }

    /// Pushes an `ID2D1Layer` with a smooth vertical alpha-gradient mask (`0 -> 1 -> 1 -> 0`)
    /// so scrolling lyrics smoothly dissolve into any animated background with zero dark band!
    pub fn push_vertical_fade_layer(&self, rect: RectF, top_fade_dip: f32, bot_fade_dip: f32) -> bool {
        if rect.w <= 2.0 || rect.h <= 8.0 {
            return false;
        }
        let t_stop = (top_fade_dip / rect.h).clamp(0.01, 0.35);
        let b_stop = (1.0 - (bot_fade_dip / rect.h)).clamp(t_stop + 0.1, 0.99);
        unsafe {
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
        self.lyrics_bmp_lru.clear();
        self.lyrics_unit_layout_cache.clear();
        self.lyrics_unit_layout_fifo.clear();
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
            slot.alloc_w = 0;
            slot.alloc_h = 0;
            slot.used_dip_w = 0.0;
            slot.used_dip_h = 0.0;
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
        if let Some(ref blur) = self.blur_effect {
            unsafe {
                blur.SetInput(0, None::<&ID2D1Image>, true);
            }
        }
        if let Some(ref blur) = self.glow_blur_effect {
            unsafe {
                blur.SetInput(0, None::<&ID2D1Image>, true);
            }
        }
        if let Some(ref blur) = self.mesh_blur_effect {
            unsafe {
                blur.SetInput(0, None::<&ID2D1Image>, true);
            }
        }
        self.gpu_staging_dirty = true;
    }

    pub fn trim_gpu_staging_if_dirty(&mut self) {
        if !self.gpu_staging_dirty {
            return;
        }
        self.gpu_staging_dirty = false;
        unsafe {
            if let Some(ref blur) = self.blur_effect {
                blur.SetInput(0, None::<&ID2D1Image>, true);
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

    /// Pre-bakes a crisp premultiplied offscreen bitmap for an inactive lyric line (main + bg + translation).
    /// MUST be called when clip/layer depth on `self.dc` is 0! Bounded to max 12 visible lines (< 4.5 MB).
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
        if self.lyrics_bmp_cache.contains_key(&key) {
            if self.lyrics_bmp_lru.back() != Some(&key) {
                self.lyrics_bmp_lru.retain(|k| k != &key);
                self.lyrics_bmp_lru.push_back(key);
            }
            return;
        }
        if self.target_bitmap.is_none() {
            return;
        }

        let pad_dip = 16.0f32;
        let dip_w = (vocal_max_w + pad_dip * 2.0).clamp(64.0, 2400.0);
        let dip_h = (total_h + pad_dip * 2.0).clamp(32.0, 640.0);
        let px_w = ((dip_w * self.dpi_scale).ceil() as u32).clamp(64, 4096);
        let px_h = ((dip_h * self.dpi_scale).ceil() as u32).clamp(32, 1024);
        let dpi = 96.0 * self.dpi_scale;

        unsafe {
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

            self.dc.SetTarget(&bmp);
            self.dc.SetDpi(dpi, dpi);
            if self.lyrics_subpixel_mode {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            }
            self.dc.SetTransform(&Matrix3x2::identity());
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
                    M11: bg_scale,
                    M12: 0.0,
                    M21: 0.0,
                    M22: bg_scale,
                    M31: bg_piv_x * (1.0 - bg_scale),
                    M32: bg_piv_y * (1.0 - bg_scale),
                };
                self.dc.SetTransform(&bg_xf);
                self.draw_text_layout_subpixel(
                    bg_lay,
                    pad_dip,
                    pad_dip + bg_dy,
                    Color::rgba(255, 255, 255, 0.72),
                );
                self.dc.SetTransform(&Matrix3x2::identity());
            }

            if let Some((tr_lay, tr_dy)) = trans_layout {
                self.draw_text_layout_subpixel(
                    tr_lay,
                    pad_dip,
                    pad_dip + tr_dy,
                    Color::rgba(255, 255, 255, 0.68),
                );
            }

            if let Some(ref main_tgt) = self.target_bitmap {
                self.dc.SetTarget(main_tgt);
                self.dc.SetDpi(dpi, dpi);
            }
            if self.lyrics_subpixel_mode {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            }
            self.dc.SetTransform(&Matrix3x2::identity());

            while self.lyrics_bmp_lru.len() >= 48 {
                if let Some(old_k) = self.lyrics_bmp_lru.pop_front() {
                    self.lyrics_bmp_cache.remove(&old_k);
                }
            }
            self.lyrics_bmp_lru.push_back(key);
            self.lyrics_bmp_cache.insert(
                key,
                CachedLyricsLineBmp {
                    bitmap: bmp,
                    dip_w,
                    dip_h,
                    pad_dip,
                },
            );
            self.gpu_staging_dirty = true;
        }
    }

    /// Draws a pre-baked lyric line through true GPU `CLSID_D2D1GaussianBlur` (`D2D1_BORDER_MODE_SOFT`)
    /// matching LyricsBlossom's optical depth-of-field blur (`lb_win_v8.png`)!
    pub fn draw_cached_lyrics_line_blurred(
        &self,
        key: LyricsLineBmpKey,
        origin_x: f32,
        screen_y: f32,
        sigma_dip: f32,
        alpha: f32,
    ) -> bool {
        let Some(entry) = self.lyrics_bmp_cache.get(&key) else {
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
            if sigma_dip <= 0.025 || self.blur_effect.is_none() {
                self.dc.DrawBitmap(
                    &entry.bitmap,
                    Some(&to_d2d_rect(dst)),
                    a,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
                return true;
            }
            let blur = self.blur_effect.as_ref().unwrap();
            blur.SetInput(0, &entry.bitmap, true);
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &sigma_dip.to_le_bytes(),
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
                let bounds = dst.inset(-16.0, -16.0);
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
                    Some(&pt2(dst.x, dst.y)),
                    None,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
                self.dc.PopLayer();
                return true;
            }
        }
        false
    }

    /// Begins offscreen glyph emission capture into `active_glow_slots[slot]` (0 or 1).
    /// MUST be called at clip/layer depth == 0! Returns `Some(pad_dip)` on success.
    pub fn begin_active_lyrics_glow_slot(
        &mut self,
        slot: usize,
        vocal_max_w: f32,
        total_h: f32,
    ) -> Option<f32> {
        if slot >= 2 || self.target_bitmap.is_none() {
            return None;
        }
        let pad_dip = 36.0f32;
        let dip_w = (vocal_max_w + pad_dip * 2.0).clamp(64.0, 2400.0);
        let dip_h = (total_h + pad_dip * 2.0).clamp(48.0, 680.0);
        // HiDPI aligned bloom target so active glow glyphs match exact 2K/Retina subpixel positions!
        let scale = self.dpi_scale.max(1.0);
        let req_px_w = ((dip_w * scale).ceil() as u32).clamp(64, 4096);
        let req_px_h = ((dip_h * scale).ceil() as u32).clamp(48, 1280);
        let dpi = 96.0 * scale;

        unsafe {
            let need_alloc = {
                let s = &self.active_glow_slots[slot];
                s.bitmap.is_none() || req_px_w > s.alloc_w || req_px_h > s.alloc_h
            };
            if need_alloc {
                let alloc_w = ((req_px_w + 255) & !255).clamp(256, 4096);
                let alloc_h = ((req_px_h + 127) & !127).clamp(128, 1280);
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
                self.active_glow_slots[slot].alloc_w = alloc_w;
                self.active_glow_slots[slot].alloc_h = alloc_h;
                self.active_glow_slots[slot].bitmap = Some(bmp);
                self.gpu_staging_dirty = true;
            }

            self.active_glow_slots[slot].used_dip_w = dip_w;
            self.active_glow_slots[slot].used_dip_h = dip_h;
            self.active_glow_slots[slot].pad_dip = pad_dip;

            let bmp_ref = self.active_glow_slots[slot].bitmap.as_ref()?;
            self.dc.SetTarget(bmp_ref);
            self.dc.SetDpi(dpi, dpi);
            if self.lyrics_subpixel_mode {
                self.dc
                    .SetTextRenderingParams(self.lyrics_text_params.as_ref());
            }
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

    pub fn end_active_lyrics_glow_slot(&self) {
        unsafe {
            if let Some(ref main_tgt) = self.target_bitmap {
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

    /// Composites a baked active-line glyph glow bitmap using TWO real GPU `CLSID_D2D1GaussianBlur` passes
    /// (`inner_sigma` glyph-stroke core glow `Specs.glowRadius = 5.0` + `outer_sigma` wide atmospheric bloom
    /// `Specs.glowRange = 0.4..30.0`) with `D2D1_COMPOSITE_MODE_PLUS` additive light blending!
    #[allow(clippy::too_many_arguments)]
    pub fn composite_active_lyrics_glow_slot(
        &self,
        slot: usize,
        origin_x: f32,
        screen_y: f32,
        inner_sigma: f32,
        inner_alpha: f32,
        outer_sigma: f32,
        outer_alpha: f32,
    ) {
        if slot >= 2 {
            return;
        }
        let s = &self.active_glow_slots[slot];
        let (Some(ref bmp), Some(ref blur)) = (&s.bitmap, &self.glow_blur_effect) else {
            return;
        };
        let dst_x = origin_x - s.pad_dip;
        let dst_y = screen_y - s.pad_dip;
        let src_rc = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: s.used_dip_w,
            bottom: s.used_dip_h,
        };
        let bounds = RectF::new(dst_x - 32.0, dst_y - 32.0, s.used_dip_w + 64.0, s.used_dip_h + 64.0);

        unsafe {
            blur.SetInput(0, bmp, true);
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

            for (sig, alp) in [(outer_sigma, outer_alpha), (inner_sigma, inner_alpha)] {
                let a = alp.clamp(0.0, 1.0);
                if a <= 0.01 || sig <= 0.2 {
                    continue;
                }
                let _ = blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &sig.to_le_bytes(),
                );
                if let Ok(out_img) = blur.GetOutput() {
                    if a < 0.995 {
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
                            D2D1_COMPOSITE_MODE_PLUS,
                        );
                        self.dc.PopLayer();
                    } else {
                        self.dc.DrawImage(
                            &out_img,
                            Some(&pt2(dst_x, dst_y)),
                            Some(&src_rc),
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            D2D1_COMPOSITE_MODE_PLUS,
                        );
                    }
                }
            }
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

    pub fn end_row_fx_bake(&self) {
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

    /// Renders LyricsBlossom / Apple Music's signature dynamic rotating album-cover mesh background
    /// into a compact `256x256` GPU surface and blurs it through `CLSID_D2D1GaussianBlur`
    /// (`free_blur = true` reproduces `MusicFreeBlur` crisp flowing art!). Must be called at layer depth 0!
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
            self.hero_cache
                .get(&stem)
                .or_else(|| self.thumb_cache.get(&stem))
                .map(|a| (a.bitmap.clone(), a.width as f32, a.height as f32))
        });
        let Some((cov_bmp, aw, ah)) = art_bmp else {
            return false;
        };

        const MESH_DIM: u32 = 256;
        unsafe {
            if self.lyrics_mesh_bmp.is_none() || self.lyrics_mesh_out_bmp.is_none() {
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
            }

            let (Some(ref src_bmp), Some(ref out_bmp), Some(ref blur)) = (
                &self.lyrics_mesh_bmp,
                &self.lyrics_mesh_out_bmp,
                &self.mesh_blur_effect,
            ) else {
                return false;
            };

            // Step A: Draw 4 counter-rotating, breathing overlapping copies of the album cover onto 256x256
            self.dc.SetTarget(src_bmp);
            self.dc.SetDpi(96.0, 96.0);
            self.dc.SetTransform(&Matrix3x2::identity());
            let base = palette[0].lerp(Color::rgb(12, 10, 18), 0.55);
            self.dc.Clear(Some(&to_d2d_color(base)));

            let breath = 1.0 + bass_energy * 0.14;
            let quads = [
                (76.0f32, 76.0f32, phase * 0.42, 232.0 * breath, 0.95f32),
                (186.0, 74.0, -phase * 0.34 + 1.2, 224.0, 0.88),
                (74.0, 188.0, -phase * 0.29 + 2.5, 236.0 * breath, 0.86),
                (184.0, 184.0, phase * 0.37 + 3.9, 220.0, 0.84),
            ];
            for (cx, cy, ang, draw_sz, alpha) in quads {
                let (sa, ca) = ang.sin_cos();
                let sx = draw_sz / aw.max(1.0);
                let sy = draw_sz / ah.max(1.0);
                let hw = aw * 0.5;
                let hh = ah * 0.5;
                // Translate(-hw, -hh) * Scale(sx, sy) * Rotate(ang) * Translate(cx, cy)
                let m11 = sx * ca;
                let m12 = sx * sa;
                let m21 = -sy * sa;
                let m22 = sy * ca;
                let m31 = cx - hw * m11 - hh * m21;
                let m32 = cy - hw * m12 - hh * m22;
                self.dc.SetTransform(&Matrix3x2 {
                    M11: m11,
                    M12: m12,
                    M21: m21,
                    M22: m22,
                    M31: m31,
                    M32: m32,
                });
                let dst = D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: aw,
                    bottom: ah,
                };
                self.dc.DrawBitmap(
                    &cov_bmp,
                    Some(&dst),
                    alpha,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
            }
            self.dc.SetTransform(&Matrix3x2::identity());

            // Step B: Run 1:1 GPU Gaussian Blur into `out_bmp` (256x256 -> <0.04ms GPU time!)
            self.dc.SetTarget(out_bmp);
            self.dc.SetDpi(96.0, 96.0);
            self.dc.Clear(Some(&to_d2d_color(base)));

            let sigma = if free_blur { 2.4f32 } else { 25.0f32 };
            blur.SetInput(0, src_bmp, true);
            let _ = blur.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &sigma.to_le_bytes(),
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
            if let Ok(blurred_img) = blur.GetOutput() {
                self.dc.DrawImage(
                    &blurred_img,
                    Some(&pt2(0.0, 0.0)),
                    Some(&D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: 256.0,
                        bottom: 256.0,
                    }),
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
            }

            // Step C: Restore main backbuffer target and draw smooth cubic-interpolated mesh across window!
            if let Some(ref main_tgt) = self.target_bitmap {
                self.dc.SetTarget(main_tgt);
                let dpi = 96.0 * self.dpi_scale;
                self.dc.SetDpi(dpi, dpi);
            }
            let crop_src = D2D_RECT_F {
                left: 10.0,
                top: 10.0,
                right: 246.0,
                bottom: 246.0,
            };
            self.dc.DrawBitmap(
                out_bmp,
                Some(&to_d2d_rect(win_rect)),
                1.0,
                D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                Some(&crop_src),
                None,
            );
        }
        true
    }

    // ========================================================================
    // Bounded Off-Thread WIC Artwork Decoder, Canonical Stem Cache & Fade-In
    // ========================================================================

    /// Thread-safe background CPU WIC decode + palette extraction (keeps UI thread 100% stutter-free!).
    pub fn decode_artwork_pixels_cpu(bytes: &[u8], max_dim: u32) -> Option<DecodedArtworkCpu> {
        if bytes.len() < 16 {
            return None;
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
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

            let scaler = wic.CreateBitmapScaler().ok()?;
            scaler
                .Initialize(&conv, dst_w, dst_h, WICBitmapInterpolationModeLinear)
                .ok()?;

            let stride = dst_w * 4;
            let mut pixels = vec![0u8; (stride * dst_h) as usize];
            scaler.CopyPixels(std::ptr::null(), stride, &mut pixels).ok()?;
            for px in pixels.chunks_exact_mut(4) {
                px[3] = 255;
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

    pub fn has_artwork(&self, key: &str, hero_res: bool) -> bool {
        let stem = crate::am_api::artwork_canonical_stem(key);
        if hero_res {
            self.hero_cache.contains_key(&stem)
        } else {
            self.thumb_cache.contains_key(&stem) || self.hero_cache.contains_key(&stem)
        }
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
                interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
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
        let cached = CachedArtwork {
            bitmap,
            brush,
            width: decoded_cpu.width,
            height: decoded_cpu.height,
            palette: decoded_cpu.palette,
            fade_in_start: is_brand_new.then_some(now),
        };

        if hero_res {
            self.hero_lru.retain(|k| k != &stem);
            while self.hero_lru.len() >= 8 {
                if let Some(old) = self.hero_lru.pop_front() {
                    self.hero_cache.remove(&old);
                }
            }
            self.hero_lru.push_back(stem.clone());
            self.hero_cache.insert(stem, cached);
        } else {
            self.thumb_lru.retain(|k| k != &stem);
            while self.thumb_lru.len() >= 200 {
                if let Some(old) = self.thumb_lru.pop_front() {
                    self.thumb_cache.remove(&old);
                }
            }
            self.thumb_lru.push_back(stem.clone());
            self.thumb_cache.insert(stem, cached);
        }
        self.gpu_staging_dirty = true;
        true
    }

    pub fn insert_artwork_bytes(&mut self, key: String, bytes: &[u8], hero_res: bool) -> bool {
        let max_dim = if hero_res { 560u32 } else { 160u32 };
        let Some(decoded) = Self::decode_artwork_pixels_cpu(bytes, max_dim) else {
            return false;
        };
        self.upload_decoded_artwork(key, decoded, hero_res)
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
            let sx = rect.w / (art.width as f32).max(1.0);
            let sy = rect.h / (art.height as f32).max(1.0);
            art.brush.SetOpacity(final_opacity);
            art.brush.SetTransform(&Matrix3x2 {
                M11: sx,
                M12: 0.0,
                M21: 0.0,
                M22: sy,
                M31: rect.x,
                M32: rect.y,
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

    /// Real-time audio-reactive vertical equalizer bars for playing track covers (4 bars)
    /// and left sidebar `可视化` icon (6 bars). Settles cleanly when silent/paused!
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
        let sw = (s * if n <= 4 { 0.28 } else { 0.19 }).clamp(1.5, 2.7);
        let span = s * if n <= 4 { 1.32 } else { 1.48 };
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
                for r_mul in [0.46f32, 0.78f32] {
                    let r = s * r_mul;
                    for side in [-1.0f32, 1.0f32] {
                        let mut pts = Vec::with_capacity(9);
                        for k in 0..=8 {
                            let ang = -0.68 + (k as f32 / 8.0) * 1.36;
                            let a = if side < 0.0 { PI - ang } else { ang };
                            pts.push((cx + r * a.cos(), cy + r * a.sin()));
                        }
                        self.draw_polyline(&pts, color, sw, false);
                    }
                }
            }
            SfIcon::Visualizer => {
                // Apple Music / Audio waveform bars glyph
                let bars = [0.35f32, 0.78, 1.0, 0.58, 0.85, 0.42];
                let span = s * 1.44;
                let x0 = cx - span * 0.5;
                for (i, &h_mul) in bars.iter().enumerate() {
                    let bx = x0 + (i as f32 / 5.0) * span;
                    let hh = s * 0.78 * h_mul;
                    self.draw_line(bx, cy - hh, bx, cy + hh, color, sw * 1.08);
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
                let mut arc = Vec::with_capacity(9);
                for k in 0..=8 {
                    let a = (k as f32 / 8.0) * PI;
                    arc.push((cx + s * 0.50 * a.cos(), cy - s * 0.10 + s * 0.50 * a.sin()));
                }
                self.draw_polyline(&arc, color, sw, false);
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
                let mut sh = Vec::with_capacity(7);
                for k in 0..=6 {
                    let a = PI + (k as f32 / 6.0) * PI;
                    sh.push((cx + s * 0.42 * a.cos(), cy + s * 0.54 + s * 0.34 * a.sin()));
                }
                self.draw_polyline(&sh, color, sw, false);
            }
            SfIcon::DownloadCircle => {
                self.stroke_circle(cx, cy, s * 0.74, color, sw);
                self.draw_line(cx, cy - s * 0.36, cx, cy + s * 0.32, color, sw);
                let arr = [
                    (cx - s * 0.26, cy + s * 0.06),
                    (cx, cy + s * 0.34),
                    (cx + s * 0.26, cy + s * 0.06),
                ];
                self.draw_polyline(&arr, color, sw, false);
            }
            SfIcon::Star => {
                let mut pts = Vec::with_capacity(10);
                for i in 0..10 {
                    let r = if i % 2 == 0 { s * 0.72 } else { s * 0.30 };
                    let a = -PI * 0.5 + (i as f32) * (PI / 5.0);
                    pts.push((cx + r * a.cos(), cy + r * a.sin()));
                }
                self.fill_polygon(&pts, color);
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
                let pts = [
                    (cx - s * 0.42, cy - s * 0.66),
                    (cx + s * 0.66, cy),
                    (cx - s * 0.42, cy + s * 0.66),
                ];
                self.fill_polygon(&pts, color);
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
                let t1 = [
                    (cx - s * 0.02, cy - s * 0.52),
                    (cx - s * 0.76, cy),
                    (cx - s * 0.02, cy + s * 0.52),
                ];
                let t2 = [
                    (cx + s * 0.72, cy - s * 0.52),
                    (cx - s * 0.02, cy),
                    (cx + s * 0.72, cy + s * 0.52),
                ];
                self.fill_polygon(&t1, color);
                self.fill_polygon(&t2, color);
            }
            SfIcon::Next => {
                let t1 = [
                    (cx - s * 0.72, cy - s * 0.52),
                    (cx + s * 0.02, cy),
                    (cx - s * 0.72, cy + s * 0.52),
                ];
                let t2 = [
                    (cx + s * 0.02, cy - s * 0.52),
                    (cx + s * 0.76, cy),
                    (cx + s * 0.02, cy + s * 0.52),
                ];
                self.fill_polygon(&t1, color);
                self.fill_polygon(&t2, color);
            }
            SfIcon::Shuffle => {
                let p1 = [
                    (cx - s * 0.68, cy - s * 0.36),
                    (cx - s * 0.18, cy - s * 0.36),
                    (cx + s * 0.22, cy + s * 0.36),
                    (cx + s * 0.66, cy + s * 0.36),
                ];
                let p2 = [
                    (cx - s * 0.68, cy + s * 0.36),
                    (cx - s * 0.18, cy + s * 0.36),
                    (cx + s * 0.22, cy - s * 0.36),
                    (cx + s * 0.66, cy - s * 0.36),
                ];
                self.draw_polyline(&p1, color, sw, false);
                self.draw_polyline(&p2, color, sw, false);
                self.draw_polyline(
                    &[
                        (cx + s * 0.46, cy - s * 0.56),
                        (cx + s * 0.68, cy - s * 0.36),
                        (cx + s * 0.46, cy - s * 0.16),
                    ],
                    color,
                    sw,
                    false,
                );
                self.draw_polyline(
                    &[
                        (cx + s * 0.46, cy + s * 0.16),
                        (cx + s * 0.68, cy + s * 0.36),
                        (cx + s * 0.46, cy + s * 0.56),
                    ],
                    color,
                    sw,
                    false,
                );
            }
            SfIcon::Repeat => {
                let r = RectF::new(cx - s * 0.66, cy - s * 0.40, s * 1.32, s * 0.80);
                self.stroke_rounded_rect(r, s * 0.34, color, sw);
                self.draw_polyline(
                    &[
                        (cx + s * 0.16, cy - s * 0.60),
                        (cx + s * 0.40, cy - s * 0.40),
                        (cx + s * 0.16, cy - s * 0.20),
                    ],
                    color,
                    sw,
                    false,
                );
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
                let pts = [
                    (cx - s * 0.52, cy + s * 0.04),
                    (cx - s * 0.12, cy + s * 0.46),
                    (cx + s * 0.56, cy - s * 0.42),
                ];
                self.draw_polyline(&pts, color, sw * 1.2, false);
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
                let sp = [
                    (cx - s * 0.64, cy - s * 0.24),
                    (cx - s * 0.32, cy - s * 0.24),
                    (cx + s * 0.06, cy - s * 0.58),
                    (cx + s * 0.06, cy + s * 0.58),
                    (cx - s * 0.32, cy + s * 0.24),
                    (cx - s * 0.64, cy + s * 0.24),
                ];
                self.fill_polygon(&sp, color);
                let mut w1 = Vec::with_capacity(7);
                for k in 0..=6 {
                    let a = -0.65 + (k as f32 / 6.0) * 1.30;
                    w1.push((cx + s * 0.16 + s * 0.42 * a.cos(), cy + s * 0.42 * a.sin()));
                }
                self.draw_polyline(&w1, color, sw, false);
            }
            SfIcon::Folder => {
                let pts = [
                    (cx - s * 0.72, cy - s * 0.52),
                    (cx - s * 0.18, cy - s * 0.52),
                    (cx + s * 0.02, cy - s * 0.30),
                    (cx + s * 0.72, cy - s * 0.30),
                    (cx + s * 0.72, cy + s * 0.56),
                    (cx - s * 0.72, cy + s * 0.56),
                ];
                self.draw_polyline(&pts, color, sw, true);
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
                let mut arc = Vec::with_capacity(13);
                for k in 0..=12 {
                    let a = -PI * 0.75 + (k as f32 / 12.0) * (PI * 1.55);
                    arc.push((cx + s * 0.62 * a.cos(), cy + s * 0.62 * a.sin()));
                }
                self.draw_polyline(&arc, color, sw, false);
            }
            SfIcon::Sparkles => {
                let star = [
                    (cx, cy - s * 0.74),
                    (cx + s * 0.18, cy - s * 0.18),
                    (cx + s * 0.74, cy),
                    (cx + s * 0.18, cy + s * 0.18),
                    (cx, cy + s * 0.74),
                    (cx - s * 0.18, cy + s * 0.18),
                    (cx - s * 0.74, cy),
                    (cx - s * 0.18, cy - s * 0.18),
                ];
                self.fill_polygon(&star, color);
            }
            SfIcon::WinMinimize => {
                self.draw_line(cx - s * 0.54, cy, cx + s * 0.54, cy, color, 1.35);
            }
            SfIcon::WinMaximize => {
                let b = RectF::new(cx - s * 0.50, cy - s * 0.50, s * 1.00, s * 1.00);
                self.stroke_rounded_rect(b, 2.0, color, 1.30);
            }
            SfIcon::WinRestore => {
                let back = RectF::new(cx - s * 0.32, cy - s * 0.54, s * 0.86, s * 0.86);
                let front = RectF::new(cx - s * 0.54, cy - s * 0.30, s * 0.86, s * 0.86);
                self.draw_polyline(
                    &[
                        (back.x, front.y),
                        (back.x, back.y),
                        (back.right(), back.y),
                        (back.right(), back.bottom()),
                        (front.right(), back.bottom()),
                    ],
                    color,
                    1.25,
                    false,
                );
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

    pub fn update_and_draw_waterfall(
        &mut self,
        plot_rect: RectF,
        bins_db: &[f32],
        advance_col: bool,
        dt_sec: f32,
    ) {
        let w = (plot_rect.w.round() as u32).clamp(120, 960);
        let h = 256u32;

        if self.waterfall_bmp.is_none() || self.waterfall_w != w || self.waterfall_h != h {
            self.waterfall_w = w;
            self.waterfall_h = h;
            self.waterfall_scroll_acc = 0.0;
            self.waterfall_pixels = vec![0u8; (w * h * 4) as usize];
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
                    .CreateBitmap(D2D_SIZE_U { width: w, height: h }, None, 0, &props)
                    .ok();
            }
        }

        if advance_col && bins_db.len() == 256 {
            // Frame-rate independent scroll speed (~92 px/sec across 60/90/120/144/165/240 Hz!)
            self.waterfall_scroll_acc += dt_sec.clamp(0.001, 0.05) * 92.0;
            let shift_cols = (self.waterfall_scroll_acc.floor() as usize).min(6).min(w as usize - 1);
            if shift_cols >= 1 {
                self.waterfall_scroll_acc -= shift_cols as f32;
                let stride = (w * 4) as usize;
                let shift_bytes = shift_cols * 4;
                for row in 0..(h as usize) {
                    let r0 = row * stride;
                    self.waterfall_pixels
                        .copy_within((r0 + shift_bytes)..(r0 + stride), r0);
                    // row = 0 is Nyquist (top = bin 255), row = 255 is 0 Hz (bottom = bin 0)
                    let bin = 255 - row;
                    let db = bins_db[bin];
                    let (r, g, b) = spek_thermal_rgb(db);
                    for c in (w as usize - shift_cols)..(w as usize) {
                        let p = r0 + c * 4;
                        self.waterfall_pixels[p] = b;
                        self.waterfall_pixels[p + 1] = g;
                        self.waterfall_pixels[p + 2] = r;
                        self.waterfall_pixels[p + 3] = 255;
                    }
                }
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
        }

        if let Some(ref bmp) = self.waterfall_bmp {
            unsafe {
                self.dc.DrawBitmap(
                    bmp,
                    Some(&to_d2d_rect(plot_rect)),
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
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
