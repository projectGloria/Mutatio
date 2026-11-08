// Release builds are GUI-subsystem so double-clicking the exe doesn't leave a console window
// behind. CLI mode re-attaches to the parent console (see `attach_parent_console`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
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
use ipc::{IpcCommand, IpcRequest, IpcResponse, IpcServer};
use cli::{handle_cli_client, DAEMON_MUTEX};
use monitor::{ActiveState, StateDetector};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use taskbar::TaskbarController;
use transition::StateTransition;
use tray::{TrayIcon, WM_TRAYICON};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetAncestor, GetCursorPos,
    MsgWaitForMultipleObjectsEx, PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, TranslateMessage, CHILDID_SELF, EVENT_OBJECT_CLOAKED,
    EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GA_ROOT, MSG,
    MWMO_INPUTAVAILABLE, OBJID_WINDOW, PM_REMOVE, QS_ALLINPUT, WINDOW_EX_STYLE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WM_COMMAND, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_LBUTTONUP, WM_QUIT, WM_SETTINGCHANGE, WNDCLASSW, WS_OVERLAPPED,
};

static RUNNING: AtomicBool = AtomicBool::new(true);
/// Set once the taskbar has been restored on exit; the console close handler waits for it.
static CLEANED_UP: AtomicBool = AtomicBool::new(false);
/// Raised by WinEvent hooks / IPC: "something on screen changed, re-run detection".
static DETECT_DIRTY: AtomicBool = AtomicBool::new(true);
/// Raised when explorer.exe (re)starts and broadcasts "TaskbarCreated".
static SHELL_RESTARTED: AtomicBool = AtomicBool::new(false);
/// Raised on display/theme changes: re-push appearance even if our target didn't change.
static FORCE_REAPPLY: AtomicBool = AtomicBool::new(false);
static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

const ID_TOGGLE_FLYOUT: usize = 2002;
/// Posted by the console ctrl handler purely to wake the idle wait.
const WM_MUTATIO_WAKE: u32 = WM_APP + 2;

/// Safety net for state changes no WinEvent covers. 1 Hz instead of the old 40 Hz poll.
const SAFETY_POLL: Duration = Duration::from_secs(1);
/// Upper bound on detection rate while events stream in (e.g. dragging a window).
const MIN_DETECT_INTERVAL: Duration = Duration::from_millis(16);
const FRAME: Duration = Duration::from_millis(16);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);
const RECONNECT_ATTEMPTS: u32 = 15;

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                if lparam.0 as u32 == WM_LBUTTONUP {
                    let _ = PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(ID_TOGGLE_FLYOUT), LPARAM(0));
                }
                // Right-click intentionally does nothing.
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                // Monitors added/removed or theme changed: explorer may have rebuilt taskbars or
                // reset their styling under us.
                FORCE_REAPPLY.store(true, Ordering::Relaxed);
                DETECT_DIRTY.store(true, Ordering::Relaxed);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            m if m != 0 && m == WM_TASKBAR_CREATED.load(Ordering::Relaxed) => {
                SHELL_RESTARTED.store(true, Ordering::Relaxed);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Out-of-context WinEvent callback; runs on the main thread inside PeekMessage.
/// Only flags work — detection itself is coalesced and rate-limited by the main loop.
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Ignore caret/cursor/child-object noise: LOCATIONCHANGE fires on every mouse move.
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    if matches!(event, EVENT_OBJECT_LOCATIONCHANGE | EVENT_OBJECT_SHOW | EVENT_OBJECT_HIDE)
        && unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
    {
        return; // child window moved/shown — can't change the taskbar state
    }
    DETECT_DIRTY.store(true, Ordering::Relaxed);
}

unsafe extern "system" fn console_ctrl_handler(ctrl: u32) -> BOOL {
    RUNNING.store(false, Ordering::Relaxed);
    wake_main_loop();
    if ctrl != CTRL_C_EVENT && ctrl != CTRL_BREAK_EVENT {
        // CLOSE / LOGOFF / SHUTDOWN: the process is killed as soon as we return, so give the
        // main thread a moment to restore the taskbar first.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !CLEANED_UP.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    BOOL(1)
}

fn wake_main_loop() {
    let raw = MAIN_HWND.load(Ordering::Relaxed);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_MUTATIO_WAKE, WPARAM(0), LPARAM(0));
        }
    }
}

