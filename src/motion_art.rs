use std::cell::RefCell;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::{s, w, Interface, GUID, HRESULT, PCSTR};
use windows::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_ALPHA_FILL_MODE_OPAQUE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Media::MediaFoundation::{
    IMFDXGIBuffer, IMFDXGIDeviceManager, MFNominalRange_0_255, MFVideoFormat_NV12,
    MFVideoTransferMatrix_BT601, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_YUV_MATRIX,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READER_D3D_MANAGER,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Maximum simultaneously active viewport-visible motion cover slots.
pub const MAX_VIEWPORT_SLOTS: usize = 20;
/// Maximum warm-paused LRU snapshots retained when covers scroll out of the viewport.
pub const MAX_WARM_PAUSED_SLOTS: usize = 28;
/// Core center viewport slots decoded at full cadence (~24-30 fps); outer peripheral slots
/// beyond this rank step every 2nd tick (~12 fps) to slash D3D11 LockDevice contention.
pub const CORE_VIEWPORT_FULL_CADENCE_SLOTS: usize = 8;
/// Maximum synchronous `MFSourceReader` opens permitted per decode tick to avoid 120-480 ms stalls.
pub const MAX_DECODER_OPENS_PER_TICK: usize = 2;
/// High-water & low-water hysteresis bounds for the in-memory motion MP4 disk cache LRU.
pub const MOTION_CACHE_MAX_FILES: usize = 128;
pub const MOTION_CACHE_TARGET_FILES: usize = 96;
pub const MOTION_CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const MOTION_CACHE_TARGET_BYTES: u64 = 200 * 1024 * 1024;

const DEFAULT_TARGET_DIM: u32 = 384;
const HERO_TARGET_DIM: u32 = 512;
const MAX_PREFETCH_BATCH: usize = 48;
const MOTION_CACHE_MIN_VALID_BYTES: u64 = 8192;
const NEGATIVE_DISK_CACHE_TTL: Duration = Duration::from_secs(8);

/// Maps a requested physical pixel dimension to one of the four standardized motion cover
/// decode/upload buckets (`256 / 320 / 384 / 512` px) so `180-220 DIP` cards never upload
/// oversized `486x486` frames (`4x-7x` oversampling).
pub fn bucket_target_dim(raw_px: u32) -> u32 {
    if raw_px == 0 {
        DEFAULT_TARGET_DIM
    } else if raw_px <= 256 {
        256
    } else if raw_px <= 320 {
        320
    } else if raw_px <= 384 {
        384
    } else {
        512
    }
}

/// Computes aspect-preserving target dimensions for a clean-cropped video frame without ever
/// upscaling above the source resolution.
pub fn compute_scaled_dimensions(src_w: u32, src_h: u32, target_px: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || target_px == 0 || src_w.max(src_h) <= target_px {
        return (src_w, src_h);
    }
    let (mut dst_w, mut dst_h) = if src_w >= src_h {
        let h = ((src_h as u64 * target_px as u64 + (src_w as u64 / 2)) / src_w as u64).max(2)
            as u32;
        (target_px, h)
    } else {
        let w = ((src_w as u64 * target_px as u64 + (src_h as u64 / 2)) / src_h as u64).max(2)
            as u32;
        (w, target_px)
    };
    if dst_w >= 4 && (dst_w & 1) != 0 {
        dst_w -= 1;
    }
    if dst_h >= 4 && (dst_h & 1) != 0 {
        dst_h -= 1;
    }
    (dst_w.min(src_w).max(2), dst_h.min(src_h).max(2))
}

/// Shared video decode & UI cadence tick in milliseconds:
/// - `33 ms` (~30.3 Hz) for `<= 4` active covers (hero / detail / lyrics / few cards)
/// - `42 ms` (~23.8 Hz) for `>= 5` simultaneously visible Home covers
pub fn cadence_ms_for_active_count(active_count: usize) -> u64 {
    if active_count >= 5 {
        42
    } else {
        33
    }
}

/// Returns the decode tick stride (`1` = full cadence every tick, `2` = half cadence every 2nd tick)
/// for a slot at 0-based center-distance `priority_rank` out of `total_active` active slots.
pub fn decode_tick_stride_for_slot(priority_rank: usize, total_active: usize) -> u64 {
    if total_active > CORE_VIEWPORT_FULL_CADENCE_SLOTS
        && priority_rank >= CORE_VIEWPORT_FULL_CADENCE_SLOTS
    {
        2
    } else {
        1
    }
}

#[derive(Clone, Debug)]
pub struct MotionFrameSnapshot {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub frame_seq: u64,
    pub bgra: Arc<Vec<u8>>,
    pub dxgi_texture: Option<ID3D11Texture2D>,
    pub target_dim: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotionTelemetry {
    pub active_slots: usize,
    pub warm_slots: usize,
    pub prefetch_queued_during_scroll: u64,
    pub warm_cache_hits: u64,
    pub dxva_decoded_frames: u64,
    pub cpu_decoded_frames: u64,
    pub downscaled_frames: u64,
}

/// Thread-safe wrapper around `IMFDXGIDeviceManager` (which is documented by Microsoft
/// Media Foundation as thread-safe / agile across worker & render threads via `LockDevice`).
#[derive(Clone)]
struct SendDxgiDeviceManager(IMFDXGIDeviceManager);

unsafe impl Send for SendDxgiDeviceManager {}
unsafe impl Sync for SendDxgiDeviceManager {}

#[derive(Clone)]
struct DxvaDeviceBundle {
    mgr: SendDxgiDeviceManager,
    d3d_dev: ID3D11Device,
    generation: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SlotRequest {
    url: String,
    active: bool,
    target_dim: u32,
    priority_rank: usize,
}

#[derive(Clone, Debug)]
struct WarmPausedEntry {
    url: String,
    #[allow(dead_code)]
    target_dim: u32,
    last_frame: Option<MotionFrameSnapshot>,
    last_used_tick: u64,
}

#[derive(Default)]
struct WorkerSharedState {
    slots: HashMap<String, SlotRequest>,
    frames: HashMap<String, MotionFrameSnapshot>,
    warm_slots: HashMap<String, WarmPausedEntry>,
    warm_lru_tick: u64,
    prefetch_queue: Vec<(String, u32)>,
    prefetch_seen: HashSet<String>,
    current_prefetch_window: HashSet<String>,
    last_prefetch_input: Vec<(String, u32)>,
    last_visible_input: Vec<(String, u32)>,
    last_visible_enabled: bool,
    slots_version: u64,
    download_events_seq: u64,
    prefetch_queued_during_scroll: u64,
    warm_cache_hits: u64,
    failed_urls: HashMap<String, Instant>,
    dxva: Option<DxvaDeviceBundle>,
    dxva_generation: u64,
    global_enabled: bool,
    shutdown: bool,
    worker_exited: bool,
}

impl WorkerSharedState {
    fn next_warm_tick(&mut self) -> u64 {
        self.warm_lru_tick = self.warm_lru_tick.wrapping_add(1);
        self.warm_lru_tick
    }

    fn demote_to_warm(&mut self, slot_key: String, req: SlotRequest, last_frame: Option<MotionFrameSnapshot>) {
        if req.url.is_empty() {
            return;
        }
        let tick = self.next_warm_tick();
        if let Some(existing) = self.warm_slots.get_mut(&slot_key) {
            existing.url = req.url;
            existing.target_dim = req.target_dim;
            if last_frame.is_some() {
                existing.last_frame = last_frame;
            }
            existing.last_used_tick = tick;
        } else {
            self.warm_slots.insert(
                slot_key,
                WarmPausedEntry {
                    url: req.url,
                    target_dim: req.target_dim,
                    last_frame,
                    last_used_tick: tick,
                },
            );
        }
        while self.warm_slots.len() > MAX_WARM_PAUSED_SLOTS {
            let oldest = self
                .warm_slots
                .iter()
                .min_by_key(|(_, entry)| entry.last_used_tick)
                .map(|(k, _)| k.clone());
            if let Some(evict_k) = oldest {
                self.warm_slots.remove(&evict_k);
            } else {
                break;
            }
        }
    }

    fn active_slot_count(&self) -> usize {
        if !self.global_enabled {
            return 0;
        }
        self.slots
            .values()
            .filter(|r| r.active && !r.url.is_empty())
            .count()
    }
}

struct WorkerTelemetryCounters {
    global_frame_seq: AtomicU64,
    dxva_decoded_frames: AtomicU64,
    cpu_decoded_frames: AtomicU64,
    downscaled_frames: AtomicU64,
    aborted_downloads: AtomicU64,
    next_download_token: AtomicU64,
}

impl Default for WorkerTelemetryCounters {
    fn default() -> Self {
        Self {
            global_frame_seq: AtomicU64::new(0),
            dxva_decoded_frames: AtomicU64::new(0),
            cpu_decoded_frames: AtomicU64::new(0),
            downscaled_frames: AtomicU64::new(0),
            aborted_downloads: AtomicU64::new(0),
            next_download_token: AtomicU64::new(0),
        }
    }
}

type InflightDownloadMap = Arc<Mutex<HashMap<String, (u64, tokio::task::AbortHandle)>>>;

pub struct MotionArtManager {
    state: Arc<(Mutex<WorkerSharedState>, Condvar)>,
    wake_hwnd: Arc<AtomicUsize>,
    worker_started: AtomicBool,
    ready_paths: Arc<Mutex<HashMap<String, PathBuf>>>,
    negative_disk_cache: Arc<Mutex<HashMap<String, Instant>>>,
    inflight_downloads: InflightDownloadMap,
    counters: Arc<WorkerTelemetryCounters>,
}

impl Default for MotionArtManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionArtManager {
    pub fn new() -> Self {
        trigger_disk_cache_index_warmup_async();
        Self {
            state: Arc::new((
                Mutex::new(WorkerSharedState {
                    global_enabled: true,
                    ..Default::default()
                }),
                Condvar::new(),
            )),
            wake_hwnd: Arc::new(AtomicUsize::new(0)),
            worker_started: AtomicBool::new(false),
            ready_paths: Arc::new(Mutex::new(HashMap::new())),
            negative_disk_cache: Arc::new(Mutex::new(HashMap::new())),
            inflight_downloads: Arc::new(Mutex::new(HashMap::new())),
            counters: Arc::new(WorkerTelemetryCounters::default()),
        }
    }

    pub fn set_wake_hwnd(&self, hwnd: usize) {
        self.wake_hwnd.store(hwnd, Ordering::Release);
    }

    /// Attaches optional DXGI / D3D11 handles for hardware DXVA decoding (`IMFDXGIDeviceManager`
    /// + `ID3D11VideoContext::VideoProcessorBlt`). Pass `(None, None)` to force or revert to CPU
    /// decode + fast downscaling.
    pub fn set_dxgi_device_manager(
        &self,
        mgr: Option<IMFDXGIDeviceManager>,
        d3d_dev: Option<ID3D11Device>,
    ) {
        if let Ok(mut guard) = self.state.0.lock() {
            guard.dxva_generation = guard.dxva_generation.wrapping_add(1);
            let generation = guard.dxva_generation;
            // Drop any old-device DXGI textures cached in active frames or warm-paused LRU
            guard.frames.retain(|_, snap| snap.dxgi_texture.is_none());
            for warm in guard.warm_slots.values_mut() {
                if warm
                    .last_frame
                    .as_ref()
                    .map(|s| s.dxgi_texture.is_some())
                    .unwrap_or(false)
                {
                    warm.last_frame = None;
                }
            }
            if let (Some(m), Some(d)) = (mgr, d3d_dev) {
                if let Ok(mt) = d.cast::<ID3D11Multithread>() {
                    unsafe {
                        let _ = mt.SetMultithreadProtected(true);
                    }
                }
                guard.dxva = Some(DxvaDeviceBundle {
                    mgr: SendDxgiDeviceManager(m),
                    d3d_dev: d,
                    generation,
                });
            } else {
                guard.dxva = None;
            }
            self.state.1.notify_all();
        }
    }

    fn ensure_worker_started(&self) {
        if self
            .worker_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let shared = Arc::clone(&self.state);
            let wake_hwnd = Arc::clone(&self.wake_hwnd);
            let ready_paths = Arc::clone(&self.ready_paths);
            let negative_disk_cache = Arc::clone(&self.negative_disk_cache);
            let inflight_downloads = Arc::clone(&self.inflight_downloads);
            let counters = Arc::clone(&self.counters);
            let _ = std::thread::Builder::new()
                .name("am-motion-art".to_string())
                .spawn(move || {
                    motion_worker_loop(
                        shared,
                        wake_hwnd,
                        ready_paths,
                        negative_disk_cache,
                        inflight_downloads,
                        counters,
                    )
                });
        }
    }

    fn abort_all_inflight_downloads(&self) {
        let handles: Vec<tokio::task::AbortHandle> =
            if let Ok(mut map) = self.inflight_downloads.lock() {
                map.drain().map(|(_, (_, h))| h).collect()
            } else {
                Vec::new()
            };
        self.finish_abort_handles(handles);
    }

    /// Collects `AbortHandle`s for in-flight downloads whose URL is in neither active slots
    /// nor the active prefetch window. Must be called while holding `state.0`; caller aborts
    /// the returned handles after releasing `state.0`.
    fn collect_stale_download_handles(
        &self,
        guard: &WorkerSharedState,
    ) -> Vec<tokio::task::AbortHandle> {
        let Ok(mut map) = self.inflight_downloads.lock() else {
            return Vec::new();
        };
        if map.is_empty() {
            return Vec::new();
        }
        let mut stale_urls: Vec<String> = Vec::new();
        for url in map.keys() {
            let in_active = guard.global_enabled
                && guard.slots.values().any(|r| r.active && r.url == *url);
            let in_prefetch = guard.global_enabled && guard.current_prefetch_window.contains(url);
            if !in_active && !in_prefetch {
                stale_urls.push(url.clone());
            }
        }
        let mut handles = Vec::with_capacity(stale_urls.len());
        for url in stale_urls {
            if let Some((_, h)) = map.remove(&url) {
                handles.push(h);
            }
        }
        handles
    }

    fn finish_abort_handles(&self, handles: Vec<tokio::task::AbortHandle>) {
        let count = handles.len() as u64;
        for h in handles {
            h.abort();
        }
        if count > 0 {
            self.counters
                .aborted_downloads
                .fetch_add(count, Ordering::Relaxed);
        }
    }

    /// Returns the number of currently tracked in-flight HLS download tasks.
    pub fn inflight_download_count(&self) -> usize {
        self.inflight_downloads
            .lock()
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Returns the cumulative count of stale HLS downloads cancelled via `AbortHandle`.
    pub fn aborted_download_count(&self) -> u64 {
        self.counters.aborted_downloads.load(Ordering::Acquire)
    }

    /// Registers an external/test `AbortHandle` for `url` if not already tracked.
    pub fn register_inflight_abort_handle_for_url(
        &self,
        url: &str,
        handle: tokio::task::AbortHandle,
    ) -> bool {
        let clean = url.trim();
        if clean.is_empty() {
            return false;
        }
        let token = self
            .counters
            .next_download_token
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if let Ok(mut map) = self.inflight_downloads.lock() {
            if map.contains_key(clean) {
                return false;
            }
            map.insert(clean.to_string(), (token, handle));
            return true;
        }
        false
    }

    pub fn set_global_enabled(&self, enabled: bool) {
        let mut should_abort_all = false;
        if let Ok(mut guard) = self.state.0.lock() {
            if guard.global_enabled != enabled {
                guard.global_enabled = enabled;
                guard.last_visible_enabled = false;
                guard.slots_version = guard.slots_version.wrapping_add(1);
                if !enabled {
                    for req in guard.slots.values_mut() {
                        req.active = false;
                    }
                    guard.frames.clear();
                    guard.warm_slots.clear();
                    guard.prefetch_queue.clear();
                    guard.prefetch_seen.clear();
                    guard.current_prefetch_window.clear();
                    guard.last_prefetch_input.clear();
                    guard.last_visible_input.clear();
                    should_abort_all = true;
                }
                self.state.1.notify_all();
            }
        }
        if should_abort_all {
            self.abort_all_inflight_downloads();
        }
    }

    pub fn slot_key_for_url(url: &str) -> String {
        format!("u_{}", hash_url_key(url.trim()))
    }

    /// Synchronizes all viewport-visible URL-keyed motion cover slots (`u_*`) in one lock
    /// acquisition:
    /// - Every currently visible cover plays simultaneously (up to `MAX_VIEWPORT_SLOTS = 20`).
    /// - Existing visible URLs NEVER change slot key and NEVER wipe their decoded frame -> zero flicker!
    /// - Any cover that scrolled out of the viewport stops decoding immediately and is demoted
    ///   into the bounded warm-paused LRU (`MAX_WARM_PAUSED_SLOTS = 28`).
    pub fn sync_visible_urls(&self, visible_urls: &[String]) {
        let targets: Vec<(String, u32)> = visible_urls
            .iter()
            .map(|u| (u.clone(), DEFAULT_TARGET_DIM))
            .collect();
        self.sync_visible_urls_with_targets(&targets);
    }

    /// Synchronizes viewport-visible URL-keyed motion cover slots with per-card physical pixel
    /// target sizes (automatically bucketed to `256 / 320 / 384 / 512` px).
    /// Zero-allocation fast path when the visible URL/target list is unchanged since the last frame.
    pub fn sync_visible_urls_with_targets(&self, visible: &[(String, u32)]) {
        let mut need_worker = false;
        let mut stale_handles: Vec<tokio::task::AbortHandle> = Vec::new();
        if let Ok(mut guard) = self.state.0.lock() {
            let effective_active = guard.global_enabled;
            // Zero-allocation fast path when visible target list matches last deduplicated input
            let same_input = guard.last_visible_enabled == effective_active && {
                let mut input_it = visible.iter().filter_map(|(raw, target_px)| {
                    let clean = raw.trim();
                    if clean.is_empty() {
                        None
                    } else {
                        Some((clean, bucket_target_dim(*target_px)))
                    }
                });
                let mut prev_it = guard.last_visible_input.iter();
                let mut eq = true;
                let mut seen_count = 0usize;
                for (cu, cd) in input_it.by_ref() {
                    if seen_count >= MAX_VIEWPORT_SLOTS {
                        break;
                    }
                    match prev_it.next() {
                        Some((pu, pd)) if pu.as_str() == cu && *pd == cd => {
                            seen_count += 1;
                        }
                        _ => {
                            eq = false;
                            break;
                        }
                    }
                }
                eq && prev_it.next().is_none()
            };

            if same_input {
                if effective_active && !guard.last_visible_input.is_empty() {
                    need_worker = true;
                }
            } else {
                let mut desired: Vec<(String, String, u32)> =
                    Vec::with_capacity(visible.len().min(MAX_VIEWPORT_SLOTS));
                let mut desired_keys: HashSet<String> =
                    HashSet::with_capacity(visible.len().min(MAX_VIEWPORT_SLOTS));
                let mut next_visible_input: Vec<(String, u32)> =
                    Vec::with_capacity(visible.len().min(MAX_VIEWPORT_SLOTS));
                for (raw, target_px) in visible {
                    let clean = raw.trim();
                    if clean.is_empty() {
                        continue;
                    }
                    let key = Self::slot_key_for_url(clean);
                    if desired_keys.insert(key.clone()) {
                        let bucket = bucket_target_dim(*target_px);
                        desired.push((key, clean.to_string(), bucket));
                        next_visible_input.push((clean.to_string(), bucket));
                        if desired.len() >= MAX_VIEWPORT_SLOTS {
                            break;
                        }
                    }
                }

                let mut changed = false;
                let stale_keys: Vec<String> = guard
                    .slots
                    .keys()
                    .filter(|k| k.starts_with("u_") && !desired_keys.contains(*k))
                    .cloned()
                    .collect();
                for sk in stale_keys {
                    let prev_req = guard.slots.remove(&sk);
                    let prev_frame = guard.frames.remove(&sk);
                    if let Some(req) = prev_req {
                        if guard.global_enabled {
                            guard.demote_to_warm(sk, req, prev_frame);
                        }
                    }
                    changed = true;
                }

                for (idx, (k, url, target_dim)) in desired.into_iter().enumerate() {
                    // Promote from warm-paused LRU immediately if returning to viewport!
                    let mut promoted_from_warm = false;
                    if effective_active {
                        let warm_match = guard
                            .warm_slots
                            .get(&k)
                            .map(|w| w.url == url)
                            .unwrap_or(false);
                        if warm_match {
                            if let Some(warm) = guard.warm_slots.remove(&k) {
                                guard.warm_cache_hits = guard.warm_cache_hits.saturating_add(1);
                                if let Some(snap) = warm.last_frame {
                                    guard.frames.entry(k.clone()).or_insert(snap);
                                    promoted_from_warm = true;
                                }
                            }
                        }
                    }

                    let next_req = SlotRequest {
                        url,
                        active: effective_active,
                        target_dim,
                        priority_rank: idx,
                    };
                    let prev = guard.slots.get(&k);
                    if prev != Some(&next_req) {
                        if !promoted_from_warm && prev.map(|p| &p.url) != Some(&next_req.url) {
                            guard.frames.remove(&k);
                        }
                        guard.slots.insert(k, next_req);
                        changed = true;
                    }
                    if effective_active {
                        need_worker = true;
                    }
                }

                guard.last_visible_input = next_visible_input;
                guard.last_visible_enabled = effective_active;

                if changed {
                    guard.slots_version = guard.slots_version.wrapping_add(1);
                    if guard.current_prefetch_window.is_empty() {
                        stale_handles = self.collect_stale_download_handles(&guard);
                    }
                    self.state.1.notify_all();
                }
            }
        }
        self.finish_abort_handles(stale_handles);
        if need_worker {
            self.ensure_worker_started();
        }
    }

    /// Queues background HLS->MP4 downloads for upcoming/offscreen cards even while fast-scrolling.
    /// Zero-clone fast path when the prefetch list is unchanged; cancels stale in-flight HLS
    /// downloads whose URLs left both the active viewport slots and the prefetch window.
    pub fn sync_prefetch_urls(&self, prefetch: &[(String, u32)]) {
        let mut need_worker = false;
        let mut stale_handles: Vec<tokio::task::AbortHandle> = Vec::new();
        if let Ok(mut guard) = self.state.0.lock() {
            if prefetch.is_empty() {
                let had_any = !guard.current_prefetch_window.is_empty()
                    || !guard.prefetch_queue.is_empty()
                    || !guard.last_prefetch_input.is_empty()
                    || !guard.prefetch_seen.is_empty();
                if had_any {
                    guard.current_prefetch_window.clear();
                    guard.last_prefetch_input.clear();
                    guard.prefetch_queue.clear();
                    guard.prefetch_seen.clear();
                    guard.slots_version = guard.slots_version.wrapping_add(1);
                    stale_handles = self.collect_stale_download_handles(&guard);
                    self.state.1.notify_all();
                } else {
                    stale_handles = self.collect_stale_download_handles(&guard);
                    if !stale_handles.is_empty() {
                        guard.slots_version = guard.slots_version.wrapping_add(1);
                        self.state.1.notify_all();
                    }
                }
            } else if guard.global_enabled {
                // Zero-allocation fast path when prefetch window & target dims are identical
                let same_input = {
                    let mut input_it = prefetch.iter().filter_map(|(raw, target_px)| {
                        let clean = raw.trim();
                        if clean.is_empty() {
                            None
                        } else {
                            Some((clean, bucket_target_dim(*target_px)))
                        }
                    });
                    let mut prev_it = guard.last_prefetch_input.iter();
                    let mut eq = true;
                    let mut seen_count = 0usize;
                    for (cu, cd) in input_it.by_ref() {
                        if seen_count >= MAX_PREFETCH_BATCH {
                            break;
                        }
                        match prev_it.next() {
                            Some((pu, pd)) if pu.as_str() == cu && *pd == cd => {
                                seen_count += 1;
                            }
                            _ => {
                                eq = false;
                                break;
                            }
                        }
                    }
                    eq && prev_it.next().is_none()
                };

                if !same_input {
                    let mut deduped: Vec<(String, u32)> =
                        Vec::with_capacity(prefetch.len().min(MAX_PREFETCH_BATCH));
                    let mut next_window: HashSet<String> =
                        HashSet::with_capacity(prefetch.len().min(MAX_PREFETCH_BATCH));
                    for (raw, target_px) in prefetch {
                        let clean = raw.trim();
                        if clean.is_empty() {
                            continue;
                        }
                        if next_window.insert(clean.to_string()) {
                            deduped.push((clean.to_string(), bucket_target_dim(*target_px)));
                            if deduped.len() >= MAX_PREFETCH_BATCH {
                                break;
                            }
                        }
                    }

                    guard.last_prefetch_input = deduped.clone();
                    guard.current_prefetch_window = next_window;

                    // Drop stale queued prefetches no longer in active slots or prefetch window
                    let mut pruned_urls: Vec<String> = Vec::new();
                    {
                        let state = &mut *guard;
                        let slots_ref = &state.slots;
                        let win_ref = &state.current_prefetch_window;
                        state.prefetch_queue.retain(|(u, _)| {
                            let keep = win_ref.contains(u)
                                || slots_ref.values().any(|r| r.active && r.url == *u);
                            if !keep {
                                pruned_urls.push(u.clone());
                            }
                            keep
                        });
                    }
                    for u in pruned_urls {
                        guard.prefetch_seen.remove(&u);
                    }
                    {
                        let state = &mut *guard;
                        let win_ref = &state.current_prefetch_window;
                        let q_ref = &state.prefetch_queue;
                        state
                            .prefetch_seen
                            .retain(|u| win_ref.contains(u) || q_ref.iter().any(|(qu, _)| qu == u));
                    }

                    // Borrow `ready_paths` under lock with zero keyset cloning!
                    let ready_guard = self.ready_paths.lock().ok();
                    let mut newly_to_queue: Vec<(String, u32)> = Vec::new();
                    for (url, target_dim) in deduped {
                        let is_ready = ready_guard
                            .as_ref()
                            .map(|m| m.contains_key(url.as_str()))
                            .unwrap_or(false)
                            || disk_index_contains_url(&url);
                        let is_active = guard.slots.values().any(|r| r.active && r.url == url);
                        if guard.failed_urls.contains_key(&url) || is_ready || is_active {
                            continue;
                        }
                        if guard.prefetch_seen.insert(url.clone()) {
                            if guard.prefetch_seen.len() > 512 {
                                guard.prefetch_seen.clear();
                                guard.prefetch_seen.insert(url.clone());
                            }
                            newly_to_queue.push((url, target_dim));
                        }
                    }
                    drop(ready_guard);

                    stale_handles = self.collect_stale_download_handles(&guard);

                    if !newly_to_queue.is_empty() {
                        guard.prefetch_queued_during_scroll = guard
                            .prefetch_queued_during_scroll
                            .saturating_add(newly_to_queue.len() as u64);
                        guard.prefetch_queue.extend(newly_to_queue);
                        if guard.prefetch_queue.len() > MAX_PREFETCH_BATCH * 2 {
                            let drain_n = guard.prefetch_queue.len() - MAX_PREFETCH_BATCH * 2;
                            let dropped: Vec<String> = guard
                                .prefetch_queue
                                .drain(0..drain_n)
                                .map(|(u, _)| u)
                                .collect();
                            for u in dropped {
                                guard.prefetch_seen.remove(&u);
                            }
                        }
                        need_worker = true;
                        self.state.1.notify_all();
                    } else if !stale_handles.is_empty() {
                        guard.slots_version = guard.slots_version.wrapping_add(1);
                        self.state.1.notify_all();
                    }
                }
            }
        }
        self.finish_abort_handles(stale_handles);
        if need_worker {
            self.ensure_worker_started();
        }
    }

    /// Non-blocking cache readiness check for hot UI/fast-scroll frames:
    /// NEVER performs synchronous `fs::metadata` syscalls on the calling thread!
    /// Consults only the in-memory `ready_paths` table and startup-warmed `MOTION_DISK_INDEX`.
    pub fn is_url_cached_ready_nonblocking(&self, url: &str) -> bool {
        let clean = url.trim();
        if clean.is_empty() {
            return false;
        }
        match disk_index_lookup_state_for_url(clean) {
            DiskIndexLookup::Hit(path) => {
                if let Ok(mut ready) = self.ready_paths.lock() {
                    ready.entry(clean.to_string()).or_insert(path);
                }
                if let Ok(mut neg) = self.negative_disk_cache.lock() {
                    neg.remove(clean);
                }
                true
            }
            DiskIndexLookup::MissInitialized => {
                if let Ok(mut ready) = self.ready_paths.lock() {
                    ready.remove(clean);
                }
                false
            }
            DiskIndexLookup::NotYetInitialized => self
                .ready_paths
                .lock()
                .map(|ready| ready.contains_key(clean))
                .unwrap_or(false),
        }
    }

    /// Returns `true` when the backing MP4 for `url` is already cached locally on disk (or in the
    /// in-memory ready/disk index), enabling callers to bypass scroll-settle debounce immediately.
    /// Consults the zero-syscall in-memory index first; negative misses are cached for 8 seconds
    /// and invalidated immediately whenever a download completes.
    pub fn is_url_cached_ready(&self, url: &str) -> bool {
        let clean = url.trim();
        if clean.is_empty() {
            return false;
        }
        if self.is_url_cached_ready_nonblocking(clean) {
            return true;
        }
        let now = Instant::now();
        if let Ok(neg) = self.negative_disk_cache.lock() {
            if let Some(checked_at) = neg.get(clean) {
                if now.duration_since(*checked_at) < NEGATIVE_DISK_CACHE_TTL {
                    return false;
                }
            }
        }
        if let Some((path, len)) = cached_mp4_path_and_len_if_ready(clean) {
            disk_index_record_existing_file(clean, len);
            if let Ok(mut ready) = self.ready_paths.lock() {
                ready.insert(clean.to_string(), path);
            }
            if let Ok(mut neg) = self.negative_disk_cache.lock() {
                neg.remove(clean);
            }
            true
        } else {
            if let Ok(mut neg) = self.negative_disk_cache.lock() {
                if neg.len() >= 512 {
                    neg.retain(|_, t| now.duration_since(*t) < NEGATIVE_DISK_CACHE_TTL);
                }
                neg.insert(clean.to_string(), now);
            }
            false
        }
    }

    pub fn latest_frame_for_url(&self, url: &str) -> Option<MotionFrameSnapshot> {
        let clean = url.trim();
        if clean.is_empty() {
            return None;
        }
        let key = Self::slot_key_for_url(clean);
        self.latest_frame(&key, Some(clean))
    }

    pub fn active_slot_urls(&self) -> Vec<String> {
        let Ok(guard) = self.state.0.lock() else {
            return Vec::new();
        };
        if !guard.global_enabled {
            return Vec::new();
        }
        guard
            .slots
            .values()
            .filter(|r| r.active && !r.url.is_empty())
            .map(|r| r.url.clone())
            .collect()
    }

    pub fn set_slot(&self, slot_key: &str, m3u8_url: Option<&str>, active: bool) {
        self.set_slot_with_target(slot_key, m3u8_url, active, HERO_TARGET_DIM);
    }

    pub fn set_slot_with_target(
        &self,
        slot_key: &str,
        m3u8_url: Option<&str>,
        active: bool,
        target_px: u32,
    ) {
        let clean_url = m3u8_url.map(str::trim).unwrap_or("").to_string();
        if !clean_url.is_empty() && active {
            self.ensure_worker_started();
        }
        let mut stale_handles: Vec<tokio::task::AbortHandle> = Vec::new();
        if let Ok(mut guard) = self.state.0.lock() {
            if clean_url.is_empty() {
                let changed = guard.slots.remove(slot_key).is_some()
                    || guard.frames.remove(slot_key).is_some();
                if changed {
                    if slot_key.starts_with("u_") {
                        guard.last_visible_input.clear();
                        guard.last_visible_enabled = false;
                    }
                    guard.slots_version = guard.slots_version.wrapping_add(1);
                    stale_handles = self.collect_stale_download_handles(&guard);
                    self.state.1.notify_all();
                }
            } else {
                let effective_active = active && guard.global_enabled;
                let next_req = SlotRequest {
                    url: clean_url,
                    active: effective_active,
                    target_dim: bucket_target_dim(target_px),
                    priority_rank: 0,
                };
                let prev = guard.slots.get(slot_key).cloned();
                if prev.as_ref() != Some(&next_req) {
                    if prev.as_ref().map(|p| &p.url) != Some(&next_req.url) {
                        guard.frames.remove(slot_key);
                    }
                    guard.slots.insert(slot_key.to_string(), next_req);
                    if slot_key.starts_with("u_") {
                        guard.last_visible_input.clear();
                        guard.last_visible_enabled = false;
                    }
                    guard.slots_version = guard.slots_version.wrapping_add(1);
                    stale_handles = self.collect_stale_download_handles(&guard);
                    self.state.1.notify_all();
                }
            }
        }
        self.finish_abort_handles(stale_handles);
    }

    pub fn latest_frame(
        &self,
        slot_key: &str,
        expected_url: Option<&str>,
    ) -> Option<MotionFrameSnapshot> {
        let mut guard = self.state.0.lock().ok()?;
        if !guard.global_enabled {
            return None;
        }
        let clean_expected = expected_url.map(str::trim);
        if let Some(req) = guard.slots.get(slot_key) {
            if !req.active {
                return None;
            }
            if let Some(exp) = clean_expected {
                if req.url != exp {
                    return None;
                }
            }
            if let Some(frame) = guard.frames.get(slot_key) {
                return Some(frame.clone());
            }
        } else if !slot_key.starts_with("u_") {
            return None;
        }

        // Fall back to warm-paused LRU snapshot for seamless zero-flash back-scrolling
        let state = &mut *guard;
        let tick = state.next_warm_tick();
        if let Some(warm) = state.warm_slots.get_mut(slot_key) {
            if let Some(exp) = clean_expected {
                if warm.url != exp {
                    return None;
                }
            }
            if let Some(snap) = warm.last_frame.clone() {
                warm.last_used_tick = tick;
                state.warm_cache_hits = state.warm_cache_hits.saturating_add(1);
                return Some(snap);
            }
        }
        None
    }

    /// Monotonically incremented whenever any active motion slot publishes a new decoded frame.
    pub fn latest_global_frame_seq(&self) -> u64 {
        self.counters.global_frame_seq.load(Ordering::Acquire)
    }

    /// Returns `true` when motion artwork is globally enabled and at least one active slot is
    /// currently assigned to play.
    pub fn has_active_playing_slots(&self) -> bool {
        let Ok(guard) = self.state.0.lock() else {
            return false;
        };
        guard.active_slot_count() > 0
    }

    /// Returns the exact shared video decode tick cadence in milliseconds (`33` or `42`),
    /// matching the motion worker thread so UI redraws lock directly to video frame updates.
    pub fn active_cadence_ms(&self) -> u64 {
        let active_count = self
            .state
            .0
            .lock()
            .map(|g| g.active_slot_count())
            .unwrap_or(0);
        cadence_ms_for_active_count(active_count)
    }

    pub fn telemetry_snapshot(&self) -> MotionTelemetry {
        let (active_slots, warm_slots, prefetch_queued_during_scroll, warm_cache_hits) = self
            .state
            .0
            .lock()
            .map(|g| {
                (
                    g.active_slot_count(),
                    g.warm_slots.len(),
                    g.prefetch_queued_during_scroll,
                    g.warm_cache_hits,
                )
            })
            .unwrap_or((0, 0, 0, 0));
        MotionTelemetry {
            active_slots,
            warm_slots,
            prefetch_queued_during_scroll,
            warm_cache_hits,
            dxva_decoded_frames: self.counters.dxva_decoded_frames.load(Ordering::Acquire),
            cpu_decoded_frames: self.counters.cpu_decoded_frames.load(Ordering::Acquire),
            downscaled_frames: self.counters.downscaled_frames.load(Ordering::Acquire),
        }
    }
}

impl Drop for MotionArtManager {
    fn drop(&mut self) {
        self.abort_all_inflight_downloads();
        let started = self.worker_started.load(Ordering::Acquire);
        if let Ok(mut guard) = self.state.0.lock() {
            guard.shutdown = true;
            self.state.1.notify_all();
            if started {
                let deadline = Instant::now() + Duration::from_millis(350);
                while !guard.worker_exited {
                    let rem = deadline.saturating_duration_since(Instant::now());
                    if rem.is_zero() {
                        break;
                    }
                    match self.state.1.wait_timeout(guard, rem) {
                        Ok((next_g, _)) => guard = next_g,
                        Err(_) => return,
                    }
                }
            }
            guard.frames.clear();
            guard.warm_slots.clear();
            guard.dxva = None;
        }
    }
}

pub fn resolve_hls_url(base_url: &str, rel: &str) -> String {
    let clean = rel.trim().trim_matches('"').trim_matches('\'').trim();
    if clean.is_empty() {
        return String::new();
    }
    if clean.starts_with("http://") || clean.starts_with("https://") {
        return clean.to_string();
    }
    if clean.starts_with("//") {
        let scheme = base_url
            .split_once("://")
            .map(|(s, _)| s)
            .unwrap_or("https");
        return format!("{scheme}:{clean}");
    }
    if let Some((scheme, rest)) = base_url.split_once("://") {
        if clean.starts_with('/') {
            let host = rest.split('/').next().unwrap_or("");
            return format!("{scheme}://{host}{clean}");
        }
        let without_query = base_url.split('?').next().unwrap_or(base_url);
        if let Some((dir, _)) = without_query.rsplit_once('/') {
            return format!("{dir}/{clean}");
        }
    }
    clean.to_string()
}

fn extract_attr_quoted<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let idx = line.find(key)?;
    let rest = &line[idx + key.len()..];
    if let Some(after_q) = rest.strip_prefix('"') {
        let end = after_q.find('"')?;
        Some(&after_q[..end])
    } else {
        let end = rest.find(',').unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

fn parse_resolution(line: &str) -> Option<(u32, u32)> {
    let raw = extract_attr_quoted(line, "RESOLUTION=")?;
    let (w_str, h_str) = raw.split_once('x').or_else(|| raw.split_once('X'))?;
    let w = w_str.trim().parse::<u32>().ok()?;
    let h = h_str.trim().parse::<u32>().ok()?;
    Some((w, h))
}

fn parse_bandwidth(line: &str) -> u64 {
    extract_attr_quoted(line, "BANDWIDTH=")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// Parses an HLS master playlist and picks the best H.264 (`avc1`) stream variant,
/// skipping `#EXT-X-I-FRAME-STREAM-INF` trickplay tracks and preferring `480..720px`
/// (with highest `avc1` under `1080px` as fallback).
pub fn parse_best_avc_variant_from_master(m3u8: &str, base_url: &str) -> Option<String> {
    let lines: Vec<&str> = m3u8
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut best: Option<((u8, u32, u64), String)> = None;

    for i in 0..lines.len() {
        let line = lines[i];
        if line.starts_with("#EXT-X-I-FRAME-STREAM-INF") {
            continue;
        }
        if !line.starts_with("#EXT-X-STREAM-INF") {
            continue;
        }
        let codecs_raw = extract_attr_quoted(line, "CODECS=").unwrap_or("");
        let codecs_low = codecs_raw.to_ascii_lowercase();
        let is_avc = if !codecs_low.is_empty() {
            codecs_low.contains("avc1")
        } else {
            !codecs_low.contains("hvc1")
                && !codecs_low.contains("hev1")
                && !codecs_low.contains("av01")
        };
        if !is_avc {
            continue;
        }

        let Some(next_uri) = lines
            .iter()
            .skip(i + 1)
            .find(|l| !l.starts_with('#') && !l.is_empty())
        else {
            continue;
        };

        let dim = parse_resolution(line).map(|(w, h)| w.max(h)).unwrap_or(0);
        let bw = parse_bandwidth(line);
        let tier = if (450..=560).contains(&dim) {
            3u8
        } else if (561..=720).contains(&dim) || (300..450).contains(&dim) {
            2u8
        } else if (1..=1080).contains(&dim) || dim == 0 {
            1u8
        } else {
            0u8
        };
        let dim_score = if tier == 3 {
            dim
        } else if (561..=720).contains(&dim) {
            // Prefer closer to 560 within 561..720 to save H.264 decode bandwidth & RAM
            1200u32.saturating_sub(dim)
        } else if dim <= 1080 {
            dim
        } else {
            0
        };
        let score = (tier, dim_score, bw);
        let resolved = resolve_hls_url(base_url, next_uri);
        if resolved.is_empty() {
            continue;
        }
        if best.as_ref().map(|(bs, _)| score > *bs).unwrap_or(true) {
            best = Some((score, resolved));
        }
    }

    best.map(|(_, u)| u)
}

/// Picks the smallest HLS AVC (`avc1`) variant whose resolution is `>= target_px` (skipping
/// `#EXT-X-I-FRAME-STREAM-INF` trickplay tracks). When no AVC variant reaches `target_px`,
/// falls back to the largest available AVC variant.
pub fn parse_avc_variant_for_target_dim(
    master_url: &str,
    m3u8: &str,
    target_px: u32,
) -> Option<String> {
    if target_px == 0 {
        return parse_best_avc_variant_from_master(m3u8, master_url);
    }

    let lines: Vec<&str> = m3u8
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut best_ge: Option<((u32, u64), String)> = None;
    let mut best_lt: Option<((u32, u64), String)> = None;

    for i in 0..lines.len() {
        let line = lines[i];
        if line.starts_with("#EXT-X-I-FRAME-STREAM-INF") {
            continue;
        }
        if !line.starts_with("#EXT-X-STREAM-INF") {
            continue;
        }
        let codecs_raw = extract_attr_quoted(line, "CODECS=").unwrap_or("");
        let codecs_low = codecs_raw.to_ascii_lowercase();
        let is_avc = if !codecs_low.is_empty() {
            codecs_low.contains("avc1")
        } else {
            !codecs_low.contains("hvc1")
                && !codecs_low.contains("hev1")
                && !codecs_low.contains("av01")
        };
        if !is_avc {
            continue;
        }

        let Some(next_uri) = lines
            .iter()
            .skip(i + 1)
            .find(|l| !l.starts_with('#') && !l.is_empty())
        else {
            continue;
        };

        let resolved = resolve_hls_url(master_url, next_uri);
        if resolved.is_empty() {
            continue;
        }
        let dim = parse_resolution(line).map(|(w, h)| w.max(h)).unwrap_or(0);
        let bw = parse_bandwidth(line);

        if dim >= target_px {
            let key = (dim, bw);
            if best_ge.as_ref().map(|(bk, _)| key < *bk).unwrap_or(true) {
                best_ge = Some((key, resolved));
            }
        } else {
            let key = (dim, bw);
            if best_lt.as_ref().map(|(bk, _)| key > *bk).unwrap_or(true) {
                best_lt = Some((key, resolved));
            }
        }
    }

    // Avoid jumping to 960p/1080p (>1.5x target_px) when a near-target variant
    // (>= 92% of target_px, e.g. 486p or 480p for bucket 512) is available, while preserving
    // exact >= target_px selection whenever an intermediate variant (e.g. 720p for 512) exists.
    if let (Some(((ge_dim, _), _)), Some(((lt_dim, _), lt_url))) = (&best_ge, &best_lt) {
        let min_tolerant_dim = (target_px as u64 * 92 + 99) / 100;
        let max_reasonable_ge = (target_px as u64 * 3) / 2;
        if (*ge_dim as u64) > max_reasonable_ge && (*lt_dim as u64) >= min_tolerant_dim {
            return Some(lt_url.clone());
        }
    }

    best_ge.or(best_lt).map(|(_, u)| u)
}

/// Parses an HLS media/variant playlist to locate the backing contiguous `.mp4` / `.m4s`
/// asset URL, preferring `#EXT-X-MAP:URI="..."` when present or first non-comment media URI.
pub fn parse_mp4_url_from_variant(m3u8: &str, variant_url: &str) -> Option<String> {
    let mut map_uri: Option<String> = None;
    let mut first_seg: Option<String> = None;

    for raw_line in m3u8.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXT-X-MAP:") {
            if let Some(uri) = extract_attr_quoted(line, "URI=") {
                let resolved = resolve_hls_url(variant_url, uri);
                if !resolved.is_empty() {
                    map_uri = Some(resolved);
                    break;
                }
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let lower = line.split('?').next().unwrap_or(line).to_ascii_lowercase();
        if lower.ends_with(".mp4") || lower.ends_with(".m4s") {
            let resolved = resolve_hls_url(variant_url, line);
            if !resolved.is_empty() {
                first_seg = Some(resolved);
                if lower.ends_with(".mp4") {
                    break;
                }
            }
        }
    }

    map_uri.or(first_seg)
}

fn collect_all_variant_segments(m3u8: &str, variant_url: &str) -> (Option<String>, Vec<String>) {
    let mut init_url: Option<String> = None;
    let mut segments: Vec<String> = Vec::new();
    for raw_line in m3u8.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXT-X-MAP:") {
            if let Some(uri) = extract_attr_quoted(line, "URI=") {
                let r = resolve_hls_url(variant_url, uri);
                if !r.is_empty() {
                    init_url = Some(r);
                }
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let r = resolve_hls_url(variant_url, line);
        if !r.is_empty() && !segments.contains(&r) {
            segments.push(r);
        }
    }
    (init_url, segments)
}

fn motion_cache_dir() -> Option<PathBuf> {
    static DIR_CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR_CACHE
        .get_or_init(|| {
            let local = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .or_else(dirs_fallback_local_appdata)?;
            let dir = local.join("AM-Light").join("cache").join("motion_art");
            let _ = std::fs::create_dir_all(&dir);
            Some(dir)
        })
        .clone()
}

fn dirs_fallback_local_appdata() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("AppData").join("Local"))
}

fn hash_url_key(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Clone, Debug)]
struct CachedFileEntry {
    file_name: String,
    size: u64,
    last_access_tick: u64,
}

enum DiskIndexLookup {
    Hit(PathBuf),
    MissInitialized,
    NotYetInitialized,
}

/// In-memory disk cache LRU index with hysteresis (`max_files -> target_files`,
/// `max_bytes -> target_bytes`), populated once via a single-stat directory scan and updated
/// incrementally on write/hit/evict — eliminating per-download `read_dir` + double-`stat` storms.
#[derive(Debug)]
pub struct MotionDiskCacheLru {
    dir: PathBuf,
    max_files: usize,
    target_files: usize,
    max_bytes: u64,
    target_bytes: u64,
    initialized: bool,
    clock: u64,
    total_bytes: u64,
    entries: HashMap<String, CachedFileEntry>,
}

impl MotionDiskCacheLru {
    pub fn new(
        dir: PathBuf,
        max_files: usize,
        target_files: usize,
        max_bytes: u64,
        target_bytes: u64,
    ) -> Self {
        Self {
            dir,
            max_files,
            target_files: target_files.min(max_files),
            max_bytes,
            target_bytes: target_bytes.min(max_bytes),
            initialized: false,
            clock: 0,
            total_bytes: 0,
            entries: HashMap::new(),
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn path_for_key(&self, key: &str) -> Option<PathBuf> {
        self.entries.get(key).map(|e| self.dir.join(&e.file_name))
    }

    /// True LRU access-time refresh on playback/hit (O(1) RAM, zero disk I/O).
    pub fn touch(&mut self, key: &str) -> Option<PathBuf> {
        if !self.entries.contains_key(key) {
            return None;
        }
        let tick = self.next_tick();
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_access_tick = tick;
            Some(self.dir.join(&entry.file_name))
        } else {
            None
        }
    }

    /// Removes a key from the index (e.g. after detecting a corrupt/missing file).
    pub fn remove(&mut self, key: &str) -> bool {
        if let Some(old) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.size);
            true
        } else {
            false
        }
    }

    /// Registers an existing on-disk `{key}.mp4` discovered via fallback probe without evicting.
    pub fn record_existing(&mut self, key: &str, size: u64) {
        let tick = self.next_tick();
        if let Some(existing) = self.entries.get_mut(key) {
            self.total_bytes = self.total_bytes.saturating_sub(existing.size);
            existing.size = size;
            existing.last_access_tick = tick;
            self.total_bytes = self.total_bytes.saturating_add(size);
        } else {
            self.entries.insert(
                key.to_string(),
                CachedFileEntry {
                    file_name: format!("{key}.mp4"),
                    size,
                    last_access_tick: tick,
                },
            );
            self.total_bytes = self.total_bytes.saturating_add(size);
        }
    }

    /// Records a newly written `{key}.mp4` of `size` bytes, refreshes its LRU rank, and when
    /// high-water bounds are exceeded returns evicted file paths down to low-water hysteresis
    /// targets so the caller can unlink them outside the mutex.
    pub fn record_write(&mut self, key: &str, size: u64) -> Vec<PathBuf> {
        self.record_existing(key, size);
        self.collect_hysteresis_evictions()
    }

    fn merge_startup_scan(
        &mut self,
        mut scanned: Vec<(String, String, u64, SystemTime)>,
    ) -> Vec<PathBuf> {
        scanned.sort_by_key(|(_, _, _, mt)| *mt);
        let mut new_items: Vec<(String, String, u64)> = Vec::with_capacity(scanned.len());
        for (key, file_name, size, _) in scanned {
            if !self.entries.contains_key(&key) {
                new_items.push((key, file_name, size));
            }
        }
        let shift = new_items.len() as u64;
        if shift > 0 {
            // Preserve newer recency for any files recorded/touched during the current session
            for existing in self.entries.values_mut() {
                existing.last_access_tick = existing.last_access_tick.saturating_add(shift);
            }
            self.clock = self.clock.saturating_add(shift);
            for (idx, (key, file_name, size)) in new_items.into_iter().enumerate() {
                self.entries.insert(
                    key,
                    CachedFileEntry {
                        file_name,
                        size,
                        last_access_tick: (idx as u64) + 1,
                    },
                );
                self.total_bytes = self.total_bytes.saturating_add(size);
            }
        }
        self.initialized = true;
        self.collect_hysteresis_evictions()
    }

    fn collect_hysteresis_evictions(&mut self) -> Vec<PathBuf> {
        if self.entries.len() <= self.max_files && self.total_bytes <= self.max_bytes {
            return Vec::new();
        }
        let mut ordered: Vec<(String, u64)> = self
            .entries
            .iter()
            .map(|(k, e)| (k.clone(), e.last_access_tick))
            .collect();
        ordered.sort_by_key(|(_, tick)| *tick);

        let mut to_delete = Vec::new();
        for (k, _) in ordered {
            if self.entries.len() <= self.target_files && self.total_bytes <= self.target_bytes {
                break;
            }
            if let Some(removed) = self.entries.remove(&k) {
                self.total_bytes = self.total_bytes.saturating_sub(removed.size);
                to_delete.push(self.dir.join(removed.file_name));
            }
        }
        to_delete
    }

    /// Scans `self.dir` once with a single `item.metadata()` stat per entry (no separate
    /// `path.is_file()`), sweeps orphaned `*.mp4.part` files, populates the index, and prunes
    /// any excess down to low-water hysteresis targets.
    pub fn scan_dir_once(&mut self) -> Vec<PathBuf> {
        let (scanned, orphan_parts) = scan_motion_cache_dir_single_stat(&self.dir);
        for part in orphan_parts {
            let _ = std::fs::remove_file(part);
        }
        let evict = self.merge_startup_scan(scanned);
        for p in &evict {
            let _ = std::fs::remove_file(p);
        }
        evict
    }
}

fn scan_motion_cache_dir_single_stat(
    dir: &Path,
) -> (Vec<(String, String, u64, SystemTime)>, Vec<PathBuf>) {
    let mut valid = Vec::new();
    let mut orphan_parts = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (valid, orphan_parts);
    };
    for item in rd.flatten() {
        let Ok(meta) = item.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let fname_os = item.file_name();
        let Some(fname) = fname_os.to_str() else {
            continue;
        };
        if fname.ends_with(".mp4.part") {
            orphan_parts.push(item.path());
            continue;
        }
        if let Some(key) = fname.strip_suffix(".mp4") {
            let len = meta.len();
            if len > MOTION_CACHE_MIN_VALID_BYTES && !key.is_empty() {
                let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
                valid.push((key.to_string(), fname.to_string(), len, mtime));
            }
        }
    }
    (valid, orphan_parts)
}

