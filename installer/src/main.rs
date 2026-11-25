#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::fs;
use std::mem::{size_of, zeroed};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};

const AM_LIGHT_EXE_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload_am_light.exe"));
const AMP_PROXY_DLL_BYTES: &[u8] = include_bytes!("../../c_proxy/amp_proxy.dll");

const APP_NAME: &str = "AM Light";
const APP_VERSION: &str = "0.2.11";
const APP_PUBLISHER: &str = "AM Light";
const UNINSTALL_KEY_SUB: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\AMLight";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;

const BS_PUSHBUTTON: u32 = 0x0000_0000;
const BS_DEFPUSHBUTTON: u32 = 0x0000_0001;
const BS_AUTOCHECKBOX: u32 = 0x0000_0003;
const ES_AUTOHSCROLL: u32 = 0x0000_0080;
const SS_LEFT: u32 = 0x0000_0000;

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SETFONT: u32 = 0x0030;
const WM_COMMAND: u32 = 0x0111;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const BST_CHECKED: usize = 1;
const SW_SHOWNORMAL: i32 = 1;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const MB_OK: u32 = 0x0000_0000;
const MB_ICONINFORMATION: u32 = 0x0000_0040;
const MB_ICONERROR: u32 = 0x0000_0010;
const MB_YESNO: u32 = 0x0000_0004;
const MB_ICONQUESTION: u32 = 0x0000_0020;
const IDYES: i32 = 6;

const IDC_DIR_EDIT: usize = 1001;
const IDC_BROWSE_BTN: usize = 1002;
const IDC_CHK_DESKTOP: usize = 1003;
const IDC_CHK_STARTMENU: usize = 1004;
const IDC_CHK_LAUNCH: usize = 1005;
const IDC_STATUS_LBL: usize = 1006;
const IDC_INSTALL_BTN: usize = 1007;
const IDC_CANCEL_BTN: usize = 1008;

const HKEY_CURRENT_USER: isize = -0x7FFF_FFFF; // 0x80000001u32 as i32 as isize
const KEY_ALL_ACCESS: u32 = 0x000F_003F;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;

const CSIDL_PROGRAMS: i32 = 0x0002;
const CSIDL_DESKTOPDIRECTORY: i32 = 0x0010;
const CSIDL_LOCAL_APPDATA: i32 = 0x001C;
const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
const BIF_NEWDIALOGSTYLE: u32 = 0x0040;
const MAX_PATH_W: usize = 520;

static H_DIR_EDIT: AtomicIsize = AtomicIsize::new(0);
static H_CHK_DESKTOP: AtomicIsize = AtomicIsize::new(0);
static H_CHK_STARTMENU: AtomicIsize = AtomicIsize::new(0);
static H_CHK_LAUNCH: AtomicIsize = AtomicIsize::new(0);
static H_STATUS_LBL: AtomicIsize = AtomicIsize::new(0);
static H_INSTALL_BTN: AtomicIsize = AtomicIsize::new(0);
static H_FONT_TITLE: AtomicIsize = AtomicIsize::new(0);
static H_FONT_BODY: AtomicIsize = AtomicIsize::new(0);

#[repr(C)]
#[derive(Clone, Copy)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const CLSID_SHELL_LINK: GUID = GUID {
    data1: 0x0002_1401,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};
const IID_ISHELL_LINK_W: GUID = GUID {
    data1: 0x0002_14F9,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};
const IID_IPERSIST_FILE: GUID = GUID {
    data1: 0x0000_010B,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[repr(C)]
struct WNDCLASSEXW {
    cb_size: u32,
    style: u32,
    lpfn_wnd_proc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: *mut c_void,
    h_icon: *mut c_void,
    h_cursor: *mut c_void,
    hbr_background: *mut c_void,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
    h_icon_sm: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default)]
struct MSG {
    hwnd: *mut c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: POINT,
}

#[repr(C)]
struct INITCOMMONCONTROLSEX {
    dw_size: u32,
    dw_icc: u32,
}

#[repr(C)]
struct BROWSEINFOW {
    hwnd_owner: *mut c_void,
    pidl_root: *const c_void,
    psz_display_name: *mut u16,
    lpsz_title: *const u16,
    ul_flags: u32,
    lpfn: *const c_void,
    l_param: isize,
    i_image: i32,
}

