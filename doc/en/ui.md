# Application Window Overview

**The application occupies all available space and has vertical division into elements:**
- Menu bar
- Panels area
- Status bar

The application also uses popup windows:
- Help
- Settings
- Application close confirmation

## Menu Bar

The menu bar is located at the top of the window and includes: menu items on the left, a menu activation hint, system resource indicators (CPU, RAM), and a clock in HH:MM format on the right.
Menu activation/deactivation and each item can be accessed by mouse click or [keyboard shortcuts](#Keyboard-Navigation-and-Panel-Management).

**Menu items:**
- `Files` opens a panel with file manager
- `Terminal` opens a panel with terminal
- `Editor` opens a panel with new file editor
- `Preferences` opens settings window
- `Help` opens help window
- `Quit` exits the application

**System Resource Indicators:**
- `CPU` - CPU usage percentage with color coding (green < 50%, yellow 50-75%, red > 75%)
- `RAM` - RAM usage in GB/MB format with color coding based on usage level

## Panels Area

The area fills the vertical space between the menu bar and status bar from left to right edge of the window.
The area always contains a non-closable file manager panel on the left, and other openable panels are placed on the remaining space on the right, or a help panel when no other panels are open.

**Possible openable panel types:**
- [file manager](file-manager.md)
- [terminal](terminal.md)
- [text editor](editor.md)

**Features of the non-closable file manager panel:**
- does not have a close "cross" button in the panel header
- cannot be closed
- is always the leftmost (first) panel
- has a default width of 30 characters

## Status Bar

The status bar is designed to display additional information about work in the active panel.
Depending on the type of active panel, corresponding data is displayed.

The status bar also shows disk space information on the right side in the format: `DEVICE used/total (usage%)` with color coding:
- Green when disk usage < 50%
- Yellow when disk usage 50-75%
- Red when disk usage > 75%

Example: `NVME0N1P2 386/467Gb (83%)`

## Keyboard Navigation and Panel Management

| Shortcut          | Action                                     |
|-------------------|--------------------------------------------|
| `Alt+M`           | Activate / deactivate menu                 |
| `Alt+F`           | Open file manager panel                    |
| `Alt+T`           | Open terminal panel                        |
| `Alt+E`           | Open new file editor panel                 |
| `Alt+D`           | Open debug panel                           |
| `Alt+P`           | Open settings window                       |
| `Alt+H`           | Open help window                           |
| `Alt+Q`           | Close application                          |
| `Alt+Delete`      | Close application                          |
| `Escape`          | Close panel                                |
| `Alt+X`           | Close panel                                |
| `Alt+Backspace`   | Close panel                                |
| `Alt+1` - `Alt+9` | Go to panel by number                      |
| `Alt+Left`        | Go to previous panel (left)                |
| `Alt+Right`       | Go to next panel (right)                   |
| `Alt+PgDn`        | Move panel left (swap positions)           |
| `Alt+PgUp`        | Move panel right (swap positions)          |
| `Alt+Minus`       | Decrease panel width by 1 character        |
| `Alt+Plus`        | Increase panel width by 1 character        |
