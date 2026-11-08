# Mutatio ⚡

<p align="center">
  <strong>Next-Generation, Ultra-Lightweight Taskbar Appearance Engine for Windows 11 & 10</strong>
  <br />
  <em>Engineered from the ground up in 100% modern Rust. Sub-15 MB RAM • 0% Idle CPU • Windows 11 24H2 XAML Support</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Language-Rust_2024-orange.svg" alt="Language: Rust" />
  <img src="https://img.shields.io/badge/OS-Windows_11_%2F_10-0078D4.svg" alt="Windows 11 / 10" />
  <img src="https://img.shields.io/badge/Win11_Build-24H2_26100+-success.svg" alt="Windows 11 24H2 Ready" />
  <img src="https://img.shields.io/badge/Memory-<15_MB_RAM-brightgreen.svg" alt="Memory: <15 MB" />
  <img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT" />
</p>

---

## 📖 About Mutatio

**Mutatio** (Latin: *change, transformation*) is a blazing-fast, crash-resistant, modern taskbar appearance engine for Windows 11 (including Build 26100 / 24H2) and Windows 10. 

While legacy C++ utilities like TranslucentTB consume ~95 MB of RAM, use invasive DLL injection into `explorer.exe`, and struggle on modern Windows 11 XAML visual trees, **Mutatio** provides a lightweight (<15 MB RAM), completely self-contained solution with smooth 60fps fade animations, zero idle CPU wakeups, an integrated Fluent Dark Flyout UI, and full CLI/IPC automation.

---

## ⚡ Highlights & Comparison

| Feature | TranslucentTB (Upstream) | **Mutatio (This Project)** |
| :--- | :--- | :--- |
| **Language & Safety** | C++ / C++/WinRT / vcpkg | **100% Rust** (`windows-rs` 0.62) |
| **Memory Footprint** | ~35 MB – 95 MB RAM | **~13 MB RAM** (sub-15 MB) |
| **Idle CPU Usage** | Periodic polling wakeups | **0.0% CPU** (Power-efficient wait) |
| **Win11 24H2 Support** | Often crashes or fails on XAML `BackgroundFill` | **Native COM `ITaskbarAppearanceService`** |
| **Smooth Transitions** | ❌ Snappy/abrupt state switching | ✅ **60fps Ease-Out Cubic Animations** |
| **Fullscreen Game Mode** | ❌ Keeps running & polling | ✅ **Auto-Sleep in Fullscreen 3D Games** (Anti-cheat safe) |
| **User Interface** | Heavy multi-megabyte XAML Island | ✅ **Native Fluent Dark Flyout UI** (Auto-dismiss, zero latency) |
| **CLI & IPC Automation** | ❌ None (GUI only) | ✅ **Built-in CLI & Named Pipe IPC Server** |
| **Packaging** | Installer + multiple loose DLLs | ✅ **Single standalone binary** |

---

## 🛠️ Architecture

```mermaid
flowchart TD
    subgraph UI ["User Interaction Layer"]
        Tray["System Tray Icon"] -->|Left Click| Flyout["Fluent Flyout UI (Auto-Dismiss)"]
        CLI["CLI Commands (e.g. --clear)"] -->|Named Pipe| IPC["IPC Server (\\\\.\\pipe\\mutatio)"]
    end

    subgraph Core ["Mutatio Engine (Rust)"]
        IPC --> Loop["State Coordinator"]
        Flyout --> Loop
        Detector["State Detector (Maximized / Start / Desktop / Game)"] --> Loop
        Loop --> Trans["60fps Ease-Out Transition Engine"]
    end

    subgraph Shell ["Windows 11 / 10 Shell"]
        Trans --> ModernEngine["Modern XAML Engine (ITaskbarAppearanceService)"]
        Trans --> LegacyEngine["Win32 Composition Fallback (SWCA / DWM)"]
        ModernEngine --> Explorer["Windows 11 Taskbar.TaskbarFrame"]
        LegacyEngine --> Explorer10["Windows 10 Shell_TrayWnd"]
    end
```

---

## ✨ Features

