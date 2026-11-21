use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::{s, w, GUID, HRESULT, PCSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Clone, Debug)]
pub struct MotionFrameSnapshot {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub frame_seq: u64,
    pub bgra: Arc<Vec<u8>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SlotRequest {
    url: String,
    active: bool,
}

#[derive(Default)]
struct WorkerSharedState {
    slots: HashMap<String, SlotRequest>,
    frames: HashMap<String, MotionFrameSnapshot>,
    failed_urls: HashMap<String, Instant>,
    global_enabled: bool,
    shutdown: bool,
    worker_exited: bool,
}

pub struct MotionArtManager {
    state: Arc<(Mutex<WorkerSharedState>, Condvar)>,
    wake_hwnd: Arc<AtomicUsize>,
    worker_started: AtomicBool,
}

impl Default for MotionArtManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionArtManager {
    pub fn new() -> Self {
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
        }
    }

    pub fn set_wake_hwnd(&self, hwnd: usize) {
        self.wake_hwnd.store(hwnd, Ordering::Release);
    }

    fn ensure_worker_started(&self) {
        if self
            .worker_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let shared = Arc::clone(&self.state);
            let wake_hwnd = Arc::clone(&self.wake_hwnd);
            let _ = std::thread::Builder::new()
                .name("am-motion-art".to_string())
                .spawn(move || motion_worker_loop(shared, wake_hwnd));
        }
    }

    pub fn set_global_enabled(&self, enabled: bool) {
        if let Ok(mut guard) = self.state.0.lock() {
            if guard.global_enabled != enabled {
                guard.global_enabled = enabled;
                if !enabled {
                    for req in guard.slots.values_mut() {
                        req.active = false;
                    }
                    guard.frames.clear();
                }
                self.state.1.notify_all();
            }
        }
    }

    pub fn slot_key_for_url(url: &str) -> String {
        format!("u_{}", hash_url_key(url.trim()))
    }

    /// Synchronizes all viewport-visible URL-keyed motion cover slots (`u_*`) in one lock
    /// acquisition:
    /// - Every currently visible cover plays simultaneously (up to 12 slots).
    /// - Existing visible URLs NEVER change slot key and NEVER wipe their decoded frame -> zero flicker!
    /// - Any cover that scrolled out of the viewport is immediately removed and its decoder stopped!
    pub fn sync_visible_urls(&self, visible_urls: &[String]) {
        const MAX_VIEWPORT_SLOTS: usize = 12;
        let mut desired: Vec<(String, String)> = Vec::new();
        let mut desired_keys: HashSet<String> = HashSet::new();
        for raw in visible_urls {
            let clean = raw.trim();
            if clean.is_empty() {
                continue;
            }
            let key = Self::slot_key_for_url(clean);
            if desired_keys.insert(key.clone()) {
                desired.push((key, clean.to_string()));
                if desired.len() >= MAX_VIEWPORT_SLOTS {
                    break;
                }
            }
        }

        let mut need_worker = false;
        if let Ok(mut guard) = self.state.0.lock() {
            let mut changed = false;
            let stale_keys: Vec<String> = guard
                .slots
                .keys()
                .filter(|k| k.starts_with("u_") && !desired_keys.contains(*k))
                .cloned()
                .collect();
            for sk in stale_keys {
                guard.slots.remove(&sk);
                guard.frames.remove(&sk);
                changed = true;
            }

            let effective_active = guard.global_enabled;
            for (k, url) in desired {
                let next_req = SlotRequest {
                    url,
                    active: effective_active,
                };
                let prev = guard.slots.get(&k);
                if prev != Some(&next_req) {
                    if prev.map(|p| &p.url) != Some(&next_req.url) {
                        guard.frames.remove(&k);
                    }
                    guard.slots.insert(k, next_req);
                    changed = true;
                }
                if effective_active {
                    need_worker = true;
                }
            }

            if changed {
                self.state.1.notify_all();
            }
        }
        if need_worker {
            self.ensure_worker_started();
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
        let clean_url = m3u8_url.map(str::trim).unwrap_or("").to_string();
        if !clean_url.is_empty() && active {
            self.ensure_worker_started();
        }
        if let Ok(mut guard) = self.state.0.lock() {
            if clean_url.is_empty() {
                let changed = guard.slots.remove(slot_key).is_some()
                    || guard.frames.remove(slot_key).is_some();
                if changed {
                    self.state.1.notify_all();
                }
                return;
            }
            let effective_active = active && guard.global_enabled;
            let next_req = SlotRequest {
                url: clean_url,
                active: effective_active,
            };
            let prev = guard.slots.get(slot_key).cloned();
            if prev.as_ref() != Some(&next_req) {
                if prev.as_ref().map(|p| &p.url) != Some(&next_req.url) {
                    guard.frames.remove(slot_key);
                }
                guard.slots.insert(slot_key.to_string(), next_req);
                self.state.1.notify_all();
            }
        }
    }

    pub fn latest_frame(&self, slot_key: &str, expected_url: Option<&str>) -> Option<MotionFrameSnapshot> {
        let guard = self.state.0.lock().ok()?;
        if !guard.global_enabled {
            return None;
        }
        let req = guard.slots.get(slot_key)?;
        if !req.active {
            return None;
        }
        if let Some(exp) = expected_url {
            if req.url != exp.trim() {
                return None;
            }
        }
        guard.frames.get(slot_key).cloned()
    }
}