static MOTION_DISK_INDEX: OnceLock<Option<Mutex<MotionDiskCacheLru>>> = OnceLock::new();
static DISK_INDEX_WARMUP_STARTED: AtomicBool = AtomicBool::new(false);

fn global_disk_index() -> Option<&'static Mutex<MotionDiskCacheLru>> {
    MOTION_DISK_INDEX
        .get_or_init(|| {
            let dir = motion_cache_dir()?;
            Some(Mutex::new(MotionDiskCacheLru::new(
                dir,
                MOTION_CACHE_MAX_FILES,
                MOTION_CACHE_TARGET_FILES,
                MOTION_CACHE_MAX_BYTES,
                MOTION_CACHE_TARGET_BYTES,
            )))
        })
        .as_ref()
}

fn trigger_disk_cache_index_warmup_async() {
    if DISK_INDEX_WARMUP_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let Some(dir) = motion_cache_dir() else {
        return;
    };
    let _ = global_disk_index();
    let _ = std::thread::Builder::new()
        .name("am-motion-cache-idx".to_string())
        .spawn(move || {
            let (scanned, orphan_parts) = scan_motion_cache_dir_single_stat(&dir);
            for part in orphan_parts {
                let _ = std::fs::remove_file(part);
            }
            let evict = if let Some(idx_lock) = global_disk_index() {
                idx_lock
                    .lock()
                    .map(|mut idx| idx.merge_startup_scan(scanned))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            for path in evict {
                let _ = std::fs::remove_file(path);
            }
        });
}

fn disk_index_lookup_state_for_url(clean_url: &str) -> DiskIndexLookup {
    let key = hash_url_key(clean_url);
    let Some(idx_lock) = global_disk_index() else {
        return DiskIndexLookup::NotYetInitialized;
    };
    let Ok(idx) = idx_lock.lock() else {
        return DiskIndexLookup::NotYetInitialized;
    };
    if let Some(p) = idx.path_for_key(&key) {
        DiskIndexLookup::Hit(p)
    } else if idx.is_initialized() {
        DiskIndexLookup::MissInitialized
    } else {
        DiskIndexLookup::NotYetInitialized
    }
}

fn disk_index_lookup_and_touch_key(key: &str) -> DiskIndexLookup {
    let Some(idx_lock) = global_disk_index() else {
        return DiskIndexLookup::NotYetInitialized;
    };
    let Ok(mut idx) = idx_lock.lock() else {
        return DiskIndexLookup::NotYetInitialized;
    };
    if let Some(p) = idx.touch(key) {
        DiskIndexLookup::Hit(p)
    } else if idx.is_initialized() {
        DiskIndexLookup::MissInitialized
    } else {
        DiskIndexLookup::NotYetInitialized
    }
}

fn disk_index_contains_url(clean_url: &str) -> bool {
    matches!(
        disk_index_lookup_state_for_url(clean_url),
        DiskIndexLookup::Hit(_)
    )
}

fn disk_index_touch_url(clean_url: &str) {
    let key = hash_url_key(clean_url);
    if let Some(idx_lock) = global_disk_index() {
        if let Ok(mut idx) = idx_lock.lock() {
            let _ = idx.touch(&key);
        }
    }
}

fn disk_index_remove_url(clean_url: &str) {
    let key = hash_url_key(clean_url);
    if let Some(idx_lock) = global_disk_index() {
        if let Ok(mut idx) = idx_lock.lock() {
            idx.remove(&key);
        }
    }
}

fn disk_index_record_existing_file(clean_url: &str, size: u64) {
    let key = hash_url_key(clean_url);
    if let Some(idx_lock) = global_disk_index() {
        if let Ok(mut idx) = idx_lock.lock() {
            idx.record_existing(&key, size);
        }
    }
}

fn disk_index_record_write_and_evict(key: &str, size: u64) {
    let evict = if let Some(idx_lock) = global_disk_index() {
        idx_lock
            .lock()
            .map(|mut idx| idx.record_write(key, size))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    for path in evict {
        let _ = std::fs::remove_file(path);
    }
}

fn cached_mp4_path_and_len_if_ready(m3u8_url: &str) -> Option<(PathBuf, u64)> {
    let dir = motion_cache_dir()?;
    let key = hash_url_key(m3u8_url);
    let out_path = dir.join(format!("{key}.mp4"));
    let meta = std::fs::metadata(&out_path).ok()?;
    let len = meta.len();
    if meta.is_file() && len > MOTION_CACHE_MIN_VALID_BYTES {
        Some((out_path, len))
    } else {
        None
    }
}

struct DownloadCancelFlag(Arc<AtomicBool>);

impl Drop for DownloadCancelFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct PartFileGuard {
    path: PathBuf,
    committed: bool,
}

impl Drop for PartFileGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn ensure_cached_mp4_for_m3u8_async(
    http: &reqwest::Client,
    m3u8_url: &str,
    target_px: u32,
    part_token: u64,
) -> Result<PathBuf, String> {
    let dir = motion_cache_dir().ok_or_else(|| "无法定位本地缓存目录".to_string())?;
    let key = hash_url_key(m3u8_url);
    let out_path = dir.join(format!("{key}.mp4"));

    match disk_index_lookup_and_touch_key(&key) {
        DiskIndexLookup::Hit(path) => return Ok(path),
        DiskIndexLookup::MissInitialized => {}
        DiskIndexLookup::NotYetInitialized => {
            let probe_path = out_path.clone();
            let existing_len = tokio::task::spawn_blocking(move || {
                std::fs::metadata(&probe_path)
                    .ok()
                    .filter(|m| m.is_file() && m.len() > MOTION_CACHE_MIN_VALID_BYTES)
                    .map(|m| m.len())
            })
            .await
            .ok()
            .flatten();
            if let Some(len) = existing_len {
                disk_index_record_write_and_evict(&key, len);
                return Ok(out_path);
            }
        }
    }

    let tmp_path = dir.join(format!("{key}.{part_token}.mp4.part"));
    let mut part_guard = PartFileGuard {
        path: tmp_path.clone(),
        committed: false,
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_guard = DownloadCancelFlag(Arc::clone(&cancelled));

    let master_text = {
        let resp = http
            .get(m3u8_url)
            .send()
            .await
            .map_err(|e| format!("拉取动态封面列表失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("动态封面 HTTP {}", resp.status()));
        }
        resp.text()
            .await
            .map_err(|e| format!("读取动态封面列表失败: {e}"))?
    };

    let (variant_url, variant_text) = if master_text.contains("#EXT-X-STREAM-INF") {
        let v_url = parse_avc_variant_for_target_dim(m3u8_url, &master_text, target_px)
            .or_else(|| parse_best_avc_variant_from_master(&master_text, m3u8_url))
            .ok_or_else(|| "未找到可用的 AVC H.264 视频流".to_string())?;
        let resp = http
            .get(&v_url)
            .send()
            .await
            .map_err(|e| format!("拉取子播放列表失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("子播放列表 HTTP {}", resp.status()));
        }
        let v_txt = resp
            .text()
            .await
            .map_err(|e| format!("读取子播放列表失败: {e}"))?;
        (v_url, v_txt)
    } else {
        (m3u8_url.to_string(), master_text)
    };

    let (init_url, segments) = collect_all_variant_segments(&variant_text, &variant_url);
    let single_mp4 = parse_mp4_url_from_variant(&variant_text, &variant_url);

    let mut download_urls: Vec<String> = Vec::new();
    let multi_segment_fmp4 = init_url.is_some()
        && segments.len() > 1
        && segments
            .first()
            .map(|s| Some(s) != init_url.as_ref())
            .unwrap_or(false);

    if multi_segment_fmp4 {
        if let Some(init) = init_url {
            download_urls.push(init);
        }
        download_urls.extend(segments.into_iter().take(64));
    } else if let Some(single) = single_mp4 {
        download_urls.push(single);
    } else if !segments.is_empty() {
        download_urls.push(segments[0].clone());
    }

    if download_urls.is_empty() {
        return Err("播放列表中未包含可下载的 MP4 媒体流".to_string());
    }

    let mut combined: Vec<u8> = Vec::with_capacity(512 * 1024);
    for seg_url in download_urls {
        let resp = http
            .get(&seg_url)
            .send()
            .await
            .map_err(|e| format!("下载动态封面分段失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("动态封面媒体 HTTP {}", resp.status()));
        }
        let chunk = resp
            .bytes()
            .await
            .map_err(|e| format!("读取动态封面字节失败: {e}"))?;
        combined.extend_from_slice(&chunk);
        if combined.len() > 24 * 1024 * 1024 {
            return Err("动态封面体积超出安全上限 (24 MB)".to_string());
        }
    }

    if combined.len() < 1024 {
        return Err("动态封面 MP4 文件过小".to_string());
    }

    let file_len = combined.len() as u64;
    let out_clone = out_path.clone();
    let key_clone = key.clone();
    let cancelled_c = Arc::clone(&cancelled);
    let res = tokio::task::spawn_blocking(move || {
        if cancelled_c.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err("动态封面下载已取消".to_string());
        }
        if let Err(e) = std::fs::write(&tmp_path, &combined) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("写入缓存文件失败: {e}"));
        }
        if cancelled_c.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err("动态封面下载已取消".to_string());
        }
        let _ = std::fs::remove_file(&out_clone);
        if let Err(e) = std::fs::rename(&tmp_path, &out_clone) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("提交缓存文件失败: {e}"));
        }
        disk_index_record_write_and_evict(&key_clone, file_len);
        Ok::<PathBuf, String>(out_clone)
    })
    .await
    .map_err(|e| format!("缓存写盘线程异常: {e}"))?;

    if res.is_ok() {
        part_guard.committed = true;
    }
    res
}