/// One taskbar (primary or secondary) and its animation state.
struct Slot {
    hwnd: HWND,
    monitor: HMONITOR,
    state: ActiveState,
    current: StateAppearance,
    transition: Option<StateTransition>,
    /// False until we've pushed an appearance to this taskbar (new monitor, explorer restart).
    applied: bool,
}

impl Slot {
    fn target(&self) -> StateAppearance {
        self.transition.as_ref().map_or(self.current, |t| t.target())
    }
}

struct App {
    config: Config,
    config_path: PathBuf,
    taskbar: TaskbarController,
    slots: Vec<Slot>,
    force_preset: Option<StateAppearance>,
    /// Whether the modern engine was ever available — only then is reconnecting worth trying.
    wants_modern: bool,
    reconnect_attempts_left: u32,
    next_reconnect: Instant,
}

impl App {
    fn save_config(&self) {
        if let Err(e) = self.config.save(&self.config_path) {
            eprintln!("[!] Failed to save config: {e}");
        }
    }

    /// Re-enumerates taskbars, keeping animation state for ones that still exist.
    fn refresh_slots(&mut self) {
        let hwnds = self.taskbar.find_taskbars();
        let mut old = std::mem::take(&mut self.slots);
        self.slots = hwnds
            .into_iter()
            .map(|hwnd| {
                let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
                match old.iter().position(|s| s.hwnd == hwnd) {
                    Some(i) => Slot { monitor, ..old.swap_remove(i) },
                    None => Slot {
                        hwnd,
                        monitor,
                        state: ActiveState::Desktop,
                        current: self.config.desktop,
                        transition: None,
                        applied: false,
                    },
                }
            })
            .collect();
    }

    /// Recomputes every taskbar's target and starts transitions where it changed.
    fn update_targets(&mut self) {
        self.refresh_slots(); // cheap (a few FindWindowEx calls) and catches new monitors
        if FORCE_REAPPLY.swap(false, Ordering::Relaxed) {
            self.slots.iter_mut().for_each(|s| s.applied = false);
        }

        let states: Vec<ActiveState> = if self.force_preset.is_some() || !self.config.dynamic_modes_enabled {
            vec![ActiveState::Desktop; self.slots.len()]
        } else {
            let monitors: Vec<HMONITOR> = self.slots.iter().map(|s| s.monitor).collect();
            StateDetector::detect(&self.config, &monitors)
        };

        // The SWCA fallback gets reset by explorer on its own (start menu, focus changes), so it
        // has to be re-pushed every pass. The modern XAML engine keeps what we set.
        let reassert = !self.taskbar.is_modern();

        for (slot, state) in self.slots.iter_mut().zip(states) {
            slot.state = state;
            let target = self.force_preset.unwrap_or_else(|| state.appearance(&self.config));

            if !slot.applied {
                slot.transition = None;
                slot.current = target;
                self.taskbar.apply(slot.hwnd, &target);
                slot.applied = true;
            } else if target != slot.target() {
                if self.config.smooth_transitions && self.config.transition_duration_ms > 0 {
                    slot.transition =
                        Some(StateTransition::new(slot.current, target, self.config.transition_duration_ms));
                } else {
                    slot.transition = None;
                    slot.current = target;
                    self.taskbar.apply(slot.hwnd, &target);
                }
            } else if reassert && slot.transition.is_none() {
                self.taskbar.apply(slot.hwnd, &slot.current);
            }
        }
    }

    /// Advances running animations by one frame. Returns true while any is still running.
    fn step_transitions(&mut self) -> bool {
        let mut animating = false;
        for slot in &mut self.slots {
            if let Some(tr) = &slot.transition {
                let (appearance, active) = tr.step();
                slot.current = appearance;
                self.taskbar.apply(slot.hwnd, &appearance);
                if active {
                    animating = true;
                } else {
                    slot.transition = None;
                }
            }
        }
        animating
    }

    fn on_shell_restarted(&mut self) {
        println!("[*] Explorer restarted; reconnecting");
        self.slots.clear(); // old HWNDs are dead
        self.reconnect_attempts_left = if self.wants_modern { RECONNECT_ATTEMPTS } else { 0 };
        self.next_reconnect = Instant::now();
        DETECT_DIRTY.store(true, Ordering::Relaxed);
    }

