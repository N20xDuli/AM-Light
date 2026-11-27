use once_cell::sync::Lazy;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

static INIT_ONCE: Once = Once::new();
static LOG_SINK: Lazy<Mutex<LoggerSink>> = Lazy::new(|| Mutex::new(LoggerSink::new()));
static LOG_DIR_CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
static CRASH_LOG_CHECKED: AtomicBool = AtomicBool::new(false);
static HAS_CRASH_LOG_CACHE: AtomicBool = AtomicBool::new(false);

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const LOG_BUF_CAPACITY: usize = 16 * 1024;
const LOG_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

struct LoggerSink {
    writer: Option<BufWriter<File>>,
    path: Option<PathBuf>,
    bytes_written: u64,
    last_flush: Instant,
    dirty: bool,
}

impl LoggerSink {
    fn new() -> Self {
        Self {
            writer: None,
            path: None,
            bytes_written: 0,
            last_flush: Instant::now(),
            dirty: false,
        }
    }

    fn close_writer(&mut self) {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        self.path = None;
        self.dirty = false;
    }

    fn ensure_open(&mut self, path: &Path) -> bool {
        if self.writer.is_some() && self.path.as_deref() == Some(path) {
            return true;
        }
        self.close_writer();
        let initial_len = rotate_if_large_closed(path);
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(f) => {
                self.writer = Some(BufWriter::with_capacity(LOG_BUF_CAPACITY, f));
                self.path = Some(path.to_path_buf());
                self.bytes_written = initial_len;
                self.last_flush = Instant::now();
                self.dirty = false;
                true
            }
            Err(_) => false,
        }
    }

    fn rotate_if_needed(&mut self, path: &Path, incoming_len: usize) {
        if self.bytes_written.saturating_add(incoming_len as u64) >= MAX_LOG_BYTES {
            // Release open Windows file handle before renaming to prevent sharing violations.
            self.close_writer();
            let prev = path.with_file_name("amlight.prev.log");
            let _ = fs::remove_file(&prev);
            let _ = fs::rename(path, &prev);
            self.bytes_written = 0;
        }
    }

    fn write_bytes(&mut self, path: &Path, data: &[u8], force_flush: bool, sync_disk: bool) {
        self.rotate_if_needed(path, data.len());
        if !self.ensure_open(path) {
            return;
        }
        if let Some(ref mut w) = self.writer {
            if w.write_all(data).is_ok() {
                self.bytes_written = self.bytes_written.saturating_add(data.len() as u64);
                self.dirty = true;
                let periodic_due = self.last_flush.elapsed() >= LOG_FLUSH_INTERVAL;
                if force_flush || sync_disk || periodic_due {
                    let _ = w.flush();
                    if sync_disk {
                        let _ = w.get_ref().sync_all();
                    }
                    self.last_flush = Instant::now();
                    self.dirty = false;
                }
            }
        }
    }

    fn flush_now(&mut self, sync_disk: bool) {
        if let Some(ref mut w) = self.writer {
            if self.dirty || sync_disk {
                let _ = w.flush();
                if sync_disk {
                    let _ = w.get_ref().sync_all();
                }
                self.last_flush = Instant::now();
                self.dirty = false;
            }
        }
    }
}

pub fn log_dir() -> Option<PathBuf> {
    LOG_DIR_CACHE
        .get_or_init(|| {
            let base = std::env::var_os("APPDATA")
                .or_else(|| std::env::var_os("LOCALAPPDATA"))
                .map(PathBuf::from)?;
            let dir = base.join("AMLight").join("logs");
            let _ = fs::create_dir_all(&dir);
            Some(dir)
        })
        .clone()
}

pub fn log_file_path() -> Option<PathBuf> {
    Some(log_dir()?.join("amlight.log"))
}

pub fn crash_log_path() -> Option<PathBuf> {
    Some(log_dir()?.join("crash_latest.log"))
}

pub fn refresh_crash_log_cache() -> bool {
    let exists = crash_log_path()
        .and_then(|p| fs::metadata(p).ok())
        .is_some_and(|m| m.len() > 0);
    HAS_CRASH_LOG_CACHE.store(exists, Ordering::Release);
    CRASH_LOG_CHECKED.store(true, Ordering::Release);
    exists
}

pub fn has_recent_crash_log() -> bool {
    if CRASH_LOG_CHECKED.load(Ordering::Acquire) {
        return HAS_CRASH_LOG_CACHE.load(Ordering::Acquire);
    }
    refresh_crash_log_cache()
}

#[cfg(target_os = "windows")]
fn format_local_timestamp() -> String {
    unsafe {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        let st = GetLocalTime();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
}

#[cfg(not(target_os = "windows"))]
fn format_local_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

#[cfg(target_os = "windows")]
fn current_tid() -> u32 {
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(not(target_os = "windows"))]
fn current_tid() -> u32 {
    0
}

fn rotate_if_large_closed(path: &Path) -> u64 {
    if let Ok(meta) = fs::metadata(path) {
        let len = meta.len();
        if len >= MAX_LOG_BYTES {
            let prev = path.with_file_name("amlight.prev.log");
            let _ = fs::remove_file(&prev);
            let _ = fs::rename(path, prev);
            return 0;
        }
        return len;
    }
    0
}

fn append_line(level: &str, tag: &str, msg: &str, force_flush: bool, sync_disk: bool) {
    let Some(path) = log_file_path() else {
        return;
    };
    let line = format!(
        "[{}] [PID:{} TID:{}] [{}] [{}] {}\r\n",
        format_local_timestamp(),
        std::process::id(),
        current_tid(),
        level,
        tag,
        msg
    );
    if let Ok(mut sink) = LOG_SINK.lock() {
        sink.write_bytes(&path, line.as_bytes(), force_flush, sync_disk);
    }
}

fn write_crash_report(report: &str) {
    if let Some(p) = crash_log_path() {
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&p)
        {
            let _ = f.write_all(report.as_bytes());
            let _ = f.flush();
            let _ = f.sync_all();
            HAS_CRASH_LOG_CACHE.store(true, Ordering::Release);
            CRASH_LOG_CHECKED.store(true, Ordering::Release);
        }
    }
    if let Some(p) = log_file_path() {
        match LOG_SINK.try_lock() {
            Ok(mut sink) => {
                sink.write_bytes(&p, report.as_bytes(), true, true);
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let mut sink = poisoned.into_inner();
                sink.write_bytes(&p, report.as_bytes(), true, true);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&p) {
                    let _ = f.write_all(report.as_bytes());
                    let _ = f.flush();
                    let _ = f.sync_all();
                }
            }
        }
    }
}

