# 主题

TermIDE 自带 38 款内置主题，并支持自定义用户主题。您可以通过编辑配置文件来切换主题。

## 内置主题

### 暗色主题

| 主题名称 | 描述 |
|-----------|-------------|
| `windows-xp` | 默认主题（Windows XP 风格） |
| `dracula` | 流行的 Dracula 主题，带紫色色调 |
| `monokai` | 经典 Monokai 主题，色彩鲜艳 |
| `nord` | Nord 主题，冷蓝色调 |
| `onedark` | Atom One Dark 主题 |
| `solarized-dark` | Solarized 配色方案的暗色变体 |
| `midnight` | Midnight Commander 风格经典蓝色主题 |
| `macos-dark` | macOS 暗色风格主题 |
| `ayu-dark` | Ayu Dark 主题，暖琥珀色调 |
| `billiard` | 台球桌深绿色主题 |
| `catppuccin-macchiato` | Catppuccin Macchiato — 柔和暗色，紫丁香色调 |
| `everforest` | Everforest — 静谧森林系低饱和绿色调 |
| `github-dark` | GitHub Dark 主题 |
| `gruvbox` | Gruvbox — 复古暖棕与哑光绿调色盘 |
| `kanagawa` | Kanagawa — 日式浮世绘风格暗色主题 |
| `material-ocean` | Material Ocean — 深蓝色 Material Design 变体 |
| `rosepine` | Rosé Pine — 尘玫瑰色调的 soho 风格 |
| `tokyonight` | Tokyo Night — 霓虹夜都市配色 |

### 亮色主题

| 主题名称 | 描述 |
|-----------|-------------|
| `atom-one-light` | Atom One Light 主题 |
| `ayu-light` | Ayu Light 主题，暖色调 |
| `blue-sky` | 蓝天白云清爽亮色主题 |
| `github-light` | GitHub Light 主题 |
| `green-backs` | 美元钞票绿色主题 |
| `manuscript` | 中世纪手稿风格，陈旧羊皮纸、铁胆墨水、朱砂色调 |
| `material-lighter` | Material Lighter 主题 |
| `pinky-pie` | 粉色少女系亮色主题 |
| `solarized-light` | Solarized 配色方案的亮色变体 |
| `macos-light` | macOS 亮色风格主题 |

### 复古主题

| 主题名称 | 描述 |
|-----------|-------------|
| `far-manager` | FAR Manager 风格主题 |
| `norton-commander` | Norton Commander 风格主题 |
| `dos-navigator` | DOS Navigator 风格主题 |
| `volkov-commander` | Volkov Commander 风格主题 |
| `windows-95` | Windows 95 风格主题 |
| `windows-98` | Windows 98 风格主题 |

### 电影主题

| 主题名称 | 描述 |
|-----------|-------------|
| `matrix` | 黑客帝国数字雨 — 黑底绿色磷光 |
| `pip-boy` | 辐射 Pip-Boy 3000 — 暖绿色磷光 CRT 显示器 |
| `terminator` | 天网 HUD / 火星 — 深红色和绯红色调 |

### 其他主题

| 主题名称 | 描述 |
|-----------|-------------|
| `terminal` | 经典终端风格（继承终端颜色） |

## 切换主题

### 方法一：使用菜单（推荐）

1. 点击菜单栏中的 **Options**
2. 从下拉菜单中选择**主题**
3. 从列表中选择您想要的主题 - 每个选项显示主题的颜色预览
4. 主题将立即应用并保存到配置中

### 方法二：使用键盘快捷键

1. 运行 TermIDE 时按 `Alt+P` 在内置编辑器中打开配置文件
2. 找到 `theme` 参数
3. 将其更改为您想要的主题名称（例如 `theme = "dracula"`）
4. 使用 `Ctrl+S` 保存文件 - 新主题将立即应用

### 方法三：手动编辑

您也可以使用任何文本编辑器直接编辑配置文件：

**Linux：**
```bash
~/.config/termide/config.toml
```

**macOS：**
```bash
~/Library/Application Support/termide/config.toml
```

**Windows (WSL)：**
```bash
~/.config/termide/config.toml
```

更改 `theme` 参数：
```toml
theme = "dracula"
language = "auto"
```

如果在 TermIDE 运行时通过 `Alt+P` 打开并编辑文件，保存时主题会立即应用。否则，新主题将在下次启动 TermIDE 时应用。

## 自定义主题

您可以将 TOML 文件放置在主题目录中来创建自己的主题。

### 主题目录位置

