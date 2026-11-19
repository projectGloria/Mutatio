// GDI draw helpers take raw geometry + style args; bundling them into structs buys nothing here.
#![allow(clippy::too_many_arguments)]

use crate::config::{Color, TaskbarState};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, MonitorFromPoint, RoundRect, ScreenToClient, SelectObject,
    SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DT_CENTER,
    DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT,
    HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID,
    SRCCOPY, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetCursorPos, IsWindowVisible, LoadCursorW,
    PostMessageW, RegisterClassW, SetCursor, SetForegroundWindow, SetWindowPos, ShowWindow,
    HWND_TOPMOST, IDC_ARROW, IDC_HAND, SWP_NOACTIVATE, SW_HIDE, SW_SHOW, WA_INACTIVE,
    WM_ACTIVATE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_USER, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

pub const WM_FLYOUT_ACTION: u32 = WM_USER + 200;

// Lives in Win32_UI_Controls in windows-rs; not worth a whole feature for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;
const HTCLIENT: isize = 1;

pub const ACTION_SET_CLEAR: usize = 1;
pub const ACTION_SET_BLUR: usize = 2;
pub const ACTION_SET_ACRYLIC: usize = 3;
pub const ACTION_SET_OPAQUE: usize = 4;
pub const ACTION_SET_DEFAULT: usize = 5;
pub const ACTION_TOGGLE_DYNAMIC: usize = 6;
pub const ACTION_TOGGLE_SMOOTH: usize = 7;
pub const ACTION_TOGGLE_GAME_MODE: usize = 8;
pub const ACTION_TOGGLE_BORDER: usize = 9;
pub const ACTION_SET_COLOR: usize = 10;
pub const ACTION_OPEN_CONFIG: usize = 11;
pub const ACTION_RESTART_EXPLORER: usize = 12;
pub const ACTION_EXIT: usize = 13;

/// Logical (96-DPI) flyout dimensions. Scaled at runtime for Per-Monitor DPI v2.
pub const FLYOUT_WIDTH: i32 = 360;
pub const FLYOUT_HEIGHT: i32 = 552;

#[derive(Clone, Debug)]
pub struct FlyoutState {
    pub active_state: TaskbarState,
    pub is_forced: bool,
    pub dynamic_enabled: bool,
    pub smooth_enabled: bool,
    pub game_pause_enabled: bool,
    pub show_border: bool,
    pub current_color: Color,
    /// Name of the active rendering backend, shown in the header.
    pub engine: &'static str,
}

impl Default for FlyoutState {
    fn default() -> Self {
        Self {
            active_state: TaskbarState::Clear,
            is_forced: false,
            dynamic_enabled: true,
            smooth_enabled: true,
            game_pause_enabled: true,
            show_border: false,
            current_color: Color::transparent(),
            engine: "",
        }
    }
}

// Everything below is only touched from the UI thread (window procs run on the thread that
// created the window), so thread_local + Cell/RefCell avoids locking overhead.
thread_local! {
    static FLYOUT_DATA: RefCell<FlyoutState> = RefCell::new(FlyoutState::default());
    static FONTS: RefCell<Option<Fonts>> = const { RefCell::new(None) };
    /// When the flyout last auto-hid on deactivation; see `FlyoutWindow::toggle`.
    static LAST_AUTO_HIDE: Cell<Option<Instant>> = const { Cell::new(None) };
}
static PARENT_NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);
static HOVER_INDEX: AtomicI32 = AtomicI32::new(-1);

#[inline]
fn scale(v: i32, dpi: i32) -> i32 {
    (v * dpi + 48) / 96
}

#[inline]
fn unscale(px: i32, dpi: i32) -> i32 {
    (px * 96 + dpi / 2) / dpi.max(1)
}

fn window_dpi(hwnd: HWND) -> i32 {
    let d = unsafe { GetDpiForWindow(hwnd) } as i32;
    if d > 0 { d } else { 96 }
}

fn monitor_dpi(hmon: HMONITOR) -> i32 {
    let mut dx = 0u32;
    let mut dy = 0u32;
    if unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_ok() && dx > 0 {
        dx as i32
    } else {
        96
    }
}

