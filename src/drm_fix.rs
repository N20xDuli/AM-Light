use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DRM_FIX_REPO_URL: &str = "https://github.com/N20xDuli/apple-music-windows-drm-fix";
pub const MAX_DRM_BACKUPS: usize = 3;

/// Retains only the newest `keep_latest` `amfix-backup-*` directories under `backup_root_parent`
/// and deletes older backup folders to prevent unbounded disk growth. Returns pruned count.
pub fn prune_old_drm_backups(backup_root_parent: &Path, keep_latest: usize) -> usize {
    let Ok(rd) = fs::read_dir(backup_root_parent) else {
        return 0;
    };
    let mut dirs: Vec<(u64, SystemTime, String, PathBuf)> = rd
        .flatten()
        .filter_map(|ent| {
            let meta = ent.metadata().ok()?;
            if !meta.is_dir() {
                return None;
            }
            let name = ent.file_name().to_str()?.to_string();
            let ts_suffix = name.strip_prefix("amfix-backup-")?.parse::<u64>().unwrap_or(0);
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            Some((ts_suffix, mtime, name, ent.path()))
        })
        .collect();
    if dirs.len() <= keep_latest {
        return 0;
    }
    // Sort newest first by parsed timestamp suffix, then mtime, then directory name.
    dirs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| b.2.cmp(&a.2)));
    let mut removed = 0usize;
    for (_, _, _, path) in dirs.into_iter().skip(keep_latest) {
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrmDiagReport {
    pub checked: bool,
    pub healthy: bool,
    pub adi_dir_exists: bool,
    pub adi_file_count: usize,
    pub unwritable_files: Vec<String>,
    pub sc_info_unwritable: bool,
    pub hklm_redirections: Vec<String>,
    pub summary: String,
    pub detail: String,
}

fn is_file_unwritable_by_user(p: &Path) -> bool {
    let Ok(meta) = fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if meta.permissions().readonly() {
        return true;
    }
    match fs::OpenOptions::new().read(true).write(true).open(p) {
        Ok(_) => false,
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(5),
    }
}

pub fn diagnose_drm_paths(
    adi_dir: &Path,
    sc_info_file: &Path,
    hklm_redirections: Vec<String>,
) -> DrmDiagReport {
    let adi_dir_exists = adi_dir.is_dir();
    let mut adi_file_count = 0usize;
    let mut unwritable_files = Vec::new();

    if adi_dir_exists {
        if let Ok(entries) = fs::read_dir(adi_dir) {
            for ent in entries.flatten() {
                let p = ent.path();
                if p.is_file() {
                    adi_file_count += 1;
                    if is_file_unwritable_by_user(&p) {
                        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                            unwritable_files.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    unwritable_files.sort();

    let sc_info_unwritable = sc_info_file.is_file() && is_file_unwritable_by_user(sc_info_file);
    let healthy = unwritable_files.is_empty() && !sc_info_unwritable;

    let mut bad_items = unwritable_files.clone();
    if sc_info_unwritable {
        bad_items.push("SC Info.txt".to_string());
    }

    let (summary, detail) = if !healthy {
        let count = bad_items.len();
        let summary = format!(
            "检测到 DRM 身份文件不可写（共 {count} 项），会导致无法播放或点歌秒切"
        );
        let mut detail = format!("异常文件：{}", bad_items.join("、"));
        if !hklm_redirections.is_empty() {
            detail.push_str(&format!(
                " · 另有 {} 项第三方 DRM 重定向",
                hklm_redirections.len()
            ));
        }
        detail.push_str(" · 请点击「一键修复」自动备份并修复权限");
        (summary, detail)
    } else if !hklm_redirections.is_empty() {
        let summary = format!(
            "身份文件可写 · 检测到 {} 项第三方 DRM 重定向（如 Sideloadly / 爱思）",
            hklm_redirections.len()
        );
        let detail = "若原版 Apple Music 出现无法播放或秒切，可点击「一键修复」备份并清理重定向"
            .to_string();
        (summary, detail)
    } else {
        let summary = if adi_file_count > 0 {
            format!("DRM 授权状态正常（{adi_file_count} 个 ADI 身份文件均可正常读写）")
        } else {
            "DRM 目录状态正常（首次在原版客户端播放时自动创建身份文件）".to_string()
        };
        let detail = "若原版 Apple Music 仍遇到点歌秒切、0:00 无声，可点击「一键修复」重置设备身份（自动备份）"
            .to_string();
        (summary, detail)
    };

    DrmDiagReport {
        checked: true,
        healthy,
        adi_dir_exists,
        adi_file_count,
        unwritable_files,
        sc_info_unwritable,
        hklm_redirections,
        summary,
        detail,
    }
}

#[cfg(target_os = "windows")]
fn query_hklm_drm_redirections() -> Vec<String> {
    const HKEY_LOCAL_MACHINE: isize = -0x7FFF_FFFE; // 0x80000002
    const KEY_READ: u32 = 0x0002_0019;
    const KEY_WOW64_64KEY: u32 = 0x0100;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: isize,
            sub_key: *const u16,
            ul_options: u32,
            sam_desired: u32,
            phk_result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: isize,
            lp_value_name: *const u16,
            lp_reserved: *const u32,
            lp_type: *mut u32,
            lp_data: *mut u8,
            lpcb_data: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }

    let targets: &[(&str, &str)] = &[
        ("SOFTWARE\\Apple Inc.\\CoreADI", "LibSidePath"),
        ("SOFTWARE\\Apple Inc.\\CoreFP", "LibSidePath"),
        ("SOFTWARE\\Apple Inc.\\CoreFP", "Libi4CFPath"),
        ("SOFTWARE\\Apple Inc.\\CoreFP", "LibiiiiPath"),
        (
            "SOFTWARE\\Apple Inc.\\Apple Application Support",
            "InsSideDir",
        ),
    ];

    let mut found = Vec::new();
    for &(sub_key, val_name) in targets {
        let sub_w: Vec<u16> = sub_key.encode_utf16().chain(Some(0)).collect();
        let val_w: Vec<u16> = val_name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let mut hkey: isize = 0;
            let rc = RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                sub_w.as_ptr(),
                0,
                KEY_READ | KEY_WOW64_64KEY,
                &mut hkey,
            );
            if rc != 0 || hkey == 0 {
                continue;
            }
            let mut ty: u32 = 0;
            let mut cb: u32 = 0;
            let rc_sz = RegQueryValueExW(
                hkey,
                val_w.as_ptr(),
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut cb,
            );
            if rc_sz == 0 && cb > 2 {
                let mut buf = vec![0u16; (cb as usize + 1) / 2];
                let mut cb2 = (buf.len() * 2) as u32;
                let rc_val = RegQueryValueExW(
                    hkey,
                    val_w.as_ptr(),
                    std::ptr::null(),
                    &mut ty,
                    buf.as_mut_ptr() as *mut u8,
                    &mut cb2,
                );
                if rc_val == 0 {
                    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    let val_str = String::from_utf16_lossy(&buf[..end]).trim().to_string();
                    if !val_str.is_empty() {
                        found.push(format!("{val_name} = {val_str}"));
                    }
                }
            }
            let _ = RegCloseKey(hkey);
        }
    }
    found
}

#[cfg(not(target_os = "windows"))]
fn query_hklm_drm_redirections() -> Vec<String> {
    Vec::new()
}

pub fn check_apple_music_drm_health() -> DrmDiagReport {
    let pd = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let apple_pd = pd.join("Apple Computer");
    let adi_dir = apple_pd.join("iTunes").join("adi");
    let sc_info = apple_pd.join("iTunes").join("SC Info").join("SC Info.txt");
    let redirs = query_hklm_drm_redirections();
    diagnose_drm_paths(&adi_dir, &sc_info, redirs)
}

const EMBEDDED_FIX_SCRIPT: &str = r#"[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BackupRoot
)

$ErrorActionPreference = 'Stop'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "此修复脚本需要管理员权限执行。" -ForegroundColor Red
    exit 1
}

$programData = $env:ProgramData
$applePd     = Join-Path $programData 'Apple Computer'
$adiDir      = Join-Path $applePd 'iTunes\adi'
$scInfoDir   = Join-Path $applePd 'iTunes\SC Info'

Write-Host "AM Light - Apple Music DRM 修复工具" -ForegroundColor Cyan
Write-Host "  备份目录: $BackupRoot"

# 先释放可能占用 .pb 文件的进程，避免清理失败
Stop-Process -Name 'AppleMusic', 'AMPLibraryAgent' -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

New-Item -ItemType Directory -Force -Path $BackupRoot | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $BackupRoot 'adi') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $BackupRoot 'SCInfo') | Out-Null

