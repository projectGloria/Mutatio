//! Console-subsystem CLI front-end, installed as `mutatio.com` next to `mutatio.exe`.
//! See src/cli.rs for why two binaries exist.
#![allow(dead_code)] // shares ipc/config with the daemon; the server half is unused here

#[path = "../cli.rs"]
mod cli;
#[path = "../config.rs"]
mod config;
#[path = "../ipc.rs"]
mod ipc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if !cli::handle_cli_client(&args) {
        // Bare `mutatio` (or --daemon) from a terminal: start the tray daemon detached and return
        // the prompt immediately.
        let preset = match (args.get(1).map(String::as_str), args.get(2).map(String::as_str)) {
            (Some("--daemon-start"), Some(name)) => Some(name),
            _ => None,
        };
        cli::spawn_daemon(preset);
    }
}
