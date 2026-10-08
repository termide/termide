# TermIDE

[![GitHub Release](https://img.shields.io/github/v/release/termide/termide)](https://github.com/termide/termide/releases)
[![CI](https://github.com/termide/termide/actions/workflows/release.yml/badge.svg)](https://github.com/termide/termide/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | **中文** | [Русский](README.ru.md)

一站式终端工作台，适用于你的工作站和服务器：带 LSP 的代码编辑器、支持 SFTP/FTP 的双栏文件管理器、终端、git、数据库查看器和编程智能体 —— 全部集成在一个零配置、使用 Rust 编写的静态二进制文件中。

**[网站](https://termide.github.io)** | **[文档](doc/zh/README.md)** | **[版本发布](https://github.com/termide/termide/releases)** | **[截图](https://termide.github.io/zh/#screenshots)**

<p align="center"><img src="assets/screenshots/termide.gif" alt="TermIDE — 编辑器、文件管理器、终端和查看器合为一个 TUI" width="900"></p>

## 为什么选择 TermIDE？

终端编辑器负责代码；而代码之外的一切 —— 远程主机上的文件、数据库、git、长时间运行的 shell、编程智能体 —— 通常需要插件或单独的工具。TermIDE 将这些全部集成在一个二进制文件中，在笔记本、服务器或手机上都能开箱即用。它并不试图取代这些工具，而是把你每天都要用到的那部分集中到一处：

| 任务 | 通常使用 | 在 TermIDE 中 |
|------|----------|---------------|
| SSH 断开后保持工作不中断 | tmux、screen | 可分离实例 |
| 在主机之间传输文件 | mc、ranger、scp | 支持 SFTP / FTP 的双栏文件管理器 |
| 编辑代码和配置 | vim、nano | 带 LSP 的编辑器 |
| 审查并提交 | lazygit、tig | Git 状态、日志和差异面板 |
| 找出占用资源的进程 | htop、ss | 资源监控 |
| 查看数据库 | sqlite3、psql | 数据库查看器 |
| 让模型修改代码 | aider、Claude Code | 智能体面板，或在其中运行这些智能体 |

编辑器、LSP、终端、git 和项目布局在终端编辑器中已是标配；下表列出的是其他编辑器所缺少或需要插件才能实现的功能：

| 功能 | TermIDE | Fresh | Vim/Neovim | Helix | Micro |
|---------|:-------:|:-----:|:----------:|:-----:|:-----:|
| 内置编程智能体（本地或云端模型） | ✓ | ✗ | 插件 | ✗ | ✗ |
| MCP 服务器 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 双栏文件管理器 | ✓ | 仅文件树 | 插件 | ✗ | ✗ |
| 后台文件操作 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 密码保险库 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 数据库查看器 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 十六进制 / 二进制查看器与编辑器 | ✓ | ✗ | 插件 | ✗ | 插件 |
| 图表查看器（Mermaid） | ✓ | ✗ | 插件 | ✗ | ✗ |
| HTML 预览 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 图片查看器 | ✓ | ✗ | 插件 | ✗ | ✗ |
| 资源监控 | ✓ | ✗ | ✗ | ✗ | ✗ |

**TermIDE = 编辑器 + 文件管理器 + 终端 + Git + 智能体，集成于一个 TUI 应用程序中。**

## 设计原则

- **自给自足** - 单个静态二进制文件，无任何运行时依赖：SSH、TLS 和加密均为纯 Rust 实现，因此同一个文件可在 Alpine、distroless 容器或 Termux 中运行。系统中已有的工具 —— git、语言服务器、供智能体网页搜索使用的浏览器 —— 存在时会自动使用。
- **桌面与服务器皆宜** - 在工作站上支持原生图形和系统剪贴板；通过 SSH 使用时，`termide --detached` 让编辑器、shell 和任务在断线后继续运行（[可分离实例](doc/zh/detached-instances.md)），文件管理器可通过 SFTP / FTP 访问其他主机。
- **你的数据归你所有** - 没有遥测，也不检查更新：termide 只连接你指定的服务器、数据库和模型端点。在你配置模型之前，智能体处于关闭状态；使用本地模型时，你的代码永远不会离开本机。
- **机密受到保护** - 连接密码保存在由主密码保护的加密保险库中（Argon2 + ChaCha20-Poly1305），绝不会出现在书签、布局或日志中（[密码保险库](doc/zh/passwords.md)）；API 密钥从环境变量读取。
- **没有任何隐藏** - 智能体模型看到的每一段提示词 —— 系统提示词、服务提示词、工具描述 —— 都是可以阅读和覆盖的普通文件，`/prompt` 可显示组装后的完整结果（[系统提示词](doc/zh/agent.md#系统提示词)）。设置、快捷键、主题和命令均为 TOML 格式。

## 功能特性

| | |
|:---:|:---:|
| <img src="assets/screenshots/agent.png" alt="工作中的编程智能体" width="440"> | <img src="assets/screenshots/file-manager.png" alt="带嵌套 Git 状态的文件管理器" width="440"> |
| 工作中的编程智能体 | 带嵌套 Git 状态的文件管理器 |
| <img src="assets/screenshots/db.png" alt="数据库查看器" width="440"> | <img src="assets/screenshots/git.png" alt="带提交图的 Git 日志" width="440"> |
| 数据库查看器 | 带提交图的 Git 日志 |

### 代码

- **编辑器** - 23 种语言的语法高亮；LSP 代码补全、悬停提示、跳转到定义、查找引用、重命名和诊断；切换注释、自动缩进、自动闭合括号；可选 Vim 模式
- **大纲与诊断** - 与光标同步的代码结构导航（`Alt+O`）和 LSP 诊断面板（`Alt+I`）
- **搜索和替换** - 实时预览、匹配计数、正则表达式
- **文档与代码并排** - 渲染 Markdown、HTML（可将页面另存为 Markdown 的文本模式浏览器）以及以文本绘制的 Mermaid 图表；`Ctrl+E` 切换到源码
- **十六进制编辑器** - 带字节光标、选择和搜索的 hex/ASCII 视图；覆盖编辑并生成 `.bak` 备份
- **图片** - 在 Kitty、WezTerm、iTerm2、Ghostty 和 foot 中原生渲染图形

### 编程智能体

- **自带模型** - 任意 OpenAI 或 Anthropic 兼容端点：本机上的 llama.cpp、Ollama、vLLM、omlx，或托管服务
- **未经你同意不做任何修改** - 每次工具调用都需许可，或在 auto 模式下交由审查模型决定；`/undo` 和检查点可撤回修改
- **计划模式与子代理** - 智能体在写出计划之前与你逐轮敲定待决问题；子代理可并行工作
- **工具** - 读取、编辑、shell、网页搜索与抓取、检索以往会话、git 历史和项目文件的 `recall`，以及 MCP 服务器
- **同一面板中的其他智能体** - 通过 Agent Client Protocol 接入 Claude Code、Codex 和 Gemini CLI
- **以文件扩展** - 技能、提示模板、命令脚本、钩子和项目说明
- **无界面运行** - `termide --prompt "..." --output json` 在脚本和 CI 中运行同一个智能体

### 文件与数据

- **双栏文件管理器** - 带嵌套 Git 状态的树形视图、glob 和正则搜索、批量操作；zip、tar 和 ISO 压缩包可像文件夹一样打开，按 `P` 打包所选内容；文件可通过系统剪贴板与其他应用互相复制粘贴
- **远程文件系统** - 纯 Rust 实现的 SFTP、FTP 和 FTPS，可在本地与远程面板之间复制；`smb://` 和 `nfs://` 通过系统挂载访问
- **后台操作** - 复制、移动、上传和下载，带进度显示，支持暂停、恢复和取消
- **数据库查看器** - 通过书签 URL 打开 SQLite、PostgreSQL 和 MySQL：服务端排序、按列过滤、单元格编辑，整行导出为 TSV、JSON 或 INSERT
- **密码保险库** - 远程主机、数据库和 git 的密码保存在由主密码保护的加密保险库中
- **书签与目录切换器** - 保存常用位置，使用 `Ctrl+\` 快速切换目录

### 服务器与运维

- **可分离实例** - `termide --detached` 让编辑器、shell 和任务在终端关闭后继续运行；`--attach` 可在任意尺寸的终端中恢复（仅限 Unix）
- **集成终端** - 完整的 PTY 支持、VT100 转义序列、鼠标跟踪
- **资源监控** - 菜单栏和状态栏显示 CPU、内存、网络和磁盘；点击即可查看占用最高的进程和监听端口
- **你的 `$EDITOR`** - `EDITOR=termide` 可用于 `git commit`、`crontab -e` 和 `visudo`
- **单个静态二进制文件** - Linux x86_64 和 ARM64（glibc 或 musl）、macOS、原生 Windows 以及 Android Termux

### 工作区

- **Git** - 状态、带彩色提交图的日志、差异、暂存、stash、blame、分支及其工作树
- **项目** - 按项目恢复面板布局；切换离开的项目在后台继续运行，并以按钮形式显示在菜单栏中；`termide --restore` 重新打开上次运行的项目
- **多面板布局** - 高度可调的面板组、全屏切换（`Alt+F11`），终端变窄时自动堆叠
- **自定义命令** - 全局或项目级命令，支持快捷键、参数表单以及终端 / 后台 / 报告模式
- **命令面板与打开提示** - `Ctrl+P` 通过模糊名称运行任意命令；`Ctrl+G` 打开文件、文件夹或 URL，带路径建议
- **设置** - 全屏设置窗口（`Alt+P`），可直接录制快捷键

### 外观与操作

- **44 款内置主题** - 暗色、亮色、复古和电影主题；可用 TOML 编写自己的主题
- **15 种界面语言** - 孟加拉语、中文、英语、法语、德语、印地语、印尼语、日语、韩语、葡萄牙语、俄语、西班牙语、泰语、土耳其语、越南语
- **键盘与鼠标** - 完整鼠标支持；快捷键在西里尔文键盘布局下同样有效；`Shift+Enter` 用系统应用打开文件

## 常见问题

**必须使用 AI 智能体吗？** 不必。默认没有设置任何提供商或模型，因此在你配置之前，智能体不会做任何事，也不会发送任何内容。其他功能都可以独立使用。

**TermIDE 会回传数据吗？** 没有遥测、无需账号，也不检查更新。只有在你要求时才会联网：访问远程位置、网页、`git push` 或 `pull`，或调用智能体的模型。

**它能取代 tmux 吗？** 对于通过 SSH 保持工作不中断这一常见场景，可以：`termide --detached` 让整个工作区保持运行，`--attach` 可将其恢复。它不像 tmux 那样管理任意会话和窗口，也可以在 tmux 中正常运行。

**需要什么终端？** 任何支持真彩色的现代终端。在 Kitty、WezTerm、iTerm2、Ghostty 和 foot 中可原生显示图片；macOS 上的 `Alt` 快捷键需要支持 Kitty 键盘协议的终端。

**能在 Windows 上运行吗？** 可以，通过 ConPTY 在 Windows Terminal 中原生运行，或在 WSL 中运行。可分离实例仅限 Unix。

## 安装

Linux 和 macOS —— 脚本会检测你的系统并提供适合的安装方式（软件包、Homebrew、二进制文件、Nix 或 Cargo）：

```bash
curl -fsSL https://raw.githubusercontent.com/termide/termide/main/install.sh | sh
```

或使用包管理器：

```bash
brew tap termide/termide && brew install termide   # macOS / Linux
yay -S termide-bin                                 # Arch Linux（AUR）
nix run github:termide/termide                     # Nix，无需安装
```

在服务器上，只需复制[静态 musl 二进制文件](#portable-static-binary)并运行，无需安装其他任何东西。

**支持的平台：** Linux（x86_64、ARM64）、macOS（Intel、Apple Silicon）、Windows（x86_64）

### 选择安装方式

<details>
<summary><b>📦 预编译二进制文件</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载适合您平台的最新版本：

```bash
# Linux x86_64（也适用于 WSL）
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-unknown-linux-gnu.tar.gz
tar xzf termide-0.40.0-x86_64-unknown-linux-gnu.tar.gz
./termide

# Linux x86_64（静态 musl — Alpine、distroless 容器、任何无 glibc 的系统）
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.40.0-x86_64-unknown-linux-musl.tar.gz
./termide

# macOS Intel (x86_64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-apple-darwin.tar.gz
tar xzf termide-0.40.0-x86_64-apple-darwin.tar.gz
./termide

# macOS Apple Silicon (ARM64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-apple-darwin.tar.gz
tar xzf termide-0.40.0-aarch64-apple-darwin.tar.gz
./termide

# Linux ARM64（树莓派、ARM 服务器）
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-unknown-linux-gnu.tar.gz
tar xzf termide-0.40.0-aarch64-unknown-linux-gnu.tar.gz
./termide

# Linux ARM64（静态 musl —— Android/Termux、Alpine ARM、任何无 glibc 的 ARM64）
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.40.0-aarch64-unknown-linux-musl.tar.gz
./termide

# Windows x86_64（从 Releases 下载 .zip，解压后在 Windows Terminal 中运行）
# https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-pc-windows-msvc.zip
```

</details>

<details>
<summary><b>🪟 Windows (.zip)</b></summary>

TermIDE 通过 ConPTY 在 Windows 10+ 上原生运行。建议使用 **Windows Terminal**
以获得最佳体验。

1. 从 [GitHub Releases](https://github.com/termide/termide/releases) 下载 `termide-0.40.0-x86_64-pc-windows-msvc.zip`。
2. 解压压缩包。
3. 在 Windows Terminal 中运行 `termide.exe`。

配置、项目布局和日志均位于 `%APPDATA%\termide\`。

或者在 **WSL/WSL2** 中，像在任意 Linux 上一样使用 Linux x86_64 构建
（`termide-0.40.0-x86_64-unknown-linux-gnu.tar.gz`）。

</details>

<details>
<summary><b>🐧 Debian/Ubuntu (.deb)</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载并安装 `.deb` 包：

```bash
# 仅限 x86_64（ARM64 请使用上面的 tar.gz）
wget https://github.com/termide/termide/releases/latest/download/termide_0.40.0-1_amd64.deb
sudo dpkg -i termide_0.40.0-1_amd64.deb
```

</details>

<details>
<summary><b>🎩 Fedora/RHEL/CentOS (.rpm)</b></summary>

从 [GitHub Releases](https://github.com/termide/termide/releases) 下载并安装 `.rpm` 包：

```bash
# 仅限 x86_64（ARM64 请使用上面的 tar.gz）
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-1.x86_64.rpm
sudo rpm -i termide-0.40.0-1.x86_64.rpm
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
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.40.0-aarch64-unknown-linux-musl.tar.gz
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

<a id="portable-static-binary"></a>
<details>
<summary><b>📦 便携静态二进制文件（Alpine / 任意 Linux）</b></summary>

每个版本都会发布完全静态的 musl 构建。它不链接任何共享库，可在任意 Linux
发行版上运行，包括 Alpine 和精简容器。整个工程是纯 Rust（rustls + russh +
russh-sftp —— 无 OpenSSL、无 libssh2），因此这与普通构建是相同的代码，只是
针对 musl 编译。

最简单的方式是从发行版下载预编译的 tarball：

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.40.0-x86_64-unknown-linux-musl.tar.gz
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

## 命令行选项

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
  -r, --restore        在上次运行所在的项目中重新打开上次运行的项目
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
- **面板：** `Alt+←/→` 与 `Alt+↑/↓` 在组之间/组内移动 · `Alt+K` 面板操作菜单
- **项目：** `Alt+1-9` 切换到已打开的项目 · `Alt+\` 项目切换器 · `Alt+N` 新建项目
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

### 主题

44 款内置主题 —— 暗色、亮色、复古（Norton Commander、FAR Manager、Windows 95）和电影风格（Matrix、Pip-Boy）—— 可通过菜单切换，或在 `config.toml` 中设置 `theme`。完整列表见[主题](doc/zh/themes.md)。

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

代码库是由模块化 crate 组成的 Cargo workspace；工具链版本由 `rust-toolchain.toml` 固定。构建、测试、Nix 开发环境和 pre-commit 钩子见 **[开发者指南](doc/zh/developer-guide.md)**；crate 布局、面板系统和事件流见 **[架构](doc/zh/architecture.md)**。

## 贡献

欢迎提交 issue 和 pull request。每个克隆执行一次 `git config core.hooksPath .githooks`：pre-commit 钩子会运行与 CI 相同的 `fmt`、`clippy` 和测试检查。

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
- [russh](https://github.com/Eugeny/russh) 和 [russh-sftp](https://github.com/AspectUnk/russh-sftp) - 纯 Rust 实现的 SSH 和 SFTP
- [suppaftp](https://github.com/veeso/suppaftp) - FTP / FTPS
- [rustls](https://github.com/rustls/rustls) - 无需 OpenSSL 的 TLS
- [SQLx](https://github.com/launchbadge/sqlx) - SQLite、PostgreSQL 和 MySQL 访问
- [RustCrypto](https://github.com/RustCrypto) - 密码保险库使用的 Argon2 和 ChaCha20-Poly1305
- [nucleo](https://github.com/helix-editor/nucleo) - 模糊匹配
- [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) 和 [html5ever](https://github.com/servo/html5ever) - Markdown 和 HTML 解析