#[repr(C)]
struct PROCESSENTRY32W {
    dw_size: u32,
    cnt_usage: u32,
    th32_process_id: u32,
    th32_default_heap_id: usize,
    th32_module_id: u32,
    cnt_threads: u32,
    th32_parent_process_id: u32,
    pc_pri_class_base: i32,
    dw_flags: u32,
    sz_exe_file: [u16; 260],
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lp_module_name: *const u16) -> *mut c_void;
    fn CloseHandle(h_object: *mut c_void) -> i32;
    fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> *mut c_void;
    fn Process32FirstW(h_snapshot: *mut c_void, lppe: *mut PROCESSENTRY32W) -> i32;
    fn Process32NextW(h_snapshot: *mut c_void, lppe: *mut PROCESSENTRY32W) -> i32;
    fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> *mut c_void;
    fn TerminateProcess(h_process: *mut c_void, u_exit_code: u32) -> i32;
    fn Sleep(dw_milliseconds: u32);
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(lpwcx: *const WNDCLASSEXW) -> u16;
    fn CreateWindowExW(
        dw_ex_style: u32,
        lp_class_name: *const u16,
        lp_window_name: *const u16,
        dw_style: u32,
        x: i32,
        y: i32,
        n_width: i32,
        n_height: i32,
        hwnd_parent: *mut c_void,
        h_menu: *mut c_void,
        h_instance: *mut c_void,
        lp_param: *mut c_void,
    ) -> *mut c_void;
    fn DefWindowProcW(hwnd: *mut c_void, msg: u32, w_param: usize, l_param: isize) -> isize;
    fn GetMessageW(lp_msg: *mut MSG, hwnd: *mut c_void, min_f: u32, max_f: u32) -> i32;
    fn TranslateMessage(lp_msg: *const MSG) -> i32;
    fn DispatchMessageW(lp_msg: *const MSG) -> isize;
    fn PostQuitMessage(exit_code: i32);
    fn DestroyWindow(hwnd: *mut c_void) -> i32;
    fn ShowWindow(hwnd: *mut c_void, n_cmd_show: i32) -> i32;
    fn UpdateWindow(hwnd: *mut c_void) -> i32;
    fn SendMessageW(hwnd: *mut c_void, msg: u32, w_param: usize, l_param: isize) -> isize;
    fn GetWindowTextLengthW(hwnd: *mut c_void) -> i32;
    fn GetWindowTextW(hwnd: *mut c_void, lp_string: *mut u16, n_max_count: i32) -> i32;
    fn SetWindowTextW(hwnd: *mut c_void, lp_string: *const u16) -> i32;
    fn EnableWindow(hwnd: *mut c_void, enable: i32) -> i32;
    fn LoadIconW(h_instance: *mut c_void, lp_icon_name: *const u16) -> *mut c_void;
    fn LoadCursorW(h_instance: *mut c_void, lp_cursor_name: *const u16) -> *mut c_void;
    fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, u_type: u32) -> i32;
    fn GetDpiForSystem() -> u32;
    fn GetSystemMetrics(n_index: i32) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateFontW(
        c_height: i32,
        c_width: i32,
        c_escapement: i32,
        c_orientation: i32,
        c_weight: i32,
        b_italic: u32,
        b_underline: u32,
        b_strike_out: u32,
        i_char_set: u32,
        i_out_precision: u32,
        i_clip_precision: u32,
        i_quality: u32,
        i_pitch_and_family: u32,
        psz_face_name: *const u16,
    ) -> *mut c_void;
    fn DeleteObject(ho: *mut c_void) -> i32;
}

