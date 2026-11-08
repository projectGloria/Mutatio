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
    GetMonitorInfoW, GetStockObject, InvalidateRect, MonitorFromPoint, RoundRect, SelectObject,
    SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DT_CENTER,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, NULL_BRUSH, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID,
    SRCCOPY, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, IsWindowVisible, PostMessageW, RegisterClassW,
    SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SW_HIDE, SW_SHOW,
    WA_INACTIVE, WM_ACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_USER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

pub const WM_FLYOUT_ACTION: u32 = WM_USER + 200;

// Lives in Win32_UI_Controls in windows-rs; not worth a whole feature for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;

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

pub const FLYOUT_WIDTH: i32 = 350;
pub const FLYOUT_HEIGHT: i32 = 530;

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
// created the window), so thread_local + Cell/RefCell replaces the old global Mutex.
thread_local! {
    static FLYOUT_DATA: RefCell<FlyoutState> = RefCell::new(FlyoutState::default());
    static FONTS: Fonts = Fonts::new();
    /// When the flyout last auto-hid on deactivation; see `FlyoutWindow::toggle`.
    static LAST_AUTO_HIDE: Cell<Option<Instant>> = const { Cell::new(None) };
}
static PARENT_NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);
static HOVER_INDEX: AtomicI32 = AtomicI32::new(-1);

/// GDI fonts created once. The old renderer created and destroyed four fonts on every
/// WM_PAINT — and every mouse move over the flyout triggers a repaint.
struct Fonts {
    title: HFONT,
    section: HFONT,
    body: HFONT,
    sub: HFONT,
}

