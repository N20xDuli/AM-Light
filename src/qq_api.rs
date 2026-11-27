use crate::am_agent::AgentTrackInput;
use crate::am_api::{EditorialItem, EditorialShelf, EditorialShelfKind};
use crate::backends::{QQQuality, ResolvedAudioStream};
use crate::lyrics::{self, ParsedLyrics};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const QQ_REFERER: &str = "https://y.qq.com/";
const QQ_MUSICU_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";

static QQ_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(16)
        .tcp_nodelay(true)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
        .expect("qq music http client")
});

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QQAccountInfo {
    pub logged_in: bool,
    pub uin: String,
    pub nickname: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct QQCard {
    pub id: String,
    pub kind: String, // "qq-playlist", "qq-toplist", "qq-album", "qq-artist", "qq-radio"
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub badge: Option<String>,
}

#[derive(Clone, Debug)]
pub struct QQDetail {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub tracks: Vec<AgentTrackInput>,
    pub related_albums: Vec<QQCard>,
}

#[derive(Clone, Debug)]
pub struct QQHomeBundle {
    pub shelves: Vec<EditorialShelf>,
    pub browse_shelves: Vec<EditorialShelf>,
    pub playlists: Vec<QQCard>,
    pub albums: Vec<QQCard>,
    pub songs: Vec<AgentTrackInput>,
}

pub fn strip_qq_id(raw: &str) -> String {
    raw.trim()
        .strip_prefix("qq:")
        .or_else(|| raw.trim().strip_prefix("qq-song:"))
        .unwrap_or(raw.trim())
        .to_string()
}

pub fn parse_qq_cookie_fields(raw_cookie: &str) -> (String, String) {
    let mut uin = String::new();
    let mut keyst = String::new();
    for seg in raw_cookie.split(';') {
        let s = seg.trim();
        if let Some((k, v)) = s.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().trim_start_matches('o').trim_start_matches('0');
            match key.as_str() {
                "uin" | "p_uin" | "wxuin" => {
                    if uin.is_empty() && !val.is_empty() && val.chars().all(|c| c.is_ascii_digit()) {
                        uin = val.to_string();
                    }
                }
                "qm_keyst" | "qqmusic_key" => {
                    if !v.trim().is_empty() {
                        keyst = v.trim().to_string();
                    }
                }
                _ => {}
            }
        }
    }
    (uin, keyst)
}

fn qq_album_cover(album_mid: &str) -> Option<String> {
    let m = album_mid.trim();
    if m.is_empty() || m == "000MkMni19ClKG" {
        None
    } else {
        Some(format!(
            "https://y.gtimg.cn/music/photo_new/T002R500x500M000{m}.jpg"
        ))
    }
}

