use crate::am_agent::AgentTrackInput;
use crate::netease_api;
use once_cell::sync::Lazy;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EAPI_KEY: &[u8; 16] = b"e82ckenh8dichen8";
const EAPI_SEP: &str = "-36cd479b6b5-";
const EAPI_PRIMARY_DOMAIN: &str = "https://interfacepc.music.163.com";
const EAPI_FALLBACK_DOMAIN: &str = "https://interface.music.163.com";

static LT_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(7))
        .pool_max_idle_per_host(12)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 NeteaseMusicDesktop/3.0.18.203152")
        .build()
        .expect("listen_together http client")
});

static LT_REDIRECT_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .tcp_nodelay(true)
        .user_agent("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148")
        .build()
        .expect("listen_together redirect client")
});

// ============================================================================
// Pure Rust RFC 1321 MD5 + FIPS 197 AES-128-ECB (Zero External Dependencies)
// ============================================================================

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

pub fn md5_hex(input: &[u8]) -> String {
    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(((input.len() + 9 + 63) / 64) * 64);
    msg.extend_from_slice(input);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            let b = &chunk[i * 4..i * 4 + 4];
            *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(MD5_K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(MD5_S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = String::with_capacity(32);
    for w in [a0, b0, c0, d0] {
        for byte in w.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

const AES_RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

#[inline]
fn xtime(x: u8) -> u8 {
    (x << 1) ^ if (x & 0x80) != 0 { 0x1b } else { 0 }
}

fn aes128_expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    let mut rk = [[0u8; 16]; 11];
    rk[0] = *key;
    for r in 1..=10 {
        let prev = rk[r - 1];
        let mut t = [prev[13], prev[14], prev[15], prev[12]];
        for b in &mut t {
            *b = AES_SBOX[*b as usize];
        }
        t[0] ^= AES_RCON[r - 1];
        let mut cur = [0u8; 16];
        for i in 0..4 {
            cur[i] = prev[i] ^ t[i];
        }
        for i in 4..16 {
            cur[i] = prev[i] ^ cur[i - 4];
        }
        rk[r] = cur;
    }
    rk
}

fn aes128_encrypt_block(rk: &[[u8; 16]; 11], input: &[u8; 16]) -> [u8; 16] {
    let mut s = *input;
    for i in 0..16 {
        s[i] ^= rk[0][i];
    }
    for round in 1..=10 {
        for b in &mut s {
            *b = AES_SBOX[*b as usize];
        }
        let t1 = s[1];
        s[1] = s[5];
        s[5] = s[9];
        s[9] = s[13];
        s[13] = t1;
        s.swap(2, 10);
        s.swap(6, 14);
        let t3 = s[15];
        s[15] = s[11];
        s[11] = s[7];
        s[7] = s[3];
        s[3] = t3;

        if round < 10 {
            for c in 0..4 {
                let i = c * 4;
                let (a0, a1, a2, a3) = (s[i], s[i + 1], s[i + 2], s[i + 3]);
                let tmp = a0 ^ a1 ^ a2 ^ a3;
                s[i] ^= tmp ^ xtime(a0 ^ a1);
                s[i + 1] ^= tmp ^ xtime(a1 ^ a2);
                s[i + 2] ^= tmp ^ xtime(a2 ^ a3);
                s[i + 3] ^= tmp ^ xtime(a3 ^ a0);
            }
        }
        for i in 0..16 {
            s[i] ^= rk[round][i];
        }
    }
    s
}

pub fn aes128_ecb_encrypt_pkcs7(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let rk = aes128_expand_key(key);
    let pad = 16 - (data.len() % 16);
    let total = data.len() + pad;
    let mut padded = Vec::with_capacity(total);
    padded.extend_from_slice(data);
    padded.resize(total, pad as u8);

    let mut out = Vec::with_capacity(total);
    for chunk in padded.chunks_exact(16) {
        let mut blk = [0u8; 16];
        blk.copy_from_slice(chunk);
        out.extend_from_slice(&aes128_encrypt_block(&rk, &blk));
    }
    out
}

/// Official NetEase EAPI `params` encryption (AES-128-ECB + MD5 digest, uppercase hex).
pub fn eapi_encrypt_params(api_path: &str, payload_json: &str) -> String {
    let msg = format!("nobody{api_path}use{payload_json}md5forencrypt");
    let digest = md5_hex(msg.as_bytes());
    let plain = format!("{api_path}{EAPI_SEP}{payload_json}{EAPI_SEP}{digest}");
    let cipher = aes128_ecb_encrypt_pkcs7(EAPI_KEY, plain.as_bytes());
    let mut hex = String::with_capacity(cipher.len() * 2);
    for b in cipher {
        hex.push_str(&format!("{b:02X}"));
    }
    hex
}

fn extract_cookie_kv(cookie: &str, key: &str) -> String {
    let prefix = format!("{key}=");
    for seg in cookie.split(';') {
        let t = seg.trim();
        if let Some(v) = t.strip_prefix(&prefix) {
            return v.trim().to_string();
        }
    }
    String::new()
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn ne_eapi_post(api_path: &str, mut body: Value, cookie: &str) -> Result<Value, String> {
    let clean_cookie = netease_api::normalize_netease_cookie(cookie);
    let music_u = extract_cookie_kv(&clean_cookie, "MUSIC_U");
    if music_u.is_empty() {
        return Err("请先在「设置 -> 网易云音乐」扫码或粘贴 Cookie 登录账号".to_string());
    }
    let csrf = extract_cookie_kv(&clean_cookie, "__csrf");
    let now_ms = now_unix_ms();
    let req_id = format!("{}_{:04}", now_ms, (now_ms % 10_000));
    let buildver = (now_ms / 1000).to_string();

    let header_obj = json!({
        "osver": "Microsoft-Windows-10-Professional-build-22631-64bit",
        "deviceId": "amlight-win-desktop",
        "os": "pc",
        "appver": "3.0.18.203152",
        "versioncode": "140",
        "mobilename": "",
        "buildver": buildver,
        "resolution": "1920x1080",
        "__csrf": csrf,
        "channel": "netease",
        "requestId": req_id,
        "MUSIC_U": music_u,
    });
    if let Some(obj) = body.as_object_mut() {
        obj.insert("header".to_string(), header_obj);
    }
    let payload_str = serde_json::to_string(&body).map_err(|e| format!("序列化失败: {e}"))?;
    let params_hex = eapi_encrypt_params(api_path, &payload_str);

    let eapi_sub = api_path
        .strip_prefix("/api/")
        .unwrap_or(api_path.trim_start_matches('/'));
    let cookie_hdr = netease_api::build_cookie_header(Some(&clean_cookie));
    let mut last_err = String::new();

    for domain in [EAPI_PRIMARY_DOMAIN, EAPI_FALLBACK_DOMAIN] {
        let url = format!("{domain}/eapi/{eapi_sub}");
        let res = LT_HTTP
            .post(&url)
            .header("Referer", "https://music.163.com/")
            .header("Origin", "https://music.163.com")
            .header("Cookie", &cookie_hdr)
            .form(&[("params", params_hex.as_str())])
            .send()
            .await;
        match res {
            Ok(resp) if resp.status().is_success() => {
                let text = resp
                    .text()
                    .await
                    .map_err(|e| format!("读取网易云响应失败: {e}"))?;
                let v: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("解析网易云一起听 JSON 失败: {e}"))?;
                return Ok(v);
            }
            Ok(resp) => {
                last_err = format!("网易云一起听 HTTP {}", resp.status().as_u16());
            }
            Err(e) => {
                last_err = format!("网易云一起听请求失败: {e}");
            }
        }
    }
    Err(last_err)
}

/// EAPI helper to query `/api/music-vip-membership/front/vip/info` when direct GET lacks `data`.
pub async fn fetch_vip_info_eapi(uid: u64, cookie: &str) -> Result<Value, String> {
    ne_eapi_post(
        "/api/music-vip-membership/front/vip/info",
        json!({ "userId": if uid > 0 { uid.to_string() } else { String::new() } }),
        cookie,
    )
    .await
}

/// EAPI helper to query `/api/nuser/account/get` when direct GET returns incomplete/empty profile.
pub async fn fetch_account_eapi(cookie: &str) -> Result<Value, String> {
    ne_eapi_post("/api/nuser/account/get", json!({}), cookie).await
}

async fn ne_fast_get(url: &str, cookie: &str) -> Result<Value, String> {
    let clean_cookie = netease_api::normalize_netease_cookie(cookie);
    let cookie_hdr = netease_api::build_cookie_header(Some(&clean_cookie));
    let resp = LT_HTTP
        .get(url)
        .timeout(Duration::from_secs(4))
        .header("Referer", "https://music.163.com/")
        .header("Origin", "https://music.163.com")
        .header("Cookie", &cookie_hdr)
        .send()
        .await
        .map_err(|e| format!("网易云请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("网易云 HTTP {}", resp.status().as_u16()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("解析网易云 JSON 失败: {e}"))
}

// ============================================================================
// Listen Together Domain Types & Smart Invite Parser (with Short-Link Support!)
// ============================================================================

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenTogetherUser {
    pub user_id: u64,
    pub nickname: String,
    pub avatar_url: Option<String>,
    pub is_host: bool,
    #[serde(default)]
    pub vip_type: i64,
    #[serde(default)]
    pub is_vip: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenTogetherFriend {
    pub user_id: u64,
    pub nickname: String,
    pub avatar_url: Option<String>,
    pub vip_type: i64,
    pub is_vip: bool,
    pub mutual: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenTogetherInvite {
    pub room_id: String,
    pub inviter_id: u64,
    pub inviter_name: String,
    pub inviter_avatar: Option<String>,
    pub song_id: Option<String>,
    pub time_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedListenInvite {
    pub room_id: String,
    pub inviter_id: Option<u64>,
    pub song_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenTogetherStatus {
    pub code: i64,
    pub in_room: bool,
    pub room_closed: bool,
    pub room_id: String,
    pub creator_id: u64,
    pub room_type: String,
    pub users: Vec<ListenTogetherUser>,
    pub remote_song_id: Option<String>,
    pub remote_play_status: String, // "PLAY" | "PAUSE" | ""
    pub remote_command_type: String, // "PLAY" | "PAUSE" | "GOTO" | "seek" | ...
    pub remote_progress_ms: i64,
    pub remote_client_seq: i64,
    pub remote_list_version: i64,
    pub remote_cmd_user_id: u64,
    pub remote_cmd_time_ms: u64,
    pub remote_playlist_ids: Vec<i64>,
    pub force_resync: bool,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteSyncAction {
    Noop,
    RoomClosed,
    SwitchTrack {
        song_id: String,
        play: bool,
        progress_ms: i64,
    },
    SetPlayPause {
        play: bool,
        progress_ms: i64,
    },
    SeekTo {
        progress_ms: i64,
    },
}

/// Official NetEase Listen Together share link compatible with official mobile/desktop apps.
pub fn build_share_url(room_id: &str, inviter_id: u64, song_id: Option<&str>) -> String {
    let sid = song_id
        .map(netease_api::strip_ne_id)
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| "186016".to_string());
    format!(
        "https://st.music.163.com/listen-together/share/?songId={sid}&roomId={room_id}&inviterId={inviter_id}"
    )
}

fn decode_url_twice(raw: &str) -> String {
    let once = percent_decode_str(raw).decode_utf8_lossy().into_owned();
    percent_decode_str(&once).decode_utf8_lossy().into_owned()
}

fn extract_query_param(text: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let lower = text.to_ascii_lowercase();
    let needle_low = needle.to_ascii_lowercase();
    let pos = lower.find(&needle_low)?;
    let rest = &text[pos + needle.len()..];
    let end = rest
        .find(|c: char| matches!(c, '&' | '#' | ' ' | '\t' | '\r' | '\n' | '"' | '\'' | ')' | ']' | '）' | '，' | '。'))
        .unwrap_or(rest.len());
    let val = rest[..end].trim();
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// Extracts the first `http://` or `https://` URL embedded inside Chinese share prose, e.g.
/// `我的耳机分你一半，和我一起听歌吧～快点开看看 https://163cn.tv/blBIyEiR (@网易云音乐)`.
pub fn extract_first_http_url(raw: &str) -> Option<String> {
    let start = raw.find("https://").or_else(|| raw.find("http://"))?;
    let rest = &raw[start..];
    let end = rest
        .find(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '<' | '>' | ')' | ']' | '）' | '】' | '，' | '。' | '！' | '；'
                )
        })
        .unwrap_or(rest.len());
    let url = rest[..end].trim();
    if url.len() >= 10 {
        Some(url.to_string())
    } else {
        None
    }
}

/// Parses any NetEase Listen Together invite string synchronously when `roomId` is already present:
/// - Full share URL (`https://st.music.163.com/listen-together/share/?songId=...&roomId=...&inviterId=...`)
/// - Share copy containing full URL inside Chinese text
/// - Double-encoded `nativeUrl` from private messages
/// - `roomId:inviterId` / `roomId inviterId` / bare `roomId`
pub fn parse_invite_input(raw: &str) -> Option<ParsedListenInvite> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let decoded = decode_url_twice(trimmed);
    if let Some(room_id) = extract_query_param(&decoded, "roomId") {
        let inviter_id = extract_query_param(&decoded, "inviterId")
            .or_else(|| extract_query_param(&decoded, "inviterUserId"))
            .or_else(|| extract_query_param(&decoded, "creatorId"))
            .and_then(|s| s.parse::<u64>().ok());
        let song_id = extract_query_param(&decoded, "songId")
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()));
        return Some(ParsedListenInvite {
            room_id,
            inviter_id,
            song_id,
        });
    }

    // Reject raw prose containing an unresolved short URL like `https://163cn.tv/...`
    // so `resolve_invite_input` follows the HTTP 302 redirect instead of treating Chinese text as a roomId!
    if extract_first_http_url(trimmed).is_some() {
        return None;
    }

    // Fallback: `roomId:inviterId`, `roomId,inviterId`, or `roomId inviterId`
    let parts: Vec<&str> = trimmed
        .split(|c: char| c == ':' || c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() >= 2
        && parts[0].is_ascii()
        && !parts[0].contains('/')
        && parts[1].chars().all(|c| c.is_ascii_digit())
    {
        return Some(ParsedListenInvite {
            room_id: parts[0].to_string(),
            inviter_id: parts[1].parse::<u64>().ok(),
            song_id: None,
        });
    }
    if parts.len() == 1
        && parts[0].len() >= 8
        && parts[0].is_ascii()
        && !parts[0].contains('/')
    {
        return Some(ParsedListenInvite {
            room_id: parts[0].to_string(),
            inviter_id: None,
            song_id: None,
        });
    }
    None
}

/// Resolves ANY NetEase Listen Together invite text asynchronously, including mobile short links
/// (`https://163cn.tv/blBIyEiR` inside Chinese share prose) via fast HTTP 302 Location inspection!
pub async fn resolve_invite_input(raw: &str) -> Result<ParsedListenInvite, String> {
    if let Some(parsed) = parse_invite_input(raw) {
        return Ok(parsed);
    }
    let Some(mut cur_url) = extract_first_http_url(raw) else {
        return Err("未识别到一起听邀请链接或房间号（支持网易云口令短链 https://163cn.tv/...、完整分享链接或 房间号:邀请者ID）".to_string());
    };

    for _hop in 0..5 {
        if let Some(parsed) = parse_invite_input(&cur_url) {
            return Ok(parsed);
        }
        let resp = LT_REDIRECT_HTTP
            .get(&cur_url)
            .send()
            .await
            .map_err(|e| format!("解析网易云短链失败: {e}"))?;

        if let Some(loc) = resp.headers().get(reqwest::header::LOCATION) {
            if let Ok(loc_str) = loc.to_str() {
                let next_url = if loc_str.starts_with("http://") || loc_str.starts_with("https://") {
                    loc_str.to_string()
                } else if let Ok(base) = url::Url::parse(&cur_url) {
                    base.join(loc_str)
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| loc_str.to_string())
                } else {
                    loc_str.to_string()
                };
                if let Some(parsed) = parse_invite_input(&next_url) {
                    return Ok(parsed);
                }
                cur_url = next_url;
                continue;
            }
        }
        // Fallback: inspect final HTML/body in case of meta-refresh or inline JS link
        if let Ok(body) = resp.text().await {
            if let Some(parsed) = parse_invite_input(&body) {
                return Ok(parsed);
            }
        }
        break;
    }

    Err("短链跳转后未包含一起听房间信息，请确认链接有效且房间未过期".to_string())
}

// ============================================================================
// Official NetEase Listen Together API Operations & Authoritative Sync Parser
// ============================================================================

fn parse_u64_field(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_i64().filter(|&n| n >= 0).map(|n| n as u64))
                .or_else(|| x.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
        })
        .unwrap_or(0)
}

fn parse_i64_field(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_u64().map(|n| n as i64))
            .or_else(|| x.as_f64().map(|f| f.round() as i64))
            .or_else(|| x.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
    })
}

fn parse_song_id_opt(v: &Value, keys: &[&str]) -> Option<String> {
    for &k in keys {
        if let Some(x) = v.get(k) {
            if let Some(n) = x.as_i64() {
                if n > 0 {
                    return Some(n.to_string());
                }
            }
            if let Some(s) = x.as_str() {
                let t = s.trim();
                if !t.is_empty() && t != "-1" && t != "0" && t.chars().all(|c| c.is_ascii_digit()) {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

fn upgrade_avatar_https(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let https = if let Some(rest) = t.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        t.to_string()
    };
    if https.contains('?') {
        Some(https)
    } else {
        Some(format!("{https}?param=120y120"))
    }
}

fn parse_user_vip_flag(u: &Value) -> (i64, bool) {
    let vip_type = parse_i64_field(u, "vipType").unwrap_or(0);
    let rights_assoc = u
        .pointer("/vipRights/associator/rights")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rights_redplus = u
        .pointer("/vipRights/redplus/rights")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rights_pkg = u
        .pointer("/vipRights/musicPackage/rights")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_vip = vip_type >= 11 || rights_assoc || rights_redplus || rights_pkg;
    (vip_type, is_vip)
}

fn extract_play_command_into(st: &mut ListenTogetherStatus, root: &Value, now_ms: u64) {
    let data = root.get("data").unwrap_or(root);
    let cmd_src = data
        .get("playCommand")
        .filter(|v| v.is_object())
        .or_else(|| data.pointer("/roomInfo/playCommand").filter(|v| v.is_object()))
        .or_else(|| data.get("playingInfo").filter(|v| v.is_object()))
        .or_else(|| data.get("status").filter(|v| v.is_object()));

    let Some(cmd) = cmd_src else {
        return;
    };

    if let Some(sid) = parse_song_id_opt(
        cmd,
        &["targetSongId", "songId", "trackId", "currentSongId", "formerSongId"],
    ) {
        st.remote_song_id = Some(sid);
    }

    let raw_play_status = cmd
        .get("playStatus")
        .or_else(|| cmd.get("status"))
        .and_then(|v| {
            v.as_str()
                .map(|s| s.trim().to_ascii_uppercase())
                .or_else(|| v.as_bool().map(|b| if b { "PLAY" } else { "PAUSE" }.to_string()))
        })
        .unwrap_or_default();
    if !raw_play_status.is_empty() {
        st.remote_play_status = raw_play_status;
    }

    if let Some(ct) = cmd.get("commandType").and_then(Value::as_str) {
        st.remote_command_type = ct.trim().to_string();
        if st.remote_play_status.is_empty() {
            let up = ct.trim().to_ascii_uppercase();
            if up == "PAUSE" {
                st.remote_play_status = "PAUSE".to_string();
            } else if matches!(up.as_str(), "PLAY" | "GOTO" | "NEXT" | "PREV") {
                st.remote_play_status = "PLAY".to_string();
            }
        }
    }

    let raw_prog = parse_i64_field(cmd, "progress").unwrap_or(0).max(0);
    let seq = parse_i64_field(cmd, "clientSeq")
        .or_else(|| parse_i64_field(cmd, "seq"))
        .unwrap_or(0);
    if seq > st.remote_client_seq {
        st.remote_client_seq = seq;
    }
    let cmd_uid = parse_u64_field(cmd, "userId").max(parse_u64_field(cmd, "operatorId"));
    if cmd_uid > 0 {
        st.remote_cmd_user_id = cmd_uid;
    }

    // Compensate elapsed wall-clock time when `playStatus == "PLAY"` and server/command timestamp is present!
    let cmd_ts = parse_u64_field(cmd, "timestamp")
        .max(parse_u64_field(cmd, "updateTime"))
        .max(parse_u64_field(cmd, "commandTime"));
    let server_ts = parse_u64_field(data, "serverTime")
        .max(parse_u64_field(root, "serverTime"))
        .max(now_ms);
    st.remote_cmd_time_ms = cmd_ts;
    let adjusted_prog = if st.remote_play_status == "PLAY" && cmd_ts > 1_600_000_000_000 && server_ts >= cmd_ts {
        let elapsed = (server_ts - cmd_ts).min(360_000) as i64;
        raw_prog.saturating_add(elapsed)
    } else {
        raw_prog
    };
    st.remote_progress_ms = adjusted_prog.max(0);
}

fn extract_playlist_meta_into(st: &mut ListenTogetherStatus, root: &Value) {
    let data = root.get("data").unwrap_or(root);
    let pl = data.get("playlist").unwrap_or(data);
    // Extract max `version` from `playlist.version` (array of `{userId, version}` or integer)
    if let Some(arr) = pl.get("version").and_then(Value::as_array) {
        for item in arr {
            if let Some(v) = parse_i64_field(item, "version") {
                if v > st.remote_list_version {
                    st.remote_list_version = v;
                }
            }
        }
    } else if let Some(v) = parse_i64_field(pl, "version") {
        if v > st.remote_list_version {
            st.remote_list_version = v;
        }
    }

    let arr_opt = pl
        .pointer("/displayList/result")
        .or_else(|| pl.get("displayList"))
        .and_then(Value::as_array);
    if let Some(arr) = arr_opt {
        let mut ids = Vec::with_capacity(arr.len().min(120));
        for v in arr.iter().take(120) {
            if let Some(n) = v
                .as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
            {
                if n > 0 && !ids.contains(&n) {
                    ids.push(n);
                }
            }
        }
        st.remote_playlist_ids = ids;
    }
}

pub fn parse_status_json(json: &Value) -> ListenTogetherStatus {
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    let msg = json
        .get("message")
        .or_else(|| json.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if code == 488 {
        return ListenTogetherStatus {
            code,
            in_room: false,
            room_closed: true,
            message: "对方已结束一起听房间".to_string(),
            ..Default::default()
        };
    }

    let data = json.get("data").unwrap_or(json);
    let room_info = data.get("roomInfo").unwrap_or(&Value::Null);
    let room_id = room_info
        .get("roomId")
        .or_else(|| data.get("roomId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let in_room = data
        .get("inRoom")
        .and_then(Value::as_bool)
        .unwrap_or(!room_id.is_empty());
    let creator_id = parse_u64_field(room_info, "creatorId")
        .max(parse_u64_field(room_info, "inviterId"));
    let room_type = room_info
        .get("roomType")
        .or_else(|| room_info.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("NORMAL")
        .to_string();

    let mut users = Vec::new();
    if let Some(arr) = room_info
        .get("roomUsers")
        .or_else(|| data.get("roomUsers"))
        .and_then(Value::as_array)
    {
        for u in arr {
            let uid = parse_u64_field(u, "userId").max(parse_u64_field(u, "uid"));
            let nickname = u
                .get("nickname")
                .or_else(|| u.get("userName"))
                .and_then(Value::as_str)
                .unwrap_or("云村听友")
                .trim()
                .to_string();
            let avatar_url = u
                .get("avatarUrl")
                .and_then(Value::as_str)
                .and_then(upgrade_avatar_https);
            let is_host = uid != 0
                && (uid == creator_id
                    || u.get("identity").and_then(Value::as_str) == Some("HOST"));
            let (vip_type, is_vip) = parse_user_vip_flag(u);
            if uid != 0 || !nickname.is_empty() {
                users.push(ListenTogetherUser {
                    user_id: uid,
                    nickname,
                    avatar_url,
                    is_host,
                    vip_type,
                    is_vip,
                });
            }
        }
    }

    let mut st = ListenTogetherStatus {
        code,
        in_room,
        room_closed: code == 488,
        room_id,
        creator_id,
        room_type,
        users,
        message: msg,
        ..Default::default()
    };
    extract_play_command_into(&mut st, json, now_unix_ms());
    extract_playlist_meta_into(&mut st, json);
    st
}

/// Parses the `/api/listen/together/sync/playlist/get` response, which is the AUTHORITATIVE
/// NetEase source of live `playCommand` (`targetSongId`, `playStatus`, `progress`, `clientSeq`)
/// and shared queue `playlist.displayList` + `playlist.version`!
pub fn parse_sync_playlist_json(room_id: &str, json: &Value, now_ms: u64) -> ListenTogetherStatus {
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code == 488 {
        return ListenTogetherStatus {
            code,
            in_room: false,
            room_closed: true,
            room_id: room_id.to_string(),
            message: "对方已结束一起听房间".to_string(),
            ..Default::default()
        };
    }
    let mut st = ListenTogetherStatus {
        code,
        in_room: code == 200,
        room_closed: false,
        room_id: room_id.to_string(),
        ..Default::default()
    };
    extract_play_command_into(&mut st, json, now_ms);
    extract_playlist_meta_into(&mut st, json);
    st
}

pub async fn create_room(cookie: &str) -> Result<ListenTogetherStatus, String> {
    let json = ne_eapi_post(
        "/api/listen/together/room/create",
        json!({ "refer": "songplay_more" }),
        cookie,
    )
    .await?;
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 200 {
        let msg = json
            .get("message")
            .or_else(|| json.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("创建一起听房间失败");
        return Err(format!("{msg} (code {code})"));
    }
    let mut st = parse_status_json(&json);
    if st.room_id.is_empty() {
        return Err("服务端未返回有效房间 ID（请确认网易云账号状态正常）".to_string());
    }
    st.in_room = true;
    let _ = check_room(&st.room_id, cookie).await;
    Ok(st)
}

pub async fn check_room(room_id: &str, cookie: &str) -> Result<(bool, String), String> {
    let json = ne_eapi_post(
        "/api/listen/together/room/check",
        json!({ "roomId": room_id }),
        cookie,
    )
    .await?;
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code == 488 {
        return Ok((false, "CLOSED".to_string()));
    }
    let data = json.get("data").unwrap_or(&json);
    let joinable = data.get("joinable").and_then(Value::as_bool).unwrap_or(code == 200);
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or(if joinable { "AVAILABLE" } else { "UNAVAILABLE" })
        .to_string();
    Ok((joinable, status))
}

pub async fn accept_invitation(
    room_id: &str,
    inviter_id: Option<u64>,
    cookie: &str,
) -> Result<ListenTogetherStatus, String> {
    let mut body = json!({
        "refer": "inbox_invite",
        "roomId": room_id,
    });
    if let Some(inv) = inviter_id.filter(|&id| id > 0) {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("inviterId".to_string(), json!(inv.to_string()));
        }
    }
    let json = ne_eapi_post("/api/listen/together/play/invitation/accept", body, cookie).await?;
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 200 {
        let msg = json
            .get("message")
            .or_else(|| json.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("加入一起听房间失败");
        return Err(format!("{msg} (code {code})"));
    }
    let actual_room_id = json
        .pointer("/data/roomInfo/roomId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(room_id)
        .to_string();
    let _ = check_room(&actual_room_id, cookie).await;
    // Immediately pull BOTH roomUsers (`status/get`) AND live `playCommand` (`sync/playlist/get`)!
    let mut st = poll_room_sync(&actual_room_id, "0", "PAUSE", 0, true, true, cookie)
        .await
        .unwrap_or_else(|_| parse_status_json(&json));
    if st.room_id.is_empty() {
        st.room_id = actual_room_id;
        st.in_room = true;
    }
    st.force_resync = true;
    Ok(st)
}

pub async fn fetch_status(cookie: &str) -> Result<ListenTogetherStatus, String> {
    let json = ne_eapi_post("/api/listen/together/status/get", json!({}), cookie).await?;
    let mut st = parse_status_json(&json);
    // If already in an active room (e.g. started on mobile), ALSO fetch `sync/playlist/get`
    // immediately so we get the live `playCommand` (`songId`, `playStatus`, `progress`, `clientSeq`)!
    if st.in_room && !st.room_id.is_empty() {
        if let Ok(pl_json) = ne_eapi_post(
            "/api/listen/together/sync/playlist/get",
            json!({ "roomId": st.room_id }),
            cookie,
        )
        .await
        {
            let pl_st = parse_sync_playlist_json(&st.room_id, &pl_json, now_unix_ms());
            if pl_st.room_closed {
                st.room_closed = true;
                st.in_room = false;
            } else {
                if pl_st.remote_song_id.is_some() {
                    st.remote_song_id = pl_st.remote_song_id;
                    st.remote_play_status = pl_st.remote_play_status;
                    st.remote_command_type = pl_st.remote_command_type;
                    st.remote_progress_ms = pl_st.remote_progress_ms;
                    st.remote_cmd_user_id = pl_st.remote_cmd_user_id;
                    st.remote_cmd_time_ms = pl_st.remote_cmd_time_ms;
                }
                st.remote_client_seq = st.remote_client_seq.max(pl_st.remote_client_seq);
                st.remote_list_version = st.remote_list_version.max(pl_st.remote_list_version);
                if !pl_st.remote_playlist_ids.is_empty() {
                    st.remote_playlist_ids = pl_st.remote_playlist_ids;
                }
            }
        }
    }
    Ok(st)
}

/// High-speed parallel room sync tick (`sync/playlist/get` + `heartbeat` + optional `status/get`):
/// retrieves the authoritative live `playCommand` (partner track switches, play/pause, seek progress,
/// and latest `clientSeq` / `version`) in a single RTT!
pub async fn poll_room_sync(
    room_id: &str,
    song_id: &str,
    play_status: &str,
    progress_ms: i64,
    fetch_users: bool,
    force_resync: bool,
    cookie: &str,
) -> Result<ListenTogetherStatus, String> {
    let sid = netease_api::strip_ne_id(song_id);
    let sid_clean = if sid.chars().all(|c| c.is_ascii_digit()) && !sid.is_empty() {
        sid
    } else {
        "0".to_string()
    };

    let pl_fut = ne_eapi_post(
        "/api/listen/together/sync/playlist/get",
        json!({ "roomId": room_id }),
        cookie,
    );
    let hb_fut = ne_eapi_post(
        "/api/listen/together/heartbeat",
        json!({
            "roomId": room_id,
            "songId": sid_clean,
            "playStatus": play_status,
            "progress": progress_ms.max(0),
        }),
        cookie,
    );
    let st_fut = async {
        if fetch_users {
            Some(ne_eapi_post("/api/listen/together/status/get", json!({}), cookie).await)
        } else {
            None
        }
    };

    let (pl_res, hb_res, st_res_opt) = tokio::join!(pl_fut, hb_fut, st_fut);

    // Check heartbeat for `code == 488` (room ended by partner)
    if let Ok(ref hb_json) = hb_res {
        let hb_code = hb_json.get("code").and_then(Value::as_i64).unwrap_or(200);
        if hb_code == 488 {
            return Ok(ListenTogetherStatus {
                code: 488,
                in_room: false,
                room_closed: true,
                room_id: room_id.to_string(),
                message: "对方已结束一起听房间".to_string(),
                ..Default::default()
            });
        }
    }

    let now_ms = now_unix_ms();
    let mut merged = match pl_res {
        Ok(pl_json) => parse_sync_playlist_json(room_id, &pl_json, now_ms),
        Err(e) => {
            // Fallback to status/get if playlist/get failed transiently
            if let Some(Ok(st_json)) = st_res_opt {
                let mut s = parse_status_json(&st_json);
                s.force_resync = force_resync;
                return Ok(s);
            }
            return Err(e);
        }
    };

    if let Some(Ok(st_json)) = st_res_opt {
        let meta = parse_status_json(&st_json);
        if meta.room_closed || (meta.code == 200 && !meta.in_room) {
            merged.room_closed = true;
            merged.in_room = false;
        } else {
            if !meta.users.is_empty() {
                merged.users = meta.users;
            }
            if meta.creator_id != 0 {
                merged.creator_id = meta.creator_id;
            }
            if !meta.room_type.is_empty() {
                merged.room_type = meta.room_type;
            }
        }
    }

    merged.force_resync = force_resync;
    Ok(merged)
}

pub async fn send_play_command(
    room_id: &str,
    command_type: &str,
    target_song_id: &str,
    former_song_id: &str,
    play_status: &str,
    progress_ms: i64,
    client_seq: i64,
    cookie: &str,
) -> Result<Value, String> {
    let target_clean = netease_api::strip_ne_id(target_song_id);
    let former_clean = {
        let s = netease_api::strip_ne_id(former_song_id);
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
            "-1".to_string()
        } else {
            s
        }
    };
    let cmd_info = json!({
        "commandType": command_type,
        "progress": progress_ms.max(0),
        "playStatus": play_status,
        "formerSongId": former_clean,
        "targetSongId": target_clean,
        "clientSeq": client_seq,
    })
    .to_string();

    ne_eapi_post(
        "/api/listen/together/play/command/report",
        json!({
            "roomId": room_id,
            "commandInfo": cmd_info,
        }),
        cookie,
    )
    .await
}

pub async fn sync_list_command(
    room_id: &str,
    user_id: u64,
    version: i64,
    song_ids: &[String],
    cookie: &str,
) -> Result<Value, String> {
    let clean_ids: Vec<String> = song_ids
        .iter()
        .map(|s| netease_api::strip_ne_id(s))
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .take(120)
        .collect();
    if clean_ids.is_empty() {
        return Ok(json!({"code": 200}));
    }
    let param = json!({
        "commandType": "REPLACE",
        "version": [{
            "userId": user_id,
            "version": version,
        }],
        "anchorSongId": "",
        "anchorPosition": -1,
        "randomList": clean_ids,
        "displayList": clean_ids,
    })
    .to_string();

    ne_eapi_post(
        "/api/listen/together/sync/list/command/report",
        json!({
            "roomId": room_id,
            "playlistParam": param,
        }),
        cookie,
    )
    .await
}

pub async fn fetch_room_playlist(
    room_id: &str,
    cookie: &str,
) -> Result<Vec<AgentTrackInput>, String> {
    let json = ne_eapi_post(
        "/api/listen/together/sync/playlist/get",
        json!({ "roomId": room_id }),
        cookie,
    )
    .await?;
    let st = parse_sync_playlist_json(room_id, &json, now_unix_ms());
    if st.remote_playlist_ids.is_empty() {
        return Ok(Vec::new());
    }
    netease_api::fetch_songs_detail_batch(&st.remote_playlist_ids, Some(cookie)).await
}

pub async fn end_room(room_id: &str, cookie: &str) -> Result<bool, String> {
    let json = ne_eapi_post(
        "/api/listen/together/end/v2",
        json!({ "roomId": room_id }),
        cookie,
    )
    .await?;
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    let ok = code == 200 || code == 488;
    Ok(ok)
}

/// Fast inbox scan for recent NetEase Listen Together invite cards (uses lightweight direct API
/// with limit=12 and falls back to EAPI only if needed).
pub async fn fetch_inbox_invites(cookie: &str) -> Result<Vec<ListenTogetherInvite>, String> {
    let fast_url = "https://music.163.com/api/msg/private/users?offset=0&limit=12&total=false";
    let json = match ne_fast_get(fast_url, cookie).await {
        Ok(v) if v.get("msgs").is_some() => v,
        _ => {
            ne_eapi_post(
                "/api/msg/private/users",
                json!({ "offset": 0, "limit": 12, "total": "false" }),
                cookie,
            )
            .await?
        }
    };
    let mut invites = Vec::new();
    let Some(msgs) = json.get("msgs").and_then(Value::as_array) else {
        return Ok(invites);
    };
    let mut seen_rooms = std::collections::HashSet::new();

    for m in msgs {
        let from_user = m.get("fromUser").unwrap_or(&Value::Null);
        let from_uid = parse_u64_field(from_user, "userId");
        let from_name = from_user
            .get("nickname")
            .and_then(Value::as_str)
            .unwrap_or("网易云好友")
            .to_string();
        let from_avatar = from_user
            .get("avatarUrl")
            .and_then(Value::as_str)
            .and_then(upgrade_avatar_https);
        let time_ms = parse_u64_field(m, "time");

        let last_msg_raw = m.get("lastMsg").and_then(Value::as_str).unwrap_or_default();
        if last_msg_raw.is_empty() {
            continue;
        }
        let parsed_msg: Value = serde_json::from_str(last_msg_raw).unwrap_or(Value::Null);
        let msg_type = parsed_msg.get("type").and_then(Value::as_i64).unwrap_or(-1);
        let native_url = parsed_msg
            .pointer("/generalMsg/nativeUrl")
            .or_else(|| parsed_msg.get("nativeUrl"))
            .or_else(|| parsed_msg.pointer("/generalMsg/webUrl"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let text_body = parsed_msg
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let candidate_str = if !native_url.is_empty() {
            native_url
        } else if text_body.contains("roomId") || text_body.contains("listen-together") {
            text_body
        } else if msg_type == 23 {
            last_msg_raw
        } else {
            continue;
        };

        if let Some(parsed) = parse_invite_input(candidate_str) {
            if seen_rooms.insert(parsed.room_id.clone()) {
                invites.push(ListenTogetherInvite {
                    room_id: parsed.room_id,
                    inviter_id: parsed.inviter_id.unwrap_or(from_uid),
                    inviter_name: from_name,
                    inviter_avatar: from_avatar,
                    song_id: parsed.song_id,
                    time_ms,
                });
            }
        }
    }
    Ok(invites)
}

/// Fetches the signed-in user's mutual-follow friends (互关好友) for 1-click Listen Together invitations.
pub async fn fetch_mutual_friends(
    uid: u64,
    cookie: &str,
) -> Result<Vec<ListenTogetherFriend>, String> {
    if uid == 0 {
        return Ok(Vec::new());
    }
    let fast_url = format!("https://music.163.com/api/user/getfollows/{uid}?offset=0&limit=80&order=true");
    let json = match ne_fast_get(&fast_url, cookie).await {
        Ok(v) if v.get("follow").is_some() => v,
        _ => {
            ne_eapi_post(
                &format!("/api/user/getfollows/{uid}"),
                json!({ "offset": 0, "limit": 80, "order": true }),
                cookie,
            )
            .await?
        }
    };

    let mut mutuals = Vec::new();
    let mut others = Vec::new();
    if let Some(arr) = json.get("follow").and_then(Value::as_array) {
        for u in arr {
            let f_uid = parse_u64_field(u, "userId");
            if f_uid == 0 || f_uid == uid {
                continue;
            }
            let nickname = u
                .get("nickname")
                .and_then(Value::as_str)
                .unwrap_or("云村好友")
                .trim()
                .to_string();
            let avatar_url = u
                .get("avatarUrl")
                .and_then(Value::as_str)
                .and_then(upgrade_avatar_https);
            let mutual = u.get("mutual").and_then(Value::as_bool).unwrap_or(false);
            let (vip_type, is_vip) = parse_user_vip_flag(u);
            let friend = ListenTogetherFriend {
                user_id: f_uid,
                nickname,
                avatar_url,
                vip_type,
                is_vip,
                mutual,
            };
            if mutual {
                mutuals.push(friend);
            } else if others.len() < 12 {
                others.push(friend);
            }
        }
    }
    if !mutuals.is_empty() {
        Ok(mutuals)
    } else {
        Ok(others)
    }
}

/// Sends a Listen Together room invitation directly to a mutual friend via NetEase private message.
pub async fn send_room_invite_to_friend(
    friend_uid: u64,
    room_id: &str,
    inviter_id: u64,
    song_id: Option<&str>,
    cookie: &str,
) -> Result<(), String> {
    if friend_uid == 0 || room_id.trim().is_empty() {
        return Err("无效的好友 ID 或房间号".to_string());
    }
    let share_url = build_share_url(room_id, inviter_id, song_id);
    let msg_text = format!("我的耳机分你一半，和我一起听歌吧～ {share_url}");
    let json = ne_eapi_post(
        "/api/msg/private/send",
        json!({
            "type": "text",
            "msg": msg_text,
            "userIds": format!("[{friend_uid}]"),
        }),
        cookie,
    )
    .await?;
    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code == 200 {
        Ok(())
    } else {
        let msg = json
            .get("message")
            .or_else(|| json.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("发送私信邀请失败");
        Err(format!("{msg} (code {code})"))
    }
}

// ============================================================================
// Anti-Loop Bidirectional Sync Decision Engine (Pure & Unit-Testable!)
// ============================================================================

/// Evaluates whether an incoming remote room status requires a local playback action.
/// - `force_resync == true` (manual "立即校准进度" or initial join): immediately aligns song,
///   play/pause, and exact progress (`> 500ms`) bypassing local echo cooldown.
/// - New remote `clientSeq` from a partner (`remote_cmd_user_id != my_uid`): reacts IMMEDIATELY
///   to partner track switches, pause/play, or seeks (`> 900ms`) with zero lag!
/// - Steady-state polling: suppresses self-echoes within `1.8s` of local commands and gently
///   resyncs continuous clock drift only when `> 2200ms` to avoid stutter.
pub fn evaluate_remote_sync_action(
    local_song_id: Option<&str>,
    local_is_playing: bool,
    local_progress_ms: i64,
    remote: &ListenTogetherStatus,
    seconds_since_local_cmd: f32,
    last_applied_remote_seq: i64,
    my_uid: u64,
) -> RemoteSyncAction {
    if remote.room_closed || (remote.code == 200 && !remote.in_room) {
        return RemoteSyncAction::RoomClosed;
    }

    let is_fresh_partner_cmd = remote.remote_client_seq > last_applied_remote_seq
        && (remote.remote_cmd_user_id == 0 || my_uid == 0 || remote.remote_cmd_user_id != my_uid);

    // Suppress self-echoes shortly after local actions unless forced or partner explicitly issued a newer seq!
    if !remote.force_resync && !is_fresh_partner_cmd && seconds_since_local_cmd < 1.8 {
        return RemoteSyncAction::Noop;
    }

    let Some(ref remote_sid) = remote.remote_song_id else {
        return RemoteSyncAction::Noop;
    };
    let remote_clean = netease_api::strip_ne_id(remote_sid);
    if remote_clean.is_empty() {
        return RemoteSyncAction::Noop;
    }
    let local_clean = local_song_id
        .map(netease_api::strip_ne_id)
        .unwrap_or_default();
    let remote_playing = remote.remote_play_status != "PAUSE";

    // 1. Track changed remotely -> switch immediately!
    if local_clean != remote_clean {
        return RemoteSyncAction::SwitchTrack {
            song_id: remote_clean,
            play: remote_playing,
            progress_ms: remote.remote_progress_ms.max(0),
        };
    }

    // 2. Play/Pause state changed remotely -> mirror immediately!
    if !remote.remote_play_status.is_empty() && remote_playing != local_is_playing {
        return RemoteSyncAction::SetPlayPause {
            play: remote_playing,
            progress_ms: remote.remote_progress_ms.max(0),
        };
    }

    // 3. Same song -> check progress drift (tight threshold on manual resync or explicit partner seek!)
    let drift = (remote.remote_progress_ms - local_progress_ms).abs();
    let seek_threshold = if remote.force_resync {
        500
    } else if is_fresh_partner_cmd {
        900
    } else {
        2200
    };
    if (remote.force_resync || is_fresh_partner_cmd || (remote_playing && local_is_playing))
        && drift > seek_threshold
    {
        return RemoteSyncAction::SeekTo {
            progress_ms: remote.remote_progress_ms.max(0),
        };
    }

    RemoteSyncAction::Noop
}

// ============================================================================
// Adaptive Idle/Paused Polling Backoff Scheduler (P1-6)
// ============================================================================

/// Active low-latency sync cadence (750 ms) whenever playback is running or state changed.
pub const POLL_INTERVAL_ACTIVE_MS: u64 = 750;
/// Intermediate idle/paused backoff cadence (2,000 ms).
pub const POLL_INTERVAL_IDLE_STEP1_MS: u64 = 2000;
/// Steady-state idle/paused backoff cadence (3,000 ms — ~75% fewer EAPI POSTs while paused!).
pub const POLL_INTERVAL_IDLE_MAX_MS: u64 = 3000;

/// Pure backoff policy: preserves `750 ms` whenever either side is playing or state changed,
/// backing off smoothly to `2000 ms` -> `3000 ms` when paused/idle and unchanged.
#[inline]
pub fn compute_next_poll_interval_ms(
    local_is_playing: bool,
    remote_is_playing: bool,
    state_changed: bool,
    unchanged_idle_ticks: u32,
) -> u64 {
    if local_is_playing || remote_is_playing || state_changed {
        return POLL_INTERVAL_ACTIVE_MS;
    }
    match unchanged_idle_ticks {
        0 => POLL_INTERVAL_ACTIVE_MS,
        1..=2 => POLL_INTERVAL_IDLE_STEP1_MS,
        _ => POLL_INTERVAL_IDLE_MAX_MS,
    }
}

/// Stateful adaptive polling scheduler for NetEase Listen Together rooms.
#[derive(Clone, Debug, Default)]
pub struct AdaptivePollState {
    unchanged_idle_ticks: u32,
    initialized: bool,
    last_song_id: Option<String>,
    last_play_status: String,
    last_client_seq: i64,
    last_list_version: i64,
    last_user_count: usize,
}

impl AdaptivePollState {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn unchanged_idle_ticks(&self) -> u32 {
        self.unchanged_idle_ticks
    }

    /// Computes the next sleep delay in milliseconds based on local playback state and the
    /// latest polled room status, immediately snapping back to `750 ms` on any play/change!
    pub fn next_delay_ms(
        &mut self,
        local_is_playing: bool,
        polled: Option<&ListenTogetherStatus>,
    ) -> u64 {
        let Some(st) = polled else {
            // Network/transient error: back off gently when idle, stay responsive when playing.
            if local_is_playing {
                self.unchanged_idle_ticks = 0;
                return POLL_INTERVAL_ACTIVE_MS;
            }
            self.unchanged_idle_ticks = self.unchanged_idle_ticks.saturating_add(1);
            return compute_next_poll_interval_ms(false, false, false, self.unchanged_idle_ticks);
        };

        let remote_is_playing = st.remote_play_status.eq_ignore_ascii_case("PLAY");
        let user_count = if st.users.is_empty() {
            self.last_user_count
        } else {
            st.users.len()
        };
        let state_changed = !self.initialized
            || st.remote_song_id != self.last_song_id
            || st.remote_play_status != self.last_play_status
            || st.remote_client_seq != self.last_client_seq
            || st.remote_list_version != self.last_list_version
            || user_count != self.last_user_count;

        self.initialized = true;
        self.last_song_id = st.remote_song_id.clone();
        self.last_play_status = st.remote_play_status.clone();
        self.last_client_seq = st.remote_client_seq;
        self.last_list_version = st.remote_list_version;
        self.last_user_count = user_count;

        if local_is_playing || remote_is_playing || state_changed {
            self.unchanged_idle_ticks = 0;
            POLL_INTERVAL_ACTIVE_MS
        } else {
            self.unchanged_idle_ticks = self.unchanged_idle_ticks.saturating_add(1);
            compute_next_poll_interval_ms(false, false, false, self.unchanged_idle_ticks)
        }
    }

    /// Determines whether the current poll tick should also query `status/get` for room users:
    /// every 3rd tick (~2.25s) while active, every 2nd tick (~4-6s) when backed off in idle.
    #[inline]
    pub fn should_fetch_users(&self, tick: u64, local_is_playing: bool) -> bool {
        if local_is_playing || self.unchanged_idle_ticks == 0 {
            tick % 3 == 0
        } else {
            tick % 2 == 0
        }
    }
}
