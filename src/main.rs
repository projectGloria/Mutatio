mod config;
mod flyout;
mod ipc;
mod monitor;
mod taskbar;
mod transition;
mod tray;

use config::{Color, Config, StateAppearance, TaskbarState};
use flyout::{
    FlyoutState, FlyoutWindow, ACTION_EXIT, ACTION_OPEN_CONFIG, ACTION_RESTART_EXPLORER,
    ACTION_SET_ACRYLIC, ACTION_SET_BLUR, ACTION_SET_CLEAR, ACTION_SET_COLOR, ACTION_SET_DEFAULT,
    ACTION_SET_OPAQUE, ACTION_TOGGLE_BORDER, ACTION_TOGGLE_DYNAMIC, ACTION_TOGGLE_GAME_MODE,
    ACTION_TOGGLE_SMOOTH, WM_FLYOUT_ACTION,
};
use ipc::{IpcCommand, IpcServer};
use monitor::{ActiveState, StateDetector};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use taskbar::TaskbarController;
use transition::StateTransition;
use tray::{TrayIcon, WM_TRAYICON};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, PeekMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, TranslateMessage, MSG, PM_REMOVE, WINDOW_EX_STYLE,
    WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

static RUNNING: AtomicBool = AtomicBool::new(true);
const ID_TOGGLE_FLYOUT: usize = 2002;

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                let event = lparam.0 as u32;
                if event == WM_LBUTTONUP {
                    // Left-clicking tray icon opens the UI flyout!
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_COMMAND,
                        WPARAM(ID_TOGGLE_FLYOUT),
                        LPARAM(0),
                    );
                }
                // Right clicking the app icon in the tray does nothing (as requested)
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn print_help() {
    println!(
        r#"Mutatio - Next-Gen Lightweight Taskbar Engine
Usage:
    mutatio                      Run daemon in background (System Tray & Flyout UI)
    mutatio --clear              Set taskbar to Clear (Transparent)
    mutatio --blur               Set taskbar to Blur
    mutatio --acrylic            Set taskbar to Acrylic
    mutatio --opaque             Set taskbar to Opaque (Pure Black)
    mutatio --normal             Restore default Windows taskbar
    mutatio --color <HEX>        Set taskbar color (e.g. #00000000 or #FF000000)
    mutatio --toggle             Toggle between Dynamic and Forced mode
    mutatio --reload             Reload configuration
    mutatio --status             Show running status
    mutatio --quit               Exit running Mutatio instance
"#
    );
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    let clean = hex.trim().trim_start_matches('#');
    match clean.len() {
        6 => {
            let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
            let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
            let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
            Some(Color::new(r, g, b, 255))
        }
        8 => {
            let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
            let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
            let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
            let a = u8::from_str_radix(&clean[6..8], 16).ok()?;
            Some(Color::new(r, g, b, a))
        }
        _ => None,
    }
}

fn spawn_daemon_with_arg(preset: &str) {
    if let Ok(exe_path) = std::env::current_exe() {
        let _ = Command::new(exe_path)
            .arg("--daemon-start")
            .arg(preset)
            .spawn();
    }
}

fn handle_cli_client(args: &[String]) -> bool {
    if args.len() <= 1 {
        return false;
    }

    if args[1] == "--daemon" || args[1] == "--daemon-start" {
        return false;
    }

    match args[1].as_str() {
        "--help" | "-h" => {
            print_help();
            true
        }
        "--clear" => {
            let res = IpcServer::send_client_command(&IpcCommand::SetState("clear".into()));
            if res.is_err() {
                println!("[*] Starting Mutatio background daemon with Clear taskbar...");
                spawn_daemon_with_arg("clear");
            } else {
                println!("{:?}", res.unwrap());
            }
            true
        }
        "--blur" => {
            let res = IpcServer::send_client_command(&IpcCommand::SetState("blur".into()));
            if res.is_err() {
                println!("[*] Starting Mutatio background daemon with Blur taskbar...");
                spawn_daemon_with_arg("blur");
            } else {
                println!("{:?}", res.unwrap());
            }
            true
        }
        "--acrylic" => {
            let res = IpcServer::send_client_command(&IpcCommand::SetState("acrylic".into()));
            if res.is_err() {
                println!("[*] Starting Mutatio background daemon with Acrylic taskbar...");
                spawn_daemon_with_arg("acrylic");
            } else {
                println!("{:?}", res.unwrap());
            }
            true
        }
        "--opaque" => {
            let res = IpcServer::send_client_command(&IpcCommand::SetState("opaque".into()));
            if res.is_err() {
                println!("[*] Starting Mutatio background daemon with Opaque Black taskbar...");
                spawn_daemon_with_arg("opaque");
            } else {
                println!("{:?}", res.unwrap());
            }
            true
        }
        "--normal" => {
            let res = IpcServer::send_client_command(&IpcCommand::SetState("normal".into()));
            if res.is_err() {
                println!("[*] Starting Mutatio background daemon with Normal taskbar...");
                spawn_daemon_with_arg("normal");
            } else {
                println!("{:?}", res.unwrap());
            }
            true
        }
        "--toggle" => {
            let res = IpcServer::send_client_command(&IpcCommand::ToggleDynamic);
            println!("{:?}", res);
            true
        }
        "--reload" => {
            let res = IpcServer::send_client_command(&IpcCommand::Reload);
            println!("{:?}", res);
            true
        }
        "--status" => {
            let res = IpcServer::send_client_command(&IpcCommand::GetStatus);
            println!("{:?}", res);
            true
        }
        "--quit" => {
            let res = IpcServer::send_client_command(&IpcCommand::Quit);
            println!("{:?}", res);
            true
        }
        "--color" => {
            if args.len() > 2 {
                if let Some(col) = parse_hex_color(&args[2]) {
                    let res = IpcServer::send_client_command(&IpcCommand::SetColor {
                        r: col.r,
                        g: col.g,
                        b: col.b,
                        a: col.a,
                    });
                    println!("{:?}", res);
                } else {
                    eprintln!("Invalid color hex: {}", args[2]);
                }
            } else {
                eprintln!("Missing color hex argument");
            }
            true
        }
        _ => {
            print_help();
            true
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if handle_cli_client(&args) {
        return;
    }

    println!("[*] Starting Mutatio Daemon...");

    let initial_forced_preset = if args.len() > 2 && args[1] == "--daemon-start" {
        match args[2].as_str() {
            "clear" => Some(StateAppearance {
                state: TaskbarState::Clear,
                color: Color::transparent(),
                show_line: false,
            }),
            "blur" => Some(StateAppearance {
                state: TaskbarState::Blur,
                color: Color::new(20, 20, 20, 120),
                show_line: false,
            }),
            "acrylic" => Some(StateAppearance {
                state: TaskbarState::Acrylic,
                color: Color::new(30, 30, 30, 180),
                show_line: false,
            }),
            "opaque" => Some(StateAppearance {
                state: TaskbarState::Opaque,
                color: Color::new(0, 0, 0, 255),
                show_line: false,
            }),
            "normal" => Some(StateAppearance {
                state: TaskbarState::Normal,
                color: Color::transparent(),
                show_line: true,
            }),
            _ => None,
        }
    } else {
        None
    };

    // 1. Load config
    let config_path = Config::default_path();
    let mut config = Config::load_or_create(&config_path);
    println!("[+] Configuration loaded from: {:?}", config_path);

    // 2. Initialize taskbar controller (Modern Win11 TAP + SWCA fallback)
    let taskbar = TaskbarController::new();
    let initial_taskbars = taskbar.find_taskbars();
    println!("[+] Found {} taskbar instances", initial_taskbars.len());

    // 3. Register message window
    let class_name = w!("MutatioMsgWindowClass");
    let hwnd = unsafe {
        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wnd_class);

        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("Mutatio Core"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
        .unwrap_or(HWND::default())
    };

    // Initialize System Tray Icon
    let _tray = TrayIcon::create(hwnd);
    println!("[+] System tray icon initialized");

    // Initialize Modern Fluent Flyout UI Window
    let flyout = FlyoutWindow::create(hwnd);
    println!("[+] Modern Fluent Flyout UI initialized");

    // 4. Start Named Pipe IPC Server
    let (cmd_tx, cmd_rx) = channel::<IpcCommand>();
    IpcServer::start(cmd_tx);
    println!("[+] Named Pipe Server listening on {}", ipc::PIPE_NAME);

    // 5. Main loop state
    let mut last_state = ActiveState::Desktop;
    let mut current_appearance = initial_forced_preset
        .clone()
        .unwrap_or_else(|| config.desktop.clone());
    let mut transition: Option<StateTransition> = None;
    let mut force_preset: Option<StateAppearance> = initial_forced_preset;

    // Apply initial appearance immediately
    taskbar.apply_to_all(&current_appearance);

    let mut last_poll = Instant::now();

    while RUNNING.load(Ordering::Relaxed) {
        // A. Handle Win32 Window messages
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    RUNNING.store(false, Ordering::Relaxed);
                    break;
                }

                if msg.message == WM_COMMAND {
                    let cmd_id = msg.wParam.0 as usize;
                    if cmd_id == ID_TOGGLE_FLYOUT {
                        let mut pt = POINT::default();
                        let _ = GetCursorPos(&mut pt);
                        let state = force_preset.as_ref().map(|f| f.state).unwrap_or(current_appearance.state);
                        let color = force_preset.as_ref().map(|f| f.color).unwrap_or(current_appearance.color);
                        let border = force_preset.as_ref().map(|f| f.show_line).unwrap_or(current_appearance.show_line);

                        flyout.update_state(FlyoutState {
                            active_state: state,
                            is_forced: force_preset.is_some(),
                            dynamic_enabled: config.dynamic_modes_enabled,
                            smooth_enabled: config.smooth_transitions,
                            game_pause_enabled: config.game_mode_pause,
                            show_border: border,
                            current_color: color,
                        });
                        flyout.toggle(pt);
                    }
                } else if msg.message == WM_FLYOUT_ACTION {
                    let action = msg.wParam.0 as usize;
                    match action {
                        ACTION_SET_CLEAR => {
                            force_preset = Some(StateAppearance {
                                state: TaskbarState::Clear,
                                color: Color::transparent(),
                                show_line: false,
                            });
                        }
                        ACTION_SET_BLUR => {
                            force_preset = Some(StateAppearance {
                                state: TaskbarState::Blur,
                                color: Color::new(20, 20, 20, 120),
                                show_line: false,
                            });
                        }
                        ACTION_SET_ACRYLIC => {
                            force_preset = Some(StateAppearance {
                                state: TaskbarState::Acrylic,
                                color: Color::new(30, 30, 30, 180),
                                show_line: false,
                            });
                        }
                        ACTION_SET_OPAQUE => {
                            // Solid Black Taskbar (#000000, 100% Alpha)
                            force_preset = Some(StateAppearance {
                                state: TaskbarState::Opaque,
                                color: Color::new(0, 0, 0, 255),
                                show_line: false,
                            });
                        }
                        ACTION_SET_DEFAULT => {
                            force_preset = Some(StateAppearance {
                                state: TaskbarState::Normal,
                                color: Color::transparent(),
                                show_line: true,
                            });
                        }
                        ACTION_SET_COLOR => {
                            let abgr = msg.lParam.0 as u32;
                            let r = (abgr & 0xFF) as u8;
                            let g = ((abgr >> 8) & 0xFF) as u8;
                            let b = ((abgr >> 16) & 0xFF) as u8;
                            let a = ((abgr >> 24) & 0xFF) as u8;
                            let new_col = Color::new(r, g, b, if a == 0 { 255 } else { a });

                            if let Some(ref mut forced) = force_preset {
                                forced.color = new_col;
                            } else {
                                config.desktop.color = new_col;
                                let _ = config.save(&config_path);
                            }
                        }
                        ACTION_TOGGLE_DYNAMIC => {
                            config.dynamic_modes_enabled = !config.dynamic_modes_enabled;
                            if config.dynamic_modes_enabled {
                                force_preset = None;
                            }
                            let _ = config.save(&config_path);
                        }
                        ACTION_TOGGLE_SMOOTH => {
                            config.smooth_transitions = !config.smooth_transitions;
                            let _ = config.save(&config_path);
                        }
                        ACTION_TOGGLE_GAME_MODE => {
                            config.game_mode_pause = !config.game_mode_pause;
                            let _ = config.save(&config_path);
                        }
                        ACTION_TOGGLE_BORDER => {
                            if let Some(ref mut forced) = force_preset {
                                forced.show_line = !forced.show_line;
                            } else {
                                config.desktop.show_line = !config.desktop.show_line;
                                let _ = config.save(&config_path);
                            }
                        }
                        ACTION_OPEN_CONFIG => {
                            let _ = std::process::Command::new("notepad.exe")
                                .arg(&config_path)
                                .spawn();
                        }
                        ACTION_RESTART_EXPLORER => {
                            let _ = std::process::Command::new("cmd.exe")
                                .args(&["/c", "taskkill /f /im explorer.exe & start explorer.exe"])
                                .spawn();
                        }
                        ACTION_EXIT => {
                            flyout.hide();
                            RUNNING.store(false, Ordering::Relaxed);
                        }
                        _ => {}
                    }

                    let state = force_preset.as_ref().map(|f| f.state).unwrap_or(current_appearance.state);
                    let color = force_preset.as_ref().map(|f| f.color).unwrap_or(current_appearance.color);
                    let border = force_preset.as_ref().map(|f| f.show_line).unwrap_or(current_appearance.show_line);

                    flyout.update_state(FlyoutState {
                        active_state: state,
                        is_forced: force_preset.is_some(),
                        dynamic_enabled: config.dynamic_modes_enabled,
                        smooth_enabled: config.smooth_transitions,
                        game_pause_enabled: config.game_mode_pause,
                        show_border: border,
                        current_color: color,
                    });
                }

                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // B. Handle IPC commands
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                IpcCommand::SetState(state_str) => match state_str.to_lowercase().as_str() {
                    "clear" => {
                        force_preset = Some(StateAppearance {
                            state: TaskbarState::Clear,
                            color: Color::transparent(),
                            show_line: false,
                        });
                    }
                    "blur" => {
                        force_preset = Some(StateAppearance {
                            state: TaskbarState::Blur,
                            color: Color::new(20, 20, 20, 120),
                            show_line: false,
                        });
                    }
                    "acrylic" => {
                        force_preset = Some(StateAppearance {
                            state: TaskbarState::Acrylic,
                            color: Color::new(30, 30, 30, 180),
                            show_line: false,
                        });
                    }
                    "opaque" => {
                        force_preset = Some(StateAppearance {
                            state: TaskbarState::Opaque,
                            color: Color::new(0, 0, 0, 255),
                            show_line: false,
                        });
                    }
                    "normal" => {
                        force_preset = Some(StateAppearance {
                            state: TaskbarState::Normal,
                            color: Color::transparent(),
                            show_line: true,
                        });
                    }
                    _ => {}
                },
                IpcCommand::SetColor { r, g, b, a } => {
                    let col = Color::new(r, g, b, a);
                    if let Some(ref mut forced) = force_preset {
                        forced.color = col;
                    } else {
                        config.desktop.color = col;
                        let _ = config.save(&config_path);
                    }
                }
                IpcCommand::ToggleDynamic => {
                    if force_preset.is_some() {
                        force_preset = None;
                    } else {
                        force_preset = Some(StateAppearance {
                            state: TaskbarState::Clear,
                            color: Color::transparent(),
                            show_line: false,
                        });
                    }
                }
                IpcCommand::Reload => {
                    config = Config::load_or_create(&config_path);
                    force_preset = None;
                }
                IpcCommand::GetStatus => {}
                IpcCommand::Quit => {
                    RUNNING.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }

        // C. Evaluate Dynamic State (Polled at 50ms intervals)
        if last_poll.elapsed() >= Duration::from_millis(50) {
            last_poll = Instant::now();

            let target_appearance = if let Some(ref forced) = force_preset {
                forced.clone()
            } else if config.dynamic_modes_enabled {
                let (detected_state, appearance) = StateDetector::detect_current_state(&config);
                if detected_state != last_state {
                    last_state = detected_state;
                }
                appearance
            } else {
                config.desktop.clone()
            };

            // D. Check if appearance needs transition
            let target_changed = target_appearance.state != current_appearance.state
                || target_appearance.color != current_appearance.color
                || target_appearance.show_line != current_appearance.show_line;

            if target_changed {
                if config.smooth_transitions {
                    transition = Some(StateTransition::new(
                        current_appearance.clone(),
                        target_appearance,
                        config.transition_duration_ms,
                    ));
                } else {
                    current_appearance = target_appearance;
                    taskbar.apply_to_all(&current_appearance);
                }
            }
        }

        // E. Step active transition animation (60fps ticks during animation)
        if let Some(ref mut tr) = transition {
            let (step_app, active) = tr.step();
            current_appearance = step_app;
            taskbar.apply_to_all(&current_appearance);

            if !active {
                transition = None;
            }
            std::thread::sleep(Duration::from_millis(16));
        } else {
            // Low-power sleep when settled (0% CPU)
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    println!("[*] Restoring taskbar before exit...");
    taskbar.restore_all();
    println!("[+] Mutatio exited cleanly.");
}
