use crate::am_agent::AgentTrackInput;
use crate::am_api::{EditorialItem, EditorialShelf, EditorialShelfKind};
use crate::backends::{NetEaseQrState, NetEaseQuality, ResolvedAudioStream};
use crate::lyrics::{self, ParsedLyrics};
use once_cell::sync::Lazy;
use qrcode::{Color as QrColor, QrCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const NE_REFERER: &str = "https://music.163.com/";
const NE_BASE_COOKIE: &str = "os=pc; appver=3.0.18.203152; channel=netease";

static NE_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .pool_max_idle_per_host(16)
        .tcp_nodelay(true)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
        .expect("netease http client")
});

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NetEaseAccountInfo {
    pub logged_in: bool,
    pub uid: u64,
    pub nickname: String,
    pub vip_type: i64,
    #[serde(default)]
    pub vip_level: i64,
    #[serde(default)]
    pub vip_label: String,
    #[serde(default)]
    pub is_vip: bool,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct NetEaseCard {
    pub id: String,
    pub kind: String, // "ne-playlist", "ne-album", "ne-artist", "ne-daily", "ne-station"
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub badge: Option<String>,
    pub track_input: Option<AgentTrackInput>,
}

#[derive(Clone, Debug)]
pub struct NetEaseDetail {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub tracks: Vec<AgentTrackInput>,
    pub related_albums: Vec<NetEaseCard>,
}

#[derive(Clone, Debug)]
pub struct NetEaseHomeBundle {
    pub shelves: Vec<EditorialShelf>,
    pub browse_shelves: Vec<EditorialShelf>,
    pub playlists: Vec<NetEaseCard>,
    pub albums: Vec<NetEaseCard>,
    pub songs: Vec<AgentTrackInput>,
}

#[derive(Clone, Debug)]
pub struct NetEaseQrPollResult {
    pub code: i64,
    pub message: String,
    pub cookie: Option<String>,
}

/// Returns true if the cookie string contains a non-empty `MUSIC_U=` credential.
pub fn has_music_u_cookie(cookie: &str) -> bool {
    for seg in cookie.split(';') {
        if let Some(val) = seg.trim().strip_prefix("MUSIC_U=") {
            if !val.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

/// Normalizes ANY pasted NetEase cookie format into canonical `"MUSIC_U=<val>"` (plus optional
/// `"; __csrf=<val>"`), returning an empty string if no valid `MUSIC_U` credential is present.
/// Supports:
/// - Full browser cookie strings (`foo=1; MUSIC_U=...; __csrf=...`)
/// - DevTools Request/Response Headers (`Cookie: MUSIC_U=...`, multi-line `Set-Cookie: MUSIC_U=...`)
/// - DevTools Cookie table copy (`MUSIC_U\t<value>` or `MUSIC_U: <value>`)
/// - Bare `MUSIC_U` token values (>= 40 chars without whitespace or `=`)
pub fn normalize_netease_cookie(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut music_u = String::new();
    let mut csrf = String::new();

    for line in trimmed.lines() {
        let mut l = line.trim();
        if l.is_empty() {
            continue;
        }
        // Strip leading `Cookie:` or `Set-Cookie:` header prefix (case-insensitive)
        let low = l.to_ascii_lowercase();
        if low.starts_with("set-cookie:") {
            l = l[11..].trim();
        } else if low.starts_with("cookie:") {
            l = l[7..].trim();
        }

        // Handle DevTools Application -> Cookies row copy: `MUSIC_U\t<value>\t.music.163.com...`
        if l.contains('\t') {
            let cols: Vec<&str> = l.split('\t').map(str::trim).filter(|s| !s.is_empty()).collect();
            if cols.len() >= 2 {
                if cols[0].eq_ignore_ascii_case("MUSIC_U") && !cols[1].is_empty() {
                    music_u = cols[1].trim_matches('"').to_string();
                    continue;
                }
                if cols[0].eq_ignore_ascii_case("__csrf") && !cols[1].is_empty() {
                    csrf = cols[1].trim_matches('"').to_string();
                    continue;
                }
            }
        }

        // Handle `MUSIC_U: <value>` colon format
        if let Some((k, v)) = l.split_once(':') {
            let kt = k.trim();
            let vt = v.trim().split(';').next().unwrap_or("").trim().trim_matches('"');
            if kt.eq_ignore_ascii_case("MUSIC_U") && !vt.is_empty() && !vt.contains('=') {
                music_u = vt.to_string();
                continue;
            }
            if kt.eq_ignore_ascii_case("__csrf") && !vt.is_empty() && !vt.contains('=') {
                csrf = vt.to_string();
                continue;
            }
        }

        for seg in l.split(';') {
            let mut s = seg.trim();
            let slow = s.to_ascii_lowercase();
            if slow.starts_with("set-cookie:") {
                s = s[11..].trim();
            } else if slow.starts_with("cookie:") {
                s = s[7..].trim();
            }
            if let Some((k, v)) = s.split_once('=') {
                let kt = k.trim();
                let vt = v.trim().trim_matches('"').trim();
                if kt.eq_ignore_ascii_case("MUSIC_U") && !vt.is_empty() {
                    music_u = vt.to_string();
                } else if kt.eq_ignore_ascii_case("__csrf") && !vt.is_empty() {
                    csrf = vt.to_string();
                }
            }
        }
    }

    if !music_u.is_empty() {
        return if csrf.is_empty() {
            format!("MUSIC_U={music_u}")
        } else {
            format!("MUSIC_U={music_u}; __csrf={csrf}")
        };
    }

    // Bare MUSIC_U value pasted directly without `MUSIC_U=` prefix
    if trimmed.len() >= 40
        && !trimmed.contains(char::is_whitespace)
        && !trimmed.contains('=')
        && !trimmed.contains(':')
        && !trimmed.contains('/')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '~'))
    {
        return format!("MUSIC_U={trimmed}");
    }

    String::new()
}

pub fn build_cookie_header(user_cookie: Option<&str>) -> String {
    match user_cookie.map(str::trim).filter(|s| !s.is_empty()) {
        Some(c) => format!("{NE_BASE_COOKIE}; {c}"),
        None => NE_BASE_COOKIE.to_string(),
    }
}

pub fn strip_ne_id(raw: &str) -> String {
    raw.trim()
        .strip_prefix("ne:")
        .or_else(|| raw.trim().strip_prefix("ne-song:"))
        .unwrap_or(raw.trim())
        .to_string()
}

fn upgrade_https_cover(url: &str, dim: u32) -> String {
    let u = url.trim();
    if u.is_empty() {
        return String::new();
    }
    let https = if let Some(rest) = u.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        u.to_string()
    };
    if https.contains('?') {
        https
    } else {
        format!("{https}?param={dim}y{dim}")
    }
}

fn format_play_count(n: u64) -> String {
    if n >= 100_000_000 {
        format!("{:.1}亿次播放", n as f64 / 100_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}万次播放", n as f64 / 10_000.0)
    } else if n > 0 {
        format!("{n}次播放")
    } else {
        "网易云精选".to_string()
    }
}

fn parse_ne_song(v: &Value, fallback_art: Option<&str>) -> Option<AgentTrackInput> {
    let raw_id = v.get("id").and_then(Value::as_i64)?;
    let name = v.get("name").and_then(Value::as_str)?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let artists_arr = v
        .get("ar")
        .or_else(|| v.get("artists"))
        .and_then(Value::as_array);
    let artist_name = artists_arr
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();

    let album_obj = v.get("al").or_else(|| v.get("album"));
    let album_name = album_obj
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let art_raw = album_obj
        .and_then(|a| a.get("picUrl"))
        .and_then(Value::as_str)
        .or_else(|| v.get("picUrl").and_then(Value::as_str))
        .unwrap_or_default();
    let artwork_url = if !art_raw.is_empty() {
        Some(upgrade_https_cover(art_raw, 480))
    } else {
        fallback_art.map(|s| s.to_string())
    };

    let dur_ms = v
        .get("dt")
        .or_else(|| v.get("duration"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let duration_sec = if dur_ms > 1000.0 { dur_ms / 1000.0 } else { dur_ms };

    let mut audio_traits = Vec::new();
    if v.get("hr").is_some_and(|x| !x.is_null()) {
        audio_traits.push("hi-res-lossless".to_string());
    } else if v.get("sq").is_some_and(|x| !x.is_null()) {
        audio_traits.push("lossless".to_string());
    }
    let fee = v.get("fee").and_then(Value::as_i64).unwrap_or(0);
    if fee == 1 {
        audio_traits.push("vip".to_string());
    }

    Some(AgentTrackInput {
        id: format!("ne:{raw_id}"),
        catalog_id: Some(raw_id.to_string()),
        kind: Some("ne-song".to_string()),
        name,
        artist_name,
        album_name,
        duration_sec,
        artwork_url,
        audio_traits,
    })
}

async fn ne_get_json(url: &str, cookie: Option<&str>) -> Result<Value, String> {
    let resp = NE_HTTP
        .get(url)
        .header("Referer", NE_REFERER)
        .header("Origin", "https://music.163.com")
        .header("Cookie", build_cookie_header(cookie))
        .send()
        .await
        .map_err(|e| format!("网易云请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("网易云接口 HTTP {}", resp.status().as_u16()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("网易云 JSON 解析失败: {e}"))
}

async fn ne_post_form_json(
    url: &str,
    form: &[(&str, String)],
    cookie: Option<&str>,
) -> Result<Value, String> {
    let resp = NE_HTTP
        .post(url)
        .header("Referer", NE_REFERER)
        .header("Origin", "https://music.163.com")
        .header("Cookie", build_cookie_header(cookie))
        .form(form)
        .send()
        .await
        .map_err(|e| format!("网易云请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("网易云接口 HTTP {}", resp.status().as_u16()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("网易云 JSON 解析失败: {e}"))
}

// ============================================================================
// Account & Native QR Code Login (Rendered in Direct2D!)
// ============================================================================

pub async fn start_qr_login() -> Result<NetEaseQrState, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let url = format!("https://music.163.com/api/login/qrcode/unikey?type=1&timestamp={ts}");
    let json = ne_get_json(&url, None).await?;
    let unikey = json
        .get("unikey")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "获取网易云登录二维码 Key 失败".to_string())?
        .to_string();
    let qr_url = format!("https://music.163.com/login?codekey={unikey}");
    let code = QrCode::new(qr_url.as_bytes()).map_err(|e| format!("生成二维码矩阵失败: {e}"))?;
    let w = code.width();
    let mut modules = Vec::with_capacity(w * w);
    for y in 0..w {
        for x in 0..w {
            modules.push(code[(x, y)] == QrColor::Dark);
        }
    }
    Ok(NetEaseQrState {
        unikey,
        qr_url,
        qr_width: w,
        qr_modules: modules,
        status_code: 801,
        status_text: "请使用网易云音乐 App 扫码登录".to_string(),
    })
}

pub async fn poll_qr_login(unikey: &str) -> Result<NetEaseQrPollResult, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let url = format!(
        "https://music.163.com/api/login/qrcode/client/login?key={unikey}&type=1&timestamp={ts}"
    );
    let resp = NE_HTTP
        .get(&url)
        .header("Referer", NE_REFERER)
        .header("Cookie", NE_BASE_COOKIE)
        .send()
        .await
        .map_err(|e| format!("轮询扫码状态失败: {e}"))?;

    // Extract MUSIC_U & __csrf cookies from Set-Cookie headers AND JSON `cookie` field on login success
    let mut raw_cookie_lines = Vec::new();
    for val in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        if let Ok(s) = val.to_str() {
            raw_cookie_lines.push(s.to_string());
        }
    }
    let json = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("解析扫码响应失败: {e}"))?;
    if let Some(jc) = json.get("cookie").and_then(Value::as_str) {
        if !jc.trim().is_empty() {
            raw_cookie_lines.push(jc.to_string());
        }
    }
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    let msg = json
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(match code {
            800 => "二维码已过期，请点击刷新",
            801 => "等待网易云 App 扫码...",
            802 => "已扫码，请在手机端确认登录",
            803 => "登录成功！正在同步个人歌单...",
            _ => "等待扫码响应...",
        })
        .to_string();

    let normalized_cookie = normalize_netease_cookie(&raw_cookie_lines.join("\n"));
    let cookie = if code == 803 && has_music_u_cookie(&normalized_cookie) {
        Some(normalized_cookie)
    } else {
        None
    };

    Ok(NetEaseQrPollResult {
        code,
        message: msg,
        cookie,
    })
}

pub fn parse_vip_info_json(vip_json: &Value, fallback_vip_type: i64) -> (bool, i64, String) {
    let data = vip_json.get("data").unwrap_or(vip_json);
    let red_lvl = data
        .get("redVipLevel")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let redplus_rights = data
        .pointer("/redplus/rights")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let assoc_rights = data
        .pointer("/associator/rights")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pkg_rights = data
        .pointer("/musicPackage/rights")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let assoc_code = data
        .pointer("/associator/vipCode")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let is_vip = redplus_rights || assoc_rights || pkg_rights || fallback_vip_type >= 11;

    let base_name = if redplus_rights {
        "黑胶 SVIP"
    } else if assoc_rights || fallback_vip_type >= 11 || assoc_code >= 100 {
        "黑胶 VIP"
    } else if pkg_rights || fallback_vip_type > 0 {
        "音乐包"
    } else {
        "普通用户"
    };
    let label = if red_lvl > 0 && is_vip {
        format!("{base_name} Lv.{red_lvl}")
    } else if red_lvl > 0 {
        format!("{base_name} (CVIP Lv.{red_lvl})")
    } else {
        base_name.to_string()
    };
    (is_vip, red_lvl, label)
}

pub async fn verify_account(cookie: &str) -> Result<NetEaseAccountInfo, String> {
    let norm = normalize_netease_cookie(cookie);
    if !has_music_u_cookie(&norm) {
        return Ok(NetEaseAccountInfo {
            logged_in: false,
            message: "未登录网易云账号（游客模式仍可浏览全部榜单并试听免费曲目）".to_string(),
            ..Default::default()
        });
    }
    let (acct_res, vip_res) = tokio::join!(
        ne_get_json("https://music.163.com/api/nuser/account/get", Some(&norm)),
        ne_get_json(
            "https://music.163.com/api/music-vip-membership/front/vip/info",
            Some(&norm)
        ),
    );
    let mut json = match acct_res {
        Ok(v) if v.get("profile").is_some_and(Value::is_object) => v,
        _ => match crate::listen_together::fetch_account_eapi(&norm).await {
            Ok(ev) => ev,
            Err(e) => return Err(e),
        },
    };
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(200);
    if code != 200 && code != 301 {
        // Retry once via EAPI if direct GET hit transient risk-control (-460 / 5xx)
        if let Ok(ev) = crate::listen_together::fetch_account_eapi(&norm).await {
            json = ev;
        } else {
            return Err(format!("网易云账号状态校验暂时受限 (code {code})"));
        }
    }
    let final_code = json.get("code").and_then(Value::as_i64).unwrap_or(200);
    if final_code != 200 && final_code != 301 {
        return Err(format!("网易云账号状态校验暂时受限 (code {final_code})"));
    }

    let profile = json.get("profile").filter(|v| v.is_object());
    let account = json.get("account").filter(|v| v.is_object());
    let is_anon = account
        .and_then(|a| a.get("anonimousUser"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let uid = profile
        .and_then(|p| p.get("userId").and_then(Value::as_u64))
        .or_else(|| {
            if !is_anon {
                account.and_then(|a| a.get("id").and_then(Value::as_u64))
            } else {
                None
            }
        })
        .unwrap_or(0);

    if uid > 0 {
        let nickname = profile
            .and_then(|p| p.get("nickname").and_then(Value::as_str))
            .or_else(|| account.and_then(|a| a.get("userName").and_then(Value::as_str)))
            .unwrap_or("网易云用户")
            .to_string();
        let vip_type = profile
            .and_then(|p| p.get("vipType").and_then(Value::as_i64))
            .or_else(|| account.and_then(|a| a.get("vipType").and_then(Value::as_i64)))
            .unwrap_or(0);
        let mut vip_json = vip_res.unwrap_or(Value::Null);
        if vip_json.get("data").filter(|v| v.is_object()).is_none() {
            if let Ok(eapi_vip) = crate::listen_together::fetch_vip_info_eapi(uid, &norm).await {
                vip_json = eapi_vip;
            }
        }
        let (is_vip, vip_level, vip_label) = parse_vip_info_json(&vip_json, vip_type);
        Ok(NetEaseAccountInfo {
            logged_in: true,
            uid,
            nickname: nickname.clone(),
            vip_type,
            vip_level,
            vip_label: vip_label.clone(),
            is_vip,
            message: format!("已登录：{nickname} · {vip_label} (UID {uid})"),
        })
    } else {
        Ok(NetEaseAccountInfo {
            logged_in: false,
            message: "Cookie 已过期或无效，请重新扫码登录或粘贴最新 MUSIC_U".to_string(),
            ..Default::default()
        })
    }
}

// ============================================================================
// Home / Discover / Radio / Search / Library / Details
// ============================================================================

pub async fn fetch_home_and_browse(cookie: Option<&str>) -> Result<NetEaseHomeBundle, String> {
    let has_login = cookie.is_some_and(|c| c.contains("MUSIC_U="));

    let (pers_res, top_res, newsong_res, newalb_res, hotpl_res, hotchart_res) = tokio::join!(
        ne_post_form_json("https://music.163.com/api/personalized/playlist?limit=12", &[], cookie),
        ne_get_json("https://music.163.com/api/toplist", cookie),
        ne_post_form_json("https://music.163.com/api/personalized/newsong?limit=16", &[], cookie),
        ne_get_json("https://music.163.com/api/album/new?limit=12&offset=0", cookie),
        ne_get_json("https://music.163.com/api/playlist/list?cat=%E5%85%A8%E9%83%A8&order=hot&limit=12&offset=0", cookie),
        ne_get_json("https://music.163.com/api/v6/playlist/detail?id=3778678&n=16", cookie),
    );

    let mut home_shelves = Vec::new();
    let mut browse_shelves = Vec::new();
    let mut playlists = Vec::new();
    let mut albums = Vec::new();
    let mut songs = Vec::new();

    // 1. Top Picks Shelf (Personal Daily Recommendation + Personalized Playlists)
    let mut top_pick_items = Vec::new();
    if has_login {
        top_pick_items.push(EditorialItem {
            id: "daily".to_string(),
            kind: "ne-daily".to_string(),
            play_kind: "ne-daily".to_string(),
            title: "每日歌曲推荐".to_string(),
            subtitle: "根据你的网易云听歌口味定制".to_string(),
            badge: "每日推荐".to_string(),
            artwork_url: "https://p1.music.126.net/6y-UleORITEDbvrOLV0Q8A==/5639395138885805.jpg?param=480y480".to_string(),
            motion_video_url: None,
            motion_tall_video_url: None,
            storefront: "ne".to_string(),
        });
    }
    if let Ok(j) = pers_res {
        if let Some(arr) = j.get("result").and_then(Value::as_array) {
            for item in arr.iter().take(12) {
                let Some(id) = item.get("id").and_then(Value::as_i64) else { continue };
                let title = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let pic = item.get("picUrl").and_then(Value::as_str).unwrap_or_default();
                let art = upgrade_https_cover(pic, 480);
                let pc = item.get("playCount").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let cw = item
                    .get("copywriter")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format_play_count(pc));
                top_pick_items.push(EditorialItem {
                    id: id.to_string(),
                    kind: "ne-playlist".to_string(),
                    play_kind: "ne-playlist".to_string(),
                    title: title.clone(),
                    subtitle: cw.clone(),
                    badge: "推荐歌单".to_string(),
                    artwork_url: art.clone(),
                    motion_video_url: None,
                    motion_tall_video_url: None,
                    storefront: "ne".to_string(),
                });
                playlists.push(NetEaseCard {
                    id: id.to_string(),
                    kind: "ne-playlist".to_string(),
                    title,
                    subtitle: cw,
                    description: String::new(),
                    artwork_url: if art.is_empty() { None } else { Some(art) },
                    badge: Some("歌单".to_string()),
                    track_input: None,
                });
            }
        }
    }
    if !top_pick_items.is_empty() {
        home_shelves.push(EditorialShelf {
            title: "为你推荐 · 网易云精选".to_string(),
            subtitle: "热门精选歌单与个性化推荐".to_string(),
            kind: EditorialShelfKind::HomeTopPicks,
            items: top_pick_items,
        });
    }

    // 2. Official Toplists Shelf
    if let Ok(j) = top_res {
        if let Some(arr) = j.get("list").and_then(Value::as_array) {
            let mut chart_items = Vec::new();
            for item in arr.iter().take(12) {
                let Some(id) = item.get("id").and_then(Value::as_i64) else { continue };
                let title = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let sub = item
                    .get("updateFrequency")
                    .and_then(Value::as_str)
                    .unwrap_or("官方榜单")
                    .to_string();
                let art = upgrade_https_cover(
                    item.get("coverImgUrl").and_then(Value::as_str).unwrap_or_default(),
                    480,
                );
                chart_items.push(EditorialItem {
                    id: id.to_string(),
                    kind: "ne-playlist".to_string(),
                    play_kind: "ne-playlist".to_string(),
                    title,
                    subtitle: sub,
                    badge: "官方榜".to_string(),
                    artwork_url: art,
                    motion_video_url: None,
                    motion_tall_video_url: None,
                    storefront: "ne".to_string(),
                });
            }
            if !chart_items.is_empty() {
                home_shelves.push(EditorialShelf {
                    title: "官方排行榜".to_string(),
                    subtitle: "飙升榜 · 新歌榜 · 热歌榜 · 原创榜".to_string(),
                    kind: EditorialShelfKind::SquareShelf,
                    items: chart_items,
                });
            }
        }
    }

    // 3. Recommended New Songs
    if let Ok(j) = newsong_res {
        if let Some(arr) = j.get("result").and_then(Value::as_array) {
            let mut tr_items = Vec::new();
            for item in arr.iter().take(12) {
                let Some(song_v) = item.get("song") else { continue };
                let fallback = item
                    .get("picUrl")
                    .and_then(Value::as_str)
                    .map(|u| upgrade_https_cover(u, 480));
                if let Some(tr) = parse_ne_song(song_v, fallback.as_deref()) {
                    tr_items.push(EditorialItem {
                        id: tr.id.clone(),
                        kind: "ne-song".to_string(),
                        play_kind: "ne-song".to_string(),
                        title: tr.name.clone(),
                        subtitle: tr.artist_name.clone(),
                        badge: "新歌".to_string(),
                        artwork_url: tr.artwork_url.clone().unwrap_or_default(),
                        motion_video_url: None,
                        motion_tall_video_url: None,
                        storefront: "ne".to_string(),
                    });
                    songs.push(tr);
                }
            }
            if !tr_items.is_empty() {
                home_shelves.push(EditorialShelf {
                    title: "推荐新音乐".to_string(),
                    subtitle: "最新上线单曲 · 点击即播".to_string(),
                    kind: EditorialShelfKind::TrackGrid,
                    items: tr_items,
                });
            }
        }
    }

    // Fallback hot songs from 热歌榜 if newsong empty
    if songs.is_empty() {
        if let Ok(j) = hotchart_res {
            if let Some(arr) = j.pointer("/playlist/tracks").and_then(Value::as_array) {
                for v in arr.iter().take(16) {
                    if let Some(tr) = parse_ne_song(v, None) {
                        songs.push(tr);
                    }
                }
            }
        }
    }

    // 4. New Albums (Browse Hero + Home fallback albums)
    if let Ok(j) = newalb_res {
        if let Some(arr) = j.get("albums").and_then(Value::as_array) {
            let mut alb_items = Vec::new();
            for item in arr.iter().take(12) {
                let Some(id) = item.get("id").and_then(Value::as_i64) else { continue };
                let title = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let artist = item
                    .pointer("/artist/name")
                    .and_then(Value::as_str)
                    .unwrap_or("新碟首发")
                    .to_string();
                let art = upgrade_https_cover(
                    item.get("picUrl")
                        .or_else(|| item.get("blurPicUrl"))
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    480,
                );
                alb_items.push(EditorialItem {
                    id: id.to_string(),
                    kind: "ne-album".to_string(),
                    play_kind: "ne-album".to_string(),
                    title: title.clone(),
                    subtitle: artist.clone(),
                    badge: "新碟".to_string(),
                    artwork_url: art.clone(),
                    motion_video_url: None,
                    motion_tall_video_url: None,
                    storefront: "ne".to_string(),
                });
                albums.push(NetEaseCard {
                    id: id.to_string(),
                    kind: "ne-album".to_string(),
                    title,
                    subtitle: artist,
                    description: String::new(),
                    artwork_url: if art.is_empty() { None } else { Some(art) },
                    badge: Some("新碟".to_string()),
                    track_input: None,
                });
            }
            if !alb_items.is_empty() {
                browse_shelves.push(EditorialShelf {
                    title: "新碟上架".to_string(),
                    subtitle: "网易云最新发行专辑".to_string(),
                    kind: EditorialShelfKind::NewFeatureHero,
                    items: alb_items.clone(),
                });
                home_shelves.push(EditorialShelf {
                    title: "新碟首发".to_string(),
                    subtitle: String::new(),
                    kind: EditorialShelfKind::SquareShelf,
                    items: alb_items,
                });
            }
        }
    }

    // 5. Hot Community Playlists (Browse Shelf 2)
    if let Ok(j) = hotpl_res {
        if let Some(arr) = j.get("playlists").and_then(Value::as_array) {
            let mut pl_items = Vec::new();
            for item in arr.iter().take(12) {
                let Some(id) = item.get("id").and_then(Value::as_i64) else { continue };
                let title = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let creator = item
                    .pointer("/creator/nickname")
                    .and_then(Value::as_str)
                    .unwrap_or("网友精选")
                    .to_string();
                let art = upgrade_https_cover(
                    item.get("coverImgUrl").and_then(Value::as_str).unwrap_or_default(),
                    480,
                );
                pl_items.push(EditorialItem {
                    id: id.to_string(),
                    kind: "ne-playlist".to_string(),
                    play_kind: "ne-playlist".to_string(),
                    title,
                    subtitle: format!("by {creator}"),
                    badge: "热歌单".to_string(),
                    artwork_url: art,
                    motion_video_url: None,
                    motion_tall_video_url: None,
                    storefront: "ne".to_string(),
                });
            }
            if !pl_items.is_empty() {
                browse_shelves.push(EditorialShelf {
                    title: "网友精选碟 · 歌单广场".to_string(),
                    subtitle: "按热度实时更新".to_string(),
                    kind: EditorialShelfKind::SquareShelf,
                    items: pl_items,
                });
            }
        }
    }

    Ok(NetEaseHomeBundle {
        shelves: home_shelves,
        browse_shelves,
        playlists,
        albums,
        songs,
    })
}

pub async fn fetch_radio_shelves(cookie: Option<&str>) -> Result<(Vec<EditorialShelf>, Vec<NetEaseCard>), String> {
    let mut shelves = Vec::new();
    let mut cards = Vec::new();

    // Fixed personal FM & curated NetEase station channels (all directly playable!)
    let stations = [
        ("fm", "私人 FM", "实时随机推送符合口味的单曲流", "https://p2.music.126.net/6y-UleORITEDbvrOLV0Q8A==/5639395138885805.jpg?param=480y480"),
        ("19723756", "云音乐飙升电台", "全网实时飙升热曲连播", "https://p2.music.126.net/pcYHpMkdC69VVvWiynNklA==/109951166952713766.jpg?param=480y480"),
        ("3778678", "热歌连播电台", "网易云热门单曲不间断", "https://p1.music.126.net/ZyUjc7K_GDpD8MO1-GQkmA==/109951166952706664.jpg?param=480y480"),
        ("2884035", "独立原创电台", "发掘宝藏独立音乐人作品", "https://p1.music.126.net/iFZ_nw2V86IFk90dc50kdQ==/109951166961388699.jpg?param=480y480"),
        ("3779629", "新歌速递电台", "第一时间听见华语与全球新声", "https://p2.music.126.net/wVmyNS6b_0Nn-y6AX8UbpQ==/109951166952686384.jpg?param=480y480"),
    ];
    let mut st_items = Vec::new();
    for (id, title, sub, art) in stations {
        st_items.push(EditorialItem {
            id: id.to_string(),
            kind: "ne-station".to_string(),
            play_kind: "ne-station".to_string(),
            title: title.to_string(),
            subtitle: sub.to_string(),
            badge: "电台".to_string(),
            artwork_url: art.to_string(),
            motion_video_url: None,
            motion_tall_video_url: None,
            storefront: "ne".to_string(),
        });
        cards.push(NetEaseCard {
            id: id.to_string(),
            kind: "ne-station".to_string(),
            title: title.to_string(),
            subtitle: sub.to_string(),
            description: String::new(),
            artwork_url: Some(art.to_string()),
            badge: Some("电台".to_string()),
            track_input: None,
        });
    }
    shelves.push(EditorialShelf {
        title: "私人 FM 与主题电台".to_string(),
        subtitle: "点击立即播放连续电台曲目流".to_string(),
        kind: EditorialShelfKind::RadioStationRow,
        items: st_items,
    });

    if let Ok(j) = ne_get_json("https://music.163.com/api/personalized/djprogram", cookie).await {
        if let Some(arr) = j.get("result").and_then(Value::as_array) {
            let mut dj_items = Vec::new();
            for item in arr.iter().take(10) {
                let main_song = item.pointer("/program/mainSong");
                let Some(song_id) = main_song.and_then(|s| s.get("id")).and_then(Value::as_i64) else {
                    continue;
                };
                let title = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let sub = item
                    .pointer("/program/radio/name")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("copywriter").and_then(Value::as_str))
                    .unwrap_or("播客电台")
                    .to_string();
                let art = upgrade_https_cover(
                    item.get("picUrl").and_then(Value::as_str).unwrap_or_default(),
                    480,
                );
                dj_items.push(EditorialItem {
                    id: format!("ne:{song_id}"),
                    kind: "ne-song".to_string(),
                    play_kind: "ne-song".to_string(),
                    title,
                    subtitle: sub,
                    badge: "播客".to_string(),
                    artwork_url: art,
                    motion_video_url: None,
                    motion_tall_video_url: None,
                    storefront: "ne".to_string(),
                });
            }
            if !dj_items.is_empty() {
                shelves.push(EditorialShelf {
                    title: "推荐播客节目".to_string(),
                    subtitle: "网易云精选声音电台".to_string(),
                    kind: EditorialShelfKind::SquareShelf,
                    items: dj_items,
                });
            }
        }
    }

    Ok((shelves, cards))
}

pub async fn fetch_station_tracks(station_id: &str, cookie: Option<&str>) -> Result<Vec<AgentTrackInput>, String> {
    if station_id == "fm" {
        let j = ne_get_json("https://music.163.com/api/v1/radio/get", cookie).await?;
        let mut tracks = Vec::new();
        if let Some(arr) = j.get("data").and_then(Value::as_array) {
            for v in arr {
                if let Some(tr) = parse_ne_song(v, None) {
                    tracks.push(tr);
                }
            }
        }
        if !tracks.is_empty() {
            return Ok(tracks);
        }
    }
    // Numeric playlist-backed radio channel (e.g. 19723756 / 3778678)
    let det = fetch_container_detail("ne-playlist", station_id, "网易云电台", cookie).await?;
    Ok(det.tracks)
}

pub async fn search_catalog(
    query: &str,
    cookie: Option<&str>,
) -> Result<(Vec<NetEaseCard>, Vec<NetEaseCard>, Vec<AgentTrackInput>), String> {
    let enc: String = url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect();
    let u_songs = format!("https://music.163.com/api/search/get/?s={enc}&type=1&limit=24&offset=0");
    let u_albums = format!("https://music.163.com/api/search/get/?s={enc}&type=10&limit=12&offset=0");
    let u_pls = format!("https://music.163.com/api/search/get/?s={enc}&type=1000&limit=12&offset=0");

    let (r_s, r_a, r_p) = tokio::join!(
        ne_post_form_json(&u_songs, &[], cookie),
        ne_post_form_json(&u_albums, &[], cookie),
        ne_post_form_json(&u_pls, &[], cookie),
    );

    let mut songs = Vec::new();
    if let Ok(j) = r_s {
        if let Some(arr) = j.pointer("/result/songs").and_then(Value::as_array) {
            // Note: /api/search/get song items sometimes omit album.picUrl; enrich first 18 via /api/v3/song/detail for crisp covers!
            let ids: Vec<i64> = arr
                .iter()
                .filter_map(|v| v.get("id").and_then(Value::as_i64))
                .take(24)
                .collect();
            if !ids.is_empty() {
                if let Ok(enriched) = fetch_songs_detail_batch(&ids, cookie).await {
                    if !enriched.is_empty() {
                        songs = enriched;
                    }
                }
            }
            if songs.is_empty() {
                for v in arr {
                    if let Some(tr) = parse_ne_song(v, None) {
                        songs.push(tr);
                    }
                }
            }
        }
    }

    let mut albums = Vec::new();
    if let Ok(j) = r_a {
        if let Some(arr) = j.pointer("/result/albums").and_then(Value::as_array) {
            for v in arr {
                let Some(id) = v.get("id").and_then(Value::as_i64) else { continue };
                let title = v.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let artist = v
                    .pointer("/artist/name")
                    .and_then(Value::as_str)
                    .unwrap_or("网易云专辑")
                    .to_string();
                let art = upgrade_https_cover(
                    v.get("picUrl")
                        .or_else(|| v.get("blurPicUrl"))
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    480,
                );
                albums.push(NetEaseCard {
                    id: id.to_string(),
                    kind: "ne-album".to_string(),
                    title,
                    subtitle: artist,
                    description: String::new(),
                    artwork_url: if art.is_empty() { None } else { Some(art) },
                    badge: Some("专辑".to_string()),
                    track_input: None,
                });
            }
        }
    }

    let mut playlists = Vec::new();
    if let Ok(j) = r_p {
        if let Some(arr) = j.pointer("/result/playlists").and_then(Value::as_array) {
            for v in arr {
                let Some(id) = v.get("id").and_then(Value::as_i64) else { continue };
                let title = v.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let creator = v
                    .pointer("/creator/nickname")
                    .and_then(Value::as_str)
                    .unwrap_or("网易云歌单")
                    .to_string();
                let art = upgrade_https_cover(
                    v.get("coverImgUrl").and_then(Value::as_str).unwrap_or_default(),
                    480,
                );
                playlists.push(NetEaseCard {
                    id: id.to_string(),
                    kind: "ne-playlist".to_string(),
                    title,
                    subtitle: creator,
                    description: String::new(),
                    artwork_url: if art.is_empty() { None } else { Some(art) },
                    badge: Some("歌单".to_string()),
                    track_input: None,
                });
            }
        }
    }

    Ok((albums, playlists, songs))
}

pub async fn fetch_songs_detail_batch(ids: &[i64], cookie: Option<&str>) -> Result<Vec<AgentTrackInput>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let c_param = format!(
        "[{}]",
        ids.iter()
            .take(120)
            .map(|id| format!(r#"{{"id":{id}}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let json = ne_post_form_json(
        "https://music.163.com/api/v3/song/detail",
        &[("c", c_param)],
        cookie,
    )
    .await?;
    let mut out = Vec::new();
    if let Some(arr) = json.get("songs").and_then(Value::as_array) {
        for v in arr {
            if let Some(tr) = parse_ne_song(v, None) {
                out.push(tr);
            }
        }
    }
    Ok(out)
}

pub async fn fetch_user_playlists(uid: u64, cookie: &str) -> Result<Vec<NetEaseCard>, String> {
    if uid == 0 || cookie.trim().is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("https://music.163.com/api/user/playlist?uid={uid}&limit=32&offset=0");
    let json = ne_get_json(&url, Some(cookie)).await?;
    let mut out = Vec::new();
    if let Some(arr) = json.get("playlist").and_then(Value::as_array) {
        for item in arr {
            let Some(id) = item.get("id").and_then(Value::as_i64) else { continue };
            let title = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let cnt = item.get("trackCount").and_then(Value::as_u64).unwrap_or(0);
            let art = upgrade_https_cover(
                item.get("coverImgUrl").and_then(Value::as_str).unwrap_or_default(),
                480,
            );
            out.push(NetEaseCard {
                id: id.to_string(),
                kind: "ne-playlist".to_string(),
                title,
                subtitle: format!("{cnt} 首歌曲"),
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                artwork_url: if art.is_empty() { None } else { Some(art) },
                badge: Some("网易云歌单".to_string()),
                track_input: None,
            });
        }
    }
    Ok(out)
}

pub async fn fetch_daily_songs(cookie: &str) -> Result<Vec<AgentTrackInput>, String> {
    if cookie.trim().is_empty() {
        return Err("请先在设置中扫码或粘贴 MUSIC_U 登录网易云账号".to_string());
    }
    let json = ne_get_json("https://music.163.com/api/v3/discovery/recommend/songs", Some(cookie)).await?;
    let mut out = Vec::new();
    if let Some(arr) = json
        .pointer("/data/dailySongs")
        .or_else(|| json.get("recommend"))
        .and_then(Value::as_array)
    {
        for v in arr {
            if let Some(tr) = parse_ne_song(v, None) {
                out.push(tr);
            }
        }
    }
    if out.is_empty() {
        return Err("未能拉取每日推荐，请检查网易云登录 Cookie 是否有效".to_string());
    }
    Ok(out)
}

pub async fn fetch_container_detail(
    kind: &str,
    id: &str,
    title_hint: &str,
    cookie: Option<&str>,
) -> Result<NetEaseDetail, String> {
    if kind == "ne-daily" {
        let tracks = fetch_daily_songs(cookie.unwrap_or_default()).await?;
        let art = tracks.first().and_then(|t| t.artwork_url.clone());
        return Ok(NetEaseDetail {
            id: "daily".to_string(),
            kind: "ne-daily".to_string(),
            title: "每日歌曲推荐".to_string(),
            subtitle: format!("网易云每日定制 · {} 首歌曲", tracks.len()),
            description: "根据你的网易云听歌偏好每日 6:00 更新".to_string(),
            artwork_url: art,
            tracks,
            related_albums: Vec::new(),
        });
    }

    if kind == "ne-album" {
        let url = format!("https://music.163.com/api/v1/album/{id}");
        let json = ne_get_json(&url, cookie).await?;
        let alb = json.get("album").unwrap_or(&Value::Null);
        let title = alb
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(title_hint)
            .to_string();
        let subtitle = alb
            .pointer("/artist/name")
            .and_then(Value::as_str)
            .unwrap_or("网易云专辑")
            .to_string();
        let description = alb
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let artwork_url = alb
            .get("picUrl")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|u| upgrade_https_cover(u, 600));
        let tracks = json
            .get("songs")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| parse_ne_song(v, artwork_url.as_deref()))
                    .collect()
            })
            .unwrap_or_default();
        return Ok(NetEaseDetail {
            id: id.to_string(),
            kind: kind.to_string(),
            title,
            subtitle,
            description,
            artwork_url,
            tracks,
            related_albums: Vec::new(),
        });
    }

    if kind == "ne-artist" {
        let url = format!("https://music.163.com/api/v1/artist/{id}");
        let alb_url = format!("https://music.163.com/api/artist/albums/{id}?offset=0&limit=24");
        let (artist_res, albums_res) = tokio::join!(
            ne_get_json(&url, cookie),
            ne_get_json(&alb_url, cookie)
        );
        let json = artist_res?;
        let artist = json.get("artist").unwrap_or(&Value::Null);
        let title = artist
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(title_hint)
            .to_string();
        let description = artist
            .get("briefDesc")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let artwork_url = artist
            .get("picUrl")
            .or_else(|| artist.get("img1v1Url"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|u| upgrade_https_cover(u, 600));
        let tracks: Vec<AgentTrackInput> = json
            .get("hotSongs")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| parse_ne_song(v, artwork_url.as_deref()))
                    .collect()
            })
            .unwrap_or_default();
        let mut related_albums = Vec::new();
        if let Ok(aj) = albums_res {
            if let Some(arr) = aj.get("hotAlbums").and_then(Value::as_array) {
                for a in arr.iter().take(24) {
                    let Some(aid) = a.get("id").and_then(Value::as_i64) else {
                        continue;
                    };
                    let aname = a
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("未知专辑")
                        .to_string();
                    let aart = a
                        .get("picUrl")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(|u| upgrade_https_cover(u, 360));
                    let sz = a.get("size").and_then(Value::as_u64).unwrap_or(0);
                    related_albums.push(NetEaseCard {
                        id: aid.to_string(),
                        kind: "ne-album".to_string(),
                        title: aname,
                        subtitle: if sz > 0 { format!("{sz} 首歌曲") } else { title.clone() },
                        description: String::new(),
                        artwork_url: aart,
                        badge: None,
                        track_input: None,
                    });
                }
            }
        }
        return Ok(NetEaseDetail {
            id: id.to_string(),
            kind: kind.to_string(),
            title,
            subtitle: format!("网易云音乐 · {} 首热门 · {} 张专辑", tracks.len(), related_albums.len()),
            description,
            artwork_url,
            tracks,
            related_albums,
        });
    }

    // Default: ne-playlist / toplist
    let url = format!("https://music.163.com/api/v6/playlist/detail?id={id}&n=200");
    let json = ne_get_json(&url, cookie).await?;
    let pl = json
        .get("playlist")
        .ok_or_else(|| "未能加载网易云歌单详情".to_string())?;
    let title = pl
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(title_hint)
        .to_string();
    let creator = pl
        .pointer("/creator/nickname")
        .and_then(Value::as_str)
        .unwrap_or("网易云歌单");
    let description = pl
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let artwork_url = pl
        .get("coverImgUrl")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|u| upgrade_https_cover(u, 600));

    let mut tracks: Vec<AgentTrackInput> = pl
        .get("tracks")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| parse_ne_song(v, artwork_url.as_deref()))
                .collect()
        })
        .unwrap_or_default();

    // If playlist has more trackIds than returned in `tracks`, fetch up to 100 tracks via batch detail!
    if let Some(tids) = pl.get("trackIds").and_then(Value::as_array) {
        if tids.len() > tracks.len() && tracks.len() < 80 {
            let ids: Vec<i64> = tids
                .iter()
                .filter_map(|v| v.get("id").and_then(Value::as_i64))
                .take(100)
                .collect();
            if let Ok(full) = fetch_songs_detail_batch(&ids, cookie).await {
                if full.len() > tracks.len() {
                    tracks = full;
                }
            }
        }
    }

    Ok(NetEaseDetail {
        id: id.to_string(),
        kind: "ne-playlist".to_string(),
        title,
        subtitle: format!("{creator} · {} 首歌曲", tracks.len()),
        description,
        artwork_url,
        tracks,
        related_albums: Vec::new(),
    })
}

