use crate::config::{StateAppearance, TaskbarState};
use std::cell::Cell;
use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;
use windows::core::{w, BOOL, GUID, HRESULT};
use windows::Win32::Foundation::{FreeLibrary, HMODULE, HWND};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, OpenDesktopW, SetThreadDesktop, DESKTOP_CONTROL_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW};

// Embedded ExplorerTAP DLL for self-contained Windows 11 Modern Taskbar support
const EMBEDDED_EXPLORER_TAP: &[u8] = include_bytes!("../assets/ExplorerTAP.dll");

pub const IID_ITASKBAR_APPEARANCE_SERVICE: GUID =
    GUID::from_u128(0x5bcf9150_c28a_4ef2_913c_4c3ea2f5ead0);

/// HRESULTs a COM proxy returns once the server process (explorer.exe) is gone.
/// Seeing one means our `ITaskbarAppearanceService` pointer is dead and must be rebuilt.
const DEAD_SERVER_HRESULTS: [u32; 6] = [
    0x8001_0108, // RPC_E_DISCONNECTED
    0x8001_0007, // RPC_E_SERVER_DIED
    0x8001_0012, // RPC_E_SERVER_DIED_DNE
    0x8007_06BA, // RPC_S_SERVER_UNAVAILABLE
    0x8007_06BE, // RPC_S_CALL_FAILED
    0x8004_01FD, // CO_E_OBJNOTCONNECTED
];

fn is_dead_server(hr: HRESULT) -> bool {
    DEAD_SERVER_HRESULTS.contains(&(hr.0 as u32))
}

#[repr(i32)]
#[derive(Debug, Clone, Copy)]
pub enum TaskbarBrush {
    Acrylic = 0,
    SolidColor = 1,
}

// COM vtable function pointer definitions
type FnSetTaskbarAppearance = unsafe extern "system" fn(*mut c_void, HWND, i32, u32) -> HRESULT;
type FnSetTaskbarBlur = unsafe extern "system" fn(*mut c_void, HWND, u32, f32) -> HRESULT;
type FnReturnTaskbarToDefaultAppearance = unsafe extern "system" fn(*mut c_void, HWND) -> HRESULT;
type FnSetTaskbarBorderVisibility = unsafe extern "system" fn(*mut c_void, HWND, BOOL) -> HRESULT;
type FnRestoreAllTaskbarsToDefault = unsafe extern "system" fn(*mut c_void) -> HRESULT;
type FnRelease = unsafe extern "system" fn(*mut c_void) -> u32;

/// Hand-rolled binding of ExplorerTAP's `ITaskbarAppearanceService`.
/// Vtable slot order MUST match the DLL's IDL exactly — nothing type-checks this.
pub struct TaskbarAppearanceService {
    p_unk: *mut c_void,
    fn_set_appearance: FnSetTaskbarAppearance,
    fn_set_blur: FnSetTaskbarBlur,
    fn_return_default: FnReturnTaskbarToDefaultAppearance,
    fn_set_border: FnSetTaskbarBorderVisibility,
    fn_restore_all: FnRestoreAllTaskbarsToDefault,
    fn_release: FnRelease,
}

