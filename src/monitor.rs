use crate::config::{Config, StateAppearance, TaskbarState};
use std::ffi::c_void;
use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowPlacement,
    GetWindowRect, IsIconic, IsWindow, IsWindowVisible, IsZoomed, GWL_EXSTYLE,
    SW_SHOWMAXIMIZED, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
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

pub struct StateDetector;

impl StateDetector {
    /// Determines current active dynamic state based on desktop & window status
    pub fn detect_current_state(config: &Config) -> (ActiveState, StateAppearance) {
        unsafe {
            // 1. Check if Start Menu is currently open
            if Self::is_start_menu_open() {
                return (ActiveState::StartOpened, config.start_opened.clone());
            }

            // 2. Check if Search is open
            if Self::is_search_open() {
                return (ActiveState::SearchOpened, config.search_opened.clone());
            }

            // 3. Inspect Foreground Window
            let fg = GetForegroundWindow();
            if fg.0 as usize != 0 && IsWindow(Some(fg)).as_bool() {
                // Check if Game / Fullscreen Exclusive mode applies
                if config.game_mode_pause && Self::is_fullscreen_exclusive(fg) {
                    let mut norm = StateAppearance::default();
                    norm.state = TaskbarState::Normal;
                    norm.show_line = true;
                    return (ActiveState::GameMode, norm);
                }

                // Check if foreground window is maximized
                if Self::is_maximized(fg) {
                    return (ActiveState::MaximizedWindow, config.maximized_window.clone());
                }

                // Check if foreground window is a regular user visible window
                if Self::is_user_visible_window(fg) {
                    return (ActiveState::VisibleWindow, config.visible_window.clone());
                }
            }

            // 4. Default: Desktop is visible
            (ActiveState::Desktop, config.desktop.clone())
        }
    }

    /// Checks if Start Menu Experience window is active/visible
    pub fn is_start_menu_open() -> bool {
        unsafe {
            let start = FindWindowW(w!("Windows.UI.Core.CoreWindow"), w!("Start"));
            if let Ok(hwnd) = start {
                if hwnd.0 as usize != 0 && IsWindowVisible(hwnd).as_bool() {
                    return !Self::is_cloaked(hwnd);
                }
            }
            false
        }
    }

    /// Checks if Search Host flyout is active/visible
    pub fn is_search_open() -> bool {
        unsafe {
            let search = FindWindowW(w!("Windows.UI.Core.CoreWindow"), w!("Search"));
            if let Ok(hwnd) = search {
                if hwnd.0 as usize != 0 && IsWindowVisible(hwnd).as_bool() {
                    return !Self::is_cloaked(hwnd);
                }
            }
            false
        }
    }

    /// Checks if window is maximized
    pub fn is_maximized(hwnd: HWND) -> bool {
        unsafe {
            if IsZoomed(hwnd).as_bool() {
                return true;
            }

            let mut wp = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };

            if GetWindowPlacement(hwnd, &mut wp).is_ok() {
                if wp.showCmd == SW_SHOWMAXIMIZED.0 as u32 {
                    return true;
                }
            }

            // Compare window rect with monitor work area
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };

            let mut wrect = RECT::default();
            if GetMonitorInfoW(hmon, &mut mi).as_bool() && GetWindowRect(hwnd, &mut wrect).is_ok() {
                let is_matching = wrect.left <= mi.rcWork.left
                    && wrect.top <= mi.rcWork.top
                    && wrect.right >= mi.rcWork.right
                    && wrect.bottom >= mi.rcWork.bottom;
                if is_matching && Self::is_user_visible_window(hwnd) {
                    return true;
                }
            }

            false
        }
    }

    /// Checks if a window is running in fullscreen exclusive mode (e.g. Games)
    pub fn is_fullscreen_exclusive(hwnd: HWND) -> bool {
        unsafe {
            let mut wrect = RECT::default();
            if GetWindowRect(hwnd, &mut wrect).is_err() {
                return false;
            }

            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };

            if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                let matches_screen = wrect.left <= mi.rcMonitor.left
                    && wrect.top <= mi.rcMonitor.top
                    && wrect.right >= mi.rcMonitor.right
                    && wrect.bottom >= mi.rcMonitor.bottom;

                if matches_screen {
                    let mut class_buf = [0u16; 64];
                    let len = GetClassNameW(hwnd, &mut class_buf);
                    if len > 0 {
                        let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
                        if class_name == "Progman"
                            || class_name == "WorkerW"
                            || class_name == "Shell_TrayWnd"
                        {
                            return false;
                        }
                    }
                    return true;
                }
            }

            false
        }
    }

    /// Determines if a window is a standard visible user desktop application window
    pub fn is_user_visible_window(hwnd: HWND) -> bool {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return false;
            }

            if Self::is_cloaked(hwnd) {
                return false;
            }

            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if (ex_style & WS_EX_TOOLWINDOW.0) != 0 {
                return false;
            }

            let mut class_buf = [0u16; 64];
            let len = GetClassNameW(hwnd, &mut class_buf);
            if len > 0 {
                let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
                if class_name == "Progman"
                    || class_name == "WorkerW"
                    || class_name == "Shell_TrayWnd"
                    || class_name == "Shell_SecondaryTrayWnd"
                    || class_name == "Windows.UI.Core.CoreWindow"
                {
                    return false;
                }
            }

            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_ok() {
                let w = rect.right - rect.left;
                let h = rect.bottom - rect.top;
                if w > 100 && h > 100 {
                    return true;
                }
            }

            false
        }
    }

    /// Check if window is cloaked by DWM (Windows 8+)
    pub fn is_cloaked(hwnd: HWND) -> bool {
        unsafe {
            let mut cloaked: u32 = 0;
            let hr = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut _ as *mut c_void,
                std::mem::size_of::<u32>() as u32,
            );
            hr.is_ok() && cloaked != 0
        }
    }
}