#[link(name = "comctl32")]
extern "system" {
    fn InitCommonControlsEx(picce: *const INITCOMMONCONTROLSEX) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn SHBrowseForFolderW(lpbi: *mut BROWSEINFOW) -> *mut c_void;
    fn SHGetPathFromIDListW(pidl: *const c_void, psz_path: *mut u16) -> i32;
    fn SHGetFolderPathW(
        hwnd: *mut c_void,
        csidl: i32,
        h_token: *mut c_void,
        dw_flags: u32,
        psz_path: *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pv_reserved: *mut c_void, dw_coinit: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(
        rclsid: *const GUID,
        p_unk_outer: *mut c_void,
        dw_cls_context: u32,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> i32;
    fn CoTaskMemFree(pv: *mut c_void);
}

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(
        h_key: isize,
        lp_sub_key: *const u16,
        reserved: u32,
        lp_class: *const u16,
        dw_options: u32,
        sam_desired: u32,
        lp_security_attributes: *const c_void,
        phk_result: *mut isize,
        lpdw_disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        h_key: isize,
        lp_value_name: *const u16,
        reserved: u32,
        dw_type: u32,
        lp_data: *const u8,
        cb_data: u32,
    ) -> i32;
    fn RegCloseKey(h_key: isize) -> i32;
    fn RegDeleteTreeW(h_key: isize, lp_sub_key: *const u16) -> i32;
}

fn to_w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn special_folder(csidl: i32) -> Option<PathBuf> {
    let mut buf = [0u16; MAX_PATH_W];
    let hr = unsafe { SHGetFolderPathW(null_mut(), csidl, null_mut(), 0, buf.as_mut_ptr()) };
    if hr < 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]);
    (!s.is_empty()).then(|| PathBuf::from(s))
}

fn default_install_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("Programs").join("AM-Light");
    }
    special_folder(CSIDL_LOCAL_APPDATA)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
        .join("Programs")
        .join("AM-Light")
}

fn terminate_running_am_light() {
    unsafe {
        let snap = CreateToolhelp32Snapshot(0x0000_0002, 0);
        if snap.is_null() || (snap as isize) == -1 {
            return;
        }
        let mut pe: PROCESSENTRY32W = zeroed();
        pe.dw_size = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe) != 0 {
            let mut killed_any = false;
            loop {
                let end = pe
                    .sz_exe_file
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(pe.sz_exe_file.len());
                let name = String::from_utf16_lossy(&pe.sz_exe_file[..end]);
                if name.eq_ignore_ascii_case("AM-Light.exe") {
                    let h_proc = OpenProcess(0x0001 /* PROCESS_TERMINATE */, 0, pe.th32_process_id);
                    if !h_proc.is_null() {
                        let _ = TerminateProcess(h_proc, 0);
                        let _ = CloseHandle(h_proc);
                        killed_any = true;
                    }
                }
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
            if killed_any {
                Sleep(350);
            }
        }
        let _ = CloseHandle(snap);
    }
}

fn create_shortcut(lnk_path: &Path, target_exe: &Path, work_dir: &Path, desc: &str) -> Result<(), String> {
    if let Some(parent) = lnk_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建快捷方式目录失败: {e}"))?;
    }
    unsafe {
        let _ = CoInitializeEx(null_mut(), 2 /* COINIT_APARTMENTTHREADED */);
        let mut sl_ptr: *mut c_void = null_mut();
        let hr = CoCreateInstance(
            &CLSID_SHELL_LINK,
            null_mut(),
            1, // CLSCTX_INPROC_SERVER
            &IID_ISHELL_LINK_W,
            &mut sl_ptr,
        );
        if hr < 0 || sl_ptr.is_null() {
            return Err(format!("CoCreateInstance(ShellLink) 失败: 0x{:08X}", hr as u32));
        }
        let vtbl = *(sl_ptr as *const *const *const c_void);
        let qi_fn: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32 =
            std::mem::transmute(*(vtbl.add(0)));
        let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*(vtbl.add(2)));
        let set_desc: unsafe extern "system" fn(*mut c_void, *const u16) -> i32 =
            std::mem::transmute(*(vtbl.add(7)));
        let set_workdir: unsafe extern "system" fn(*mut c_void, *const u16) -> i32 =
            std::mem::transmute(*(vtbl.add(9)));
        let set_icon: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 =
            std::mem::transmute(*(vtbl.add(17)));
        let set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> i32 =
            std::mem::transmute(*(vtbl.add(20)));

        let w_exe = to_w(&target_exe.to_string_lossy());
        let w_dir = to_w(&work_dir.to_string_lossy());
        let w_desc = to_w(desc);
        let _ = set_path(sl_ptr, w_exe.as_ptr());
        let _ = set_workdir(sl_ptr, w_dir.as_ptr());
        let _ = set_desc(sl_ptr, w_desc.as_ptr());
        let _ = set_icon(sl_ptr, w_exe.as_ptr(), 0);

        let mut pf_ptr: *mut c_void = null_mut();
        let hr_qi = qi_fn(sl_ptr, &IID_IPERSIST_FILE, &mut pf_ptr);
        if hr_qi >= 0 && !pf_ptr.is_null() {
            let pf_vtbl = *(pf_ptr as *const *const *const c_void);
            let pf_release: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(*(pf_vtbl.add(2)));
            let pf_save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 =
                std::mem::transmute(*(pf_vtbl.add(6)));
            let w_lnk = to_w(&lnk_path.to_string_lossy());
            let hr_save = pf_save(pf_ptr, w_lnk.as_ptr(), 1);
            let _ = pf_release(pf_ptr);
            let _ = release_fn(sl_ptr);
            if hr_save < 0 {
                return Err(format!("保存快捷方式失败: 0x{:08X}", hr_save as u32));
            }
            return Ok(());
        }
        let _ = release_fn(sl_ptr);
        Err(format!("QueryInterface(IPersistFile) 失败: 0x{:08X}", hr_qi as u32))
    }
}

