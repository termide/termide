# TermIDE

[![GitHub Release](https://img.shields.io/github/v/release/termide/termide)](https://github.com/termide/termide/releases)
[![CI](https://github.com/termide/termide/actions/workflows/release.yml/badge.svg)](https://github.com/termide/termide/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | **中文** | [Русский](README.ru.md)

一站式终端工作台，适用于你的工作站和服务器：带 LSP 的代码编辑器、支持 SFTP/FTP 的双栏文件管理器、终端、git、数据库查看器和编程智能体 —— 全部集成在一个零配置、使用 Rust 编写的静态二进制文件中。

**[网站](https://termide.github.io)** | **[文档](doc/zh/README.md)** | **[版本发布](https://github.com/termide/termide/releases)** | **[截图](https://termide.github.io/zh/#screenshots)**

<p align="center"><img src="assets/screenshots/termide.gif" alt="TermIDE — 编辑器、文件管理器、终端和查看器合为一个 TUI" width="900"></p>

## 为什么选择 TermIDE？

终端编辑器负责代码；而代码之外的一切 —— 远程主机上的文件、数据库、git、长时间运行的 shell、编程智能体 —— 通常需要插件或单独的工具。TermIDE 将这些全部集成在一个二进制文件中，在笔记本、服务器或手机上都能开箱即用：

| 功能 | TermIDE | Fresh | Vim/Neovim | Helix | Micro |
|---------|:-------:|:-----:|:----------:|:-----:|:-----:|
| LSP 支持 | ✓ | ✓ | ✓ | ✓ | 插件 |
| 零配置 | ✓ | ✓ | ✗ | ✓ | ✓ |
| 脚本自动化 | ✓ | ✓ | ✓ | ✗ | 插件 |
| 外部智能体（Claude Code、Codex、Gemini CLI） | ✓ | ✓ | 插件 | ✗ | ✗ |
| 远程文件系统（SFTP/FTP） | ✓ | SSH | ✓ | ✗ | ✗ |
| Markdown 预览 | ✓ | ✓ | 插件 | ✗ | ✗ |
| 内置终端 | ✓ | ✓ | 插件 | ✗ | ✗ |
| Git 集成 | ✓ | ✓ | 插件 | ✗ | ✗ |
| 项目布局 | ✓ | ✓ | 插件 | ✗ | ✗ |
| 多面板布局 | ✓ | ✓ | 插件 | ✗ | ✗ |
| 书签 | ✓ | ✓ | 插件 | ✗ | ✗ |
| 十六进制 / 二进制查看器 | ✓ | ✗ | 插件 | ✗ | 插件 |
| 浏览压缩包（zip/tar） | ✓ | ✗ | ✓ | ✗ | ✗ |
| 文件管理器 | ✓ | 仅文件树 | 插件 | ✗ | ✗ |
| 可分离实例 | ✓ | ✓ | ✗ | ✗ | ✗ |
| 内置编程智能体（本地或云端模型） | ✓ | ✗ | 插件 | ✗ | ✗ |
| MCP 服务器 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 数据库查看器 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 图表查看器（Mermaid） | ✓ | ✗ | 插件 | ✗ | ✗ |
| 图片查看器 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 后台文件操作 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 资源监控 | ✓ | ✗ | ✗ | ✗ | ✗ |

**TermIDE = 编辑器 + 文件管理器 + 终端 + Git + 智能体，集成于一个 TUI 应用程序中。**

## 功能特性

- **基于终端的 IDE** - 支持 23 种语言的语法高亮、单词导航（Ctrl+Left/Right）、段落/符号导航（Ctrl+Up/Down）、自动缩进、自动关闭括号
- **LSP 支持** - 代码补全、查找引用、重命名符号、跳转到定义，通过 rust-analyzer、pylsp、typescript-language-server 及其他 LSP 服务器实现
- **编码代理** - 一个面板（`Alt+A`），语言模型通过任意 OpenAI 或 Anthropic 兼容端点（本地 llama.cpp / Ollama / vLLM / omlx 或托管服务）在你的项目中读取、编辑和运行命令，每次工具调用都会征求许可，并可通过 `/undo` 和检查点撤回其修改；技能、提示模板、MCP 服务器、命令钩子，以及通过 ACP 接入的外部代理（Claude Code、Codex、Gemini CLI）都在同一面板中
- **智能文件管理器** - 可展开目录的树形视图、嵌套 Git 状态、批量操作、文件/内容搜索（glob/正则表达式）、树内增量搜索；zip、tar 和 ISO 压缩包可像只读目录一样打开（包括服务器上的和嵌套在其他压缩包中的），按 `P` 可将所选内容打包为 zip 或 tar
- **远程文件系统** - 在文件管理器中通过 SFTP / FTP / FTPS 浏览和编辑远程服务器上的文件，在本地与远程面板之间复制 —— 纯 Rust（russh + rustls），无需原生库，可在静态 musl 上运行（`smb://` / `nfs://` 走系统挂载）
- **后台文件操作** - 复制、移动、上传、下载、删除及批量传输在后台运行，每个操作带进度条、字节/耗时读数，支持暂停 / 恢复 / 取消（操作面板）
- **集成终端** - 完整的 PTY 支持、VT100 转义序列、鼠标跟踪
- **Git 集成** - 状态面板、带彩色 Unicode 提交图（ASCII 回退）的提交日志、暂存/取消暂存、分支及其工作树（worktree）、分支切换、暂存管理（stash）、内联 blame 注解
- **数据库查看器** - 通过书签 URL 打开的 SQLite / PostgreSQL / MySQL 只读浏览器：带二维单元格光标的表格、服务端单列排序与按列类型感知过滤、滑动窗口分页，以及可复制为 TSV / JSON / INSERT 的整行详情对话框
- **多面板布局** - 垂直拆分的面板组，每个面板高度可调，一键全屏切换（`Alt+F11`）；终端变窄时智能自动堆叠
- **图片查看器** - 在 Kitty、WezTerm、iTerm2、Ghostty、foot 终端中原生渲染图形
- **十六进制 / 二进制查看与编辑器** - 二进制文件的 hex/ASCII 视图（按 16 字节自适应分段），字节光标在两个区域同时显示，支持拖动/Shift 选择与剪贴板复制、ASCII 与十六进制字节搜索，以及 hex↔文本切换（`Ctrl+L`）；`F4` 以覆盖方式编辑，保存时生成 `.bak` 备份
- **Markdown 预览** - `.md` / `.markdown` 的只读渲染视图（标题、列表、表格、语法高亮代码块、可点击链接与图片图标），支持光标导航、选择与剪贴板复制；`Ctrl+E` 切换到可编辑源码；内嵌的 ```mermaid``` 代码块渲染为图表
- **Mermaid 图表查看器** - 将 `.mmd` / `.mermaid` 文件渲染为文本伪图形 —— flowchart、sequence、state、class、ER、gantt、pie、journey、mindmap、timeline、gitGraph、quadrant；二维滚动、复制到剪贴板，`Ctrl+E` 编辑源码
- **外部应用** - 使用系统默认应用程序打开文件（Shift+Enter）
- **38 款内置主题** - 暗色、亮色、复古和电影主题（Dracula、Nord、Monokai、Solarized、Matrix、Pip-Boy、Norton Commander、Windows 95 等）
- **自定义主题** - 使用 TOML 格式创建自己的主题
- **15 种界面语言** - 孟加拉语、中文、英语、法语、德语、印地语、印尼语、日语、韩语、葡萄牙语、俄语、西班牙语、泰语、土耳其语、越南语
- **项目管理** - 按项目自动保存和恢复面板布局；切换离开的项目会在后台保持打开（终端继续运行，未保存的修改得以保留），“项目”菜单和 `Alt+\` 切换窗口列出已打开和最近使用的项目
- **可分离实例** - `termide --detached` 让整个实例（编辑器、shell、LSP 服务器、运行中的任务）在终端关闭后继续运行；`termide --attach` 可从任意终端、任意尺寸重新接入（仅限 Unix）
- **系统监控** - 菜单栏实时显示 CPU、RAM、网络 I/O；状态栏显示磁盘使用情况；点击指标可打开详细模态窗口
- **搜索和替换** - 实时预览、匹配计数、正则表达式支持
- **自定义命令** - 在命令菜单中运行 `commands.toml`（全局和项目级）中定义的 shell 命令：快捷键、分组、参数表单，以及终端 / 后台 / 报告模式
- **跨平台** - Linux（x86_64、ARM64）、macOS（Intel、Apple Silicon）、Windows（原生 ConPTY、WSL）
- **完整鼠标支持** - 点击导航、滚动、双击操作
- **键盘布局** - 西里尔文支持，自动快捷键翻译
- **Vim 模式** - 可选的 Vim 风格编辑，支持西里尔文键盘
- **命令面板** - 使用 Ctrl+P 快速打开命令，支持模糊匹配
- **打开提示** - 使用 Ctrl+G 打开文件、目录或 URL，带路径建议
- **目录切换器** - 使用 `Ctrl+\` 快速切换目录
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
wget https://github.com/termide/termide/releases/latest/download/termide-0.38.0-x86_64-unknown-linux-gnu.tar.gz
tar xzf termide-0.38.0-x86_64-unknown-linux-gnu.tar.gz
./termide

# Linux x86_64（静态 musl — Alpine、distroless 容器、任何无 glibc 的系统）
wget https://github.com/termide/termide/releases/latest/download/termide-0.38.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.38.0-x86_64-unknown-linux-musl.tar.gz
./termide

# macOS Intel (x86_64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.38.0-x86_64-apple-darwin.tar.gz
tar xzf termide-0.38.0-x86_64-apple-darwin.tar.gz
./termide

# macOS Apple Silicon (ARM64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.38.0-aarch64-apple-darwin.tar.gz
tar xzf termide-0.38.0-aarch64-apple-darwin.tar.gz
./termide

# Linux ARM64（树莓派、ARM 服务器）
wget https://github.com/termide/termide/releases/latest/download/termide-0.38.0-aarch64-unknown-linux-gnu.tar.gz
tar xzf termide-0.38.0-aarch64-unknown-linux-gnu.tar.gz
./termide

# Linux ARM64（静态 musl —— Android/Termux、Alpine ARM、任何无 glibc 的 ARM64）
wget https://github.com/termide/termide/releases/latest/download/termide-0.38.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.38.0-aarch64-unknown-linux-musl.tar.gz
./termide

# Windows x86_64（从 Releases 下载 .zip，解压后在 Windows Terminal 中运行）
# https://github.com/termide/termide/releases/latest/download/termide-0.38.0-x86_64-pc-windows-msvc.zip
```

</details>

<details>
<summary><b>🪟 Windows (.zip)</b></summary>

TermIDE 通过 ConPTY 在 Windows 10+ 上原生运行。建议使用 **Windows Terminal**
以获得最佳体验。

1. 从 [GitHub Releases](https://github.com/termide/termide/releases) 下载 `termide-0.38.0-x86_64-pc-windows-msvc.zip`。
2. 解压压缩包。
3. 在 Windows Terminal 中运行 `termide.exe`。

配置、项目布局和日志均位于 `%APPDATA%\termide\`。

或者在 **WSL/WSL2** 中，像在任意 Linux 上一样使用 Linux x86_64 构建
（`termide-0.38.0-x86_64-unknown-linux-gnu.tar.gz`）。

</details>

<details>
<summary><b>🐧 Debian/Ubuntu (.deb)</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载并安装 `.deb` 包：

```bash
# 仅限 x86_64（ARM64 请使用上面的 tar.gz）
wget https://github.com/termide/termide/releases/latest/download/termide_0.38.0-1_amd64.deb
sudo dpkg -i termide_0.38.0-1_amd64.deb
```

</details>

<details>
<summary><b>🎩 Fedora/RHEL/CentOS (.rpm)</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载并安装 `.rpm` 包：

```bash
# 仅限 x86_64（ARM64 请使用上面的 tar.gz）
wget https://github.com/termide/termide/releases/latest/download/termide-0.38.0-1.x86_64.rpm
sudo rpm -i termide-0.38.0-1.x86_64.rpm
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
wget https://github.com/termide/termide/releases/latest/download/termide-0.38.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.38.0-aarch64-unknown-linux-musl.tar.gz
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
wget https://github.com/termide/termide/releases/latest/download/termide-0.38.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.38.0-x86_64-unknown-linux-musl.tar.gz
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

### 命令行选项

```
termide [OPTIONS] [FILE]...

参数:
  [FILE]...            要打开的文件或目录。给定路径时，TermIDE 以干净的视图启动
                       （不恢复也不保存项目布局）。文本在编辑器中打开，因此可作为
                       git、crontab、visudo 等的 $EDITOR 使用；图片、SQLite 文件、
                       其他二进制文件和目录会在相应的查看器、十六进制编辑器或
                       文件管理器中打开。

选项:
  --log-level <LEVEL>  设置日志级别（trace、debug、info、warn、error）
  --no-lsp             禁用 LSP 语言服务器
  --config <PATH>      使用自定义配置文件路径
  --diagnostics        运行启动前诊断并退出（无 UI）
  --detached           启动一个在终端关闭后仍继续运行的可分离实例，并打印其 ID
                       （仅限 Unix）
  --attach [<ID>]      接入某个可分离实例，省略时接入最近的一个
  -f, --force          与 --attach 连用：从已接入的客户端接管该实例，
                       并使该客户端分离
  --kill <ID>          结束某个可分离实例及其中所有 shell 和任务后退出；
                       其中未保存的更改将丢失
  --list-instances     列出可分离实例并退出
  --completions <SHELL>
                       打印补全脚本（bash、zsh、fish）并退出
  --install-completions [<SHELL>]
                       为 $SHELL 或指定的 shell 安装补全脚本
  --prompt <PROMPT>    不启动 UI 运行一次智能体任务，将回答打印到 stdout 后退出；
                       `-` 表示从 stdin 读取提示
  --agent <NAME>       与 --prompt 连用：使用的智能体定义
  --output <FORMAT>    与 --prompt 连用：text（默认）、json 或 stream-json
  -h, --help           打印帮助
  -V, --version        打印版本
```

用作您的编辑器：

```sh
export EDITOR=termide   # git commit, crontab -e, visudo, ...
```

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

**项目数据位置：**
- Linux/BSD: `~/.local/share/termide/projects/`（或 `$XDG_DATA_HOME/termide/projects/`）
- macOS: `~/Library/Application Support/termide/projects/`
- Windows: `%APPDATA%\termide\projects\`

**日志文件位置：** 每次运行都会在上述项目数据位置下对应项目的目录中写入各自的
`session-<date>-<time>.log`；超过 24 小时的日志会被删除。设置 `logging.file_path`
可改为使用单个固定文件。

**书签位置：**
- Linux/BSD: `~/.config/termide/bookmarks.toml`（或 `$XDG_CONFIG_HOME/termide/bookmarks.toml`）
- macOS: `~/Library/Application Support/termide/bookmarks.toml`
- Windows: `%APPDATA%\termide\bookmarks.toml`

### 配置示例

```toml
[general]
theme = "windows-xp"
language = "auto"  # auto, bn, de, en, es, fr, hi, id, ja, ko, pt, ru, th, tr, vi, zh
vim_mode = false
project_retention_days = 30
bell_on_operation_complete = true
icon_mode = "auto"  # auto, emoji, unicode
always_detachable = false  # 实例在终端关闭后继续运行（仅限 Unix）
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

### 自定义命令

常用的 shell 命令写在 `commands.toml` 中——全局文件位于配置目录，项目文件位于
`<项目>/.termide/commands.toml`——并显示在**命令**菜单中：

```toml
[test]
name = "Run tests"
command = "cargo nextest run"
group = "cargo"
key = "Ctrl+Shift+T"

[clippy]
command = "cargo clippy --workspace -- -D warnings"
mode = "report"  # terminal（默认）、background 或 report
```

`命令 → 添加命令...` 通过表单创建命令。模式、参数和快捷键见
[自定义命令](doc/zh/actions.md)。

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
