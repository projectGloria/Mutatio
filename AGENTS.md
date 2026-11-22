# AGENTS.md — Mutatio

Windows-only (x86_64, Win10/11) taskbar appearance daemon and CLI built in Rust (edition 2024, `windows` crate 0.62).

## Commands
- Run / Dev: `cargo run` (debug console subsystem with visible logs); CLI client: `cargo run -- --status`
- Test (all): `cargo test`
- Test (single): `cargo test hex_parsing`
- Lint / Typecheck: `cargo clippy --all-targets -- -D warnings`
- Build / Package: `cargo build --release` (GUI subsystem -> `target/release/mutatio.exe` + `mutatio-cli.exe`); installer: `powershell -ExecutionPolicy Bypass -File installer\build.ps1` (requires Windows SDK `rc.exe` and Inno Setup 6 -> `dist\`)

## Architecture
- `src/main.rs` — STA event-driven tray daemon (`mutatio.exe`): drains messages/IPC, coalesces WinEvent hooks (`DETECT_DIRTY`, ~60 Hz cap), steps 16 ms transition frames only while animating (1 s safety poll otherwise), and blocks in `MsgWaitForMultipleObjectsEx`.
- `src/bin/mutatio-cli.rs` — Console-subsystem CLI (`mutatio-cli`, installed as `mutatio.com` so PowerShell resolves `.com` before `.exe` for reliable stdout); pulls `cli.rs`/`ipc.rs`/`config.rs` via `#[path]`.
- `src/monitor.rs` — Per-taskbar `Slot` detection (`StateDetector::detect`): single `EnumWindows` pass per monitor; Start/Search identified by owning process (`StartMenuExperienceHost.exe`, `SearchHost.exe`), not localized titles.
- `src/taskbar.rs` — Dual backends: Win11 XAML injects embedded `assets/ExplorerTAP.dll` (extracted to `%LOCALAPPDATA%\Mutatio\`) and calls `ITaskbarAppearanceService` via raw vtable slots 3–7; Win10/ExplorerPatcher fallback uses `SetWindowCompositionAttribute` + DWM (re-pushed every detection pass).
- `src/ipc.rs` — Named pipe `\\.\pipe\mutatio` (message mode, rejects remote clients, serde-tagged JSON) forwarding `IpcRequest` to the main thread with a 3 s timeout.
- `src/flyout.rs` & `src/tray.rs` — Custom-painted GDI popup (`HIT_ZONES` must match `render_flyout`; `SWATCHES` shared by clicks and painting; thread-local state/font cache). "Restart Shell" kills and restarts `explorer.exe`.
- `src/config.rs` — Atomic JSON saves to `%APPDATA%\Mutatio\config.json` (`#[serde(default)]` everywhere; unparsable files backed up to `config.json.bak`); `battery_saver_fallback` restores stock taskbar on Battery Saver.
- **Version Manifests:** `Cargo.toml` (single source of truth; `build.rs` injects `CARGO_PKG_VERSION` into `mutatio.rc`, and `installer/mutatio.iss` reads `ProductVersion` from the built exe).

## Personal Rules (Brain)
- **Git Branching:** Never create new branches or git worktrees. Always work directly on `main` until explicitly told otherwise.
- **Versioning (Minimal Changes):** When runtime code or packaging changes, bump the alpha number by `+1` (e.g., `0.1.0-alpha.1` -> `0.1.0-alpha.2`) across all version manifests in the repo **at most once per uncommitted change set** (check `git diff HEAD` first—if the working tree version is already ahead of `HEAD`, do not bump again until committed). Skip version bumps on docs-only or read-only tasks.
- **Versioning (Major/Minor Changes):** Before changing minor/major versions or dropping `-alpha` on breaking or major feature work, stop and ask for the exact target version.
- **Commit Format:** Write one plain sentence stating what changed and why; do not use `feat:`/`fix:`/`chore:` conventional-commit prefixes.
- **Commit & Push Policy:** Never run `git commit`, `git tag`, or `git push` unless explicitly asked; never force-push to `main`, and push only after the build and tests pass.
- **Path Privacy:** Always use `C:\Users\you` in documentation, comments, and examples—never hardcode the local machine's real username.

## Playbooks (Read Before Task)
- **Packaging & Installers:** Read `.brain/building-the-project.md` before packaging, compiling release binaries, or building an installer.
- **GitHub Actions & Releases:** Read `.brain/github-actions.md` before creating or modifying `.github/workflows/*.yml` or release automation.

## Repo Guardrails & Gotchas
- **Event Loop:** Do not reintroduce unconditional sleeps or busy polling in `src/main.rs`.
- **State Precedence & Reuse:** `force_preset` overrides dynamic/fixed modes; use `App::toggle_dynamic`, `StateAppearance::preset`, and `TaskbarState::parse`/`name` instead of duplicating logic.
- **COM & Shell Restarts:** `TaskbarCreated` requires a real top-level window (not `HWND_MESSAGE`); drop the COM proxy before `CoUninitialize` (`TaskbarController::drop`); `restore_all` is explicit on exit while `Drop` only releases; raw vtable slots 3–7 in `src/taskbar.rs` must match `ExplorerTAP.dll` exactly.
- **Color Byte Order:** `Color` is RGBA in config/JSON, but backends and window-message payloads use ABGR (`to_abgr` / `from_abgr`).
- **Manual Shell Verification:** Only one daemon runs (`Local\Mutatio.Daemon` mutex; `--quit` blocks until exit). Quit running instances before testing, drive via CLI (`--status`, `--blur`, `--color`, `--toggle`, `--quit`), and restore `%APPDATA%\Mutatio\config.json` after toggle tests.
- **Code Over README:** Trust source code over `README.md` (modern backend injects prebuilt TranslucentTB `assets/ExplorerTAP.dll`, including `TTBTAP_Ready` internals).

## Done Criteria
Run `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo build --release` before reporting complete. For shell/taskbar changes, manually verify against a running daemon and restore `%APPDATA%\Mutatio\config.json`. Verify `Cargo.toml` version reflects any required bump.

<!-- brain-sync: 4401790b0df9 -->
