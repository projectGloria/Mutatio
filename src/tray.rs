use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetSystemMetrics, LoadIconW, LoadImageW, HICON, IDI_APPLICATION, IMAGE_ICON,
    LR_DEFAULTCOLOR, SM_CXSMICON, SM_CYSMICON, WM_USER,
};

pub const WM_TRAYICON: u32 = WM_USER + 100;

/// Resource ID of the app icon, declared in mutatio.rc (`1 ICON "Mutatio.ico"`).
const APP_ICON_ID: u16 = 1;

pub struct TrayIcon {
    nid: NOTIFYICONDATAW,
    owned_icon: Option<HICON>,
}

impl TrayIcon {
    pub fn create(hwnd: HWND) -> Self {
        let owned_icon = load_app_icon();
        let icon = owned_icon.unwrap_or_else(|| unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() });

        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: icon,
            ..Default::default()
        };

        let tip: Vec<u16> = "Mutatio - Click to open settings".encode_utf16().collect();
        let copy_len = tip.len().min(nid.szTip.len() - 1); // keep the NUL terminator
        nid.szTip[..copy_len].copy_from_slice(&tip[..copy_len]);

        let tray = Self { nid, owned_icon };
        tray.add();
        tray
    }

    /// (Re-)registers the icon. Must be called again when explorer restarts (TaskbarCreated):
    /// the new shell has no memory of icons added to the old one.
    pub fn add(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.nid);
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid);
            if let Some(icon) = self.owned_icon.take() {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

/// Loads the embedded app icon at the tray's small-icon size. LoadImageW picks the closest
/// image from the multi-size .ico (16..256 px), so it stays crisp at any DPI scale.
fn load_app_icon() -> Option<HICON> {
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let handle: HANDLE = LoadImageW(
            Some(module.into()),
            windows::core::PCWSTR(APP_ICON_ID as usize as *const u16), // MAKEINTRESOURCEW
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        )
        .ok()?;
        Some(HICON(handle.0))
    }
}
