# Themes

TermIDE comes with 12 built-in themes and supports custom user themes. You can switch themes by editing the configuration file.

## Built-in Themes

### Dark Themes

| Theme Name | Description |
|-----------|-------------|
| `default` | Default dark theme with green accents |
| `midnight` | Midnight Commander inspired classic blue theme |
| `dracula` | Popular Dracula theme with purple accents |
| `onedark` | Atom One Dark theme |
| `monokai` | Classic Monokai theme with vibrant colors |
| `nord` | Nord theme with cool blue tones |
| `solarized-dark` | Dark variant of the Solarized color scheme |

### Light Themes

| Theme Name | Description |
|-----------|-------------|
| `atom-one-light` | Atom One Light theme |
| `ayu-light` | Ayu Light theme with warm tones |
| `github-light` | GitHub Light theme |
| `material-lighter` | Material Lighter theme |
| `solarized-light` | Light variant of the Solarized color scheme |

## Switching Themes

### Method 1: Using TermIDE (Recommended)

1. Press `Alt+P` while running TermIDE to open the configuration file in the built-in editor
2. Find the `theme` parameter
3. Change it to your desired theme name (e.g., `theme = "dracula"`)
4. Save the file with `Ctrl+S` - the new theme will be applied immediately

### Method 2: Manual Edit

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

Supported named colors: `Black`, `Red`, `Green`, `Yellow`, `Blue`, `Magenta`, `Cyan`, `White`, `Gray`, `DarkGray`

**2. RGB Values:**
```toml
bg = { rgb = [20, 20, 20] }
accented_bg = { rgb = [40, 40, 40] }
selected_bg = { rgb = [0, 120, 215] }
```

RGB values range from 0 to 255 for each channel (red, green, blue).

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