impl TaskbarAppearanceService {
    /// # Safety
    /// `p_unk` must be a live `ITaskbarAppearanceService*` whose reference we now own.
    pub unsafe fn from_raw(p_unk: *mut c_void) -> Option<Self> {
        if p_unk.is_null() {
            return None;
        }

        unsafe {
            let vtable = *(p_unk as *const *const *const c_void);
            // Read only the slots we use. The old code built a 10-entry slice; a proxy vtable is
            // exactly as long as the interface, so that read past its end.
            let slot = |i: usize| *vtable.add(i);
            Some(Self {
                p_unk,
                fn_release: std::mem::transmute::<*const c_void, FnRelease>(slot(2)),
                fn_set_appearance: std::mem::transmute::<*const c_void, FnSetTaskbarAppearance>(slot(3)),
                fn_set_blur: std::mem::transmute::<*const c_void, FnSetTaskbarBlur>(slot(4)),
                fn_return_default: std::mem::transmute::<*const c_void, FnReturnTaskbarToDefaultAppearance>(slot(5)),
                fn_set_border: std::mem::transmute::<*const c_void, FnSetTaskbarBorderVisibility>(slot(6)),
                fn_restore_all: std::mem::transmute::<*const c_void, FnRestoreAllTaskbarsToDefault>(slot(7)),
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
        // Only Release here. Restoring is an explicit decision of the controller: on reconnect
        // the server is already dead, and restore-on-drop made exit restore three times.
        unsafe {
            (self.fn_release)(self.p_unk);
        }
    }
}

// Undocumented SetWindowCompositionAttribute definitions (Win10 fallback)
const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_DISABLED: u32 = 0;
const ACCENT_ENABLE_GRADIENT: u32 = 1;
const ACCENT_ENABLE_TRANSPARENTGRADIENT: u32 = 2;
const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttribData {
    attrib: u32,
    pv_data: *mut c_void,
    cb_data: usize,
}

type PfnSetWindowCompositionAttribute =
    unsafe extern "system" fn(HWND, *const WindowCompositionAttribData) -> BOOL;

type PfnInjectExplorerTAP = unsafe extern "C" fn(HWND, *const GUID, *mut *mut c_void) -> HRESULT;

pub struct TaskbarController {
    pfn_swca: Option<PfnSetWindowCompositionAttribute>,
    tap_service: Option<TaskbarAppearanceService>,
    h_tap_module: Option<HMODULE>,
    /// Set from `apply` (which only has `&self`) when a COM call reports explorer died.
    tap_dead: Cell<bool>,
    com_initialized: bool,
}

impl TaskbarController {
    pub fn new() -> Self {
        unsafe {
            // Attach this thread to the interactive desktop once.
            if let Ok(hdesk) = OpenDesktopW(w!("Default"), DESKTOP_CONTROL_FLAGS(0), false, 0x1000_0000) {
                let _ = SetThreadDesktop(hdesk);
                let _ = CloseDesktop(hdesk);
            }
        }

        // S_FALSE ("already initialized") still needs a matching CoUninitialize;
        // RPC_E_CHANGED_MODE must not get one.
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };

        let pfn_swca = unsafe {
            LoadLibraryW(w!("user32.dll")).ok().and_then(|hmod| {
                GetProcAddress(hmod, windows::core::s!("SetWindowCompositionAttribute")).map(|p| {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, PfnSetWindowCompositionAttribute>(p)
                })
            })
        };

        let mut controller = Self {
            pfn_swca,
            tap_service: None,
            h_tap_module: None,
            tap_dead: Cell::new(false),
            com_initialized,
        };

        if controller.reconnect() {
            println!("[+] Windows 11 Modern Taskbar XAML Engine connected successfully");
        } else {
            println!("[*] Using Native DWM / SWCA fallback engine");
        }
        controller
    }

    pub fn is_modern(&self) -> bool {
        self.tap_service.is_some()
    }

    pub fn engine_name(&self) -> &'static str {
        if self.is_modern() { "Win11 XAML" } else { "DWM / SWCA" }
    }

    /// True once a COM call has failed because explorer.exe went away.
    pub fn is_disconnected(&self) -> bool {
        self.tap_dead.get()
    }

    /// (Re)creates the ExplorerTAP connection. Call after explorer restarts (TaskbarCreated).
    pub fn reconnect(&mut self) -> bool {
        // Drop the stale proxy and free the previously loaded module
        self.tap_service = None;
        if let Some(hmod) = self.h_tap_module.take() {
            unsafe {
                let _ = FreeLibrary(hmod);
            }
        }
        self.tap_dead.set(false);
        let (service, hmod) = Self::init_tap_service();
        self.tap_service = service;
        self.h_tap_module = hmod;
        self.tap_service.is_some()
    }

    fn init_tap_service() -> (Option<TaskbarAppearanceService>, Option<HMODULE>) {
        let Some(primary_hwnd) = (unsafe { FindWindowW(w!("Shell_TrayWnd"), None).ok() }) else {
            return (None, None);
        };
        if primary_hwnd.is_invalid() {
            return (None, None);
        }

        let Some(tap_dll_path) = Self::ensure_tap_dll() else {
            return (None, None);
        };

        unsafe {
            let path_w: Vec<u16> = tap_dll_path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let Ok(h_module) = LoadLibraryW(windows::core::PCWSTR(path_w.as_ptr())) else {
                return (None, None);
            };
            let Some(proc_inject) = GetProcAddress(h_module, windows::core::s!("InjectExplorerTAP")) else {
                let _ = FreeLibrary(h_module);
                return (None, None);
            };
            let pfn_inject: PfnInjectExplorerTAP = std::mem::transmute(proc_inject);

            let mut p_service: *mut c_void = std::ptr::null_mut();
            let hr = pfn_inject(primary_hwnd, &IID_ITASKBAR_APPEARANCE_SERVICE, &mut p_service);

            if hr.is_ok() && !p_service.is_null() {
                (TaskbarAppearanceService::from_raw(p_service), Some(h_module))
            } else {
                let _ = FreeLibrary(h_module);
                (None, None)
            }
        }
    }

    fn ensure_tap_dll() -> Option<PathBuf> {
        let base_dir = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));

        let tap_dir = base_dir.join("Mutatio");
        let _ = fs::create_dir_all(&tap_dir);
        // Include a short content hash so new versions never collide with a locked DLL loaded in explorer.
        let hash = format!("{:08x}", EMBEDDED_EXPLORER_TAP.len() ^ (EMBEDDED_EXPLORER_TAP.first().copied().unwrap_or(0) as usize));
        let tap_path = tap_dir.join(format!("ExplorerTAP_{hash}.dll"));

        // Compare full contents, not just length: a same-size rebuild of the DLL would otherwise
        // never be updated. 550 KB read once at startup is negligible.
        let up_to_date = fs::read(&tap_path).is_ok_and(|bytes| bytes == EMBEDDED_EXPLORER_TAP);