impl Drop for MotionArtManager {
    fn drop(&mut self) {
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
                        Err(_) => break,
                    }
                }
            }
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
    let lines: Vec<&str> = m3u8.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
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
            !codecs_low.contains("hvc1") && !codecs_low.contains("hev1") && !codecs_low.contains("av01")
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
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| dirs_fallback_local_appdata())?;
    let dir = local.join("AM-Light").join("cache").join("motion_art");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

fn dirs_fallback_local_appdata() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("AppData").join("Local"))
}

fn hash_url_key(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn prune_motion_cache_dir(dir: &Path) {
    const MAX_FILES: usize = 48;
    const MAX_BYTES: u64 = 96 * 1024 * 1024;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let mut total_bytes = 0u64;
    for item in rd.flatten() {
        let path = item.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = item.metadata() else {
            continue;
        };
        let len = meta.len();
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        total_bytes = total_bytes.saturating_add(len);
        entries.push((path, len, mtime));
    }
    if entries.len() <= MAX_FILES && total_bytes <= MAX_BYTES {
        return;
    }
    entries.sort_by_key(|(_, _, mt)| *mt);
    let mut count = entries.len();
    for (path, len, _) in entries {
        if count <= MAX_FILES && total_bytes <= MAX_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            count = count.saturating_sub(1);
            total_bytes = total_bytes.saturating_sub(len);
        }
    }
}

fn cached_mp4_path_if_ready(m3u8_url: &str) -> Option<PathBuf> {
    let dir = motion_cache_dir()?;
    let key = hash_url_key(m3u8_url);
    let out_path = dir.join(format!("{key}.mp4"));
    let meta = std::fs::metadata(&out_path).ok()?;
    if meta.is_file() && meta.len() > 8192 {
        Some(out_path)
    } else {
        None
    }
}

async fn ensure_cached_mp4_for_m3u8_async(
    http: &reqwest::Client,
    m3u8_url: &str,
) -> Result<PathBuf, String> {
    let dir = motion_cache_dir().ok_or_else(|| "无法定位本地缓存目录".to_string())?;
    let key = hash_url_key(m3u8_url);
    let out_path = dir.join(format!("{key}.mp4"));
    if let Ok(meta) = std::fs::metadata(&out_path) {
        if meta.is_file() && meta.len() > 8192 {
            return Ok(out_path);
        }
    }

    let tmp_path = dir.join(format!("{key}.mp4.part"));
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
        let v_url = parse_best_avc_variant_from_master(&master_text, m3u8_url)
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
        && segments.first().map(|s| Some(s) != init_url.as_ref()).unwrap_or(false);

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

    let out_clone = out_path.clone();
    let dir_clone = dir.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::write(&tmp_path, &combined)
            .map_err(|e| format!("写入缓存文件失败: {e}"))?;
        let _ = std::fs::remove_file(&out_clone);
        std::fs::rename(&tmp_path, &out_clone)
            .map_err(|e| format!("提交缓存文件失败: {e}"))?;
        prune_motion_cache_dir(&dir_clone);
        Ok::<PathBuf, String>(out_clone)
    })
    .await
    .map_err(|e| format!("缓存写盘线程异常: {e}"))?
}