Write-Host "=== 1/5 备份并清理失效 Anisette / ADI 身份文件 ===" -ForegroundColor Cyan
if (Test-Path $adiDir) {
    foreach ($f in @(Get-ChildItem $adiDir -File -Force -ErrorAction SilentlyContinue)) {
        Copy-Item $f.FullName (Join-Path $BackupRoot 'adi') -Force -ErrorAction SilentlyContinue
        Remove-Item $f.FullName -Force -ErrorAction Stop
        Write-Host "  [已清理] $($f.Name)" -ForegroundColor Yellow
    }
}

Write-Host "=== 2/5 备份并修复 Apple Computer 目录写入权限 ===" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $adiDir | Out-Null
if (Test-Path $applePd) {
    & icacls.exe $applePd /save (Join-Path $BackupRoot 'acl-apple-computer.txt') /T /C | Out-Null
    & icacls.exe $applePd /grant '*S-1-5-32-545:(OI)(CI)M' /T /C | Out-Null
    Write-Host "  [完成] 已为当前用户组 (BUILTIN\Users) 授予递归修改权限" -ForegroundColor Green
}

Write-Host "=== 3/5 备份并重置 FairPlay SC Info ===" -ForegroundColor Cyan
if (Test-Path $scInfoDir) {
    foreach ($f in @(Get-ChildItem $scInfoDir -File -Force -ErrorAction SilentlyContinue)) {
        Copy-Item $f.FullName (Join-Path $BackupRoot 'SCInfo') -Force -ErrorAction SilentlyContinue
        Remove-Item $f.FullName -Force -ErrorAction Stop
        Write-Host "  [已清理] $($f.Name)" -ForegroundColor Yellow
    }
}

