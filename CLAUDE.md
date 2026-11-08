# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Mutatio is a Windows-only (x86_64, Win10/11) taskbar appearance daemon written in Rust (edition 2024, `windows` crate 0.62). Single binary: the same `mutatio.exe` acts as the tray daemon or as a CLI client that talks to a running daemon over a named pipe.

## Commands

```bash
cargo build                 # debug
cargo build --release       # LTO, codegen-units=1, panic=abort, stripped → target/release/mutatio.exe
cargo clippy --all-targets
cargo run                   # starts the daemon (tray icon + flyout); Ctrl+C or `--quit` from another shell
cargo run -- --status       # CLI client mode against a running daemon
cargo run -- --clear        # if no daemon is running, spawns one with `--daemon-start clear`
```

There are no tests yet (`cargo test` runs nothing). Verification is manual: run the daemon and observe the real taskbar. Exiting the daemon restores the stock taskbar; if it crashes mid-transition, use `--normal` or restart explorer.

## Architecture

**Single-threaded main loop** (`src/main.rs`). `main()` owns all state (`config`, `force_preset`, `current_appearance`, `transition`) and runs a `PeekMessageW` loop rather than a blocking `GetMessage` loop. Each iteration:
1. Drains Win32 messages — tray clicks (`WM_TRAYICON` → `WM_COMMAND`/`ID_TOGGLE_FLYOUT`) and flyout actions (`WM_FLYOUT_ACTION`, `wParam` = `ACTION_*` constant, `lParam` = packed ABGR color for `ACTION_SET_COLOR`).
2. Drains IPC commands from an `mpsc` channel fed by the pipe thread.
3. Every 50 ms, computes the target appearance: `force_preset` wins; else `StateDetector` (if `dynamic_modes_enabled`); else `config.desktop`.
4. If the target differs, starts a `StateTransition` (or applies instantly), then steps it at ~16 ms; otherwise sleeps 25 ms.

Note: despite the README's "0% idle CPU" claim, this is a polling loop (25 ms sleep / 50 ms detection). Don't describe it as event-driven.

**Forced vs dynamic mode.** `force_preset: Option<StateAppearance>` is the "forced" override set by flyout presets, CLI `--clear/--blur/...`, and IPC `set_state`. `None` means dynamic detection is in charge. Color/border edits mutate `force_preset` if set, otherwise they mutate and persist `config.desktop`.

**Preset definitions are duplicated** in three places in `main.rs`: `--daemon-start` args, the `WM_FLYOUT_ACTION` handler, and the `IpcCommand::SetState` handler. Change a preset's color in all three (or refactor into one function).

**Two rendering backends** (`src/taskbar.rs`, `TaskbarController`):
- *Modern (Win11 XAML taskbar)*: `assets/ExplorerTAP.dll` is embedded via `include_bytes!`, written to `%LOCALAPPDATA%\Mutatio\ExplorerTAP.dll` (rewritten only when the file size differs), loaded, and its `InjectExplorerTAP` export injects into explorer to return an `ITaskbarAppearanceService` COM pointer. The vtable is called manually by index (slots 2–7: Release, SetTaskbarAppearance, SetTaskbarBlur, ReturnTaskbarToDefaultAppearance, SetTaskbarBorderVisibility, RestoreAllTaskbarsToDefault). Slot order must match the DLL exactly — there is no type checking.
- *Fallback (Win10 / ExplorerPatcher)*: undocumented `SetWindowCompositionAttribute` (`WCA_ACCENT_POLICY`) on `Shell_TrayWnd` / `Shell_SecondaryTrayWnd`, plus `DwmSetWindowAttribute` for backdrop type and border color.

The choice is made once at `TaskbarController::new()`; if explorer restarts, the TAP service pointer is stale and there's no reconnection logic. `apply_to_all` re-enumerates taskbar HWNDs on every call (including every animation frame). `Drop` for both `TaskbarAppearanceService` and `TaskbarController` restores defaults, and the controller calls `CoUninitialize` — keep the controller on the main (STA) thread.

**State detection** (`src/monitor.rs`): priority order is Start menu → Search (both `Windows.UI.Core.CoreWindow` found by title, checked for DWM cloaking) → fullscreen "game" (foreground window covering the whole monitor; forces `Normal`) → maximized → visible window → desktop. `ActiveState` is computed but main currently only uses the returned `StateAppearance`.

**Transitions** (`src/transition.rs`): ease-out cubic lerp of RGBA only; the discrete `TaskbarState` (brush type) flips at 50 % progress.

**Colors** (`src/config.rs`): `Color` is RGBA in config/JSON; backends expect ABGR (`0xAABBGGRR`, `to_abgr()`). The flyout packs swatch colors the same way into `lParam`. Opaque state forces alpha 255 and remaps near-black to pure black in the backend, not in config.

**IPC** (`src/ipc.rs`): pipe `\\.\pipe\mutatio`, message mode, one JSON request/response per connection, 1 KB buffers. `IpcCommand` is serde-tagged `{"action": "...", "payload": ...}` (e.g. `{"action":"set_state","payload":"blur"}`). The server replies `success: true` as soon as the command is queued — it does not report daemon state, so `--status` returns no real status. The pipe has no ACL beyond defaults.

**Flyout UI** (`src/flyout.rs`): a custom-painted GDI popup (no XAML/controls). Layout is hardcoded pixel geometry: `hit_test()` zone rectangles must be kept in sync by hand with the drawing code in `WM_PAINT`. Shared state goes through the `FLYOUT_DATA` mutex and `PARENT_NOTIFY_HWND` atomic; actions are posted back to the main window, never handled in the flyout. The "Reload" button (`ZONE_BTN_RELOAD`) maps to `ACTION_RESTART_EXPLORER`, which kills and restarts `explorer.exe`.

**Config**: `%APPDATA%\Mutatio\config.json` (falls back to `./mutatio.json`). Parse failure silently replaces the file with defaults — a typo in the user's config wipes it. All fields are required by serde (no `#[serde(default)]`), so adding a field to `Config` invalidates every existing config file unless you add defaults. `battery_saver_fallback` is persisted but unused.

## Repo notes

- `Cargo.lock` is gitignored even though this is a binary crate.
- The README is marketing-heavy; trust the code over its claims (e.g. it knocks TranslucentTB for DLL injection into explorer, yet the modern backend injects `ExplorerTAP.dll`).