// ============================================================================
// Pure Win32 Dynamic Media Foundation H.264 -> RGB32 (BGRA) Decoder
// ============================================================================

const MF_VERSION: u32 = 0x0002_0070;
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
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
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
    set_unknown: *const c_void,
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
    get_buffer_count: *const c_void,
    get_buffer_by_index: *const c_void,
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
            let fn_shutdown: FnMfShutdown = load_proc(mfplat, s!("MFShutdown")).ok_or_else(|| {
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

            let hr = fn_startup(MF_VERSION, 0);
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

struct Mp4VideoDecoder {
    reader: ComPtr,
    coded_width: u32,
    coded_height: u32,
    initial_display_w: u32,
    initial_display_h: u32,
    crop_x: u32,
    crop_y: u32,
    crop_w: u32,
    crop_h: u32,
    stride: i32,
    mp4_path: PathBuf,
    first_sample_ts_100ns: Option<i64>,
    playback_epoch: Instant,
    frame_seq: u64,
}

impl Mp4VideoDecoder {
    fn open(mf: &MfRuntime, mp4_path: &Path) -> Result<Self, String> {
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
            let hr = (mf.fn_create_source_reader)(wide.as_mut_ptr(), attrs.as_raw(), &mut reader_raw);
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
                return Err(format!("SetCurrentMediaType(RGB32) 失败: 0x{:08X}", hr.0 as u32));
            }

            let mut dec = Self {
                reader,
                coded_width: 0,
                coded_height: 0,
                initial_display_w: 0,
                initial_display_h: 0,
                crop_x: 0,
                crop_y: 0,
                crop_w: 0,
                crop_h: 0,
                stride: 0,
                mp4_path: mp4_path.to_path_buf(),
                first_sample_ts_100ns: None,
                playback_epoch: Instant::now(),
                frame_seq: 0,
            };
            dec.refresh_media_geometry()?;
            dec.initial_display_w = dec.crop_w;
            dec.initial_display_h = dec.crop_h;
            Ok(dec)
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
                return Err(format!("读取 MF_MT_FRAME_SIZE 失败: 0x{:08X}", hr_sz.0 as u32));
            }
            let w = (size_packed >> 32) as u32;
            let h = (size_packed & 0xFFFF_FFFF) as u32;
            if w == 0 || h == 0 || w > 1920 || h > 1920 {
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
            let stride = if (cur.vtbl::<MfAttributesVtbl>().get_uint32)(
                cur.as_raw(),
                &MF_MT_DEFAULT_STRIDE,
                &mut stride_u32,
            )
            .is_ok()
            {
                stride_u32 as i32
            } else {
                (w * 4) as i32
            };
            self.coded_width = w;
            self.coded_height = h;
            self.crop_x = cx;
            self.crop_y = cy;
            self.crop_w = cw;
            self.crop_h = ch;
            self.stride = if stride == 0 { (w * 4) as i32 } else { stride };
            Ok(())
        }
    }

    fn rewind(&mut self, mf: &MfRuntime) -> Result<(), String> {
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
                let reopened = Self::open(mf, &self.mp4_path)?;
                self.reader = reopened.reader;
                self.coded_width = reopened.coded_width;
                self.coded_height = reopened.coded_height;
                self.initial_display_w = reopened.initial_display_w;
                self.initial_display_h = reopened.initial_display_h;
                self.crop_x = reopened.crop_x;
                self.crop_y = reopened.crop_y;
                self.crop_w = reopened.crop_w;
                self.crop_h = reopened.crop_h;
                self.stride = reopened.stride;
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
    ) -> Result<(Option<MotionFrameSnapshot>, Duration), String> {
        unsafe {
            let mut actual_idx: u32 = 0;
            let mut flags: u32 = 0;
            let mut ts_100ns: i64 = 0;
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
            let _sample_guard = ComPtr::from_raw_opt(sample_raw);

            if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) != 0 {
                let _ = self.refresh_media_geometry();
            }
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM) != 0 || _sample_guard.is_none() {
                self.rewind(mf)?;
                return Ok((None, Duration::from_millis(8)));
            }
            let sample = _sample_guard.as_ref().unwrap();

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
            let w = self.crop_w as usize;
            let h = self.crop_h as usize;
            let row_bytes = w * 4;
            let src_x_bytes = cx * 4;
            let min_src_row_bytes = (cx + w) * 4;
            let mut out = vec![0u8; row_bytes * h];

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
                        for y in 0..h {
                            let src_y = cy + y;
                            let src_row_ptr = if pitch >= 0 {
                                scan0.add(src_y * pitch as usize)
                            } else {
                                scan0.offset((src_y as isize) * (pitch as isize))
                            };
                            let dst_row = &mut out[y * row_bytes..(y + 1) * row_bytes];
                            std::ptr::copy_nonoverlapping(
                                src_row_ptr.add(src_x_bytes),
                                dst_row.as_mut_ptr(),
                                row_bytes,
                            );
                        }
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
                for y in 0..h {
                    let src_y = cy + y;
                    let src_row_idx = if top_down {
                        src_y
                    } else {
                        coded_h.saturating_sub(1 + src_y)
                    };
                    let off = src_row_idx.saturating_mul(abs_stride).saturating_add(src_x_bytes);
                    if off + row_bytes <= avail {
                        let dst_row = &mut out[y * row_bytes..(y + 1) * row_bytes];
                        std::ptr::copy_nonoverlapping(
                            data_ptr.add(off),
                            dst_row.as_mut_ptr(),
                            row_bytes,
                        );
                    }
                }
                let _ = (buf.vtbl::<MfMediaBufferVtbl>().unlock)(buf.as_raw());
            }

            let wait_dur = if let Some(first_ts) = self.first_sample_ts_100ns {
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
            };

            self.frame_seq = self.frame_seq.wrapping_add(1);
            let snap = MotionFrameSnapshot {
                key: slot_key.to_string(),
                width: self.crop_w,
                height: self.crop_h,
                frame_seq: self.frame_seq,
                bgra: Arc::new(out),
            };
            Ok((Some(snap), wait_dur))
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