// ============================================================================
// Fast Strided BGRA Box / Bilinear Downscaler (CPU Fallback)
// ============================================================================

/// Reusable horizontal/vertical interpolation LUT buffers for `resample_bgra_strided`,
/// eliminating per-frame `Vec` allocations and inner-loop integer divisions on the CPU fallback path.
#[derive(Default)]
struct ResampleLutCache {
    bilinear_key: (usize, usize, usize, usize),
    x_idx0: Vec<usize>,
    x_idx1: Vec<usize>,
    x_frac: Vec<u32>,
    y_idx0: Vec<usize>,
    y_idx1: Vec<usize>,
    y_frac: Vec<u32>,
    box_key: (usize, usize, usize, usize),
    bx0: Vec<usize>,
    bx1: Vec<usize>,
    by0: Vec<usize>,
    by1: Vec<usize>,
}

impl ResampleLutCache {
    fn ensure_bilinear(&mut self, crop_w: usize, crop_h: usize, dst_w: usize, dst_h: usize) {
        let key = (crop_w, crop_h, dst_w, dst_h);
        if self.bilinear_key == key && self.x_idx0.len() == dst_w && self.y_idx0.len() == dst_h {
            return;
        }
        self.bilinear_key = key;
        let max_sx = crop_w.saturating_sub(1);
        let max_sy = crop_h.saturating_sub(1);

        self.x_idx0.clear();
        self.x_idx1.clear();
        self.x_frac.clear();
        if self.x_idx0.capacity() < dst_w {
            self.x_idx0.reserve(dst_w);
            self.x_idx1.reserve(dst_w);
            self.x_frac.reserve(dst_w);
        }
        for dx in 0..dst_w {
            // Fixed-point 8.8 center-aligned coordinate: ((dx + 0.5) * crop_w / dst_w - 0.5) * 256
            let num = ((dx as i64 * 2 + 1) * (crop_w as i64) * 128) / (dst_w as i64) - 128;
            let clamped = num.clamp(0, (max_sx as i64) * 256);
            let ix0 = (clamped >> 8) as usize;
            let ix1 = (ix0 + 1).min(max_sx);
            let fx = (clamped & 0xFF) as u32;
            self.x_idx0.push(ix0 * 4);
            self.x_idx1.push(ix1 * 4);
            self.x_frac.push(fx);
        }

        self.y_idx0.clear();
        self.y_idx1.clear();
        self.y_frac.clear();
        if self.y_idx0.capacity() < dst_h {
            self.y_idx0.reserve(dst_h);
            self.y_idx1.reserve(dst_h);
            self.y_frac.reserve(dst_h);
        }
        for dy in 0..dst_h {
            let num_y = ((dy as i64 * 2 + 1) * (crop_h as i64) * 128) / (dst_h as i64) - 128;
            let clamped_y = num_y.clamp(0, (max_sy as i64) * 256);
            let iy0 = (clamped_y >> 8) as usize;
            let iy1 = (iy0 + 1).min(max_sy);
            let fy = (clamped_y & 0xFF) as u32;
            self.y_idx0.push(iy0);
            self.y_idx1.push(iy1);
            self.y_frac.push(fy);
        }
    }

