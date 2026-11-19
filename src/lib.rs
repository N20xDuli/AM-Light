pub mod am_agent;
pub mod am_api;
pub mod audio_viz;
pub mod binaural_dsp;
pub mod bplist;
pub mod downloads;
pub mod drm_fix;
pub mod lyrics;
pub mod smtc;
pub mod theme;

#[cfg(target_os = "windows")]
pub mod app_ui;
#[cfg(target_os = "windows")]
pub mod d2d_renderer;
#[cfg(target_os = "windows")]
pub mod lyrics_view;
#[cfg(target_os = "windows")]
pub mod visualizers;

#[cfg(target_os = "windows")]
mod win_app {
    use crate::am_agent;
    use crate::app_ui::{AppUi, PageRoute, UiAction};
    use crate::audio_viz::{self, WM_APP_AUDIO_WAKE};
    use crate::binaural_dsp;
    use crate::d2d_renderer::D2DRenderer;

    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use windows::core::{PCWSTR, w};
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    use windows::Win32::Graphics::Gdi::{
        EnumDisplaySettingsW, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, ScreenToClient,
        ValidateRect, DEVMODEW, ENUM_CURRENT_SETTINGS, MONITORINFO, MONITORINFOEXW,
        MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{
        CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject,
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
    use windows::Win32::UI::Input::Ime::{
        ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow, CFS_POINT, COMPOSITIONFORM,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TME_NONCLIENT,
        TRACKMOUSEEVENT, VK_CONTROL, VK_ESCAPE, VK_OEM_COMMA,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetCursorPos,
        GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, IsIconic,
        IsZoomed, KillTimer, LoadCursorW, LoadIconW, MINMAXINFO, MSG, NCCALCSIZE_PARAMS,
        PeekMessageW, PostQuitMessage, RegisterClassExW, SetCursor, SetTimer, SetWindowLongPtrW,
        SetWindowPos, ShowWindow, TranslateMessage, GWL_EXSTYLE, HTBOTTOM, HTBOTTOMLEFT,
        HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
        IDC_ARROW, IDC_HAND, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNORMAL, WM_ACTIVATE,
        WM_CHAR, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_ERASEBKGND,
        WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCMOUSELEAVE,
        WM_NCMOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_SIZE, WM_SIZING, WM_TIMER, WM_WINDOWPOSCHANGED,
        WNDCLASSEXW, WS_EX_NOREDIRECTIONBITMAP, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP,
        WS_SYSMENU, WS_THICKFRAME,
    };
    const WM_MOUSELEAVE_MSG: u32 = 0x02A3;

    const SIZEMOVE_TIMER_ID: usize = 0xA117;

    struct WindowContext {
        renderer: D2DRenderer,
        ui: AppUi,
        dirty: bool,
        in_sizemove: bool,
        last_modal_render: Instant,
        high_surrogate: Option<u16>,
        cached_hmon: isize,
        last_mon_poll: Instant,
        monitor_hz: u32,
        force_mon_refresh: bool,
    }

    thread_local! {
        static WIN_CTX: RefCell<Option<WindowContext>> = const { RefCell::new(None) };
    }

    fn dpi_scale_for_hwnd(hwnd: HWND) -> f32 {
        unsafe {
            let dpi = GetDpiForWindow(hwnd);
            if dpi > 0 {
                (dpi as f32) / 96.0
            } else {
                1.0
            }
        }
    }

    unsafe fn query_window_monitor_hz(
        hwnd: HWND,
        cached_mon: &mut isize,
        last_query: &mut Instant,
        cur_hz: &mut u32,
        force: bool,
    ) {
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let hmon_val = hmon.0 as isize;
        if !force && hmon_val == *cached_mon && last_query.elapsed() < Duration::from_secs(2) {
            return;
        }
        *cached_mon = hmon_val;
        *last_query = Instant::now();
        let mut mi_ex = MONITORINFOEXW::default();
        mi_ex.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi_ex as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            let mut dm = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            if EnumDisplaySettingsW(
                PCWSTR(mi_ex.szDevice.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut dm,
            )
            .as_bool()
                && (30..=500).contains(&dm.dmDisplayFrequency)
            {
                *cur_hz = dm.dmDisplayFrequency;
            }
        }
    }

    unsafe fn sync_window_geometry_if_needed(hwnd: HWND, ctx: &mut WindowContext) -> bool {
        if IsIconic(hwnd).as_bool() {
            return false;
        }
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_err() {
            return false;
        }
        let w_px = (rc.right - rc.left).max(0) as u32;
        let h_px = (rc.bottom - rc.top).max(0) as u32;
        if w_px == 0 || h_px == 0 {
            return false;
        }
        let maximized = IsZoomed(hwnd).as_bool();
        let scale = dpi_scale_for_hwnd(hwnd);
        let changed = w_px != ctx.renderer.width_px
            || h_px != ctx.renderer.height_px
            || (scale - ctx.renderer.dpi_scale).abs() > 0.005
            || maximized != ctx.ui.is_window_maximized;
        if changed {
            ctx.ui.is_window_maximized = maximized;
            ctx.renderer.resize(w_px, h_px, scale);
            ctx.ui.lyrics_view.invalidate_layout_cache();
            ctx.dirty = true;
        }
        changed
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => LRESULT(1),

            // Remove OS titlebar & non-client frame completely for custom rounded Liquid Glass window!
            WM_NCCALCSIZE => {
                if wparam.0 != 0 {
                    if IsZoomed(hwnd).as_bool() {
                        let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
                        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                        let mut mi = MONITORINFO {
                            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                            ..Default::default()
                        };
                        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                            params.rgrc[0] = mi.rcWork;
                        }
                    }
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // Custom frameless resize borders + Immersive Top Drag Bar (`HTCAPTION` outside buttons!)
            WM_NCHITTEST => {
                let sx = (lparam.0 as i16) as i32;
                let sy = ((lparam.0 >> 16) as i16) as i32;
                let mut pt = POINT { x: sx, y: sy };
                let _ = ScreenToClient(hwnd, &mut pt);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let w_px = (rc.right - rc.left).max(1);
                let h_px = (rc.bottom - rc.top).max(1);
                if pt.x < 0 || pt.y < 0 || pt.x >= w_px || pt.y >= h_px {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }

                let scale = dpi_scale_for_hwnd(hwnd);
                let lx = (pt.x as f32) / scale;
                let ly = (pt.y as f32) / scale;
                let win_w = (w_px as f32) / scale;
                let win_h = (h_px as f32) / scale;

                if !IsZoomed(hwnd).as_bool() {
                    let edge = 6.0f32;
                    let corner = 12.0f32;
                    let on_l = lx < edge;
                    let on_r = lx >= win_w - edge;
                    let on_t = ly < edge;
                    let on_b = ly >= win_h - edge;

                    if (on_t && lx < corner) || (on_l && ly < corner) {
                        return LRESULT(HTTOPLEFT as isize);
                    }
                    if (on_t && lx >= win_w - corner) || (on_r && ly < corner) {
                        return LRESULT(HTTOPRIGHT as isize);
                    }
                    if (on_b && lx < corner) || (on_l && ly >= win_h - corner) {
                        return LRESULT(HTBOTTOMLEFT as isize);
                    }
                    if (on_b && lx >= win_w - corner) || (on_r && ly >= win_h - corner) {
                        return LRESULT(HTBOTTOMRIGHT as isize);
                    }
                    if on_l {
                        return LRESULT(HTLEFT as isize);
                    }
                    if on_r {
                        return LRESULT(HTRIGHT as isize);
                    }
                    if on_t {
                        return LRESULT(HTTOP as isize);
                    }
                    if on_b {
                        return LRESULT(HTBOTTOM as isize);
                    }
                }

                let mut interactive = false;
                WIN_CTX.with(|cell| {
                    if let Ok(borrow) = cell.try_borrow() {
                        if let Some(ctx) = borrow.as_ref() {
                            interactive = ctx.ui.is_interactive_at(lx, ly);
                        }
                    }
                });

                if interactive {
                    return LRESULT(HTCLIENT as isize);
                }
                if ly < crate::app_ui::TOP_BAR_GUARD_H {
                    return LRESULT(HTCAPTION as isize);
                }
                LRESULT(HTCLIENT as isize)
            }

            WM_NCMOUSEMOVE => {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                let sx = (lparam.0 as i16) as i32;
                let sy = ((lparam.0 >> 16) as i16) as i32;
                let mut pt = POINT { x: sx, y: sy };
                let _ = ScreenToClient(hwnd, &mut pt);
                let scale = dpi_scale_for_hwnd(hwnd);
                let lx = (pt.x as f32) / scale;
                let ly = (pt.y as f32) / scale;
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            if ((ctx.ui.mouse_pos.0 - lx).abs() > 0.5
                                || (ctx.ui.mouse_pos.1 - ly).abs() > 0.5)
                                && ctx.ui.on_mouse_move(lx, ly)
                            {
                                ctx.dirty = true;
                            }
                        }
                    }
                });
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_MOUSELEAVE_MSG | WM_NCMOUSELEAVE => {
                let mut pt = POINT::default();
                let mut rc = RECT::default();
                let still_inside = GetCursorPos(&mut pt).is_ok()
                    && ScreenToClient(hwnd, &mut pt).as_bool()
                    && GetClientRect(hwnd, &mut rc).is_ok()
                    && pt.x >= rc.left
                    && pt.x < rc.right
                    && pt.y >= rc.top
                    && pt.y < rc.bottom;
                if !still_inside {
                    WIN_CTX.with(|cell| {
                        if let Ok(mut borrow) = cell.try_borrow_mut() {
                            if let Some(ctx) = borrow.as_mut() {
                                if ctx.ui.mouse_pos.0 >= 0.0
                                    && ctx.ui.on_mouse_move(-1000.0, -1000.0)
                                {
                                    ctx.dirty = true;
                                }
                            }
                        }
                    });
                }
                LRESULT(0)
            }

            WM_SETCURSOR => {
                let hit = (lparam.0 & 0xFFFF) as u32;
                if hit == HTCLIENT {
                    let mut over_btn = false;
                    WIN_CTX.with(|cell| {
                        if let Ok(borrow) = cell.try_borrow() {
                            if let Some(ctx) = borrow.as_ref() {
                                let (lx, ly) = ctx.ui.mouse_pos;
                                over_btn = ctx.ui.is_interactive_at(lx, ly);
                            }
                        }
                    });
                    let cur_id = if over_btn { IDC_HAND } else { IDC_ARROW };
                    if let Ok(cur) = LoadCursorW(None, cur_id) {
                        let _ = SetCursor(Some(cur));
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_GETMINMAXINFO => {
                let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
                let scale = dpi_scale_for_hwnd(hwnd);
                mmi.ptMinTrackSize.x = (780.0 * scale) as i32;
                mmi.ptMinTrackSize.y = (540.0 * scale) as i32;
                let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                    mmi.ptMaxPosition.x = mi.rcWork.left - mi.rcMonitor.left;
                    mmi.ptMaxPosition.y = mi.rcWork.top - mi.rcMonitor.top;
                    mmi.ptMaxSize.x = (mi.rcWork.right - mi.rcWork.left).max(mmi.ptMinTrackSize.x);
                    mmi.ptMaxSize.y = (mi.rcWork.bottom - mi.rcWork.top).max(mmi.ptMinTrackSize.y);
                }
                LRESULT(0)
            }

            WM_ENTERSIZEMOVE => {
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.in_sizemove = true;
                            ctx.last_modal_render = Instant::now();
                        }
                    }
                });
                let _ = SetTimer(Some(hwnd), SIZEMOVE_TIMER_ID, 15, None);
                LRESULT(0)
            }

            WM_EXITSIZEMOVE => {
                let _ = KillTimer(Some(hwnd), SIZEMOVE_TIMER_ID);
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.in_sizemove = false;
                            ctx.force_mon_refresh = true;
                            let _ = sync_window_geometry_if_needed(hwnd, ctx);
                            ctx.dirty = true;
                        }
                    }
                });
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }

            WM_SIZING => {
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(1)
            }

            WM_TIMER => {
                if wparam.0 == SIZEMOVE_TIMER_ID && !IsIconic(hwnd).as_bool() {
                    WIN_CTX.with(|cell| {
                        if let Ok(mut borrow) = cell.try_borrow_mut() {
                            if let Some(ctx) = borrow.as_mut() {
                                if ctx.in_sizemove {
                                    let _ = sync_window_geometry_if_needed(hwnd, ctx);
                                    let _ = ctx.ui.drain_messages(&mut ctx.renderer);
                                    if (ctx.dirty || ctx.ui.needs_continuous_animation())
                                        && ctx.last_modal_render.elapsed()
                                            >= std::time::Duration::from_millis(12)
                                    {
                                        let dt = ctx
                                            .last_modal_render
                                            .elapsed()
                                            .as_secs_f32()
                                            .clamp(0.002, 0.05);
                                        ctx.last_modal_render = Instant::now();
                                        ctx.ui.step_and_render_ex(&mut ctx.renderer, dt, true);
                                        ctx.dirty = false;
                                    }
                                }
                            }
                        }
                    });
                }
                LRESULT(0)
            }

            WM_SIZE => {
                let w_px = (lparam.0 as u32) & 0xFFFF;
                let h_px = ((lparam.0 as u32) >> 16) & 0xFFFF;
                let iconic = IsIconic(hwnd).as_bool();
                if w_px > 0 && h_px > 0 {
                    WIN_CTX.with(|cell| {
                        if let Ok(mut borrow) = cell.try_borrow_mut() {
                            if let Some(ctx) = borrow.as_mut() {
                                let _ = sync_window_geometry_if_needed(hwnd, ctx);
                                if !iconic {
                                    // Real-time live layout + non-blocking D2D/DComp present during resize/restore!
                                    let dt = ctx
                                        .last_modal_render
                                        .elapsed()
                                        .as_secs_f32()
                                        .clamp(0.001, 0.033);
                                    ctx.last_modal_render = Instant::now();
                                    let _ = ctx.ui.drain_messages(&mut ctx.renderer);
                                    ctx.ui.step_and_render_ex(&mut ctx.renderer, dt, true);
                                    ctx.dirty = false;
                                } else {
                                    ctx.dirty = true;
                                }
                            }
                        }
                    });
                }
                LRESULT(0)
            }

            WM_WINDOWPOSCHANGED => {
                let res = DefWindowProcW(hwnd, msg, wparam, lparam);
                if !IsIconic(hwnd).as_bool() {
                    WIN_CTX.with(|cell| {
                        if let Ok(mut borrow) = cell.try_borrow_mut() {
                            if let Some(ctx) = borrow.as_mut() {
                                ctx.force_mon_refresh = true;
                                if sync_window_geometry_if_needed(hwnd, ctx) {
                                    ctx.dirty = true;
                                }
                            }
                        }
                    });
                }
                res
            }

            WM_DISPLAYCHANGE => {
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.force_mon_refresh = true;
                            ctx.dirty = true;
                        }
                    }
                });
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }

            WM_ACTIVATE => {
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.dirty = true;
                        }
                    }
                });
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_DPICHANGED => {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                );
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.force_mon_refresh = true;
                            let _ = sync_window_geometry_if_needed(hwnd, ctx);
                            ctx.dirty = true;
                        }
                    }
                });
                LRESULT(0)
            }

            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                if !IsIconic(hwnd).as_bool() {
                    WIN_CTX.with(|cell| {
                        if let Ok(mut borrow) = cell.try_borrow_mut() {
                            if let Some(ctx) = borrow.as_mut() {
                                let _ = sync_window_geometry_if_needed(hwnd, ctx);
                                if ctx.in_sizemove {
                                    let dt = ctx
                                        .last_modal_render
                                        .elapsed()
                                        .as_secs_f32()
                                        .clamp(0.001, 0.033);
                                    ctx.last_modal_render = Instant::now();
                                    let _ = ctx.ui.drain_messages(&mut ctx.renderer);
                                    ctx.ui.step_and_render_ex(&mut ctx.renderer, dt, true);
                                    ctx.dirty = false;
                                } else {
                                    ctx.dirty = true;
                                }
                            }
                        }
                    });
                }
                LRESULT(0)
            }

            WM_MOUSEMOVE => {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                let px = (lparam.0 as i16) as f32;
                let py = ((lparam.0 >> 16) as i16) as f32;
                let scale = dpi_scale_for_hwnd(hwnd);
                let lx = px / scale;
                let ly = py / scale;
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            if (ctx.ui.mouse_pos.0 - lx).abs() > 0.5
                                || (ctx.ui.mouse_pos.1 - ly).abs() > 0.5
                            {
                                if ctx.ui.on_mouse_move(lx, ly) {
                                    ctx.dirty = true;
                                }
                            }
                        }
                    }
                });
                LRESULT(0)
            }

            WM_LBUTTONDOWN => {
                let _ = SetCapture(hwnd);
                let px = (lparam.0 as i16) as f32;
                let py = ((lparam.0 >> 16) as i16) as f32;
                let scale = dpi_scale_for_hwnd(hwnd);
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.ui.on_mouse_down(px / scale, py / scale);
                            ctx.dirty = true;
                        }
                    }
                });
                LRESULT(0)
            }

            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let px = (lparam.0 as i16) as f32;
                let py = ((lparam.0 >> 16) as i16) as f32;
                let scale = dpi_scale_for_hwnd(hwnd);
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.ui.on_mouse_up(px / scale, py / scale);
                            ctx.dirty = true;
                        }
                    }
                });
                LRESULT(0)
            }

            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) as i16) as f32 / 120.0;
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.ui.on_wheel(delta);
                            ctx.dirty = true;
                        }
                    }
                });
                LRESULT(0)
            }

            WM_IME_STARTCOMPOSITION => {
                let scale = dpi_scale_for_hwnd(hwnd);
                let mut sb_rect = None;
                WIN_CTX.with(|cell| {
                    if let Ok(borrow) = cell.try_borrow() {
                        if let Some(ctx) = borrow.as_ref() {
                            sb_rect = Some(ctx.ui.search_box_rect);
                        }
                    }
                });
                if let Some(sr) = sb_rect {
                    let himc = ImmGetContext(hwnd);
                    if !himc.is_invalid() {
                        let cf = COMPOSITIONFORM {
                            dwStyle: CFS_POINT,
                            ptCurrentPos: windows::Win32::Foundation::POINT {
                                x: ((sr.x + 36.0) * scale) as i32,
                                y: ((sr.bottom() + 2.0) * scale) as i32,
                            },
                            rcArea: RECT::default(),
                        };
                        let _ = ImmSetCompositionWindow(himc, &cf);
                        let _ = ImmReleaseContext(hwnd, himc);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_KEYDOWN => {
                let vk = wparam.0 as u16;
                let ctrl = (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                let mut handled = false;

                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            if vk == VK_ESCAPE.0 {
                                if ctx.ui.show_lyrics_overlay {
                                    ctx.ui.show_lyrics_overlay = false;
                                    ctx.ui.update_viz_subscription();
                                    handled = true;
                                } else if ctx.ui.show_queue_popover || ctx.ui.show_more_popover {
                                    ctx.ui.show_queue_popover = false;
                                    ctx.ui.show_more_popover = false;
                                    handled = true;
                                } else if ctx.ui.search_focused {
                                    ctx.ui.search_focused = false;
                                    handled = true;
                                }
                            } else if ctrl && vk == b'F' as u16 {
                                ctx.ui.execute_action(UiAction::FocusSearch);
                                handled = true;
                            } else if ctrl && vk == VK_OEM_COMMA.0 {
                                ctx.ui.execute_action(UiAction::Navigate(PageRoute::Settings));
                                handled = true;
                            } else if ctrl && vk == b'V' as u16 {
                                if let Some(txt) = read_clip_quick() {
                                    if ctx.ui.on_paste_text(&txt) {
                                        handled = true;
                                    }
                                }
                            }
                            if handled {
                                ctx.dirty = true;
                            }
                        }
                    }
                });

                if handled {
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }

            WM_CHAR => {
                let cu = wparam.0 as u16;
                let ctrl = (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                if ctrl && cu != 8 && cu != 13 {
                    return LRESULT(0);
                }
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            if (0xD800..=0xDBFF).contains(&cu) {
                                ctx.high_surrogate = Some(cu);
                                return;
                            }
                            let decoded_char = if let Some(hi) = ctx.high_surrogate.take() {
                                char::decode_utf16([hi, cu]).next().and_then(|r| r.ok())
                            } else {
                                char::from_u32(cu as u32)
                            };
                            if let Some(ch) = decoded_char {
                                if ctx.ui.on_char_input(ch) {
                                    ctx.dirty = true;
                                }
                            }
                        }
                    }
                });
                LRESULT(0)
            }

            WM_APP_AUDIO_WAKE => {
                WIN_CTX.with(|cell| {
                    if let Ok(mut borrow) = cell.try_borrow_mut() {
                        if let Some(ctx) = borrow.as_mut() {
                            ctx.dirty = true;
                        }
                    }
                });
                LRESULT(0)
            }

            windows::Win32::UI::WindowsAndMessaging::WM_DROPFILES => {
                use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
                let hdrop = HDROP(wparam.0 as *mut c_void);
                let mut path_buf = [0u16; 520];
                let len = DragQueryFileW(hdrop, 0, Some(&mut path_buf));
                if len > 0 {
                    let path_str = String::from_utf16_lossy(&path_buf[..len as usize]);
                    let path = std::path::PathBuf::from(path_str);
                    WIN_CTX.with(|cell| {
                        if let Ok(mut borrow) = cell.try_borrow_mut() {
                            if let Some(ctx) = borrow.as_mut() {
                                if ctx.ui.import_lyrics_from_path(&path) {
                                    ctx.dirty = true;
                                }
                            }
                        }
                    });
                }
                DragFinish(hdrop);
                LRESULT(0)
            }

            WM_DESTROY => {
                WIN_CTX.with(|cell| {
                    if let Ok(borrow) = cell.try_borrow() {
                        if let Some(ctx) = borrow.as_ref() {
                            ctx.ui.settings.save();
                        }
                    }
                });
                PostQuitMessage(0);
                LRESULT(0)
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn read_clip_quick() -> Option<String> {
        unsafe {
            use windows::Win32::System::DataExchange::{
                CloseClipboard, GetClipboardData, OpenClipboard,
            };
            use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
            use windows::Win32::System::Ole::CF_UNICODETEXT;

            let mut opened = false;
            for _ in 0..6 {
                if OpenClipboard(None).is_ok() {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
            if !opened {
                return None;
            }
            let mut out = None;
            if let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
                let hg = windows::Win32::Foundation::HGLOBAL(h.0);
                let ptr = GlobalLock(hg) as *const u16;
                if !ptr.is_null() {
                    let mut len = 0usize;
                    while *ptr.add(len) != 0 && len < 65_536 {
                        len += 1;
                    }
                    out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len)));
                    let _ = GlobalUnlock(hg);
                }
            } else if let Ok(h) = GetClipboardData(1 /* CF_TEXT */) {
                let hg = windows::Win32::Foundation::HGLOBAL(h.0);
                let ptr = GlobalLock(hg) as *const u8;
                if !ptr.is_null() {
                    let mut len = 0usize;
                    while *ptr.add(len) != 0 && len < 65_536 {
                        len += 1;
                    }
                    out = Some(String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned());
                    let _ = GlobalUnlock(hg);
                }
            }
            let _ = CloseClipboard();
            out
        }
    }

    pub fn run_native_app() {
        if !am_agent::ensure_single_ui_instance_or_focus() {
            return;
        }

        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            // Must run immediately after the first CoInitializeEx in the process and BEFORE any
            // CoCreateInstance (WICImagingFactory, MMDeviceEnumerator, PolicyConfig, SMTC) so
            // CoInitializeSecurity succeeds with S_OK instead of RPC_E_TOO_LATE (0x80010119).
            let _ = am_agent::init_process_com_security();
            binaural_dsp::restore_stale_routing_on_startup();
            let hinstance = GetModuleHandleW(None).unwrap_or_default();
            let class_name = w!("AMLightNativeWindowClass");

            let icon = LoadIconW(Some(hinstance.into()), PCWSTR(1 as *const u16)).unwrap_or_default();
            let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance.into(),
                hIcon: icon,
                hCursor: cursor,
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&wc);

            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);
            let sys_dpi = GetDpiForSystem();
            let sys_scale = if sys_dpi > 0 {
                (sys_dpi as f32 / 96.0).max(1.0)
            } else {
                1.0
            };
            let screen_dip_w = (screen_w as f32) / sys_scale;
            let screen_dip_h = (screen_h as f32) / sys_scale;
            let target_dip_w = (screen_dip_w * 0.82)
                .clamp(980.0, 1380.0)
                .min((screen_dip_w - 28.0).max(800.0));
            let target_dip_h = (screen_dip_h * 0.84)
                .clamp(650.0, 900.0)
                .min((screen_dip_h - 48.0).max(560.0));
            let init_w = ((target_dip_w * sys_scale).round() as i32).min((screen_w - 24).max(640));
            let init_h = ((target_dip_h * sys_scale).round() as i32).min((screen_h - 40).max(480));
            let pos_x = ((screen_w - init_w) / 2).max(12);
            let pos_y = ((screen_h - init_h) / 2).max(12);

            let hwnd = match CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                class_name,
                w!("AM Light"),
                WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU,
                pos_x,
                pos_y,
                init_w,
                init_h,
                None,
                None,
                Some(hinstance.into()),
                None,
            ) {
                Ok(h) => h,
                Err(e) => {
                    am_agent::show_fatal_error_dialog(&format!("创建窗口失败: {e}"));
                    return;
                }
            };

            // Disable OS window rounding & OS rectangular border so DirectComposition renders our
            // exact adjustable rounded-rectangle Liquid Glass window border (0..28 px)!
            let dark_on: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark_on as *const i32 as *const c_void,
                std::mem::size_of::<i32>() as u32,
            );
            let corner_pref = DWMWCP_DONOTROUND.0 as u32;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner_pref as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );
            let border_none: u32 = DWMWA_COLOR_NONE;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                &border_none as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );
            windows::Win32::UI::Shell::DragAcceptFiles(hwnd, true);

            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let w_px = (rc.right - rc.left).max(64) as u32;
            let h_px = (rc.bottom - rc.top).max(64) as u32;
            let scale = dpi_scale_for_hwnd(hwnd);

            let renderer = match D2DRenderer::new(hwnd, w_px, h_px, scale) {
                Ok(r) => r,
                Err(e) => {
                    am_agent::show_fatal_error_dialog(&format!("初始化 Direct2D 渲染器失败: {e}"));
                    return;
                }
            };

            // Safety fallback: if DirectComposition wasn't available and we fell back to CreateSwapChainForHwnd,
            // strip WS_EX_NOREDIRECTIONBITMAP so the HWND flip swapchain renders visibly!
            if !renderer.premultiplied_swapchain {
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                let _ = SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    ex & !(WS_EX_NOREDIRECTIONBITMAP.0 as isize),
                );
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );

            let hwnd_usize = hwnd.0 as usize;
            audio_viz::register_wake_hwnd(hwnd_usize);
            let mut ui = AppUi::new(hwnd_usize);
            let mut init_mon_hz = 60u32;
            let mut init_hmon = 0isize;
            let mut init_mon_poll = Instant::now();
            query_window_monitor_hz(
                hwnd,
                &mut init_hmon,
                &mut init_mon_poll,
                &mut init_mon_hz,
                true,
            );
            ui.current_monitor_hz = init_mon_hz;
            ui.is_window_focused = GetForegroundWindow() == hwnd;

            WIN_CTX.with(|cell| {
                *cell.borrow_mut() = Some(WindowContext {
                    renderer,
                    ui,
                    dirty: true,
                    in_sizemove: false,
                    last_modal_render: Instant::now(),
                    high_surrogate: None,
                    cached_hmon: init_hmon,
                    last_mon_poll: init_mon_poll,
                    monitor_hz: init_mon_hz,
                    force_mon_refresh: false,
                });
            });

            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = InvalidateRect(Some(hwnd), None, false);

            // High-resolution kernel timer for sub-millisecond frame pacing (30..360 Hz & adaptive sub-multiples)
            let hr_timer: Option<HANDLE> = CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .ok();

            let mut last_frame = Instant::now();
            let mut msg = MSG::default();
            let mut running = true;

            while running {
                let is_minimized = IsIconic(hwnd).as_bool();
                let mut animating = false;

                WIN_CTX.with(|cell| {
                    if let Some(ctx) = cell.borrow_mut().as_mut() {
                        if !is_minimized {
                            let _ = sync_window_geometry_if_needed(hwnd, ctx);
                        }
                        let force_mon = ctx.force_mon_refresh;
                        ctx.force_mon_refresh = false;
                        query_window_monitor_hz(
                            hwnd,
                            &mut ctx.cached_hmon,
                            &mut ctx.last_mon_poll,
                            &mut ctx.monitor_hz,
                            force_mon,
                        );
                        ctx.ui.current_monitor_hz = ctx.monitor_hz;
                        ctx.ui.is_window_focused = GetForegroundWindow() == hwnd;
                        if ctx.ui.drain_messages(&mut ctx.renderer) {
                            ctx.dirty = true;
                        }
                        animating = !is_minimized && ctx.ui.needs_continuous_animation();
                    }
                });

                if animating {
                    let frame_start = Instant::now();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                            running = false;
                            break;
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if !running {
                        break;
                    }

                    let dt = last_frame.elapsed().as_secs_f32().clamp(0.001, 0.05);
                    last_frame = Instant::now();

                    let mut pacing = (1u32, 60u32, None::<u64>);
                    WIN_CTX.with(|cell| {
                        if let Some(ctx) = cell.borrow_mut().as_mut() {
                            if !IsIconic(hwnd).as_bool() {
                                let _ = sync_window_geometry_if_needed(hwnd, ctx);
                            }
                            ctx.ui.current_monitor_hz = ctx.monitor_hz;
                            ctx.ui.is_window_focused = GetForegroundWindow() == hwnd;
                            let _ = ctx.ui.drain_messages(&mut ctx.renderer);
                            ctx.ui.step_and_render_ex(&mut ctx.renderer, dt, false);
                            ctx.dirty = false;
                            pacing = ctx.ui.effective_frame_pacing();
                        }
                    });

                    // Hybrid Hardware VBlank (`Present(sync_interval)`) + precision hr_timer fallback:
                    // - When `sync_interval > 0` (`VSync` or exact divisor like 240/60=4, 144/72=2),
                    //   only sleep via hr_timer if Present returned early without waiting for VBlank (e.g. occluded).
                    // - When `sync_interval == 0`, sleep precisely to `sw_target_ns`.
                    let (sync_interval, resolved_hz, sw_target_ns) = pacing;
                    let elapsed_ns = frame_start.elapsed().as_nanos() as u64;
                    let wait_ns = if sync_interval > 0 {
                        let min_ns = 1_000_000_000u64 / (resolved_hz.max(30) as u64);
                        if elapsed_ns + 1_800_000 < min_ns {
                            min_ns.saturating_sub(elapsed_ns)
                        } else {
                            0
                        }
                    } else if let Some(target_ns) = sw_target_ns {
                        target_ns.saturating_sub(elapsed_ns)
                    } else {
                        0
                    };

                    if wait_ns >= 250_000 {
                        if let Some(th) = hr_timer {
                            let due_100ns = -((wait_ns / 100) as i64);
                            if SetWaitableTimer(th, &due_100ns, 0, None, None, false).is_ok() {
                                let _ = WaitForSingleObject(th, 40);
                            }
                        } else {
                            std::thread::sleep(std::time::Duration::from_nanos(wait_ns));
                        }
                    }
                } else {
                    // === IDLE / SILENT STATE: Non-blocking present (`Present(0, 0)`) if dirty so clicks/hovers NEVER stall `GetMessageW`,
                    //     then block in kernel `GetMessageW` (0.00% CPU & 0.00% GPU!) ===
                    let should_draw = !is_minimized
                        && WIN_CTX.with(|cell| {
                            cell.borrow().as_ref().map(|c| c.dirty).unwrap_or(false)
                        });

                    if should_draw {
                        let dt = last_frame.elapsed().as_secs_f32().clamp(0.004, 0.05);
                        last_frame = Instant::now();
                        WIN_CTX.with(|cell| {
                            if let Some(ctx) = cell.borrow_mut().as_mut() {
                                let _ = sync_window_geometry_if_needed(hwnd, ctx);
                                ctx.ui.current_monitor_hz = ctx.monitor_hz;
                                ctx.ui.is_window_focused = GetForegroundWindow() == hwnd;
                                let _ = ctx.ui.drain_messages(&mut ctx.renderer);
                                ctx.ui.step_and_render_ex(&mut ctx.renderer, dt, true);
                                ctx.dirty = false;
                            }
                        });
                        // Re-check if that state change started an animation before sleeping!
                        let started_anim = WIN_CTX.with(|cell| {
                            cell.borrow()
                                .as_ref()
                                .map(|c| c.ui.needs_continuous_animation())
                                .unwrap_or(false)
                        });
                        if started_anim && !is_minimized {
                            continue;
                        }
                    }

                    WIN_CTX.with(|cell| {
                        if let Some(ctx) = cell.borrow_mut().as_mut() {
                            if is_minimized {
                                ctx.ui.lyrics_view.release_gpu_caches(&mut ctx.renderer);
                                ctx.renderer.release_row_fx_surfaces();
                                ctx.renderer.trim_gpu_staging_if_dirty();
                            }
                        }
                    });

                    // Kernel-blocking wait: zero CPU, zero GPU until next OS input or async wakeup message!
                    let ret = GetMessageW(&mut msg, None, 0, 0);
                    if ret.0 <= 0 {
                        break;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);

                    // Drain any coalesced messages in the queue before rendering
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                            running = false;
                            break;
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    last_frame = Instant::now() - std::time::Duration::from_millis(16);
                }
            }

            if let Some(th) = hr_timer {
                let _ = windows::Win32::Foundation::CloseHandle(th);
            }

            // Drop UI & Direct2D resources cleanly before disconnecting audio/COM sessions
            WIN_CTX.with(|cell| {
                *cell.borrow_mut() = None;
            });
            audio_viz::set_viz_subscribed(false);
            binaural_dsp::shutdown_binaural_cleanly();
            let _ = am_agent::disconnect_agent();
        }
    }
}

pub fn run() {
    #[cfg(target_os = "windows")]
    win_app::run_native_app();
}