        // A failed write is fatal only if the file on disk is wrong. (It can be locked because
        // explorer still has it loaded from a previous run — that's fine when it's identical.)
        if !up_to_date && fs::write(&tap_path, EMBEDDED_EXPLORER_TAP).is_err() {
            return None;
        }

        Some(tap_path)
    }

    /// Finds all primary and secondary taskbars
    pub fn find_taskbars(&self) -> Vec<HWND> {
        let mut taskbars = Vec::new();

        unsafe {
            if let Ok(h) = FindWindowW(w!("Shell_TrayWnd"), None) {
                taskbars.push(h);
            }

            let mut prev = HWND::default();
            while let Ok(h) = FindWindowExW(None, Some(prev), w!("Shell_SecondaryTrayWnd"), None) {
                taskbars.push(h);
                prev = h;
            }
        }

        taskbars
    }

    /// Applies appearance to a single taskbar
    pub fn apply(&self, taskbar: HWND, appearance: &StateAppearance) {
        if let Some(ref service) = self.tap_service {
            let color = appearance.color;
            let hr = match appearance.state {
                TaskbarState::Normal => service.return_to_default(taskbar),
                TaskbarState::Clear => {
                    service.set_appearance(taskbar, TaskbarBrush::SolidColor, color.to_abgr())
                }
                TaskbarState::Opaque => {
                    // Opaque means opaque: force alpha only. (The old code also silently remapped
                    // #181818 to #000000, so the user could never pick that color.)
                    let solid = crate::config::Color { a: 255, ..color };
                    service.set_appearance(taskbar, TaskbarBrush::SolidColor, solid.to_abgr())
                }
                TaskbarState::Blur => service.set_blur(taskbar, color.to_abgr(), 10.0),
                TaskbarState::Acrylic => {
                    // Alpha 0 acrylic renders as plain transparent; keep a hint of tint.
                    let tinted = crate::config::Color { a: color.a.max(1), ..color };
                    service.set_appearance(taskbar, TaskbarBrush::Acrylic, tinted.to_abgr())
                }
            };
            let hr_border = service.set_border_visibility(taskbar, appearance.show_line);

            if is_dead_server(hr) || is_dead_server(hr_border) {
                self.tap_dead.set(true);
            }
            return;
        }

        self.apply_swca(taskbar, appearance);
    }

    /// Fallback for Windows 10 & ExplorerPatcher / Classic Taskbar
    fn apply_swca(&self, taskbar: HWND, appearance: &StateAppearance) {
        let color = appearance.color;
        let (accent_state, accent_flags, color_val) = match appearance.state {
            TaskbarState::Normal => (ACCENT_DISABLED, 0, 0),
            TaskbarState::Clear => (ACCENT_ENABLE_TRANSPARENTGRADIENT, 2, color.to_abgr()),
            TaskbarState::Opaque => (
                ACCENT_ENABLE_GRADIENT,
                0,
                crate::config::Color { a: 255, ..color }.to_abgr(),
            ),
            TaskbarState::Blur => (ACCENT_ENABLE_BLURBEHIND, 0, color.to_abgr()),
            TaskbarState::Acrylic => (
                ACCENT_ENABLE_ACRYLICBLURBEHIND,
                0,
                crate::config::Color { a: color.a.max(1), ..color }.to_abgr(),
            ),
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

        unsafe {
            if let Some(swca) = self.pfn_swca {
                let _ = swca(taskbar, &data);
            }

            let backdrop_type: u32 = match appearance.state {
                TaskbarState::Normal => 0,
                TaskbarState::Clear | TaskbarState::Opaque => 1,
                TaskbarState::Blur => 2,
                TaskbarState::Acrylic => 3,
            };
            let _ = DwmSetWindowAttribute(
                taskbar,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop_type as *const _ as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );

            // DWMWA_COLOR_DEFAULT vs DWMWA_COLOR_NONE
            let border_color: u32 = if appearance.show_line { 0xFFFF_FFFF } else { 0xFFFF_FFFE };
            let _ = DwmSetWindowAttribute(
                taskbar,
                DWMWA_BORDER_COLOR,
                &border_color as *const _ as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );
        }
    }

    /// Restores all taskbars to standard Windows appearance
    pub fn restore_all(&self) {
        if let Some(ref service) = self.tap_service {
            let _ = service.restore_all();
            return;
        }

        let normal = StateAppearance::preset(TaskbarState::Normal);
        for tb in self.find_taskbars() {
            self.apply_swca(tb, &normal);
        }
    }
}

impl Drop for TaskbarController {
    fn drop(&mut self) {
        // Order matters: the COM proxy must be released BEFORE CoUninitialize. Field drop glue
        // runs after this body, so the old code released it on an already-uninitialized apartment.
        self.tap_service = None;
        if let Some(hmod) = self.h_tap_module.take() {
            unsafe {
                let _ = FreeLibrary(hmod);
            }
        }
        if self.com_initialized {
            unsafe { CoUninitialize() };
        }
    }
}