    fn ensure_box(&mut self, crop_w: usize, crop_h: usize, dst_w: usize, dst_h: usize) {
        let key = (crop_w, crop_h, dst_w, dst_h);
        if self.box_key == key && self.bx0.len() == dst_w && self.by0.len() == dst_h {
            return;
        }
        self.box_key = key;

        self.bx0.clear();
        self.bx1.clear();
        if self.bx0.capacity() < dst_w {
            self.bx0.reserve(dst_w);
            self.bx1.reserve(dst_w);
        }
        for dx in 0..dst_w {
            let x0 = (dx * crop_w) / dst_w;
            let x1 = (((dx + 1) * crop_w) / dst_w).max(x0 + 1).min(crop_w);
            self.bx0.push(x0);
            self.bx1.push(x1);
        }

        self.by0.clear();
        self.by1.clear();
        if self.by0.capacity() < dst_h {
            self.by0.reserve(dst_h);
            self.by1.reserve(dst_h);
        }
        for dy in 0..dst_h {
            let y0 = (dy * crop_h) / dst_h;
            let y1 = (((dy + 1) * crop_h) / dst_h).max(y0 + 1).min(crop_h);
            self.by0.push(y0);
            self.by1.push(y1);
        }
    }
}

thread_local! {
    static THREAD_RESAMPLE_LUT: RefCell<ResampleLutCache> =
        RefCell::new(ResampleLutCache::default());
}

/// Fast BGRA frame downscaler (box-area for >=2x reduction, fixed-point bilinear for <2x),
/// reusing thread-local horizontal/vertical interpolation LUT buffers with zero per-frame LUT allocs.
pub fn downscale_bgra_frame(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<u8> {
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw.saturating_mul(sh).saturating_mul(4) {
        return Vec::new();
    }
    let mut dst = vec![0u8; dw * dh * 4];
    THREAD_RESAMPLE_LUT.with(|cell| {
        let mut lut = cell.borrow_mut();
        unsafe {
            resample_bgra_strided(
                src.as_ptr(),
                (sw * 4) as isize,
                0,
                0,
                sw,
                sh,
                dst.as_mut_ptr(),
                dw,
                dh,
                &mut lut,
            );
        }
    });
    dst
}

/// Crops `[crop_x..crop_x+crop_w, crop_y..crop_y+crop_h]` from a strided BGRA buffer (supporting
/// both positive top-down and negative bottom-up `pitch_bytes`, where `scan0` points to row 0)
/// and writes directly into tightly-packed `dst` (`dst_w x dst_h x 4`), performing zero-copy
/// row blits when 1:1, fixed-point bilinear when `1 < scale < 2`, and exact box-area averaging
/// when `scale >= 2`, reusing `lut` with zero per-frame LUT allocations.
#[allow(clippy::too_many_arguments)]
unsafe fn resample_bgra_strided(
    scan0: *const u8,
    pitch_bytes: isize,
    crop_x: usize,
    crop_y: usize,
    crop_w: usize,
    crop_h: usize,
    dst: *mut u8,
    dst_w: usize,
    dst_h: usize,
    lut: &mut ResampleLutCache,
) {
    let dst_row_bytes = dst_w * 4;
    let src_x_bytes = crop_x * 4;

    if dst_w == crop_w && dst_h == crop_h {
        for y in 0..dst_h {
            let src_row = scan0
                .offset(((crop_y + y) as isize) * pitch_bytes)
                .add(src_x_bytes);
            std::ptr::copy_nonoverlapping(src_row, dst.add(y * dst_row_bytes), dst_row_bytes);
        }
        return;
    }

    if crop_w >= dst_w * 2 || crop_h >= dst_h * 2 {
        // Exact integer box-area downsampling (zero shimmer / zero aliasing for >=2x reduction)
        lut.ensure_box(crop_w, crop_h, dst_w, dst_h);
        let bx0 = lut.bx0.as_slice();
        let bx1 = lut.bx1.as_slice();
        let by0 = lut.by0.as_slice();
        let by1 = lut.by1.as_slice();
        for dy in 0..dst_h {
            let y0 = *by0.get_unchecked(dy);
            let y1 = *by1.get_unchecked(dy);
            let dst_row = dst.add(dy * dst_row_bytes);
            for dx in 0..dst_w {
                let x0 = *bx0.get_unchecked(dx);
                let x1 = *bx1.get_unchecked(dx);
                let mut sb = 0u32;
                let mut sg = 0u32;
                let mut sr = 0u32;
                let count = ((y1 - y0) * (x1 - x0)) as u32;
                for sy in y0..y1 {
                    let srow = scan0
                        .offset(((crop_y + sy) as isize) * pitch_bytes)
                        .add(src_x_bytes);
                    for sx in x0..x1 {
                        let p = srow.add(sx * 4);
                        sb += *p as u32;
                        sg += *p.add(1) as u32;
                        sr += *p.add(2) as u32;
                    }
                }
                let half = count >> 1;
                let dp = dst_row.add(dx * 4);
                *dp = ((sb + half) / count) as u8;
                *dp.add(1) = ((sg + half) / count) as u8;
                *dp.add(2) = ((sr + half) / count) as u8;
                *dp.add(3) = 255;
            }
        }
        return;
    }

    // Center-aligned 8-bit fixed-point bilinear downscale (silky smooth for 1.0x..2.0x ratios)
    lut.ensure_bilinear(crop_w, crop_h, dst_w, dst_h);
    let x_idx0 = lut.x_idx0.as_slice();
    let x_idx1 = lut.x_idx1.as_slice();
    let x_frac = lut.x_frac.as_slice();
    let y_idx0 = lut.y_idx0.as_slice();
    let y_idx1 = lut.y_idx1.as_slice();
    let y_frac = lut.y_frac.as_slice();

    for dy in 0..dst_h {
        let iy0 = *y_idx0.get_unchecked(dy);
        let iy1 = *y_idx1.get_unchecked(dy);
        let fy = *y_frac.get_unchecked(dy);
        let inv_fy = 256 - fy;

        let row0 = scan0
            .offset(((crop_y + iy0) as isize) * pitch_bytes)
            .add(src_x_bytes);
        let row1 = scan0
            .offset(((crop_y + iy1) as isize) * pitch_bytes)
            .add(src_x_bytes);
        let dst_row = dst.add(dy * dst_row_bytes);

        for dx in 0..dst_w {
            let ox0 = *x_idx0.get_unchecked(dx);
            let ox1 = *x_idx1.get_unchecked(dx);
            let fx = *x_frac.get_unchecked(dx);
            let inv_fx = 256 - fx;

            let w00 = inv_fx * inv_fy;
            let w10 = fx * inv_fy;
            let w01 = inv_fx * fy;
            let w11 = fx * fy;

            let p00 = row0.add(ox0);
            let p10 = row0.add(ox1);
            let p01 = row1.add(ox0);
            let p11 = row1.add(ox1);

            let b = (*p00 as u32 * w00
                + *p10 as u32 * w10
                + *p01 as u32 * w01
                + *p11 as u32 * w11
                + 32768)
                >> 16;
            let g = (*p00.add(1) as u32 * w00
                + *p10.add(1) as u32 * w10
                + *p01.add(1) as u32 * w01
                + *p11.add(1) as u32 * w11
                + 32768)
                >> 16;
            let r = (*p00.add(2) as u32 * w00
                + *p10.add(2) as u32 * w10
                + *p01.add(2) as u32 * w01
                + *p11.add(2) as u32 * w11
                + 32768)
                >> 16;

            let dp = dst_row.add(dx * 4);
            *dp = b as u8;
            *dp.add(1) = g as u8;
            *dp.add(2) = r as u8;
            *dp.add(3) = 255;
        }
    }
}

// ============================================================================
// Pure Win32 Dynamic Media Foundation H.264 Decoder (DXVA + CPU RGB32 Fallback)
// ============================================================================

const MF_VERSION: u32 = 0x0002_0070;
const MFSTARTUP_NOSOCKET: u32 = 0x1;
const MF_SOURCE_READER_FIRST_VIDEO_STREAM: u32 = 0xFFFF_FFFC;
const MF_SOURCE_READERF_ENDOFSTREAM: u32 = 0x0000_0002;
const MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED: u32 = 0x0000_0020;

const MF_MT_MAJOR_TYPE: GUID = GUID::from_u128(0x48eba18e_f8c9_4687_bf11_0a74c9f96a8f);
const MF_MT_SUBTYPE: GUID = GUID::from_u128(0xf7e34c9a_42e8_4714_b74b_cb29d72c35e5);
const MF_MT_FRAME_SIZE: GUID = GUID::from_u128(0x1652c33d_d6b2_4012_b834_72030849a37d);
const MF_MT_DEFAULT_STRIDE: GUID = GUID::from_u128(0x644b4e48_1e02_4516_b0eb_c01ca9d49ac6);
const MF_MT_MINIMUM_DISPLAY_APERTURE: GUID =
    GUID::from_u128(0xd7388766_18fe_48c6_a177_ee894867c8c4);
const MF_MT_GEOMETRIC_APERTURE: GUID =
    GUID::from_u128(0x66758743_7e5f_400d_980a_aa8596c85696);
const MF_MT_PAN_SCAN_APERTURE: GUID =
    GUID::from_u128(0x79614dde_9187_48fb_b8c7_4d52689de649);
const MF_MEDIA_TYPE_VIDEO: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00aa00389b71);
const MF_VIDEO_FORMAT_RGB32: GUID = GUID::from_u128(0x00000016_0000_0010_8000_00aa00389b71);
const MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING: GUID =
    GUID::from_u128(0xfb394f3d_ccf1_42ee_bbb3_f9b845d5681d);
