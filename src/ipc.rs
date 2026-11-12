use serde::{Deserialize, Serialize};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, SetNamedPipeHandleState,
    WaitNamedPipeW, NAMED_PIPE_MODE, PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const PIPE_NAME: &str = r"\\.\pipe\mutatio";
const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const BUF_SIZE: usize = 4096;

/// Posted to the daemon's message window so its idle wait wakes up for a queued IPC command.
pub const WM_MUTATIO_IPC: u32 = WM_APP + 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", content = "payload")]
pub enum IpcCommand {
    #[serde(rename = "set_state")]
    SetState(String), // "clear", "blur", "acrylic", "opaque", "normal"

    #[serde(rename = "set_color")]
    SetColor { r: u8, g: u8, b: u8, a: u8 },

    #[serde(rename = "toggle_dynamic")]
    ToggleDynamic,

    #[serde(rename = "reload")]
    Reload,

    #[serde(rename = "status")]
    GetStatus,

    #[serde(rename = "quit")]
    Quit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub success: bool,
    pub message: String,
    pub active_state: Option<String>,
}

impl IpcResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self { success: true, message: message.into(), active_state: None }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self { success: false, message: message.into(), active_state: None }
    }
}

/// A command plus the channel the main thread answers on. The old server replied "success" the
/// moment a command was queued, so `--status` could never report anything real.
pub struct IpcRequest {
    pub command: IpcCommand,
    pub reply: Sender<IpcResponse>,
}

pub struct IpcServer;

impl IpcServer {
    /// `wake_hwnd` is the daemon's message window (as a raw isize because HWND isn't Send).
    pub fn start(cmd_sender: Sender<IpcRequest>, wake_hwnd: isize) {
        thread::spawn(move || {
            let pipe_name_w: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();

            loop {
                let h_pipe = unsafe {
                    CreateNamedPipeW(
                        windows::core::PCWSTR(pipe_name_w.as_ptr()),
                        FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX),
                        // Reject SMB clients: the default pipe is reachable from the network.
                        PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                        PIPE_UNLIMITED_INSTANCES,
                        BUF_SIZE as u32,
                        BUF_SIZE as u32,
                        0,
                        None,
                    )
                };

                if h_pipe.is_invalid() {
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }

                // Blocks this thread (not the UI thread) until a client arrives.
                let connected = match unsafe { ConnectNamedPipe(h_pipe, None) } {
                    Ok(()) => true,
                    // Client connected between Create and Connect — still a valid connection.
                    Err(_) => (unsafe { GetLastError() }) == ERROR_PIPE_CONNECTED,
                };

                if connected {
                    Self::serve_one(h_pipe, &cmd_sender, wake_hwnd);
                    unsafe {
                        let _ = FlushFileBuffers(h_pipe); // let the client read before we disconnect
                        let _ = DisconnectNamedPipe(h_pipe);
                    }
                }

                unsafe {
                    let _ = CloseHandle(h_pipe);
                }
            }
        });
    }

    fn serve_one(h_pipe: HANDLE, cmd_sender: &Sender<IpcRequest>, wake_hwnd: isize) {
        let Some(msg) = read_message(h_pipe) else { return };

        let resp = match serde_json::from_str::<IpcCommand>(msg.trim()) {
            Ok(command) => {
                let (reply_tx, reply_rx) = channel();
                if cmd_sender.send(IpcRequest { command, reply: reply_tx }).is_err() {
                    IpcResponse::err("Daemon is shutting down")
                } else {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(wake_hwnd as *mut _)),
                            WM_MUTATIO_IPC,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                    reply_rx
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap_or_else(|_| IpcResponse::err("Daemon did not respond in time"))
                }
            }
            Err(e) => IpcResponse::err(format!("Unrecognized command ({e})")),
        };

        if let Ok(json) = serde_json::to_string(&resp) {
            write_message(h_pipe, json.as_bytes());
        }
    }

    /// Sends a command to the running server via named pipe
    pub fn send_client_command(cmd: &IpcCommand) -> Result<IpcResponse, String> {
        let pipe_name_w: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let pipe_name = windows::core::PCWSTR(pipe_name_w.as_ptr());
        let req_json = serde_json::to_string(cmd).map_err(|e| format!("Serialization error: {e}"))?;

        unsafe {
            // The server has exactly one listening instance and re-creates it after each client,
            // so a concurrent client can see ERROR_PIPE_BUSY. Wait instead of failing — the old
            // code treated "busy" as "not running" and spawned a second daemon.
            let handle = loop {
                match CreateFileW(
                    pipe_name,
                    FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    None,
                ) {
                    Ok(h) => break h,
                    Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                        if !WaitNamedPipeW(pipe_name, 2000).as_bool() {
                            return Err("Mutatio instance is busy".into());
                        }
                    }
                    Err(_) => return Err("Could not connect to Mutatio instance. Is it running?".into()),
                }
            };

            // Match the server's message mode so one ReadFile == one reply.
            let mode = PIPE_READMODE_MESSAGE;
            let _ = SetNamedPipeHandleState(handle, Some(&mode as *const NAMED_PIPE_MODE), None, None);

            let ok = write_message(handle, req_json.as_bytes());
            let reply = if ok { read_message(handle) } else { None };
            let _ = CloseHandle(handle);

            let reply = reply.ok_or("No response received from Mutatio")?;
            serde_json::from_str::<IpcResponse>(&reply).map_err(|e| format!("JSON parse error: {e}"))
        }
    }
}

fn read_message(h: HANDLE) -> Option<String> {
    let mut buffer = [0u8; BUF_SIZE];
    let mut bytes_read = 0u32;
    // Guard against malicious/hung clients holding the connection without writing.
    let start = std::time::Instant::now();
    loop {
        let mut avail = 0u32;
        let ok = unsafe { PeekNamedPipe(h, None, 0, None, Some(&mut avail), None) };
        if ok.is_err() {
            return None;
        }
        if avail > 0 {
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
    unsafe { ReadFile(h, Some(&mut buffer), Some(&mut bytes_read), None).ok()? };
    if bytes_read == 0 {
        return None;
    }
    String::from_utf8(buffer[..bytes_read as usize].to_vec()).ok()
}

fn write_message(h: HANDLE, bytes: &[u8]) -> bool {
    let mut written = 0u32;
    unsafe { WriteFile(h, Some(bytes), Some(&mut written), None).is_ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_buffer_safety() {
        let oversized = vec![b'a'; 8192];
        let res = serde_json::from_slice::<IpcCommand>(&oversized);
        assert!(res.is_err());
    }

    #[test]
    fn test_ipc_command_serialization() {
        let cmd = IpcCommand::SetState("blur".into());
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("set_state"));
        assert!(json.contains("blur"));
        let parsed: IpcCommand = serde_json::from_str(&json).unwrap();
        match parsed {
            IpcCommand::SetState(s) => assert_eq!(s, "blur"),
            _ => panic!("wrong variant"),
        }
    }
}