/// GDI fonts cached per DPI so mouse-move repaints don't recreate GDI objects.
struct Fonts {
    dpi: i32,
    title: HFONT,
    section: HFONT,
    body: HFONT,
    sub: HFONT,
}

impl Fonts {
    fn new(dpi: i32) -> Self {
        let make = |height: i32, weight: i32, face: windows::core::PCWSTR| unsafe {
            CreateFontW(
                -scale(height, dpi),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                face,
            )
        };
        Self {
            dpi,
            title: make(18, FW_BOLD.0 as i32, w!("Segoe UI Variable Display")),
            section: make(10, FW_BOLD.0 as i32, w!("Segoe UI")),
            body: make(13, FW_SEMIBOLD.0 as i32, w!("Segoe UI Variable Text")),
            sub: make(11, FW_NORMAL.0 as i32, w!("Segoe UI")),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for f in [self.title, self.section, self.body, self.sub] {
                let _ = DeleteObject(f.into());
            }
        }
    }
}

fn with_fonts<R>(dpi: i32, f: impl FnOnce(&Fonts) -> R) -> R {
    FONTS.with(|slot| {
        let mut guard = slot.borrow_mut();
        if guard.as_ref().is_none_or(|fonts| fonts.dpi != dpi) {
            *guard = Some(Fonts::new(dpi));
        }
        f(guard.as_ref().unwrap())
    })
}

/// Quick-tint swatches: (zone, color). Shared by click handling and painting so they can't
/// disagree.
const SWATCHES: [(i32, Color); 6] = [
    (ZONE_SWATCH_BLACK, Color::new(0, 0, 0, 255)),
    (ZONE_SWATCH_SLATE, Color::new(24, 24, 28, 255)),
    (ZONE_SWATCH_NAVY, Color::new(14, 30, 56, 255)),
    (ZONE_SWATCH_CRIMSON, Color::new(50, 12, 18, 255)),
    (ZONE_SWATCH_EMERALD, Color::new(10, 42, 26, 255)),
    (ZONE_SWATCH_WHITE, Color::new(255, 255, 255, 255)),
];

pub struct FlyoutWindow {
    hwnd: HWND,
}

impl FlyoutWindow {
    pub fn create(parent_notify_hwnd: HWND) -> Self {
        unsafe {
            PARENT_NOTIFY_HWND.store(parent_notify_hwnd.0 as isize, Ordering::Relaxed);

            let class_name = w!("MutatioFlyoutWindowClass");
            let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
            let wnd_class = WNDCLASSW {
                lpfnWndProc: Some(flyout_wnd_proc),
                hCursor: arrow,
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassW(&wnd_class);

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class_name,
                w!("Mutatio Flyout"),
                WS_POPUP,
                0,
                0,
                FLYOUT_WIDTH,
                FLYOUT_HEIGHT,
                None,
                None,
                None,
                None,
            )
            .unwrap_or(HWND::default());

            // Windows 11 Rounded corners and Dark Mode
            let corner_pref: u32 = 2; // DWMWCP_ROUND
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner_pref as *const _ as *const _,
                std::mem::size_of::<u32>() as u32,
            );

            let dark_mode: u32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark_mode as *const _ as *const _,
                std::mem::size_of::<u32>() as u32,
            );