    /// Retries the ExplorerTAP connection; explorer's XAML may not be ready the moment its
    /// window appears. Returns how long until the next attempt, if any remain.
    fn poll_reconnect(&mut self) -> Option<Duration> {
        if self.taskbar.is_disconnected() && self.reconnect_attempts_left == 0 {
            // A COM call failed without a TaskbarCreated (explorer crashed and is still down).
            self.reconnect_attempts_left = RECONNECT_ATTEMPTS;
        }
        if self.reconnect_attempts_left == 0 {
            return None;
        }
        let now = Instant::now();
        if now < self.next_reconnect {
            return Some(self.next_reconnect - now);
        }

        self.reconnect_attempts_left -= 1;
        if self.taskbar.reconnect() {
            println!("[+] Modern taskbar engine reconnected");
            self.reconnect_attempts_left = 0;
            self.slots.iter_mut().for_each(|s| s.applied = false);
            DETECT_DIRTY.store(true, Ordering::Relaxed);
            None
        } else if self.reconnect_attempts_left == 0 {
            println!("[!] Could not reconnect modern engine; using fallback");
            None
        } else {
            self.next_reconnect = now + RECONNECT_INTERVAL;
            Some(RECONNECT_INTERVAL)
        }
    }

    fn set_preset(&mut self, state: TaskbarState) {
        self.force_preset = Some(StateAppearance::preset(state));
    }

    fn set_color(&mut self, color: Color) {
        match self.force_preset.as_mut() {
            Some(forced) => forced.color = color,
            None => {
                self.config.desktop.color = color;
                self.save_config();
            }
        }
    }

    /// Shared by the flyout switch and `--toggle` (they used to do two different things).
    /// "On" means: drop any forced preset and let detection drive.
    fn toggle_dynamic(&mut self) -> bool {
        let currently_on = self.config.dynamic_modes_enabled && self.force_preset.is_none();
        if currently_on {
            self.config.dynamic_modes_enabled = false;
        } else {
            self.config.dynamic_modes_enabled = true;
            self.force_preset = None;
        }
        self.save_config();
        !currently_on
    }

