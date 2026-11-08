# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Mutatio is a Windows-only (x86_64, Win10/11) taskbar appearance daemon written in Rust (edition 2024, `windows` crate 0.62). Two binaries share the code:
- `mutatio` (`src/main.rs`, GUI subsystem in release): the tray daemon. It also accepts CLI flags.
- `mutatio-cli` (`src/bin/mutatio-cli.rs`, console subsystem): pulls in `cli.rs`/`ipc.rs`/`config.rs` via `#[path]` and is installed as `mutatio.com`. A bare `mutatio` in a terminal resolves `.com` before `.exe`, so CLI output works in PowerShell while autostart/shortcuts get no console window.

The version lives only in `Cargo.toml`. `build.rs` fills the `mutatio.rc` template (icon + VERSIONINFO) from it, and the installer reads it back from the built exe.

## Commands

```bash
cargo build                 # debug: console subsystem, daemon logs visible
cargo build --release       # LTO, panic=abort, stripped, GUI subsystem → target/release/mutatio.exe
cargo test                  # unit tests for pure logic (colors, config parsing, transitions, hit-testing)
cargo test hex_parsing      # single test by name filter
cargo clippy --all-targets  # keep this warning-free
cargo run                   # start the daemon
cargo run -- --status       # CLI client mode against a running daemon (prints per-taskbar state)
powershell -ExecutionPolicy Bypass -File installer\build.ps1   # release build + Inno Setup installer → dist\
```

The build needs the Windows SDK (`rc.exe`, found by `embed-resource`). The installer needs Inno Setup 6. `installer/build.ps1` generates the wizard BMPs from `Mutatio.jpg` into `installer/build/` (gitignored).

Anything touching the shell must be verified manually: run the daemon, drive it with the CLI (`--status`, `--blur`, `--color`, `--toggle`, `--quit`), and look at the real taskbar. `--toggle` and flyout toggles persist to the user's real `%APPDATA%\Mutatio\config.json` — restore it after testing. Only one daemon can run. The named mutex `Local\Mutatio.Daemon` is *owned* by the daemon, so `--quit` blocks on it until the process has really exited; the uninstaller relies on this to delete the exe. Quit any running instance before testing.

## Architecture

**Event-driven main loop** (`src/main.rs`). All state lives in `App` on the main (STA) thread. The loop drains messages, drains IPC requests, handles explorer restarts, re-runs detection if `DETECT_DIRTY` is set, steps animations, then blocks in `MsgWaitForMultipleObjectsEx`. Wake sources:
- out-of-context WinEvent hooks (foreground, minimize, show/hide, location change, cloak) → `win_event_proc` only sets `DETECT_DIRTY`; detection is coalesced and capped at ~60 Hz;
- `WM_MUTATIO_IPC` posted by the pipe thread;
- 16 ms frames only while a transition runs; a 1 s safety poll otherwise.

Don't reintroduce unconditional sleeps/polling in the loop.

**Per-taskbar slots.** Each `Shell_TrayWnd` / `Shell_SecondaryTrayWnd` is a `Slot` with its own monitor, detected `ActiveState`, current appearance and transition. `StateDetector::detect` (`src/monitor.rs`) does one `EnumWindows` pass and returns a state per monitor: Start/Search/fullscreen apply only to the foreground window's monitor; maximized/visible are any qualifying window on that monitor. Start/Search are identified by owning process (`StartMenuExperienceHost.exe`, `SearchHost.exe`, …), not by the localized window title.

**Forced vs dynamic vs fixed.** `force_preset: Option<StateAppearance>` (flyout presets, `--clear` etc., IPC `set_state`) overrides everything. Otherwise `config.dynamic_modes_enabled` chooses detection vs. a fixed `config.desktop`. `App::toggle_dynamic` is the single implementation used by both the flyout switch and `--toggle`. Preset looks come from `StateAppearance::preset` and names from `TaskbarState::parse`/`name` — don't duplicate them.

**Two rendering backends** (`src/taskbar.rs`):
- *Modern (Win11 XAML)*: `assets/ExplorerTAP.dll` (a prebuilt TranslucentTB binary) is embedded, written to `%LOCALAPPDATA%\Mutatio\` when its contents differ, and its `InjectExplorerTAP` export injects into explorer and returns an `ITaskbarAppearanceService` COM proxy. Methods are called by raw vtable slot (3–7); slot order must match the DLL exactly.
- *Fallback (Win10 / ExplorerPatcher)*: `SetWindowCompositionAttribute` + DWM attributes. Explorer resets these on its own, so `update_targets` re-pushes them every detection pass; the modern engine doesn't need that.

**Explorer restarts.** `TaskbarCreated` (registered message, needs a real top-level window, not `HWND_MESSAGE`) sets `SHELL_RESTARTED` → tray icon re-added, slots dropped, `poll_reconnect` retries ExplorerTAP for ~15 s. A COM call returning an RPC "server died" HRESULT sets `is_disconnected()` and triggers the same retry. Restoring is explicit (`restore_all` on exit); `Drop` impls only release. The COM proxy must be dropped before `CoUninitialize` (handled in `TaskbarController::drop`).

**Colors**: `Color` is RGBA in config/JSON; backends and window-message payloads use ABGR (`to_abgr` / `from_abgr`).

**IPC** (`src/ipc.rs`): pipe `\\.\pipe\mutatio`, message mode, remote clients rejected, one JSON request/response per connection. The server forwards an `IpcRequest { command, reply }` to the main thread and waits up to 3 s for the real response. Wire format is serde-tagged: `{"action":"set_state","payload":"blur"}`.

**Flyout UI** (`src/flyout.rs`): custom-painted GDI popup. Click geometry lives in `HIT_ZONES` and must match the drawing coordinates in `render_flyout`; swatch colors live in `SWATCHES` (used for both clicks and painting). Fonts are cached in a thread-local. All flyout state is thread-local because it's only touched on the UI thread. The "Restart Shell" button kills and restarts `explorer.exe`.

**Config** (`%APPDATA%\Mutatio\config.json`): `#[serde(default)]` everywhere, so missing keys fall back to defaults. An unparsable file is copied to `config.json.bak` before defaults are written. Saves are atomic (temp file + rename). `battery_saver_fallback` is persisted but not implemented.

## Repo notes

- When `mutatio.exe` (GUI subsystem) is given CLI flags it calls `AttachConsole(ATTACH_PARENT_PROCESS)`, but PowerShell does not wait for GUI programs, so the output is unreliable; `mutatio.com` exists for that reason. A daemon spawned by the CLI is started detached with null stdio.
- The README is marketing-heavy; trust the code. It criticizes TranslucentTB for DLL injection, yet the modern backend injects TranslucentTB's own `ExplorerTAP.dll`, which still contains TranslucentTB internals (e.g. the `TTBTAP_Ready` event name). Renaming those requires rebuilding the DLL from source.
