use base64::{engine::general_purpose, Engine as _};
use once_cell::sync::Lazy;
use percent_encoding::percent_decode_str;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AMP_API_BASE: &str = "https://amp-api.music.apple.com/";
const WEB_ORIGIN: &str = "https://music.apple.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const OFFICIAL_AM_PFN: &str = "AppleInc.AppleMusicWin_nzyj5cx40ttqa";

static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(24)
        .tcp_nodelay(true)
        .user_agent(USER_AGENT)
        .build()
        .expect("failed to build reqwest client")
});

static HERO_ART_SEM: Lazy<tokio::sync::Semaphore> =
    Lazy::new(|| tokio::sync::Semaphore::new(2));

static ARTWORK_SEM: Lazy<tokio::sync::Semaphore> =
    Lazy::new(|| tokio::sync::Semaphore::new(6));

/// Bounded concurrency semaphore for `art_cache` NVMe/SATA reads so fast scrolling never
/// floods Tokio's blocking thread pool with 50-140 simultaneous NTFS `fs::read` calls.
static ART_DISK_READ_SEM: Lazy<tokio::sync::Semaphore> =
    Lazy::new(|| tokio::sync::Semaphore::new(8));

pub const ART_CACHE_MAX_FILES: usize = 640;
pub const ART_CACHE_TARGET_FILES: usize = 440;
pub const ART_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const ART_CACHE_TARGET_BYTES: u64 = 48 * 1024 * 1024;
const ART_CACHE_PRUNE_WRITE_INTERVAL: u64 = 100;

static ART_CACHE_WRITE_COUNT: AtomicU64 = AtomicU64::new(0);
static ART_CACHE_PRUNE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static ART_TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Atomically writes artwork bytes into `target` via a unique sibling `.tmp` file + `rename`
/// so readers never observe truncated/half-written covers.
pub fn write_artwork_cache_file_atomic(
    target: &std::path::Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    let seq = ART_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = target.with_extension(format!("img.{seq}.tmp"));
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    match std::fs::rename(&tmp, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(target);
            match std::fs::rename(&tmp, target) {
                Ok(()) => Ok(()),
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    Err(e)
                }
            }
        }
    }
}

/// Single-pass mtime-LRU pruner for `art_cache` enforcing BOTH file-count and total-byte caps
/// (`max_files` -> `target_files`, `max_bytes` -> `target_bytes`) while cleaning stale `.tmp` files.
/// Returns the number of deleted files.
pub fn prune_art_cache_dir(
    dir: &std::path::Path,
    max_files: usize,
    target_files: usize,
    max_bytes: u64,
    target_bytes: u64,
) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut entries: Vec<(std::path::PathBuf, SystemTime, u64)> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut removed = 0usize;

    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let path = e.path();
        let mt = meta.modified().unwrap_or(UNIX_EPOCH);
        if path.extension().and_then(|x| x.to_str()) == Some("tmp") {
            let age_ok = now
                .duration_since(mt)
                .map(|d| d >= Duration::from_secs(60))
                .unwrap_or(false);
            if age_ok && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
            continue;
        }
        let len = meta.len();
        total_bytes = total_bytes.saturating_add(len);
        entries.push((path, mt, len));
    }

    if entries.len() > max_files || total_bytes > max_bytes {
        entries.sort_by_key(|(_, mt, _)| *mt);
        let mut remaining_files = entries.len();
        let mut remaining_bytes = total_bytes;
        for (p, _, len) in entries {
            if remaining_files <= target_files && remaining_bytes <= target_bytes {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                removed += 1;
            }
            remaining_files = remaining_files.saturating_sub(1);
            remaining_bytes = remaining_bytes.saturating_sub(len);
        }
    }

    removed
}

fn maybe_trigger_runtime_art_cache_prune(dir: std::path::PathBuf) {
    let writes = ART_CACHE_WRITE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if writes % ART_CACHE_PRUNE_WRITE_INTERVAL != 0 {
        return;
    }
    if ART_CACHE_PRUNE_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tokio::task::spawn_blocking(move || {
        struct PruneGuard;
        impl Drop for PruneGuard {
            fn drop(&mut self) {
                ART_CACHE_PRUNE_IN_FLIGHT.store(false, Ordering::Release);
            }
        }
        let _guard = PruneGuard;
        let _ = prune_art_cache_dir(
            &dir,
            ART_CACHE_MAX_FILES,
            ART_CACHE_TARGET_FILES,
            ART_CACHE_MAX_BYTES,
            ART_CACHE_TARGET_BYTES,
        );
    });
}

static ART_DISK_DIR: Lazy<Option<std::path::PathBuf>> = Lazy::new(|| {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(std::path::PathBuf::from)?;
    let dir = base.join("AM-Light").join("art_cache");
    let _ = std::fs::create_dir_all(&dir);
    // Prune disk cache asynchronously off the Tokio worker thread using true mtime LRU!
    let prune_dir = dir.clone();
    let _ = std::thread::Builder::new()
        .name("am-art-cache-prune".to_string())
        .spawn(move || {
            let _ = prune_art_cache_dir(
                &prune_dir,
                ART_CACHE_MAX_FILES,
                ART_CACHE_TARGET_FILES,
                ART_CACHE_MAX_BYTES,
                ART_CACHE_TARGET_BYTES,
            );
        });
    Some(dir)
});

fn fnv1a_64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Default maximum number of pages to fetch for Apple Music playlists (100 pages * 100/page = 10,000 tracks).
pub const DEFAULT_PLAYLIST_MAX_PAGES: usize = 100;
/// Safe batch size for Apple Music library playlist write endpoints (`POST /v1/me/library/playlists` and `/tracks`).
pub const DEFAULT_LIBRARY_WRITE_BATCH: usize = 50;

fn is_mzstatic_size_tail(tail: &str) -> bool {
    let Some((w_str, rest)) = tail.split_once('x') else {
        return false;
    };
    if w_str.is_empty() || w_str.len() > 4 || !w_str.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let h_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if h_len == 0 || h_len > 4 {
        return false;
    }
    let h_str = &rest[..h_len];
    let suffix = &rest[h_len..];
    if suffix.is_empty() || !suffix.contains('.') {
        return false;
    }
    let (Ok(w), Ok(h)) = (w_str.parse::<u32>(), h_str.parse::<u32>()) else {
        return false;
    };
    (16..=8192).contains(&w) && (16..=8192).contains(&h)
}

/// Zero-allocation `&str` variant of [`artwork_canonical_stem`]: normalizes an Apple Music / mzstatic
/// artwork URL across sizes (`120x120`, `240x240`, `360x360`, `600x600`) to a single canonical
/// identity stem with ZERO heap allocation.
/// Strictly preserves `smtc://` keys, signed/generated query URLs (e.g. `.../gen/1200x1200AM.*.jpg?...`),
/// and non-dimension CDN filenames (e.g. NetEase/QQ numeric IDs) so distinct covers NEVER collide.
pub fn artwork_canonical_stem_ref(url: &str) -> &str {
    let t = url.trim();
    if t.starts_with("smtc://") || (!t.starts_with("http://") && !t.starts_with("https://")) {
        return t;
    }
    // Query-bearing URLs (including Apple's `/gen/1200x1200AM.*.jpg?c1=...&t=...` generated playlist art
    // and signed blobstore URLs) distinguish artwork by query params and must NEVER be collapsed.
    if t.contains('?') {
        return t;
    }
    if let Some(slash) = t.rfind('/') {
        let tail = &t[slash + 1..];
        if is_mzstatic_size_tail(tail) {
            return &t[..slash];
        }
    }
    t
}

/// Owned `String` wrapper over [`artwork_canonical_stem_ref`] for backwards compatibility.
pub fn artwork_canonical_stem(url: &str) -> String {
    artwork_canonical_stem_ref(url).to_string()
}

/// Fetches album/playlist artwork bytes with bounded disk-read concurrency (`ART_DISK_READ_SEM(8)`),
/// atomic `.tmp -> rename` disk caching, periodic runtime LRU pruning (`640 files / 64 MiB`),
/// and immediate post-semaphore `art_epoch` cancellation checks to eliminate HOL blocking and
/// stale off-route downloads after page navigation.
/// When `epoch_guard` is `Some((epoch_atomic, expected_epoch))` and the atomic has moved past
/// `expected_epoch`, aborts immediately with `Err("CANCELLED".to_string())`.
pub async fn fetch_artwork_bytes_if_current(
    url: &str,
    high_priority: bool,
    epoch_guard: Option<(&AtomicU64, u64)>,
) -> Result<Vec<u8>, String> {
    let is_stale = || {
        epoch_guard.is_some_and(|(ep, want)| ep.load(Ordering::Acquire) != want)
    };
    if is_stale() {
        return Err("CANCELLED".to_string());
    }

    let disk_path = ART_DISK_DIR
        .as_ref()
        .map(|d| d.join(format!("{:016x}.img", fnv1a_64(url))));

    // Check local NVMe disk cache FIRST behind bounded disk-read semaphore — NEVER block cache hits
    // behind slow network semaphore slots, and NEVER flood blocking pool with 100+ concurrent NTFS reads!
    if let Some(ref p) = disk_path {
        let disk_permit = ART_DISK_READ_SEM
            .acquire()
            .await
            .map_err(|e| format!("disk semaphore closed: {e}"))?;
        if is_stale() {
            return Err("CANCELLED".to_string());
        }
        let p_read = p.clone();
        let read_res = tokio::task::spawn_blocking(move || std::fs::read(p_read)).await;
        drop(disk_permit);
        if let Ok(Ok(cached)) = read_res {
            if cached.len() >= 16 {
                if is_stale() {
                    return Err("CANCELLED".to_string());
                }
                return Ok(cached);
            }
        }
    }

    if is_stale() {
        return Err("CANCELLED".to_string());
    }

    let sem = if high_priority {
        &*HERO_ART_SEM
    } else {
        &*ARTWORK_SEM
    };
    let _permit = sem
        .acquire()
        .await
        .map_err(|e| format!("semaphore closed: {e}"))?;

    // CRITICAL (P1-2): Re-check `art_epoch` IMMEDIATELY AFTER acquiring `ARTWORK_SEM(6)`!
    // Tasks queued before a page navigation drop their permit in <1us with ZERO HTTP traffic!
    if is_stale() {
        return Err("CANCELLED".to_string());
    }

    let referer = if url.contains("126.net") || url.contains("163.com") {
        "https://music.163.com/".to_string()
    } else if url.contains("gtimg.cn") || url.contains("qq.com") {
        "https://y.qq.com/".to_string()
    } else {
        format!("{WEB_ORIGIN}/")
    };

    for attempt in 0..2 {
        if is_stale() {
            return Err("CANCELLED".to_string());
        }
        let res = HTTP_CLIENT
            .get(url)
            .timeout(Duration::from_secs(8))
            .header("Referer", &referer)
            .send()
            .await;
        match res {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| format!("读取封面字节失败: {e}"))?;
                if bytes.len() >= 16 {
                    if let Some(p) = disk_path.clone() {
                        let wb = bytes.clone();
                        let dir_opt = ART_DISK_DIR.clone();
                        tokio::task::spawn_blocking(move || {
                            let _ = write_artwork_cache_file_atomic(&p, &wb);
                        });
                        if let Some(d) = dir_opt {
                            maybe_trigger_runtime_art_cache_prune(d);
                        }
                    }
                    if is_stale() {
                        return Err("CANCELLED".to_string());
                    }
                    return Ok(bytes.to_vec());
                }
            }
            Ok(resp) if resp.status().as_u16() == 404 => {
                return Err("封面 HTTP 404".to_string());
            }
            _ => {
                if attempt == 0 {
                    tokio::time::sleep(Duration::from_millis(180)).await;
                }
            }
        }
    }
    Err("获取封面超时或失败".to_string())
}

/// Fetches album/playlist artwork bytes using zero-RAM local NVMe disk cache + keep-alive TLS connection pool,
/// with a dedicated priority lane for hero/now-playing artwork so route switches never starve behind grid thumbnails.
pub async fn fetch_artwork_bytes(url: &str, high_priority: bool) -> Result<Vec<u8>, String> {
    fetch_artwork_bytes_if_current(url, high_priority, None).await
}

/// Async singleflight coordinator for Apple Music Developer Token fetches so concurrent cold-start
/// or 401-refresh callers share ONE JS bundle download instead of a 3-5x thundering herd.
pub struct DevTokenSingleflight {
    cache: Mutex<Option<CachedDevToken>>,
    fetch_lock: tokio::sync::Mutex<()>,
    refresh_gen: AtomicU64,
}

impl Default for DevTokenSingleflight {
    fn default() -> Self {
        Self::new()
    }
}

impl DevTokenSingleflight {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
            fetch_lock: tokio::sync::Mutex::new(()),
            refresh_gen: AtomicU64::new(0),
        }
    }

    fn token_is_valid(tok: &CachedDevToken, now_secs: u64) -> bool {
        !tok.token.is_empty()
            && tok
                .exp
                .map(|exp| exp > now_secs.saturating_add(120))
                .unwrap_or(true)
    }

    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = None;
        }
    }

    pub fn get_cached(&self, now_secs: u64) -> Option<CachedDevToken> {
        self.cache
            .lock()
            .ok()
            .and_then(|g| g.as_ref().filter(|t| Self::token_is_valid(t, now_secs)).cloned())
    }

    pub fn put_cached(&self, tok: CachedDevToken) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(tok);
        }
        self.refresh_gen.fetch_add(1, Ordering::AcqRel);
    }

    /// Returns a valid cached token immediately, or serializes behind `fetch_lock` so only one
    /// concurrent caller executes `fetcher`; waiters reuse the freshly fetched token.
    pub async fn get_or_fetch<F, Fut>(
        &self,
        force_refresh: bool,
        now_secs: u64,
        fetcher: F,
    ) -> Result<CachedDevToken, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<CachedDevToken, String>>,
    {
        if !force_refresh {
            if let Some(tok) = self.get_cached(now_secs) {
                return Ok(tok);
            }
        }
        let gen_before = self.refresh_gen.load(Ordering::Acquire);
        let _sf_guard = self.fetch_lock.lock().await;
        if !force_refresh || self.refresh_gen.load(Ordering::Acquire) != gen_before {
            if let Some(tok) = self.get_cached(now_secs) {
                return Ok(tok);
            }
        }
        let tok = fetcher().await?;
        self.put_cached(tok.clone());
        Ok(tok)
    }
}

static DEV_TOKEN_SF: Lazy<DevTokenSingleflight> = Lazy::new(DevTokenSingleflight::new);

static JWT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}")
        .expect("valid JWT regex")
});