**Linux：**
```bash
~/.config/termide/themes/
```

**macOS：**
```bash
~/Library/Application Support/termide/themes/
```

**Windows (WSL)：**
```bash
~/.config/termide/themes/
```

### 创建自定义主题

1. 在主题目录中创建一个新的 `.toml` 文件：
   ```bash
   mkdir -p ~/.config/termide/themes
   nano ~/.config/termide/themes/my-theme.toml
   ```

2. 使用以下结构定义您的主题颜色。

3. 在配置中设置您的主题：
   ```toml
   theme = "my-theme"
   ```

用户主题优先于同名的内置主题。

## 主题文件结构

主题文件是一个 TOML 文件，结构如下：

```toml
# 主题元数据
name = "my-theme"

[colors]
# 基础颜色
bg = { rgb = [20, 20, 20] }          # 背景颜色 (RGB)
fg = "White"                          # 前景/文字颜色

# 强调元素（活动面板、聚焦项目）
accented_bg = { rgb = [40, 40, 40] }  # 强调背景
accented_fg = "Green"                 # 强调前景

# 选择（选中的文件、文本选择）
selected_bg = "Blue"                  # 选择背景
selected_fg = "White"                 # 选择前景

# 禁用的 UI 元素
disabled = "Gray"                     # 禁用项目颜色

# 语义颜色（状态指示器）
success = "Green"                     # 成功消息、资源充足
warning = "Yellow"                    # 警告消息、资源中等
error = "Red"                         # 错误消息、资源不足
```

### 颜色格式

颜色可以通过两种方式指定：

**1. 颜色名称：**
```toml
fg = "White"
bg = "Black"
error = "Red"
```

支持的颜色名称：`Black`、`Red`、`Green`、`Yellow`、`Blue`、`Magenta`、`Cyan`、`White`、`Gray`、`DarkGray`、`LightRed`、`LightGreen`、`LightYellow`、`LightBlue`、`LightMagenta`、`LightCyan`、`Reset`。名称不区分大小写（`Reset` 与 `reset` 等价）；无法识别的名称回退为 `White`。

**2. RGB 值：**
```toml
bg = { rgb = [20, 20, 20] }
accented_bg = { rgb = [40, 40, 40] }
selected_bg = { rgb = [0, 120, 215] }
```

RGB 值范围为每个通道（红、绿、蓝）0 到 255。

**3. `"Reset"` — 透明背景：**
```toml
bg = "Reset"
```

将某个颜色设为 `"Reset"` 会使其透明——终端的背景会透出来。当你希望 termide 融入终端原生配色时很有用。所有主题颜色都支持 `"Reset"`，但最常用于 `bg`（面板背景）和 `accented_bg`（状态栏/菜单背景）。

> **提示：** 浅色/深色自动检测读取背景颜色，而 `"Reset"` 被视为深色。如果你的终端使用浅色背景，请显式设置 `is_light = true`（主题顶层的 `is_light` 字段），以便语法高亮选用浅色调色板。

## 主题颜色用途

不同颜色用于不同的 UI 元素：

| 颜色 | 用途 |
|-------|----------|
| `bg` / `fg` | 默认背景和文字 |
| `accented_bg` / `accented_fg` | 活动面板边框、聚焦项目 |
| `selected_bg` / `selected_fg` | 选中的文件、编辑器中的文本选择 |
| `disabled` | 非活动 UI 元素、灰色文字 |
| `success` | CPU/内存/磁盘低于 50%、成功消息 |
| `warning` | CPU/内存/磁盘 50-75%、警告消息 |
| `error` | CPU/内存/磁盘超过 75%、错误消息 |

## 示例：创建自定义暗色主题

```toml
name = "my-dark-theme"

[colors]
# 深色背景配亮色文字
bg = { rgb = [30, 30, 30] }
fg = { rgb = [220, 220, 220] }

# 紫色强调
accented_bg = { rgb = [60, 40, 80] }
accented_fg = { rgb = [200, 150, 255] }

# 青色选择
selected_bg = { rgb = [0, 150, 200] }
selected_fg = "White"

# 灰色禁用
disabled = { rgb = [100, 100, 100] }

# 标准语义颜色
success = "Green"
warning = "Yellow"
error = "Red"
```

将此文件保存为 `~/.config/termide/themes/my-dark-theme.toml`，并在配置中设置 `theme = "my-dark-theme"`。

## 主题截图

请参阅 [README](../../README.md#theme-examples) 查看主题截图和视觉示例。
