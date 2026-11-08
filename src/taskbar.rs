use crate::config::{Color, StateAppearance, TaskbarState};
use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;
use windows::core::{w, BOOL, GUID, HRESULT};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::StationsAndDesktops::{
    OpenDesktopW, SetThreadDesktop, DESKTOP_CONTROL_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, ShowWindow, SW_HIDE, SW_SHOWNA,
};

// Embedded ExplorerTAP DLL for self-contained Windows 11 Modern Taskbar support
const EMBEDDED_EXPLORER_TAP: &[u8] = include_bytes!("../assets/ExplorerTAP.dll");

pub const IID_ITASKBAR_APPEARANCE_SERVICE: GUID =
    GUID::from_u128(0x5bcf9150_c28a_4ef2_913c_4c3ea2f5ead0);

#[repr(i32)]
#[derive(Debug, Clone, Copy)]
pub enum TaskbarBrush {
    Acrylic = 0,
    SolidColor = 1,
}

// COM vtable function pointer definitions
type FnSetTaskbarAppearance =
    unsafe extern "system" fn(*mut c_void, HWND, i32, u32) -> HRESULT;
type FnSetTaskbarBlur =
    unsafe extern "system" fn(*mut c_void, HWND, u32, f32) -> HRESULT;
type FnReturnTaskbarToDefaultAppearance =
    unsafe extern "system" fn(*mut c_void, HWND) -> HRESULT;
type FnSetTaskbarBorderVisibility =
    unsafe extern "system" fn(*mut c_void, HWND, BOOL) -> HRESULT;
type FnRestoreAllTaskbarsToDefault =
    unsafe extern "system" fn(*mut c_void) -> HRESULT;
type FnRelease =
    unsafe extern "system" fn(*mut c_void) -> u32;

pub struct TaskbarAppearanceService {
    p_unk: *mut c_void,
    fn_set_appearance: FnSetTaskbarAppearance,
    fn_set_blur: FnSetTaskbarBlur,
    fn_return_default: FnReturnTaskbarToDefaultAppearance,
    fn_set_border: FnSetTaskbarBorderVisibility,
    fn_restore_all: FnRestoreAllTaskbarsToDefault,
    fn_release: FnRelease,
}

unsafe impl Send for TaskbarAppearanceService {}
unsafe impl Sync for TaskbarAppearanceService {}

impl TaskbarAppearanceService {
    pub unsafe fn from_raw(p_unk: *mut c_void) -> Option<Self> {
        if p_unk.is_null() {
            return None;
        }

        unsafe {
            let vtable_ptr = *(p_unk as *const *const *const c_void);
            let vtable = std::slice::from_raw_parts(vtable_ptr, 10);

            let fn_release: FnRelease = std::mem::transmute(vtable[2]);
            let fn_set_appearance: FnSetTaskbarAppearance = std::mem::transmute(vtable[3]);
            let fn_set_blur: FnSetTaskbarBlur = std::mem::transmute(vtable[4]);
            let fn_return_default: FnReturnTaskbarToDefaultAppearance = std::mem::transmute(vtable[5]);
            let fn_set_border: FnSetTaskbarBorderVisibility = std::mem::transmute(vtable[6]);
            let fn_restore_all: FnRestoreAllTaskbarsToDefault = std::mem::transmute(vtable[7]);

            Some(Self {
                p_unk,
                fn_set_appearance,
                fn_set_blur,
                fn_return_default,
                fn_set_border,
                fn_restore_all,
                fn_release,
            })
        }
    }

    pub fn set_appearance(&self, taskbar: HWND, brush: TaskbarBrush, color: u32) -> HRESULT {
        unsafe { (self.fn_set_appearance)(self.p_unk, taskbar, brush as i32, color) }
    }

    pub fn set_blur(&self, taskbar: HWND, color: u32, blur_amount: f32) -> HRESULT {
        unsafe { (self.fn_set_blur)(self.p_unk, taskbar, color, blur_amount) }
    }