fn motion_worker_loop(
    shared: Arc<(Mutex<WorkerSharedState>, Condvar)>,
    wake_hwnd: Arc<AtomicUsize>,
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
            mark_worker_exited(&shared);
            unsafe {
                CoUninitialize();
            }
            return;
        }
    };

    let mut mf: Option<MfRuntime> = None;
    let mut decoders: HashMap<String, ActiveDecoderSlot> = HashMap::new();
    let downloading_urls: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let dl_sem = Arc::new(tokio::sync::Semaphore::new(4));

    loop {
        let active_reqs: Vec<(String, String)> = {
            let Ok(mut guard) = shared.0.lock() else {
                break;
            };
            loop {
                if guard.shutdown {
                    drop(guard);
                    decoders.clear();
                    drop(mf);
                    mark_worker_exited(&shared);
                    unsafe {
                        CoUninitialize();
                    }
                    return;
                }
                let now = Instant::now();
                guard
                    .failed_urls
                    .retain(|_, failed_at| now.duration_since(*failed_at) < Duration::from_secs(120));

                if guard.global_enabled {
                    let wanted: Vec<(String, String)> = guard
                        .slots
                        .iter()
                        .filter(|(_, req)| {
                            req.active
                                && !req.url.is_empty()
                                && !guard.failed_urls.contains_key(&req.url)
                        })
                        .map(|(k, req)| (k.clone(), req.url.clone()))
                        .collect();
                    if !wanted.is_empty() {
                        break wanted;
                    }
                }
                // Release decoders and MF runtime when completely idle to minimize RAM/handles
                if !decoders.is_empty() {
                    decoders.clear();
                }
                if let Ok(next_g) = shared.1.wait_timeout(guard, Duration::from_millis(600)) {
                    guard = next_g.0;
                } else {
                    drop(decoders);
                    drop(mf);
                    mark_worker_exited(&shared);
                    unsafe {
                        CoUninitialize();
                    }
                    return;
                }
            }
        };

        // Drop stale decoders immediately when a card scrolls out of viewport!
        decoders.retain(|k, slot| {
            active_reqs
                .iter()
                .any(|(rk, ru)| rk == k && ru == &slot.url)
        });

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
        let mut next_wake: Option<Instant> = None;
        let total_active = active_reqs.len();
        // Smooth adaptive pacing: 30fps for hero/<=4 covers, 24fps when 5+ cards play simultaneously
        let min_frame_pace = if total_active >= 5 {
            Duration::from_millis(42)
        } else {
            Duration::from_millis(33)
        };

        for (slot_key, url) in active_reqs {
            let need_open = decoders
                .get(&slot_key)
                .map(|s| s.url != url)
                .unwrap_or(true);
            if need_open {
                if let Some(cached_path) = cached_mp4_path_if_ready(&url) {
                    match Mp4VideoDecoder::open(mf_ref, &cached_path) {
                        Ok(dec) => {
                            decoders.insert(
                                slot_key.clone(),
                                ActiveDecoderSlot {
                                    url: url.clone(),
                                    decoder: dec,
                                    next_due: Instant::now(),
                                },
                            );
                        }
                        Err(_) => {
                            let _ = std::fs::remove_file(&cached_path);
                            if let Ok(mut guard) = shared.0.lock() {
                                guard.failed_urls.insert(url, Instant::now());
                            }
                            continue;
                        }
                    }
                } else {
                    // Non-blocking concurrent HLS download so playing covers NEVER stutter!
                    let should_spawn = downloading_urls
                        .lock()
                        .map(|mut set| set.insert(url.clone()))
                        .unwrap_or(false);
                    if should_spawn {
                        let http_c = http.clone();
                        let url_c = url.clone();
                        let shared_c = Arc::clone(&shared);
                        let dl_set = Arc::clone(&downloading_urls);
                        let sem_c = Arc::clone(&dl_sem);
                        rt.spawn(async move {
                            let _permit = sem_c.acquire().await;
                            let res = ensure_cached_mp4_for_m3u8_async(&http_c, &url_c).await;
                            if let Ok(mut set) = dl_set.lock() {
                                set.remove(&url_c);
                            }
                            if res.is_err() {
                                if let Ok(mut guard) = shared_c.0.lock() {
                                    guard.failed_urls.insert(url_c, Instant::now());
                                }
                            }
                            shared_c.1.notify_all();
                        });
                    }
                    let retry_due = Instant::now() + Duration::from_millis(90);
                    next_wake = Some(match next_wake {
                        Some(w) => w.min(retry_due),
                        None => retry_due,
                    });
                    continue;
                }
            }

            if let Some(slot_dec) = decoders.get_mut(&slot_key) {
                let now = Instant::now();
                if now < slot_dec.next_due {
                    next_wake = Some(match next_wake {
                        Some(w) => w.min(slot_dec.next_due),
                        None => slot_dec.next_due,
                    });
                    continue;
                }
                match slot_dec.decoder.read_next_frame(mf_ref, &slot_key) {
                    Ok((Some(snap), wait)) => {
                        let due = Instant::now() + wait.max(min_frame_pace);
                        slot_dec.next_due = due;
                        next_wake = Some(match next_wake {
                            Some(w) => w.min(due),
                            None => due,
                        });
                        if let Ok(mut guard) = shared.0.lock() {
                            if guard
                                .slots
                                .get(&slot_key)
                                .map(|r| r.active && r.url == url)
                                .unwrap_or(false)
                            {
                                guard.frames.insert(slot_key.clone(), snap);
                                any_frame_updated = true;
                            }
                        }
                    }
                    Ok((None, wait)) => {
                        let due = Instant::now() + wait.max(min_frame_pace);
                        slot_dec.next_due = due;
                        next_wake = Some(match next_wake {
                            Some(w) => w.min(due),
                            None => due,
                        });
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

        let sleep_dur = next_wake
            .map(|w| w.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|| Duration::from_millis(33))
            .clamp(Duration::from_millis(8), Duration::from_millis(83));

        if let Ok(guard) = shared.0.lock() {
            if !guard.shutdown {
                let _ = shared.1.wait_timeout(guard, sleep_dur);
            }
        }
    }

    decoders.clear();
    drop(mf);
    mark_worker_exited(&shared);
    unsafe {
        CoUninitialize();
    }
}