pub async fn resolve_artist_by_name(name: &str, cookie: Option<&str>) -> Option<(String, String)> {
    let clean = name
        .split(&['/', ',', '&', '、', '，'][..])
        .next()
        .unwrap_or(name)
        .trim();
    if clean.is_empty() {
        return None;
    }
    let json = ne_post_form_json(
        "https://music.163.com/api/cloudsearch/pc",
        &[
            ("s", clean.to_string()),
            ("type", "100".to_string()),
            ("limit", "5".to_string()),
            ("offset", "0".to_string()),
        ],
        cookie,
    )
    .await
    .ok()?;
    let arr = json.pointer("/result/artists").and_then(Value::as_array)?;
    let want = crate::backends::normalize_match_text(clean);
    let best = arr
        .iter()
        .find(|v| {
            v.get("name")
                .and_then(Value::as_str)
                .is_some_and(|nm| crate::backends::normalize_match_text(nm) == want)
        })
        .or_else(|| arr.first())?;
    let id = best.get("id").and_then(Value::as_i64)?.to_string();
    let nm = best
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(clean)
        .to_string();
    Some((id, nm))
}

pub async fn resolve_album_by_name(
    album_name: &str,
    artist_name: &str,
    cookie: Option<&str>,
) -> Option<(String, String)> {
    let alb = album_name.trim();
    if alb.is_empty() {
        return None;
    }
    let q = if artist_name.trim().is_empty() {
        alb.to_string()
    } else {
        format!("{alb} {}", artist_name.trim())
    };
    let json = ne_post_form_json(
        "https://music.163.com/api/cloudsearch/pc",
        &[
            ("s", q),
            ("type", "10".to_string()),
            ("limit", "5".to_string()),
            ("offset", "0".to_string()),
        ],
        cookie,
    )
    .await
    .ok()?;
    let arr = json.pointer("/result/albums").and_then(Value::as_array)?;
    let want = crate::backends::normalize_match_text(alb);
    let best = arr
        .iter()
        .find(|v| {
            v.get("name")
                .and_then(Value::as_str)
                .is_some_and(|nm| crate::backends::normalize_match_text(nm) == want)
        })
        .or_else(|| arr.first())?;
    let id = best.get("id").and_then(Value::as_i64)?.to_string();
    let nm = best
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(alb)
        .to_string();
    Some((id, nm))
}

