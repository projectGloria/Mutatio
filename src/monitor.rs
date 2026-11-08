use crate::config::{Config, StateAppearance, TaskbarState};
use std::ffi::c_void;
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveState {
    Desktop,
    VisibleWindow,
    MaximizedWindow,
    StartOpened,
    SearchOpened,
    GameMode,
}

impl ActiveState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::VisibleWindow => "visible_window",
            Self::MaximizedWindow => "maximized_window",
            Self::StartOpened => "start_opened",
            Self::SearchOpened => "search_opened",
            Self::GameMode => "fullscreen",
        }
    }

    pub fn appearance(self, config: &Config) -> StateAppearance {
        match self {
            Self::Desktop => config.desktop,
            Self::VisibleWindow => config.visible_window,
            Self::MaximizedWindow => config.maximized_window,
            Self::StartOpened => config.start_opened,
            Self::SearchOpened => config.search_opened,
            Self::GameMode => StateAppearance::preset(TaskbarState::Normal),
        }
    }
}

/// What a single EnumWindows pass found on one monitor.
#[derive(Default, Clone, Copy)]
struct MonitorWindows {
    monitor: isize,
    visible: bool,
    maximized: bool,
}

pub struct StateDetector;

impl StateDetector {
    /// Computes the dynamic state for each taskbar monitor in one pass.
    ///
    /// The old detector looked only at the foreground window and applied the result to every
    /// taskbar — a maximized window on monitor A turned monitor B's taskbar opaque too, and a
    /// small focused window on top of a maximized one flipped the bar back to "visible".
    pub fn detect(config: &Config, monitors: &[HMONITOR]) -> Vec<ActiveState> {
        unsafe {
            let fg = GetForegroundWindow();
            let fg_monitor = (!fg.is_invalid()).then(|| MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST));

            // Start / Search / fullscreen only ever matter when they own the foreground.
            let mut fg_override = None;
            if !fg.is_invalid() {
                fg_override = match Self::shell_flyout_kind(fg) {
                    Some(ShellFlyout::Start) => Some(ActiveState::StartOpened),
                    Some(ShellFlyout::Search) => Some(ActiveState::SearchOpened),
                    None if config.game_mode_pause && Self::is_fullscreen(fg) => Some(ActiveState::GameMode),
                    None => None,
                };
            }

            let mut found: Vec<MonitorWindows> = monitors
                .iter()
                .map(|m| MonitorWindows { monitor: m.0 as isize, ..Default::default() })
                .collect();
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut found as *mut _ as isize));

            monitors
                .iter()
                .zip(&found)
                .map(|(mon, info)| {
                    if let (Some(state), Some(fm)) = (fg_override, fg_monitor)
                        && fm == *mon
                    {
                        return state;
                    }
                    if info.maximized {
                        ActiveState::MaximizedWindow
                    } else if info.visible {
                        ActiveState::VisibleWindow
                    } else {
                        ActiveState::Desktop
                    }
                })
                .collect()
        }
    }

    /// Identifies Start / Search by the owning process instead of the window title.
    /// The title ("Start", "Search") is localized, so the old FindWindow-by-title check silently
    /// never matched on non-English Windows.
    fn shell_flyout_kind(hwnd: HWND) -> Option<ShellFlyout> {
        if class_name(hwnd).as_deref() != Some("Windows.UI.Core.CoreWindow") || is_cloaked(hwnd) {
            return None;
        }
        let exe = process_image_name(hwnd)?.to_ascii_lowercase();
        match exe.as_str() {
            "startmenuexperiencehost.exe" => Some(ShellFlyout::Start),
            "searchhost.exe" | "searchapp.exe" | "searchui.exe" => Some(ShellFlyout::Search),
            _ => None,
        }
    }

    /// Borderless/exclusive fullscreen: covers the whole monitor (not just the work area).
    fn is_fullscreen(hwnd: HWND) -> bool {
        unsafe {
            // A maximized window covers the monitor too whenever the taskbar auto-hides
            // (work area == monitor area). Without this check, every maximized window was treated
            // as a "game" and the taskbar snapped back to stock.
            if IsZoomed(hwnd).as_bool() {
                return false;
            }
            let Some(rect) = window_rect(hwnd) else { return false };
            let Some(mi) = monitor_info(hwnd) else { return false };
            covers(&rect, &mi.rcMonitor) && !is_shell_window(hwnd)
        }
    }
}

enum ShellFlyout {
    Start,
    Search,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let found = &mut *(lparam.0 as *mut Vec<MonitorWindows>);

        // Cheapest rejections first: most top-level windows are hidden.
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() || !is_user_window(hwnd) {
            return BOOL(1);
        }

        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST).0 as isize;
        if let Some(entry) = found.iter_mut().find(|m| m.monitor == mon) {
            entry.visible = true;
            if !entry.maximized && is_maximized(hwnd) {
                entry.maximized = true;
            }
        }

        // Stop once every monitor is already known to have a maximized window.
        BOOL((!found.iter().all(|m| m.maximized)) as i32)
    }
}

/// A normal app window: not cloaked (other virtual desktop / suspended UWP), not a tool window,
/// not shell chrome, and not a tiny helper window.
unsafe fn is_user_window(hwnd: HWND) -> bool {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 || is_cloaked(hwnd) || is_shell_window(hwnd) {
            return false;
        }
        window_rect(hwnd).is_some_and(|r| r.right - r.left > 100 && r.bottom - r.top > 100)
    }
}

unsafe fn is_maximized(hwnd: HWND) -> bool {
    unsafe {
        if IsZoomed(hwnd).as_bool() {
            return true;
        }
        // Windows that fill the work area without the maximized flag (borderless apps, snapped
        // full-height+full-width layouts) look identical to the user.
        match (window_rect(hwnd), monitor_info(hwnd)) {
            (Some(r), Some(mi)) => covers(&r, &mi.rcWork),
            _ => false,
        }
    }
}

fn covers(r: &RECT, area: &RECT) -> bool {
    r.left <= area.left && r.top <= area.top && r.right >= area.right && r.bottom >= area.bottom
}

fn is_shell_window(hwnd: HWND) -> bool {
    matches!(
        class_name(hwnd).as_deref(),
        Some("Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Windows.UI.Core.CoreWindow")
    )
}

fn class_name(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    (len > 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r).ok()? };
    Some(r)
}

fn monitor_info(hwnd: HWND) -> Option<MONITORINFO> {
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(hmon, &mut mi).as_bool().then_some(mi)
    }
}

/// DWM cloaking hides windows without un-showing them (other virtual desktops, suspended UWP).
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
    }
}

/// File name (no path) of the process owning `hwnd`.
fn process_image_name(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        res.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().map(str::to_owned)
    }
}