Write-Host "=== 4/5 备份并清理第三方 DRM 重定向 (HKLM) ===" -ForegroundColor Cyan
$redirections = @(
    @{ Key = 'HKLM:\SOFTWARE\Apple Inc.\CoreADI';                   Name = 'LibSidePath' },
    @{ Key = 'HKLM:\SOFTWARE\Apple Inc.\CoreFP';                    Name = 'LibSidePath' },
    @{ Key = 'HKLM:\SOFTWARE\Apple Inc.\CoreFP';                    Name = 'Libi4CFPath' },
    @{ Key = 'HKLM:\SOFTWARE\Apple Inc.\CoreFP';                    Name = 'LibiiiiPath' },
    @{ Key = 'HKLM:\SOFTWARE\Apple Inc.\Apple Application Support'; Name = 'InsSideDir' }
)
$regKeys = @($redirections | ForEach-Object { $_.Key } | Select-Object -Unique)
foreach ($k in $regKeys) {
    if (Test-Path $k) {
        $name = ($k -replace '[\\: ]', '_')
        $regPath = ($k -replace '^HKLM:\\', 'HKLM\') -replace '^HKCU:\\', 'HKCU\'
        & reg.exe export "$regPath" (Join-Path $BackupRoot "$name.reg") /y | Out-Null
    }
}
foreach ($r in $redirections) {
    if (Test-Path $r.Key) {
        $cur = (Get-ItemProperty -Path $r.Key -Name $r.Name -ErrorAction SilentlyContinue).($r.Name)
        if ($cur) {
            Remove-ItemProperty -Path $r.Key -Name $r.Name -Force -ErrorAction SilentlyContinue
            Write-Host "  [已移除] $($r.Name) = $cur" -ForegroundColor Yellow
        }
    }
}

Write-Host "=== 5/5 重启 Apple Mobile Device 服务并生成回滚脚本 ===" -ForegroundColor Cyan
$svc = Get-Service -ErrorAction SilentlyContinue |
       Where-Object { $_.Name -match 'Apple.*Mobile.*Device' -or $_.DisplayName -match 'Apple Mobile Device' } |
       Select-Object -First 1
if ($svc) {
    try { Restart-Service $svc.Name -Force; Write-Host "  [完成] 已重启服务: $($svc.Name)" -ForegroundColor Green }
    catch { Write-Host "  [警告] 无法重启 $($svc.Name)" -ForegroundColor Magenta }
}

$rbPath = Join-Path $BackupRoot 'rollback-amfix.ps1'
$rb = @'
#Requires -RunAsAdministrator
$ErrorActionPreference = 'Continue'
$bk = $PSScriptRoot
$applePd = Join-Path $env:ProgramData 'Apple Computer'
$adiDir  = Join-Path $applePd 'iTunes\adi'
$scDir   = Join-Path $applePd 'iTunes\SC Info'
New-Item -ItemType Directory -Force -Path $adiDir, $scDir | Out-Null
Get-ChildItem (Join-Path $bk 'adi')    -Force -File -ErrorAction SilentlyContinue | Copy-Item -Destination $adiDir -Force
Get-ChildItem (Join-Path $bk 'SCInfo') -Force -File -ErrorAction SilentlyContinue | Copy-Item -Destination $scDir  -Force
Get-ChildItem $bk -Filter '*.reg' -File | ForEach-Object { & reg.exe import $_.FullName }
if (Test-Path (Join-Path $bk 'acl-apple-computer.txt')) {
    & icacls.exe $applePd /restore (Join-Path $bk 'acl-apple-computer.txt') /C | Out-Null
}
$svc = Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'Apple.*Mobile.*Device' } | Select-Object -First 1
if ($svc) { Restart-Service $svc.Name -Force -ErrorAction SilentlyContinue }
Write-Host 'Rollback complete.'
'@
Set-Content -Path $rbPath -Value $rb -Encoding UTF8
Write-Host "修复完成！请打开原版 Apple Music 播放任意歌曲完成身份初始化。" -ForegroundColor Green
Start-Sleep -Milliseconds 900
exit 0
"#;