    fn mode_name(&self) -> &'static str {
        match (&self.force_preset, self.config.dynamic_modes_enabled) {
            (Some(_), _) => "forced",
            (None, true) => "dynamic",
            (None, false) => "fixed",
        }
    }

    fn status(&self) -> IpcResponse {
        let bars: Vec<String> = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let t = s.target();
                let src = if self.force_preset.is_some() { "forced" } else { s.state.name() };
                format!(
                    "  taskbar {i}: {src} -> {} #{:02X}{:02X}{:02X}{:02X}",
                    t.state.name(),
                    t.color.r,
                    t.color.g,
                    t.color.b,
                    t.color.a
                )
            })
            .collect();
        let mut resp = IpcResponse::ok(format!(
            "Mutatio v{} | mode: {} | engine: {} | {} taskbar(s)\n{}",
            env!("CARGO_PKG_VERSION"),
            self.mode_name(),
            self.taskbar.engine_name(),
            self.slots.len(),
            bars.join("\n")
        ));
        resp.active_state = self.slots.first().map(|s| s.target().state.name().to_string());
        resp
    }

    fn handle_ipc(&mut self, cmd: IpcCommand) -> IpcResponse {
        DETECT_DIRTY.store(true, Ordering::Relaxed);
        match cmd {
            IpcCommand::SetState(name) => match TaskbarState::parse(&name) {
                Some(state) => {
                    self.set_preset(state);
                    IpcResponse::ok(format!("Taskbar forced to {}", state.name()))
                }
                None => IpcResponse::err(format!("Unknown state '{name}'")),
            },
            IpcCommand::SetColor { r, g, b, a } => {
                self.set_color(Color::new(r, g, b, a));
                IpcResponse::ok(format!("Color set to #{r:02X}{g:02X}{b:02X}{a:02X}"))
            }
            IpcCommand::ToggleDynamic => {
                let on = self.toggle_dynamic();
                IpcResponse::ok(format!("Dynamic mode {}", if on { "enabled" } else { "disabled" }))
            }
            IpcCommand::Reload => {
                self.config = Config::load_or_create(&self.config_path);
                self.force_preset = None;
                IpcResponse::ok("Configuration reloaded")
            }
            IpcCommand::GetStatus => self.status(),
            IpcCommand::Quit => {
                RUNNING.store(false, Ordering::Relaxed);
                IpcResponse::ok("Mutatio is exiting")
            }
        }
    }

    fn handle_flyout_action(&mut self, action: usize, lparam: LPARAM) {
        DETECT_DIRTY.store(true, Ordering::Relaxed);
        match action {
            ACTION_SET_CLEAR => self.set_preset(TaskbarState::Clear),
            ACTION_SET_BLUR => self.set_preset(TaskbarState::Blur),
            ACTION_SET_ACRYLIC => self.set_preset(TaskbarState::Acrylic),
            ACTION_SET_OPAQUE => self.set_preset(TaskbarState::Opaque),
            ACTION_SET_DEFAULT => self.set_preset(TaskbarState::Normal),
            ACTION_SET_COLOR => {
                let c = Color::from_abgr(lparam.0 as u32);
                self.set_color(Color { a: if c.a == 0 { 255 } else { c.a }, ..c });
            }
            ACTION_TOGGLE_DYNAMIC => {
                self.toggle_dynamic();
            }
            ACTION_TOGGLE_SMOOTH => {
                self.config.smooth_transitions = !self.config.smooth_transitions;
                self.save_config();
            }
            ACTION_TOGGLE_GAME_MODE => {
                self.config.game_mode_pause = !self.config.game_mode_pause;
                self.save_config();
            }
            ACTION_TOGGLE_BORDER => match self.force_preset.as_mut() {
                Some(forced) => forced.show_line = !forced.show_line,
                None => {
                    self.config.desktop.show_line = !self.config.desktop.show_line;
                    self.save_config();
                }
            },
            ACTION_OPEN_CONFIG => {
                if !self.config_path.exists() {
                    self.save_config();
                }
                let _ = Command::new("notepad.exe").arg(&self.config_path).spawn();
            }
            ACTION_RESTART_EXPLORER => {
                // Off the UI thread: taskkill takes a few hundred ms. CREATE_NO_WINDOW stops the
                // old `cmd /c` approach from flashing a console window.
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                std::thread::spawn(|| {
                    let _ = Command::new("taskkill")
                        .args(["/f", "/im", "explorer.exe"])
                        .creation_flags(CREATE_NO_WINDOW)
                        .status();
                    let _ = Command::new("explorer.exe").spawn();
                });
                // TaskbarCreated will trigger the reconnect once the new shell is up.
            }
            ACTION_EXIT => RUNNING.store(false, Ordering::Relaxed),
            _ => {}
        }
    }

    fn flyout_state(&self) -> FlyoutState {
        let shown = self
            .force_preset
            .or_else(|| self.slots.first().map(Slot::target))
            .unwrap_or(self.config.desktop);
        FlyoutState {
            active_state: shown.state,
            is_forced: self.force_preset.is_some(),
            dynamic_enabled: self.config.dynamic_modes_enabled,
            smooth_enabled: self.config.smooth_transitions,
            game_pause_enabled: self.config.game_mode_pause,
            show_border: shown.show_line,
            current_color: shown.color,
            engine: self.taskbar.engine_name(),
        }
    }
}

fn install_win_event_hooks() -> Vec<HWINEVENTHOOK> {
    let ranges = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];
    ranges
        .iter()
        .filter_map(|&(min, max)| {
            let hook = unsafe {
                SetWinEventHook(
                    min,
                    max,
                    None,
                    Some(win_event_proc),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            (!hook.is_invalid()).then_some(hook)
        })
        .collect()
}

fn create_message_window() -> HWND {
    let class_name = w!("MutatioMsgWindowClass");
    unsafe {
        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wnd_class);

        // A real (hidden) top-level window, not HWND_MESSAGE: message-only windows don't receive
        // broadcasts such as TaskbarCreated / WM_SETTINGCHANGE.
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
        .unwrap_or_default()
    }
}

