use percent_encoding::percent_decode_str;
use regex::Regex;
use serde::{Deserialize, Serialize};
use once_cell::sync::Lazy;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadedTrackItem {
    pub path: String,
    pub title: String,
    pub track_number: Option<u32>,
    pub artist: String,
    pub album: String,
    pub format: String,
    pub resolution: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub size_bytes: u64,
    pub complete: bool,
    pub modified_unix: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadsOverview {
    pub media_folder: String,
    pub folder_exists: bool,
    pub total_bytes: u64,
    pub complete_count: usize,
    pub partial_count: usize,
    pub items: Vec<DownloadedTrackItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedMovpkgMeta {
    pub format: String,
    pub resolution: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub complete: bool,
}

static STREAM_BLOCK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<Stream\b([^>]*)>(.*?)</Stream>"#).expect("valid Stream regex")
});

static ATTR_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"ID="([^"]+)""#).expect("valid ID attr regex"));

static ATTR_URL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"NetworkURL="([^"]+)""#).expect("valid NetworkURL attr regex"));

static GR_RATE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"_gr(\d+)_"#).expect("valid gr rate regex"));

static TRACK_PREFIX_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"^\s*(?:(\d+)-)?(\d{1,3})\s+(.+)$"#).expect("valid track prefix"));

pub fn decode_file_url_to_path(url_or_path: &str) -> Option<PathBuf> {
    let trimmed = url_or_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed
        .strip_prefix("file://localhost/")
        .or_else(|| trimmed.strip_prefix("file:///"))
    {
        let decoded = percent_decode_str(rest).decode_utf8().ok()?;
        let normalized = decoded.trim_end_matches('/').replace('/', "\\");
        return Some(PathBuf::from(normalized));
    }
    Some(PathBuf::from(trimmed))
}

pub fn resolve_media_music_dir(library_hint: Option<&str>) -> PathBuf {
    if let Some(hint) = library_hint {
        if let Some(p) = decode_file_url_to_path(hint) {
            let base = if p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("musiclibrary"))
                .unwrap_or(false)
            {
                p.parent().map(Path::to_path_buf).unwrap_or(p)
            } else {
                p
            };

            if base.ends_with("Media\\Music") || base.ends_with("Media/Music") {
                return base;
            }
            let candidate = base.join("Media").join("Music");
            if candidate.exists() {
                return candidate;
            }
            let localized = base.join("Media.localized").join("Music");
            if localized.exists() {
                return localized;
            }
            return candidate;
        }
    }

    if let Ok(profile) = std::env::var("USERPROFILE") {
        return PathBuf::from(profile)
            .join("Music")
            .join("Apple Music")
            .join("Media")
            .join("Music");
    }

    PathBuf::from("Media\\Music")
}

fn map_group_rate_to_resolution(gr: u32, is_alac: bool) -> Option<String> {
    if is_alac {
        let res = match gr {
            1350..=1460 => "16-bit / 44.1 kHz",
            1480..=1600 => "16-bit / 48 kHz",
            2050..=2180 => "24-bit / 44.1 kHz",
            2240..=2400 => "24-bit / 48 kHz",
            4100..=4350 => "24-bit / 88.2 kHz",
            4500..=4750 => "24-bit / 96 kHz",
            8300..=8600 => "24-bit / 176.4 kHz",
            9000..=9400 => "24-bit / 192 kHz",
            _ => return None,
        };
        Some(res.to_string())
    } else {
        match gr {
            240..=320 => Some("44.1 kHz • 256 kbps".to_string()),
            48..=96 => Some("HE-AAC 省流".to_string()),
            _ => None,
        }
    }
}

pub fn parse_movpkg_boot_xml(xml: &str) -> ParsedMovpkgMeta {
    #[derive(Clone)]
    struct Candidate {
        complete: bool,
        bps: u64,
        url: String,
    }

    let mut streams = Vec::new();
    for cap in STREAM_BLOCK_RE.captures_iter(xml) {
        let attrs = &cap[1];
        let inner = &cap[2];
        let complete = inner.contains("<Complete>YES</Complete>");
        let id_str = ATTR_ID_RE
            .captures(attrs)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .unwrap_or_default();
        let bps = id_str
            .split('-')
            .nth(1)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let url = ATTR_URL_RE
            .captures(attrs)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        streams.push(Candidate { complete, bps, url });
    }

    let any_complete = streams.iter().any(|s| s.complete);
    let chosen = streams
        .iter()
        .filter(|s| s.complete && s.bps > 0)
        .max_by_key(|s| s.bps)
        .or_else(|| streams.iter().filter(|s| s.complete).max_by_key(|s| s.bps))
        .or_else(|| streams.iter().max_by_key(|s| s.bps));

    let Some(best) = chosen else {
        return ParsedMovpkgMeta {
            format: "Apple Music 离线包".to_string(),
            resolution: None,
            bitrate_kbps: None,
            complete: any_complete,
        };
    };

    let url_lower = best.url.to_ascii_lowercase();
    let gr_val = GR_RATE_RE
        .captures(&url_lower)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u32>().ok());

    let is_alac = url_lower.contains("_alac") || url_lower.contains("lossless");
    let is_atmos = url_lower.contains("atmos")
        || url_lower.contains("_ec3")
        || url_lower.contains("ec-3")
        || url_lower.contains("ac-4");
    let is_aac = url_lower.contains("mp4a");

    let bitrate_kbps = if best.bps > 0 {
        Some(((best.bps + 500) / 1000) as u32)
    } else {
        None
    };

    let is_hires = is_alac
        && (gr_val.map(|g| g > 2400).unwrap_or(false)
            || bitrate_kbps.map(|b| b > 1950).unwrap_or(false));

    let format = if is_hires {
        "Hi-Res 无损 ALAC".to_string()
    } else if is_alac {
        "无损 ALAC".to_string()
    } else if is_atmos {
        "杜比全景声".to_string()
    } else if is_aac {
        "高质量 AAC".to_string()
    } else {
        "Apple Music 离线包".to_string()
    };

    let resolution = gr_val
        .and_then(|g| map_group_rate_to_resolution(g, is_alac))
        .or_else(|| {
            if is_atmos {
                Some("48 kHz • 空间音频".to_string())
            } else {
                None
            }
        });

    ParsedMovpkgMeta {
        format,
        resolution,
        bitrate_kbps,
        complete: any_complete,
    }
}

fn split_track_stem(stem: &str) -> (Option<u32>, String) {
    let trimmed = stem.trim();
    if let Some(cap) = TRACK_PREFIX_RE.captures(trimmed) {
        let num = cap.get(2).and_then(|m| m.as_str().parse::<u32>().ok());
        let name = cap
            .get(3)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_else(|| trimmed.to_string());
        if !name.is_empty() {
            return (num, name);
        }
    }
    (None, trimmed.to_string())
}

fn sum_dir_size_and_mtime(dir: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut latest_mtime = dir
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let Ok(entries) = fs::read_dir(dir) else {
        return (bytes, latest_mtime);
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if let Ok(meta) = entry.metadata() {
            if let Ok(mt) = meta.modified() {
                if let Ok(dur) = mt.duration_since(UNIX_EPOCH) {
                    latest_mtime = latest_mtime.max(dur.as_secs());
                }
            }
            if meta.is_dir() {
                let (sub_b, sub_m) = sum_dir_size_and_mtime(&p);
                bytes += sub_b;
                latest_mtime = latest_mtime.max(sub_m);
            } else {
                bytes += meta.len();
            }
        }
    }
    (bytes, latest_mtime)
}

fn extract_artist_album(item_path: &Path, root_dir: &Path) -> (String, String) {
    let Ok(rel) = item_path.strip_prefix(root_dir) else {
        return ("未知艺人".to_string(), "未知专辑".to_string());
    };
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    match comps.len() {
        0 | 1 => ("本地缓存".to_string(), "单曲".to_string()),
        2 => (comps[0].clone(), "未分类专辑".to_string()),
        _ => (comps[0].clone(), comps[1].clone()),
    }
}

fn walk_music_dir(dir: &Path, root_dir: &Path, depth: usize, out: &mut Vec<DownloadedTrackItem>) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let file_name = entry.file_name().to_string_lossy().to_string();

        if ft.is_dir() {
            if file_name.to_ascii_lowercase().ends_with(".movpkg") {
                let stem = &file_name[..file_name.len().saturating_sub(7)];
                let (track_number, title) = split_track_stem(stem);
                let (artist, album) = extract_artist_album(&path, root_dir);
                let (size_bytes, modified_unix) = sum_dir_size_and_mtime(&path);

                let boot_path = path.join("boot.xml");
                let parsed = match fs::read_to_string(&boot_path) {
                    Ok(xml) => parse_movpkg_boot_xml(&xml),
                    Err(_) => ParsedMovpkgMeta {
                        format: "Apple Music 离线包".to_string(),
                        resolution: None,
                        bitrate_kbps: None,
                        complete: false,
                    },
                };

                out.push(DownloadedTrackItem {
                    path: path.to_string_lossy().to_string(),
                    title,
                    track_number,
                    artist,
                    album,
                    format: parsed.format,
                    resolution: parsed.resolution,
                    bitrate_kbps: parsed.bitrate_kbps,
                    size_bytes,
                    complete: parsed.complete,
                    modified_unix,
                });
            } else {
                walk_music_dir(&path, root_dir, depth + 1, out);
            }
        } else if ft.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if matches!(
                ext.as_str(),
                "m4a" | "m4p" | "flac" | "mp3" | "wav" | "alac"
            ) {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&file_name);
                let (track_number, title) = split_track_stem(stem);
                let (artist, album) = extract_artist_album(&path, root_dir);
                let meta = entry.metadata().ok();
                let size_bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified_unix = meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                out.push(DownloadedTrackItem {
                    path: path.to_string_lossy().to_string(),
                    title,
                    track_number,
                    artist,
                    album,
                    format: format!("本地音频 ({})", ext.to_ascii_uppercase()),
                    resolution: None,
                    bitrate_kbps: None,
                    size_bytes,
                    complete: size_bytes > 0,
                    modified_unix,
                });
            }
        }
    }
}

