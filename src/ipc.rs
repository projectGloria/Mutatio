use serde::{Deserialize, Serialize};
use std::sync::mpsc::Sender;
use std::thread;
use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

pub const PIPE_NAME: &str = r"\\.\pipe\mutatio";
pub const PIPE_ACCESS_DUPLEX_VAL: u32 = 0x00000003;

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

pub struct IpcServer;

impl IpcServer {
    pub fn start(cmd_sender: Sender<IpcCommand>) {
        thread::spawn(move || {
            let pipe_name_w: Vec<u16> = PIPE_NAME
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            loop {
                unsafe {
                    let h_pipe = CreateNamedPipeW(
                        windows::core::PCWSTR(pipe_name_w.as_ptr()),
                        FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX_VAL),
                        PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                        PIPE_UNLIMITED_INSTANCES,
                        512,
                        512,
                        0,
                        None,
                    );

                    if h_pipe == INVALID_HANDLE_VALUE {
                        thread::sleep(std::time::Duration::from_millis(500));
                        continue;
                    }

                    let connected = ConnectNamedPipe(h_pipe, None).is_ok()
                        || windows::Win32::Foundation::GetLastError().0 == 535; // ERROR_PIPE_CONNECTED

                    if connected {
                        let mut buffer = [0u8; 1024];
                        let mut bytes_read = 0u32;

                        if ReadFile(
                            h_pipe,
                            Some(&mut buffer),
                            Some(&mut bytes_read),
                            None,
                        )
                        .is_ok()
                            && bytes_read > 0
                        {
                            if let Ok(msg) = std::str::from_utf8(&buffer[..bytes_read as usize]) {
                                let resp = if let Ok(cmd) = serde_json::from_str::<IpcCommand>(msg.trim()) {
                                    let _ = cmd_sender.send(cmd);
                                    IpcResponse {
                                        success: true,
                                        message: "Command processed successfully".to_string(),
                                        active_state: None,
                                    }
                                } else {
                                    IpcResponse {
                                        success: false,
                                        message: format!("Unrecognized command: {}", msg),
                                        active_state: None,
                                    }
                                };

                                if let Ok(resp_json) = serde_json::to_string(&resp) {
                                    let mut written = 0u32;
                                    let _ = WriteFile(
                                        h_pipe,
                                        Some(resp_json.as_bytes()),
                                        Some(&mut written),
                                        None,
                                    );
                                }
                            }
                        }

                        let _ = DisconnectNamedPipe(h_pipe);
                    }

                    let _ = CloseHandle(h_pipe);
                }
            }
        });
    }

    /// Sends a command to the running server via named pipe
    pub fn send_client_command(cmd: &IpcCommand) -> Result<IpcResponse, String> {
        let pipe_name_w: Vec<u16> = PIPE_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let req_json = serde_json::to_string(cmd)
            .map_err(|e| format!("Serialization error: {}", e))?;

        unsafe {
            let h_pipe = CreateFileW(
                windows::core::PCWSTR(pipe_name_w.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            );

            if h_pipe.is_err() || h_pipe.as_ref().unwrap().0 as usize == 0 {
                return Err("Could not connect to Mutatio instance. Is it running?".to_string());
            }

            let handle = h_pipe.unwrap();
            let mut written = 0u32;
            let write_res = WriteFile(
                handle,
                Some(req_json.as_bytes()),
                Some(&mut written),
                None,
            );

            if write_res.is_err() {
                let _ = CloseHandle(handle);
                return Err("Failed to write to Mutatio pipe".to_string());
            }

            let mut buffer = [0u8; 1024];
            let mut read_bytes = 0u32;
            let read_res = ReadFile(
                handle,
                Some(&mut buffer),
                Some(&mut read_bytes),
                None,
            );

            let _ = CloseHandle(handle);

            if read_res.is_err() || read_bytes == 0 {
                return Err("No response received from Mutatio".to_string());
            }

            let resp_str = std::str::from_utf8(&buffer[..read_bytes as usize])
                .map_err(|e| format!("UTF8 error: {}", e))?;

            serde_json::from_str::<IpcResponse>(resp_str)
                .map_err(|e| format!("JSON parse error: {}", e))
        }
    }
}