fn drain_ipc(app: &mut App, rx: &Receiver<IpcRequest>) {
    while let Ok(req) = rx.try_recv() {
        let resp = app.handle_ipc(req.command);
        let _ = req.reply.send(resp);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if handle_cli_client(&args) {
        return;
    }

    // Single instance. Two daemons would fight over the taskbar every frame, and the pipe server
    // happily allowed it (PIPE_UNLIMITED_INSTANCES). Handle intentionally leaked: it must live
    // for the whole process. Initially *owned* on purpose: `--quit` waits on it, and an owned
    // mutex becomes signalled (abandoned) exactly when this process dies.
    unsafe {
        let _mutex = CreateMutexW(None, true, DAEMON_MUTEX);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            eprintln!("[!] Mutatio is already running");
            return;
        }
    }

    println!("[*] Starting Mutatio Daemon...");

    let initial_forced_preset = match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("--daemon-start"), Some(name)) => TaskbarState::parse(name).map(StateAppearance::preset),
        _ => None,
    };

    let config_path = Config::default_path();
    let config = Config::load_or_create(&config_path);
    println!("[+] Configuration loaded from: {:?}", config_path);

    let taskbar = TaskbarController::new();

    let hwnd = create_message_window();
    MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    WM_TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, Ordering::Relaxed);
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(console_ctrl_handler), true);
    }

    let tray = TrayIcon::create(hwnd);
    println!("[+] System tray icon initialized");

    let flyout = FlyoutWindow::create(hwnd);
    println!("[+] Modern Fluent Flyout UI initialized");

    let (cmd_tx, cmd_rx) = channel::<IpcRequest>();
    IpcServer::start(cmd_tx, hwnd.0 as isize);
    println!("[+] Named Pipe Server listening on {}", ipc::PIPE_NAME);

    let hooks = install_win_event_hooks();
    println!("[+] Installed {} window event hooks", hooks.len());

    let wants_modern = taskbar.is_modern();
    let mut app = App {
        config,
        config_path,
        taskbar,
        slots: Vec::new(),
        force_preset: initial_forced_preset,
        wants_modern,
        reconnect_attempts_left: 0,
        next_reconnect: Instant::now(),
    };
    app.update_targets();
    println!("[+] Found {} taskbar instances", app.slots.len());

    let mut last_detect = Instant::now();
    let mut animating = false;

    while RUNNING.load(Ordering::Relaxed) {
        // A. Drain the message queue (WinEvent callbacks and window procs run in here too).
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                match msg.message {
                    WM_QUIT => RUNNING.store(false, Ordering::Relaxed),
                    WM_COMMAND if msg.wParam.0 == ID_TOGGLE_FLYOUT => {
                        let mut pt = POINT::default();
                        let _ = GetCursorPos(&mut pt);
                        flyout.update_state(app.flyout_state());
                        flyout.toggle(pt);
                    }
                    WM_FLYOUT_ACTION => {
                        app.handle_flyout_action(msg.wParam.0, msg.lParam);
                        if msg.wParam.0 == ACTION_EXIT {
                            flyout.hide();
                        }
                        // Re-run detection now so the flyout shows the post-action state.
                        app.update_targets();
                        flyout.update_state(app.flyout_state());
                    }
                    _ => {}
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // B. IPC commands (the pipe thread posted WM_MUTATIO_IPC to wake us).
        drain_ipc(&mut app, &cmd_rx);
        if !RUNNING.load(Ordering::Relaxed) {
            break;
        }

        // C. Explorer restarts / dead COM connection.
        if SHELL_RESTARTED.swap(false, Ordering::Relaxed) {
            tray.add();
            app.on_shell_restarted();
        }
        let reconnect_wait = app.poll_reconnect();

        // D. Event-driven detection, coalesced and capped at ~60 Hz, plus a slow safety poll.
        let now = Instant::now();
        let since_detect = now - last_detect;
        let mut detect_wait = None;
        if DETECT_DIRTY.load(Ordering::Relaxed) || since_detect >= SAFETY_POLL {
            if since_detect >= MIN_DETECT_INTERVAL {
                DETECT_DIRTY.store(false, Ordering::Relaxed);
                app.update_targets();
                last_detect = Instant::now();
            } else {
                detect_wait = Some(MIN_DETECT_INTERVAL - since_detect);
            }
        }

        // E. Animation frame.
        if animating || app.slots.iter().any(|s| s.transition.is_some()) {
            animating = app.step_transitions();
        }

        // F. Sleep until something happens. The old loop woke 40x/s forever; idle is now one
        // wake-up per second, and zero CPU between window events.
        let safety_left = SAFETY_POLL.saturating_sub(last_detect.elapsed());
        let timeout = [Some(safety_left), detect_wait, reconnect_wait, animating.then_some(FRAME)]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(SAFETY_POLL);
        unsafe {
            MsgWaitForMultipleObjectsEx(None, timeout.as_millis() as u32, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }

    // Answer any client still waiting (e.g. the `--quit` that triggered this exit).
    drain_ipc(&mut app, &cmd_rx);

    for hook in hooks {
        unsafe {
            let _ = UnhookWinEvent(hook);
        }
    }

    println!("[*] Restoring taskbar before exit...");
    app.taskbar.restore_all();
    CLEANED_UP.store(true, Ordering::Release);
    println!("[+] Mutatio exited cleanly.");
}