pub fn list_downloads(library_hint: Option<&str>) -> Result<DownloadsOverview, String> {
    let media_dir = resolve_media_music_dir(library_hint);
    let folder_exists = media_dir.exists();
    let mut items = Vec::new();

    if folder_exists {
        walk_music_dir(&media_dir, &media_dir, 0, &mut items);
    }

    items.sort_by(|a, b| {
        b.modified_unix
            .cmp(&a.modified_unix)
            .then_with(|| a.artist.cmp(&b.artist))
            .then_with(|| a.album.cmp(&b.album))
            .then_with(|| a.track_number.unwrap_or(999).cmp(&b.track_number.unwrap_or(999)))
    });

    let total_bytes = items.iter().map(|i| i.size_bytes).sum();
    let complete_count = items.iter().filter(|i| i.complete).count();
    let partial_count = items.len().saturating_sub(complete_count);

    Ok(DownloadsOverview {
        media_folder: media_dir.to_string_lossy().to_string(),
        folder_exists,
        total_bytes,
        complete_count,
        partial_count,
        items,
    })
}

pub fn delete_download_item(target_path: &str, library_hint: Option<&str>) -> Result<(), String> {
    let media_dir = resolve_media_music_dir(library_hint);
    if !media_dir.exists() {
        return Err("离线媒体目录不存在".to_string());
    }
    let canon_root = fs::canonicalize(&media_dir)
        .map_err(|e| format!("无法校验离线媒体根目录: {e}"))?;
    let canon_target = fs::canonicalize(target_path)
        .map_err(|e| format!("目标文件不存在或无法访问: {e}"))?;

    if !canon_target.starts_with(&canon_root) || canon_target == canon_root {
        return Err("安全限制：仅允许删除 Apple Music Media\\Music 目录下的离线音频条目".to_string());
    }

    let ext = canon_target
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if canon_target.is_dir() {
        if ext != "movpkg" {
            return Err("安全限制：仅允许移除 .movpkg 离线音频包目录".to_string());
        }
        fs::remove_dir_all(&canon_target)
            .map_err(|e| format!("删除离线包失败（可能正在被播放占用）: {e}"))?;
    } else if matches!(
        ext.as_str(),
        "m4a" | "m4p" | "flac" | "mp3" | "wav" | "alac"
    ) {
        fs::remove_file(&canon_target).map_err(|e| format!("删除音频文件失败: {e}"))?;
    } else {
        return Err("安全限制：不支持删除该类型文件".to_string());
    }

    // Clean up empty Album / Artist parent folders if left empty
    let mut parent_opt = canon_target.parent().map(Path::to_path_buf);
    for _ in 0..2 {
        if let Some(parent) = parent_opt {
            if parent != canon_root && parent.starts_with(&canon_root) {
                let is_empty = fs::read_dir(&parent)
                    .map(|mut it| it.next().is_none())
                    .unwrap_or(false);
                if is_empty {
                    let next_parent = parent.parent().map(Path::to_path_buf);
                    let _ = fs::remove_dir(&parent);
                    parent_opt = next_parent;
                    continue;
                }
            }
        }
        break;
    }

    Ok(())
}

pub fn reveal_in_explorer(target_path: &str) -> Result<(), String> {
    let p = Path::new(target_path);
    if !p.exists() {
        return Err("该离线文件不存在或已被移除".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(format!("/select,{}", p.display()))
            .spawn()
            .map_err(|e| format!("打开资源管理器失败: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("仅支持 Windows 平台".to_string())
    }
}

pub fn open_media_folder(library_hint: Option<&str>) -> Result<String, String> {
    let media_dir = resolve_media_music_dir(library_hint);
    let mut open_target = media_dir.clone();
    while !open_target.exists() {
        if let Some(p) = open_target.parent() {
            open_target = p.to_path_buf();
        } else {
            break;
        }
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(&open_target)
            .spawn()
            .map_err(|e| format!("打开目录失败: {e}"))?;
        Ok(open_target.to_string_lossy().to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("仅支持 Windows 平台".to_string())
    }
}