const IID_IMF2DBUFFER: GUID = GUID::from_u128(0x7dc9d5f9_9ed9_44ec_9bbf_0600bb589fbb);
const GUID_NULL: GUID = GUID::from_u128(0);

#[repr(C)]
struct UnknownVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct PropVariantI8 {
    vt: u16,
    reserved1: u16,
    reserved2: u16,
    reserved3: u16,
    val: i64,
    pad: u64,
}

#[repr(C)]
struct MfAttributesVtbl {
    base: UnknownVtbl,
    get_item: *const c_void,
    get_item_type: *const c_void,
    compare_item: *const c_void,
    compare: *const c_void,
    get_uint32: unsafe extern "system" fn(*mut c_void, *const GUID, *mut u32) -> HRESULT,
    get_uint64: unsafe extern "system" fn(*mut c_void, *const GUID, *mut u64) -> HRESULT,
    get_double: *const c_void,
    get_guid: *const c_void,
    get_string_length: *const c_void,
    get_string: *const c_void,
    get_allocated_string: *const c_void,
    get_blob_size: *const c_void,
    get_blob: unsafe extern "system" fn(*mut c_void, *const GUID, *mut u8, u32, *mut u32) -> HRESULT,
    get_allocated_blob: *const c_void,
    get_unknown: *const c_void,
    set_item: *const c_void,
    delete_item: *const c_void,
    delete_all_items: *const c_void,
    set_uint32: unsafe extern "system" fn(*mut c_void, *const GUID, u32) -> HRESULT,
    set_uint64: *const c_void,
    set_double: *const c_void,
    set_guid: unsafe extern "system" fn(*mut c_void, *const GUID, *const GUID) -> HRESULT,
    set_string: *const c_void,
    set_blob: *const c_void,
    set_unknown: unsafe extern "system" fn(*mut c_void, *const GUID, *mut c_void) -> HRESULT,
    lock_store: *const c_void,
    unlock_store: *const c_void,
    get_count: *const c_void,
    get_item_by_index: *const c_void,
    copy_all_items: *const c_void,
}

#[repr(C)]
struct MfSourceReaderVtbl {
    base: UnknownVtbl,
    get_stream_selection: *const c_void,
    set_stream_selection: *const c_void,
    get_native_media_type: *const c_void,
    get_current_media_type:
        unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    set_current_media_type:
        unsafe extern "system" fn(*mut c_void, u32, *const u32, *mut c_void) -> HRESULT,
    set_current_position:
        unsafe extern "system" fn(*mut c_void, *const GUID, *const PropVariantI8) -> HRESULT,
    read_sample: unsafe extern "system" fn(
        *mut c_void,
        u32,
        u32,
        *mut u32,
        *mut u32,
        *mut i64,
        *mut *mut c_void,
    ) -> HRESULT,
    flush: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    get_service_for_stream: *const c_void,
    get_presentation_attribute: *const c_void,
}

#[repr(C)]
struct MfSampleVtbl {
    base: MfAttributesVtbl,
    get_sample_flags: *const c_void,
    set_sample_flags: *const c_void,
    get_sample_time: *const c_void,
    set_sample_time: *const c_void,
    get_sample_duration: *const c_void,
    set_sample_duration: *const c_void,
    get_buffer_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_buffer_by_index: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    convert_to_contiguous_buffer:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    add_buffer: *const c_void,
    remove_buffer_by_index: *const c_void,
    remove_all_buffers: *const c_void,
    get_total_length: *const c_void,
    copy_to_buffer: *const c_void,
}

#[repr(C)]
struct MfMediaBufferVtbl {
    base: UnknownVtbl,
    lock: unsafe extern "system" fn(*mut c_void, *mut *mut u8, *mut u32, *mut u32) -> HRESULT,
    unlock: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    get_current_length: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    set_current_length: *const c_void,
    get_max_length: *const c_void,
}

#[repr(C)]
struct Mf2dBufferVtbl {
    base: UnknownVtbl,
    lock2d: unsafe extern "system" fn(*mut c_void, *mut *mut u8, *mut i32) -> HRESULT,
    unlock2d: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    get_scanline0_and_pitch: *const c_void,
    is_contiguous_format: *const c_void,
    get_contiguous_length: *const c_void,
    contiguous_copy_to: *const c_void,
    contiguous_copy_from: *const c_void,
}

struct ComPtr {
    raw: *mut c_void,
}

impl ComPtr {
    fn from_raw_opt(raw: *mut c_void) -> Option<Self> {
        if raw.is_null() {
            None
        } else {
            Some(Self { raw })
        }
    }

    unsafe fn vtbl<T>(&self) -> &T {
        let vt = *(self.raw as *const *const T);
        &*vt
    }

    fn as_raw(&self) -> *mut c_void {
        self.raw
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                let unk = &**(self.raw as *const *const UnknownVtbl);
                (unk.release)(self.raw);
            }
            self.raw = std::ptr::null_mut();
        }
    }
}

type FnMfStartup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type FnMfShutdown = unsafe extern "system" fn() -> HRESULT;
type FnMfCreateAttributes = unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT;
type FnMfCreateMediaType = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type FnMfCreateSourceReaderFromURL =
    unsafe extern "system" fn(*const u16, *mut c_void, *mut *mut c_void) -> HRESULT;

struct MfRuntime {
    mfplat: HMODULE,
    mfreadwrite: HMODULE,
    started: bool,
    fn_shutdown: FnMfShutdown,
    fn_create_attributes: FnMfCreateAttributes,
    fn_create_media_type: FnMfCreateMediaType,
    fn_create_source_reader: FnMfCreateSourceReaderFromURL,
}

unsafe fn load_proc<T>(module: HMODULE, name: PCSTR) -> Option<T> {
    let proc = GetProcAddress(module, name)?;
    Some(std::mem::transmute_copy(&proc))
}

impl MfRuntime {
    fn load() -> Result<Self, String> {
        unsafe {
            let mfplat = LoadLibraryW(w!("mfplat.dll"))
                .map_err(|e| format!("加载 mfplat.dll 失败: {e}"))?;
            let mfreadwrite = match LoadLibraryW(w!("mfreadwrite.dll")) {
                Ok(h) => h,
                Err(e) => {
                    let _ = FreeLibrary(mfplat);
                    return Err(format!("加载 mfreadwrite.dll 失败: {e}"));
                }
            };

            let fn_startup: FnMfStartup = load_proc(mfplat, s!("MFStartup")).ok_or_else(|| {
                let _ = FreeLibrary(mfreadwrite);
                let _ = FreeLibrary(mfplat);
                "缺少 MFStartup".to_string()
            })?;
            let fn_shutdown: FnMfShutdown =
                load_proc(mfplat, s!("MFShutdown")).ok_or_else(|| {
                    let _ = FreeLibrary(mfreadwrite);
                    let _ = FreeLibrary(mfplat);
                    "缺少 MFShutdown".to_string()
                })?;
            let fn_create_attributes: FnMfCreateAttributes =
                load_proc(mfplat, s!("MFCreateAttributes")).ok_or_else(|| {
                    let _ = FreeLibrary(mfreadwrite);
                    let _ = FreeLibrary(mfplat);
                    "缺少 MFCreateAttributes".to_string()
                })?;
            let fn_create_media_type: FnMfCreateMediaType =
                load_proc(mfplat, s!("MFCreateMediaType")).ok_or_else(|| {
                    let _ = FreeLibrary(mfreadwrite);
                    let _ = FreeLibrary(mfplat);
                    "缺少 MFCreateMediaType".to_string()
                })?;
            let fn_create_source_reader: FnMfCreateSourceReaderFromURL =
                load_proc(mfreadwrite, s!("MFCreateSourceReaderFromURL")).ok_or_else(|| {
                    let _ = FreeLibrary(mfreadwrite);
                    let _ = FreeLibrary(mfplat);
                    "缺少 MFCreateSourceReaderFromURL".to_string()
                })?;

            let hr = fn_startup(MF_VERSION, MFSTARTUP_NOSOCKET);
            if hr.is_err() {
                let _ = FreeLibrary(mfreadwrite);
                let _ = FreeLibrary(mfplat);
                return Err(format!("MFStartup 失败: 0x{:08X}", hr.0 as u32));
            }

            Ok(Self {
                mfplat,
                mfreadwrite,
                started: true,
                fn_shutdown,
                fn_create_attributes,
                fn_create_media_type,
                fn_create_source_reader,
            })
        }
    }
}

impl Drop for MfRuntime {
    fn drop(&mut self) {
        unsafe {
            if self.started {
                let _ = (self.fn_shutdown)();
                self.started = false;
            }
            if !self.mfreadwrite.is_invalid() {
                let _ = FreeLibrary(self.mfreadwrite);
            }
            if !self.mfplat.is_invalid() {
                let _ = FreeLibrary(self.mfplat);
            }
        }
    }
}

/// Resolves the exact visible crop rectangle `(crop_x, crop_y, crop_w, crop_h)` inside a
/// macroblock-padded Media Foundation H.264 buffer (`coded_w x coded_h`), preventing uninitialized
/// right/bottom YUV `(0,0,0)` macroblock padding from rendering as flashing green borders
/// (e.g. `486x486` padded to `496x496` after `MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED`).
pub fn compute_clean_display_crop(
    coded_w: u32,
    coded_h: u32,
    aperture: Option<(i16, i16, i32, i32)>,
    initial_w: u32,
    initial_h: u32,
) -> (u32, u32, u32, u32) {
    if let Some((ox, oy, cx, cy)) = aperture {
        if ox >= 0 && oy >= 0 && cx > 0 && cy > 0 {
            let ux = ox as u32;
            let uy = oy as u32;
            let uw = cx as u32;
            let uh = cy as u32;
            if ux + uw <= coded_w && uy + uh <= coded_h {
                return (ux, uy, uw, uh);
            }
        }
    }
    let clean_w = if initial_w > 0 && initial_w < coded_w && coded_w - initial_w <= 32 {
        initial_w
    } else {
        coded_w
    };
    let clean_h = if initial_h > 0 && initial_h < coded_h && coded_h - initial_h <= 32 {
        initial_h
    } else {
        coded_h
    };
    (0, 0, clean_w, clean_h)
}

unsafe fn read_mf_video_area(media_type: &ComPtr, key: &GUID) -> Option<(i16, i16, i32, i32)> {
    let mut buf = [0u8; 16];
    let mut got: u32 = 0;
    let hr = (media_type.vtbl::<MfAttributesVtbl>().get_blob)(
        media_type.as_raw(),
        key,
        buf.as_mut_ptr(),
        16,
        &mut got,
    );
    if hr.is_ok() && got >= 16 {
        // MFVideoArea = { MFOffset OffsetX { fract: u16, value: i16 }, MFOffset OffsetY, SIZE { cx: i32, cy: i32 } }
        let ox = i16::from_le_bytes([buf[2], buf[3]]);
        let oy = i16::from_le_bytes([buf[6], buf[7]]);
        let cx = i32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let cy = i32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        Some((ox, oy, cx, cy))
    } else {
        None
    }
}

struct MultithreadGuard<'a>(Option<&'a ID3D11Multithread>);

impl<'a> MultithreadGuard<'a> {
    fn enter(mt: Option<&'a ID3D11Multithread>) -> Self {
        if let Some(m) = mt {
            unsafe {
                m.Enter();
            }
        }
        Self(mt)
    }
}

impl Drop for MultithreadGuard<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.0 {
            unsafe {
                m.Leave();
            }
        }
    }
}

struct DxgiDeviceLockGuard<'a> {
    mgr: &'a IMFDXGIDeviceManager,
    h_device: HANDLE,
    _locked_dev: Option<ComPtr>,
    locked: bool,
}

impl<'a> DxgiDeviceLockGuard<'a> {
    unsafe fn lock(mgr: &'a IMFDXGIDeviceManager, h_device: HANDLE) -> Result<Self, String> {
        let mut dev_raw: *mut c_void = std::ptr::null_mut();
        mgr.LockDevice(h_device, &ID3D11Device::IID, &mut dev_raw, true)
            .map_err(|e| format!("IMFDXGIDeviceManager::LockDevice 失败: {e}"))?;
        Ok(Self {
            mgr,
            h_device,
            _locked_dev: ComPtr::from_raw_opt(dev_raw),
            locked: true,
        })
    }
}

impl Drop for DxgiDeviceLockGuard<'_> {
    fn drop(&mut self) {
        if self.locked {
            self._locked_dev = None;
            unsafe {
                let _ = self.mgr.UnlockDevice(self.h_device, false);
            }
            self.locked = false;
        }
    }
}

const MAX_CACHED_VPIVS: usize = 32;