static SCRIPT_SRC_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(/assets/index[^"'\s<>]+\.js)"#).expect("valid bundle script regex")
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedDevToken {
    pub token: String,
    pub kid: Option<String>,
    pub exp: Option<u64>,
}

fn dev_token_disk_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = std::path::PathBuf::from(appdata).join("AMLight");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("dev_token.json"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthVerifyResult {
    pub dev_token_ok: bool,
    pub dev_token_kid: Option<String>,
    pub dev_token_exp: Option<u64>,
    pub has_user_token: bool,
    pub normalized_token_len: usize,
    pub user_token_ok: bool,
    pub detected_storefront: Option<String>,
    pub subscription_active: Option<bool>,
    pub http_status: Option<u16>,
    pub message: String,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn decode_jwt_json(segment: &str) -> Option<Value> {
    let bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .or_else(|_| general_purpose::URL_SAFE.decode(segment))
        .ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

pub fn extract_best_dev_token(bundle_text: &str, now_secs: u64) -> Option<CachedDevToken> {
    let mut seen = HashSet::new();
    let mut best: Option<(i32, CachedDevToken)> = None;

    for m in JWT_RE.find_iter(bundle_text) {
        let token = m.as_str();
        if !seen.insert(token) {
            continue;
        }
        let mut parts = token.split('.');
        let Some(header_b64) = parts.next() else { continue };
        let Some(payload_b64) = parts.next() else { continue };
        let Some(_sig_b64) = parts.next() else { continue };
        if parts.next().is_some() {
            continue;
        }

        let Some(header) = decode_jwt_json(header_b64) else { continue };
        let Some(payload) = decode_jwt_json(payload_b64) else { continue };

        let exp = payload.get("exp").and_then(|v| v.as_u64());
        if let Some(exp_ts) = exp {
            if exp_ts <= now_secs.saturating_add(60) {
                continue;
            }
        }

        let mut score = 0;
        if payload.get("iss").and_then(|v| v.as_str()) == Some("AMPWebPlay") {
            score += 100;
        }
        let kid = header
            .get("kid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if kid.as_deref() == Some("WebPlayKid") {
            score += 50;
        }
        if header.get("alg").and_then(|v| v.as_str()) == Some("ES256") {
            score += 20;
        }
        if header.get("typ").and_then(|v| v.as_str()) == Some("JWT") {
            score += 10;
        }
        if exp.is_some() {
            score += 5;
        }

        let candidate = CachedDevToken {
            token: token.to_string(),
            kid,
            exp,
        };

        if best.as_ref().is_none_or(|(best_score, _)| score > *best_score) {
            best = Some((score, candidate));
        }
    }

    best.map(|(_, tok)| tok)
}

/// Normalizes user input into a clean `Media-User-Token` header value.
/// Supports raw token, `media-user-token=...`, full HTTP `Cookie:` strings,
/// `Media-User-Token: ...` headers, surrounding quotes/newlines, and URL-encoding.
pub fn normalize_user_token(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut extracted = trimmed;

    // Handle pasted full Cookie string or semicolon/newline delimited pairs
    if trimmed.to_ascii_lowercase().contains("media-user-token") {
        for part in trimmed.split(|c| c == ';' || c == '\n' || c == '\r') {
            let seg = part.trim();
            if seg.is_empty() {
                continue;
            }
            // Strip optional "Cookie:" prefix on the first segment
            let seg = if seg.len() >= 7 && seg[..7].eq_ignore_ascii_case("cookie:") {
                seg[7..].trim()
            } else {
                seg
            };

            if let Some(pos) = seg.find(['=', ':']) {
                let (k, v) = seg.split_at(pos);
                let v = &v[1..];
                if k.trim().eq_ignore_ascii_case("media-user-token") {
                    extracted = v.trim();
                    break;
                }
            }
        }
    }

    let cleaned = extracted
        .trim()
        .trim_end_matches(';')
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim();

    if cleaned.is_empty() {
        return String::new();
    }

    let decoded = if cleaned.contains('%') {
        percent_decode_str(cleaned)
            .decode_utf8()
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| cleaned.to_string())
    } else {
        cleaned.to_string()
    };

    decoded
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
}

pub fn invalidate_dev_token_cache() {
    DEV_TOKEN_SF.invalidate();
    if let Some(p) = dev_token_disk_path() {
        let _ = std::fs::remove_file(p);
    }
}

pub async fn get_public_dev_token(
    force_refresh: bool,
    storefront_hint: Option<&str>,
) -> Result<CachedDevToken, String> {
    let now = now_unix_secs();
    let sf_hint_owned = storefront_hint.map(str::to_string);
    DEV_TOKEN_SF
        .get_or_fetch(force_refresh, now, move || async move {
            if !force_refresh {
                if let Some(p) = dev_token_disk_path() {
                    let disk_opt = tokio::task::spawn_blocking(move || {
                        let txt = std::fs::read_to_string(p).ok()?;
                        serde_json::from_str::<CachedDevToken>(&txt).ok()
                    })
                    .await
                    .ok()
                    .flatten();
                    if let Some(disk_tok) = disk_opt {
                        if DevTokenSingleflight::token_is_valid(&disk_tok, now_unix_secs()) {
                            return Ok(disk_tok);
                        }
                    }
                }
            }

            let mut storefronts = Vec::new();
            if let Some(ref sf) = sf_hint_owned {
                let sf_clean = sf.trim().to_ascii_lowercase();
                if !sf_clean.is_empty() && sf_clean.chars().all(|c| c.is_ascii_alphanumeric()) {
                    storefronts.push(sf_clean);
                }
            }
            for fallback in ["cn", "us"] {
                if !storefronts.iter().any(|s| s == fallback) {
                    storefronts.push(fallback.to_string());
                }
            }

            let mut errors = Vec::new();
            for sf in storefronts {
                let page_url = format!("{WEB_ORIGIN}/{sf}/browse");
                let html = match HTTP_CLIENT.get(&page_url).send().await {
                    Ok(resp) if resp.status().is_success() => match resp.text().await {
                        Ok(t) => t,
                        Err(e) => {
                            errors.push(format!("{page_url}: 读取 HTML 失败 ({e})"));
                            continue;
                        }
                    },
                    Ok(resp) => {
                        errors.push(format!("{page_url}: HTTP {}", resp.status()));
                        continue;
                    }
                    Err(e) => {
                        errors.push(format!("{page_url}: 网络请求失败 ({e})"));
                        continue;
                    }
                };

                let mut bundle_paths = Vec::new();
                for cap in SCRIPT_SRC_RE.captures_iter(&html) {
                    let p = cap[1].to_string();
                    if !bundle_paths.contains(&p) {
                        bundle_paths.push(p);
                    }
                }

                if bundle_paths.is_empty() {
                    errors.push(format!("{page_url}: 未找到 index-*.js bundle"));
                    continue;
                }

                for bundle_path in bundle_paths {
                    let bundle_url = format!("{WEB_ORIGIN}{bundle_path}");
                    let js_text = match HTTP_CLIENT.get(&bundle_url).send().await {
                        Ok(resp) if resp.status().is_success() => match resp.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                errors.push(format!("{bundle_url}: 读取 JS 失败 ({e})"));
                                continue;
                            }
                        },
                        Ok(resp) => {
                            errors.push(format!("{bundle_url}: HTTP {}", resp.status()));
                            continue;
                        }
                        Err(e) => {
                            errors.push(format!("{bundle_url}: 网络失败 ({e})"));
                            continue;
                        }
                    };

                    if let Some(tok) = extract_best_dev_token(&js_text, now_unix_secs()) {
                        if let Some(p) = dev_token_disk_path() {
                            if let Ok(json_str) = serde_json::to_string(&tok) {
                                tokio::task::spawn_blocking(move || {
                                    let _ = std::fs::write(p, json_str);
                                });
                            }
                        }
                        return Ok(tok);
                    }
                    errors.push(format!("{bundle_url}: bundle 中未匹配到有效 Developer JWT"));
                }
            }

            Err(format!(
                "获取 Apple Music Developer Token 失败: {}",
                errors.join(" | ")
            ))
        })
        .await
}

fn build_amp_url(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("API 路径不能为空".to_string());
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        if trimmed.starts_with(AMP_API_BASE) {
            return Ok(trimmed.to_string());
        }
        return Err("仅允许请求 amp-api.music.apple.com 接口".to_string());
    }
    Ok(format!("{AMP_API_BASE}{}", trimmed.trim_start_matches('/')))
}

fn format_apple_api_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<Value>(body) {
        if let Some(first) = json.get("errors").and_then(|e| e.as_array()).and_then(|a| a.first()) {
            let title = first.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let detail = first.get("detail").and_then(|v| v.as_str()).unwrap_or("");
            let combined = [title, detail]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" - ");
            if !combined.is_empty() {
                return format!("Apple Music API HTTP {} ({})", status.as_u16(), combined);
            }
        }
    }
    let snippet: String = body.trim().chars().take(180).collect();
    if snippet.is_empty() {
        format!("Apple Music API HTTP {}", status.as_u16())
    } else {
        format!("Apple Music API HTTP {}: {snippet}", status.as_u16())
    }
}

async fn send_amp_get(
    url: &str,
    dev_token: &str,
    user_token: Option<&str>,
) -> Result<(reqwest::StatusCode, String), String> {
    let mut req = HTTP_CLIENT
        .get(url)
        .header("Authorization", format!("Bearer {dev_token}"))
        .header("Origin", WEB_ORIGIN)
        .header("Referer", format!("{WEB_ORIGIN}/"));

    if let Some(ut) = user_token {
        if !ut.is_empty() {
            req = req.header("Media-User-Token", ut);
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求 amp-api 失败: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取响应体失败: {e}"))?;
    Ok((status, body))
}

fn is_retryable_amp_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

pub async fn fetch_am_api(
    path: String,
    user_token: Option<String>,
    dev_token: Option<String>,
    storefront_hint: Option<String>,
) -> Result<Value, String> {
    let url = build_amp_url(&path)?;
    let normalized_user = user_token
        .as_deref()
        .map(normalize_user_token)
        .filter(|s| !s.is_empty());

    if url.contains("/v1/me/") && normalized_user.is_none() {
        return Err("此接口需要有效的 Media-User-Token，请先在设置中登录或粘贴 Cookie".to_string());
    }

    let custom_dev = dev_token
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut effective_dev = match &custom_dev {
        Some(t) => t.clone(),
        None => {
            get_public_dev_token(false, storefront_hint.as_deref())
                .await?
                .token
        }
    };

    let mut last_err: Option<String> = None;
    let mut refreshed_dev = false;
    let backoffs_ms = [250u64, 650];

    for attempt in 0..=backoffs_ms.len() {
        match send_amp_get(&url, &effective_dev, normalized_user.as_deref()).await {
            Ok((mut status, mut body)) => {
                // Retry once with refreshed public Developer Token on 401
                if status == reqwest::StatusCode::UNAUTHORIZED
                    && custom_dev.is_none()
                    && !refreshed_dev
                {
                    refreshed_dev = true;
                    invalidate_dev_token_cache();
                    if let Ok(fresh) = get_public_dev_token(true, storefront_hint.as_deref()).await
                    {
                        effective_dev = fresh.token;
                        if let Ok(retry) =
                            send_amp_get(&url, &effective_dev, normalized_user.as_deref()).await
                        {
                            status = retry.0;
                            body = retry.1;
                        }
                    }
                }

                if status.is_success() {
                    return serde_json::from_str::<Value>(&body)
                        .map_err(|e| format!("解析 Apple Music JSON 失败: {e}"));
                }

                let err_msg = format_apple_api_error(status, &body);
                if is_retryable_amp_status(status) && attempt < backoffs_ms.len() {
                    last_err = Some(err_msg);
                    tokio::time::sleep(Duration::from_millis(backoffs_ms[attempt])).await;
                    continue;
                }
                return Err(err_msg);
            }
            Err(e) => {
                if attempt < backoffs_ms.len() {
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(backoffs_ms[attempt])).await;
                    continue;
                }
                return Err(e);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| "请求 amp-api 失败".to_string()))
}

/// Splits an AMP relative or absolute path into `(base_path_without_leading_slash, query_pairs)`.
fn split_amp_path_query(path: &str) -> (String, Vec<(String, String)>) {
    let trimmed = path.trim();
    let stripped = trimmed
        .strip_prefix(AMP_API_BASE)
        .unwrap_or(trimmed)
        .trim_start_matches('/');
    let Some((base, query)) = stripped.split_once('?') else {
        return (stripped.to_string(), Vec::new());
    };
    let pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|seg| match seg.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (seg.to_string(), String::new()),
        })
        .collect();
    (base.to_string(), pairs)
}

fn join_amp_path_query(base: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return base.to_string();
    }
    let q = pairs
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                k.clone()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{q}")
}

/// Extracts numeric `limit=...` from an AMP path if present and > 0.
pub fn parse_amp_limit(path: &str) -> Option<usize> {
    let (_, pairs) = split_amp_path_query(path);
    pairs
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("limit"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
}

/// Builds the next AMP API page path from Apple Music's `"next"` link while preserving important
/// query parameters (`limit`, `include`, `extend`, `l`, etc.) from `initial_path` that Apple's
/// pagination cursor frequently drops on page 2+, and guarding against Apple's library API quirk
/// where `"next"` omits the `/tracks` suffix.
pub fn build_next_amp_path(initial_path: &str, next_href: &str) -> String {
    let (init_base, init_pairs) = split_amp_path_query(initial_path);
    let (mut next_base, mut next_pairs) = split_amp_path_query(next_href);

    // Guard against known Apple Music library API bug where `next` drops `/tracks`
    // (e.g. `/v1/me/library/playlists/{id}?offset=100` instead of `.../tracks?offset=100`).
    if init_base.ends_with("/tracks") && !next_base.ends_with("/tracks") {
        let stripped_init = init_base.trim_end_matches("/tracks");
        if next_base.is_empty() || next_base == stripped_init {
            next_base = init_base.clone();
        }
    } else if next_base.is_empty() {
        next_base = init_base.clone();
    }

    for (k, v) in init_pairs {
        if k.eq_ignore_ascii_case("offset") {
            continue;
        }
        let already = next_pairs
            .iter()
            .any(|(nk, _)| nk.eq_ignore_ascii_case(&k));
        if !already {
            next_pairs.push((k, v));
        }
    }

    // Ensure `/tracks` endpoints always retain `limit=100` on page 2+ rather than dropping to Apple's default 25.
    if next_base.ends_with("/tracks")
        && !next_pairs
            .iter()
            .any(|(nk, _)| nk.eq_ignore_ascii_case("limit"))
    {
        next_pairs.push(("limit".to_string(), "100".to_string()));
    }

    join_amp_path_query(&next_base, &next_pairs)
}

/// Builds a stripped fallback path from Apple's raw `"next"` link keeping only `limit` and `l`
/// (dropping `include`/`extend`) in case an endpoint rejects extra query params with HTTP 400 on page 2+.
fn build_minimal_next_amp_path(initial_path: &str, next_href: &str) -> String {
    let (init_base, init_pairs) = split_amp_path_query(initial_path);
    let (mut next_base, mut next_pairs) = split_amp_path_query(next_href);
    if init_base.ends_with("/tracks") && !next_base.ends_with("/tracks") {
        let stripped_init = init_base.trim_end_matches("/tracks");
        if next_base.is_empty() || next_base == stripped_init {
            next_base = init_base;
        }
    } else if next_base.is_empty() {
        next_base = init_base;
    }
    for (k, v) in init_pairs {
        if k.eq_ignore_ascii_case("limit") || k.eq_ignore_ascii_case("l") {
            if !next_pairs.iter().any(|(nk, _)| nk.eq_ignore_ascii_case(&k)) {
                next_pairs.push((k, v));
            }
        }
    }
    join_amp_path_query(&next_base, &next_pairs)
}

/// Replaces or appends `offset={offset}` on an AMP path while preserving all other query params.
pub fn build_offset_amp_path(initial_path: &str, offset: usize) -> String {
    let (base, init_pairs) = split_amp_path_query(initial_path);
    let mut pairs: Vec<(String, String)> = init_pairs
        .into_iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("offset"))
        .collect();
    pairs.push(("offset".to_string(), offset.to_string()));
    join_amp_path_query(&base, &pairs)
}

/// Builds an offset-paginated path combining `initial_path` and Apple's `"next"` link (when present)
/// so both `initial_path`'s query params and any extra base/params from `next_href` are preserved.
pub fn build_offset_from_next_amp_path(
    initial_path: &str,
    next_href: Option<&str>,
    offset: usize,
) -> String {
    match next_href {
        Some(nh) if !nh.trim().is_empty() => {
            let merged = build_next_amp_path(initial_path, nh);
            build_offset_amp_path(&merged, offset)
        }
        _ => build_offset_amp_path(initial_path, offset),
    }
}

