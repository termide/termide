# Themes

TermIDE comes with 38 built-in themes and supports custom user themes. You can switch themes by editing the configuration file.

## Built-in Themes

### Dark Themes

| Theme Name | Description |
|-----------|-------------|
| `windows-xp` | Default theme (Windows XP style) |
| `dracula` | Popular Dracula theme with purple accents |
| `monokai` | Classic Monokai theme with vibrant colors |
| `nord` | Nord theme with cool blue tones |
| `onedark` | Atom One Dark theme |
| `solarized-dark` | Dark variant of the Solarized color scheme |
| `midnight` | Midnight Commander inspired classic blue theme |
| `macos-dark` | macOS dark style theme |
| `ayu-dark` | Ayu Dark theme with warm amber accents |
| `billiard` | Green billiard table, deep green tones |
| `catppuccin-macchiato` | Catppuccin Macchiato — pastel dark with mauve accents |
| `everforest` | Everforest — muted green woodland palette |
| `github-dark` | GitHub Dark theme |
| `gruvbox` | Gruvbox — retro warm browns and muted greens |
| `kanagawa` | Kanagawa — Japanese woodblock inspired dark theme |
| `material-ocean` | Material Ocean — deep blue Material Design variant |
| `rosepine` | Rosé Pine — soho vibes with dusty rose accents |
| `tokyonight` | Tokyo Night — neon-lit dark city palette |

### Light Themes

| Theme Name | Description |
|-----------|-------------|
| `atom-one-light` | Atom One Light theme |
| `blue-sky` | Gentle blue sky and cloud castle tones |
| `ayu-light` | Ayu Light theme with warm tones |
| `github-light` | GitHub Light theme |
| `green-backs` | Green dollar bill aesthetic |
| `manuscript` | Medieval manuscript with aged parchment, iron gall ink, vermillion accents |
| `material-lighter` | Material Lighter theme |
| `pinky-pie` | Pink unicorn and rainbow tones |
| `solarized-light` | Light variant of the Solarized color scheme |
| `macos-light` | macOS light style theme |

### Retro Themes

| Theme Name | Description |
|-----------|-------------|
| `far-manager` | FAR Manager style theme |
| `norton-commander` | Norton Commander style theme |
| `dos-navigator` | DOS Navigator style theme |
| `volkov-commander` | Volkov Commander style theme |
| `windows-95` | Windows 95 style theme |
| `windows-98` | Windows 98 style theme |

### Cinematic Themes

| Theme Name | Description |
|-----------|-------------|
| `matrix` | The Matrix digital rain — green phosphor on black |
| `pip-boy` | Fallout Pip-Boy 3000 — warm green phosphor CRT display |
| `terminator` | Skynet HUD / Mars — deep red and crimson tones |

### Other Themes

| Theme Name | Description |
|-----------|-------------|
| `terminal` | Classic terminal style (inherits terminal colors) |

## Switching Themes

### Method 1: Using the Menu (Recommended)

1. Click on **Options** in the menu bar
2. Select **Themes** from the dropdown
3. Choose your desired theme from the list - each item shows a color preview of the theme
4. The theme will be applied immediately and saved to config

### Method 2: Using Keyboard Shortcut

1. Press `Alt+P` while running TermIDE to open the configuration file in the built-in editor
2. Find the `theme` parameter
3. Change it to your desired theme name (e.g., `theme = "dracula"`)
4. Save the file with `Ctrl+S` - the new theme will be applied immediately

### Method 3: Manual Edit

You can also edit the configuration file directly with any text editor:

**Linux:**
```bash
~/.config/termide/config.toml
```

**macOS:**
```bash
~/Library/Application Support/termide/config.toml
```

**Windows (WSL):**
```bash
~/.config/termide/config.toml
```

Change the `theme` parameter:
```toml
theme = "dracula"
language = "auto"
```

If you edit the file while TermIDE is running and it's opened via `Alt+P`, the theme will be applied immediately when you save. Otherwise, the new theme will be applied when you start TermIDE.