fn write_uninstall_registry(install_dir: &Path, target_exe: &Path, uninstaller_exe: &Path) {
    unsafe {
        let mut h_key: isize = 0;
        let sub = to_w(UNINSTALL_KEY_SUB);
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            null(),
            0,
            KEY_ALL_ACCESS,
            null(),
            &mut h_key,
            null_mut(),
        );
        if rc != 0 || h_key == 0 {
            return;
        }
        let set_str = |name: &str, val: &str| {
            let wn = to_w(name);
            let wv = to_w(val);
            let _ = RegSetValueExW(
                h_key,
                wn.as_ptr(),
                0,
                REG_SZ,
                wv.as_ptr() as *const u8,
                (wv.len() * 2) as u32,
            );
        };
        let set_dword = |name: &str, val: u32| {
            let wn = to_w(name);
            let bytes = val.to_le_bytes();
            let _ = RegSetValueExW(h_key, wn.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4);
        };

        set_str("DisplayName", APP_NAME);
        set_str("DisplayVersion", APP_VERSION);
        set_str("Publisher", APP_PUBLISHER);
        set_str("DisplayIcon", &target_exe.to_string_lossy());
        set_str("InstallLocation", &install_dir.to_string_lossy());
        set_str(
            "UninstallString",
            &format!("\"{}\" --uninstall", uninstaller_exe.display()),
        );
        set_dword("NoModify", 1);
        set_dword("NoRepair", 1);
        let _ = RegCloseKey(h_key);
    }
}