impl Fonts {
    fn new() -> Self {
        let make = |height: i32, weight: i32, face: windows::core::PCWSTR| unsafe {
            CreateFontW(
                height, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, face,
            )
        };
        Self {
            title: make(20, FW_BOLD.0 as i32, w!("Segoe UI Variable Display")),
            section: make(12, FW_BOLD.0 as i32, w!("Segoe UI")),
            body: make(14, FW_SEMIBOLD.0 as i32, w!("Segoe UI Variable Text")),
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
            let wnd_class = WNDCLASSW {
                lpfnWndProc: Some(flyout_wnd_proc),
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
        // No point repainting a hidden window; show_at() invalidates anyway.
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

            let half_w = FLYOUT_WIDTH / 2;
            let x = (pt.x - half_w).clamp(work.left + 12, work.right - FLYOUT_WIDTH - 12);

            let y = if pt.y < work.top + 100 {
                work.top + 12
            } else {
                work.bottom - FLYOUT_HEIGHT - 12
            };

            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                FLYOUT_WIDTH,
                FLYOUT_HEIGHT,
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

/// Clickable regions as (zone, left, top, right, bottom), inclusive. Must match the geometry
/// drawn in `render_flyout`.
const HIT_ZONES: &[(i32, i32, i32, i32, i32)] = &[
    (ZONE_CLOSE, 310, 10, 340, 40),
    (ZONE_PRESET_CLEAR, 16, 65, 116, 110),
    (ZONE_PRESET_BLUR, 125, 65, 225, 110),
    (ZONE_PRESET_ACRYLIC, 234, 65, 334, 110),
    (ZONE_PRESET_OPAQUE, 16, 118, 170, 158),
    (ZONE_PRESET_DEFAULT, 180, 118, 334, 158),
    (ZONE_SWATCH_BLACK, 20, 195, 50, 225),
    (ZONE_SWATCH_SLATE, 65, 195, 95, 225),
    (ZONE_SWATCH_NAVY, 110, 195, 140, 225),
    (ZONE_SWATCH_CRIMSON, 155, 195, 185, 225),
    (ZONE_SWATCH_EMERALD, 200, 195, 230, 225),
    (ZONE_SWATCH_WHITE, 245, 195, 275, 225),
    (ZONE_TOGGLE_DYNAMIC, 16, 258, 334, 292),
    (ZONE_TOGGLE_SMOOTH, 16, 296, 334, 330),
    (ZONE_TOGGLE_GAME, 16, 334, 334, 368),
    (ZONE_TOGGLE_BORDER, 16, 372, 334, 406),
    (ZONE_BTN_CONFIG, 16, 464, 114, 500),
    (ZONE_BTN_RESTART_SHELL, 120, 464, 228, 500),
    (ZONE_BTN_EXIT, 234, 464, 334, 500),
];

fn hit_test(x: i32, y: i32) -> i32 {
    HIT_ZONES
        .iter()
        .find(|&&(_, l, t, r, b)| (l..=r).contains(&x) && (t..=b).contains(&y))
        .map_or(-1, |&(zone, ..)| zone)
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

                render_flyout(mem_dc, w, h);

                let _ = BitBlt(hdc, 0, 0, w, h, Some(mem_dc), 0, 0, SRCCOPY);

                SelectObject(mem_dc, old_bm);
                let _ = DeleteObject(mem_bm.into());
                let _ = DeleteDC(mem_dc);

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
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
                // Without TrackMouseEvent the last hovered item stayed highlighted forever.
                if HOVER_INDEX.swap(-1, Ordering::Relaxed) != -1 {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => LRESULT(0),
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let zone = hit_test(x, y);

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
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let zone = hit_test(x, y);
                if HOVER_INDEX.swap(zone, Ordering::Relaxed) != zone {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                // Re-arm every move: TME_LEAVE is one-shot and cheap to request.
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

unsafe fn draw_toggle(hdc: HDC, x: i32, y: i32, active: bool) {
    unsafe {
        let w = 40;
        let h = 20;
        let bg = if active { rgb(0, 120, 212) } else { rgb(55, 55, 60) };
        let border = if active { rgb(0, 120, 212) } else { rgb(75, 75, 80) };
        draw_round_box(hdc, x, y, w, h, bg, border, 20);

        let thumb_x = if active { x + 29 } else { x + 11 };
        let thumb_y = y + 10;
        draw_circle(hdc, thumb_x, thumb_y, 7, rgb(255, 255, 255), rgb(255, 255, 255));
    }
}

unsafe fn render_flyout(hdc: HDC, width: i32, height: i32) {
    unsafe {
        let state = FLYOUT_DATA.with(|d| d.borrow().clone());
        let (font_title, font_section, font_body, font_sub) =
            FONTS.with(|f| (f.title, f.section, f.body, f.sub));
        let hover = HOVER_INDEX.load(Ordering::Relaxed);

        let bg_brush = CreateSolidBrush(rgb(25, 25, 28));
        let full_rc = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        FillRect(hdc, &full_rc, bg_brush);
        let _ = DeleteObject(bg_brush.into());

        let border_pen = CreatePen(PS_SOLID, 1, rgb(45, 45, 52));
        let old_pen = SelectObject(hdc, border_pen.into());
        let null_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        let _ = RoundRect(hdc, 0, 0, width, height, 16, 16);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, null_brush);
        let _ = DeleteObject(border_pen.into());

        SetBkMode(hdc, TRANSPARENT);

        // HEADER
        let old_font = SelectObject(hdc, font_title.into());
        SetTextColor(hdc, rgb(255, 255, 255));
        let mut title_rc = RECT { left: 18, top: 14, right: 200, bottom: 38 };
        let mut title_txt: Vec<u16> = "Mutatio\0".encode_utf16().collect();
        let len = title_txt.len();
        let _ = DrawTextW(hdc, &mut title_txt[..len - 1], &mut title_rc, DT_LEFT | DT_SINGLELINE);

        SelectObject(hdc, font_sub.into());
        SetTextColor(hdc, rgb(52, 211, 153));
        let mut status_rc = RECT { left: 18, top: 36, right: 300, bottom: 52 };
        let status = format!("● {} engine active • v{}\0", state.engine, env!("CARGO_PKG_VERSION"));
        let mut status_txt: Vec<u16> = status.encode_utf16().collect();
        let len = status_txt.len();
        let _ = DrawTextW(hdc, &mut status_txt[..len - 1], &mut status_rc, DT_LEFT | DT_SINGLELINE);

        // Close button (✕)
        let close_hover = hover == ZONE_CLOSE;
        let close_bg = if close_hover { rgb(60, 60, 68) } else { rgb(38, 38, 44) };
        draw_round_box(hdc, 312, 14, 24, 24, close_bg, rgb(55, 55, 62), 12);
        SetTextColor(hdc, rgb(220, 220, 225));
        SelectObject(hdc, font_body.into());
        let mut close_rc = RECT { left: 312, top: 14, right: 336, bottom: 38 };
        let mut close_txt: Vec<u16> = "✕\0".encode_utf16().collect();
        let len = close_txt.len();
        let _ = DrawTextW(hdc, &mut close_txt[..len - 1], &mut close_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

        // SECTION 1: APPEARANCE PRESET
        SelectObject(hdc, font_section.into());
        SetTextColor(hdc, rgb(140, 140, 150));
        let mut sec1_rc = RECT { left: 18, top: 58, right: 300, bottom: 74 };
        let mut sec1_txt: Vec<u16> = "TASKBAR APPEARANCE\0".encode_utf16().collect();
        let len = sec1_txt.len();
        let _ = DrawTextW(hdc, &mut sec1_txt[..len - 1], &mut sec1_rc, DT_LEFT | DT_SINGLELINE);

        let is_clear = state.active_state == TaskbarState::Clear;
        let is_blur = state.active_state == TaskbarState::Blur;
        let is_acrylic = state.active_state == TaskbarState::Acrylic;
        let is_opaque = state.active_state == TaskbarState::Opaque;
        let is_default = state.active_state == TaskbarState::Normal;

        draw_card(hdc, 16, 76, 100, 36, "Clear", "Transparent", is_clear, hover == ZONE_PRESET_CLEAR, font_body, font_sub);
        draw_card(hdc, 125, 76, 100, 36, "Blur", "Aero glass", is_blur, hover == ZONE_PRESET_BLUR, font_body, font_sub);
        draw_card(hdc, 234, 76, 100, 36, "Acrylic", "Fluent blur", is_acrylic, hover == ZONE_PRESET_ACRYLIC, font_body, font_sub);
        draw_card(hdc, 16, 120, 154, 36, "Opaque Black", "Solid black (#000000)", is_opaque, hover == ZONE_PRESET_OPAQUE, font_body, font_sub);
        draw_card(hdc, 180, 120, 154, 36, "Windows Default", "Restore stock", is_default, hover == ZONE_PRESET_DEFAULT, font_body, font_sub);

        // SECTION 2: ACCENT COLOR PRESETS
        SelectObject(hdc, font_section.into());
        SetTextColor(hdc, rgb(140, 140, 150));
        let mut sec2_rc = RECT { left: 18, top: 168, right: 300, bottom: 184 };
        let mut sec2_txt: Vec<u16> = "QUICK COLOR TINT\0".encode_utf16().collect();
        let len = sec2_txt.len();
        let _ = DrawTextW(hdc, &mut sec2_txt[..len - 1], &mut sec2_rc, DT_LEFT | DT_SINGLELINE);

        for (i, (zone, color)) in SWATCHES.iter().enumerate() {
            let cx = 35 + 45 * i as i32;
            let fill = rgb(color.r, color.g, color.b);
            draw_swatch(hdc, cx, 196, fill, hover == *zone, state.current_color == *color);
        }

        // SECTION 3: AUTOMATIONS & TOGGLES
        SelectObject(hdc, font_section.into());
        SetTextColor(hdc, rgb(140, 140, 150));
        let mut sec3_rc = RECT { left: 18, top: 236, right: 300, bottom: 252 };
        let mut sec3_txt: Vec<u16> = "SMART AUTOMATION\0".encode_utf16().collect();
        let len = sec3_txt.len();
        let _ = DrawTextW(hdc, &mut sec3_txt[..len - 1], &mut sec3_rc, DT_LEFT | DT_SINGLELINE);

        draw_toggle_row(hdc, 16, 258, 318, 34, "Dynamic Auto-Detect", "Adapt on maximize / start menu", state.dynamic_enabled && !state.is_forced, hover == ZONE_TOGGLE_DYNAMIC, font_body, font_sub);
        draw_toggle_row(hdc, 16, 296, 318, 34, "Smooth 60fps Animations", "Cubic fade transitions between states", state.smooth_enabled, hover == ZONE_TOGGLE_SMOOTH, font_body, font_sub);
        draw_toggle_row(hdc, 16, 334, 318, 34, "Fullscreen Game Sleep", "Stock taskbar while a fullscreen app runs", state.game_pause_enabled, hover == ZONE_TOGGLE_GAME, font_body, font_sub);
        draw_toggle_row(hdc, 16, 372, 318, 34, "Taskbar Top Border Line", "Subtle accent line on top of bar", state.show_border, hover == ZONE_TOGGLE_BORDER, font_body, font_sub);

        // Status badge
        let active_hint_bg = rgb(32, 32, 38);
        draw_round_box(hdc, 16, 414, 318, 36, active_hint_bg, rgb(45, 45, 54), 8);
        SelectObject(hdc, font_sub.into());
        SetTextColor(hdc, rgb(170, 170, 185));
        let mut hint_rc = RECT { left: 24, top: 416, right: 326, bottom: 448 };
        let hint_str = if state.is_forced {
            "Status: [Forced Preset Active] (Dynamic overridden)\0"
        } else if state.dynamic_enabled {
            "Status: [Dynamic Active] Auto-adapting to active windows\0"
        } else {
            "Status: [Fixed Desktop Appearance]\0"
        };
        let mut hint_txt: Vec<u16> = hint_str.encode_utf16().collect();
        let len = hint_txt.len();
        let _ = DrawTextW(hdc, &mut hint_txt[..len - 1], &mut hint_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // SECTION 4: FOOTER ACTIONS
        draw_action_btn(hdc, 16, 464, 98, 34, "Config JSON", hover == ZONE_BTN_CONFIG, rgb(40, 40, 48), font_body);
        draw_action_btn(hdc, 120, 464, 108, 34, "Restart Shell", hover == ZONE_BTN_RESTART_SHELL, rgb(40, 40, 48), font_body);
        draw_action_btn(hdc, 234, 464, 100, 34, "Exit Mutatio", hover == ZONE_BTN_EXIT, rgb(60, 25, 28), font_body);

        SelectObject(hdc, old_font);
    }
}

unsafe fn draw_card(
    hdc: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: &str,
    sub: &str,
    is_active: bool,
    is_hover: bool,
    font_body: HFONT,
    font_sub: HFONT,
) {
    unsafe {
        let (bg, border) = if is_active {
            (rgb(18, 48, 80), rgb(0, 120, 212))
        } else if is_hover {
            (rgb(42, 42, 50), rgb(70, 70, 80))
        } else {
            (rgb(32, 32, 38), rgb(48, 48, 56))
        };

        draw_round_box(hdc, x, y, w, h, bg, border, 10);

        SelectObject(hdc, font_body.into());
        SetTextColor(hdc, if is_active { rgb(255, 255, 255) } else { rgb(220, 220, 225) });
        let mut title_rc = RECT { left: x + 8, top: y + 3, right: x + w - 8, bottom: y + 20 };
        let mut title_txt: Vec<u16> = format!("{}\0", title).encode_utf16().collect();
        let len = title_txt.len();
        let _ = DrawTextW(hdc, &mut title_txt[..len - 1], &mut title_rc, DT_LEFT | DT_SINGLELINE);

        SelectObject(hdc, font_sub.into());
        SetTextColor(hdc, if is_active { rgb(90, 185, 255) } else { rgb(130, 130, 140) });
        let mut sub_rc = RECT { left: x + 8, top: y + 19, right: x + w - 8, bottom: y + 34 };
        let mut sub_txt: Vec<u16> = format!("{}\0", sub).encode_utf16().collect();
        let len = sub_txt.len();
        let _ = DrawTextW(hdc, &mut sub_txt[..len - 1], &mut sub_rc, DT_LEFT | DT_SINGLELINE);

        if is_active {
            let mut chk_rc = RECT { left: x + w - 20, top: y + 4, right: x + w - 4, bottom: y + 20 };
            let mut chk_txt: Vec<u16> = "✓\0".encode_utf16().collect();
            let len = chk_txt.len();
            SetTextColor(hdc, rgb(0, 150, 255));
            let _ = DrawTextW(hdc, &mut chk_txt[..len - 1], &mut chk_rc, DT_CENTER | DT_SINGLELINE);
        }
    }
}

unsafe fn draw_swatch(
    hdc: HDC,
    cx: i32,
    cy: i32,
    color: COLORREF,
    is_hover: bool,
    is_selected: bool,
) {
    unsafe {
        let border = if is_selected {
            rgb(0, 120, 212)
        } else if is_hover {
            rgb(180, 180, 190)
        } else {
            rgb(70, 70, 80)
        };

        if is_selected {
            draw_circle(hdc, cx, cy, 14, rgb(25, 25, 28), border);
        }

        draw_circle(hdc, cx, cy, 11, color, border);
    }
}

unsafe fn draw_toggle_row(
    hdc: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: &str,
    sub: &str,
    active: bool,
    is_hover: bool,
    font_body: HFONT,
    font_sub: HFONT,
) {
    unsafe {
        let bg = if is_hover { rgb(38, 38, 46) } else { rgb(28, 28, 34) };
        let border = if is_hover { rgb(60, 60, 70) } else { rgb(42, 42, 50) };
        draw_round_box(hdc, x, y, w, h, bg, border, 8);

        SelectObject(hdc, font_body.into());
        SetTextColor(hdc, rgb(230, 230, 235));
        let mut title_rc = RECT { left: x + 10, top: y + 3, right: x + w - 50, bottom: y + 19 };
        let mut title_txt: Vec<u16> = format!("{}\0", title).encode_utf16().collect();
        let len = title_txt.len();
        let _ = DrawTextW(hdc, &mut title_txt[..len - 1], &mut title_rc, DT_LEFT | DT_SINGLELINE);

        SelectObject(hdc, font_sub.into());
        SetTextColor(hdc, rgb(130, 130, 145));
        let mut sub_rc = RECT { left: x + 10, top: y + 18, right: x + w - 50, bottom: y + 32 };
        let mut sub_txt: Vec<u16> = format!("{}\0", sub).encode_utf16().collect();
        let len = sub_txt.len();
        let _ = DrawTextW(hdc, &mut sub_txt[..len - 1], &mut sub_rc, DT_LEFT | DT_SINGLELINE);

        draw_toggle(hdc, x + w - 48, y + 7, active);
    }
}

unsafe fn draw_action_btn(
    hdc: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    title: &str,
    is_hover: bool,
    base_bg: COLORREF,
    font_body: HFONT,
) {
    unsafe {
        let bg = if is_hover {
            rgb(
                (((base_bg.0 & 0xFF) + 20).min(255)) as u8,
                ((((base_bg.0 >> 8) & 0xFF) + 20).min(255)) as u8,
                ((((base_bg.0 >> 16) & 0xFF) + 20).min(255)) as u8,
            )
        } else {
            base_bg
        };
        let border = if is_hover { rgb(80, 80, 95) } else { rgb(52, 52, 62) };
        draw_round_box(hdc, x, y, w, h, bg, border, 8);

        SelectObject(hdc, font_body.into());
        SetTextColor(hdc, rgb(235, 235, 240));
        let mut rc = RECT { left: x, top: y, right: x + w, bottom: y + h };
        let mut txt: Vec<u16> = format!("{}\0", title).encode_utf16().collect();
        let len = txt.len();
        let _ = DrawTextW(hdc, &mut txt[..len - 1], &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
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
        assert_eq!(hit_test(66, 90), ZONE_PRESET_CLEAR);
        assert_eq!(hit_test(35, 210), ZONE_SWATCH_BLACK);
        assert_eq!(hit_test(170, 480), ZONE_BTN_RESTART_SHELL);
        assert_eq!(hit_test(5, 5), -1);
    }

    #[test]
    fn swatch_click_carries_its_color() {
        let (action, lp) = zone_action(ZONE_SWATCH_NAVY).unwrap();
        assert_eq!(action, ACTION_SET_COLOR);
        assert_eq!(Color::from_abgr(lp as u32), Color::new(14, 30, 56, 255));
    }
}