## Custom Themes

You can create your own themes by placing TOML files in the themes directory.

### Theme Directory Locations

**Linux:**
```bash
~/.config/termide/themes/
```

**macOS:**
```bash
~/Library/Application Support/termide/themes/
```

**Windows (WSL):**
```bash
~/.config/termide/themes/
```

### Creating a Custom Theme

1. Create a new `.toml` file in the themes directory:
   ```bash
   mkdir -p ~/.config/termide/themes
   nano ~/.config/termide/themes/my-theme.toml
   ```

2. Define your theme colors using the structure below.

3. Set your theme in the configuration:
   ```toml
   theme = "my-theme"
   ```

User themes take priority over built-in themes with the same name.

## Theme File Structure

A theme file is a TOML file with the following structure:

```toml
# Theme metadata
name = "my-theme"

[colors]
# Base colors
bg = { rgb = [20, 20, 20] }          # Background color (RGB)
fg = "White"                          # Foreground/text color

# Accented elements (active panel, focused items)
accented_bg = { rgb = [40, 40, 40] }  # Accented background
accented_fg = "Green"                 # Accented foreground

# Selection (selected files, text selection)
selected_bg = "Blue"                  # Selection background
selected_fg = "White"                 # Selection foreground

# Disabled UI elements
disabled = "Gray"                     # Disabled items color

# Semantic colors (status indicators)
success = "Green"                     # Success messages, high resources
warning = "Yellow"                    # Warning messages, medium resources
error = "Red"                         # Error messages, low resources
```

### Color Formats

Colors can be specified in two ways:

**1. Named Colors:**
```toml
fg = "White"
bg = "Black"
error = "Red"
```

Supported named colors: `Black`, `Red`, `Green`, `Yellow`, `Blue`, `Magenta`, `Cyan`, `White`, `Gray`, `DarkGray`, `Reset`

**2. RGB Values:**
```toml
bg = { rgb = [20, 20, 20] }
accented_bg = { rgb = [40, 40, 40] }
selected_bg = { rgb = [0, 120, 215] }
```

RGB values range from 0 to 255 for each channel (red, green, blue).

**3. `"Reset"` — Transparent Background:**
```toml
bg = "Reset"
```

Setting a color to `"Reset"` makes it transparent — the terminal's background shows through. This is useful when you want termide to blend into your terminal's native color scheme. All theme colors support `"Reset"`, but it's most commonly used for `bg` (panel backgrounds) and `accented_bg` (status bar / menu background).

## Theme Color Usage

Different colors are used for different UI elements:

| Color | Used For |
|-------|----------|
| `bg` / `fg` | Default background and text |
| `accented_bg` / `accented_fg` | Active panel borders, focused items |
| `selected_bg` / `selected_fg` | Selected files, text selection in editor |
| `disabled` | Inactive UI elements, grayed-out text |
| `success` | CPU/RAM/Disk under 50%, success messages |
| `warning` | CPU/RAM/Disk 50-75%, warning messages |
| `error` | CPU/RAM/Disk over 75%, error messages |

## Example: Creating a Custom Dark Theme

```toml
name = "my-dark-theme"

[colors]
# Dark background with light text
bg = { rgb = [30, 30, 30] }
fg = { rgb = [220, 220, 220] }

# Purple accents
accented_bg = { rgb = [60, 40, 80] }
accented_fg = { rgb = [200, 150, 255] }

# Cyan selection
selected_bg = { rgb = [0, 150, 200] }
selected_fg = "White"

# Gray for disabled
disabled = { rgb = [100, 100, 100] }

# Standard semantic colors
success = "Green"
warning = "Yellow"
error = "Red"
```

Save this as `~/.config/termide/themes/my-dark-theme.toml` and set `theme = "my-dark-theme"` in your config.

## Theme Screenshots

See the [README](../../README.md#theme-examples) for theme screenshots and visual examples.