fn ensure_medium_integrity(dir: &Path) {
    let dir_str = dir.to_string_lossy().into_owned();
    let _ = std::process::Command::new("icacls")
        .args([&dir_str, "/setintegritylevel", "Medium", "/T"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

struct InstallOpts {
    dir: PathBuf,
    desktop_shortcut: bool,
    startmenu_shortcut: bool,
    launch_after: bool,
    register_app: bool,
}

fn perform_install(opts: &InstallOpts) -> Result<PathBuf, String> {
    if AM_LIGHT_EXE_BYTES.len() < 1024 {
        return Err("安装包未内置有效 AM-Light.exe 负载。".to_string());
    }
    terminate_running_am_light();
    fs::create_dir_all(&opts.dir).map_err(|e| format!("无法创建安装目录 {}: {e}", opts.dir.display()))?;

    let target_exe = opts.dir.join("AM-Light.exe");
    let target_dll = opts.dir.join("amp_proxy.dll");
    let uninstaller_exe = opts.dir.join("Uninstall-AM-Light.exe");

    fs::write(&target_exe, AM_LIGHT_EXE_BYTES)
        .map_err(|e| format!("写入 {} 失败: {e}", target_exe.display()))?;
    fs::write(&target_dll, AMP_PROXY_DLL_BYTES)
        .map_err(|e| format!("写入 {} 失败: {e}", target_dll.display()))?;

    if let Ok(self_exe) = std::env::current_exe() {
        if self_exe != uninstaller_exe {
            let _ = fs::copy(&self_exe, &uninstaller_exe);
        }
    }

    ensure_medium_integrity(&opts.dir);

    if opts.desktop_shortcut {
        if let Some(desktop) = special_folder(CSIDL_DESKTOPDIRECTORY) {
            create_shortcut(
                &desktop.join("AM Light.lnk"),
                &target_exe,
                &opts.dir,
                "AM Light Apple Music Desktop Client",
            )?;
        }
    }
    if opts.startmenu_shortcut {
        if let Some(progs) = special_folder(CSIDL_PROGRAMS) {
            let sm_dir = progs.join("AM Light");
            create_shortcut(
                &sm_dir.join("AM Light.lnk"),
                &target_exe,
                &opts.dir,
                "AM Light Apple Music Desktop Client",
            )?;
            if uninstaller_exe.exists() {
                let _ = create_shortcut(
                    &sm_dir.join("卸载 AM Light.lnk"),
                    &uninstaller_exe,
                    &opts.dir,
                    "卸载 AM Light",
                );
            }
        }
    }
    if opts.register_app {
        write_uninstall_registry(&opts.dir, &target_exe, &uninstaller_exe);
    }

    if opts.launch_after {
        let _ = std::process::Command::new(&target_exe)
            .current_dir(&opts.dir)
            .spawn();
    }
    Ok(target_exe)
}

fn perform_uninstall(dir: &Path, clean_shortcuts: bool) -> Result<(), String> {
    terminate_running_am_light();

    if clean_shortcuts {
        if let Some(desktop) = special_folder(CSIDL_DESKTOPDIRECTORY) {
            let _ = fs::remove_file(desktop.join("AM Light.lnk"));
        }
        if let Some(progs) = special_folder(CSIDL_PROGRAMS) {
            let _ = fs::remove_dir_all(progs.join("AM Light"));
        }
        unsafe {
            let sub = to_w(UNINSTALL_KEY_SUB);
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, sub.as_ptr());
        }
    }

    for fname in ["AM-Light.exe", "amp_proxy.dll", "settings.json"] {
        let p = dir.join(fname);
        if p.exists() {
            let _ = fs::remove_file(&p);
        }
    }

    let uninst = dir.join("Uninstall-AM-Light.exe");
    let self_exe = std::env::current_exe().ok();
    if uninst.exists() && self_exe.as_ref().is_none_or(|s| s == &uninst) {
        let cmd = format!(
            "ping 127.0.0.1 -n 2 >nul & del /f /q \"{}\" & rmdir \"{}\" >nul 2>nul",
            uninst.display(),
            dir.display()
        );
        let _ = std::process::Command::new("cmd.exe")
            .args(["/c", &cmd])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    } else if uninst.exists() {
        let _ = fs::remove_file(&uninst);
        let _ = fs::remove_dir(dir);
    } else {
        let _ = fs::remove_dir(dir);
    }
    Ok(())
}

fn read_control_text(hwnd: *mut c_void) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len as usize) + 1];
        let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got.max(0) as usize]).trim().to_string()
    }
}

fn dpi_scale() -> f32 {
    unsafe {
        let d = GetDpiForSystem();
        if d > 0 {
            (d as f32 / 96.0).max(1.0)
        } else {
            1.0
        }
    }
}

unsafe fn set_font_all(hwnd: *mut c_void, hfont: *mut c_void) {
    SendMessageW(hwnd, WM_SETFONT, hfont as usize, 1);
}