- **Integrated Fluent Dark Flyout UI**: Left-click the tray icon to open an ultra-responsive, beautiful settings panel anchored right above your taskbar. Auto-dismisses when you click away. Right-clicking does nothing.
- **5 Appearance Presets**:
  - **Clear**: 100% crystal-clear taskbar transparency.
  - **Blur**: Aero glass frosted effect.
  - **Acrylic**: Windows 11 Fluent Acrylic material with depth.
  - **Opaque Black**: Pure solid pitch-black (`#000000`, 100% alpha) taskbar.
  - **Windows Default**: Cleanly restores stock Windows taskbar styling.
- **Smart Dynamic Automation**:
  - Automatically adapts appearance when windows maximize, minimize, or when the Start Menu or Search flyout opens.
- **Smooth Animated Transitions**:
  - Linear color interpolation with ease-out cubic curve (60fps ticks during transitions, zero CPU when idle).
- **Fullscreen Game Auto-Sleep**:
  - Detects exclusive fullscreen DirectX/Vulkan games and puts the engine to sleep to eliminate frame drops and avoid anti-cheat flags.
- **Named Pipe IPC & CLI**:
  - Script or bind any state to shortcuts, AutoHotkey, or Stream Deck.

---

## 🚀 Quick Start

### 1. Run the Background Daemon
Double-click `mutatio.exe` or run from terminal:
```powershell
.\mutatio.exe
```
Mutatio will start in your system tray:
- **Left-click** the tray icon: Opens the Flyout UI.
- **Click outside**: Closes the UI.

### 2. Command Line Interface (CLI)
You can control a running Mutatio instance or launch it with a preset:

```powershell
# Set taskbar to 100% transparent
mutatio.exe --clear

# Set taskbar to solid pitch black
mutatio.exe --opaque

# Set taskbar to Fluent Acrylic
mutatio.exe --acrylic

# Set taskbar to Aero Blur
mutatio.exe --blur

# Set taskbar to a custom HEX color
mutatio.exe --color "#FF000000"

# Restore default Windows appearance
mutatio.exe --normal

# Toggle between Dynamic and Forced modes
mutatio.exe --toggle

# Reload configuration from disk
mutatio.exe --reload

# Show daemon status
mutatio.exe --status

# Exit Mutatio cleanly and restore taskbars
mutatio.exe --quit
```

---

## ⚙️ Configuration (`config.json`)

Configuration is stored at `%APPDATA%\Mutatio\config.json`:

```json
{
  "desktop": {
    "state": "clear",
    "color": { "r": 0, "g": 0, "b": 0, "a": 0 },
    "show_line": false
  },
  "visible_window": {
    "state": "blur",
    "color": { "r": 20, "g": 20, "b": 20, "a": 120 },
    "show_line": false
  },
  "maximized_window": {
    "state": "opaque",
    "color": { "r": 0, "g": 0, "b": 0, "a": 255 },
    "show_line": false
  },
  "start_opened": {
    "state": "acrylic",
    "color": { "r": 30, "g": 30, "b": 30, "a": 180 },
    "show_line": false
  },
  "search_opened": {
    "state": "acrylic",
    "color": { "r": 30, "g": 30, "b": 30, "a": 180 },
    "show_line": false
  },
  "dynamic_modes_enabled": true,
  "smooth_transitions": true,
  "transition_duration_ms": 180,
  "game_mode_pause": true,
  "battery_saver_fallback": true
}
```

---

## 🔨 Building from Source

### Prerequisites
- [Rust](https://rustup.rs/) (1.80+ or latest stable)
- Windows 10 or 11 (x86_64)

### Build Steps
```powershell
# Clone your repository
git clone https://github.com/yourusername/mutatio.git
cd mutatio

# Build optimized release binary
cargo build --release
```
The compiled, self-contained binary will be located at:
`target/release/mutatio.exe`

---

## 📄 License & Credits

- Licensed under the [MIT License](LICENSE).
- Powered by the [`windows-rs`](https://github.com/microsoft/windows-rs) crate by Microsoft.
- Explorer TAP bridge derived from reverse-engineered COM interfaces of the Windows Modern Shell.
