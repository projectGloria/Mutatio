//! Command-line client. Shared by both binaries:
//! - `mutatio.exe` (GUI subsystem: the tray daemon; also accepts these flags), and
//! - `mutatio.com` (console subsystem, built from src/bin/mutatio-cli.rs). Windows resolves a bare
//!   `mutatio` to `.com` before `.exe` (PATHEXT order), so terminal use gets normal, synchronous
//!   console output while shortcuts/autostart launch the windowless `.exe`. Same trick as
//!   Visual Studio's devenv.com/devenv.exe.

use crate::config::{Color, TaskbarState};
use crate::ipc::{IpcCommand, IpcResponse, IpcServer};
use std::os::windows::process::CommandExt;
use std::process::Command;
use windows::core::w;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::Threading::{OpenMutexW, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE};

/// Held (owned) by the running daemon for its whole lifetime. Doubles as the single-instance lock
/// and as an "has it exited yet?" signal: waiting on it returns once the daemon process is gone.
pub const DAEMON_MUTEX: windows::core::PCWSTR = w!("Local\\Mutatio.Daemon");

pub fn print_help() {
    println!(
        r#"Mutatio {} - Next-Gen Lightweight Taskbar Engine
Usage:
    mutatio                      Run daemon in background (System Tray & Flyout UI)
    mutatio --clear              Set taskbar to Clear (Transparent)
    mutatio --blur               Set taskbar to Blur
    mutatio --acrylic            Set taskbar to Acrylic
    mutatio --opaque             Set taskbar to Opaque (Pure Black)
    mutatio --normal             Restore default Windows taskbar
    mutatio --color <HEX>        Set taskbar color (#RRGGBB or #RRGGBBAA)
    mutatio --toggle             Toggle dynamic auto-detection on/off
    mutatio --reload             Reload configuration
    mutatio --status             Show running status
    mutatio --quit               Exit running Mutatio instance (waits until it has exited)
"#,
        env!("CARGO_PKG_VERSION")
    );
}

/// GUI-subsystem processes start without stdout; borrow the launching terminal's console so
/// output isn't lost. Fails harmlessly in the console-subsystem `.com` build.
fn attach_parent_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Starts the daemon (`mutatio.exe`, next to whichever binary is running) detached from this
/// console — otherwise it would die when the terminal closes.
pub fn spawn_daemon(preset: Option<&str>) {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let Ok(current) = std::env::current_exe() else { return };
    let daemon = current.with_file_name("mutatio.exe");
    let mut cmd = Command::new(daemon);
    if let Some(p) = preset {
        cmd.args(["--daemon-start", p]);
    }
    // Null stdio: an inherited pipe handle would keep a `mutatio | ...` pipeline open for as long
    // as the daemon lives.
    let _ = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn();
}

fn print_response(res: Result<IpcResponse, String>) -> bool {
    match res {
        Ok(r) if r.success => {
            println!("{}", r.message);
            true
        }
        Ok(r) => {
            eprintln!("Error: {}", r.message);
            false
        }
        Err(e) => {
            eprintln!("Error: {e}");
            false
        }
    }
}

/// Blocks until the daemon process has exited (its owned mutex becomes abandoned), so callers
/// such as the uninstaller can delete the exe right after `--quit` returns.
fn wait_for_daemon_exit() {
    unsafe {
        if let Ok(h) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, DAEMON_MUTEX) {
            let _ = WaitForSingleObject(h, 5000);
            let _ = CloseHandle(h);
        }
    }
}

/// Handles a CLI invocation. Returns false when there is nothing to do as a client (no args or a
/// `--daemon*` flag), i.e. the caller should run/start the daemon instead.
pub fn handle_cli_client(args: &[String]) -> bool {
    let Some(flag) = args.get(1) else { return false };
    if flag == "--daemon" || flag == "--daemon-start" {
        return false;
    }

    attach_parent_console();

    if let Some(state) = flag.strip_prefix("--").and_then(TaskbarState::parse) {
        match IpcServer::send_client_command(&IpcCommand::SetState(state.name().into())) {
            Ok(r) => {
                print_response(Ok(r));
            }
            Err(_) => {
                println!("[*] Starting Mutatio background daemon with {} taskbar...", state.name());
                spawn_daemon(Some(state.name()));
            }
        }
        return true;
    }

    let command = match flag.as_str() {
        "--toggle" => IpcCommand::ToggleDynamic,
        "--reload" => IpcCommand::Reload,
        "--status" => IpcCommand::GetStatus,
        "--quit" => IpcCommand::Quit,
        "--color" => match args.get(2).map(|h| (h, Color::from_hex(h))) {
            Some((_, Some(c))) => IpcCommand::SetColor { r: c.r, g: c.g, b: c.b, a: c.a },
            Some((h, None)) => {
                eprintln!("Invalid color hex: {h} (expected #RRGGBB or #RRGGBBAA)");
                return true;
            }
            None => {
                eprintln!("Missing color hex argument");
                return true;
            }
        },
        _ => {
            print_help();
            return true;
        }
    };

    let is_quit = matches!(command, IpcCommand::Quit);
    if print_response(IpcServer::send_client_command(&command)) && is_quit {
        wait_for_daemon_exit();
    }
    true
}