unsafe extern "system" fn setup_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    w_param: usize,
    l_param: isize,
) -> isize {
    match msg {
        WM_CREATE => {
            let sc = dpi_scale();
            let px = |v: i32| -> i32 { ((v as f32) * sc).round() as i32 };
            let hinst = GetModuleHandleW(null());

            let face = to_w("Microsoft YaHei UI");
            let f_title = CreateFontW(
                -px(20), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr(),
            );
            let f_body = CreateFontW(
                -px(13), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr(),
            );
            H_FONT_TITLE.store(f_title as isize, Ordering::Release);
            H_FONT_BODY.store(f_body as isize, Ordering::Release);

            let cls_static = to_w("STATIC");
            let cls_edit = to_w("EDIT");
            let cls_btn = to_w("BUTTON");

            let h_title = CreateWindowExW(
                0,
                cls_static.as_ptr(),
                to_w("AM Light 安装向导").as_ptr(),
                WS_CHILD | WS_VISIBLE | SS_LEFT,
                px(24), px(18), px(440), px(28),
                hwnd, null_mut(), hinst, null_mut(),
            );
            set_font_all(h_title, f_title);

            let h_sub = CreateWindowExW(
                0,
                cls_static.as_ptr(),
                to_w(&format!("版本 {} · 原生 Direct2D Apple Music 桌面客户端", APP_VERSION)).as_ptr(),
                WS_CHILD | WS_VISIBLE | SS_LEFT,
                px(24), px(50), px(450), px(20),
                hwnd, null_mut(), hinst, null_mut(),
            );
            set_font_all(h_sub, f_body);

            let h_dir_lbl = CreateWindowExW(
                0,
                cls_static.as_ptr(),
                to_w("安装目录:").as_ptr(),
                WS_CHILD | WS_VISIBLE | SS_LEFT,
                px(24), px(88), px(120), px(20),
                hwnd, null_mut(), hinst, null_mut(),
            );
            set_font_all(h_dir_lbl, f_body);

            let def_dir = default_install_dir();
            let h_edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                cls_edit.as_ptr(),
                to_w(&def_dir.to_string_lossy()).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL,
                px(24), px(112), px(352), px(26),
                hwnd, IDC_DIR_EDIT as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_edit, f_body);
            H_DIR_EDIT.store(h_edit as isize, Ordering::Release);

            let h_browse = CreateWindowExW(
                0,
                cls_btn.as_ptr(),
                to_w("浏览...").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                px(384), px(111), px(88), px(28),
                hwnd, IDC_BROWSE_BTN as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_browse, f_body);

            let h_chk_d = CreateWindowExW(
                0,
                cls_btn.as_ptr(),
                to_w("创建桌面快捷方式").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                px(24), px(152), px(220), px(22),
                hwnd, IDC_CHK_DESKTOP as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_chk_d, f_body);
            SendMessageW(h_chk_d, BM_SETCHECK, BST_CHECKED, 0);
            H_CHK_DESKTOP.store(h_chk_d as isize, Ordering::Release);

            let h_chk_s = CreateWindowExW(
                0,
                cls_btn.as_ptr(),
                to_w("创建开始菜单快捷方式").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                px(24), px(180), px(220), px(22),
                hwnd, IDC_CHK_STARTMENU as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_chk_s, f_body);
            SendMessageW(h_chk_s, BM_SETCHECK, BST_CHECKED, 0);
            H_CHK_STARTMENU.store(h_chk_s as isize, Ordering::Release);

            let h_chk_l = CreateWindowExW(
                0,
                cls_btn.as_ptr(),
                to_w("安装完成后立即启动 AM Light").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                px(24), px(208), px(260), px(22),
                hwnd, IDC_CHK_LAUNCH as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_chk_l, f_body);
            SendMessageW(h_chk_l, BM_SETCHECK, BST_CHECKED, 0);
            H_CHK_LAUNCH.store(h_chk_l as isize, Ordering::Release);

            let h_stat = CreateWindowExW(
                0,
                cls_static.as_ptr(),
                to_w("准备就绪，将安装到用户本地目录（无需管理员权限）。").as_ptr(),
                WS_CHILD | WS_VISIBLE | SS_LEFT,
                px(24), px(244), px(448), px(20),
                hwnd, IDC_STATUS_LBL as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_stat, f_body);
            H_STATUS_LBL.store(h_stat as isize, Ordering::Release);

            let h_inst = CreateWindowExW(
                0,
                cls_btn.as_ptr(),
                to_w("立即安装").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
                px(272), px(274), px(96), px(32),
                hwnd, IDC_INSTALL_BTN as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_inst, f_body);
            H_INSTALL_BTN.store(h_inst as isize, Ordering::Release);

            let h_cancel = CreateWindowExW(
                0,
                cls_btn.as_ptr(),
                to_w("取消").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                px(376), px(274), px(96), px(32),
                hwnd, IDC_CANCEL_BTN as *mut c_void, hinst, null_mut(),
            );
            set_font_all(h_cancel, f_body);
            0
        }

        WM_COMMAND => {
            let id = w_param & 0xFFFF;
            match id {
                IDC_BROWSE_BTN => {
                    let mut disp = [0u16; MAX_PATH_W];
                    let title = to_w("选择 AM Light 安装目录");
                    let mut bi = BROWSEINFOW {
                        hwnd_owner: hwnd,
                        pidl_root: null(),
                        psz_display_name: disp.as_mut_ptr(),
                        lpsz_title: title.as_ptr(),
                        ul_flags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
                        lpfn: null(),
                        l_param: 0,
                        i_image: 0,
                    };
                    let pidl = SHBrowseForFolderW(&mut bi);
                    if !pidl.is_null() {
                        let mut chosen = [0u16; MAX_PATH_W];
                        if SHGetPathFromIDListW(pidl, chosen.as_mut_ptr()) != 0 {
                            let end = chosen.iter().position(|&c| c == 0).unwrap_or(chosen.len());
                            let base = String::from_utf16_lossy(&chosen[..end]);
                            let p = PathBuf::from(&base);
                            let final_p = if p
                                .file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.eq_ignore_ascii_case("AM-Light"))
                            {
                                p
                            } else {
                                p.join("AM-Light")
                            };
                            let h_edit = H_DIR_EDIT.load(Ordering::Acquire) as *mut c_void;
                            if !h_edit.is_null() {
                                let wt = to_w(&final_p.to_string_lossy());
                                let _ = SetWindowTextW(h_edit, wt.as_ptr());
                            }
                        }
                        CoTaskMemFree(pidl);
                    }
                    0
                }
                IDC_INSTALL_BTN => {
                    let h_edit = H_DIR_EDIT.load(Ordering::Acquire) as *mut c_void;
                    let h_chk_d = H_CHK_DESKTOP.load(Ordering::Acquire) as *mut c_void;
                    let h_chk_s = H_CHK_STARTMENU.load(Ordering::Acquire) as *mut c_void;
                    let h_chk_l = H_CHK_LAUNCH.load(Ordering::Acquire) as *mut c_void;
                    let h_stat = H_STATUS_LBL.load(Ordering::Acquire) as *mut c_void;
                    let h_btn = H_INSTALL_BTN.load(Ordering::Acquire) as *mut c_void;

                    let dir_txt = read_control_text(h_edit);
                    let dir = if dir_txt.is_empty() {
                        default_install_dir()
                    } else {
                        PathBuf::from(dir_txt)
                    };
                    let opts = InstallOpts {
                        dir,
                        desktop_shortcut: SendMessageW(h_chk_d, BM_GETCHECK, 0, 0)
                            == BST_CHECKED as isize,
                        startmenu_shortcut: SendMessageW(h_chk_s, BM_GETCHECK, 0, 0)
                            == BST_CHECKED as isize,
                        launch_after: SendMessageW(h_chk_l, BM_GETCHECK, 0, 0)
                            == BST_CHECKED as isize,
                        register_app: true,
                    };

                    let _ = EnableWindow(h_btn, 0);
                    let _ = SetWindowTextW(h_stat, to_w("正在复制文件与创建快捷方式...").as_ptr());
                    UpdateWindow(hwnd);

                    match perform_install(&opts) {
                        Ok(_) => {
                            let _ = SetWindowTextW(h_stat, to_w("安装成功！").as_ptr());
                            MessageBoxW(
                                hwnd,
                                to_w(&format!(
                                    "AM Light v{} 已成功安装到：\n{}",
                                    APP_VERSION,
                                    opts.dir.display()
                                ))
                                .as_ptr(),
                                to_w("安装完成").as_ptr(),
                                MB_OK | MB_ICONINFORMATION,
                            );
                            let _ = DestroyWindow(hwnd);
                        }
                        Err(e) => {
                            let _ = EnableWindow(h_btn, 1);
                            let _ = SetWindowTextW(h_stat, to_w("安装失败，请查看提示。").as_ptr());
                            MessageBoxW(
                                hwnd,
                                to_w(&e).as_ptr(),
                                to_w("安装失败").as_ptr(),
                                MB_OK | MB_ICONERROR,
                            );
                        }
                    }
                    0
                }
                IDC_CANCEL_BTN => {
                    let _ = DestroyWindow(hwnd);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, w_param, l_param),
            }
        }

        WM_DESTROY => {
            let ft = H_FONT_TITLE.swap(0, Ordering::AcqRel) as *mut c_void;
            if !ft.is_null() {
                let _ = DeleteObject(ft);
            }
            let fb = H_FONT_BODY.swap(0, Ordering::AcqRel) as *mut c_void;
            if !fb.is_null() {
                let _ = DeleteObject(fb);
            }
            PostQuitMessage(0);
            0
        }

        _ => DefWindowProcW(hwnd, msg, w_param, l_param),
    }
}