pub fn info(tag: &str, msg: &str) {
    let flush_on_lifecycle = matches!(tag, "boot" | "window" | "exit");
    append_line("INFO", tag, msg, flush_on_lifecycle, false);
}

pub fn warn(tag: &str, msg: &str) {
    append_line("WARN", tag, msg, true, false);
}

pub fn error(tag: &str, msg: &str) {
    append_line("ERROR", tag, msg, true, true);
}

/// Flushes any buffered log lines held in the persistent `BufWriter<File>`.
pub fn flush() {
    if let Ok(mut sink) = LOG_SINK.lock() {
        sink.flush_now(false);
    }
}

/// Returns `true` when the persistent buffered log file handle is currently open.
pub fn is_persistent_writer_open() -> bool {
    LOG_SINK
        .lock()
        .map(|sink| sink.writer.is_some())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn unhandled_exception_filter(
    info: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::Diagnostics::Debug::EXCEPTION_CONTINUE_SEARCH;
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };

    let (code, addr, rip, rsp) = if !info.is_null() {
        let rec = (*info).ExceptionRecord;
        let ctx = (*info).ContextRecord;
        let (c, a) = if !rec.is_null() {
            ((*rec).ExceptionCode.0 as u32, (*rec).ExceptionAddress as usize)
        } else {
            (0, 0)
        };
        #[cfg(target_arch = "x86_64")]
        let (r_ip, r_sp) = if !ctx.is_null() {
            ((*ctx).Rip as usize, (*ctx).Rsp as usize)
        } else {
            (0, 0)
        };
        #[cfg(not(target_arch = "x86_64"))]
        let (r_ip, r_sp) = (0usize, 0usize);
        (c, a, r_ip, r_sp)
    } else {
        (0, 0, 0, 0)
    };

    let mut mod_desc = format!("0x{addr:016X}");
    if addr != 0 {
        let mut hmod = HMODULE::default();
        let flags =
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        if GetModuleHandleExW(flags, PCWSTR(addr as *const u16), &mut hmod).is_ok()
            && !hmod.0.is_null()
        {
            let mut buf = [0u16; 260];
            let len = GetModuleFileNameW(Some(hmod), &mut buf) as usize;
            let full = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
            let base_name = std::path::Path::new(&full)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let rva = addr.saturating_sub(hmod.0 as usize);
            mod_desc = format!("{base_name}+0x{rva:X}");
        }
    }

    let bt = std::backtrace::Backtrace::force_capture();
    let report = format!(
        "================ AM LIGHT FATAL EXCEPTION ================\r\n\
         Time    : {}\r\n\
         Version : v{}\r\n\
         PID/TID : {} / {}\r\n\
         Code    : 0x{:08X}\r\n\
         Fault   : {} (RIP=0x{:016X}, RSP=0x{:016X})\r\n\
         Backtrace:\r\n{}\r\n\
         ==========================================================\r\n",
        format_local_timestamp(),
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        current_tid(),
        code,
        mod_desc,
        rip,
        rsp,
        bt
    );
    write_crash_report(&report);
    EXCEPTION_CONTINUE_SEARCH
}

/// Initializes rolling file logger, Rust panic hook (persisting panic location & backtrace before
/// `panic = "abort"` terminates), and Win32 `SetUnhandledExceptionFilter`.
pub fn init() {
    INIT_ONCE.call_once(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let thread = std::thread::current();
            let tname = thread.name().unwrap_or("<unnamed>");
            let loc = panic_info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let payload = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "<non-string panic payload>".to_string()
            };
            let bt = std::backtrace::Backtrace::force_capture();
            let report = format!(
                "================ AM LIGHT RUST PANIC ================\r\n\
                 Time     : {}\r\n\
                 Version  : v{}\r\n\
                 PID/TID  : {} / {} ({})\r\n\
                 Location : {}\r\n\
                 Message  : {}\r\n\
                 Backtrace:\r\n{}\r\n\
                 =====================================================\r\n",
                format_local_timestamp(),
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
                current_tid(),
                tname,
                loc,
                payload,
                bt
            );
            write_crash_report(&report);
            prev_hook(panic_info);
        }));

        #[cfg(target_os = "windows")]
        unsafe {
            windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(
                unhandled_exception_filter,
            ));
        }

        info(
            "boot",
            &format!("AM Light v{} logger initialized", env!("CARGO_PKG_VERSION")),
        );
    });
}

pub fn open_logs_folder() -> Result<(), String> {
    let dir = log_dir().ok_or_else(|| "无法定位本地日志目录".to_string())?;
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let op = HSTRING::from("open");
        let target = HSTRING::from(dir.to_string_lossy().as_ref());
        let _ = ShellExecuteW(None, &op, &target, None, None, SW_SHOWNORMAL);
    }
    Ok(())
}