    pub fn return_to_default(&self, taskbar: HWND) -> HRESULT {
        unsafe { (self.fn_return_default)(self.p_unk, taskbar) }
    }

    pub fn set_border_visibility(&self, taskbar: HWND, visible: bool) -> HRESULT {
        unsafe { (self.fn_set_border)(self.p_unk, taskbar, BOOL::from(visible)) }
    }

    pub fn restore_all(&self) -> HRESULT {
        unsafe { (self.fn_restore_all)(self.p_unk) }
    }
}

impl Drop for TaskbarAppearanceService {
    fn drop(&mut self) {
        unsafe {
            let _ = self.restore_all();
            (self.fn_release)(self.p_unk);
        }
    }
}

// Undocumented SetWindowCompositionAttribute definitions (Win10 fallback)
pub const WCA_ACCENT_POLICY: u32 = 19;
pub const ACCENT_DISABLED: u32 = 0;
pub const ACCENT_ENABLE_GRADIENT: u32 = 1;
pub const ACCENT_ENABLE_TRANSPARENTGRADIENT: u32 = 2;
pub const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
pub const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AccentPolicy {
    pub accent_state: u32,
    pub accent_flags: u32,
    pub gradient_color: u32,
    pub animation_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WindowCompositionAttribData {
    pub attrib: u32,
    pub pv_data: *mut c_void,
    pub cb_data: usize,
}

type PfnSetWindowCompositionAttribute =
    unsafe extern "system" fn(HWND, *const WindowCompositionAttribData) -> BOOL;

type PfnInjectExplorerTAP =
    unsafe extern "C" fn(HWND, *const GUID, *mut *mut c_void) -> HRESULT;

pub struct TaskbarController {
    pfn_swca: Option<PfnSetWindowCompositionAttribute>,
    tap_service: Option<TaskbarAppearanceService>,
}

#[derive(Debug, Clone)]
pub struct TaskbarInfo {
    pub main_hwnd: HWND,
    pub worker_w: Option<HWND>,
    #[allow(dead_code)]
    pub is_secondary: bool,
}

impl TaskbarController {
    pub fn new() -> Self {
        unsafe {
            // Ensure thread is attached to user's interactive Default desktop
            if let Ok(hdesk) = OpenDesktopW(
                w!("Default"),
                DESKTOP_CONTROL_FLAGS(0),
                false,
                0x10000000,
            ) {
                let _ = SetThreadDesktop(hdesk);
            }

            // Initialize COM for this thread
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }

        let pfn_swca = unsafe {
            let user32 = LoadLibraryW(w!("user32.dll")).ok();
            if let Some(hmod) = user32 {
                let proc = GetProcAddress(hmod, windows::core::s!("SetWindowCompositionAttribute"));
                proc.map(|p| std::mem::transmute::<_, PfnSetWindowCompositionAttribute>(p))
            } else {
                None
            }
        };

        let mut tap_service = None;

        // Try initializing Windows 11 Modern XAML Taskbar TAP Service
        if let Some(service) = Self::init_tap_service() {
            println!("[+] Windows 11 Modern Taskbar XAML Engine connected successfully");
            tap_service = Some(service);
        } else {
            println!("[*] Using Native DWM / SWCA fallback engine");
        }

        Self {
            pfn_swca,
            tap_service,
        }
    }

    fn init_tap_service() -> Option<TaskbarAppearanceService> {
        let primary_hwnd = unsafe {
            let h = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
            if h.0 as usize == 0 {
                return None;
            }
            h
        };

        // Ensure ExplorerTAP.dll exists in LocalAppData
        let tap_dll_path = Self::ensure_tap_dll()?;

        unsafe {
            let path_w: Vec<u16> = tap_dll_path
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let h_module = LoadLibraryW(windows::core::PCWSTR(path_w.as_ptr())).ok()?;
            let proc_inject = GetProcAddress(h_module, windows::core::s!("InjectExplorerTAP"))?;
            let pfn_inject: PfnInjectExplorerTAP = std::mem::transmute(proc_inject);

            let mut p_service: *mut c_void = std::ptr::null_mut();
            let hr = pfn_inject(
                primary_hwnd,
                &IID_ITASKBAR_APPEARANCE_SERVICE,
                &mut p_service,
            );

            if hr.is_ok() && !p_service.is_null() {
                TaskbarAppearanceService::from_raw(p_service)
            } else {
                None
            }
        }
    }

    fn ensure_tap_dll() -> Option<PathBuf> {
        let base_dir = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));

        let tap_dir = base_dir.join("Mutatio");
        let _ = fs::create_dir_all(&tap_dir);
        let tap_path = tap_dir.join("ExplorerTAP.dll");

        let need_write = if tap_path.exists() {
            fs::metadata(&tap_path)
                .map(|m| m.len() != EMBEDDED_EXPLORER_TAP.len() as u64)
                .unwrap_or(true)
        } else {
            true
        };

        if need_write {
            if fs::write(&tap_path, EMBEDDED_EXPLORER_TAP).is_err() {
                return None;
            }
        }

        Some(tap_path)
    }

    /// Finds all primary and secondary taskbars
    pub fn find_taskbars(&self) -> Vec<TaskbarInfo> {
        let mut taskbars = Vec::new();

        unsafe {
            // 1. Primary taskbar
            let primary = FindWindowW(w!("Shell_TrayWnd"), None);
            let hwnd = match primary {
                Ok(h) if h.0 as usize != 0 => Some(h),
                _ => {
                    if let Ok(hdesk) = OpenDesktopW(
                        w!("Default"),
                        DESKTOP_CONTROL_FLAGS(0),
                        false,
                        0x10000000,
                    ) {
                        if SetThreadDesktop(hdesk).is_ok() {
                            FindWindowW(w!("Shell_TrayWnd"), None)
                                .ok()
                                .filter(|h| h.0 as usize != 0)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            };

            if let Some(h) = hwnd {
                taskbars.push(TaskbarInfo {
                    main_hwnd: h,
                    worker_w: None,
                    is_secondary: false,
                });
            }

            // 2. Secondary taskbars
            let mut sec = HWND::default();
            loop {
                let next_sec = FindWindowExW(None, Some(sec), w!("Shell_SecondaryTrayWnd"), None);
                match next_sec {
                    Ok(h) if h.0 as usize != 0 => {
                        taskbars.push(TaskbarInfo {
                            main_hwnd: h,
                            worker_w: None,
                            is_secondary: true,
                        });
                        sec = h;
                    }
                    _ => break,
                }
            }
        }

        taskbars
    }

    /// Applies state appearance to all found taskbars
    pub fn apply_to_all(&self, appearance: &StateAppearance) {
        let taskbars = self.find_taskbars();
        for tb in &taskbars {
            self.apply_to_taskbar(tb, appearance);
        }
    }

    /// Applies appearance to a single taskbar
    pub fn apply_to_taskbar(&self, tb: &TaskbarInfo, appearance: &StateAppearance) {
        // Modern Windows 11 XAML Engine
        if let Some(ref service) = self.tap_service {
            match appearance.state {
                TaskbarState::Normal => {
                    let _ = service.return_to_default(tb.main_hwnd);
                }
                TaskbarState::Clear => {
                    let abgr = appearance.color.to_abgr();
                    let _ = service.set_appearance(tb.main_hwnd, TaskbarBrush::SolidColor, abgr);
                }
                TaskbarState::Opaque => {
                    let mut col = appearance.color;
                    if (col.r == 24 && col.g == 24 && col.b == 24) || (col.r == 0 && col.g == 0 && col.b == 0) {
                        col = Color::new(0, 0, 0, 255);
                    }
                    col.a = 255;
                    let _ = service.set_appearance(tb.main_hwnd, TaskbarBrush::SolidColor, col.to_abgr());
                }
                TaskbarState::Blur => {
                    let abgr = appearance.color.to_abgr();
                    let _ = service.set_blur(tb.main_hwnd, abgr, 10.0);
                }
                TaskbarState::Acrylic => {
                    let mut col = appearance.color;
                    if col.a == 0 {
                        col.a = 1;
                    }
                    let _ = service.set_appearance(tb.main_hwnd, TaskbarBrush::Acrylic, col.to_abgr());
                }
            }

            let _ = service.set_border_visibility(tb.main_hwnd, appearance.show_line);
            return;
        }

        // Fallback for Windows 10 & ExplorerPatcher / Classic Taskbar
        unsafe {
            let (accent_state, accent_flags, color_val) = match appearance.state {
                TaskbarState::Normal => (ACCENT_DISABLED, 0, 0),
                TaskbarState::Clear => {
                    let abgr = appearance.color.to_abgr();
                    (ACCENT_ENABLE_TRANSPARENTGRADIENT, 2, abgr)
                }
                TaskbarState::Opaque => {
                    let mut col = appearance.color;
                    if (col.r == 24 && col.g == 24 && col.b == 24) || (col.r == 0 && col.g == 0 && col.b == 0) {
                        col = Color::new(0, 0, 0, 255);
                    }
                    col.a = 255;
                    (ACCENT_ENABLE_GRADIENT, 0, col.to_abgr())
                }
                TaskbarState::Blur => {
                    let abgr = appearance.color.to_abgr();
                    (ACCENT_ENABLE_BLURBEHIND, 0, abgr)
                }
                TaskbarState::Acrylic => {
                    let mut col = appearance.color;
                    if col.a == 0 {
                        col.a = 1;
                    }
                    (ACCENT_ENABLE_ACRYLICBLURBEHIND, 0, col.to_abgr())
                }
            };

            let mut policy = AccentPolicy {
                accent_state,
                accent_flags,
                gradient_color: color_val,
                animation_id: 0,
            };

            let data = WindowCompositionAttribData {
                attrib: WCA_ACCENT_POLICY,
                pv_data: &mut policy as *mut _ as *mut c_void,
                cb_data: std::mem::size_of::<AccentPolicy>(),
            };

            if let Some(swca) = self.pfn_swca {
                let _ = swca(tb.main_hwnd, &data);
            }

            let backdrop_type: u32 = match appearance.state {
                TaskbarState::Normal => 0,
                TaskbarState::Clear => 1,
                TaskbarState::Opaque => 1,
                TaskbarState::Blur => 2,
                TaskbarState::Acrylic => 3,
            };

            let _ = DwmSetWindowAttribute(
                tb.main_hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop_type as *const _ as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );

            let border_color: u32 = if appearance.show_line {
                0xFFFFFFFF
            } else {
                0xFFFFFFFE
            };

            let _ = DwmSetWindowAttribute(
                tb.main_hwnd,
                DWMWA_BORDER_COLOR,
                &border_color as *const _ as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );

            if let Some(worker) = tb.worker_w {
                let _ = ShowWindow(worker, if appearance.show_line { SW_SHOWNA } else { SW_HIDE });
            }
        }
    }

    /// Restores all taskbars to standard Windows appearance
    pub fn restore_all(&self) {
        if let Some(ref service) = self.tap_service {
            let _ = service.restore_all();
            return;
        }

        let normal = StateAppearance {
            state: TaskbarState::Normal,
            color: Color::transparent(),
            show_line: true,
        };
        self.apply_to_all(&normal);
    }
}

impl Drop for TaskbarController {
    fn drop(&mut self) {
        self.restore_all();
        unsafe {
            CoUninitialize();
        }
    }
}