#[inline]
fn rect_eq(a: RECT, b: RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

struct DxvaVideoPipeline {
    mgr: IMFDXGIDeviceManager,
    h_device: HANDLE,
    d3d_dev: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_ctx: ID3D11VideoContext,
    multithread: Option<ID3D11Multithread>,
    vp_enum: Option<ID3D11VideoProcessorEnumerator>,
    vp: Option<ID3D11VideoProcessor>,
    /// 2-slot ping-pong BGRA output ring so D2D reading frame N-1 never races with
    /// `VideoProcessorBlt` writing frame N.
    out_tex: [Option<ID3D11Texture2D>; 2],
    out_vpov: [Option<ID3D11VideoProcessorOutputView>; 2],
    out_ring_idx: usize,
    /// Cached `ID3D11VideoProcessorInputView`s keyed by `(src_tex raw ptr, subresource_index)`,
    /// eliminating ~476/s COM VPIV allocations across concurrent DXVA slots.
    vpiv_cache: HashMap<(usize, u32), ID3D11VideoProcessorInputView>,
    configured_in_w: u32,
    configured_in_h: u32,
    configured_out_w: u32,
    configured_out_h: u32,
    configured_yuv_matrix: u32,
    configured_nominal_range: u32,
    last_src_rect: Option<RECT>,
    last_dst_rect: Option<RECT>,
}

impl DxvaVideoPipeline {
    fn new(bundle: &DxvaDeviceBundle) -> Result<Self, String> {
        unsafe {
            let video_device: ID3D11VideoDevice = bundle
                .d3d_dev
                .cast()
                .map_err(|e| format!("ID3D11VideoDevice 不可用: {e}"))?;
            let imm_ctx: ID3D11DeviceContext = bundle
                .d3d_dev
                .GetImmediateContext()
                .map_err(|e| format!("GetImmediateContext 失败: {e}"))?;
            let video_ctx: ID3D11VideoContext = imm_ctx
                .cast()
                .map_err(|e| format!("ID3D11VideoContext 不可用: {e}"))?;
            let multithread = bundle.d3d_dev.cast::<ID3D11Multithread>().ok();
            if let Some(ref mt) = multithread {
                let _ = mt.SetMultithreadProtected(true);
            }
            let h_device = bundle
                .mgr
                .0
                .OpenDeviceHandle()
                .map_err(|e| format!("OpenDeviceHandle 失败: {e}"))?;
            Ok(Self {
                mgr: bundle.mgr.0.clone(),
                h_device,
                d3d_dev: bundle.d3d_dev.clone(),
                video_device,
                video_ctx,
                multithread,
                vp_enum: None,
                vp: None,
                out_tex: [None, None],
                out_vpov: [None, None],
                out_ring_idx: 0,
                vpiv_cache: HashMap::with_capacity(24),
                configured_in_w: 0,
                configured_in_h: 0,
                configured_out_w: 0,
                configured_out_h: 0,
                configured_yuv_matrix: u32::MAX,
                configured_nominal_range: u32::MAX,
                last_src_rect: None,
                last_dst_rect: None,
            })
        }
    }

    unsafe fn ensure_processor(
        &mut self,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
        yuv_matrix: u32,
        nominal_range: u32,
    ) -> Result<(), String> {
        let same_geom = self.vp_enum.is_some()
            && self.vp.is_some()
            && self.out_tex[0].is_some()
            && self.out_tex[1].is_some()
            && self.out_vpov[0].is_some()
            && self.out_vpov[1].is_some()
            && self.configured_in_w == in_w
            && self.configured_in_h == in_h
            && self.configured_out_w == out_w
            && self.configured_out_h == out_h;
        if same_geom
            && self.configured_yuv_matrix == yuv_matrix
            && self.configured_nominal_range == nominal_range
        {
            return Ok(());
        }

        if !same_geom {
            // VPIVs are bound to the previous enumerator; drop them before rebuilding.
            self.vpiv_cache.clear();
            self.last_src_rect = None;
            self.last_dst_rect = None;
            self.out_ring_idx = 0;

            let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL {
                    Numerator: 30,
                    Denominator: 1,
                },
                InputWidth: in_w,
                InputHeight: in_h,
                OutputFrameRate: DXGI_RATIONAL {
                    Numerator: 30,
                    Denominator: 1,
                },
                OutputWidth: out_w,
                OutputHeight: out_h,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let vp_enum = self
                .video_device
                .CreateVideoProcessorEnumerator(&content_desc)
                .map_err(|e| format!("CreateVideoProcessorEnumerator 失败: {e}"))?;
            let vp = self
                .video_device
                .CreateVideoProcessor(&vp_enum, 0)
                .map_err(|e| format!("CreateVideoProcessor 失败: {e}"))?;

            // Double-buffered single-slice BGRA textures usable both as VideoProcessorOutputView
            // (RENDER_TARGET) and zero-copy Direct2D CreateBitmapFromDxgiSurface (SHADER_RESOURCE | RENDER_TARGET)
            let tex_desc = D3D11_TEXTURE2D_DESC {
                Width: out_w,
                Height: out_h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let vpov_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };

            let mut new_out_tex: [Option<ID3D11Texture2D>; 2] = [None, None];
            let mut new_out_vpov: [Option<ID3D11VideoProcessorOutputView>; 2] = [None, None];
            for i in 0..2 {
                let mut out_tex_opt: Option<ID3D11Texture2D> = None;
                self.d3d_dev
                    .CreateTexture2D(&tex_desc, None, Some(&mut out_tex_opt))
                    .map_err(|e| format!("CreateTexture2D(BGRA) 失败: {e}"))?;
                let out_tex =
                    out_tex_opt.ok_or_else(|| "CreateTexture2D 返回空纹理".to_string())?;

                let mut vpov_opt: Option<ID3D11VideoProcessorOutputView> = None;
                self.video_device
                    .CreateVideoProcessorOutputView(
                        &out_tex,
                        &vp_enum,
                        &vpov_desc,
                        Some(&mut vpov_opt),
                    )
                    .map_err(|e| format!("CreateVideoProcessorOutputView 失败: {e}"))?;
                let out_vpov =
                    vpov_opt.ok_or_else(|| "CreateVideoProcessorOutputView 返回空".to_string())?;

                new_out_tex[i] = Some(out_tex);
                new_out_vpov[i] = Some(out_vpov);
            }

            self.vp_enum = Some(vp_enum);
            self.vp = Some(vp);
            self.out_tex = new_out_tex;
            self.out_vpov = new_out_vpov;
            self.configured_in_w = in_w;
            self.configured_in_h = in_h;
            self.configured_out_w = out_w;
            self.configured_out_h = out_h;
        }

        if let Some(ref vp) = self.vp {
            let _mt = MultithreadGuard::enter(self.multithread.as_ref());
            self.video_ctx
                .VideoProcessorSetStreamAutoProcessingMode(vp, 0, false);
            self.video_ctx.VideoProcessorSetOutputAlphaFillMode(
                vp,
                D3D11_VIDEO_PROCESSOR_ALPHA_FILL_MODE_OPAQUE,
                0,
            );
            // Input YCbCr color space: BT.709 (default) or BT.601; limited 16-235 (default) or full 0-255
            let bt709_bit = if yuv_matrix == MFVideoTransferMatrix_BT601.0 as u32 {
                0u32
            } else {
                1u32 << 2
            };
            let nom_bits = if nominal_range == MFNominalRange_0_255.0 as u32 {
                2u32 << 4
            } else {
                1u32 << 4
            };
            let in_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: bt709_bit | nom_bits,
            };
            let out_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: 2u32 << 4, // Full-range sRGB 0..255
            };
            self.video_ctx.VideoProcessorSetStreamColorSpace(vp, 0, &in_cs);
            self.video_ctx.VideoProcessorSetOutputColorSpace(vp, &out_cs);
            self.configured_yuv_matrix = yuv_matrix;
            self.configured_nominal_range = nominal_range;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn blit_to_bgra(
        &mut self,
        src_tex: &ID3D11Texture2D,
        subresource_index: u32,
        crop_x: u32,
        crop_y: u32,
        crop_w: u32,
        crop_h: u32,
        out_w: u32,
        out_h: u32,
        yuv_matrix: u32,
        nominal_range: u32,
    ) -> Result<ID3D11Texture2D, String> {
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        src_tex.GetDesc(&mut src_desc);
        if src_desc.Width == 0 || src_desc.Height == 0 {
            return Err("DXVA 源纹理尺寸无效".to_string());
        }

        let mgr = self.mgr.clone();
        let _dev_lock = DxgiDeviceLockGuard::lock(&mgr, self.h_device)?;
        self.ensure_processor(
            src_desc.Width,
            src_desc.Height,
            out_w,
            out_h,
            yuv_matrix,
            nominal_range,
        )?;

        let vpiv_key = (src_tex.as_raw() as usize, subresource_index);
        let vpiv = if let Some(cached) = self.vpiv_cache.get(&vpiv_key) {
            cached.clone()
        } else {
            if self.vpiv_cache.len() >= MAX_CACHED_VPIVS {
                self.vpiv_cache.clear();
            }
            let vp_enum = self.vp_enum.as_ref().unwrap();
            let vpiv_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: subresource_index,
                    },
                },
            };
            let mut vpiv_opt: Option<ID3D11VideoProcessorInputView> = None;
            self.video_device
                .CreateVideoProcessorInputView(src_tex, vp_enum, &vpiv_desc, Some(&mut vpiv_opt))
                .map_err(|e| format!("CreateVideoProcessorInputView 失败: {e}"))?;
            let created =
                vpiv_opt.ok_or_else(|| "CreateVideoProcessorInputView 返回空".to_string())?;
            self.vpiv_cache.insert(vpiv_key, created.clone());
            created
        };

        let clamped_x = crop_x.min(src_desc.Width.saturating_sub(1));
        let clamped_y = crop_y.min(src_desc.Height.saturating_sub(1));
        let clamped_r = (clamped_x + crop_w).min(src_desc.Width).max(clamped_x + 1);
        let clamped_b = (clamped_y + crop_h).min(src_desc.Height).max(clamped_y + 1);
        let src_rect = RECT {
            left: clamped_x as i32,
            top: clamped_y as i32,
            right: clamped_r as i32,
            bottom: clamped_b as i32,
        };
        let dst_rect = RECT {
            left: 0,
            top: 0,
            right: out_w as i32,
            bottom: out_h as i32,
        };

        let ring_slot = self.out_ring_idx & 1;
        let update_src = self
            .last_src_rect
            .map(|r| !rect_eq(r, src_rect))
            .unwrap_or(true);
        let update_dst = self
            .last_dst_rect
            .map(|r| !rect_eq(r, dst_rect))
            .unwrap_or(true);

        let blt_res = {
            let vp = self.vp.as_ref().unwrap();
            let out_vpov = self.out_vpov[ring_slot].as_ref().unwrap();
            let _mt = MultithreadGuard::enter(self.multithread.as_ref());
            if update_src {
                self.video_ctx
                    .VideoProcessorSetStreamSourceRect(vp, 0, true, Some(&src_rect));
            }
            if update_dst {
                self.video_ctx
                    .VideoProcessorSetStreamDestRect(vp, 0, true, Some(&dst_rect));
                self.video_ctx
                    .VideoProcessorSetOutputTargetRect(vp, true, Some(&dst_rect));
            }

            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: ManuallyDrop::new(Some(vpiv)),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let res = self
                .video_ctx
                .VideoProcessorBlt(vp, out_vpov, 0, std::slice::from_ref(&stream));
            ManuallyDrop::drop(&mut stream.pInputSurface);
            ManuallyDrop::drop(&mut stream.pInputSurfaceRight);
            res
        };
        blt_res.map_err(|e| format!("VideoProcessorBlt 失败: {e}"))?;

        if update_src {
            self.last_src_rect = Some(src_rect);
        }
        if update_dst {
            self.last_dst_rect = Some(dst_rect);
        }
        let out_tex = self.out_tex[ring_slot].as_ref().unwrap().clone();
        self.out_ring_idx = ring_slot ^ 1;
        Ok(out_tex)
    }
}

impl Drop for DxvaVideoPipeline {
    fn drop(&mut self) {
        self.vpiv_cache.clear();
        self.out_vpov = [None, None];
        self.out_tex = [None, None];
        self.vp = None;
        self.vp_enum = None;
        if !self.h_device.is_invalid() {
            unsafe {
                let _ = self.mgr.CloseDeviceHandle(self.h_device);
            }
            self.h_device = HANDLE::default();
        }
    }
}

struct Mp4VideoDecoder {
    reader: ComPtr,
    dxva: Option<DxvaVideoPipeline>,
    dxva_generation: u64,
    target_dim: u32,
    coded_width: u32,
    coded_height: u32,
    initial_display_w: u32,
    initial_display_h: u32,
    crop_x: u32,
    crop_y: u32,
    crop_w: u32,
    crop_h: u32,
    stride: i32,
    yuv_matrix: u32,
    nominal_range: u32,
    mp4_path: PathBuf,
    first_sample_ts_100ns: Option<i64>,
    playback_epoch: Instant,
    frame_seq: u64,
    frame_pool: Vec<Arc<Vec<u8>>>,
    empty_bgra: Arc<Vec<u8>>,
    resample_lut: ResampleLutCache,
}

impl Mp4VideoDecoder {
    fn open(
        mf: &MfRuntime,
        mp4_path: &Path,
        target_dim: u32,
        dxva_bundle: Option<&DxvaDeviceBundle>,
    ) -> Result<Self, String> {
        let bucket = bucket_target_dim(target_dim);
        if let Some(bundle) = dxva_bundle {
            if let Ok(dxva_dec) = Self::open_dxva(mf, mp4_path, bucket, bundle) {
                return Ok(dxva_dec);
            }
        }
        let gen = dxva_bundle.map(|b| b.generation).unwrap_or(0);
        Self::open_cpu(mf, mp4_path, bucket, gen)
    }

    fn open_dxva(
        mf: &MfRuntime,
        mp4_path: &Path,
        target_dim: u32,
        bundle: &DxvaDeviceBundle,
    ) -> Result<Self, String> {
        let pipeline = DxvaVideoPipeline::new(bundle)?;
        // First try with MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING + hardware transforms,
        // then retry with D3D_MANAGER + hardware transforms alone if a driver rejects advanced VP.
        let reader = Self::create_dxva_reader(mf, mp4_path, &bundle.mgr.0, true)
            .or_else(|_| Self::create_dxva_reader(mf, mp4_path, &bundle.mgr.0, false))?;

        let mut dec = Self {
            reader,
            dxva: Some(pipeline),
            dxva_generation: bundle.generation,
            target_dim,
            coded_width: 0,
            coded_height: 0,
            initial_display_w: 0,
            initial_display_h: 0,
            crop_x: 0,
            crop_y: 0,
            crop_w: 0,
            crop_h: 0,
            stride: 0,
            yuv_matrix: 0,
            nominal_range: 0,
            mp4_path: mp4_path.to_path_buf(),
            first_sample_ts_100ns: None,
            playback_epoch: Instant::now(),
            frame_seq: 0,
            frame_pool: Vec::new(),
            empty_bgra: Arc::new(Vec::new()),
            resample_lut: ResampleLutCache::default(),
        };
        dec.refresh_media_geometry()?;
        dec.initial_display_w = dec.crop_w;
        dec.initial_display_h = dec.crop_h;
        Ok(dec)
    }

    fn create_dxva_reader(
        mf: &MfRuntime,
        mp4_path: &Path,
        mgr: &IMFDXGIDeviceManager,
        enable_advanced_vp: bool,
    ) -> Result<ComPtr, String> {
        unsafe {
            let mut attrs_raw: *mut c_void = std::ptr::null_mut();
            let hr = (mf.fn_create_attributes)(&mut attrs_raw, 4);
            let attrs = ComPtr::from_raw_opt(attrs_raw)
                .ok_or_else(|| format!("MFCreateAttributes 失败: 0x{:08X}", hr.0 as u32))?;
            let hr_mgr = (attrs.vtbl::<MfAttributesVtbl>().set_unknown)(
                attrs.as_raw(),
                &MF_SOURCE_READER_D3D_MANAGER,
                mgr.as_raw(),
            );
            if hr_mgr.is_err() {
                return Err(format!(
                    "SetUnknown(MF_SOURCE_READER_D3D_MANAGER) 失败: 0x{:08X}",
                    hr_mgr.0 as u32
                ));
            }
            let _ = (attrs.vtbl::<MfAttributesVtbl>().set_uint32)(
                attrs.as_raw(),
                &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                1,
            );
            if enable_advanced_vp {
                let _ = (attrs.vtbl::<MfAttributesVtbl>().set_uint32)(
                    attrs.as_raw(),
                    &MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
                    1,
                );
            }

            let mut wide: Vec<u16> = mp4_path
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mut reader_raw: *mut c_void = std::ptr::null_mut();
            let hr = (mf.fn_create_source_reader)(
                wide.as_mut_ptr(),
                attrs.as_raw(),
                &mut reader_raw,
            );
            let reader = ComPtr::from_raw_opt(reader_raw).ok_or_else(|| {
                format!(
                    "MFCreateSourceReaderFromURL(DXVA) 失败: 0x{:08X}",
                    hr.0 as u32
                )
            })?;

            let mut mtype_raw: *mut c_void = std::ptr::null_mut();
            let hr = (mf.fn_create_media_type)(&mut mtype_raw);
            let mtype = ComPtr::from_raw_opt(mtype_raw)
                .ok_or_else(|| format!("MFCreateMediaType 失败: 0x{:08X}", hr.0 as u32))?;
            let _ = (mtype.vtbl::<MfAttributesVtbl>().set_guid)(
                mtype.as_raw(),
                &MF_MT_MAJOR_TYPE,
                &MF_MEDIA_TYPE_VIDEO,
            );
            let _ = (mtype.vtbl::<MfAttributesVtbl>().set_guid)(
                mtype.as_raw(),
                &MF_MT_SUBTYPE,
                &MFVideoFormat_NV12,
            );
            let hr = (reader.vtbl::<MfSourceReaderVtbl>().set_current_media_type)(
                reader.as_raw(),
                MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                std::ptr::null(),
                mtype.as_raw(),
            );
            if hr.is_err() {
                return Err(format!(
                    "SetCurrentMediaType(NV12) 失败: 0x{:08X}",
                    hr.0 as u32
                ));
            }
            Ok(reader)
        }
    }

    fn open_cpu(
        mf: &MfRuntime,
        mp4_path: &Path,
        target_dim: u32,
        dxva_generation: u64,
    ) -> Result<Self, String> {
        unsafe {
            let mut attrs_raw: *mut c_void = std::ptr::null_mut();
            let hr = (mf.fn_create_attributes)(&mut attrs_raw, 1);
            let attrs = ComPtr::from_raw_opt(attrs_raw)
                .ok_or_else(|| format!("MFCreateAttributes 失败: 0x{:08X}", hr.0 as u32))?;
            let _ = (attrs.vtbl::<MfAttributesVtbl>().set_uint32)(
                attrs.as_raw(),
                &MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
                1,
            );

            let mut wide: Vec<u16> = mp4_path
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mut reader_raw: *mut c_void = std::ptr::null_mut();
            let hr = (mf.fn_create_source_reader)(
                wide.as_mut_ptr(),
                attrs.as_raw(),
                &mut reader_raw,
            );
            let reader = ComPtr::from_raw_opt(reader_raw).ok_or_else(|| {
                format!("MFCreateSourceReaderFromURL 失败: 0x{:08X}", hr.0 as u32)
            })?;

            let mut mtype_raw: *mut c_void = std::ptr::null_mut();
            let hr = (mf.fn_create_media_type)(&mut mtype_raw);
            let mtype = ComPtr::from_raw_opt(mtype_raw)
                .ok_or_else(|| format!("MFCreateMediaType 失败: 0x{:08X}", hr.0 as u32))?;
            let _ = (mtype.vtbl::<MfAttributesVtbl>().set_guid)(
                mtype.as_raw(),
                &MF_MT_MAJOR_TYPE,
                &MF_MEDIA_TYPE_VIDEO,
            );
            let _ = (mtype.vtbl::<MfAttributesVtbl>().set_guid)(
                mtype.as_raw(),
                &MF_MT_SUBTYPE,
                &MF_VIDEO_FORMAT_RGB32,
            );
            let hr = (reader.vtbl::<MfSourceReaderVtbl>().set_current_media_type)(
                reader.as_raw(),
                MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                std::ptr::null(),
                mtype.as_raw(),
            );
            if hr.is_err() {
                return Err(format!(
                    "SetCurrentMediaType(RGB32) 失败: 0x{:08X}",
                    hr.0 as u32
                ));
            }

            let mut dec = Self {
                reader,
                dxva: None,
                dxva_generation,
                target_dim,
                coded_width: 0,
                coded_height: 0,
                initial_display_w: 0,
                initial_display_h: 0,
                crop_x: 0,
                crop_y: 0,
                crop_w: 0,
                crop_h: 0,
                stride: 0,
                yuv_matrix: 0,
                nominal_range: 0,
                mp4_path: mp4_path.to_path_buf(),
                first_sample_ts_100ns: None,
                playback_epoch: Instant::now(),
                frame_seq: 0,
                frame_pool: Vec::with_capacity(3),
                empty_bgra: Arc::new(Vec::new()),
                resample_lut: ResampleLutCache::default(),
            };
            dec.refresh_media_geometry()?;
            dec.initial_display_w = dec.crop_w;
            dec.initial_display_h = dec.crop_h;
            Ok(dec)
        }
    }

