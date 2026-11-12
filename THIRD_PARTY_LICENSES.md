# Third-Party Licenses & Attributions

Mutatio incorporates and interfaces with third-party open-source components. This document details their licenses and attributions.

---

## ExplorerTAP (TranslucentTB Shell Bridge)

- **Component**: `assets/ExplorerTAP.dll`
- **Origin**: [TranslucentTB](https://github.com/TranslucentTB/TranslucentTB)
- **Copyright**: Copyright (c) 2016–2024 TranslucentTB Contributors
- **License**: [GNU General Public License v3.0 (GPL-3.0)](https://www.gnu.org/licenses/gpl-3.0.en.html)

### Description
`ExplorerTAP.dll` is an in-process shell bridge utilized on Windows 11 to interface with modern Windows 11 XAML taskbars (`ITaskbarAppearanceService`). In accordance with the GNU General Public License v3.0:

1. The TAP component is licensed under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
2. Upstream source code for TranslucentTB and its TAP component is freely available at: https://github.com/TranslucentTB/TranslucentTB
3. Any modifications or rebuilds of the ExplorerTAP component distributed with Mutatio remain subject to the terms of the GPL-3.0 license.

---

## GNU GENERAL PUBLIC LICENSE (Version 3)

```text
                    GNU GENERAL PUBLIC LICENSE
                       Version 3, 29 June 2007

 Copyright (C) 2007 Free Software Foundation, Inc. <https://fsf.org/>
 Everyone is permitted to copy and distribute verbatim copies
 of this license document, but changing it is not allowed.

                            Preamble

  The GNU General Public License is a free, copyleft license for
software and other kinds of works.
[Full GPL-3.0 text available at https://www.gnu.org/licenses/gpl-3.0.txt]
```

---

## Mutatio Core

The Mutatio core engine, tray daemon, CLI wrapper, and UI code are independently authored in Rust and licensed under the MIT License (see [LICENSE](LICENSE)).
