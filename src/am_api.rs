use base64::{engine::general_purpose, Engine as _};
use once_cell::sync::Lazy;
use percent_encoding::percent_decode_str;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AMP_API_BASE: &str = "https://amp-api.music.apple.com/";
const WEB_ORIGIN: &str = "https://music.apple.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const OFFICIAL_AM_PFN: &str = "AppleInc.AppleMusicWin_nzyj5cx40ttqa";

static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
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

static ART_DISK_DIR: Lazy<Option<std::path::PathBuf>> = Lazy::new(|| {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(std::path::PathBuf::from)?;
    let dir = base.join("AM-Light").join("art_cache");
    let _ = std::fs::create_dir_all(&dir);
    // Keep disk cache bounded (< ~600 files) on startup without blocking UI
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let entries: Vec<_> = rd.flatten().collect();
        if entries.len() > 640 {
            for e in entries.into_iter().take(240) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
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

/// Normalizes an Apple Music / mzstatic artwork URL across sizes (`120x120`, `240x240`, `360x360`, `600x600`)
/// to a single canonical identity stem so cached thumbnails seamlessly stand in for hero/lyrics covers
/// with zero black-frame flicker while higher-res textures decode!
pub fn artwork_canonical_stem(url: &str) -> String {
    let t = url.trim();
    if let Some(slash) = t.rfind('/') {
        let tail = &t[slash + 1..];
        let has_dim = tail
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == 'x')
            .count();
        if has_dim >= 5 && tail.contains('x') {
            return t[..slash].to_string();
        }
    }
    t.to_string()
}

/// Fetches album/playlist artwork bytes using zero-RAM local NVMe disk cache + keep-alive TLS connection pool,
/// with a dedicated priority lane for hero/now-playing artwork so route switches never starve behind grid thumbnails.
pub async fn fetch_artwork_bytes(url: &str, high_priority: bool) -> Result<Vec<u8>, String> {
    let sem = if high_priority {
        &*HERO_ART_SEM
    } else {
        &*ARTWORK_SEM
    };
    let _permit = sem
        .acquire()
        .await
        .map_err(|e| format!("semaphore closed: {e}"))?;

    let disk_path = ART_DISK_DIR
        .as_ref()
        .map(|d| d.join(format!("{:016x}.img", fnv1a_64(url))));

    if let Some(ref p) = disk_path {
        let p_read = p.clone();
        if let Ok(Ok(cached)) = tokio::task::spawn_blocking(move || std::fs::read(p_read)).await {
            if cached.len() >= 16 {
                return Ok(cached);
            }
        }
    }

    for attempt in 0..2 {
        let res = HTTP_CLIENT
            .get(url)
            .timeout(Duration::from_secs(8))
            .header("Referer", format!("{WEB_ORIGIN}/"))
            .send()
            .await;
        match res {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| format!("读取封面字节失败: {e}"))?;
                if bytes.len() >= 16 {
                    let vec_b = bytes.to_vec();
                    if let Some(p) = disk_path.clone() {
                        let wb = vec_b.clone();
                        let _ = tokio::task::spawn_blocking(move || std::fs::write(p, wb)).await;
                    }
                    return Ok(vec_b);
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

static DEV_TOKEN_CACHE: Lazy<Mutex<Option<CachedDevToken>>> = Lazy::new(|| Mutex::new(None));

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
    if let Ok(mut guard) = DEV_TOKEN_CACHE.lock() {
        *guard = None;
    }
    if let Some(p) = dev_token_disk_path() {
        let _ = std::fs::remove_file(p);
    }
}

pub async fn get_public_dev_token(
    force_refresh: bool,
    storefront_hint: Option<&str>,
) -> Result<CachedDevToken, String> {
    let now = now_unix_secs();
    if !force_refresh {
        if let Ok(mut guard) = DEV_TOKEN_CACHE.lock() {
            if let Some(cached) = guard.as_ref() {
                let valid = cached
                    .exp
                    .map(|exp| exp > now.saturating_add(120))
                    .unwrap_or(true);
                if valid {
                    return Ok(cached.clone());
                }
            } else if let Some(p) = dev_token_disk_path() {
                if let Ok(txt) = std::fs::read_to_string(p) {
                    if let Ok(disk_tok) = serde_json::from_str::<CachedDevToken>(&txt) {
                        let valid = disk_tok
                            .exp
                            .map(|exp| exp > now.saturating_add(120))
                            .unwrap_or(true);
                        if valid && !disk_tok.token.is_empty() {
                            *guard = Some(disk_tok.clone());
                            return Ok(disk_tok);
                        }
                    }
                }
            }
        }
    }

    let mut storefronts = Vec::new();
    if let Some(sf) = storefront_hint {
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
                if let Ok(mut guard) = DEV_TOKEN_CACHE.lock() {
                    *guard = Some(tok.clone());
                }
                if let Some(p) = dev_token_disk_path() {
                    if let Ok(json_str) = serde_json::to_string(&tok) {
                        let _ = std::fs::write(p, json_str);
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

    let effective_dev = match &custom_dev {
        Some(t) => t.clone(),
        None => {
            get_public_dev_token(false, storefront_hint.as_deref())
                .await?
                .token
        }
    };

    let (mut status, mut body) =
        send_amp_get(&url, &effective_dev, normalized_user.as_deref()).await?;

    // Retry once with refreshed public Developer Token on 401
    if status == reqwest::StatusCode::UNAUTHORIZED && custom_dev.is_none() {
        invalidate_dev_token_cache();
        if let Ok(fresh) = get_public_dev_token(true, storefront_hint.as_deref()).await {
            let retry = send_amp_get(&url, &fresh.token, normalized_user.as_deref()).await?;
            status = retry.0;
            body = retry.1;
        }
    }

    if !status.is_success() {
        return Err(format_apple_api_error(status, &body));
    }

    serde_json::from_str::<Value>(&body)
        .map_err(|e| format!("解析 Apple Music JSON 失败: {e}"))
}

async fn send_amp_post(
    url: &str,
    dev_token: &str,
    user_token: &str,
) -> Result<(reqwest::StatusCode, String), String> {
    let resp = HTTP_CLIENT
        .post(url)
        .header("Authorization", format!("Bearer {dev_token}"))
        .header("Media-User-Token", user_token)
        .header("Origin", WEB_ORIGIN)
        .header("Referer", format!("{WEB_ORIGIN}/"))
        .header("Content-Length", "0")
        .body("")
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
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let musics_uri = convert_to_musics_uri(uri)?;

    let winrt_attempt = (|| -> Result<bool, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = crate::am_agent::init_process_com_security();
        }
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