/// Extracts numeric `offset=...` from an Apple Music `"next"` link if present.
pub fn parse_next_offset(next_href: &str) -> Option<usize> {
    let (_, pairs) = split_amp_path_query(next_href);
    pairs
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("offset"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
}

fn page_fingerprint(items: &[Value]) -> Option<(usize, String, String)> {
    if items.is_empty() {
        return None;
    }
    let item_sig = |v: &Value| -> String {
        if let Some(id) = v.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return id.to_string();
        }
        let nm = v
            .pointer("/attributes/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let ar = v
            .pointer("/attributes/artistName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        format!("{nm}\x1f{ar}")
    };
    Some((
        items.len(),
        item_sig(items.first().unwrap()),
        item_sig(items.last().unwrap()),
    ))
}

fn push_amp_items(
    out: &mut Vec<Value>,
    seen_ids: &mut HashSet<String>,
    items: Vec<Value>,
    dedupe_by_id: bool,
) {
    for item in items {
        if dedupe_by_id {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                if !id.is_empty() && !seen_ids.insert(id.to_string()) {
                    continue;
                }
            }
        }
        out.push(item);
    }
}

async fn fetch_amp_page_with_400_fallback<F, Fut>(
    fetcher: &F,
    initial_path: &str,
    primary_path: &str,
    raw_next_href: Option<&str>,
) -> Result<Value, String>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    match fetcher(primary_path.to_string()).await {
        Ok(json) => Ok(json),
        Err(err) => {
            if err.contains("HTTP 400") {
                if let Some(nh) = raw_next_href {
                    let minimal = build_minimal_next_amp_path(initial_path, nh);
                    if minimal != primary_path {
                        if let Ok(json) = fetcher(minimal).await {
                            return Ok(json);
                        }
                    }
                    let raw_clean = nh.trim().trim_start_matches('/').to_string();
                    if !raw_clean.is_empty() && raw_clean != primary_path {
                        if let Ok(json) = fetcher(raw_clean).await {
                            return Ok(json);
                        }
                    }
                }
            }
            Err(err)
        }
    }
}

async fn join_batch_up_to_4<F, Fut>(
    fetcher: &F,
    chunk: &[(usize, String)],
) -> Vec<(usize, String, Result<Value, String>)>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    match chunk {
        [] => Vec::new(),
        [(o0, p0)] => {
            let r0 = fetcher(p0.clone()).await;
            vec![(*o0, p0.clone(), r0)]
        }
        [(o0, p0), (o1, p1)] => {
            let (r0, r1) = tokio::join!(fetcher(p0.clone()), fetcher(p1.clone()));
            vec![(*o0, p0.clone(), r0), (*o1, p1.clone(), r1)]
        }
        [(o0, p0), (o1, p1), (o2, p2)] => {
            let (r0, r1, r2) =
                tokio::join!(fetcher(p0.clone()), fetcher(p1.clone()), fetcher(p2.clone()));
            vec![
                (*o0, p0.clone(), r0),
                (*o1, p1.clone(), r1),
                (*o2, p2.clone(), r2),
            ]
        }
        [(o0, p0), (o1, p1), (o2, p2), (o3, p3), ..] => {
            let (r0, r1, r2, r3) = tokio::join!(
                fetcher(p0.clone()),
                fetcher(p1.clone()),
                fetcher(p2.clone()),
                fetcher(p3.clone()),
            );
            vec![
                (*o0, p0.clone(), r0),
                (*o1, p1.clone(), r1),
                (*o2, p2.clone(), r2),
                (*o3, p3.clone(), r3),
            ]
        }
    }
}

/// Core Apple Music pagination engine decoupled from the network transport so both production
/// HTTP calls and offline unit tests exercise the exact same pagination, parallel fast-path,
/// partial-failure sequential recovery, implicit offset probing, and duplicate-page protection.
pub async fn paginate_am_pages_with_fetcher<F, Fut>(
    initial_path: &str,
    first_items: Vec<Value>,
    mut next_href: Option<String>,
    total_hint: Option<usize>,
    max_pages: usize,
    dedupe_by_id: bool,
    fetcher: F,
) -> Vec<Value>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    let first_len = first_items.len();
    let mut out = Vec::with_capacity(
        total_hint
            .map(|t| t.min(first_len.max(100).saturating_mul(max_pages.max(1))))
            .unwrap_or(first_len),
    );
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut seen_page_fps: HashSet<(usize, String, String)> = HashSet::new();
    if let Some(fp) = page_fingerprint(&first_items) {
        seen_page_fps.insert(fp);
    }
    push_amp_items(&mut out, &mut seen_ids, first_items, dedupe_by_id);

    let max_extra_pages = max_pages.saturating_sub(1);
    if max_extra_pages == 0 {
        return out;
    }

    let (init_base, _) = split_amp_path_query(initial_path);
    let requested_limit = parse_amp_limit(initial_path).unwrap_or(100);
    let mut effective_total = total_hint;
    let mut probing_implicit_offset = false;

    // Synthesize offset continuation when Apple omits `"next"` despite `meta.total > first_len`,
    // or when a `/tracks` page hits the full `limit` ceiling (e.g. 100 tracks) with no `meta.total`
    // (guards against Apple Music's known omitted-`next` bug on large playlists).
    if next_href.is_none() && first_len > 0 {
        if effective_total.is_some_and(|t| t > first_len) {
            next_href = Some(build_offset_amp_path(initial_path, first_len));
            probing_implicit_offset = true;
        } else if effective_total.is_none()
            && init_base.ends_with("/tracks")
            && first_len >= requested_limit
        {
            next_href = Some(build_offset_amp_path(initial_path, first_len));
            probing_implicit_offset = true;
        }
    }

    if next_href.is_none() {
        return out;
    }

    let mut pages_fetched = 0usize;
    let mut visited: HashSet<String> = HashSet::new();

    // Fast path: when Apple returns `meta.total` AND numeric `offset == first_len`, fetch pages 2..N
    // concurrently in ordered chunks of 4 over HTTP/2 keep-alive. On any partial batch failure,
    // NEVER discard `next_href` — seamlessly hand off the failed offset to the sequential recovery loop!
    if let (Some(total), Some(orig_nh)) = (effective_total, next_href.clone()) {
        if first_len > 0 && total > first_len {
            if let Some(next_off) = parse_next_offset(&orig_nh) {
                if next_off == first_len {
                    let mut offsets: Vec<(usize, String)> = Vec::new();
                    let mut off = first_len;
                    while off < total && offsets.len() < max_extra_pages {
                        let p = build_offset_from_next_amp_path(initial_path, Some(&orig_nh), off);
                        offsets.push((off, p));
                        off = off.saturating_add(first_len);
                    }
                    let mut last_page_next: Option<String> = None;
                    let mut recovery_next: Option<String> = None;
                    let mut stopped_early = false;
                    let mut batch_cache: HashMap<usize, Value> = HashMap::new();

                    for chunk in offsets.chunks(4) {
                        let results = join_batch_up_to_4(&fetcher, chunk).await;
                        for (idx_in_chunk, (o, p, res)) in results.into_iter().enumerate() {
                            if stopped_early {
                                if let Ok(ok_json) = res {
                                    batch_cache.insert(o, ok_json);
                                }
                                continue;
                            }
                            match res {
                                Ok(json) => {
                                    visited.insert(p);
                                    pages_fetched += 1;
                                    let items = json
                                        .get("data")
                                        .and_then(Value::as_array)
                                        .cloned()
                                        .unwrap_or_default();
                                    let got_empty = items.is_empty();
                                    let dup_page = page_fingerprint(&items)
                                        .is_some_and(|fp| !seen_page_fps.insert(fp));
                                    if got_empty || dup_page {
                                        last_page_next = None;
                                        recovery_next = None;
                                        stopped_early = true;
                                        continue;
                                    }
                                    push_amp_items(&mut out, &mut seen_ids, items, dedupe_by_id);
                                    last_page_next = json
                                        .get("next")
                                        .and_then(Value::as_str)
                                        .map(|s| s.to_string())
                                        .filter(|s| !s.trim().is_empty());
                                }
                                Err(err) => {
                                    // If a trailing page past actual playlist length returns 404 after
                                    // the prior page already had no `"next"`, treat as clean EOF.
                                    let clean_eof = err.contains("HTTP 404")
                                        && idx_in_chunk > 0
                                        && last_page_next.is_none();
                                    if clean_eof {
                                        last_page_next = None;
                                        recovery_next = None;
                                    } else {
                                        // Hand off the exact failed offset to sequential recovery loop
                                        // (which also handles HTTP 400 stripped-param fallback)!
                                        recovery_next = Some(
                                            last_page_next.take().unwrap_or_else(|| {
                                                build_offset_from_next_amp_path(
                                                    initial_path,
                                                    Some(&orig_nh),
                                                    o,
                                                )
                                            }),
                                        );
                                    }
                                    stopped_early = true;
                                }
                            }
                        }
                        if stopped_early {
                            break;
                        }
                    }

                    // If recovery_next is set and we already cached later pages from the same chunk,
                    // keep them ready for instant consumption once the single failed offset succeeds!
                    if recovery_next.is_some() {
                        next_href = recovery_next;
                    } else if stopped_early {
                        next_href = None;
                    } else {
                        next_href = last_page_next;
                    }

                    // Drain sequential loop with `batch_cache` reuse below
                    while pages_fetched < max_extra_pages {
                        let Some(nh) = next_href.take() else {
                            break;
                        };
                        let next_path = build_next_amp_path(initial_path, &nh);
                        if !visited.insert(next_path.clone()) {
                            break;
                        }
                        let cur_off = parse_next_offset(&next_path);
                        let res = if let Some(cached_json) =
                            cur_off.and_then(|co| batch_cache.remove(&co))
                        {
                            Ok(cached_json)
                        } else {
                            fetch_amp_page_with_400_fallback(
                                &fetcher,
                                initial_path,
                                &next_path,
                                Some(&nh),
                            )
                            .await
                        };
                        match res {
                            Ok(json) => {
                                pages_fetched += 1;
                                let items = json
                                    .get("data")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                if items.is_empty() {
                                    break;
                                }
                                if page_fingerprint(&items)
                                    .is_some_and(|fp| !seen_page_fps.insert(fp))
                                {
                                    break;
                                }
                                let page_len = items.len();
                                push_amp_items(&mut out, &mut seen_ids, items, dedupe_by_id);
                                if let Some(t) = json
                                    .pointer("/meta/total")
                                    .and_then(Value::as_u64)
                                    .map(|n| n as usize)
                                {
                                    effective_total = Some(effective_total.map_or(t, |e| e.max(t)));
                                }
                                let json_next = json
                                    .get("next")
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string())
                                    .filter(|s| !s.trim().is_empty());
                                if json_next.is_some() {
                                    next_href = json_next;
                                } else if let Some(co) = cur_off {
                                    let advance = co.saturating_add(page_len);
                                    if effective_total.is_some_and(|t| advance < t) {
                                        next_href = Some(build_offset_from_next_amp_path(
                                            initial_path,
                                            Some(&nh),
                                            advance,
                                        ));
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    return out;
                }
            }
        }
    }

    // Sequential `"next"` cursor & implicit-offset traversal (handles opaque cursors,
    // missing `meta.total`, omitted `"next"` at full-page ceiling, or overflow past `meta.total`)
    while pages_fetched < max_extra_pages {
        let Some(nh) = next_href.take() else {
            break;
        };
        let next_path = build_next_amp_path(initial_path, &nh);
        if !visited.insert(next_path.clone()) {
            break;
        }
        let cur_off = parse_next_offset(&next_path);
        match fetch_amp_page_with_400_fallback(&fetcher, initial_path, &next_path, Some(&nh)).await
        {
            Ok(json) => {
                pages_fetched += 1;
                let items = json
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if items.is_empty() {
                    break;
                }
                if page_fingerprint(&items).is_some_and(|fp| !seen_page_fps.insert(fp)) {
                    break;
                }
                let page_len = items.len();
                push_amp_items(&mut out, &mut seen_ids, items, dedupe_by_id);
                if let Some(t) = json
                    .pointer("/meta/total")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                {
                    effective_total = Some(effective_total.map_or(t, |e| e.max(t)));
                }
                let json_next = json
                    .get("next")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .filter(|s| !s.trim().is_empty());
                if json_next.is_some() {
                    next_href = json_next;
                } else if let Some(co) = cur_off {
                    let advance = co.saturating_add(page_len);
                    let more_by_total = effective_total.is_some_and(|t| advance < t);
                    let more_by_full_page = effective_total.is_none()
                        && (probing_implicit_offset || init_base.ends_with("/tracks"))
                        && page_len >= requested_limit;
                    if more_by_total || more_by_full_page {
                        probing_implicit_offset = true;
                        next_href = Some(build_offset_from_next_amp_path(
                            initial_path,
                            Some(&nh),
                            advance,
                        ));
                    }
                }
            }
            Err(_) => break,
        }
    }

    out
}

/// Follows remaining Apple Music API pages given page 1's `data`, optional `"next"` href,
/// and optional `meta.total` hint. Uses bounded 4-way parallel offset fetching when `meta.total`
/// and numeric `offset=` align, and safely falls back to sequential `"next"` traversal otherwise.
pub async fn fetch_am_remaining_pages(
    initial_path: &str,
    first_items: Vec<Value>,
    next_href: Option<String>,
    total_hint: Option<usize>,
    user_token: Option<String>,
    storefront_hint: Option<String>,
    max_pages: usize,
    dedupe_by_id: bool,
) -> Vec<Value> {
    paginate_am_pages_with_fetcher(
        initial_path,
        first_items,
        next_href,
        total_hint,
        max_pages,
        dedupe_by_id,
        move |p| {
            let ut = user_token.clone();
            let sf = storefront_hint.clone();
            async move { fetch_am_api(p, ut, None, sf).await }
        },
    )
    .await
}

/// Fetches all pages of an Apple Music API collection endpoint (`data` + `"next"` cursor),
/// preserving query parameters (`include`, `extend`, `limit`, `l`) across every page.
pub async fn fetch_am_paginated_data(
    initial_path: &str,
    user_token: Option<String>,
    storefront_hint: Option<String>,
    max_pages: usize,
    dedupe_by_id: bool,
) -> Result<Vec<Value>, String> {
    let first_json = fetch_am_api(
        initial_path.to_string(),
        user_token.clone(),
        None,
        storefront_hint.clone(),
    )
    .await?;
    let first_items = first_json
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let first_next = first_json
        .get("next")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());
    let total_hint = first_json
        .pointer("/meta/total")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    Ok(fetch_am_remaining_pages(
        initial_path,
        first_items,
        first_next,
        total_hint,
        user_token,
        storefront_hint,
        max_pages,
        dedupe_by_id,
    )
    .await)
}

/// Fetches all tracks of an Apple Music catalog playlist or album given the container root JSON
/// (`data[0]` from `v1/catalog/{sf}/{kind}/{id}?include=tracks...`), transparently paginating
/// beyond Apple's 100-item inline relationship cap even when `relationships.tracks.next` is omitted.
pub async fn fetch_catalog_container_remaining_tracks(
    sf: &str,
    kind: &str,
    id: &str,
    root: &Value,
    user_token: Option<String>,
    max_pages: usize,
) -> Vec<Value> {
    let first_items = root
        .pointer("/relationships/tracks/data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tracks_next = root
        .pointer("/relationships/tracks/next")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());
    let tracks_total = root
        .pointer("/relationships/tracks/meta/total")
        .and_then(Value::as_u64)
        .map(|n| n as usize);

    let needs_more = tracks_next.is_some()
        || tracks_total.is_some_and(|t| t > first_items.len())
        || (kind == "playlists" && first_items.len() >= 100);
    if !needs_more {
        return first_items;
    }

    let base_tracks_path =
        format!("v1/catalog/{sf}/{kind}/{id}/tracks?extend=audioTraits&limit=100&l=zh-Hans-CN");
    fetch_am_remaining_pages(
        &base_tracks_path,
        first_items,
        tracks_next,
        tracks_total,
        user_token,
        Some(sf.to_string()),
        max_pages.max(1),
        false,
    )
    .await
}

/// Fetches all tracks of an Apple Music library playlist or library album across all pages
/// (`v1/me/library/{sub}/{id}/tracks?include=catalog&extend=audioTraits&limit=100&l=zh-Hans-CN`),
/// transparently falling back to `v1/catalog/{sf}/{sub}/{catalog_id}/tracks` when the library
/// endpoint returns 404/empty (subscribed catalog playlists) or truncates a subscribed playlist at 100.
pub async fn fetch_library_container_tracks(
    sub: &str,
    id: &str,
    catalog_id_hint: Option<&str>,
    user_token: Option<String>,
    storefront: &str,
    max_pages: usize,
) -> Result<Vec<Value>, String> {
    let sf = if storefront.trim().is_empty() {
        "cn"
    } else {
        storefront.trim()
    };
    let tracks_path = format!(
        "v1/me/library/{sub}/{id}/tracks?include=catalog&extend=audioTraits&limit=100&l=zh-Hans-CN"
    );
    let tr_res = fetch_am_paginated_data(
        &tracks_path,
        user_token.clone(),
        Some(sf.to_string()),
        max_pages.max(1),
        false,
    )
    .await;

    let mut items = match tr_res {
        Ok(v) => v,
        Err(err) => {
            if let Some(cid) = catalog_id_hint.map(str::trim).filter(|s| !s.is_empty()) {
                let cat_path = format!(
                    "v1/catalog/{sf}/{sub}/{cid}/tracks?extend=audioTraits&limit=100&l=zh-Hans-CN"
                );
                match fetch_am_paginated_data(
                    &cat_path,
                    user_token.clone(),
                    Some(sf.to_string()),
                    max_pages.max(1),
                    false,
                )
                .await
                {
                    Ok(cat_items) => return Ok(cat_items),
                    Err(_) => return Err(err),
                }
            }
            return Err(err);
        }
    };

    // Subscribed catalog playlists occasionally return empty or truncate at 100 on the library endpoint;
    // cross-check the catalog endpoint when `catalog_id_hint` is known!
    if (items.is_empty() || (sub == "playlists" && items.len() == 100))
        && catalog_id_hint.is_some_and(|s| !s.trim().is_empty())
    {
        if let Some(cid) = catalog_id_hint.map(str::trim).filter(|s| !s.is_empty()) {
            let cat_path = format!(
                "v1/catalog/{sf}/{sub}/{cid}/tracks?extend=audioTraits&limit=100&l=zh-Hans-CN"
            );
            if let Ok(cat_items) = fetch_am_paginated_data(
                &cat_path,
                user_token,
                Some(sf.to_string()),
                max_pages.max(1),
                false,
            )
            .await
            {
                if cat_items.len() > items.len() {
                    items = cat_items;
                }
            }
        }
    }

    Ok(items)
}

async fn send_amp_post(
    url: &str,
    dev_token: &str,
    user_token: &str,
) -> Result<(reqwest::StatusCode, String), String> {
    let mut req = HTTP_CLIENT
        .post(url)
        .header("Authorization", format!("Bearer {dev_token}"))
        .header("Origin", WEB_ORIGIN)
        .header("Referer", format!("{WEB_ORIGIN}/"))
        .header("Content-Length", "0")
        .body("");
    if !user_token.trim().is_empty() {
        req = req.header("Media-User-Token", user_token.trim())
            .header("Music-User-Token", user_token.trim());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求 amp-api 写入失败: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    Ok((status, body))
}

async fn send_amp_post_json(
    url: &str,
    dev_token: &str,
    user_token: &str,
    payload: &Value,
) -> Result<(reqwest::StatusCode, String), String> {
    let mut req = HTTP_CLIENT
        .post(url)
        .header("Authorization", format!("Bearer {dev_token}"))
        .header("Origin", WEB_ORIGIN)
        .header("Referer", format!("{WEB_ORIGIN}/"))
        .header("Content-Type", "application/json")
        .json(payload);
    if !user_token.trim().is_empty() {
        req = req
            .header("Media-User-Token", user_token.trim())
            .header("Music-User-Token", user_token.trim());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求 amp-api JSON 写入失败: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    Ok((status, body))
}

/// Sanitizes and partitions song IDs into ordered Apple Music library playlist write batches.
pub fn chunk_library_playlist_song_ids(song_ids: &[String], batch_size: usize) -> Vec<Vec<String>> {
    let step = batch_size.max(1);
    let clean: Vec<String> = song_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        })
        .collect();
    clean.chunks(step).map(|c| c.to_vec()).collect()
}

/// Creates a new cloud library playlist on Apple Music (`POST /v1/me/library/playlists`)
/// and optionally seeds it with matched catalog song IDs (chunked cleanly for arbitrarily large playlists).
pub async fn create_library_playlist(
    name: &str,
    description: &str,
    catalog_song_ids: &[String],
    user_token: Option<String>,
    storefront_hint: Option<String>,
) -> Result<String, String> {
    let normalized_user = user_token
        .as_deref()
        .map(normalize_user_token)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "创建 Apple Music 歌单需要有效的 Media-User-Token，请先在设置中登录 Apple Music 账号"
                .to_string()
        })?;

    let batches = chunk_library_playlist_song_ids(catalog_song_ids, DEFAULT_LIBRARY_WRITE_BATCH);
    let first_batch: Vec<Value> = batches
        .first()
        .map(|ids| {
            ids.iter()
                .map(|id| {
                    let ty = if id.starts_with("i.") {
                        "library-songs"
                    } else {
                        "songs"
                    };
                    serde_json::json!({ "id": id, "type": ty })
                })
                .collect()
        })
        .unwrap_or_default();

    let payload = if first_batch.is_empty() {
        serde_json::json!({
            "attributes": {
                "name": name.trim(),
                "description": description.trim(),
            }
        })
    } else {
        serde_json::json!({
            "attributes": {
                "name": name.trim(),
                "description": description.trim(),
            },
            "relationships": {
                "tracks": { "data": first_batch }
            }
        })
    };

    let url = format!("{AMP_API_BASE}v1/me/library/playlists");
    let dev = get_public_dev_token(false, storefront_hint.as_deref()).await?;
    let (mut status, mut body) = send_amp_post_json(&url, &dev.token, &normalized_user, &payload).await?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_dev_token_cache();
        if let Ok(fresh) = get_public_dev_token(true, storefront_hint.as_deref()).await {
            let retry = send_amp_post_json(&url, &fresh.token, &normalized_user, &payload).await?;
            status = retry.0;
            body = retry.1;
        }
    }
    if !status.is_success() {
        return Err(format_apple_api_error(status, &body));
    }

    let playlist_id = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|j| j.pointer("/data/0/id").and_then(Value::as_str).map(|s| s.to_string()))
        .unwrap_or_default();

    if !playlist_id.is_empty() && batches.len() > 1 {
        let remaining: Vec<String> = batches[1..].iter().flatten().cloned().collect();
        add_tracks_to_library_playlist_internal(
            &playlist_id,
            &remaining,
            Some(normalized_user),
            storefront_hint,
            true,
        )
        .await?;
    }

    Ok(playlist_id)
}