fn run_gui_installer() {
    unsafe {
        let _ = CoInitializeEx(null_mut(), 2);
        let icc = INITCOMMONCONTROLSEX {
            dw_size: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dw_icc: 0x0000_0008 | 0x0000_4000, // ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES
        };
        let _ = InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(null());
        let cls_name = to_w("AMLightInstallerWnd");
        let h_icon = LoadIconW(hinst, 1usize as *const u16);
        let h_cursor = LoadCursorW(null_mut(), 32512usize as *const u16);

        let wc = WNDCLASSEXW {
            cb_size: size_of::<WNDCLASSEXW>() as u32,
            style: 0x0003,
            lpfn_wnd_proc: setup_wnd_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: hinst,
            h_icon,
            h_cursor,
            hbr_background: (5 /* COLOR_WINDOW */ + 1) as isize as *mut c_void,
            lpsz_menu_name: null(),
            lpsz_class_name: cls_name.as_ptr(),
            h_icon_sm: h_icon,
        };
        if RegisterClassExW(&wc) == 0 {
            return;
        }

        let sc = dpi_scale();
        let win_w = (508.0 * sc).round() as i32;
        let win_h = (358.0 * sc).round() as i32;
        let scr_w = GetSystemMetrics(SM_CXSCREEN);
        let scr_h = GetSystemMetrics(SM_CYSCREEN);
        let pos_x = ((scr_w - win_w) / 2).max(20);
        let pos_y = ((scr_h - win_h) / 2).max(20);

        let style = WS_OVERLAPPEDWINDOW & !(WS_MAXIMIZEBOX | WS_THICKFRAME);
        let hwnd = CreateWindowExW(
            0,
            cls_name.as_ptr(),
            to_w(&format!("AM Light v{} 安装程序", APP_VERSION)).as_ptr(),
            style,
            pos_x,
            pos_y,
            win_w,
            win_h,
            null_mut(),
            null_mut(),
            hinst,
            null_mut(),
        );
        if hwnd.is_null() {
            return;
        }
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = UpdateWindow(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        CoUninitialize();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let invoked_as_uninstaller = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
        .is_some_and(|n| n.contains("uninstall"));

    let mut silent = false;
    let mut uninstall = invoked_as_uninstaller;
    let mut no_shortcuts = false;
    let mut no_launch = false;
    let mut dir_override: Option<PathBuf> = None;

    let mut i = 1usize;
    while i < args.len() {
        let a = args[i].as_str();
        if a.eq_ignore_ascii_case("--silent") || a.eq_ignore_ascii_case("/S") {
            silent = true;
        } else if a.eq_ignore_ascii_case("--uninstall") {
            uninstall = true;
        } else if a.eq_ignore_ascii_case("--no-shortcuts") {
            no_shortcuts = true;
        } else if a.eq_ignore_ascii_case("--no-launch") {
            no_launch = true;
        } else if let Some(d) = a.strip_prefix("--dir=") {
            dir_override = Some(PathBuf::from(d));
        } else if a.eq_ignore_ascii_case("--dir") && i + 1 < args.len() {
            dir_override = Some(PathBuf::from(&args[i + 1]));
            i += 1;
        }
        i += 1;
    }

    if uninstall {
        let target_dir = dir_override.unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .unwrap_or_else(default_install_dir)
        });
        if !silent {
            let ans = unsafe {
                MessageBoxW(
                    null_mut(),
                    to_w(&format!("确定要卸载 AM Light 并移除以下目录吗？\n{}", target_dir.display()))
                        .as_ptr(),
                    to_w("卸载 AM Light").as_ptr(),
                    MB_YESNO | MB_ICONQUESTION,
                )
            };
            if ans != IDYES {
                return;
            }
        }
        match perform_uninstall(&target_dir, !no_shortcuts) {
            Ok(()) if !silent => unsafe {
                MessageBoxW(
                    null_mut(),
                    to_w("AM Light 已卸载完成。").as_ptr(),
                    to_w("卸载完成").as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
            },
            Err(e) if !silent => unsafe {
                MessageBoxW(
                    null_mut(),
                    to_w(&e).as_ptr(),
                    to_w("卸载失败").as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            },
            _ => {}
        }
        return;
    }

    if silent {
        let opts = InstallOpts {
            dir: dir_override.unwrap_or_else(default_install_dir),
            desktop_shortcut: !no_shortcuts,
            startmenu_shortcut: !no_shortcuts,
            launch_after: !no_launch,
            register_app: !no_shortcuts,
        };
        if perform_install(&opts).is_err() {
            std::process::exit(1);
        }
        return;
    }

    run_gui_installer();
}
