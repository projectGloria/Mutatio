use crate::config::{Color, TaskbarState};
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
use std::sync::Mutex;
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
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, IsWindowVisible, PostMessageW, RegisterClassW,
    SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SW_HIDE, SW_SHOW,
    WA_INACTIVE, WM_ACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_USER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

pub const WM_FLYOUT_ACTION: u32 = WM_USER + 200;

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
        }
    }
}

static FLYOUT_DATA: Mutex<Option<FlyoutState>> = Mutex::new(None);
static PARENT_NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);
static HOVER_INDEX: AtomicI32 = AtomicI32::new(-1);

pub struct FlyoutWindow {
    hwnd: HWND,
}

impl FlyoutWindow {
    pub fn create(parent_notify_hwnd: HWND) -> Self {
        unsafe {
            PARENT_NOTIFY_HWND.store(parent_notify_hwnd.0 as isize, Ordering::Relaxed);
            if let Ok(mut lock) = FLYOUT_DATA.lock() {
                *lock = Some(FlyoutState::default());
            }

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
        if let Ok(mut lock) = FLYOUT_DATA.lock() {
            *lock = Some(state);
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn toggle(&self, cursor_pt: POINT) {
        if self.is_visible() {
            self.hide();
        } else {
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
const ZONE_BTN_RELOAD: i32 = 31;
const ZONE_BTN_EXIT: i32 = 32;

fn hit_test(x: i32, y: i32) -> i32 {
    if x >= 310 && x <= 340 && y >= 10 && y <= 40 {
        return ZONE_CLOSE;
    }

    if y >= 65 && y <= 110 {
        if x >= 16 && x <= 116 {
            return ZONE_PRESET_CLEAR;
        }
        if x >= 125 && x <= 225 {
            return ZONE_PRESET_BLUR;
        }
        if x >= 234 && x <= 334 {
            return ZONE_PRESET_ACRYLIC;
        }
    }

    if y >= 118 && y <= 158 {
        if x >= 16 && x <= 170 {
            return ZONE_PRESET_OPAQUE;
        }
        if x >= 180 && x <= 334 {
            return ZONE_PRESET_DEFAULT;
        }
    }

    if y >= 195 && y <= 225 {
        if x >= 20 && x <= 50 {
            return ZONE_SWATCH_BLACK;
        }
        if x >= 65 && x <= 95 {
            return ZONE_SWATCH_SLATE;
        }
        if x >= 110 && x <= 140 {
            return ZONE_SWATCH_NAVY;
        }
        if x >= 155 && x <= 185 {
            return ZONE_SWATCH_CRIMSON;
        }
        if x >= 200 && x <= 230 {
            return ZONE_SWATCH_EMERALD;
        }
        if x >= 245 && x <= 275 {
            return ZONE_SWATCH_WHITE;
        }
    }

    if y >= 258 && y <= 292 && x >= 16 && x <= 334 {
        return ZONE_TOGGLE_DYNAMIC;
    }
    if y >= 296 && y <= 330 && x >= 16 && x <= 334 {
        return ZONE_TOGGLE_SMOOTH;
    }
    if y >= 334 && y <= 368 && x >= 16 && x <= 334 {
        return ZONE_TOGGLE_GAME;
    }
    if y >= 372 && y <= 406 && x >= 16 && x <= 334 {
        return ZONE_TOGGLE_BORDER;
    }

    if y >= 464 && y <= 500 {
        if x >= 16 && x <= 114 {
            return ZONE_BTN_CONFIG;
        }
        if x >= 120 && x <= 228 {
            return ZONE_BTN_RELOAD;
        }
        if x >= 234 && x <= 334 {
            return ZONE_BTN_EXIT;
        }
    }

    -1
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
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => LRESULT(0),
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let zone = hit_test(x, y);

                let parent_raw = PARENT_NOTIFY_HWND.load(Ordering::Relaxed);
                if parent_raw != 0 {
                    let parent = HWND(parent_raw as *mut _);
                    match zone {
                        ZONE_CLOSE => {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                        ZONE_PRESET_CLEAR => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_CLEAR), LPARAM(0));
                        }
                        ZONE_PRESET_BLUR => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_BLUR), LPARAM(0));
                        }
                        ZONE_PRESET_ACRYLIC => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_ACRYLIC), LPARAM(0));
                        }
                        ZONE_PRESET_OPAQUE => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_OPAQUE), LPARAM(0));
                        }
                        ZONE_PRESET_DEFAULT => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_DEFAULT), LPARAM(0));
                        }
                        ZONE_SWATCH_BLACK => {
                            let col = Color::new(0, 0, 0, 255).to_abgr() as isize;
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_COLOR), LPARAM(col));
                        }
                        ZONE_SWATCH_SLATE => {
                            let col = Color::new(24, 24, 28, 255).to_abgr() as isize;
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_COLOR), LPARAM(col));
                        }
                        ZONE_SWATCH_NAVY => {
                            let col = Color::new(14, 30, 56, 255).to_abgr() as isize;
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_COLOR), LPARAM(col));
                        }
                        ZONE_SWATCH_CRIMSON => {
                            let col = Color::new(50, 12, 18, 255).to_abgr() as isize;
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_COLOR), LPARAM(col));
                        }
                        ZONE_SWATCH_EMERALD => {
                            let col = Color::new(10, 42, 26, 255).to_abgr() as isize;
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_COLOR), LPARAM(col));
                        }
                        ZONE_SWATCH_WHITE => {
                            let col = Color::new(255, 255, 255, 255).to_abgr() as isize;
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_SET_COLOR), LPARAM(col));
                        }
                        ZONE_TOGGLE_DYNAMIC => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_TOGGLE_DYNAMIC), LPARAM(0));
                        }
                        ZONE_TOGGLE_SMOOTH => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_TOGGLE_SMOOTH), LPARAM(0));
                        }
                        ZONE_TOGGLE_GAME => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_TOGGLE_GAME_MODE), LPARAM(0));
                        }
                        ZONE_TOGGLE_BORDER => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_TOGGLE_BORDER), LPARAM(0));
                        }
                        ZONE_BTN_CONFIG => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_OPEN_CONFIG), LPARAM(0));
                        }
                        ZONE_BTN_RELOAD => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_RESTART_EXPLORER), LPARAM(0));
                        }
                        ZONE_BTN_EXIT => {
                            let _ = PostMessageW(Some(parent), WM_FLYOUT_ACTION, WPARAM(ACTION_EXIT), LPARAM(0));
                        }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let zone = hit_test(x, y);
                if zone != HOVER_INDEX.load(Ordering::Relaxed) {
                    HOVER_INDEX.store(zone, Ordering::Relaxed);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
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
        let state = if let Ok(lock) = FLYOUT_DATA.lock() {
            lock.clone().unwrap_or_default()
        } else {
            FlyoutState::default()
        };
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

        let font_title = CreateFontW(
            20, 0, 0, 0, FW_BOLD.0 as i32, 0, 0, 0, DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0,
            w!("Segoe UI Variable Display"),
        );
        let font_section = CreateFontW(
            12, 0, 0, 0, FW_BOLD.0 as i32, 0, 0, 0, DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0,
            w!("Segoe UI"),
        );
        let font_body = CreateFontW(
            14, 0, 0, 0, FW_SEMIBOLD.0 as i32, 0, 0, 0, DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0,
            w!("Segoe UI Variable Text"),
        );
        let font_sub = CreateFontW(
            11, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0,
            w!("Segoe UI"),
        );

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
        let mut status_txt: Vec<u16> = "● Win11 XAML Engine Active • v1.0\0".encode_utf16().collect();
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

        draw_swatch(hdc, 35, 196, rgb(0, 0, 0), "Black", hover == ZONE_SWATCH_BLACK, state.current_color == Color::new(0,0,0,255));
        draw_swatch(hdc, 80, 196, rgb(24, 24, 28), "Slate", hover == ZONE_SWATCH_SLATE, state.current_color == Color::new(24,24,28,255));
        draw_swatch(hdc, 125, 196, rgb(14, 30, 56), "Navy", hover == ZONE_SWATCH_NAVY, state.current_color == Color::new(14,30,56,255));
        draw_swatch(hdc, 170, 196, rgb(50, 12, 18), "Crimson", hover == ZONE_SWATCH_CRIMSON, state.current_color == Color::new(50,12,18,255));
        draw_swatch(hdc, 215, 196, rgb(10, 42, 26), "Emerald", hover == ZONE_SWATCH_EMERALD, state.current_color == Color::new(10,42,26,255));
        draw_swatch(hdc, 260, 196, rgb(255, 255, 255), "White", hover == ZONE_SWATCH_WHITE, state.current_color == Color::new(255,255,255,255));

        // SECTION 3: AUTOMATIONS & TOGGLES
        SelectObject(hdc, font_section.into());
        SetTextColor(hdc, rgb(140, 140, 150));
        let mut sec3_rc = RECT { left: 18, top: 236, right: 300, bottom: 252 };
        let mut sec3_txt: Vec<u16> = "SMART AUTOMATION\0".encode_utf16().collect();
        let len = sec3_txt.len();
        let _ = DrawTextW(hdc, &mut sec3_txt[..len - 1], &mut sec3_rc, DT_LEFT | DT_SINGLELINE);

        draw_toggle_row(hdc, 16, 258, 318, 34, "Dynamic Auto-Detect", "Adapt on maximize / start menu", state.dynamic_enabled, hover == ZONE_TOGGLE_DYNAMIC, font_body, font_sub);
        draw_toggle_row(hdc, 16, 296, 318, 34, "Smooth 60fps Animations", "Cubic fade transitions between states", state.smooth_enabled, hover == ZONE_TOGGLE_SMOOTH, font_body, font_sub);
        draw_toggle_row(hdc, 16, 334, 318, 34, "Fullscreen Game Sleep", "0% CPU pause in games (anti-cheat safe)", state.game_pause_enabled, hover == ZONE_TOGGLE_GAME, font_body, font_sub);
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
        draw_action_btn(hdc, 120, 464, 108, 34, "Restart Shell", hover == ZONE_BTN_RELOAD, rgb(40, 40, 48), font_body);
        draw_action_btn(hdc, 234, 464, 100, 34, "Exit Mutatio", hover == ZONE_BTN_EXIT, rgb(60, 25, 28), font_body);

        SelectObject(hdc, old_font);
        let _ = DeleteObject(font_title.into());
        let _ = DeleteObject(font_section.into());
        let _ = DeleteObject(font_body.into());
        let _ = DeleteObject(font_sub.into());
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
    _name: &str,
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