fn upgrade_https(url: &str) -> String {
    let u = url.trim();
    if let Some(rest) = u.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        u.to_string()
    }
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn parse_qq_song_modern(v: &Value, fallback_art: Option<&str>) -> Option<AgentTrackInput> {
    let mid = v
        .get("mid")
        .or_else(|| v.get("songmid"))
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if mid.is_empty() {
        return None;
    }
    let name = v
        .get("name")
        .or_else(|| v.get("title"))
        .or_else(|| v.get("songname"))
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }

    let artist_name = v
        .get("singer")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.get("name").or_else(|| s.get("title")).and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();

    let album_name = v
        .pointer("/album/name")
        .or_else(|| v.pointer("/album/title"))
        .or_else(|| v.get("albumname"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let album_mid = v
        .pointer("/album/mid")
        .or_else(|| v.get("albummid"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let artwork_url = qq_album_cover(album_mid).or_else(|| fallback_art.map(|s| s.to_string()));

    let media_mid = v
        .pointer("/file/media_mid")
        .or_else(|| v.get("strMediaMid"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&mid)
        .to_string();

    let duration_sec = v
        .get("interval")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);

    let mut audio_traits = Vec::new();
    if v.pointer("/file/size_hires").and_then(Value::as_u64).unwrap_or(0) > 0 {
        audio_traits.push("hi-res-lossless".to_string());
    } else if v.pointer("/file/size_flac").and_then(Value::as_u64).unwrap_or(0) > 0
        || v.get("sizeflac").and_then(Value::as_u64).unwrap_or(0) > 0
    {
        audio_traits.push("lossless".to_string());
    }
    if v.pointer("/pay/pay_play").and_then(Value::as_i64).unwrap_or(0) == 1 {
        audio_traits.push("vip".to_string());
    }

    Some(AgentTrackInput {
        id: format!("qq:{mid}"),
        catalog_id: Some(media_mid),
        kind: Some("qq-song".to_string()),
        name,
        artist_name,
        album_name,
        duration_sec,
        artwork_url,
        audio_traits,
    })
}

async fn qq_musicu(mut body: Value, cookie: Option<&str>) -> Result<Value, String> {
    let (uin, keyst) = cookie
        .map(parse_qq_cookie_fields)
        .unwrap_or_else(|| (String::new(), String::new()));

    let mut comm = json!({
        "ct": 11,
        "cv": 1003006,
        "v": 1003006,
        "os_ver": "12",
        "phonetype": "0",
        "devicelevel": "31",
        "tmeAppID": "qqmusiclight"
    });
    if !uin.is_empty() {
        comm["uin"] = Value::String(uin);
    }
    if !keyst.is_empty() {
        comm["authst"] = Value::String(keyst);
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert("comm".to_string(), comm);
    }

    let mut req = QQ_HTTP
        .post(QQ_MUSICU_URL)
        .header("Referer", QQ_REFERER)
        .header("Origin", "https://y.qq.com")
        .header("Content-Type", "application/json")
        .json(&body);
    if let Some(ck) = cookie.map(str::trim).filter(|s| !s.is_empty()) {
        req = req.header("Cookie", ck);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("QQ 音乐接口请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("QQ 音乐接口 HTTP {}", resp.status().as_u16()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("QQ 音乐响应解析失败: {e}"))
}

// ============================================================================
// Account Verification & Personal Playlists
// ============================================================================

pub async fn verify_account(raw_cookie: &str) -> Result<QQAccountInfo, String> {
    let trimmed = raw_cookie.trim();
    if trimmed.is_empty() {
        return Ok(QQAccountInfo {
            logged_in: false,
            message: "未配置 QQ 音乐 Cookie（游客模式可浏览全站巅峰榜/歌单并播放免费曲目）".to_string(),
            ..Default::default()
        });
    }
    let (uin, keyst) = parse_qq_cookie_fields(trimmed);
    if uin.is_empty() {
        return Ok(QQAccountInfo {
            logged_in: false,
            message: "未从 Cookie 中解析到 uin 字段，请从 y.qq.com 复制完整 Cookie".to_string(),
            ..Default::default()
        });
    }
    let url = format!(
        "https://c.y.qq.com/rsc/fcgi-bin/fcg_get_profile_homepage.fcg?cid=205360838&userid={uin}&reqfrom=1&format=json"
    );
    let resp = QQ_HTTP
        .get(&url)
        .header("Referer", QQ_REFERER)
        .header("Cookie", trimmed)
        .send()
        .await
        .map_err(|e| format!("验证 QQ 音乐账号失败: {e}"))?;
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 QQ 主页信息失败: {e}"))?;

    let nick = json
        .pointer("/data/creator/nick")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&uin)
        .to_string();
    let has_key = !keyst.is_empty();
    let status = if has_key {
        format!("已连接：{nick} (QQ {uin} · 已携带鉴权 Key)")
    } else {
        format!("已识别账号：{nick} (QQ {uin} · 未检测到 qm_keyst，仅限公开歌单)")
    };
    Ok(QQAccountInfo {
        logged_in: has_key || json.pointer("/data/creator/nick").is_some(),
        uin,
        nickname: nick,
        message: status,
    })
}

pub async fn fetch_user_playlists(raw_cookie: &str) -> Result<Vec<QQCard>, String> {
    let (uin, _) = parse_qq_cookie_fields(raw_cookie);
    if uin.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!(
        "https://c.y.qq.com/rsc/fcgi-bin/fcg_get_profile_homepage.fcg?cid=205360838&userid={uin}&reqfrom=1&format=json"
    );
    let resp = QQ_HTTP
        .get(&url)
        .header("Referer", QQ_REFERER)
        .header("Cookie", raw_cookie.trim())
        .send()
        .await
        .map_err(|e| format!("获取 QQ 个人歌单失败: {e}"))?;
    let json: Value = resp.json().await.map_err(|e| format!("解析歌单失败: {e}"))?;

    let mut out = Vec::new();
    // Liked songs playlist (`mymusic[0]`)
    if let Some(fav) = json.pointer("/data/mymusic/0") {
        if let Some(id) = fav.get("id").and_then(Value::as_u64).filter(|&x| x > 0) {
            let num = fav.get("num0").and_then(Value::as_u64).unwrap_or(0);
            let pic = fav
                .get("laypic")
                .and_then(Value::as_str)
                .map(upgrade_https)
                .filter(|s| !s.is_empty());
            out.push(QQCard {
                id: id.to_string(),
                kind: "qq-playlist".to_string(),
                title: "我喜欢 (QQ音乐)".to_string(),
                subtitle: format!("{num} 首喜爱歌曲"),
                description: String::new(),
                artwork_url: pic,
                badge: Some("我喜欢".to_string()),
            });
        }
    }
    if let Some(arr) = json.pointer("/data/mydiss/list").and_then(Value::as_array) {
        for item in arr {
            let id = item
                .get("dissid")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
                .or_else(|| item.get("dissid").and_then(Value::as_str).map(|s| s.to_string()));
            let Some(id) = id.filter(|s| !s.is_empty() && s != "0") else { continue };
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("QQ 歌单")
                .to_string();
            let sub = item
                .get("subtitle")
                .and_then(Value::as_str)
                .unwrap_or("自建歌单")
                .to_string();
            let pic = item
                .get("picurl")
                .and_then(Value::as_str)
                .map(upgrade_https)
                .filter(|s| !s.is_empty());
            out.push(QQCard {
                id,
                kind: "qq-playlist".to_string(),
                title,
                subtitle: sub,
                description: String::new(),
                artwork_url: pic,
                badge: Some("QQ 歌单".to_string()),
            });
        }
    }
    Ok(out)
}

// ============================================================================
// Home / Discover / Radio / Search / Details
// ============================================================================

pub async fn fetch_home_and_browse(cookie: Option<&str>) -> Result<QQHomeBundle, String> {
    let payload = json!({
        "rec_square": {
            "module": "music.playlist.PlaylistSquare",
            "method": "GetRecommendFeed",
            "param": {"From": 0, "Size": 12}
        },
        "hot_pl": {
            "module": "playlist.HotRecommendServer",
            "method": "get_new_hot_recommend",
            "param": {"cmd": 0, "page": 0, "size": 12}
        },
        "toplist_all": {
            "module": "musicToplist.ToplistInfoServer",
            "method": "GetAll",
            "param": {}
        },
        "hot_songs": {
            "module": "musicToplist.ToplistInfoServer",
            "method": "GetDetail",
            "param": {"topId": 26, "offset": 0, "num": 16}
        },
        "new_songs": {
            "module": "musicToplist.ToplistInfoServer",
            "method": "GetDetail",
            "param": {"topId": 27, "offset": 0, "num": 12}
        },
        "new_albums": {
            "module": "newalbum.NewAlbumServer",
            "method": "get_new_album_info",
            "param": {"area": 1, "sin": 0, "num": 12}
        }
    });

    let json = qq_musicu(payload, cookie).await?;
    let mut home_shelves = Vec::new();
    let mut browse_shelves = Vec::new();
    let mut playlists = Vec::new();
    let mut albums = Vec::new();
    let mut songs = Vec::new();

    // 1. Recommended Playlists (HomeTopPicks)
    let mut top_items = Vec::new();
    if let Some(arr) = json.pointer("/rec_square/data/List").and_then(Value::as_array) {
        for entry in arr.iter().take(12) {
            let Some(basic) = entry.pointer("/Playlist/basic") else { continue };
            let tid = basic
                .get("tid")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
                .or_else(|| basic.get("tid").and_then(Value::as_str).map(|s| s.to_string()));
            let Some(id) = tid.filter(|s| !s.is_empty()) else { continue };
            let title = basic
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let nick = basic
                .pointer("/creator/nick")
                .and_then(Value::as_str)
                .unwrap_or("QQ 音乐达人");
            let art = upgrade_https(
                basic
                    .pointer("/cover/default_url")
                    .or_else(|| basic.pointer("/cover/medium_url"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            top_items.push(EditorialItem {
                id: id.clone(),
                kind: "qq-playlist".to_string(),
                play_kind: "qq-playlist".to_string(),
                title: title.clone(),
                subtitle: format!("by {nick}"),
                badge: "精选歌单".to_string(),
                artwork_url: art.clone(),
                motion_video_url: None,
                motion_tall_video_url: None,
                storefront: "qq".to_string(),
            });
            playlists.push(QQCard {
                id,
                kind: "qq-playlist".to_string(),
                title,
                subtitle: format!("by {nick}"),
                description: String::new(),
                artwork_url: if art.is_empty() { None } else { Some(art) },
                badge: Some("歌单".to_string()),
            });
        }
    }
    if !top_items.is_empty() {
        home_shelves.push(EditorialShelf {
            title: "QQ 音乐 · 为你推荐歌单".to_string(),
            subtitle: "广场高赞精选歌单".to_string(),
            kind: EditorialShelfKind::HomeTopPicks,
            items: top_items,
        });
    }

    // 2. QQ Toplists (`巅峰榜`)
    if let Some(groups) = json.pointer("/toplist_all/data/group").and_then(Value::as_array) {
        let mut chart_items = Vec::new();
        for g in groups {
            if let Some(list) = g.get("toplist").and_then(Value::as_array) {
                for item in list {
                    let Some(tid) = item.get("topId").and_then(Value::as_i64) else { continue };
                    let title = item
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let sub = item
                        .get("updateTips")
                        .or_else(|| item.get("period"))
                        .and_then(Value::as_str)
                        .unwrap_or("巅峰榜")
                        .to_string();
                    let art = upgrade_https(
                        item.get("frontPicUrl")
                            .or_else(|| item.get("headPicUrl"))
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                    chart_items.push(EditorialItem {
                        id: tid.to_string(),
                        kind: "qq-toplist".to_string(),
                        play_kind: "qq-toplist".to_string(),
                        title,
                        subtitle: sub,
                        badge: "巅峰榜".to_string(),
                        artwork_url: art,
                        motion_video_url: None,
                        motion_tall_video_url: None,
                        storefront: "qq".to_string(),
                    });
                    if chart_items.len() >= 14 {
                        break;
                    }
                }
            }
            if chart_items.len() >= 14 {
                break;
            }
        }
        if !chart_items.is_empty() {
            home_shelves.push(EditorialShelf {
                title: "QQ 音乐巅峰榜".to_string(),
                subtitle: "热歌榜 · 新歌榜 · 飙升榜 · 流行指数榜".to_string(),
                kind: EditorialShelfKind::SquareShelf,
                items: chart_items,
            });
        }
    }

    // 3. Hot Songs TrackGrid (热歌榜 topId=26)
    if let Some(arr) = json.pointer("/hot_songs/data/songInfoList").and_then(Value::as_array) {
        let mut tr_items = Vec::new();
        for v in arr.iter().take(16) {
            if let Some(tr) = parse_qq_song_modern(v, None) {
                if tr_items.len() < 12 {
                    tr_items.push(EditorialItem {
                        id: tr.id.clone(),
                        kind: "qq-song".to_string(),
                        play_kind: "qq-song".to_string(),
                        title: tr.name.clone(),
                        subtitle: tr.artist_name.clone(),
                        badge: "热歌".to_string(),
                        artwork_url: tr.artwork_url.clone().unwrap_or_default(),
                        motion_video_url: None,
                        motion_tall_video_url: None,
                        storefront: "qq".to_string(),
                    });
                }
                songs.push(tr);
            }
        }
        if !tr_items.is_empty() {
            home_shelves.push(EditorialShelf {
                title: "热歌榜精选单曲".to_string(),
                subtitle: "点击直接解码播放".to_string(),
                kind: EditorialShelfKind::TrackGrid,
                items: tr_items,
            });
        }
    }

    // 4. New Albums (Home + Browse Hero)
    if let Some(arr) = json.pointer("/new_albums/data/albums").and_then(Value::as_array) {
        let mut alb_items = Vec::new();
        for item in arr.iter().take(12) {
            let Some(mid) = item.get("mid").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                continue;
            };
            let title = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let artist = item
                .get("singers")
                .and_then(Value::as_array)
                .map(|s| {
                    s.iter()
                        .filter_map(|x| x.get("name").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            let art = qq_album_cover(mid).unwrap_or_default();
            alb_items.push(EditorialItem {
                id: mid.to_string(),
                kind: "qq-album".to_string(),
                play_kind: "qq-album".to_string(),
                title: title.clone(),
                subtitle: artist.clone(),
                badge: "新碟".to_string(),
                artwork_url: art.clone(),
                motion_video_url: None,
                motion_tall_video_url: None,
                storefront: "qq".to_string(),
            });
            albums.push(QQCard {
                id: mid.to_string(),
                kind: "qq-album".to_string(),
                title,
                subtitle: artist,
                description: String::new(),
                artwork_url: if art.is_empty() { None } else { Some(art) },
                badge: Some("新碟".to_string()),
            });
        }
        if !alb_items.is_empty() {
            browse_shelves.push(EditorialShelf {
                title: "新碟首发".to_string(),
                subtitle: "QQ 音乐最新发行专辑".to_string(),
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

    // 5. Browse Shelf 2: Hot Recommend Playlists (`hot_pl.data.grids`)
    if let Some(arr) = json.pointer("/hot_pl/data/grids").and_then(Value::as_array) {
        let mut pl_items = Vec::new();
        for item in arr.iter().take(12) {
            let id = item
                .get("id")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
                .or_else(|| item.get("id").and_then(Value::as_str).map(|s| s.to_string()));
            let Some(id) = id.filter(|s| !s.is_empty()) else { continue };
            let title = item.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
            let sub = item
                .pointer("/creator/nick")
                .or_else(|| item.get("rcmdcontent"))
                .and_then(Value::as_str)
                .unwrap_or("热门推荐")
                .to_string();
            let art = upgrade_https(
                item.get("cover")
                    .or_else(|| item.get("picurl"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            pl_items.push(EditorialItem {
                id,
                kind: "qq-playlist".to_string(),
                play_kind: "qq-playlist".to_string(),
                title,
                subtitle: sub,
                badge: "广场".to_string(),
                artwork_url: art,
                motion_video_url: None,
                motion_tall_video_url: None,
                storefront: "qq".to_string(),
            });
        }
        if !pl_items.is_empty() {
            browse_shelves.push(EditorialShelf {
                title: "热门分类歌单".to_string(),
                subtitle: "官方编辑与达人精选".to_string(),
                kind: EditorialShelfKind::SquareShelf,
                items: pl_items,
            });
        }
    }

    // 6. Browse Shelf 3: New Songs TrackGrid (topId=27)
    if let Some(arr) = json.pointer("/new_songs/data/songInfoList").and_then(Value::as_array) {
        let mut tr_items = Vec::new();
        for v in arr.iter().take(12) {
            if let Some(tr) = parse_qq_song_modern(v, None) {
                tr_items.push(EditorialItem {
                    id: tr.id.clone(),
                    kind: "qq-song".to_string(),
                    play_kind: "qq-song".to_string(),
                    title: tr.name,
                    subtitle: tr.artist_name,
                    badge: "新歌榜".to_string(),
                    artwork_url: tr.artwork_url.unwrap_or_default(),
                    motion_video_url: None,
                    motion_tall_video_url: None,
                    storefront: "qq".to_string(),
                });
            }
        }
        if !tr_items.is_empty() {
            browse_shelves.push(EditorialShelf {
                title: "巅峰新歌速递".to_string(),
                subtitle: "QQ 音乐新歌榜实时排行".to_string(),
                kind: EditorialShelfKind::TrackGrid,
                items: tr_items,
            });
        }
    }

    Ok(QQHomeBundle {
        shelves: home_shelves,
        browse_shelves,
        playlists,
        albums,
        songs,
    })
}

pub async fn fetch_radio_shelves(cookie: Option<&str>) -> Result<(Vec<EditorialShelf>, Vec<QQCard>), String> {
    let payload = json!({
        "radio": {
            "module": "pf.radiosvr",
            "method": "GetRadiolist",
            "param": {"ct": "24"}
        }
    });
    let json = qq_musicu(payload, cookie).await?;
    let mut shelves = Vec::new();
    let mut cards = Vec::new();

    if let Some(groups) = json.pointer("/radio/data/radio_list").and_then(Value::as_array) {
        for (gi, g) in groups.iter().take(4).enumerate() {
            let gtitle = g
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("QQ 音乐电台")
                .to_string();
            let mut items = Vec::new();
            if let Some(list) = g.get("list").and_then(Value::as_array) {
                for r in list.iter().take(12) {
                    let Some(rid) = r.get("id").and_then(Value::as_i64) else { continue };
                    let title = r.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
                    let sub = r
                        .get("listenDesc")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("QQ 音乐精选电台")
                        .to_string();
                    let art = upgrade_https(
                        r.get("pic_url").and_then(Value::as_str).unwrap_or_default(),
                    );
                    items.push(EditorialItem {
                        id: rid.to_string(),
                        kind: "qq-radio".to_string(),
                        play_kind: "qq-radio".to_string(),
                        title: title.clone(),
                        subtitle: sub.clone(),
                        badge: "电台".to_string(),
                        artwork_url: art.clone(),
                        motion_video_url: None,
                        motion_tall_video_url: None,
                        storefront: "qq".to_string(),
                    });
                    if cards.len() < 18 {
                        cards.push(QQCard {
                            id: rid.to_string(),
                            kind: "qq-radio".to_string(),
                            title,
                            subtitle: sub,
                            description: String::new(),
                            artwork_url: if art.is_empty() { None } else { Some(art) },
                            badge: Some("电台".to_string()),
                        });
                    }
                }
            }
            if !items.is_empty() {
                shelves.push(EditorialShelf {
                    title: format!("QQ 电台 · {gtitle}"),
                    subtitle: "点击立即加载电台曲目流并播放".to_string(),
                    kind: if gi == 0 {
                        EditorialShelfKind::RadioStationRow
                    } else {
                        EditorialShelfKind::SquareShelf
                    },
                    items,
                });
            }
        }
    }

    Ok((shelves, cards))
}

pub async fn fetch_radio_tracks(label_id: &str, cookie: Option<&str>) -> Result<Vec<AgentTrackInput>, String> {
    let lid = if label_id.trim() == "99" {
        "199"
    } else {
        label_id.trim()
    };
    let url = format!("https://c.y.qq.com/v8/fcg-bin/fcg_v8_radiosonglist.fcg?labelid={lid}&format=json");
    let mut req = QQ_HTTP.get(&url).header("Referer", QQ_REFERER);
    if let Some(ck) = cookie.map(str::trim).filter(|s| !s.is_empty()) {
        req = req.header("Cookie", ck);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("拉取 QQ 电台曲目失败: {e}"))?;
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 QQ 电台曲目失败: {e}"))?;
    let mut tracks = Vec::new();
    if let Some(arr) = json.get("data").and_then(Value::as_array) {
        for v in arr {
            if let Some(tr) = parse_qq_song_modern(v, None) {
                tracks.push(tr);
            }
        }
    }
    if tracks.is_empty() {
        // Fallback to QQ Hot TopList 26 if a niche radio label returns empty
        let det = fetch_container_detail("qq-toplist", "26", "热歌电台", cookie).await?;
        return Ok(det.tracks);
    }
    Ok(tracks)
}

pub async fn search_catalog(
    query: &str,
    cookie: Option<&str>,
) -> Result<(Vec<QQCard>, Vec<QQCard>, Vec<QQCard>, Vec<AgentTrackInput>), String> {
    let q = query.trim();
    let payload = json!({
        "s_song": {
            "module": "music.search.SearchCgiService",
            "method": "DoSearchForQQMusicDesktop",
            "param": {"query": q, "search_type": 0, "num_per_page": 15, "page_num": 1, "grp": 1}
        },
        "s_art": {
            "module": "music.search.SearchCgiService",
            "method": "DoSearchForQQMusicDesktop",
            "param": {"query": q, "search_type": 1, "num_per_page": 8, "page_num": 1}
        },
        "s_alb": {
            "module": "music.search.SearchCgiService",
            "method": "DoSearchForQQMusicDesktop",
            "param": {"query": q, "search_type": 2, "num_per_page": 10, "page_num": 1, "grp": 1}
        },
        "s_pl": {
            "module": "music.search.SearchCgiService",
            "method": "DoSearchForQQMusicDesktop",
            "param": {"query": q, "search_type": 3, "num_per_page": 10, "page_num": 1}
        }
    });

    let json = qq_musicu(payload, cookie).await?;
    let songs = json
        .pointer("/s_song/data/body/song/list")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| parse_qq_song_modern(v, None))
                .collect()
        })
        .unwrap_or_default();

    let mut artists = Vec::new();
    if let Some(arr) = json.pointer("/s_art/data/body/singer/list").and_then(Value::as_array) {
        for v in arr {
            let Some(mid) = v
                .get("singerMID")
                .or_else(|| v.get("singerMid"))
                .or_else(|| v.get("singer_mid"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let title = v
                .get("singerName")
                .or_else(|| v.get("singer_name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let art = v
                .get("singerPic")
                .or_else(|| v.get("singer_pic"))
                .and_then(Value::as_str)
                .map(upgrade_https)
                .filter(|s| !s.is_empty())
                .or_else(|| Some(format!("https://y.gtimg.cn/music/photo_new/T001R300x300M000{mid}.jpg")));
            artists.push(QQCard {
                id: mid.to_string(),
                kind: "qq-artist".to_string(),
                title,
                subtitle: "QQ 音乐艺人".to_string(),
                description: String::new(),
                artwork_url: art,
                badge: Some("艺人".to_string()),
            });
        }
    }

    let mut albums = Vec::new();
    if let Some(arr) = json.pointer("/s_alb/data/body/album/list").and_then(Value::as_array) {
        for v in arr {
            let Some(mid) = v.get("albumMID").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                continue;
            };
            let title = v.get("albumName").and_then(Value::as_str).unwrap_or_default().to_string();
            let artist = v
                .get("singerName")
                .and_then(Value::as_str)
                .unwrap_or("QQ 音乐专辑")
                .to_string();
            let art = v
                .get("albumPic")
                .and_then(Value::as_str)
                .map(upgrade_https)
                .filter(|s| !s.is_empty())
                .or_else(|| qq_album_cover(mid));
            albums.push(QQCard {
                id: mid.to_string(),
                kind: "qq-album".to_string(),
                title,
                subtitle: artist,
                description: String::new(),
                artwork_url: art,
                badge: Some("专辑".to_string()),
            });
        }
    }

    let mut playlists = Vec::new();
    if let Some(arr) = json.pointer("/s_pl/data/body/songlist/list").and_then(Value::as_array) {
        for v in arr {
            let id = v
                .get("dissid")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .or_else(|| v.get("dissid").and_then(Value::as_u64).map(|n| n.to_string()));
            let Some(id) = id.filter(|s| !s.is_empty()) else { continue };
            let title = decode_html_entities(
                v.get("dissname").and_then(Value::as_str).unwrap_or_default(),
            );
            let creator = v
                .pointer("/creator/name")
                .and_then(Value::as_str)
                .unwrap_or("QQ 歌单")
                .to_string();
            let art = v
                .get("imgurl")
                .and_then(Value::as_str)
                .map(upgrade_https)
                .filter(|s| !s.is_empty());
            playlists.push(QQCard {
                id,
                kind: "qq-playlist".to_string(),
                title,
                subtitle: creator,
                description: String::new(),
                artwork_url: art,
                badge: Some("歌单".to_string()),
            });
        }
    }

    Ok((artists, albums, playlists, songs))
}

pub async fn fetch_container_detail(
    kind: &str,
    id: &str,
    title_hint: &str,
    cookie: Option<&str>,
) -> Result<QQDetail, String> {
    if kind == "qq-toplist" {
        let top_id = id.parse::<i64>().unwrap_or(26);
        let payload = json!({
            "top": {
                "module": "musicToplist.ToplistInfoServer",
                "method": "GetDetail",
                "param": {"topId": top_id, "offset": 0, "num": 60}
            }
        });
        let json = qq_musicu(payload, cookie).await?;
        let meta = json.pointer("/top/data/data").unwrap_or(&Value::Null);
        let title = meta
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(title_hint)
            .to_string();
        let subtitle = meta
            .get("titleDetail")
            .and_then(Value::as_str)
            .unwrap_or("QQ 音乐巅峰榜")
            .to_string();
        let description = decode_html_entities(
            meta.get("intro").and_then(Value::as_str).unwrap_or_default(),
        );
        let artwork_url = meta
            .get("frontPicUrl")
            .or_else(|| meta.get("headPicUrl"))
            .and_then(Value::as_str)
            .map(upgrade_https)
            .filter(|s| !s.is_empty());
        let tracks = json
            .pointer("/top/data/songInfoList")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| parse_qq_song_modern(v, artwork_url.as_deref()))
                    .collect()
            })
            .unwrap_or_default();
        return Ok(QQDetail {
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

    if kind == "qq-album" {
        let payload = json!({
            "info": {
                "module": "music.musichallAlbum.AlbumInfoServer",
                "method": "GetAlbumDetail",
                "param": {"albumMid": id}
            },
            "songs": {
                "module": "music.musichallAlbum.AlbumSongList",
                "method": "GetAlbumSongList",
                "param": {"albumMid": id, "begin": 0, "num": 80}
            }
        });
        let json = qq_musicu(payload, cookie).await?;
        let basic = json.pointer("/info/data/basicInfo").unwrap_or(&Value::Null);
        let title = basic
            .get("albumName")
            .and_then(Value::as_str)
            .unwrap_or(title_hint)
            .to_string();
        let singer = json
            .pointer("/info/data/singer/singerList")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.get("name").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "QQ 音乐专辑".to_string());
        let description = basic
            .get("desc")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let artwork_url = qq_album_cover(id);
        let tracks: Vec<AgentTrackInput> = json
            .pointer("/songs/data/songList")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let s = v.get("songInfo").unwrap_or(v);
                        parse_qq_song_modern(s, artwork_url.as_deref())
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(QQDetail {
            id: id.to_string(),
            kind: kind.to_string(),
            title,
            subtitle: singer,
            description,
            artwork_url,
            tracks,
            related_albums: Vec::new(),
        });
    }

    if kind == "qq-artist" {
        let payload = json!({
            "songs": {
                "module": "musichall.song_list_server",
                "method": "GetSingerSongList",
                "param": {"singerMid": id, "begin": 0, "num": 50, "order": 1}
            },
            "albums": {
                "module": "music.musichallAlbum.AlbumListServer",
                "method": "GetAlbumList",
                "param": {"singerMid": id, "begin": 0, "num": 24, "order": 0}
            }
        });
        let json = qq_musicu(payload, cookie).await?;
        let artwork_url = Some(format!(
            "https://y.gtimg.cn/music/photo_new/T001R500x500M000{id}.jpg"
        ));
        let tracks: Vec<AgentTrackInput> = json
            .pointer("/songs/data/songList")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let s = v.get("songInfo").unwrap_or(v);
                        parse_qq_song_modern(s, artwork_url.as_deref())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut related_albums = Vec::new();
        if let Some(arr) = json.pointer("/albums/data/albumList").and_then(Value::as_array) {
            for a in arr.iter().take(24) {
                let amid = a
                    .get("albumMid")
                    .or_else(|| a.get("album_mid"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if amid.is_empty() {
                    continue;
                }
                let aname = a
                    .get("albumName")
                    .or_else(|| a.get("album_name"))
                    .and_then(Value::as_str)
                    .unwrap_or("未知专辑")
                    .to_string();
                let pdate = a
                    .get("publishDate")
                    .and_then(Value::as_str)
                    .unwrap_or(title_hint)
                    .to_string();
                related_albums.push(QQCard {
                    id: amid.to_string(),
                    kind: "qq-album".to_string(),
                    title: aname,
                    subtitle: pdate,
                    description: String::new(),
                    artwork_url: qq_album_cover(amid),
                    badge: None,
                });
            }
        }
        return Ok(QQDetail {
            id: id.to_string(),
            kind: kind.to_string(),
            title: title_hint.to_string(),
            subtitle: format!("QQ 音乐 · {} 首热门 · {} 张专辑", tracks.len(), related_albums.len()),
            description: String::new(),
            artwork_url,
            tracks,
            related_albums,
        });
    }

    // Default: qq-playlist (`disstid`)
    let disstid = id.parse::<u64>().unwrap_or(0);
    let payload = json!({
        "pl": {
            "module": "music.srfDissInfo.aiDissInfo",
            "method": "uniform_get_Dissinfo",
            "param": {"disstid": disstid, "userinfo": 1, "tag": 1, "song_begin": 0, "song_num": 100}
        }
    });
    let json = qq_musicu(payload, cookie).await?;
    let dir = json.pointer("/pl/data/dirinfo").unwrap_or(&Value::Null);
    let title = decode_html_entities(
        dir.get("title")
            .and_then(Value::as_str)
            .unwrap_or(title_hint),
    );
    let creator = dir
        .pointer("/creator/nick")
        .and_then(Value::as_str)
        .unwrap_or("QQ 音乐歌单");
    let description = decode_html_entities(
        dir.get("desc").and_then(Value::as_str).unwrap_or_default(),
    );
    let artwork_url = dir
        .get("picurl")
        .and_then(Value::as_str)
        .map(upgrade_https)
        .filter(|s| !s.is_empty());
    let tracks: Vec<AgentTrackInput> = json
        .pointer("/pl/data/songlist")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| parse_qq_song_modern(v, artwork_url.as_deref()))
                .collect()
        })
        .unwrap_or_default();

    Ok(QQDetail {
        id: id.to_string(),
        kind: "qq-playlist".to_string(),
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
    let payload = json!({
        "search": {
            "module": "music.search.SearchCgiService",
            "method": "DoSearchForQQMusicDesktop",
            "param": {
                "query": clean,
                "page_num": 1,
                "num_per_page": 6,
                "search_type": 1
            }
        }
    });
    let json = qq_musicu(payload, cookie).await.ok()?;
    let arr = json.pointer("/search/data/body/singer/list").and_then(Value::as_array)?;
    let want = crate::backends::normalize_match_text(clean);
    let best = arr
        .iter()
        .find(|v| {
            v.get("singerName")
                .or_else(|| v.get("singer_name"))
                .and_then(Value::as_str)
                .is_some_and(|nm| crate::backends::normalize_match_text(nm) == want)
        })
        .or_else(|| arr.first())?;
    let mid = best
        .get("singerMID")
        .or_else(|| best.get("singerMid"))
        .or_else(|| best.get("singer_mid"))
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if mid.is_empty() {
        return None;
    }
    let nm = best
        .get("singerName")
        .or_else(|| best.get("singer_name"))
        .and_then(Value::as_str)
        .unwrap_or(clean)
        .to_string();
    Some((mid, nm))
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
    let payload = json!({
        "search": {
            "module": "music.search.SearchCgiService",
            "method": "DoSearchForQQMusicDesktop",
            "param": {
                "query": q,
                "page_num": 1,
                "num_per_page": 6,
                "search_type": 2
            }
        }
    });
    let json = qq_musicu(payload, cookie).await.ok()?;
    let arr = json.pointer("/search/data/body/album/list").and_then(Value::as_array)?;
    let want = crate::backends::normalize_match_text(alb);
    let best = arr
        .iter()
        .find(|v| {
            v.get("albumName")
                .and_then(Value::as_str)
                .is_some_and(|nm| crate::backends::normalize_match_text(nm) == want)
        })
        .or_else(|| arr.first())?;
    let mid = best.get("albumMID").and_then(Value::as_str)?.trim().to_string();
    if mid.is_empty() {
        return None;
    }
    let nm = best
        .get("albumName")
        .and_then(Value::as_str)
        .unwrap_or(alb)
        .to_string();
    Some((mid, nm))
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
    let payload = json!({
        "search": {
            "module": "music.search.SearchCgiService",
            "method": "DoSearchForQQMusicDesktop",
            "param": {
                "query": q,
                "page_num": 1,
                "num_per_page": 8,
                "search_type": 0
            }
        }
    });
    let json = qq_musicu(payload, cookie).await.ok()?;
    let arr = json.pointer("/search/data/body/song/list").and_then(Value::as_array)?;
    let mut best: Option<(f32, AgentTrackInput)> = None;
    for node in arr {
        let Some(tr) = parse_qq_song_modern(node, None) else {
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
// Audio Stream URL Resolver (`CgiGetVkey` with Quality Fallback) & Direct Lyrics
// ============================================================================

pub async fn resolve_song_stream(
    raw_song_mid: &str,
    media_mid_opt: Option<&str>,
    quality: QQQuality,
    cookie: Option<&str>,
) -> Result<ResolvedAudioStream, String> {
    let song_mid = strip_qq_id(raw_song_mid);
    if song_mid.is_empty() {
        return Err("无效的 QQ 音乐 songmid".to_string());
    }
    let media_mid = media_mid_opt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&song_mid);
    let (uin, _) = cookie
        .map(parse_qq_cookie_fields)
        .unwrap_or_else(|| ("0".to_string(), String::new()));
    let uin_param = if uin.is_empty() { "0".to_string() } else { uin };

    for &(prefix, ext, qlabel, kbps) in quality.fallback_chain() {
        let filename = format!("{prefix}{media_mid}.{ext}");
        let payload = json!({
            "req_0": {
                "module": "vkey.GetVkeyServer",
                "method": "CgiGetVkey",
                "param": {
                    "guid": "542516244",
                    "songmid": [song_mid],
                    "songtype": [0],
                    "uin": uin_param,
                    "loginflag": 1,
                    "platform": "20",
                    "filename": [filename]
                }
            }
        });
        if let Ok(vj) = qq_musicu(payload, cookie).await {
            let purl = vj
                .pointer("/req_0/data/midurlinfo/0/purl")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !purl.is_empty() {
                let sip = vj
                    .pointer("/req_0/data/sip/0")
                    .and_then(Value::as_str)
                    .filter(|s| s.starts_with("http"))
                    .unwrap_or("https://ws.stream.qqmusic.qq.com/");
                let full_url = if purl.starts_with("http") {
                    purl.to_string()
                } else {
                    format!("{sip}{purl}")
                };
                return Ok(ResolvedAudioStream {
                    url: full_url,
                    referer: QQ_REFERER.to_string(),
                    cookie: cookie.map(|s| s.to_string()),
                    file_ext: ext.to_string(),
                    quality_label: qlabel.to_string(),
                    nominal_bitrate_kbps: kbps,
                    is_trial: false,
                });
            }
        }
    }

    // Final default CgiGetVkey request without explicit filename (lets server pick any free/trial stream)
    let fallback_payload = json!({
        "req_0": {
            "module": "vkey.GetVkeyServer",
            "method": "CgiGetVkey",
            "param": {
                "guid": "542516244",
                "songmid": [song_mid],
                "songtype": [0],
                "uin": uin_param,
                "loginflag": 1,
                "platform": "20"
            }
        }
    });
    if let Ok(vj) = qq_musicu(fallback_payload, cookie).await {
        let purl = vj
            .pointer("/req_0/data/midurlinfo/0/purl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !purl.is_empty() {
            let sip = vj
                .pointer("/req_0/data/sip/0")
                .and_then(Value::as_str)
                .filter(|s| s.starts_with("http"))
                .unwrap_or("https://ws.stream.qqmusic.qq.com/");
            return Ok(ResolvedAudioStream {
                url: format!("{sip}{purl}"),
                referer: QQ_REFERER.to_string(),
                cookie: cookie.map(|s| s.to_string()),
                file_ext: "m4a".to_string(),
                quality_label: "M4A 标准".to_string(),
                nominal_bitrate_kbps: 96,
                is_trial: false,
            });
        }
    }

    Err("该曲目在 QQ 音乐需要绿钻 VIP 或受版权限制（可在设置粘贴 QQ 音乐 Cookie 解锁）".to_string())
}

/// Fetches direct QQ Music LRC + Chinese translation by exact `songmid`!
pub async fn fetch_lyrics_by_mid(raw_song_mid: &str, cookie: Option<&str>) -> Option<ParsedLyrics> {
    let mid = strip_qq_id(raw_song_mid);
    if mid.is_empty() {
        return None;
    }
    let url = format!(
        "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg?songmid={mid}&format=json&nobase64=1"
    );
    let mut req = QQ_HTTP.get(&url).header("Referer", QQ_REFERER);
    if let Some(ck) = cookie.map(str::trim).filter(|s| !s.is_empty()) {
        req = req.header("Cookie", ck);
    }
    let resp = req.send().await.ok()?;
    let json: Value = resp.json().await.ok()?;
    let lrc_raw = json
        .get("lyric")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())?;
    let lrc = decode_html_entities(lrc_raw);
    let trans_owned = json
        .get("trans")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(decode_html_entities);
    lyrics::parse_lrc_lyrics(&lrc, trans_owned.as_deref(), "QQ 音乐歌词 (LRC)")
}