async fn add_tracks_to_library_playlist_internal(
    playlist_id: &str,
    song_ids: &[String],
    user_token: Option<String>,
    storefront_hint: Option<String>,
    retry_404_after_create: bool,
) -> Result<(), String> {
    let normalized_user = user_token
        .as_deref()
        .map(normalize_user_token)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "添加到 Apple Music 歌单需要有效的 Media-User-Token，请先在设置中登录绑定 Cookie"
                .to_string()
        })?;
    let pid = playlist_id.trim();
    if pid.is_empty() {
        return Err("无效的目标歌单 ID".to_string());
    }
    let batches = chunk_library_playlist_song_ids(song_ids, DEFAULT_LIBRARY_WRITE_BATCH);
    if batches.is_empty() {
        return Err("没有可添加的有效歌曲 ID".to_string());
    }
    let url = format!("{AMP_API_BASE}v1/me/library/playlists/{pid}/tracks");
    let mut dev_tok = get_public_dev_token(false, storefront_hint.as_deref())
        .await?
        .token;
    let backoffs_ms = [350u64, 800, 1500];

    for (batch_idx, chunk) in batches.iter().enumerate() {
        if batch_idx > 0 {
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        let items: Vec<Value> = chunk
            .iter()
            .map(|id| {
                let ty = if id.starts_with("i.") {
                    "library-songs"
                } else {
                    "songs"
                };
                serde_json::json!({ "id": id, "type": ty })
            })
            .collect();
        let payload = serde_json::json!({ "data": items });
        let mut refreshed_dev = false;

        for attempt in 0..=backoffs_ms.len() {
            match send_amp_post_json(&url, &dev_tok, &normalized_user, &payload).await {
                Ok((mut status, mut body)) => {
                    if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed_dev {
                        refreshed_dev = true;
                        invalidate_dev_token_cache();
                        if let Ok(fresh) =
                            get_public_dev_token(true, storefront_hint.as_deref()).await
                        {
                            dev_tok = fresh.token;
                            if let Ok(retry) =
                                send_amp_post_json(&url, &dev_tok, &normalized_user, &payload).await
                            {
                                status = retry.0;
                                body = retry.1;
                            }
                        }
                    }
                    if status.is_success() {
                        break;
                    }
                    let should_retry = is_retryable_amp_status(status)
                        || (retry_404_after_create && status == reqwest::StatusCode::NOT_FOUND);
                    if should_retry && attempt < backoffs_ms.len() {
                        tokio::time::sleep(Duration::from_millis(backoffs_ms[attempt])).await;
                        continue;
                    }
                    return Err(format_apple_api_error(status, &body));
                }
                Err(e) => {
                    if attempt < backoffs_ms.len() {
                        tokio::time::sleep(Duration::from_millis(backoffs_ms[attempt])).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}

/// Appends catalog/library tracks to an existing Apple Music cloud library playlist
/// (`POST /v1/me/library/playlists/{playlist_id}/tracks`), automatically chunking large lists.
pub async fn add_tracks_to_library_playlist(
    playlist_id: &str,
    song_ids: &[String],
    user_token: Option<String>,
    storefront_hint: Option<String>,
) -> Result<(), String> {
    add_tracks_to_library_playlist_internal(playlist_id, song_ids, user_token, storefront_hint, false)
        .await
}

/// Resolves an Apple Music artist ID & canonical name from an artist display string.
pub async fn resolve_artist_by_name(
    artist_name: &str,
    storefront: &str,
    user_token: Option<String>,
) -> Option<(String, String)> {
    let clean = artist_name
        .split(&['/', ',', '&', '、', '，'][..])
        .next()
        .unwrap_or(artist_name)
        .trim();
    if clean.is_empty() {
        return None;
    }
    let sf = if storefront.trim().is_empty() { "cn" } else { storefront.trim() };
    let enc: String = url::form_urlencoded::byte_serialize(clean.as_bytes()).collect();
    let path = format!("v1/catalog/{sf}/search?term={enc}&types=artists&limit=5&l=zh-Hans-CN");
    let json = fetch_am_api(path, user_token, None, Some(sf.to_string())).await.ok()?;
    let arr = json.pointer("/results/artists/data").and_then(Value::as_array)?;
    let want_norm = crate::backends::normalize_match_text(clean);
    let best = arr
        .iter()
        .find(|n| {
            n.pointer("/attributes/name")
                .and_then(Value::as_str)
                .is_some_and(|nm| crate::backends::normalize_match_text(nm) == want_norm)
        })
        .or_else(|| arr.first())?;
    let id = best.get("id").and_then(Value::as_str)?.trim().to_string();
    let name = best
        .pointer("/attributes/name")
        .and_then(Value::as_str)
        .unwrap_or(clean)
        .to_string();
    Some((id, name))
}

/// Resolves an Apple Music album ID & canonical name from album/artist strings.
pub async fn resolve_album_by_name(
    album_name: &str,
    artist_name: &str,
    storefront: &str,
    user_token: Option<String>,
) -> Option<(String, String)> {
    let alb = album_name.trim();
    if alb.is_empty() {
        return None;
    }
    let sf = if storefront.trim().is_empty() { "cn" } else { storefront.trim() };
    let q = if artist_name.trim().is_empty() {
        alb.to_string()
    } else {
        format!("{alb} {}", artist_name.trim())
    };
    let enc: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
    let path = format!("v1/catalog/{sf}/search?term={enc}&types=albums&limit=5&l=zh-Hans-CN");
    let json = fetch_am_api(path, user_token, None, Some(sf.to_string())).await.ok()?;
    let arr = json.pointer("/results/albums/data").and_then(Value::as_array)?;
    let want = crate::backends::normalize_match_text(alb);
    let best = arr
        .iter()
        .find(|n| {
            n.pointer("/attributes/name")
                .and_then(Value::as_str)
                .is_some_and(|nm| crate::backends::normalize_match_text(nm) == want)
        })
        .or_else(|| arr.first())?;
    let id = best.get("id").and_then(Value::as_str)?.trim().to_string();
    let name = best
        .pointer("/attributes/name")
        .and_then(Value::as_str)
        .unwrap_or(alb)
        .to_string();
    Some((id, name))
}

/// Cross-platform song matcher targeting Apple Music catalog (`v1/catalog/{sf}/search`).
pub async fn search_best_song_match(
    title: &str,
    artist: &str,
    duration_sec: f64,
    storefront: &str,
    user_token: Option<String>,
) -> Option<crate::am_agent::AgentTrackInput> {
    let clean_t = title.trim();
    if clean_t.is_empty() {
        return None;
    }
    let sf = if storefront.trim().is_empty() { "cn" } else { storefront.trim() };
    let primary_artist = artist
        .split(&['/', ',', '&', '、', '，'][..])
        .next()
        .unwrap_or(artist)
        .trim();
    let q = if primary_artist.is_empty() {
        clean_t.to_string()
    } else {
        format!("{clean_t} {primary_artist}")
    };
    let enc: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
    let path = format!("v1/catalog/{sf}/search?term={enc}&types=songs&limit=8&l=zh-Hans-CN");
    let json = fetch_am_api(path, user_token, None, Some(sf.to_string())).await.ok()?;
    let arr = json.pointer("/results/songs/data").and_then(Value::as_array)?;
    let mut best: Option<(f32, crate::am_agent::AgentTrackInput)> = None;
    for node in arr {
        let Some(tr) = crate::am_agent::parse_track_from_am_json(node, None, None) else {
            continue;
        };
        let sc = crate::backends::match_track_score(
            clean_t,
            artist,
            duration_sec,
            &tr.name,
            &tr.artist_name,
            tr.duration_sec,
        );
        if sc >= 0.54 && best.as_ref().is_none_or(|(bs, _)| sc > *bs) {
            best = Some((sc, tr));
        }
    }
    best.map(|(_, tr)| tr)
}

/// Apple Music `POST /v1/me/stations/next-tracks/{station_id}` strictly enforces `limit <= 10`
/// (any larger value returns HTTP 400 `Invalid Parameter Value`).
pub const STATION_NEXT_TRACKS_MAX_LIMIT: usize = 10;

#[inline]
pub fn clamp_station_next_tracks_limit(limit: usize) -> usize {
    limit.clamp(1, STATION_NEXT_TRACKS_MAX_LIMIT)
}

/// Fetches the next playable track queue for an Apple Music radio/personal/mood station via
/// `POST /v1/me/stations/next-tracks/{station_id}` so stations play natively via `AMPLibraryAgent`
/// without launching the official Apple Music desktop GUI.
pub async fn fetch_station_next_tracks(
    station_id: &str,
    user_token: Option<String>,
    storefront_hint: Option<String>,
    limit: usize,
) -> Result<Value, String> {
    let clean_id = station_id.trim();
    if clean_id.is_empty() {
        return Err("电台 ID 不能为空".to_string());
    }
    let normalized_user = user_token
        .as_deref()
        .map(normalize_user_token)
        .unwrap_or_default();
    let lim = clamp_station_next_tracks_limit(limit);
    let url = format!(
        "{AMP_API_BASE}v1/me/stations/next-tracks/{clean_id}?limit={lim}&extend=audioTraits&l=zh-Hans-CN"
    );

    let dev = get_public_dev_token(false, storefront_hint.as_deref()).await?;
    let (mut status, mut body) = send_amp_post(&url, &dev.token, &normalized_user).await?;

    if status.as_u16() == 401 {
        if let Ok(fresh) = get_public_dev_token(true, storefront_hint.as_deref()).await {
            let retry = send_amp_post(&url, &fresh.token, &normalized_user).await?;
            status = retry.0;
            body = retry.1;
        }
    }

    if !status.is_success() {
        return Err(format_apple_api_error(status, &body));
    }

    serde_json::from_str::<Value>(&body)
        .map_err(|e| format!("解析电台曲目列表 JSON 失败: {e}"))
}

pub async fn add_to_library(
    kind: &str,
    id: &str,
    user_token: Option<String>,
    storefront_hint: Option<String>,
) -> Result<(), String> {
    let normalized_user = user_token
        .as_deref()
        .map(normalize_user_token)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "添加到 Apple Music 资料库需要有效的 Media-User-Token，请先在设置中登录绑定 Cookie"
                .to_string()
        })?;

    let resource_type = match kind.trim() {
        "song" | "songs" | "library-songs" => "songs",
        "album" | "albums" | "library-albums" => "albums",
        "playlist" | "playlists" | "library-playlists" => "playlists",
        other => return Err(format!("不支持添加到资料库的类型: {other}")),
    };

    let clean_id = id.trim();
    if clean_id.is_empty()
        || !clean_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err("无效的曲目或合集 ID".to_string());
    }

    let url = format!("{AMP_API_BASE}v1/me/library?ids[{resource_type}]={clean_id}");
    let dev = get_public_dev_token(false, storefront_hint.as_deref()).await?;
    let (mut status, mut body) = send_amp_post(&url, &dev.token, &normalized_user).await?;

    if status == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_dev_token_cache();
        if let Ok(fresh) = get_public_dev_token(true, storefront_hint.as_deref()).await {
            let retry = send_amp_post(&url, &fresh.token, &normalized_user).await?;
            status = retry.0;
            body = retry.1;
        }
    }

    if !status.is_success() {
        return Err(format_apple_api_error(status, &body));
    }

    Ok(())
}

pub async fn fetch_enhanced_hls_manifest(m3u8_url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(m3u8_url.trim())
        .map_err(|e| format!("无效的 HLS 清单地址: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("仅允许 HTTPS HLS 清单地址".to_string());
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if !host.ends_with(".apple.com") && !host.ends_with(".mzstatic.com") {
        return Err("仅允许请求 Apple 官方音频清单域名".to_string());
    }

    let resp = HTTP_CLIENT
        .get(parsed.as_str())
        .send()
        .await
        .map_err(|e| format!("读取 HLS 音质清单失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HLS 清单 HTTP {}", resp.status().as_u16()));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("解码 HLS 清单文本失败: {e}"))?;
    if !text.contains("#EXTM3U") {
        return Err("响应不是有效的 M3U8 清单".to_string());
    }
    Ok(text)
}

pub async fn verify_am_auth(
    user_token: Option<String>,
    storefront: Option<String>,
) -> Result<AuthVerifyResult, String> {
    let dev = match get_public_dev_token(false, storefront.as_deref()).await {
        Ok(d) => d,
        Err(err) => {
            return Ok(AuthVerifyResult {
                dev_token_ok: false,
                dev_token_kid: None,
                dev_token_exp: None,
                has_user_token: false,
                normalized_token_len: 0,
                user_token_ok: false,
                detected_storefront: None,
                subscription_active: None,
                http_status: None,
                message: err,
            });
        }
    };

    let normalized = user_token
        .as_deref()
        .map(normalize_user_token)
        .unwrap_or_default();

    if normalized.is_empty() {
        return Ok(AuthVerifyResult {
            dev_token_ok: true,
            dev_token_kid: dev.kid,
            dev_token_exp: dev.exp,
            has_user_token: false,
            normalized_token_len: 0,
            user_token_ok: false,
            detected_storefront: None,
            subscription_active: None,
            http_status: None,
            message: "Developer JWT 获取正常；未填写 Media-User-Token（仅可浏览排行榜和搜索）"
                .to_string(),
        });
    }

    let account_url = format!("{AMP_API_BASE}v1/me/account?meta=subscription");
    let (mut status, mut body) = send_amp_get(&account_url, &dev.token, Some(&normalized)).await?;

    if status == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_dev_token_cache();
        if let Ok(fresh) = get_public_dev_token(true, storefront.as_deref()).await {
            let retry = send_amp_get(&account_url, &fresh.token, Some(&normalized)).await?;
            status = retry.0;
            body = retry.1;
        }
    }

    if status.is_success() {
        let json: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let detected_storefront = json
            .pointer("/data/0/attributes/storefront/id")
            .or_else(|| json.pointer("/meta/subscription/storefront"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_lowercase());
        let subscription_active = json
            .pointer("/meta/subscription/active")
            .or_else(|| json.pointer("/data/0/attributes/active"))
            .and_then(|v| v.as_bool());

        let msg = match (&detected_storefront, subscription_active) {
            (Some(sf), Some(true)) => format!("验证成功：已连接 Apple Music（区域 {sf}，订阅有效）"),
            (Some(sf), _) => format!("验证成功：已连接 Apple Music 账户（区域 {sf}）"),
            _ => "验证成功：Media-User-Token 有效".to_string(),
        };

        return Ok(AuthVerifyResult {
            dev_token_ok: true,
            dev_token_kid: dev.kid,
            dev_token_exp: dev.exp,
            has_user_token: true,
            normalized_token_len: normalized.len(),
            user_token_ok: true,
            detected_storefront,
            subscription_active,
            http_status: Some(status.as_u16()),
            message: msg,
        });
    }

    // Fallback check against /v1/me/recommendations in case /v1/me/account is restricted
    let rec_url = format!("{AMP_API_BASE}v1/me/recommendations?l=zh-Hans-CN&limit=1");
    let (rec_status, rec_body) = send_amp_get(&rec_url, &dev.token, Some(&normalized)).await?;
    if rec_status.is_success() {
        return Ok(AuthVerifyResult {
            dev_token_ok: true,
            dev_token_kid: dev.kid,
            dev_token_exp: dev.exp,
            has_user_token: true,
            normalized_token_len: normalized.len(),
            user_token_ok: true,
            detected_storefront: None,
            subscription_active: None,
            http_status: Some(rec_status.as_u16()),
            message: "验证成功：已可访问个人推荐与资料库接口".to_string(),
        });
    }

    let detail = format_apple_api_error(rec_status, &rec_body);
    let hint = if rec_status == reqwest::StatusCode::UNAUTHORIZED
        || rec_status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        "Media-User-Token 无效或已过期，请使用下方「网页登录自动获取」或重新复制完整的 media-user-token 值"
    } else {
        "Apple Music 个性化接口返回异常"
    };

    Ok(AuthVerifyResult {
        dev_token_ok: true,
        dev_token_kid: dev.kid,
        dev_token_exp: dev.exp,
        has_user_token: true,
        normalized_token_len: normalized.len(),
        user_token_ok: false,
        detected_storefront: None,
        subscription_active: None,
        http_status: Some(rec_status.as_u16()),
        message: format!("{hint}（{detail}）"),
    })
}

pub fn convert_to_musics_uri(uri: &str) -> Result<String, String> {
    let trimmed = uri.trim();
    if let Some(rest) = trimmed.strip_prefix("https://") {
        if rest.starts_with("music.apple.com/") || rest == "music.apple.com" {
            return Ok(format!("musics://{rest}"));
        }
        return Err("仅支持打开 music.apple.com 链接".to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("http://") {
        if rest.starts_with("music.apple.com/") || rest == "music.apple.com" {
            return Ok(format!("musics://{rest}"));
        }
        return Err("仅支持打开 music.apple.com 链接".to_string());
    }
    if trimmed.starts_with("musics://") {
        return Ok(trimmed.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("music://") {
        return Ok(format!("musics://{rest}"));
    }
    Err(format!("不支持的协议 URI: {trimmed}"))
}

#[cfg(target_os = "windows")]
pub fn launch_official_apple_music_uri(uri: &str) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Foundation::Uri;
    use windows::System::{Launcher, LauncherOptions};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let musics_uri = convert_to_musics_uri(uri)?;

    struct WinrtComGuard(bool);
    impl Drop for WinrtComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    let winrt_attempt = (|| -> Result<bool, String> {
        let _com_guard = unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = crate::am_agent::init_process_com_security();
            WinrtComGuard(hr.0 == 0 || hr.0 == 1)
        };
        let target = Uri::CreateUri(&HSTRING::from(&musics_uri))
            .map_err(|e| format!("构造 URI 失败: {e}"))?;
        let options = LauncherOptions::new().map_err(|e| format!("创建 LauncherOptions 失败: {e}"))?;
        options
            .SetTargetApplicationPackageFamilyName(&HSTRING::from(OFFICIAL_AM_PFN))
            .map_err(|e| format!("设置目标 Apple Music 包名失败: {e}"))?;
        let launched = Launcher::LaunchUriWithOptionsAsync(&target, &options)
            .map_err(|e| format!("调用 Launcher 失败: {e}"))?
            .join()
            .map_err(|e| format!("等待 Launcher 返回失败: {e}"))?;
        Ok(launched)
    })();

    if matches!(winrt_attempt, Ok(true)) {
        return Ok(());
    }

    // Fallback to ShellExecuteW if the Store Apple Music package is not registered under that PFN
    let wide_uri: Vec<u16> = musics_uri.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_open: Vec<u16> = "open\0".encode_utf16().collect();
    let hinst = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(wide_open.as_ptr()),
            PCWSTR(wide_uri.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (hinst.0 as isize) <= 32 {
        let winrt_msg = match winrt_attempt {
            Err(e) => format!("（定向拉起失败: {e}）"),
            Ok(false) => "（未检测到官方 Apple Music UWP 包）".to_string(),
            Ok(true) => String::new(),
        };
        return Err(format!(
            "拉起 Apple Music 失败（ShellExecuteW={}）{winrt_msg}",
            hinst.0 as isize
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn launch_official_apple_music_uri(_uri: &str) -> Result<(), String> {
    Err("仅支持 Windows 平台".to_string())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialClientAccountInfo {
    pub signed_in: bool,
    pub dsid: Option<String>,
    pub storefront: Option<String>,
    pub subscription_active: bool,
    pub display_name: Option<String>,
    pub library_name: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LegacyAmLightCredentials {
    pub user_token: Option<String>,
    pub storefront: Option<String>,
}

pub fn apple_storefront_id_to_code(raw: &str) -> Option<String> {
    let digits: String = raw
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let code = match digits.as_str() {
        "143465" => "cn",
        "143462" => "jp",
        "143441" => "us",
        "143463" => "hk",
        "143470" => "tw",
        "143444" => "gb",
        "143464" => "sg",
        "143466" => "kr",
        "143460" => "au",
        "143455" => "ca",
        "143443" => "de",
        "143442" => "fr",
        "143450" => "it",
        "143454" => "es",
        "143452" => "nl",
        "143459" => "ch",
        "143467" => "in",
        "143473" => "my",
        "143475" => "th",
        "143474" => "ph",
        "143476" => "id",
        "143471" => "vn",
        "143503" => "br",
        "143468" => "mx",
        _ => return None,
    };
    Some(code.to_string())
}

fn normalize_cjk_profile_name(raw: &str) -> String {
    let trimmed = raw.trim();
    for honorific in ["先生", "女士", "同学", "老师"] {
        if let Some(rest) = trimmed.strip_prefix(honorific) {
            let surname = rest.trim();
            if !surname.is_empty() && surname.chars().count() <= 3 {
                return format!("{surname}{honorific}");
            }
        }
    }
    trimmed.to_string()
}

/// Reads the signed-in Apple Music desktop client (`AppleInc.AppleMusicWin_nzyj5cx40ttqa`)
/// account metadata, active subscription flag, storefront region, and profile display name.
pub fn detect_official_am_account() -> OfficialClientAccountInfo {
    let mut info = OfficialClientAccountInfo::default();

    let user_profile = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
    let local_appdata = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);

    // 1. PlayActivityFeed.plist in user's Apple Music Library (`DSID`, `store-front`, `sb-enabled`)
    if let Some(ref home) = user_profile {
        let feed_path = home
            .join("Music")
            .join("Apple Music")
            .join("Apple Music Library.musiclibrary")
            .join("preferences")
            .join("PlayActivityFeed.plist");
        if let Ok(bytes) = std::fs::read(&feed_path) {
            if let Ok(plist) = crate::bplist::decode_bplist(&bytes) {
                if let Some(dict) = plist.as_dict() {
                    for (k, v) in dict {
                        if k.len() >= 5 && k.chars().all(|c| c.is_ascii_digit()) {
                            info.signed_in = true;
                            if info.dsid.is_none() {
                                info.dsid = Some(k.clone());
                            }
                            if let Some(events) = v.get("events").and_then(|e| e.as_array()) {
                                for ev in events.iter().rev() {
                                    if ev.get("sb-enabled").and_then(|b| b.as_bool()) == Some(true)
                                    {
                                        info.subscription_active = true;
                                    }
                                    if info.storefront.is_none() {
                                        if let Some(sf_raw) =
                                            ev.get("store-front").and_then(|s| s.as_str())
                                        {
                                            info.storefront = apple_storefront_id_to_code(sf_raw);
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

    if let Some(ref local) = local_appdata {
        let pkg_root = local.join("Packages").join(OFFICIAL_AM_PFN);

        // 2. SubscriptionPlayCache/PlayCacheInfo.xml (`cloud-dsid`)
        let play_cache_xml = pkg_root
            .join("LocalCache")
            .join("Local")
            .join("Apple")
            .join("AMPLibraryAgent")
            .join("SubscriptionPlayCache")
            .join("PlayCacheInfo.xml");
        if let Ok(xml) = std::fs::read_to_string(&play_cache_xml) {
            if let Some(pos) = xml.find("<key>cloud-dsid</key>") {
                let tail = &xml[pos + "<key>cloud-dsid</key>".len()..];
                if let (Some(i0), Some(i1)) = (tail.find("<integer>"), tail.find("</integer>")) {
                    let start = i0 + "<integer>".len();
                    if i1 > start {
                        let dsid = tail[start..i1].trim();
                        if !dsid.is_empty()
                            && dsid != "0"
                            && dsid.chars().all(|c| c.is_ascii_digit())
                        {
                            info.signed_in = true;
                            info.subscription_active = true;
                            if info.dsid.is_none() {
                                info.dsid = Some(dsid.to_string());
                            }
                        }
                    }
                }
            }
        }

        // 3. Cached MusicUI social profile JSON (`AC/INetCache/*/profile*.json`)
        let inet_cache = pkg_root.join("AC").join("INetCache");
        if let Ok(subdirs) = std::fs::read_dir(&inet_cache) {
            let mut profile_files: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
            for sub in subdirs.flatten() {
                if let Ok(entries) = std::fs::read_dir(sub.path()) {
                    for ent in entries.flatten() {
                        let p = ent.path();
                        let fname = p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        if fname.starts_with("profile") && fname.ends_with(".json") {
                            let mtime = ent
                                .metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::UNIX_EPOCH);
                            profile_files.push((mtime, p));
                        }
                    }
                }
            }
            profile_files.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, p) in profile_files {
                if let Ok(txt) = std::fs::read_to_string(&p) {
                    if let Ok(json) = serde_json::from_str::<Value>(&txt) {
                        if info.display_name.is_none() {
                            if let Some(name) = json
                                .pointer("/resources/personal-social-profiles/me/attributes/name")
                                .and_then(Value::as_str)
                            {
                                let clean = normalize_cjk_profile_name(name);
                                if !clean.is_empty() {
                                    info.display_name = Some(clean);
                                    info.signed_in = true;
                                }
                            }
                        }
                        if info.storefront.is_none() {
                            if let Some(href) = json
                                .pointer("/resources/personal-social-profiles/me/relationships/social-profile/data/0/href")
                                .and_then(Value::as_str)
                            {
                                // Format: "/v1/social/jp/social-profiles/..."
                                if let Some(rest) = href.strip_prefix("/v1/social/") {
                                    if let Some(sf) = rest.split('/').next() {
                                        let sf_low = sf.trim().to_ascii_lowercase();
                                        if (2..=4).contains(&sf_low.len())
                                            && sf_low.chars().all(|c| c.is_ascii_alphabetic())
                                        {
                                            info.storefront = Some(sf_low);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. MediaSharing preferences (`shared-library-name`)
        let sharing_plist = pkg_root
            .join("LocalCache")
            .join("Roaming")
            .join("Apple Computer")
            .join("Preferences")
            .join("com.apple.amp.mediasharingd.plist");
        if let Ok(bytes) = std::fs::read(&sharing_plist) {
            if let Ok(plist) = crate::bplist::decode_bplist(&bytes) {
                if let Some(lib_name) = plist.get("shared-library-name").and_then(|v| v.as_str()) {
                    let trimmed = lib_name.trim();
                    if !trimmed.is_empty() {
                        info.library_name = Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    info
}

fn decode_leveldb_varint(buf: &[u8]) -> Option<(usize, usize)> {
    let mut result: usize = 0;
    let mut shift = 0usize;
    for (i, &b) in buf.iter().take(5).enumerate() {
        result |= ((b & 0x7F) as usize) << shift;
        if (b & 0x80) == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}

fn extract_leveldb_dom_value_after_key(raw: &[u8], key: &[u8]) -> Option<String> {
    let mut latest: Option<String> = None;
    let mut search_from = 0usize;
    while search_from + key.len() < raw.len() {
        let Some(rel) = raw[search_from..]
            .windows(key.len())
            .position(|w| w == key)
        else {
            break;
        };
        let after_key = search_from + rel + key.len();
        search_from = after_key;

        if let Some((val_len, varint_bytes)) = decode_leveldb_varint(&raw[after_key..]) {
            let val_start = after_key + varint_bytes;
            if (2..=4096).contains(&val_len) && val_start + val_len <= raw.len() {
                let fmt = raw[val_start];
                let payload = &raw[val_start + 1..val_start + val_len];
                let decoded = match fmt {
                    0x01 => Some(String::from_utf8_lossy(payload).into_owned()),
                    0x00 if payload.len() % 2 == 0 => {
                        let u16s: Vec<u16> = payload
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        Some(String::from_utf16_lossy(&u16s))
                    }
                    _ => None,
                };
                if let Some(s) = decoded {
                    let clean = s.trim().to_string();
                    if !clean.is_empty() {
                        latest = Some(clean);
                    }
                }
            }
        }
    }
    latest
}

/// Automatically migrates saved `am_user_token` and `am_storefront` from the previous
/// Tauri/WebView2 `am-light` installation (`%LOCALAPPDATA%\com.local.amlight\EBWebView\Default\Local Storage\leveldb`).
pub fn import_legacy_amlight_credentials() -> LegacyAmLightCredentials {
    let mut out = LegacyAmLightCredentials::default();
    let mut candidate_dirs = Vec::new();
    for env_key in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(base) = std::env::var_os(env_key).map(std::path::PathBuf::from) {
            for folder in ["com.local.amlight", "am-light"] {
                candidate_dirs.push(
                    base.join(folder)
                        .join("EBWebView")
                        .join("Default")
                        .join("Local Storage")
                        .join("leveldb"),
                );
            }
        }
    }

    for dir in candidate_dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let ext = p
                    .extension()
                    .and_then(|x| x.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if ext == "log" || ext == "ldb" {
                    let mt = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    Some((mt, p))
                } else {
                    None
                }
            })
            .collect();
        // Sort oldest -> newest so newer log entries overwrite older ones
        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        for (_, fp) in files {
            let Ok(raw) = std::fs::read(&fp) else {
                continue;
            };
            if let Some(tok_raw) = extract_leveldb_dom_value_after_key(&raw, b"am_user_token") {
                let norm = normalize_user_token(&tok_raw);
                if looks_like_media_user_token(&norm) {
                    out.user_token = Some(norm);
                }
            }
            if let Some(sf_raw) = extract_leveldb_dom_value_after_key(&raw, b"am_storefront") {
                let sf = sf_raw.trim().to_ascii_lowercase();
                if (2..=4).contains(&sf.len()) && sf.chars().all(|c| c.is_ascii_alphabetic()) {
                    out.storefront = Some(sf);
                }
            }
        }
    }

    out
}

pub fn looks_like_media_user_token(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.to_ascii_lowercase().contains("media-user-token") {
        return !normalize_user_token(trimmed).is_empty();
    }
    let norm = normalize_user_token(trimmed);
    if norm.len() < 56 || norm.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    norm.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '.' | '-' | '_' | ':'))
}

pub fn mask_user_token_preview(token: &str) -> String {
    let t = token.trim();
    if t.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= 18 {
        return format!("{}••••", chars.iter().take(4).collect::<String>());
    }
    let head: String = chars[..10].iter().collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}••••••••{tail}")
}

// ============================================================================
// Official Apple Music Web SSR Shelves (`/new`, `/radio`) & Motion Video URLs
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorialShelfKind {
    HomeTopPicks,
    NewFeatureHero,
    HeroFlowcase,
    TrackGrid,
    SquareShelf,
    RadioStationRow,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EditorialItem {
    pub id: String,
    pub kind: String,
    pub play_kind: String,
    pub title: String,
    pub subtitle: String,
    pub badge: String,
    pub artwork_url: String,
    pub motion_video_url: Option<String>,
    #[serde(default)]
    pub motion_tall_video_url: Option<String>,
    pub storefront: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorialShelf {
    pub title: String,
    pub subtitle: String,
    pub kind: EditorialShelfKind,
    pub items: Vec<EditorialItem>,
}

fn fill_artwork_template(raw: &str, w: u32, h: u32) -> String {
    if raw.is_empty() {
        return String::new();
    }
    raw.replace("{w}", &w.to_string())
        .replace("{h}", &h.to_string())
        .replace("{c}", "")
        .replace("{f}", "jpg")
}

fn normalize_editorial_kind(raw: &str) -> (String, String) {
    let low = raw.trim().to_ascii_lowercase();
    match low.as_str() {
        "song" | "songs" | "track" => ("song".to_string(), "songs".to_string()),
        "album" | "albums" => ("album".to_string(), "albums".to_string()),
        "playlist" | "playlists" | "curator" => ("playlist".to_string(), "playlists".to_string()),
        "station" | "stations" | "radiostation" | "radio-station" | "radio-stations"
        | "apple-radio" | "broadcast" => ("station".to_string(), "stations".to_string()),
        "music-video" | "musicvideo" | "music-videos" => {
            ("music-video".to_string(), "music-videos".to_string())
        }
        other => (other.to_string(), other.to_string()),
    }
}

pub fn extract_editorial_motion_video_from_attrs(attrs: &Value) -> Option<String> {
    let ev = attrs.get("editorialVideo").or_else(|| attrs.pointer("/editorialVideo"))?;
    for slot in [
        "motionSquareVideo1x1",
        "motionDetailSquare",
        "motionDetailTall",
        "motionTallVideo3x4",
        "motionDetailHorizontal",
    ] {
        if let Some(v) = ev.pointer(&format!("/{slot}/video")).and_then(Value::as_str) {
            let trimmed = v.trim();
            if !trimmed.is_empty() && (trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn extract_editorial_tall_motion_video_from_attrs(attrs: &Value) -> Option<String> {
    let ev = attrs.get("editorialVideo").or_else(|| attrs.pointer("/editorialVideo"))?;
    for slot in [
        "motionTallVideo3x4",
        "motionDetailTall",
        "motionSquareVideo1x1",
        "motionDetailSquare",
    ] {
        if let Some(v) = ev.pointer(&format!("/{slot}/video")).and_then(Value::as_str) {
            let trimmed = v.trim();
            if !trimmed.is_empty() && (trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_editorial_motion_video_from_node(node: &Value) -> Option<String> {
    if let Some(u) = extract_editorial_motion_video_from_attrs(node) {
        return Some(u);
    }
    if let Some(attrs) = node.get("attributes") {
        if let Some(u) = extract_editorial_motion_video_from_attrs(attrs) {
            return Some(u);
        }
    }
    for key in [
        "motionDetailSquare",
        "motionSquareVideo1x1",
        "motionDetailTall",
        "video",
    ] {
        if let Some(v) = node.pointer(&format!("/{key}/video")).and_then(Value::as_str) {
            let t = v.trim();
            if t.starts_with("http://") || t.starts_with("https://") {
                return Some(t.to_string());
            }
        }
        if let Some(v) = node.get(key).and_then(Value::as_str) {
            let t = v.trim();
            if (t.starts_with("http://") || t.starts_with("https://")) && t.contains(".m3u8") {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn extract_web_item_descriptor(item: &Value) -> (String, String, String) {
    for ptr in [
        "/contentDescriptor",
        "/playAction/contentDescriptor",
        "/link/segue/destination/contentDescriptor",
        "/overlay/playAction/contentDescriptor",
    ] {
        if let Some(cd) = item.pointer(ptr) {
            let raw_kind = cd.get("kind").and_then(Value::as_str).unwrap_or("");
            let id = cd
                .pointer("/identifiers/storeAdamID")
                .or_else(|| cd.pointer("/identifiers/cloudLibraryID"))
                .or_else(|| cd.get("id"))
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                })
                .unwrap_or_default();
            if !id.is_empty() {
                let (k, pk) = normalize_editorial_kind(raw_kind);
                return (id, k, pk);
            }
        }
    }
    if let Some(id) = item
        .pointer("/identifiers/storeAdamID")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
    {
        let raw_kind = item.get("type").and_then(Value::as_str).unwrap_or("album");
        let (k, pk) = normalize_editorial_kind(raw_kind);
        return (id.to_string(), k, pk);
    }
    (String::new(), String::new(), String::new())
}

fn extract_web_artwork_url(item: &Value, is_wide: bool) -> String {
    let raw = item
        .pointer("/artwork/dictionary/url")
        .or_else(|| item.pointer("/artwork/url"))
        .or_else(|| item.pointer("/editorialArtwork/superHeroWide/url"))
        .or_else(|| item.pointer("/editorialArtwork/subscriptionHero/url"))
        .or_else(|| item.pointer("/attributes/artwork/url"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if is_wide {
        fill_artwork_template(raw, 960, 540)
    } else {
        fill_artwork_template(raw, 600, 600)
    }
}

pub fn parse_serialized_server_data_shelves(html: &str, storefront: &str) -> Vec<EditorialShelf> {
    let Some(id_pos) = html.find("id=\"serialized-server-data\"") else {
        return Vec::new();
    };
    let Some(gt_rel) = html[id_pos..].find('>') else {
        return Vec::new();
    };
    let json_start = id_pos + gt_rel + 1;
    let Some(end_rel) = html[json_start..].find("</script>") else {
        return Vec::new();
    };
    let json_str = html[json_start..json_start + end_rel].trim();
    let Ok(root) = serde_json::from_str::<Value>(json_str) else {
        return Vec::new();
    };

    let sf = if storefront.trim().is_empty() {
        "cn".to_string()
    } else {
        storefront.trim().to_ascii_lowercase()
    };

    let mut shelves = Vec::new();
    let Some(data_arr) = root.get("data").and_then(Value::as_array) else {
        return shelves;
    };

    for page_entry in data_arr {
        let Some(sections) = page_entry
            .pointer("/data/sections")
            .and_then(Value::as_array)
        else {
            continue;
        };

        for sec in sections {
            let sec_kind_hint = sec.get("itemKind").and_then(Value::as_str).unwrap_or("");
            let title = sec
                .pointer("/header/item/titleLink/title")
                .or_else(|| sec.pointer("/header/item/title"))
                .or_else(|| sec.pointer("/header/title"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let shelf_sub = sec
                .pointer("/header/item/subtitle")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();

            let Some(raw_items) = sec.get("items").and_then(Value::as_array) else {
                continue;
            };
            if raw_items.is_empty() {
                continue;
            }

            let first_ik = raw_items
                .first()
                .and_then(|x| x.get("itemKind").and_then(Value::as_str))
                .unwrap_or(sec_kind_hint);
            let shelf_kind = match first_ik {
                "flowcaseLockup" | "heroLockup" | "superHeroBanner" => {
                    EditorialShelfKind::NewFeatureHero
                }
                "trackLockup" | "songLockup" => EditorialShelfKind::TrackGrid,
                "stationLockup" | "liveRadioLockup" | "radioStationLockup" => {
                    EditorialShelfKind::RadioStationRow
                }
                _ => {
                    if title.contains("广播") || title.contains("电台") || title.contains("Radio") {
                        EditorialShelfKind::RadioStationRow
                    } else {
                        EditorialShelfKind::SquareShelf
                    }
                }
            };
            let is_wide = matches!(
                shelf_kind,
                EditorialShelfKind::HeroFlowcase | EditorialShelfKind::NewFeatureHero
            );

            let mut parsed_items = Vec::new();
            for it in raw_items.iter().take(24) {
                let it_title = it
                    .get("title")
                    .or_else(|| it.get("heading"))
                    .or_else(|| it.pointer("/titleLink/title"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let mut sub = it
                    .get("subtitle")
                    .or_else(|| it.get("subheading"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if sub.is_empty() {
                    if let Some(links) = it.get("subtitleLinks").and_then(Value::as_array) {
                        let names: Vec<&str> = links
                            .iter()
                            .filter_map(|l| l.get("title").and_then(Value::as_str))
                            .collect();
                        sub = names.join("、");
                    }
                }
                let badge = it
                    .get("heading")
                    .or_else(|| it.get("badge"))
                    .or_else(|| it.get("editorialBadge"))
                    .or_else(|| it.get("eyebrow"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();

                let (mut id, mut kind, mut play_kind) = extract_web_item_descriptor(it);
                if id.is_empty() && is_wide {
                    // Promo banner may link via action URL or child lockup
                    if let Some(url) = it.pointer("/link/segue/destination/url").and_then(Value::as_str) {
                        if let Some(seg) = url.trim_end_matches('/').rsplit('/').next() {
                            id = seg.to_string();
                        }
                    }
                }
                if kind.is_empty() {
                    if shelf_kind == EditorialShelfKind::TrackGrid {
                        kind = "song".to_string();
                        play_kind = "songs".to_string();
                    } else if shelf_kind == EditorialShelfKind::RadioStationRow {
                        kind = "station".to_string();
                        play_kind = "stations".to_string();
                    } else {
                        kind = "album".to_string();
                        play_kind = "albums".to_string();
                    }
                }
                let artwork_url = extract_web_artwork_url(it, is_wide);
                let motion_video_url = extract_editorial_motion_video_from_node(it);

                if it_title.is_empty() && artwork_url.is_empty() {
                    continue;
                }
                if id.is_empty() && !is_wide {
                    continue;
                }

                parsed_items.push(EditorialItem {
                    id,
                    kind: kind.clone(),
                    play_kind,
                    title: if it_title.is_empty() {
                        badge.clone()
                    } else {
                        it_title
                    },
                    subtitle: sub,
                    badge: if is_wide && !badge.is_empty() {
                        badge
                    } else if kind == "station" {
                        "LIVE".to_string()
                    } else {
                        String::new()
                    },
                    artwork_url,
                    motion_video_url,
                    motion_tall_video_url: None,
                    storefront: sf.clone(),
                });
            }

            if !parsed_items.is_empty() {
                shelves.push(EditorialShelf {
                    title: if title.is_empty() {
                        match shelf_kind {
                            EditorialShelfKind::HomeTopPicks => "为你精选".to_string(),
                            EditorialShelfKind::NewFeatureHero | EditorialShelfKind::HeroFlowcase => {
                                String::new()
                            }
                            EditorialShelfKind::TrackGrid => "热门新歌".to_string(),
                            EditorialShelfKind::RadioStationRow => "Apple Music 广播电台".to_string(),
                            EditorialShelfKind::SquareShelf => "推荐精选".to_string(),
                        }
                    } else {
                        title
                    },
                    subtitle: shelf_sub,
                    kind: shelf_kind,
                    items: parsed_items,
                });
            }
        }
    }

    shelves
}

pub async fn fetch_apple_music_web_shelves(
    storefront: &str,
    page: &str,
) -> Result<Vec<EditorialShelf>, String> {
    let sf = if storefront.trim().is_empty() {
        "cn"
    } else {
        storefront.trim()
    };
    let clean_page = match page.trim() {
        "radio" => "radio",
        _ => "new",
    };
    let url = format!("{WEB_ORIGIN}/{sf}/{clean_page}");
    let resp = HTTP_CLIENT
        .get(&url)
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await
        .map_err(|e| format!("拉取 Apple Music 官方网页失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Apple Music 官方网页 HTTP {}", resp.status()));
    }
    let html = resp
        .text()
        .await
        .map_err(|e| format!("读取官方网页内容失败: {e}"))?;
    let mut shelves = parse_serialized_server_data_shelves(&html, sf);
    // Official Browse (/new) & Radio (/radio) use crisp static artwork; dynamic covers are Home & Lyrics only
    for sh in &mut shelves {
        for it in &mut sh.items {
            it.motion_video_url = None;
            it.motion_tall_video_url = None;
        }
    }
    if shelves.is_empty() {
        return Err("未能从官方网页解析出推荐栏目".to_string());
    }
    Ok(shelves)
}

fn parse_amp_resource_to_editorial_item(v: &Value, sf: &str, artwork_dim: u32) -> Option<EditorialItem> {
    let id = v.get("id").and_then(Value::as_str)?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let raw_type = v.get("type").and_then(Value::as_str).unwrap_or("albums");
    let (kind, play_kind) = normalize_editorial_kind(raw_type);
    let attrs = v.get("attributes")?;
    let title = attrs
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }
    let subtitle = attrs
        .get("artistName")
        .or_else(|| attrs.get("curatorName"))
        .or_else(|| attrs.pointer("/editorialNotes/short"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let subtitle = if subtitle.is_empty() && kind == "station" {
        "广播电台".to_string()
    } else {
        subtitle
    };
    let badge = if attrs
        .get("audioTraits")
        .and_then(Value::as_array)
        .map(|a| a.iter().any(|t| t.as_str() == Some("atmos")))
        .unwrap_or(false)
    {
        "空间音频".to_string()
    } else if kind == "station" {
        "广播电台".to_string()
    } else {
        String::new()
    };
    let raw_art = attrs
        .pointer("/artwork/url")
        .and_then(Value::as_str)
        .unwrap_or("");
    let artwork_url = fill_artwork_template(raw_art, artwork_dim, artwork_dim);
    let motion_video_url = extract_editorial_motion_video_from_attrs(attrs);
    let motion_tall_video_url = extract_editorial_tall_motion_video_from_attrs(attrs);
    Some(EditorialItem {
        id,
        kind,
        play_kind,
        title,
        subtitle,
        badge,
        artwork_url,
        motion_video_url,
        motion_tall_video_url,
        storefront: sf.to_string(),
    })
}

/// Batch-verifies `editorialVideo` availability once for ALL Home items (albums, playlists,
/// personal stations, and mood stations) whose initial response did not inline `editorialVideo`.
async fn confirm_home_shelves_motion_artwork(
    shelves: &mut [EditorialShelf],
    sf: &str,
    utok: Option<&str>,
) {
    let mut album_ids: Vec<String> = Vec::new();
    let mut playlist_ids: Vec<String> = Vec::new();
    let mut station_ids: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for sh in shelves.iter() {
        for it in &sh.items {
            if it.motion_video_url.is_some()
                && (sh.kind != EditorialShelfKind::HomeTopPicks
                    || it.motion_tall_video_url.is_some())
            {
                continue;
            }
            let id = it.id.trim();
            if id.is_empty()
                || id.starts_with("i.")
                || id.starts_with("l.")
                || id.starts_with("p.")
            {
                continue;
            }
            let dedup_key = format!("{}:{}", it.play_kind, id);
            if !seen.insert(dedup_key) {
                continue;
            }
            match it.play_kind.as_str() {
                "albums" => album_ids.push(id.to_string()),
                "playlists" => playlist_ids.push(id.to_string()),
                "stations" => station_ids.push(id.to_string()),
                _ => {}
            }
        }
    }

    let mut found_sq: HashMap<String, String> = HashMap::new();
    let mut found_tall: HashMap<String, String> = HashMap::new();

    let mut batch_tasks = Vec::new();
    for (resource, ids) in [
        ("stations", station_ids),
        ("playlists", playlist_ids),
        ("albums", album_ids),
    ] {
        for chunk in ids.chunks(25) {
            if chunk.is_empty() {
                continue;
            }
            let joined = chunk.join(",");
            let encoded_ids: String = url::form_urlencoded::byte_serialize(joined.as_bytes()).collect();
            let path = format!(
                "v1/catalog/{sf}/{resource}?ids={encoded_ids}&extend=editorialVideo&l=zh-Hans-CN"
            );
            let tok_opt = utok.map(str::to_string);
            let sf_s = sf.to_string();
            batch_tasks.push(tokio::spawn(async move {
                fetch_am_api(path, tok_opt, None, Some(sf_s)).await.ok()
            }));
        }
    }

    for task in batch_tasks {
        if let Ok(Some(json)) = task.await {
            if let Some(arr) = json.get("data").and_then(Value::as_array) {
                for entry in arr {
                    let Some(id) = entry.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(attrs) = entry.get("attributes") else {
                        continue;
                    };
                    if let Some(sq) = extract_editorial_motion_video_from_attrs(attrs) {
                        found_sq.insert(id.to_string(), sq);
                    }
                    if let Some(tl) = extract_editorial_tall_motion_video_from_attrs(attrs) {
                        found_tall.insert(id.to_string(), tl);
                    }
                }
            }
        }
    }

    if found_sq.is_empty() && found_tall.is_empty() {
        return;
    }
    for sh in shelves.iter_mut() {
        for it in &mut sh.items {
            if it.motion_video_url.is_none() {
                if let Some(u) = found_sq.get(&it.id) {
                    it.motion_video_url = Some(u.clone());
                }
            }
            if it.motion_tall_video_url.is_none() {
                if let Some(u) = found_tall.get(&it.id) {
                    it.motion_tall_video_url = Some(u.clone());
                }
            }
        }
    }
}

pub async fn fetch_home_editorial_shelves(
    storefront: &str,
    user_token: Option<&str>,
) -> Result<Vec<EditorialShelf>, String> {
    let sf = if storefront.trim().is_empty() {
        "cn"
    } else {
        storefront.trim()
    };
    let utok = user_token
        .map(normalize_user_token)
        .filter(|s| !s.is_empty());

    let mut shelves: Vec<EditorialShelf> = Vec::new();
    let loc = crate::i18n::am_api_locale();

    if let Some(ref tok) = utok {
        // Official Apple Music Personalized Home: Recommendations (up to 20 groups: Top Picks, Mixes,
        // Recently Played, New Releases, Stations / Personal Station, Mood Stations) + Recent Played!
        let rec_path = format!("v1/me/recommendations?l={loc}&limit=20&extend=editorialVideo&extend%5Balbums%5D=editorialVideo&extend%5Bplaylists%5D=editorialVideo&extend%5Bstations%5D=editorialVideo");
        let recent_path = format!("v1/me/recent/played?l={loc}&limit=12&extend=editorialVideo&extend%5Balbums%5D=editorialVideo&extend%5Bplaylists%5D=editorialVideo&extend%5Bstations%5D=editorialVideo");
        let tok1 = tok.clone();
        let sf1 = sf.to_string();
        let rec_task = tokio::spawn(async move {
            fetch_am_api(rec_path, Some(tok1), None, Some(sf1)).await
        });
        let tok2 = tok.clone();
        let sf2 = sf.to_string();
        let recent_task = tokio::spawn(async move {
            fetch_am_api(recent_path, Some(tok2), None, Some(sf2)).await
        });
        let rec_res = rec_task
            .await
            .unwrap_or_else(|e| Err(format!("rec task join err: {e}")));
        let recent_res = recent_task
            .await
            .unwrap_or_else(|e| Err(format!("recent task join err: {e}")));

        let mut rec_groups: Vec<(String, Vec<EditorialItem>)> = Vec::new();
        let mut recent_items: Vec<EditorialItem> = Vec::new();

        if let Ok(rec_json) = rec_res {
            if let Some(groups) = rec_json.get("data").and_then(Value::as_array) {
                for g in groups.iter().take(18) {
                    let g_title = g
                        .pointer("/attributes/title/stringForDisplay")
                        .and_then(Value::as_str)
                        .unwrap_or("为你推荐")
                        .trim()
                        .to_string();
                    let items: Vec<EditorialItem> = g
                        .pointer("/relationships/contents/data")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| parse_amp_resource_to_editorial_item(v, sf, 600))
                                .collect()
                        })
                        .unwrap_or_default();
                    if !items.is_empty() {
                        rec_groups.push((g_title, items));
                    }
                }
            }
        }
        if let Ok(rj) = recent_res {
            if let Some(arr) = rj.get("data").and_then(Value::as_array) {
                recent_items = arr
                    .iter()
                    .filter_map(|v| parse_amp_resource_to_editorial_item(v, sf, 600))
                    .collect();
            }
        }

        // Build Shelf 1: `为你精选` (`HomeTopPicks` tall 3:4 portrait cards matching official Apple Music!)
        // Priority: Personal/Mood stations -> Top animated Mixes/Playlists -> Latest Releases / Recent Picks
        let mut top_picks: Vec<EditorialItem> = Vec::new();
        let mut picked_ids = std::collections::HashSet::new();
        let mut push_pick = |it: &EditorialItem, eyebrow: &str, max_n: usize| {
            if top_picks.len() >= max_n || !picked_ids.insert(it.id.clone()) {
                return;
            }
            let mut p = it.clone();
            p.badge = eyebrow.to_string();
            top_picks.push(p);
        };

        // 1. User's personal station & animated stations first (e.g. "xxx的电台", Discovery Station, Mood Stations)
        for (gt, items) in &rec_groups {
            let is_station_group = gt.contains("电台")
                || gt.contains("ステーション")
                || gt.contains("心情")
                || gt.contains("ムード")
                || gt.to_ascii_lowercase().contains("station")
                || gt.to_ascii_lowercase().contains("mood");
            if is_station_group {
                for it in items.iter().filter(|x| x.motion_video_url.is_some() || x.kind == "station").take(2) {
                    let lbl = if gt.contains("心情") || gt.contains("ムード") {
                        "符合心情的音乐"
                    } else {
                        "为你推荐"
                    };
                    push_pick(it, lbl, 8);
                }
            }
        }
        // 2. Top personalized mixes & new releases
        for (gt, items) in &rec_groups {
            if gt.contains("最近播放") || gt.contains("最近の再生") {
                continue;
            }
            let lbl = if gt.contains("新发行") || gt.contains("ニューリリース") {
                "新发行"
            } else if gt.contains("歌单") || gt.contains("Mix") {
                "为你推荐"
            } else {
                gt.as_str()
            };
            if let Some(best) = items
                .iter()
                .find(|x| x.motion_tall_video_url.is_some() || x.motion_video_url.is_some())
                .or_else(|| items.first())
            {
                push_pick(best, lbl, 8);
            }
        }
        // 3. Fill remaining slots up to 6 with animated items from recent/recommendations
        for it in &recent_items {
            if it.motion_video_url.is_some() {
                push_pick(it, "为你推荐", 6);
            }
        }

        if !top_picks.is_empty() {
            shelves.push(EditorialShelf {
                title: "为你精选".to_string(),
                subtitle: "Top Picks for You".to_string(),
                kind: EditorialShelfKind::HomeTopPicks,
                items: top_picks,
            });
        }

        // Build Shelf 2: `最近播放` (prefer recommendations `最近播放` if it already has inline motion, else `v1/me/recent/played`)
        let mut added_recent = false;
        if let Some((_, rec_recent)) = rec_groups
            .iter()
            .find(|(gt, _)| gt.contains("最近播放") || gt.contains("最近の再生"))
        {
            shelves.push(EditorialShelf {
                title: "最近播放".to_string(),
                subtitle: "Recently Played".to_string(),
                kind: EditorialShelfKind::SquareShelf,
                items: rec_recent.clone(),
            });
            added_recent = true;
        } else if !recent_items.is_empty() {
            shelves.push(EditorialShelf {
                title: "最近播放".to_string(),
                subtitle: "Recently Played".to_string(),
                kind: EditorialShelfKind::SquareShelf,
                items: recent_items,
            });
            added_recent = true;
        }

        // Remaining personalized recommendation shelves (Mixes, New Releases, Anime/Genres, Recommended Stations, Moods, Decades...)
        for (gt, items) in rec_groups {
            if added_recent && (gt.contains("最近播放") || gt.contains("最近の再生")) {
                continue;
            }
            if shelves.len() >= 14 {
                break;
            }
            shelves.push(EditorialShelf {
                title: gt,
                subtitle: "Made For You".to_string(),
                kind: EditorialShelfKind::SquareShelf,
                items,
            });
        }
    }

    // Fallback (signed out or empty recommendations): official Charts + Spotlight
    if shelves.is_empty() {
        let charts_path = format!(
            "v1/catalog/{sf}/charts?types=albums,playlists,songs&extend=editorialVideo,editorialArtwork,editorialNotes,audioTraits&limit=18&l={loc}"
        );
        if let Ok(cj) = fetch_am_api(charts_path, utok.clone(), None, Some(sf.to_string())).await {
            let mut picks: Vec<EditorialItem> = Vec::new();
            let mut chart_shelves: Vec<EditorialShelf> = Vec::new();

            if let Some(pl_arr) = cj
                .pointer("/results/playlists/0/data")
                .and_then(Value::as_array)
            {
                let items: Vec<EditorialItem> = pl_arr
                    .iter()
                    .filter_map(|v| parse_amp_resource_to_editorial_item(v, sf, 480))
                    .collect();
                for it in &items {
                    if it.motion_video_url.is_some() && picks.len() < 4 {
                        let mut p = it.clone();
                        p.badge = "编辑精选歌单".to_string();
                        picks.push(p);
                    }
                }
                if !items.is_empty() {
                    chart_shelves.push(EditorialShelf {
                        title: "热门精选歌单".to_string(),
                        subtitle: "Official Charts".to_string(),
                        kind: EditorialShelfKind::SquareShelf,
                        items,
                    });
                }
            }
            if let Some(alb_arr) = cj
                .pointer("/results/albums/0/data")
                .and_then(Value::as_array)
            {
                let items: Vec<EditorialItem> = alb_arr
                    .iter()
                    .filter_map(|v| parse_amp_resource_to_editorial_item(v, sf, 480))
                    .collect();
                for it in &items {
                    if it.motion_video_url.is_some() && picks.len() < 6 {
                        let mut p = it.clone();
                        p.badge = "热门精选专辑".to_string();
                        picks.push(p);
                    }
                }
                if !items.is_empty() {
                    chart_shelves.push(EditorialShelf {
                        title: "热门专辑".to_string(),
                        subtitle: "Top Albums".to_string(),
                        kind: EditorialShelfKind::SquareShelf,
                        items,
                    });
                }
            }
            if let Some(song_arr) = cj
                .pointer("/results/songs/0/data")
                .and_then(Value::as_array)
            {
                let items: Vec<EditorialItem> = song_arr
                    .iter()
                    .take(12)
                    .filter_map(|v| parse_amp_resource_to_editorial_item(v, sf, 160))
                    .collect();
                if !items.is_empty() {
                    chart_shelves.push(EditorialShelf {
                        title: "飙升单曲榜单".to_string(),
                        subtitle: "Top Songs".to_string(),
                        kind: EditorialShelfKind::TrackGrid,
                        items,
                    });
                }
            }
            if !picks.is_empty() {
                shelves.push(EditorialShelf {
                    title: "为你精选".to_string(),
                    subtitle: "Top Picks".to_string(),
                    kind: EditorialShelfKind::HomeTopPicks,
                    items: picks,
                });
            }
            shelves.extend(chart_shelves);
        }
    }

    if shelves.is_empty() {
        return fetch_apple_music_web_shelves(sf, "new").await;
    }

    // Automatically confirm motion cover availability once for EVERY Home item (albums, playlists, stations)
    confirm_home_shelves_motion_artwork(&mut shelves, sf, utok.as_deref()).await;

    Ok(shelves)
}

/// Fetches authentic Apple Music Radio shelves directly via `amp-api.music.apple.com`
/// (avoiding `music.apple.com/{sf}/radio` GeoIP redirects to `/cn/new`):
/// 1. User's personal, mood, artist & recently played stations (`v1/me/recent/radio-stations` + `v1/me/recommendations`)
/// 2. Official Apple Music Live Radio channels (`filter[featured]=apple-music-live-radio`)
/// 3. Official Station Genres & their track-based stations (`v1/catalog/{sf}/station-genres`)
pub async fn fetch_official_radio_shelves(
    storefront: &str,
    user_token: Option<String>,
) -> Result<Vec<EditorialShelf>, String> {
    let sf = if storefront.trim().is_empty() {
        "cn"
    } else {
        storefront.trim()
    };
    let utok = user_token
        .as_deref()
        .map(normalize_user_token)
        .filter(|s| !s.is_empty());

    let mut shelves: Vec<EditorialShelf> = Vec::new();

    // 1. Personal / Recent / Recommended Stations + 2. Live Radio + 3. Station Genres — all in PARALLEL!
    let loc = crate::i18n::am_api_locale();

    let personal_fut = async {
        let Some(utok_ref) = utok.as_ref() else {
            return None;
        };
        let rec_recent_path = format!("v1/me/recent/radio-stations?limit=12&l={loc}");
        let rec_path = format!("v1/me/recommendations?limit=15&l={loc}");
        let (recent_res, rec_res) = tokio::join!(
            fetch_am_api(rec_recent_path, Some(utok_ref.clone()), None, Some(sf.to_string())),
            fetch_am_api(rec_path, Some(utok_ref.clone()), None, Some(sf.to_string())),
        );

        let mut seen = HashSet::new();
        let mut personal_items: Vec<EditorialItem> = Vec::new();
        if let Ok(rj) = recent_res {
            if let Some(arr) = rj.get("data").and_then(Value::as_array) {
                for v in arr {
                    if let Some(mut it) = parse_amp_resource_to_editorial_item(v, sf, 600) {
                        it.motion_video_url = None;
                        it.motion_tall_video_url = None;
                        it.badge = if v
                            .pointer("/attributes/isLive")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                        {
                            "LIVE".to_string()
                        } else {
                            String::new()
                        };
                        if seen.insert(it.id.clone()) {
                            personal_items.push(it);
                        }
                    }
                }
            }
        }
        if let Ok(recj) = rec_res {
            if let Some(groups) = recj.get("data").and_then(Value::as_array) {
                for g in groups {
                    if let Some(arr) = g
                        .pointer("/relationships/contents/data")
                        .and_then(Value::as_array)
                    {
                        for v in arr {
                            if v.get("type").and_then(Value::as_str) != Some("stations") {
                                continue;
                            }
                            if let Some(mut it) = parse_amp_resource_to_editorial_item(v, sf, 600) {
                                it.motion_video_url = None;
                                it.motion_tall_video_url = None;
                                it.badge = if v
                                    .pointer("/attributes/isLive")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false)
                                {
                                    "LIVE".to_string()
                                } else {
                                    String::new()
                                };
                                if seen.insert(it.id.clone()) {
                                    personal_items.push(it);
                                }
                            }
                        }
                    }
                }
            }
        }
        (!personal_items.is_empty()).then(|| EditorialShelf {
            title: "你的专属与常用电台".to_string(),
            subtitle: "Personal & Recommended Stations".to_string(),
            kind: EditorialShelfKind::RadioStationRow,
            items: personal_items,
        })
    };

    let live_fut = async {
        let mut live_items: Vec<EditorialItem> = Vec::new();
        for live_sf in [sf, "jp", "us"] {
            let p = format!(
                "v1/catalog/{live_sf}/stations?filter[featured]=apple-music-live-radio&l={loc}"
            );
            if let Ok(lj) = fetch_am_api(p, utok.clone(), None, Some(live_sf.to_string())).await {
                if let Some(arr) = lj.get("data").and_then(Value::as_array) {
                    for v in arr {
                        if let Some(mut it) = parse_amp_resource_to_editorial_item(v, live_sf, 600) {
                            it.motion_video_url = None;
                            it.motion_tall_video_url = None;
                            it.badge = "LIVE".to_string();
                            if it.subtitle.is_empty() || it.subtitle == "广播电台" {
                                if let Some(tag) = v
                                    .pointer("/attributes/editorialNotes/tagline")
                                    .or_else(|| v.pointer("/attributes/editorialNotes/short"))
                                    .and_then(Value::as_str)
                                {
                                    let t = tag.trim();
                                    if !t.is_empty() {
                                        it.subtitle = t.to_string();
                                    }
                                }
                            }
                            live_items.push(it);
                        }
                    }
                }
            }
            if !live_items.is_empty() {
                break;
            }
        }
        (!live_items.is_empty()).then(|| EditorialShelf {
            title: "Apple Music 广播电台".to_string(),
            subtitle: "Featured Live Radio".to_string(),
            kind: EditorialShelfKind::RadioStationRow,
            items: live_items,
        })
    };

    let genres_fut = async {
        let mut genres: Vec<(String, String, String)> = Vec::new();
        for g_sf in [sf, "jp", "us"] {
            let gp = format!("v1/catalog/{g_sf}/station-genres?limit=8&l={loc}");
            if let Ok(gj) = fetch_am_api(gp, utok.clone(), None, Some(g_sf.to_string())).await {
                if let Some(arr) = gj.get("data").and_then(Value::as_array) {
                    for g in arr.iter().take(4) {
                        let gid = g.get("id").and_then(Value::as_str).unwrap_or("").trim();
                        let gname = g
                            .pointer("/attributes/name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim();
                        if !gid.is_empty() && !gname.is_empty() {
                            genres.push((g_sf.to_string(), gid.to_string(), gname.to_string()));
                        }
                    }
                }
            }
            if !genres.is_empty() {
                break;
            }
        }

        let mut genre_futs = Vec::new();
        for (g_sf, gid, gname) in genres {
            let ut = utok.clone();
            genre_futs.push(async move {
                let sp = format!("v1/catalog/{g_sf}/station-genres/{gid}/stations?limit=12&l={loc}");
                let json = fetch_am_api(sp, ut, None, Some(g_sf.clone())).await.ok()?;
                let arr = json.get("data").and_then(Value::as_array)?;
                let mut items = Vec::new();
                for v in arr {
                    if let Some(mut it) = parse_amp_resource_to_editorial_item(v, &g_sf, 600) {
                        it.motion_video_url = None;
                        it.motion_tall_video_url = None;
                        it.badge.clear();
                        items.push(it);
                    }
                }
                (!items.is_empty()).then(|| EditorialShelf {
                    title: format!("{gname} 电台"),
                    subtitle: "Station Genre".to_string(),
                    kind: EditorialShelfKind::RadioStationRow,
                    items,
                })
            });
        }
        futures_join_all_bounded(genre_futs).await
    };

    let (personal_opt, live_opt, genre_shelves) = tokio::join!(personal_fut, live_fut, genres_fut);
    if let Some(s) = personal_opt {
        shelves.push(s);
    }
    if let Some(s) = live_opt {
        shelves.push(s);
    }
    for sh in genre_shelves {
        if let Some(s) = sh {
            shelves.push(s);
        }
    }

    if shelves.is_empty() {
        return Err("未能加载 Apple Music 广播电台栏目".to_string());
    }
    Ok(shelves)
}

async fn futures_join_all_bounded<F, T>(futs: Vec<F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let mut handles = Vec::with_capacity(futs.len());
    for f in futs {
        handles.push(tokio::spawn(f));
    }
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(v) = h.await {
            out.push(v);
        }
    }
    out
}

#[inline]
pub fn is_apple_catalog_numeric_id(raw: &str) -> bool {
    let t = raw.trim();
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
}

fn strip_album_edition_suffix(raw: &str) -> &str {
    let t = raw.trim();
    for sep in [" - Single", " - EP", " - single", " - ep", " – Single", " – EP"] {
        if let Some(prefix) = t.strip_suffix(sep) {
            return prefix.trim();
        }
    }
    t
}

fn artist_tokens_match(want_artist: &str, cand_artist: &str) -> bool {
    let wa = crate::backends::normalize_match_text(want_artist);
    let ca = crate::backends::normalize_match_text(cand_artist);
    if wa.is_empty() || ca.is_empty() {
        return false;
    }
    if wa == ca {
        return true;
    }
    let split_tokens = |s: &str| -> Vec<String> {
        s.split(&['/', ',', '&', '、', '，', ';'][..])
            .map(crate::backends::normalize_match_text)
            .filter(|p| !p.is_empty())
            .collect()
    };
    let w_parts = split_tokens(want_artist);
    let c_parts = split_tokens(cand_artist);
    w_parts.iter().any(|wp| {
        c_parts.iter().any(|cp| {
            cp == wp
                || (wp.chars().count() >= 2
                    && cp.chars().count() >= 2
                    && (cp.contains(wp) || wp.contains(cp)))
        })
    })
}

/// Strictly validates whether an Apple Music album search candidate matches the playing track's
/// artist AND album (or single title when album is absent) before adopting its editorial motion video.
/// Prevents unrelated albums' motion covers from overwriting the playing song's real cover artwork.
pub fn motion_album_candidate_matches(
    want_title: &str,
    want_artist: &str,
    want_album: &str,
    cand_album_name: &str,
    cand_artist_name: &str,
) -> bool {
    if !artist_tokens_match(want_artist, cand_artist_name) {
        return false;
    }
    let cand_norm = crate::backends::normalize_match_text(strip_album_edition_suffix(cand_album_name));
    if cand_norm.is_empty() {
        return false;
    }
    let alb_norm = crate::backends::normalize_match_text(strip_album_edition_suffix(want_album));
    if !alb_norm.is_empty() {
        if cand_norm == alb_norm {
            return true;
        }
        let min_chars = cand_norm.chars().count().min(alb_norm.chars().count());
        return min_chars >= 3 && (cand_norm.contains(&alb_norm) || alb_norm.contains(&cand_norm));
    }
    let title_norm = crate::backends::normalize_match_text(strip_album_edition_suffix(want_title));
    !title_norm.is_empty() && cand_norm == title_norm
}

pub async fn fetch_album_motion_video_url(
    storefront: &str,
    album_or_song_id: &str,
    is_song: bool,
) -> Option<String> {
    let clean_id = album_or_song_id.trim();
    if !is_apple_catalog_numeric_id(clean_id) {
        return None;
    }
    let sf = if storefront.trim().is_empty() {
        "cn"
    } else {
        storefront.trim()
    };
    let path = if is_song {
        format!(
            "v1/catalog/{sf}/songs/{clean_id}?include=albums&extend%5Balbums%5D=editorialVideo&l=zh-Hans-CN"
        )
    } else {
        format!("v1/catalog/{sf}/albums/{clean_id}?extend=editorialVideo&l=zh-Hans-CN")
    };
    let json = fetch_am_api(path, None, None, Some(sf.to_string()))
        .await
        .ok()?;

    if let Some(attrs) = json.pointer("/data/0/attributes") {
        if let Some(v) = extract_editorial_motion_video_from_attrs(attrs) {
            return Some(v);
        }
    }
    if let Some(attrs) = json.pointer("/data/0/relationships/albums/data/0/attributes") {
        if let Some(v) = extract_editorial_motion_video_from_attrs(attrs) {
            return Some(v);
        }
    }
    if let Some(inc) = json.get("included").and_then(Value::as_array) {
        for item in inc {
            if let Some(attrs) = item.get("attributes") {
                if let Some(v) = extract_editorial_motion_video_from_attrs(attrs) {
                    return Some(v);
                }
            }
        }
    }
    if is_song {
        if let Some(alb_id) = json
            .pointer("/data/0/relationships/albums/data/0/id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| is_apple_catalog_numeric_id(s))
        {
            let alb_path =
                format!("v1/catalog/{sf}/albums/{alb_id}?extend=editorialVideo&l=zh-Hans-CN");
            if let Ok(alb_json) = fetch_am_api(alb_path, None, None, Some(sf.to_string())).await {
                if let Some(attrs) = alb_json.pointer("/data/0/attributes") {
                    if let Some(v) = extract_editorial_motion_video_from_attrs(attrs) {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

/// Returns the ordered list of Apple Music storefronts to probe sequentially for motion artwork:
/// always queries the active storefront FIRST and only falls back to `"us"` when active != `"us"`.
pub fn ordered_motion_probe_storefronts(storefront: &str) -> Vec<String> {
    let sf = storefront.trim().to_ascii_lowercase();
    let active = if sf.is_empty() {
        "cn".to_string()
    } else {
        sf
    };
    if active == "us" {
        vec![active]
    } else {
        vec![active, "us".to_string()]
    }
}

pub async fn fetch_track_motion_video_url(
    storefront: &str,
    adam_id: Option<&str>,
    title: &str,
    artist: &str,
    album: &str,
) -> Option<String> {
    // P1-5: Query the active storefront FIRST and only fall back to `"us"` on miss,
    // halving per-track-switch motion probe API requests on non-US accounts!
    let storefronts = ordered_motion_probe_storefronts(storefront);
    if let Some(id) = adam_id
        .map(str::trim)
        .filter(|s| is_apple_catalog_numeric_id(s))
    {
        for try_sf in &storefronts {
            if let Some(url) = fetch_album_motion_video_url(try_sf, id, true).await {
                return Some(url);
            }
        }
    }
    let query_raw = if !album.trim().is_empty() && !artist.trim().is_empty() {
        format!("{} {}", album.trim(), artist.trim())
    } else if !title.trim().is_empty() && !artist.trim().is_empty() {
        format!("{} {}", title.trim(), artist.trim())
    } else {
        return None;
    };
    let encoded: String = url::form_urlencoded::byte_serialize(query_raw.as_bytes()).collect();
    let title_s = title.trim().to_string();
    let artist_s = artist.trim().to_string();
    let album_s = album.trim().to_string();
    for try_sf in storefronts {
        let search_path = format!(
            "v1/catalog/{try_sf}/search?types=albums&term={encoded}&limit=5&extend=editorialVideo&l=zh-Hans-CN"
        );
        let Ok(json) = fetch_am_api(search_path, None, None, Some(try_sf)).await else {
            continue;
        };
        let Some(albums) = json.pointer("/results/albums/data").and_then(Value::as_array) else {
            continue;
        };
        for alb in albums {
            if let Some(attrs) = alb.get("attributes") {
                if let Some(v) = extract_editorial_motion_video_from_attrs(attrs) {
                    let cand_name = attrs
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let cand_artist = attrs
                        .get("artistName")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if motion_album_candidate_matches(
                        &title_s,
                        &artist_s,
                        &album_s,
                        cand_name,
                        cand_artist,
                    ) {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}