#[cfg(target_os = "windows")]
pub fn run_elevated_drm_repair_blocking() -> Result<String, String> {
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0,
    };
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;

    #[repr(C)]
    struct ShellExecuteInfoW {
        cb_size: u32,
        f_mask: u32,
        hwnd: *mut std::ffi::c_void,
        lp_verb: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show: i32,
        h_inst_app: *mut std::ffi::c_void,
        lp_id_list: *mut std::ffi::c_void,
        lp_class: *const u16,
        hkey_class: *mut std::ffi::c_void,
        dw_hot_key: u32,
        h_monitor_or_icon: *mut std::ffi::c_void,
        h_process: *mut std::ffi::c_void,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(p_exec_info: *mut ShellExecuteInfoW) -> i32;
    }

    let base_dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("AMLight");
    let fix_dir = base_dir.join("drm-fix");
    let backup_root_parent = base_dir.join("drm-backups");
    fs::create_dir_all(&fix_dir).map_err(|e| format!("创建修复脚本目录失败: {e}"))?;
    fs::create_dir_all(&backup_root_parent).map_err(|e| format!("创建备份目录失败: {e}"))?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_dir = backup_root_parent.join(format!("amfix-backup-{ts}"));
    let script_path = fix_dir.join("Fix-AppleMusicDRM.ps1");

    // Prepend UTF-8 BOM so Windows PowerShell 5.1 always reads UTF-8 Chinese messages properly
    let script_with_bom = format!("\u{FEFF}{EMBEDDED_FIX_SCRIPT}");
    fs::write(&script_path, script_with_bom.as_bytes())
        .map_err(|e| format!("写入修复脚本失败: {e}"))?;

    let verb_w: Vec<u16> = "runas\0".encode_utf16().collect();
    let file_w: Vec<u16> = "powershell.exe\0".encode_utf16().collect();
    let params_s = format!(
        "-NoProfile -ExecutionPolicy Bypass -File \"{}\" -BackupRoot \"{}\"\0",
        script_path.display(),
        backup_dir.display()
    );
    let params_w: Vec<u16> = params_s.encode_utf16().collect();

    unsafe {
        let mut sei: ShellExecuteInfoW = std::mem::zeroed();
        sei.cb_size = std::mem::size_of::<ShellExecuteInfoW>() as u32;
        sei.f_mask = SEE_MASK_NOCLOSEPROCESS;
        sei.lp_verb = verb_w.as_ptr();
        sei.lp_file = file_w.as_ptr();
        sei.lp_parameters = params_w.as_ptr();
        sei.n_show = SW_SHOWNORMAL.0;

        if ShellExecuteExW(&mut sei) == 0 {
            let err = GetLastError();
            if err == ERROR_CANCELLED {
                return Err("已取消管理员授权，未对系统做出任何修改".to_string());
            }
            return Err(format!("无法启动管理员修复进程 (Win32={})", err.0));
        }

        let hproc = HANDLE(sei.h_process);
        if hproc.0.is_null() {
            return Err("未获取到管理员修复进程句柄".to_string());
        }
        let wait_rc = WaitForSingleObject(hproc, 90_000);
        let mut exit_code: u32 = 1;
        let got_code = GetExitCodeProcess(hproc, &mut exit_code).is_ok();
        let _ = CloseHandle(hproc);

        if wait_rc != WAIT_OBJECT_0 {
            return Err("DRM 修复脚本执行超时，请查看系统终端输出或前往项目主页排查".to_string());
        }
        if !got_code || exit_code != 0 {
            return Err(format!(
                "DRM 修复未完成（退出码 {exit_code}），可前往项目主页手动执行诊断"
            ));
        }
    }

    let _ = prune_old_drm_backups(&backup_root_parent, MAX_DRM_BACKUPS);

    Ok(format!(
        "DRM 修复已完成（已自动备份至 {}），请先打开原版 Apple Music 播放任意歌曲完成授权重建",
        backup_dir.display()
    ))
}

#[cfg(not(target_os = "windows"))]
pub fn run_elevated_drm_repair_blocking() -> Result<String, String> {
    Err("仅支持在 Windows 系统执行 DRM 修复".to_string())
}