            Self { hwnd }
        }
    }

    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    pub fn update_state(&self, state: FlyoutState) {
        FLYOUT_DATA.with(|d| *d.borrow_mut() = state);
        if self.is_visible() {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    pub fn toggle(&self, cursor_pt: POINT) {
        // Clicking the tray icon while the flyout is open deactivates the flyout on mouse-DOWN
        // (auto-hide), then the tray's mouse-UP arrives here and would immediately reopen it.
        // Treat a toggle right after an auto-hide as the "close" click it was meant to be.
        let just_auto_hidden = LAST_AUTO_HIDE
            .with(|t| t.take())
            .is_some_and(|t| t.elapsed() < Duration::from_millis(300));

        if self.is_visible() {
            self.hide();
        } else if !just_auto_hidden {
            self.show_at(cursor_pt);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn show_at(&self, pt: POINT) {
        unsafe {
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let _ = GetMonitorInfoW(hmon, &mut mi);
            let work = mi.rcWork;
            let dpi = monitor_dpi(hmon);

            let win_w = scale(FLYOUT_WIDTH, dpi);
            let win_h = scale(FLYOUT_HEIGHT, dpi);
            let margin = scale(12, dpi);

            let x = (pt.x - win_w / 2).clamp(work.left + margin, (work.right - win_w - margin).max(work.left));
            let y = if pt.y < work.top + scale(100, dpi) {
                work.top + margin
            } else {
                (work.bottom - win_h - margin).max(work.top + margin)
            };

            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                win_w,
                win_h,
                SWP_NOACTIVATE,
            );

            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
}

const ZONE_CLOSE: i32 = 100;
const ZONE_PRESET_CLEAR: i32 = 1;
const ZONE_PRESET_BLUR: i32 = 2;
const ZONE_PRESET_ACRYLIC: i32 = 3;
const ZONE_PRESET_OPAQUE: i32 = 4;
const ZONE_PRESET_DEFAULT: i32 = 5;

const ZONE_SWATCH_BLACK: i32 = 10;
const ZONE_SWATCH_SLATE: i32 = 11;
const ZONE_SWATCH_NAVY: i32 = 12;
const ZONE_SWATCH_CRIMSON: i32 = 13;
const ZONE_SWATCH_EMERALD: i32 = 14;
const ZONE_SWATCH_WHITE: i32 = 15;

const ZONE_TOGGLE_DYNAMIC: i32 = 20;
const ZONE_TOGGLE_SMOOTH: i32 = 21;
const ZONE_TOGGLE_GAME: i32 = 22;
const ZONE_TOGGLE_BORDER: i32 = 23;

const ZONE_BTN_CONFIG: i32 = 30;
const ZONE_BTN_RESTART_SHELL: i32 = 31;
const ZONE_BTN_EXIT: i32 = 32;

/// Clickable regions in logical DIPs as (zone, left, top, right, bottom), inclusive.
/// Must match the geometry drawn in `render_flyout`.
const HIT_ZONES: &[(i32, i32, i32, i32, i32)] = &[
    (ZONE_CLOSE, 316, 16, 344, 44),
    (ZONE_PRESET_CLEAR, 16, 86, 120, 126),
    (ZONE_PRESET_BLUR, 128, 86, 232, 126),
    (ZONE_PRESET_ACRYLIC, 240, 86, 344, 126),
    (ZONE_PRESET_OPAQUE, 16, 134, 176, 174),
    (ZONE_PRESET_DEFAULT, 184, 134, 344, 174),
    (ZONE_SWATCH_BLACK, 20, 204, 52, 236),
    (ZONE_SWATCH_SLATE, 70, 204, 102, 236),
    (ZONE_SWATCH_NAVY, 120, 204, 152, 236),
    (ZONE_SWATCH_CRIMSON, 170, 204, 202, 236),
    (ZONE_SWATCH_EMERALD, 220, 204, 252, 236),
    (ZONE_SWATCH_WHITE, 270, 204, 302, 236),
    (ZONE_TOGGLE_DYNAMIC, 16, 270, 344, 308),
    (ZONE_TOGGLE_SMOOTH, 16, 314, 344, 352),
    (ZONE_TOGGLE_GAME, 16, 358, 344, 396),
    (ZONE_TOGGLE_BORDER, 16, 402, 344, 440),
    (ZONE_BTN_CONFIG, 16, 498, 120, 534),
    (ZONE_BTN_RESTART_SHELL, 128, 498, 236, 534),
    (ZONE_BTN_EXIT, 244, 498, 344, 534),
];

fn hit_test(dip_x: i32, dip_y: i32) -> i32 {
    HIT_ZONES
        .iter()
        .find(|&&(_, l, t, r, b)| (l..=r).contains(&dip_x) && (t..=b).contains(&dip_y))
        .map_or(-1, |&(zone, ..)| zone)
}

fn client_dip_from_lparam(hwnd: HWND, lparam: LPARAM) -> (i32, i32) {
    let dpi = window_dpi(hwnd);
    let px_x = (lparam.0 & 0xFFFF) as i16 as i32;
    let px_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (unscale(px_x, dpi), unscale(px_y, dpi))
}

unsafe extern "system" fn flyout_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);

                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let w = rect.right - rect.left;
                let h = rect.bottom - rect.top;

                let mem_dc = CreateCompatibleDC(Some(hdc));
                let mem_bm = CreateCompatibleBitmap(hdc, w, h);
                let old_bm = SelectObject(mem_dc, mem_bm.into());

                render_flyout(mem_dc, w, h, window_dpi(hwnd));

                let _ = BitBlt(hdc, 0, 0, w, h, Some(mem_dc), 0, 0, SRCCOPY);

                SelectObject(mem_dc, old_bm);
                let _ = DeleteObject(mem_bm.into());
                let _ = DeleteDC(mem_dc);

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                if (lparam.0 & 0xFFFF) == HTCLIENT {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(hwnd, &mut pt);
                    let dpi = window_dpi(hwnd);
                    let zone = hit_test(unscale(pt.x, dpi), unscale(pt.y, dpi));
                    let cursor_id = if zone >= 0 { IDC_HAND } else { IDC_ARROW };
                    if let Ok(cursor) = LoadCursorW(None, cursor_id) {
                        SetCursor(Some(cursor));
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DPICHANGED => {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOACTIVATE,
                );
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_ACTIVATE => {
                let active = (wparam.0 & 0xFFFF) as u16;
                if active == WA_INACTIVE as u16 {
                    auto_hide(hwnd);
                }
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                auto_hide(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if HOVER_INDEX.swap(-1, Ordering::Relaxed) != -1 {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => LRESULT(0),
            WM_LBUTTONUP => {
                let (dip_x, dip_y) = client_dip_from_lparam(hwnd, lparam);
                let zone = hit_test(dip_x, dip_y);

                if zone == ZONE_CLOSE {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    return LRESULT(0);
                }

                let parent_raw = PARENT_NOTIFY_HWND.load(Ordering::Relaxed);
                if let (Some((action, payload)), true) = (zone_action(zone), parent_raw != 0) {
                    let parent = HWND(parent_raw as *mut _);
                    let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(action), LPARAM(payload));
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (dip_x, dip_y) = client_dip_from_lparam(hwnd, lparam);
                let zone = hit_test(dip_x, dip_y);
                if HOVER_INDEX.swap(zone, Ordering::Relaxed) != zone {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Hides on focus loss and remembers when, for the tray re-open race in `toggle`.
unsafe fn auto_hide(hwnd: HWND) {
    unsafe {
        if IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LAST_AUTO_HIDE.with(|t| t.set(Some(Instant::now())));
        }
    }
}

/// Maps a clicked zone to the (action, lparam) posted to the main window.
fn zone_action(zone: i32) -> Option<(usize, isize)> {
    if let Some((_, color)) = SWATCHES.iter().find(|(z, _)| *z == zone) {
        return Some((ACTION_SET_COLOR, color.to_abgr() as isize));
    }
    let action = match zone {
        ZONE_PRESET_CLEAR => ACTION_SET_CLEAR,
        ZONE_PRESET_BLUR => ACTION_SET_BLUR,
        ZONE_PRESET_ACRYLIC => ACTION_SET_ACRYLIC,
        ZONE_PRESET_OPAQUE => ACTION_SET_OPAQUE,
        ZONE_PRESET_DEFAULT => ACTION_SET_DEFAULT,
        ZONE_TOGGLE_DYNAMIC => ACTION_TOGGLE_DYNAMIC,
        ZONE_TOGGLE_SMOOTH => ACTION_TOGGLE_SMOOTH,
        ZONE_TOGGLE_GAME => ACTION_TOGGLE_GAME_MODE,
        ZONE_TOGGLE_BORDER => ACTION_TOGGLE_BORDER,
        ZONE_BTN_CONFIG => ACTION_OPEN_CONFIG,
        ZONE_BTN_RESTART_SHELL => ACTION_RESTART_EXPLORER,
        ZONE_BTN_EXIT => ACTION_EXIT,
        _ => return None,
    };
    Some((action, 0))
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

unsafe fn draw_text(hdc: HDC, text: &str, left: i32, top: i32, right: i32, bottom: i32, flags: u32) {
    unsafe {
        let mut rc = RECT { left, top, right, bottom };
        let mut utf16: Vec<u16> = text.encode_utf16().collect();
        let _ = DrawTextW(hdc, &mut utf16, &mut rc, windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(flags));
    }
}

unsafe fn draw_round_box(
    hdc: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    bg_color: COLORREF,
    border_color: COLORREF,
    radius: i32,
) {
    unsafe {
        let brush = CreateSolidBrush(bg_color);
        let pen = CreatePen(PS_SOLID, 1, border_color);

        let old_brush = SelectObject(hdc, brush.into());
        let old_pen = SelectObject(hdc, pen.into());

        let _ = RoundRect(hdc, x, y, x + w, y + h, radius, radius);

        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

unsafe fn draw_circle(
    hdc: HDC,
    cx: i32,
    cy: i32,
    radius: i32,
    bg_color: COLORREF,
    border_color: COLORREF,
) {
    unsafe {
        let brush = CreateSolidBrush(bg_color);
        let pen = CreatePen(PS_SOLID, 1, border_color);

        let old_brush = SelectObject(hdc, brush.into());
        let old_pen = SelectObject(hdc, pen.into());

        let _ = Ellipse(
            hdc,
            cx - radius,
            cy - radius,
            cx + radius,
            cy + radius,
        );

        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

unsafe fn draw_toggle(hdc: HDC, x: i32, y: i32, dpi: i32, active: bool) {
    unsafe {
        let w = scale(38, dpi);
        let h = scale(20, dpi);
        let bg = if active { rgb(0, 120, 212) } else { rgb(48, 48, 56) };
        let border = if active { rgb(59, 158, 255) } else { rgb(74, 74, 84) };
        draw_round_box(hdc, x, y, w, h, bg, border, h);

        let thumb_x = if active { x + scale(28, dpi) } else { x + scale(10, dpi) };
        let thumb_y = y + h / 2;
        draw_circle(hdc, thumb_x, thumb_y, scale(6, dpi), rgb(255, 255, 255), rgb(255, 255, 255));
    }
}

unsafe fn render_flyout(hdc: HDC, width: i32, height: i32, dpi: i32) {
    let s = |v: i32| scale(v, dpi);
    let state = FLYOUT_DATA.with(|d| d.borrow().clone());
    let hover = HOVER_INDEX.load(Ordering::Relaxed);

    with_fonts(dpi, |fonts| unsafe {
        let bg_brush = CreateSolidBrush(rgb(24, 24, 28));
        let full_rc = RECT { left: 0, top: 0, right: width, bottom: height };
        FillRect(hdc, &full_rc, bg_brush);
        let _ = DeleteObject(bg_brush.into());

        // Subtle 1px inset frame (DWM handles outer rounded corners cleanly).
        let border_pen = CreatePen(PS_SOLID, 1, rgb(46, 46, 54));
        let old_pen = SelectObject(hdc, border_pen.into());
        let old_brush = SelectObject(
            hdc,
            windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::NULL_BRUSH),
        );
        let _ = RoundRect(hdc, 0, 0, width, height, s(12), s(12));
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(border_pen.into());

        SetBkMode(hdc, TRANSPARENT);
        let old_font = SelectObject(hdc, fonts.title.into());

        // HEADER
        SetTextColor(hdc, rgb(248, 248, 252));
        draw_text(hdc, "Mutatio", s(18), s(14), s(120), s(38), DT_LEFT.0 | DT_SINGLELINE.0);

        // Version pill
        draw_round_box(hdc, s(94), s(18), s(48), s(18), rgb(34, 34, 42), rgb(52, 52, 62), s(9));
        SelectObject(hdc, fonts.section.into());
        SetTextColor(hdc, rgb(155, 160, 175));
        draw_text(
            hdc,
            &format!("v{}", env!("CARGO_PKG_VERSION")),
            s(94),
            s(18),
            s(142),
            s(36),
            DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );

        // Engine status line
        draw_circle(hdc, s(22), s(47), s(3), rgb(52, 211, 153), rgb(52, 211, 153));
        SelectObject(hdc, fonts.sub.into());
        SetTextColor(hdc, rgb(140, 215, 180));
        draw_text(
            hdc,
            &format!("{} engine", state.engine),
            s(30),
            s(39),
            s(290),
            s(55),
            DT_LEFT.0 | DT_SINGLELINE.0,
        );

        // Close button (✕)
        let close_hover = hover == ZONE_CLOSE;
        let close_bg = if close_hover { rgb(58, 58, 68) } else { rgb(34, 34, 42) };
        let close_border = if close_hover { rgb(82, 82, 95) } else { rgb(48, 48, 58) };
        draw_round_box(hdc, s(316), s(16), s(28), s(28), close_bg, close_border, s(8));
        SetTextColor(hdc, rgb(210, 212, 220));
        SelectObject(hdc, fonts.body.into());
        draw_text(hdc, "✕", s(316), s(16), s(344), s(44), DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0);

        // SECTION 1: APPEARANCE PRESETS
        SelectObject(hdc, fonts.section.into());
        SetTextColor(hdc, rgb(132, 136, 150));
        draw_text(hdc, "APPEARANCE", s(18), s(68), s(200), s(82), DT_LEFT.0 | DT_SINGLELINE.0);

        draw_card(hdc, dpi, s(16), s(86), s(104), s(40), "Clear", "Transparent", state.active_state == TaskbarState::Clear, hover == ZONE_PRESET_CLEAR, fonts);
        draw_card(hdc, dpi, s(128), s(86), s(104), s(40), "Blur", "Aero glass", state.active_state == TaskbarState::Blur, hover == ZONE_PRESET_BLUR, fonts);
        draw_card(hdc, dpi, s(240), s(86), s(104), s(40), "Acrylic", "Fluent blur", state.active_state == TaskbarState::Acrylic, hover == ZONE_PRESET_ACRYLIC, fonts);
        draw_card(hdc, dpi, s(16), s(134), s(160), s(40), "Opaque", "Solid black", state.active_state == TaskbarState::Opaque, hover == ZONE_PRESET_OPAQUE, fonts);
        draw_card(hdc, dpi, s(184), s(134), s(160), s(40), "Default", "Windows stock", state.active_state == TaskbarState::Normal, hover == ZONE_PRESET_DEFAULT, fonts);

        // SECTION 2: TINT COLOR
        SelectObject(hdc, fonts.section.into());
        SetTextColor(hdc, rgb(132, 136, 150));
        draw_text(hdc, "TINT COLOR", s(18), s(186), s(180), s(200), DT_LEFT.0 | DT_SINGLELINE.0);

        let c = state.current_color;
        let hex_label = if c.a == 0 {
            "Transparent".to_string()
        } else {
            format!("#{:02X}{:02X}{:02X} · {}%", c.r, c.g, c.b, (c.a as u32 * 100 / 255))
        };
        SelectObject(hdc, fonts.sub.into());
        SetTextColor(hdc, rgb(150, 154, 168));
        draw_text(hdc, &hex_label, s(180), s(185), s(344), s(201), DT_RIGHT.0 | DT_SINGLELINE.0);

        for (i, (zone, color)) in SWATCHES.iter().enumerate() {
            let cx = s(36 + 50 * i as i32);
            let cy = s(220);
            let fill = rgb(color.r, color.g, color.b);
            draw_swatch(hdc, dpi, cx, cy, fill, hover == *zone, state.current_color == *color);
        }

        // SECTION 3: SMART AUTOMATION
        SelectObject(hdc, fonts.section.into());
        SetTextColor(hdc, rgb(132, 136, 150));
        draw_text(hdc, "AUTOMATION", s(18), s(252), s(220), s(266), DT_LEFT.0 | DT_SINGLELINE.0);

        draw_toggle_row(hdc, dpi, s(16), s(270), s(328), s(38), "Dynamic Auto-Detect", "Adapt to windows & Start menu", state.dynamic_enabled && !state.is_forced, hover == ZONE_TOGGLE_DYNAMIC, fonts);
        draw_toggle_row(hdc, dpi, s(16), s(314), s(328), s(38), "Smooth Animations", "60fps cubic fade transitions", state.smooth_enabled, hover == ZONE_TOGGLE_SMOOTH, fonts);
        draw_toggle_row(hdc, dpi, s(16), s(358), s(328), s(38), "Fullscreen Game Sleep", "Stock taskbar in fullscreen apps", state.game_pause_enabled, hover == ZONE_TOGGLE_GAME, fonts);
        draw_toggle_row(hdc, dpi, s(16), s(402), s(328), s(38), "Top Border Line", "Accent hairline above taskbar", state.show_border, hover == ZONE_TOGGLE_BORDER, fonts);

        // Status banner
        draw_round_box(hdc, s(16), s(452), s(328), s(32), rgb(30, 30, 37), rgb(46, 46, 56), s(8));
        let (dot_col, hint_str) = if state.is_forced {
            (rgb(251, 191, 36), "Forced Preset  —  Dynamic mode paused")
        } else if state.dynamic_enabled {
            (rgb(52, 211, 153), "Dynamic Active  —  Adapting to windows")
        } else {
            (rgb(156, 163, 175), "Fixed Mode  —  Static desktop appearance")
        };
        draw_circle(hdc, s(30), s(468), s(4), dot_col, dot_col);
        SelectObject(hdc, fonts.sub.into());
        SetTextColor(hdc, rgb(185, 188, 200));
        draw_text(hdc, hint_str, s(42), s(452), s(336), s(484), DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0);

        // SECTION 4: FOOTER ACTIONS
        draw_action_btn(hdc, dpi, s(16), s(498), s(104), s(36), "Config JSON", hover == ZONE_BTN_CONFIG, rgb(36, 36, 44), rgb(54, 54, 66), fonts);
        draw_action_btn(hdc, dpi, s(128), s(498), s(108), s(36), "Restart Shell", hover == ZONE_BTN_RESTART_SHELL, rgb(36, 36, 44), rgb(54, 54, 66), fonts);
        draw_action_btn(hdc, dpi, s(244), s(498), s(100), s(36), "Exit", hover == ZONE_BTN_EXIT, rgb(62, 26, 30), rgb(94, 40, 46), fonts);

        SelectObject(hdc, old_font);
    });
}

unsafe fn draw_card(
    hdc: HDC,
    dpi: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: &str,
    sub: &str,
    is_active: bool,
    is_hover: bool,
    fonts: &Fonts,
) {
    let s = |v: i32| scale(v, dpi);
    unsafe {
        let (bg, border) = if is_active {
            (rgb(20, 48, 78), rgb(59, 158, 255))
        } else if is_hover {
            (rgb(42, 42, 50), rgb(68, 68, 80))
        } else {
            (rgb(32, 32, 39), rgb(46, 46, 56))
        };

        draw_round_box(hdc, x, y, w, h, bg, border, s(10));

        SelectObject(hdc, fonts.body.into());
        SetTextColor(hdc, if is_active { rgb(255, 255, 255) } else { rgb(224, 226, 232) });
        draw_text(hdc, title, x + s(10), y + s(4), x + w - s(22), y + s(21), DT_LEFT.0 | DT_SINGLELINE.0);

        SelectObject(hdc, fonts.sub.into());
        SetTextColor(hdc, if is_active { rgb(125, 198, 255) } else { rgb(132, 136, 150) });
        draw_text(hdc, sub, x + s(10), y + s(21), x + w - s(8), y + s(37), DT_LEFT.0 | DT_SINGLELINE.0);

        if is_active {
            SetTextColor(hdc, rgb(59, 158, 255));
            draw_text(hdc, "✓", x + w - s(22), y + s(4), x + w - s(6), y + s(22), DT_CENTER.0 | DT_SINGLELINE.0);
        }
    }
}

unsafe fn draw_swatch(
    hdc: HDC,
    dpi: i32,
    cx: i32,
    cy: i32,
    color: COLORREF,
    is_hover: bool,
    is_selected: bool,
) {
    let s = |v: i32| scale(v, dpi);
    unsafe {
        if is_selected {
            draw_circle(hdc, cx, cy, s(15), rgb(24, 24, 28), rgb(59, 158, 255));
        } else if is_hover {
            draw_circle(hdc, cx, cy, s(15), rgb(24, 24, 28), rgb(130, 134, 148));
        }

        let inner_border = if is_selected {
            rgb(59, 158, 255)
        } else if is_hover {
            rgb(190, 194, 204)
        } else {
            rgb(76, 76, 88)
        };
        draw_circle(hdc, cx, cy, s(11), color, inner_border);
    }
}

unsafe fn draw_toggle_row(
    hdc: HDC,
    dpi: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: &str,
    sub: &str,
    active: bool,
    is_hover: bool,
    fonts: &Fonts,
) {
    let s = |v: i32| scale(v, dpi);
    unsafe {
        let bg = if is_hover { rgb(40, 40, 48) } else { rgb(30, 30, 37) };
        let border = if is_hover { rgb(64, 64, 76) } else { rgb(44, 44, 54) };
        draw_round_box(hdc, x, y, w, h, bg, border, s(8));

        SelectObject(hdc, fonts.body.into());
        SetTextColor(hdc, rgb(232, 234, 240));
        draw_text(hdc, title, x + s(12), y + s(4), x + w - s(54), y + s(20), DT_LEFT.0 | DT_SINGLELINE.0);

        SelectObject(hdc, fonts.sub.into());
        SetTextColor(hdc, rgb(132, 136, 150));
        draw_text(hdc, sub, x + s(12), y + s(20), x + w - s(54), y + s(35), DT_LEFT.0 | DT_SINGLELINE.0);

        draw_toggle(hdc, x + w - s(48), y + s(9), dpi, active);
    }
}

unsafe fn draw_action_btn(
    hdc: HDC,
    dpi: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: &str,
    is_hover: bool,
    base_bg: COLORREF,
    base_border: COLORREF,
    fonts: &Fonts,
) {
    let s = |v: i32| scale(v, dpi);
    unsafe {
        let bg = if is_hover {
            rgb(
                (((base_bg.0 & 0xFF) + 18).min(255)) as u8,
                ((((base_bg.0 >> 8) & 0xFF) + 18).min(255)) as u8,
                ((((base_bg.0 >> 16) & 0xFF) + 18).min(255)) as u8,
            )
        } else {
            base_bg
        };
        let border = if is_hover { rgb(95, 98, 114) } else { base_border };
        draw_round_box(hdc, x, y, w, h, bg, border, s(8));

        SelectObject(hdc, fonts.body.into());
        SetTextColor(hdc, rgb(236, 238, 244));
        draw_text(hdc, title, x, y, x + w, y + h, DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_zone_maps_to_an_action_or_close() {
        for &(zone, ..) in HIT_ZONES {
            assert!(zone == ZONE_CLOSE || zone_action(zone).is_some(), "zone {zone} does nothing");
        }
    }

    #[test]
    fn hit_test_samples() {
        assert_eq!(hit_test(66, 100), ZONE_PRESET_CLEAR);
        assert_eq!(hit_test(16, 86), ZONE_PRESET_CLEAR);
        assert_eq!(hit_test(120, 126), ZONE_PRESET_CLEAR);
        assert_eq!(hit_test(36, 220), ZONE_SWATCH_BLACK);
        assert_eq!(hit_test(20, 204), ZONE_SWATCH_BLACK);
        assert_eq!(hit_test(180, 515), ZONE_BTN_RESTART_SHELL);
        assert_eq!(hit_test(5, 5), -1);
    }

    #[test]
    fn dpi_scale_round_trip() {
        for dpi in [96, 120, 144, 192] {
            for dip in [16, 104, 220, 360, 552] {
                assert_eq!(unscale(scale(dip, dpi), dpi), dip);
            }
        }
    }

    #[test]
    fn swatch_click_carries_its_color() {
        let (action, lp) = zone_action(ZONE_SWATCH_NAVY).unwrap();
        assert_eq!(action, ACTION_SET_COLOR);
        assert_eq!(Color::from_abgr(lp as u32), Color::new(14, 30, 56, 255));
    }
}