pub async fn create_playlist(name: &str, cookie: &str) -> Result<String, String> {
    if cookie.trim().is_empty() {
        return Err("创建网易云歌单需要先扫码或粘贴 Cookie 登录网易云账号".to_string());
    }
    let json = ne_post_form_json(
        "https://music.163.com/api/playlist/create",
        &[
            ("name", name.trim().to_string()),
            ("privacy", "0".to_string()),
            ("type", "NORMAL".to_string()),
        ],
        Some(cookie),
    )
    .await?;
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 200 {
        let msg = json
            .get("message")
            .or_else(|| json.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("创建歌单失败，请确认 Cookie 登录态有效");
        return Err(format!("网易云创建歌单失败 ({code}): {msg}"));
    }
    let pid = json
        .get("id")
        .and_then(Value::as_i64)
        .or_else(|| json.pointer("/playlist/id").and_then(Value::as_i64))
        .ok_or_else(|| "网易云未返回新建歌单 ID".to_string())?;
    Ok(pid.to_string())
}

pub async fn add_tracks_to_playlist(
    playlist_id: &str,
    song_ids: &[String],
    cookie: &str,
) -> Result<(), String> {
    if cookie.trim().is_empty() {
        return Err("收藏歌曲到网易云歌单需要先登录网易云账号".to_string());
    }
    let clean_ids: Vec<String> = song_ids
        .iter()
        .map(|s| strip_ne_id(s))
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .collect();
    if clean_ids.is_empty() {
        return Err("没有有效的网易云歌曲 ID".to_string());
    }
    for chunk in clean_ids.chunks(50) {
        let arr_str = format!("[{}]", chunk.join(","));
        let json = ne_post_form_json(
            "https://music.163.com/api/playlist/tracks",
            &[
                ("op", "add".to_string()),
                ("pid", playlist_id.trim().to_string()),
                ("trackIds", arr_str),
                ("imme", "true".to_string()),
            ],
            Some(cookie),
        )
        .await?;
        let code = json.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 200 && code != 502 {
            let msg = json
                .get("message")
                .or_else(|| json.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("写入歌单曲目失败");
            return Err(format!("网易云写入歌单失败 ({code}): {msg}"));
        }
    }
    Ok(())
}

pub async fn search_best_song_match(
    title: &str,
    artist: &str,
    duration_sec: f64,
    cookie: Option<&str>,
) -> Option<AgentTrackInput> {
    let clean_t = title.trim();
    if clean_t.is_empty() {
        return None;
    }
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
    let json = ne_post_form_json(
        "https://music.163.com/api/cloudsearch/pc",
        &[
            ("s", q),
            ("type", "1".to_string()),
            ("limit", "8".to_string()),
            ("offset", "0".to_string()),
        ],
        cookie,
    )
    .await
    .ok()?;
    let arr = json.pointer("/result/songs").and_then(Value::as_array)?;
    let mut best: Option<(f32, AgentTrackInput)> = None;
    for node in arr {
        let Some(tr) = parse_ne_song(node, None) else {
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

// ============================================================================
// Audio Stream URL Resolver & Direct Exact-ID YRC Lyrics
// ============================================================================

pub async fn resolve_song_stream(
    raw_song_id: &str,
    quality: NetEaseQuality,
    cookie: Option<&str>,
) -> Result<ResolvedAudioStream, String> {
    let sid = strip_ne_id(raw_song_id);
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("无效的网易云歌曲 ID: {raw_song_id}"));
    }

    let encode_type = match quality {
        NetEaseQuality::Lossless | NetEaseQuality::HiRes => "flac",
        _ => "mp3",
    };
    let ids_json = format!("[{sid}]");

    // Primary: interface3 player/url/v1 (supports standard/exhigh/lossless/hires + VIP cookie)
    for endpoint in [
        "https://interface3.music.163.com/api/song/enhance/player/url/v1",
        "https://music.163.com/api/song/enhance/player/url/v1",
    ] {
        if let Ok(json) = ne_post_form_json(
            endpoint,
            &[
                ("ids", ids_json.clone()),
                ("level", quality.api_level().to_string()),
                ("encodeType", encode_type.to_string()),
            ],
            cookie,
        )
        .await
        {
            if let Some(d) = json.pointer("/data/0") {
                if let Some(url) = d.get("url").and_then(Value::as_str).filter(|u| u.starts_with("http")) {
                    let br = d.get("br").and_then(Value::as_u64).unwrap_or(128_000) as u32;
                    let ext = d
                        .get("type")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(encode_type)
                        .to_ascii_lowercase();
                    let is_trial = d.get("freeTrialInfo").is_some_and(|v| !v.is_null());
                    let qlabel = if is_trial {
                        "VIP 试听".to_string()
                    } else if ext == "flac" && br >= 1_200_000 {
                        "Hi-Res 无损".to_string()
                    } else if ext == "flac" {
                        "无损 FLAC".to_string()
                    } else if br >= 300_000 {
                        "极高 320k".to_string()
                    } else {
                        "标准音质".to_string()
                    };
                    return Ok(ResolvedAudioStream {
                        url: url.to_string(),
                        referer: NE_REFERER.to_string(),
                        cookie: cookie.map(|s| s.to_string()),
                        file_ext: ext,
                        quality_label: qlabel,
                        nominal_bitrate_kbps: (br / 1000).max(128),
                        is_trial,
                    });
                }
            }
        }
    }

    // Secondary: legacy player/url with br parameter
    let legacy_url = format!(
        "https://music.163.com/api/song/enhance/player/url?ids=[{sid}]&br={}",
        quality.bitrate_param()
    );
    if let Ok(json) = ne_get_json(&legacy_url, cookie).await {
        if let Some(d) = json.pointer("/data/0") {
            if let Some(url) = d.get("url").and_then(Value::as_str).filter(|u| u.starts_with("http")) {
                let br = d.get("br").and_then(Value::as_u64).unwrap_or(128_000) as u32;
                let ext = d
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("mp3")
                    .to_ascii_lowercase();
                let is_trial = d.get("freeTrialInfo").is_some_and(|v| !v.is_null());
                return Ok(ResolvedAudioStream {
                    url: url.to_string(),
                    referer: NE_REFERER.to_string(),
                    cookie: cookie.map(|s| s.to_string()),
                    file_ext: ext,
                    quality_label: if is_trial { "VIP 试听".to_string() } else { quality.label().to_string() },
                    nominal_bitrate_kbps: (br / 1000).max(128),
                    is_trial,
                });
            }
        }
    }

    Err("该曲目在网易云需要 VIP 或暂无版权音源（可在设置登录黑胶 VIP 账号）".to_string())
}

/// Fetches exact NetEase YRC syllable-synced lyrics + Chinese translation directly by song ID!
pub async fn fetch_lyrics_by_id(raw_song_id: &str, cookie: Option<&str>) -> Option<ParsedLyrics> {
    let sid = strip_ne_id(raw_song_id);
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let url = format!(
        "https://music.163.com/api/song/lyric?os=pc&id={sid}&lv=-1&kv=-1&tv=-1&yv=-1"
    );
    let json = ne_get_json(&url, cookie).await.ok()?;
    let tlyric = json
        .pointer("/tlyric/lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());

    if let Some(yrc) = json
        .pointer("/yrc/lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        if let Some(mut parsed) = lyrics::parse_yrc_lyrics(yrc, tlyric) {
            parsed.source_name = "网易云逐字歌词 (YRC)".to_string();
            return Some(parsed);
        }
    }
    if let Some(lrc) = json
        .pointer("/lrc/lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        if let Some(parsed) = lyrics::parse_lrc_lyrics(lrc, tlyric, "网易云歌词 (LRC)") {
            return Some(parsed);
        }
    }
    None
}
