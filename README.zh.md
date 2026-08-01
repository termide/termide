# TermIDE

[![GitHub Release](https://img.shields.io/github/v/release/termide/termide)](https://github.com/termide/termide/releases)
[![CI](https://github.com/termide/termide/actions/workflows/release.yml/badge.svg)](https://github.com/termide/termide/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | **中文** | [Русский](README.ru.md)

一款零配置的终端 IDE，将编辑器、文件管理器和终端合为一体 —— 内置 git、数据库、十六进制、Markdown、图片和 Mermaid 查看器 —— 全部在一个跨平台 TUI 中，使用 Rust 编写。

**[网站](https://termide.github.io)** | **[文档](doc/zh/README.md)** | **[版本发布](https://github.com/termide/termide/releases)** | **[截图](https://ibb.co/album/nPX6p6)**

<p align="center"><img src="assets/screenshots/termide.png" alt="TermIDE — 编辑器、文件管理器、终端和查看器合为一个 TUI" width="900"></p>

## 为什么选择 TermIDE？

与需要大量插件配置的传统终端编辑器不同，TermIDE 开箱即用：

| 功能 | TermIDE | Vim/Neovim | Helix | Micro |
|---------|:-------:|:----------:|:-----:|:-----:|
| LSP 支持 | ✓ | ✓ | ✓ | 插件 |
| 零配置 | ✓ | ✗ | ✓ | ✓ |
| 脚本自动化 | ✓ | ✓ | ✗ | 插件 |
| 远程文件系统（SFTP/FTP） | ✓ | ✓ | ✗ | ✗ |
| 十六进制 / 二进制查看器 | ✓ | 插件 | ✗ | 插件 |
| 数据库查看器 | ✓ | 插件 | ✗ | ✗ |
| Markdown 预览 | ✓ | 插件 | ✗ | ✗ |
| 图表查看器（Mermaid） | ✓ | 插件 | ✗ | ✗ |
| 图片查看器 | ✓ | 插件 | ✗ | ✗ |
| 内置终端 | ✓ | 插件 | ✗ | ✗ |
| 文件管理器 | ✓ | 插件 | ✗ | ✗ |
| 后台文件操作 | ✓ | 插件 | ✗ | ✗ |
| Git 集成 | ✓ | 插件 | ✗ | ✗ |
| 会话管理 | ✓ | 插件 | ✗ | ✗ |
| 多面板布局 | ✓ | 插件 | ✗ | ✗ |
| 书签 | ✓ | 插件 | ✗ | ✗ |
| 资源监控 | ✓ | ✗ | ✗ | ✗ |

**TermIDE = 编辑器 + 文件管理器 + 终端，集成于一个 TUI 应用程序中。**

## 功能特性

- **基于终端的 IDE** - 支持 22 种语言的语法高亮、单词导航（Ctrl+Left/Right）、段落/符号导航（Ctrl+Up/Down）、自动缩进、自动关闭括号
- **LSP 支持** - 代码补全、查找引用、重命名符号、跳转到定义，通过 rust-analyzer、pylsp、typescript-language-server 及其他 LSP 服务器实现
- **智能文件管理器** - 可展开目录的树形视图、嵌套 Git 状态、批量操作、文件/内容搜索（glob/正则表达式）、树内增量搜索
- **远程文件系统** - 在文件管理器中通过 SFTP / FTP / FTPS 浏览和编辑远程服务器上的文件，在本地与远程面板之间复制 —— 纯 Rust（russh + rustls），无需原生库，可在静态 musl 上运行（`smb://` / `nfs://` 走系统挂载）
- **后台文件操作** - 复制、移动、上传、下载、删除及批量传输在后台运行，每个操作带进度条、字节/耗时读数，支持暂停 / 恢复 / 取消（操作面板）
- **集成终端** - 完整的 PTY 支持、VT100 转义序列、鼠标跟踪
- **Git 集成** - 状态面板、带彩色 Unicode 提交图（ASCII 回退）的提交日志、暂存/取消暂存、分支切换、暂存管理（stash）、内联 blame 注解
- **数据库查看器** - 通过书签 URL 打开的 SQLite / PostgreSQL / MySQL 只读浏览器：带二维单元格光标的表格、服务端单列排序与按列类型感知过滤、滑动窗口分页，以及可复制为 TSV / JSON / INSERT 的整行详情对话框
- **多面板布局** - 垂直拆分的面板组，每个面板高度可调，一键全屏切换（`Alt+F11`）；终端变窄时智能自动堆叠
- **图片查看器** - 在 Kitty、WezTerm、iTerm2、Ghostty、foot 终端中原生渲染图形
- **十六进制 / 二进制查看与编辑器** - 二进制文件的 hex/ASCII 视图（按 16 字节自适应分段），字节光标在两个区域同时显示，支持拖动/Shift 选择与剪贴板复制、ASCII 与十六进制字节搜索，以及 hex↔文本切换（`Ctrl+L`）；`F4` 以覆盖方式编辑，保存时生成 `.bak` 备份
- **Markdown 预览** - `.md` / `.markdown` 的只读渲染视图（标题、列表、表格、语法高亮代码块、可点击链接与图片图标），支持光标导航、选择与剪贴板复制；`Ctrl+E` 切换到可编辑源码；内嵌的 ```mermaid``` 代码块渲染为图表
- **Mermaid 图表查看器** - 将 `.mmd` / `.mermaid` 文件渲染为文本伪图形 —— flowchart、sequence、state、class、ER、gantt、pie、journey、mindmap、timeline、gitGraph、quadrant；二维滚动、复制到剪贴板，`Ctrl+E` 编辑源码
- **外部应用** - 使用系统默认应用程序打开文件（Shift+Enter）
- **39 款内置主题** - 暗色、亮色、复古和电影主题（Dracula、Nord、Monokai、Solarized、Matrix、Pip-Boy、Norton Commander、Windows 95 等）
- **自定义主题** - 使用 TOML 格式创建自己的主题
- **15 种界面语言** - 孟加拉语、中文、英语、法语、德语、印地语、印尼语、日语、韩语、葡萄牙语、俄语、西班牙语、泰语、土耳其语、越南语
- **会话管理** - 自动保存和恢复面板布局
- **系统监控** - 菜单栏实时显示 CPU、RAM、网络 I/O；状态栏显示磁盘使用情况；点击指标可打开详细模态窗口
- **搜索和替换** - 实时预览、匹配计数、正则表达式支持
- **自定义脚本** - 从脚本菜单运行用户定义的脚本（支持 `.bg.` 后台执行、`.report.` 模态输出）
- **跨平台** - Linux（x86_64、ARM64）、macOS（Intel、Apple Silicon）、Windows（原生 ConPTY、WSL）
- **完整鼠标支持** - 点击导航、滚动、双击操作
- **键盘布局** - 西里尔文支持，自动快捷键翻译
- **Vim 模式** - 可选的 Vim 风格编辑，支持西里尔文键盘
- **命令面板** - 使用 Ctrl+P 快速打开命令
- **目录切换器** - 使用 Ctrl+/ 快速切换目录
- **书签** - 保存和管理常用位置

## 安装

**快速开始：** 从 [GitHub Releases](https://github.com/termide/termide/releases) 下载预编译的二进制文件，或通过包管理器安装。

**支持的平台：** Linux（x86_64、ARM64）、macOS（Intel、Apple Silicon）、Windows（x86_64）

### 选择安装方式

<details open>
<summary><b>📦 预编译二进制文件（推荐）</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载适合您平台的最新版本：

```bash
# Linux x86_64（也适用于 WSL）
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.6-x86_64-unknown-linux-gnu.tar.gz
tar xzf termide-0.29.6-x86_64-unknown-linux-gnu.tar.gz
./termide

# Linux x86_64（静态 musl — Alpine、distroless 容器、任何无 glibc 的系统）
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.6-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.29.6-x86_64-unknown-linux-musl.tar.gz
./termide

# macOS Intel (x86_64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.29.6-x86_64-apple-darwin.tar.gz
tar xzf termide-0.29.6-x86_64-apple-darwin.tar.gz
./termide

# macOS Apple Silicon (ARM64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.29.6-aarch64-apple-darwin.tar.gz
tar xzf termide-0.29.6-aarch64-apple-darwin.tar.gz
./termide

# Linux ARM64（树莓派、ARM 服务器）
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.6-aarch64-unknown-linux-gnu.tar.gz
tar xzf termide-0.29.6-aarch64-unknown-linux-gnu.tar.gz
./termide

# Linux ARM64（静态 musl —— Android/Termux、Alpine ARM、任何无 glibc 的 ARM64）
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.6-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.29.6-aarch64-unknown-linux-musl.tar.gz
./termide

# Windows x86_64（从 Releases 下载 .zip，解压后在 Windows Terminal 中运行）
# https://github.com/termide/termide/releases/latest/download/termide-0.29.6-x86_64-pc-windows-msvc.zip
```

</details>

<details>
<summary><b>🪟 Windows (.zip)</b></summary>

TermIDE 通过 ConPTY 在 Windows 10+ 上原生运行。建议使用 **Windows Terminal**
以获得最佳体验。

1. 从 [GitHub Releases](https://github.com/termide/termide/releases) 下载 `termide-0.29.6-x86_64-pc-windows-msvc.zip`。
2. 解压压缩包。
3. 在 Windows Terminal 中运行 `termide.exe`。

配置位于 `%APPDATA%\termide\`（配置、会话），日志位于
`%LOCALAPPDATA%\termide\cache\`。

或者在 **WSL/WSL2** 中，像在任意 Linux 上一样使用 Linux x86_64 构建
（`termide-0.29.6-x86_64-unknown-linux-gnu.tar.gz`）。

</details>

<details>
<summary><b>🐧 Debian/Ubuntu (.deb)</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载并安装 `.deb` 包：

```bash
# 仅限 x86_64（ARM64 请使用上面的 tar.gz）
wget https://github.com/termide/termide/releases/latest/download/termide_0.29.6-1_amd64.deb
sudo dpkg -i termide_0.29.6-1_amd64.deb
```

</details>

<details>
<summary><b>🎩 Fedora/RHEL/CentOS (.rpm)</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载并安装 `.rpm` 包：

```bash
# 仅限 x86_64（ARM64 请使用上面的 tar.gz）
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.6-1.x86_64.rpm
sudo rpm -i termide-0.29.6-1.x86_64.rpm
```

</details>

<details>
<summary><b>🐧 Arch Linux (AUR)</b></summary>

使用您喜欢的 AUR 助手从 AUR 安装：

```bash
# 从源码构建
yay -S termide

# 或安装预编译二进制文件
yay -S termide-bin
```

或手动安装：

```bash
git clone https://aur.archlinux.org/termide.git
cd termide
makepkg -si
```

</details>

<details>
<summary><b>🍺 Homebrew (macOS/Linux)</b></summary>

通过 Homebrew tap 安装：

```bash
brew tap termide/termide
brew install termide
```

</details>

<details>
<summary><b>❄️ NixOS/Nix (Flakes)</b></summary>

使用 Nix flakes 安装：

```bash
# 无需安装直接运行
nix run github:termide/termide

# 安装到用户配置
nix profile install github:termide/termide

# 或添加到 NixOS configuration.nix
{
  nixpkgs.overlays = [
    (import (builtins.fetchTarball "https://github.com/termide/termide/archive/main.tar.gz")).overlays.default
  ];
  environment.systemPackages = [ pkgs.termide ];
}
```

</details>

<details>
<summary><b>🤖 Android (Termux)</b></summary>

在 [Termux](https://termux.dev) 中请使用**静态 ARM64 musl** 构建（glibc 的
`aarch64-unknown-linux-gnu` 构建无法在 Android 的 Bionic libc 上运行）：

```bash
pkg install git openssh   # termide 会调用的工具（以及所需的 LSP 服务器）
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.6-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.29.6-aarch64-unknown-linux-musl.tar.gz
./termide
```

注意：Android 上没有系统剪贴板（无 X11/Wayland），资源监视器可能因 `/proc` 受限而显示不完整；
编辑器、文件管理器、git 和内置终端均可正常使用。

</details>

<details>
<summary><b>🔨 从源码构建（Cargo）</b></summary>

使用 Cargo 从源码构建：

```bash
# 克隆仓库
git clone https://github.com/termide/termide.git
cd termide

# 构建并运行
cargo run --release
```

</details>

<details>
<summary><b>🔨 从源码构建（Nix）</b></summary>

使用 Nix 从源码构建（用于开发）：

```bash
# 克隆仓库
git clone https://github.com/termide/termide.git
cd termide

# 进入开发环境（包含 Rust 工具链和所有依赖）
nix develop

# 构建项目
cargo build --release

# 运行
./target/release/termide
```

</details>

<details>
<summary><b>📦 便携静态二进制文件（Alpine / 任意 Linux）</b></summary>

每个版本都会发布完全静态的 musl 构建。它不链接任何共享库，可在任意 Linux
发行版上运行，包括 Alpine 和精简容器。整个工程是纯 Rust（rustls + russh +
russh-sftp —— 无 OpenSSL、无 libssh2），因此这与普通构建是相同的代码，只是
针对 musl 编译。

最简单的方式是从发行版下载预编译的 tarball：

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.29.6-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.29.6-x86_64-unknown-linux-musl.tar.gz
./termide

# 验证完全静态 —— 无共享库
ldd ./termide   # → "not a dynamic executable"
```

如果你想自行构建（例如针对其他 musl 变体），flake 暴露了相同的派生：

```bash
nix build github:termide/termide#termide-static
./result/bin/termide
```

任一二进制文件都可以拷贝到任何地方 —— 容器、精简的 Alpine 镜像、嵌入式设备 ——
无需安装 musl-dev 或 glibc 即可运行。

</details>

## 系统要求

- 预编译二进制文件：无额外要求
- 从源码构建：
  - Rust 1.70+（stable）
  - Nix 用户：需启用 flakes 的 Nix

## 使用方法

### 快速开始

启动 TermIDE 后，您将看到自适应宽度的布局：
- **宽终端（>= 160 列）：** 侧边栏（Git 状态与 Operations 同列堆叠）+ 两个文件管理器面板
- **普通终端（< 160 列）：** 侧边栏（Git 状态、文件管理器与 Operations 同列堆叠）+ 文件管理器面板
- 顶部为菜单栏，底部为状态栏

同列堆叠的面板高度可独立调整。`Alt+F11` 切换"全屏当前面板"预设（聚焦面板占满整列，其余仅显示标题行）；`Ctrl+Alt+=` / `Ctrl+Alt+-` 让聚焦面板增高/减小 3 行。

使用 `Alt+←/→` 在面板组之间切换，`Alt+↑/↓` 在组内导航，`Alt+M` 打开菜单。

### 文档

详细文档请参阅：
- **英文**: [doc/en/README.md](doc/en/README.md)
- **俄文**: [doc/ru/README.md](doc/ru/README.md)
- **中文**: [doc/zh/README.md](doc/zh/README.md)

### 键盘快捷键

所有快捷键均可在 `config.toml` 中自定义（参见[配置](#配置)）。核心快捷键：

- **导航：** `Alt+M` 菜单 · `Alt+H` 帮助 · `Alt+Q` 退出 · `Ctrl+P` 命令面板
- **面板：** `Alt+←/→` 与 `Alt+↑/↓` 在组之间/组内移动 · `Alt+1-9` 跳转面板 · `Alt+K` 面板操作菜单
- **打开：** `Alt+F` 文件 · `Alt+T` 终端 · `Alt+E` 编辑器 · `Alt+G` Git · `Alt+P` 设置
- **文件与查看器：** `F3` 预览（Markdown / 图表 / 十六进制 / 图片）· `Ctrl+E` 预览↔源码切换 · `Ctrl+F` 查找 · `Ctrl+R` 从磁盘重载 · `Ctrl+S` 保存

📖 完整的分面板参考（文件管理器、编辑器、git、查看器）：**[doc/zh/keybindings.md](doc/zh/keybindings.md)**。

## 配置

TermIDE 遵循 [XDG Base Directory 规范](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html) 进行文件组织。

**配置文件位置：**
- Linux/BSD: `~/.config/termide/config.toml`（或 `$XDG_CONFIG_HOME/termide/config.toml`）
- macOS: `~/Library/Application Support/termide/config.toml`
- Windows: `%APPDATA%\termide\config.toml`

**会话数据位置：**
- Linux/BSD: `~/.local/share/termide/sessions/`（或 `$XDG_DATA_HOME/termide/sessions/`）
- macOS: `~/Library/Application Support/termide/sessions/`
- Windows: `%APPDATA%\termide\sessions\`

**日志文件位置：**
- Linux/BSD: `~/.cache/termide/termide.log`（或 `$XDG_CACHE_HOME/termide/termide.log`）
- macOS: `~/Library/Caches/termide/termide.log`
- Windows: `%LOCALAPPDATA%\termide\cache\termide.log`

**书签位置：**
- Linux/BSD: `~/.local/share/termide/bookmarks.toml`（或 `$XDG_DATA_HOME/termide/bookmarks.toml`）
- macOS: `~/Library/Application Support/termide/bookmarks.toml`

### 配置示例

```toml
[general]
theme = "windows-xp"
language = "auto"  # auto, bn, de, en, es, fr, hi, id, ja, ko, pt, ru, th, tr, vi, zh
vim_mode = false
session_retention_days = 30
bell_on_operation_complete = true
icon_mode = "auto"  # auto, emoji, unicode
resource_monitor_interval = 1000

[editor]
tab_size = 4
show_git_diff = true
word_wrap = true

[file_manager]
extended_view_width = 50

[lsp]
enabled = true
auto_completion = true

[logging]
min_level = "info"
```

### 可用主题

**暗色主题：**
- `windows-xp` - 默认主题（Windows XP 风格）
- `dracula` - 流行的 Dracula 主题
- `monokai` - 经典 Monokai 主题
- `nord` - Nord 蓝色调主题
- `onedark` - Atom One Dark 主题
- `solarized-dark` - 暗色 Solarized 主题
- `midnight` - Midnight Commander 风格
- `macos-dark` - macOS 暗色风格

**亮色主题：**
- `atom-one-light` - Atom One Light 主题
- `ayu-light` - Ayu Light 主题
- `github-light` - GitHub Light 主题
- `manuscript` - 中世纪手稿风格，陈旧羊皮纸色调
- `material-lighter` - Material Lighter 主题
- `solarized-light` - 亮色 Solarized 主题
- `macos-light` - macOS 亮色风格

**复古主题：**
- `far-manager` - FAR Manager 风格
- `norton-commander` - Norton Commander 风格
- `dos-navigator` - DOS Navigator 风格
- `volkov-commander` - Volkov Commander 风格
- `windows-95` - Windows 95 风格
- `windows-98` - Windows 98 风格

**电影主题：**
- `matrix` - 黑客帝国数字雨（黑底绿字）
- `pip-boy` - 辐射 Pip-Boy 3000 磷光 CRT
- `terminator` - 天网 HUD / 火星红色调

**其他主题：**
- `terminal` - 经典终端风格（继承终端颜色）

**主题示例：**

| | | |
|:---:|:---:|:---:|
| ![Windows XP](assets/screenshots/themes/windows-xp.png) | ![Dracula](assets/screenshots/themes/dracula.png) | ![Ayu Light](assets/screenshots/themes/ayu-light.png) |
| Windows XP（默认） | Dracula | Ayu Light |
| ![Monokai](assets/screenshots/themes/monokai.png) | ![Nord](assets/screenshots/themes/nord.png) | ![Material Lighter](assets/screenshots/themes/material-lighter.png) |
| Monokai | Nord | Material Lighter |

### 自定义主题

您可以将 TOML 文件放置在主题目录中来创建自定义主题：
- Linux: `~/.config/termide/themes/`
- macOS: `~/Library/Application Support/termide/themes/`
- Windows: `%APPDATA%\termide\themes\`

用户主题优先于同名的内置主题。请参阅仓库中的 `themes/` 目录了解主题文件格式示例。

### 自定义脚本

您可以将可执行文件放置在以下目录中，将自定义脚本添加到脚本菜单：
- Linux: `~/.local/share/termide/scripts/`
- macOS: `~/Library/Application Support/termide/scripts/`
- Windows: `%APPDATA%\termide\scripts\`

**功能特性：**
- 脚本显示在脚本菜单中（菜单栏）
- 子目录创建嵌套子菜单
- 在文件名中添加 `.bg.` 以实现后台执行（例如 `deploy.bg.sh`）
- 在文件名中添加 `.report.` 以实现后台执行并显示模态输出（例如 `check.report.sh`）
- 显示名称为第一个点号之前的部分

**示例：**
```bash
# 创建脚本目录
mkdir -p ~/.local/share/termide/scripts

# 添加一个简单脚本
cat > ~/.local/share/termide/scripts/hello.sh << 'EOF'
#!/bin/bash
echo "Hello from TermIDE!"
read -p "Press Enter to close..."
EOF

# 设置可执行权限（Unix 系统必需）
chmod +x ~/.local/share/termide/scripts/hello.sh
```

**注意：** 在 Unix 系统上，脚本必须具有可执行权限（`chmod +x`）。使用 `选项 → 管理脚本` 打开脚本文件夹。

## 开发

代码库是由模块化 crate 组成的 Cargo workspace。crate 布局、面板系统和事件流程，
请参见 **[开发者指南](doc/zh/developer-guide.md)** 和 **[架构](doc/zh/architecture.md)**。

### 构建

```bash
# 开发构建
cargo build

# 带优化的发布构建
cargo build --release

# 运行测试
cargo test

# 代码质量检查
cargo clippy
cargo fmt --check
```

### Nix 开发

项目包含 Nix flake 以实现可重复的开发环境：

```bash
# 进入开发 shell
nix develop

# 使用 Nix 构建
nix build

# 运行检查
nix flake check
```

## 贡献

欢迎贡献！请随时提交 issue 和 pull request。

## 许可证

本项目基于 MIT 许可证授权。

## 致谢

使用以下技术构建：
- [ratatui](https://github.com/ratatui-org/ratatui) - 终端 UI 框架
- [crossterm](https://github.com/crossterm-rs/crossterm) - 跨平台终端控制
- [portable-pty](https://github.com/wez/wezterm/tree/main/pty) - PTY 实现
- [tree-sitter](https://github.com/tree-sitter/tree-sitter) - 语法高亮
- [ropey](https://github.com/cessen/ropey) - 文本缓冲区
- [sysinfo](https://github.com/GuillaumeGomez/sysinfo) - 系统资源监控