    fn fallback_to_cpu(&mut self, mf: &MfRuntime) -> Result<(), String> {
        let prev_seq = self.frame_seq;
        let reopened = Self::open_cpu(mf, &self.mp4_path, self.target_dim, self.dxva_generation)?;
        *self = reopened;
        self.frame_seq = prev_seq;
        Ok(())
    }

    fn set_target_dim(&mut self, target_dim: u32) {
        let bucket = bucket_target_dim(target_dim);
        if self.target_dim != bucket {
            self.target_dim = bucket;
            self.frame_pool.clear();
        }
    }

    fn refresh_media_geometry(&mut self) -> Result<(), String> {
        unsafe {
            let mut cur_raw: *mut c_void = std::ptr::null_mut();
            let hr = (self.reader.vtbl::<MfSourceReaderVtbl>().get_current_media_type)(
                self.reader.as_raw(),
                MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                &mut cur_raw,
            );
            let cur = ComPtr::from_raw_opt(cur_raw).ok_or_else(|| {
                format!("GetCurrentMediaType 失败: 0x{:08X}", hr.0 as u32)
            })?;
            let mut size_packed: u64 = 0;
            let hr_sz = (cur.vtbl::<MfAttributesVtbl>().get_uint64)(
                cur.as_raw(),
                &MF_MT_FRAME_SIZE,
                &mut size_packed,
            );
            if hr_sz.is_err() {
                return Err(format!(
                    "读取 MF_MT_FRAME_SIZE 失败: 0x{:08X}",
                    hr_sz.0 as u32
                ));
            }
            let w = (size_packed >> 32) as u32;
            let h = (size_packed & 0xFFFF_FFFF) as u32;
            if w == 0 || h == 0 || w > 2560 || h > 2560 {
                return Err(format!("视频尺寸超出支持范围: {w}x{h}"));
            }
            let aperture = read_mf_video_area(&cur, &MF_MT_MINIMUM_DISPLAY_APERTURE)
                .or_else(|| read_mf_video_area(&cur, &MF_MT_GEOMETRIC_APERTURE))
                .or_else(|| read_mf_video_area(&cur, &MF_MT_PAN_SCAN_APERTURE));
            let (cx, cy, cw, ch) = compute_clean_display_crop(
                w,
                h,
                aperture,
                self.initial_display_w,
                self.initial_display_h,
            );
            let mut stride_u32: u32 = 0;
            let default_stride = if self.dxva.is_some() {
                w as i32
            } else {
                (w * 4) as i32
            };
            let stride = if (cur.vtbl::<MfAttributesVtbl>().get_uint32)(
                cur.as_raw(),
                &MF_MT_DEFAULT_STRIDE,
                &mut stride_u32,
            )
            .is_ok()
            {
                stride_u32 as i32
            } else {
                default_stride
            };
            let mut yuv_matrix = 0u32;
            let _ = (cur.vtbl::<MfAttributesVtbl>().get_uint32)(
                cur.as_raw(),
                &MF_MT_YUV_MATRIX,
                &mut yuv_matrix,
            );
            let mut nominal_range = 0u32;
            let _ = (cur.vtbl::<MfAttributesVtbl>().get_uint32)(
                cur.as_raw(),
                &MF_MT_VIDEO_NOMINAL_RANGE,
                &mut nominal_range,
            );

            self.coded_width = w;
            self.coded_height = h;
            self.crop_x = cx;
            self.crop_y = cy;
            self.crop_w = cw;
            self.crop_h = ch;
            self.stride = if stride == 0 { default_stride } else { stride };
            self.yuv_matrix = yuv_matrix;
            self.nominal_range = nominal_range;
            Ok(())
        }
    }

    fn rewind(
        &mut self,
        mf: &MfRuntime,
        dxva_bundle: Option<&DxvaDeviceBundle>,
    ) -> Result<(), String> {
        unsafe {
            let pv = PropVariantI8 {
                vt: 20, // VT_I8
                reserved1: 0,
                reserved2: 0,
                reserved3: 0,
                val: 0,
                pad: 0,
            };
            let hr = (self.reader.vtbl::<MfSourceReaderVtbl>().set_current_position)(
                self.reader.as_raw(),
                &GUID_NULL,
                &pv,
            );
            let _ = (self.reader.vtbl::<MfSourceReaderVtbl>().flush)(
                self.reader.as_raw(),
                MF_SOURCE_READER_FIRST_VIDEO_STREAM,
            );
            if hr.is_err() {
                let prev_seq = self.frame_seq;
                let use_bundle = if self.dxva.is_some() { dxva_bundle } else { None };
                let reopened = Self::open(mf, &self.mp4_path, self.target_dim, use_bundle)?;
                *self = reopened;
                self.frame_seq = prev_seq;
            }
            self.first_sample_ts_100ns = None;
            self.playback_epoch = Instant::now();
            Ok(())
        }
    }

    fn read_next_frame(
        &mut self,
        mf: &MfRuntime,
        slot_key: &str,
        dxva_bundle: Option<&DxvaDeviceBundle>,
        counters: &WorkerTelemetryCounters,
    ) -> Result<(Option<MotionFrameSnapshot>, Duration), String> {
        if self.dxva.is_some() {
            match self.read_next_frame_dxva(mf, slot_key, dxva_bundle, counters) {
                Ok(res) => return Ok(res),
                Err(_) => {
                    // Transparent per-decoder fallback from DXVA -> CPU RGB32 + fast downscale
                    self.fallback_to_cpu(mf)?;
                }
            }
        }
        self.read_next_frame_cpu(mf, slot_key, dxva_bundle, counters)
    }

    fn read_next_frame_dxva(
        &mut self,
        mf: &MfRuntime,
        slot_key: &str,
        dxva_bundle: Option<&DxvaDeviceBundle>,
        counters: &WorkerTelemetryCounters,
    ) -> Result<(Option<MotionFrameSnapshot>, Duration), String> {
        unsafe {
            let mut ts_100ns: i64 = 0;
            let mut _sample_guard: Option<ComPtr> = None;

            for _attempt in 0..3 {
                let mut actual_idx: u32 = 0;
                let mut flags: u32 = 0;
                let mut sample_raw: *mut c_void = std::ptr::null_mut();

                let hr = (self.reader.vtbl::<MfSourceReaderVtbl>().read_sample)(
                    self.reader.as_raw(),
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                    0,
                    &mut actual_idx,
                    &mut flags,
                    &mut ts_100ns,
                    &mut sample_raw,
                );
                if hr.is_err() {
                    return Err(format!("ReadSample(DXVA) 失败: 0x{:08X}", hr.0 as u32));
                }
                let sample_opt = ComPtr::from_raw_opt(sample_raw);

                if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) != 0 {
                    let _ = self.refresh_media_geometry();
                }
                if (flags & MF_SOURCE_READERF_ENDOFSTREAM) != 0 {
                    self.rewind(mf, dxva_bundle)?;
                    return Ok((None, Duration::from_millis(8)));
                }
                if sample_opt.is_some() {
                    _sample_guard = sample_opt;
                    break;
                }
            }
            let Some(sample) = _sample_guard.as_ref() else {
                return Ok((None, Duration::from_millis(8)));
            };

            // Prefer GetBufferByIndex(0) so DXGI buffers are NOT copied into contiguous system RAM
            let mut buf_raw: *mut c_void = std::ptr::null_mut();
            let hr_buf = (sample.vtbl::<MfSampleVtbl>().get_buffer_by_index)(
                sample.as_raw(),
                0,
                &mut buf_raw,
            );
            let buf = ComPtr::from_raw_opt(buf_raw).ok_or_else(|| {
                format!("GetBufferByIndex(0) 失败: 0x{:08X}", hr_buf.0 as u32)
            })?;

            let mut dxgi_buf_raw: *mut c_void = std::ptr::null_mut();
            let hr_qi = (buf.vtbl::<UnknownVtbl>().query_interface)(
                buf.as_raw(),
                &IMFDXGIBuffer::IID,
                &mut dxgi_buf_raw,
            );
            if hr_qi.is_err() || dxgi_buf_raw.is_null() {
                return Err(format!("QI(IMFDXGIBuffer) 失败: 0x{:08X}", hr_qi.0 as u32));
            }
            let dxgi_buf = IMFDXGIBuffer::from_raw(dxgi_buf_raw);

            let mut src_tex_raw: *mut c_void = std::ptr::null_mut();
            dxgi_buf
                .GetResource(&ID3D11Texture2D::IID, &mut src_tex_raw)
                .map_err(|e| format!("IMFDXGIBuffer::GetResource 失败: {e}"))?;
            if src_tex_raw.is_null() {
                return Err("IMFDXGIBuffer::GetResource 返回空指针".to_string());
            }
            let src_tex = ID3D11Texture2D::from_raw(src_tex_raw);
            let subresource_index = dxgi_buf.GetSubresourceIndex().unwrap_or(0);

            let (out_w, out_h) =
                compute_scaled_dimensions(self.crop_w, self.crop_h, self.target_dim);
            let downscaled = out_w < self.crop_w || out_h < self.crop_h;

            let out_tex = {
                let pipe = self
                    .dxva
                    .as_mut()
                    .ok_or_else(|| "DXVA 管线未初始化".to_string())?;
                pipe.blit_to_bgra(
                    &src_tex,
                    subresource_index,
                    self.crop_x,
                    self.crop_y,
                    self.crop_w,
                    self.crop_h,
                    out_w,
                    out_h,
                    self.yuv_matrix,
                    self.nominal_range,
                )?
            };

            let wait_dur = self.compute_wait_duration(ts_100ns);
            self.frame_seq = self.frame_seq.wrapping_add(1);
            counters.dxva_decoded_frames.fetch_add(1, Ordering::Relaxed);
            if downscaled {
                counters.downscaled_frames.fetch_add(1, Ordering::Relaxed);
            }

            let snap = MotionFrameSnapshot {
                key: slot_key.to_string(),
                width: out_w,
                height: out_h,
                frame_seq: self.frame_seq,
                bgra: Arc::clone(&self.empty_bgra),
                dxgi_texture: Some(out_tex),
                target_dim: self.target_dim,
            };
            Ok((Some(snap), wait_dur))
        }
    }

    fn read_next_frame_cpu(
        &mut self,
        mf: &MfRuntime,
        slot_key: &str,
        dxva_bundle: Option<&DxvaDeviceBundle>,
        counters: &WorkerTelemetryCounters,
    ) -> Result<(Option<MotionFrameSnapshot>, Duration), String> {
        unsafe {
            let mut ts_100ns: i64 = 0;
            let mut _sample_guard: Option<ComPtr> = None;

            for _attempt in 0..3 {
                let mut actual_idx: u32 = 0;
                let mut flags: u32 = 0;
                let mut sample_raw: *mut c_void = std::ptr::null_mut();

                let hr = (self.reader.vtbl::<MfSourceReaderVtbl>().read_sample)(
                    self.reader.as_raw(),
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                    0,
                    &mut actual_idx,
                    &mut flags,
                    &mut ts_100ns,
                    &mut sample_raw,
                );
                if hr.is_err() {
                    return Err(format!("ReadSample 失败: 0x{:08X}", hr.0 as u32));
                }
                let sample_opt = ComPtr::from_raw_opt(sample_raw);

                if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) != 0 {
                    let _ = self.refresh_media_geometry();
                }
                if (flags & MF_SOURCE_READERF_ENDOFSTREAM) != 0 {
                    self.rewind(mf, dxva_bundle)?;
                    return Ok((None, Duration::from_millis(8)));
                }
                if sample_opt.is_some() {
                    _sample_guard = sample_opt;
                    break;
                }
            }
            let Some(sample) = _sample_guard.as_ref() else {
                return Ok((None, Duration::from_millis(8)));
            };

            let mut buf_raw: *mut c_void = std::ptr::null_mut();
            let hr_buf = (sample.vtbl::<MfSampleVtbl>().convert_to_contiguous_buffer)(
                sample.as_raw(),
                &mut buf_raw,
            );
            let buf = ComPtr::from_raw_opt(buf_raw).ok_or_else(|| {
                format!("ConvertToContiguousBuffer 失败: 0x{:08X}", hr_buf.0 as u32)
            })?;

            let coded_h = self.coded_height as usize;
            let cx = self.crop_x as usize;
            let cy = self.crop_y as usize;
            let cw = self.crop_w as usize;
            let ch = self.crop_h as usize;
            let (out_w, out_h) =
                compute_scaled_dimensions(self.crop_w, self.crop_h, self.target_dim);
            let downscaled = out_w < self.crop_w || out_h < self.crop_h;
            let dw = out_w as usize;
            let dh = out_h as usize;
            let min_src_row_bytes = (cx + cw) * 4;
            let total_bytes = dw * dh * 4;

            // Recycle an unreferenced target-size BGRA buffer from our 3-slot pool
            let mut out_arc = if let Some(pos) = self
                .frame_pool
                .iter()
                .position(|b| Arc::strong_count(b) == 1 && b.len() == total_bytes)
            {
                self.frame_pool.swap_remove(pos)
            } else {
                Arc::new(vec![0u8; total_bytes])
            };
            let out: &mut [u8] = Arc::get_mut(&mut out_arc)
                .expect("recycled motion frame buffer must be uniquely owned")
                .as_mut_slice();

            // Try IMF2DBuffer first for exact scanline0 and signed pitch
            let mut buf2d_raw: *mut c_void = std::ptr::null_mut();
            let qi_ok = (buf.vtbl::<UnknownVtbl>().query_interface)(
                buf.as_raw(),
                &IID_IMF2DBUFFER,
                &mut buf2d_raw,
            )
            .is_ok();
            let buf2d = if qi_ok {
                ComPtr::from_raw_opt(buf2d_raw)
            } else {
                None
            };

            let mut copied = false;
            if let Some(ref b2d) = buf2d {
                let mut scan0: *mut u8 = std::ptr::null_mut();
                let mut pitch: i32 = 0;
                if (b2d.vtbl::<Mf2dBufferVtbl>().lock2d)(b2d.as_raw(), &mut scan0, &mut pitch)
                    .is_ok()
                {
                    if !scan0.is_null() && pitch.unsigned_abs() as usize >= min_src_row_bytes {
                        resample_bgra_strided(
                            scan0,
                            pitch as isize,
                            cx,
                            cy,
                            cw,
                            ch,
                            out.as_mut_ptr(),
                            dw,
                            dh,
                            &mut self.resample_lut,
                        );
                        copied = true;
                    }
                    let _ = (b2d.vtbl::<Mf2dBufferVtbl>().unlock2d)(b2d.as_raw());
                }
            }

            if !copied {
                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut max_len: u32 = 0;
                let mut cur_len: u32 = 0;
                let hr_lock = (buf.vtbl::<MfMediaBufferVtbl>().lock)(
                    buf.as_raw(),
                    &mut data_ptr,
                    &mut max_len,
                    &mut cur_len,
                );
                if hr_lock.is_err() || data_ptr.is_null() {
                    return Err(format!("MediaBuffer Lock 失败: 0x{:08X}", hr_lock.0 as u32));
                }
                let abs_stride = (self.stride.unsigned_abs() as usize).max(min_src_row_bytes);
                let top_down = self.stride >= 0;
                let avail = cur_len as usize;
                let need_full_src = coded_h.saturating_mul(abs_stride);
                if coded_h > 0 && need_full_src <= avail {
                    let (scan0_ptr, signed_pitch) = if top_down {
                        (data_ptr as *const u8, abs_stride as isize)
                    } else {
                        (
                            data_ptr.add((coded_h - 1) * abs_stride) as *const u8,
                            -(abs_stride as isize),
                        )
                    };
                    resample_bgra_strided(
                        scan0_ptr,
                        signed_pitch,
                        cx,
                        cy,
                        cw,
                        ch,
                        out.as_mut_ptr(),
                        dw,
                        dh,
                        &mut self.resample_lut,
                    );
                } else {
                    // Bounds-safe fallback copy if media buffer is shorter than expected
                    let src_x_bytes = cx * 4;
                    let row_bytes = cw * 4;
                    let mut tmp = vec![0u8; row_bytes * ch];
                    for y in 0..ch {
                        let src_y = cy + y;
                        let src_row_idx = if top_down {
                            src_y
                        } else {
                            coded_h.saturating_sub(1 + src_y)
                        };
                        let off = src_row_idx
                            .saturating_mul(abs_stride)
                            .saturating_add(src_x_bytes);
                        if off + row_bytes <= avail {
                            std::ptr::copy_nonoverlapping(
                                data_ptr.add(off),
                                tmp.as_mut_ptr().add(y * row_bytes),
                                row_bytes,
                            );
                        }
                    }
                    resample_bgra_strided(
                        tmp.as_ptr(),
                        row_bytes as isize,
                        0,
                        0,
                        cw,
                        ch,
                        out.as_mut_ptr(),
                        dw,
                        dh,
                        &mut self.resample_lut,
                    );
                }
                let _ = (buf.vtbl::<MfMediaBufferVtbl>().unlock)(buf.as_raw());
            }

            let wait_dur = self.compute_wait_duration(ts_100ns);
            self.frame_seq = self.frame_seq.wrapping_add(1);
            if self.frame_pool.len() < 3 {
                self.frame_pool.push(Arc::clone(&out_arc));
            }
            counters.cpu_decoded_frames.fetch_add(1, Ordering::Relaxed);
            if downscaled {
                counters.downscaled_frames.fetch_add(1, Ordering::Relaxed);
            }

            let snap = MotionFrameSnapshot {
                key: slot_key.to_string(),
                width: out_w,
                height: out_h,
                frame_seq: self.frame_seq,
                bgra: out_arc,
                dxgi_texture: None,
                target_dim: self.target_dim,
            };
            Ok((Some(snap), wait_dur))
        }
    }

    fn compute_wait_duration(&mut self, ts_100ns: i64) -> Duration {
        if let Some(first_ts) = self.first_sample_ts_100ns {
            let rel_ms = ((ts_100ns - first_ts).max(0) / 10_000) as u64;
            let target = Duration::from_millis(rel_ms);
            let elapsed = self.playback_epoch.elapsed();
            if target > elapsed {
                (target - elapsed).clamp(Duration::from_millis(33), Duration::from_millis(83))
            } else {
                if elapsed.saturating_sub(target) > Duration::from_millis(200) {
                    self.first_sample_ts_100ns = Some(ts_100ns);
                    self.playback_epoch = Instant::now();
                }
                Duration::from_millis(33)
            }
        } else {
            self.first_sample_ts_100ns = Some(ts_100ns);
            self.playback_epoch = Instant::now();
            Duration::from_millis(33)
        }
    }
}

struct ActiveDecoderSlot {
    url: String,
    decoder: Mp4VideoDecoder,
    next_due: Instant,
}

fn mark_worker_exited(shared: &Arc<(Mutex<WorkerSharedState>, Condvar)>) {
    if let Ok(mut guard) = shared.0.lock() {
        guard.worker_exited = true;
        shared.1.notify_all();
    }
}

struct InflightDownloadGuard {
    map: InflightDownloadMap,
    url: String,
    token: u64,
}

impl Drop for InflightDownloadGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = self.map.lock() {
            if m.get(&self.url).map(|(t, _)| *t) == Some(self.token) {
                m.remove(&self.url);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_hls_download(
    rt: &tokio::runtime::Runtime,
    http: &reqwest::Client,
    url: String,
    target_dim: u32,
    is_prefetch: bool,
    shared: &Arc<(Mutex<WorkerSharedState>, Condvar)>,
    inflight_downloads: &InflightDownloadMap,
    ready_paths: &Arc<Mutex<HashMap<String, PathBuf>>>,
    negative_disk_cache: &Arc<Mutex<HashMap<String, Instant>>>,
    counters: &Arc<WorkerTelemetryCounters>,
    dl_sem: &Arc<tokio::sync::Semaphore>,
    prefetch_sem: &Arc<tokio::sync::Semaphore>,
) {
    let Ok(mut dl_map) = inflight_downloads.lock() else {
        return;
    };
    if dl_map.contains_key(&url) {
        return;
    }

    let token = counters
        .next_download_token
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    let http_c = http.clone();
    let shared_c = Arc::clone(shared);
    let dl_map_c = Arc::clone(inflight_downloads);
    let ready_c = Arc::clone(ready_paths);
    let neg_c = Arc::clone(negative_disk_cache);
    let sem_c = Arc::clone(dl_sem);
    let psem_c = Arc::clone(prefetch_sem);
    let url_key = url.clone();

    let join_handle = rt.spawn(async move {
        let dl_guard = InflightDownloadGuard {
            map: dl_map_c,
            url: url.clone(),
            token,
        };
        // Reserve active download lanes so offscreen prefetches never starve visible cards
        let _prefetch_permit = if is_prefetch {
            psem_c.acquire().await.ok()
        } else {
            None
        };
        let _permit = sem_c.acquire().await;
        let res = ensure_cached_mp4_for_m3u8_async(&http_c, &url, target_dim, token).await;
        drop(dl_guard);
        match res {
            Ok(path) => {
                if let Ok(mut m) = ready_c.lock() {
                    m.insert(url.clone(), path);
                }
                if let Ok(mut neg) = neg_c.lock() {
                    neg.remove(&url);
                }
                if let Ok(mut guard) = shared_c.0.lock() {
                    guard.download_events_seq = guard.download_events_seq.wrapping_add(1);
                }
            }
            Err(_) => {
                if let Ok(mut guard) = shared_c.0.lock() {
                    guard.failed_urls.insert(url, Instant::now());
                    guard.download_events_seq = guard.download_events_seq.wrapping_add(1);
                }
            }
        }
        shared_c.1.notify_all();
    });
    dl_map.insert(url_key, (token, join_handle.abort_handle()));
}

fn motion_worker_loop(
    shared: Arc<(Mutex<WorkerSharedState>, Condvar)>,
    wake_hwnd: Arc<AtomicUsize>,
    ready_paths: Arc<Mutex<HashMap<String, PathBuf>>>,
    negative_disk_cache: Arc<Mutex<HashMap<String, Instant>>>,
    inflight_downloads: InflightDownloadMap,
    counters: Arc<WorkerTelemetryCounters>,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("am-motion-dl")
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(_) => {
            mark_worker_exited(&shared);
            unsafe {
                CoUninitialize();
            }
            return;
        }
    };
    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(18))
        .pool_max_idle_per_host(8)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            rt.shutdown_background();
            mark_worker_exited(&shared);
            unsafe {
                CoUninitialize();
            }
            return;
        }
    };

    let mut mf: Option<MfRuntime> = None;
    let mut decoders: HashMap<String, ActiveDecoderSlot> = HashMap::new();
    let dl_sem = Arc::new(tokio::sync::Semaphore::new(6));
    let prefetch_sem = Arc::new(tokio::sync::Semaphore::new(3));
    let mut global_tick_due = Instant::now();
    let mut decode_tick_seq: u64 = 0;

    loop {
        let (
            active_reqs,
            prefetch_reqs,
            dxva_bundle,
            dxva_gen,
            observed_slots_ver,
            observed_dl_seq,
        ): (
            Vec<(String, String, u32, usize)>,
            Vec<(String, u32)>,
            Option<DxvaDeviceBundle>,
            u64,
            u64,
            u64,
        ) = {
            let Ok(mut guard) = shared.0.lock() else {
                break;
            };
            loop {
                if guard.shutdown {
                    drop(guard);
                    decoders.clear();
                    drop(mf);
                    rt.shutdown_background();
                    mark_worker_exited(&shared);
                    unsafe {
                        CoUninitialize();
                    }
                    return;
                }
                let now = Instant::now();
                let expired_failed: Vec<String> = guard
                    .failed_urls
                    .iter()
                    .filter(|(_, failed_at)| {
                        now.duration_since(**failed_at) >= Duration::from_secs(120)
                    })
                    .map(|(u, _)| u.clone())
                    .collect();
                for u in expired_failed {
                    guard.failed_urls.remove(&u);
                    guard.prefetch_seen.remove(&u);
                }

                if guard.global_enabled {
                    let mut wanted: Vec<(String, String, u32, usize)> = guard
                        .slots
                        .iter()
                        .filter(|(_, req)| {
                            req.active
                                && !req.url.is_empty()
                                && !guard.failed_urls.contains_key(&req.url)
                        })
                        .map(|(k, req)| {
                            (k.clone(), req.url.clone(), req.target_dim, req.priority_rank)
                        })
                        .collect();
                    wanted.sort_by_key(|(_, _, _, rank)| *rank);
                    let prefetches: Vec<(String, u32)> =
                        std::mem::take(&mut guard.prefetch_queue);
                    if !wanted.is_empty() || !prefetches.is_empty() {
                        break (
                            wanted,
                            prefetches,
                            guard.dxva.clone(),
                            guard.dxva_generation,
                            guard.slots_version,
                            guard.download_events_seq,
                        );
                    }
                }
                // Release decoders when completely idle to minimize RAM/handles/DXVA sessions
                if !decoders.is_empty() {
                    decoders.clear();
                }

                // Pure Condvar::wait (0 idle wakeups/s!) unless an active slot is waiting on a
                // 120s failed_urls retry window.
                let next_failed_retry_wait: Option<Duration> = if guard.global_enabled {
                    guard
                        .slots
                        .values()
                        .filter(|r| r.active && !r.url.is_empty())
                        .filter_map(|r| guard.failed_urls.get(&r.url))
                        .map(|failed_at| {
                            Duration::from_secs(120)
                                .saturating_sub(now.duration_since(*failed_at))
                                .max(Duration::from_secs(1))
                        })
                        .min()
                } else {
                    None
                };

                let wait_res = if let Some(retry_wait) = next_failed_retry_wait {
                    shared.1.wait_timeout(guard, retry_wait).ok().map(|(g, _)| g)
                } else {
                    shared.1.wait(guard).ok()
                };
                if let Some(next_g) = wait_res {
                    guard = next_g;
                } else {
                    decoders.clear();
                    drop(mf);
                    rt.shutdown_background();
                    mark_worker_exited(&shared);
                    unsafe {
                        CoUninitialize();
                    }
                    return;
                }
            }
        };

        // Drop stale decoders immediately when a card scrolls out of viewport (or DXVA device changed)
        decoders.retain(|k, slot| {
            let still_active = active_reqs
                .iter()
                .any(|(rk, ru, _, _)| rk == k && ru == &slot.url);
            let same_dxva = slot.decoder.dxva_generation == dxva_gen;
            still_active && same_dxva
        });

        // Queue any scroll-active offscreen prefetches immediately (non-blocking, zero stat once indexed)
        let mut any_prefetch_pending = false;
        for (url, target_dim) in prefetch_reqs {
            let cached_hit = match disk_index_lookup_state_for_url(&url) {
                DiskIndexLookup::Hit(path) => {
                    if let Ok(mut m) = ready_paths.lock() {
                        m.entry(url.clone()).or_insert(path);
                    }
                    if let Ok(mut neg) = negative_disk_cache.lock() {
                        neg.remove(&url);
                    }
                    true
                }
                DiskIndexLookup::MissInitialized => false,
                DiskIndexLookup::NotYetInitialized => {
                    let in_ready = ready_paths
                        .lock()
                        .ok()
                        .map(|m| m.contains_key(&url))
                        .unwrap_or(false);
                    if in_ready {
                        true
                    } else if let Some((path, len)) = cached_mp4_path_and_len_if_ready(&url) {
                        disk_index_record_existing_file(&url, len);
                        if let Ok(mut m) = ready_paths.lock() {
                            m.insert(url.clone(), path);
                        }
                        if let Ok(mut neg) = negative_disk_cache.lock() {
                            neg.remove(&url);
                        }
                        true
                    } else {
                        false
                    }
                }
            };
            if cached_hit {
                continue;
            }
            let is_dl = inflight_downloads
                .lock()
                .map(|map| map.contains_key(&url))
                .unwrap_or(false);
            if is_dl {
                any_prefetch_pending = true;
                continue;
            }
            spawn_hls_download(
                &rt,
                &http,
                url,
                target_dim,
                true,
                &shared,
                &inflight_downloads,
                &ready_paths,
                &negative_disk_cache,
                &counters,
                &dl_sem,
                &prefetch_sem,
            );
            any_prefetch_pending = true;
        }

        if active_reqs.is_empty() {
            // Loop back to the top where pure Condvar::wait parks at 0 wakeups/s until
            // a download finishes or UI updates slots/prefetch.
            continue;
        }

        if mf.is_none() {
            match MfRuntime::load() {
                Ok(m) => mf = Some(m),
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }
        let Some(mf_ref) = mf.as_ref() else {
            continue;
        };

        let mut any_frame_updated = false;
        let total_active = active_reqs.len();
        let min_frame_pace = Duration::from_millis(cadence_ms_for_active_count(total_active));
        let tick_now = Instant::now();
        let tick_ready = tick_now >= global_tick_due;
        if tick_ready {
            decode_tick_seq = decode_tick_seq.wrapping_add(1);
        }
        let next_shared_due = if tick_ready {
            let next = global_tick_due + min_frame_pace;
            if next > tick_now {
                next
            } else {
                tick_now + min_frame_pace
            }
        } else {
            global_tick_due
        };

        let mut any_decoder_active = false;
        let mut any_download_pending = any_prefetch_pending;
        let mut opens_this_tick: usize = 0;
        let mut any_open_deferred = false;

        for (slot_key, url, target_dim, priority_rank) in active_reqs {
            let need_open = decoders
                .get(&slot_key)
                .map(|s| s.url != url)
                .unwrap_or(true);
            if need_open {
                let cached_path_opt = match disk_index_lookup_state_for_url(&url) {
                    DiskIndexLookup::Hit(path) => {
                        if let Ok(mut m) = ready_paths.lock() {
                            m.entry(url.clone()).or_insert_with(|| path.clone());
                        }
                        if let Ok(mut neg) = negative_disk_cache.lock() {
                            neg.remove(&url);
                        }
                        Some(path)
                    }
                    DiskIndexLookup::MissInitialized => {
                        if let Ok(mut m) = ready_paths.lock() {
                            m.remove(&url);
                        }
                        None
                    }
                    DiskIndexLookup::NotYetInitialized => ready_paths
                        .lock()
                        .ok()
                        .and_then(|m| m.get(&url).cloned())
                        .or_else(|| {
                            let is_dl = inflight_downloads
                                .lock()
                                .map(|map| map.contains_key(&url))
                                .unwrap_or(false);
                            if is_dl {
                                return None;
                            }
                            let found = cached_mp4_path_and_len_if_ready(&url);
                            if let Some((ref p, len)) = found {
                                disk_index_record_existing_file(&url, len);
                                if let Ok(mut m) = ready_paths.lock() {
                                    m.insert(url.clone(), p.clone());
                                }
                                if let Ok(mut neg) = negative_disk_cache.lock() {
                                    neg.remove(&url);
                                }
                            }
                            found.map(|(p, _)| p)
                        }),
                };

                if let Some(cached_path) = cached_path_opt {
                    // Cap synchronous MFSourceReader opens per tick so active slots never freeze 120-480ms!
                    if opens_this_tick >= MAX_DECODER_OPENS_PER_TICK
                        || (opens_this_tick > 0 && tick_now.elapsed() >= Duration::from_millis(24))
                    {
                        any_open_deferred = true;
                        continue;
                    }
                    opens_this_tick += 1;
                    disk_index_touch_url(&url);
                    match Mp4VideoDecoder::open(mf_ref, &cached_path, target_dim, dxva_bundle.as_ref())
                    {
                        Ok(dec) => {
                            decoders.insert(
                                slot_key.clone(),
                                ActiveDecoderSlot {
                                    url: url.clone(),
                                    decoder: dec,
                                    next_due: tick_now,
                                },
                            );
                        }
                        Err(_) => {
                            let _ = std::fs::remove_file(&cached_path);
                            disk_index_remove_url(&url);
                            if let Ok(mut m) = ready_paths.lock() {
                                m.remove(&url);
                            }
                            if let Ok(mut guard) = shared.0.lock() {
                                guard.failed_urls.insert(url, Instant::now());
                            }
                            continue;
                        }
                    }
                } else {
                    spawn_hls_download(
                        &rt,
                        &http,
                        url,
                        target_dim,
                        false,
                        &shared,
                        &inflight_downloads,
                        &ready_paths,
                        &negative_disk_cache,
                        &counters,
                        &dl_sem,
                        &prefetch_sem,
                    );
                    any_download_pending = true;
                    continue;
                }
            }

            if let Some(slot_dec) = decoders.get_mut(&slot_key) {
                any_decoder_active = true;
                slot_dec.decoder.set_target_dim(target_dim);
                let is_first_frame = slot_dec.decoder.frame_seq == 0;
                if !is_first_frame {
                    if !tick_ready && tick_now < slot_dec.next_due {
                        continue;
                    }
                    let stride = decode_tick_stride_for_slot(priority_rank, total_active);
                    if stride > 1
                        && tick_ready
                        && ((decode_tick_seq.wrapping_add((priority_rank as u64) & 1)) % stride)
                            != 0
                    {
                        slot_dec.next_due = next_shared_due;
                        continue;
                    }
                }
                match slot_dec.decoder.read_next_frame(
                    mf_ref,
                    &slot_key,
                    dxva_bundle.as_ref(),
                    &counters,
                ) {
                    Ok((Some(snap), _wait)) => {
                        slot_dec.next_due = next_shared_due;
                        if let Ok(mut guard) = shared.0.lock() {
                            if guard
                                .slots
                                .get(&slot_key)
                                .map(|r| r.active && r.url == url)
                                .unwrap_or(false)
                            {
                                guard.frames.insert(slot_key.clone(), snap);
                                counters.global_frame_seq.fetch_add(1, Ordering::AcqRel);
                                any_frame_updated = true;
                            }
                        }
                    }
                    Ok((None, _wait)) => {
                        slot_dec.next_due = next_shared_due;
                    }
                    Err(_) => {
                        decoders.remove(&slot_key);
                        if let Ok(mut guard) = shared.0.lock() {
                            guard.failed_urls.insert(url, Instant::now());
                        }
                    }
                }
            }
        }

        if tick_ready && any_decoder_active {
            let now_after = Instant::now();
            global_tick_due = if next_shared_due > now_after {
                next_shared_due
            } else {
                now_after + min_frame_pace
            };
        }

        if any_frame_updated {
            let raw_hwnd = wake_hwnd.load(Ordering::Acquire);
            if raw_hwnd != 0 {
                unsafe {
                    let hw = HWND(raw_hwnd as *mut c_void);
                    let _ = PostMessageW(
                        Some(hw),
                        crate::audio_viz::WM_APP_AUDIO_WAKE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }

        if any_open_deferred {
            let wait_dur = if any_decoder_active {
                global_tick_due
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_millis(2), Duration::from_millis(8))
            } else {
                Duration::from_millis(4)
            };
            if let Ok(guard) = shared.0.lock() {
                if !guard.shutdown && guard.prefetch_queue.is_empty() {
                    let _ = shared.1.wait_timeout(guard, wait_dur);
                }
            }
        } else if any_decoder_active {
            let sleep_dur = global_tick_due
                .saturating_duration_since(Instant::now())
                .clamp(Duration::from_millis(2), min_frame_pace);
            if let Ok(guard) = shared.0.lock() {
                if !guard.shutdown && guard.prefetch_queue.is_empty() {
                    let _ = shared.1.wait_timeout(guard, sleep_dur);
                }
            }
        } else if any_download_pending {
            // Pure Condvar::wait (0 wakeups/s) until Tokio download completes or slots change
            if let Ok(guard) = shared.0.lock() {
                if !guard.shutdown
                    && guard.prefetch_queue.is_empty()
                    && guard.slots_version == observed_slots_ver
                    && guard.download_events_seq == observed_dl_seq
                {
                    drop(shared.1.wait(guard));
                }
            }
        }
    }

    decoders.clear();
    drop(mf);
    rt.shutdown_background();
    mark_worker_exited(&shared);
    unsafe {
        CoUninitialize();
    }
}
