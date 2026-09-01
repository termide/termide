# 安装指南

本指南介绍在您的系统上安装 TermIDE 的不同方法。

## 下载预编译二进制文件（推荐）

最简单的入门方式是下载适合您平台的预编译二进制文件。

### 第 1 步：下载

访问 [GitHub Releases](https://github.com/termide/termide/releases) 页面，下载适合您平台的最新版本：

**Linux x86_64**（也适用于 WSL/WSL2）：
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-x86_64-unknown-linux-gnu.tar.gz
```

**Linux x86_64 — 静态 musl**（Alpine、distroless 容器、任何没有 glibc
的系统）：
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-x86_64-unknown-linux-musl.tar.gz
```

**Linux ARM64**（树莓派、ARM 服务器）：
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-aarch64-unknown-linux-gnu.tar.gz
```

**Linux ARM64 — 静态 musl**（Android/Termux、Alpine ARM、任何无 glibc 的 ARM64 系统）：
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-aarch64-unknown-linux-musl.tar.gz
```

**macOS Intel (x86_64)**：
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.31.0-x86_64-apple-darwin.tar.gz
```

**macOS Apple Silicon (M1/M2/M3)**：
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.31.0-aarch64-apple-darwin.tar.gz
```

### 第 2 步：解压

```bash
tar xzf termide-*.tar.gz
```

### 第 3 步：运行

```bash
./termide
```

### 第 4 步：全局安装（可选）

要将 TermIDE 安装到系统中，请将二进制文件移动到 PATH 中的目录：

```bash
# Linux
sudo mv termide /usr/local/bin/

# macOS
sudo mv termide /usr/local/bin/
```

现在您可以在终端的任何位置运行 `termide`。

## 便携静态二进制文件（Alpine / 容器）

每个版本都会发布完全静态的 **musl** 构建：它不链接任何共享库，可在任意 Linux
发行版上运行，包括 Alpine 和精简容器。整个工程是纯 Rust（rustls + russh +
russh-sftp —— 无 OpenSSL、无 libssh2），因此这与普通构建是相同的代码，只是针对
musl 编译。

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.31.0-x86_64-unknown-linux-musl.tar.gz
./termide

# 验证完全静态 —— 无共享库
ldd ./termide   # → "not a dynamic executable"
```

如需自行构建（例如针对其他 musl 变体），flake 暴露了相同的派生：

```bash
nix build github:termide/termide#termide-static
./result/bin/termide
```

该二进制文件可拷贝到容器或精简的 Alpine 镜像中，无需安装 `musl-dev` 或 `glibc`
即可运行。（ARM64 musl 构建也是 [Android / Termux](#android-termux) 所使用的。）

## 通过包管理器安装

### Debian/Ubuntu (.deb)

```bash
wget https://github.com/termide/termide/releases/latest/download/termide_0.31.0-1_amd64.deb
sudo dpkg -i termide_0.31.0-1_amd64.deb
```

### Fedora/RHEL/CentOS (.rpm)

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-1.x86_64.rpm
sudo rpm -i termide-0.31.0-1.x86_64.rpm
```

### Arch Linux (AUR)

```bash
# 从源码构建
yay -S termide

# 或安装预编译二进制文件
yay -S termide-bin
```

### Homebrew (macOS/Linux)

```bash
brew tap termide/termide
brew install termide
```

### NixOS/Nix (Flakes)

```bash
# 无需安装直接运行
nix run github:termide/termide

# 安装到用户配置文件
nix profile install github:termide/termide
```

## 从源码构建

### 前置条件

- **Rust 1.70+**（stable 工具链）
- **Git**

### 使用 Cargo

```bash
# 克隆仓库
git clone https://github.com/termide/termide.git
cd termide

# 以 release 模式构建
cargo build --release

# 二进制文件位于 target/release/termide
./target/release/termide

# 可选：安装到 ~/.cargo/bin
cargo install --path .

# 或者无需克隆，直接从仓库安装：
cargo install --git https://github.com/termide/termide --locked
```

> **注意：** TermIDE **未**发布到 crates.io。`cargo install termide` 会拉取一个
> 过时且不相关的早期版本——请始终从源码构建（上面的克隆方式）或使用
> `cargo install --git …`。

### 使用 Nix（配合 Flakes）

```bash
# 克隆仓库
git clone https://github.com/termide/termide.git
cd termide

# 进入开发 shell
nix develop

# 使用 cargo 构建
cargo build --release

# 或直接使用 Nix 构建
nix build
```

## 平台特定说明

### Linux

预编译二进制文件无需额外依赖。

从源码构建时，可能需要安装开发包：
```bash
# Debian/Ubuntu
sudo apt-get install build-essential

# Fedora/RHEL
sudo dnf install gcc
```

### macOS

首次运行时，macOS 可能会因为应用程序未签名而阻止运行。要允许运行：
1. 右键点击 `termide` 并选择"打开"
2. 在安全对话框中点击"打开"

或者，移除隔离属性：
```bash
xattr -d com.apple.quarantine termide
```

### Windows（原生）

TermIDE 通过 ConPTY 在 Windows 10+ 上原生运行。建议使用 Windows Terminal 以获得最佳体验。

1. 从 [GitHub Releases](https://github.com/termide/termide/releases) 下载 `.zip` 压缩包：
   - `termide-VERSION-x86_64-pc-windows-msvc.zip`
2. 解压压缩包
3. 在 Windows Terminal 中运行 `termide.exe`

**配置路径：**
- 配置文件: `%APPDATA%\termide\config.toml`
- 会话数据: `%APPDATA%\termide\sessions\`
- 日志文件: `%LOCALAPPDATA%\termide\cache\termide.log`

### Windows (WSL)

TermIDE 也可在 Windows Subsystem for Linux（WSL 和 WSL2）中运行：

1. 如果尚未安装，请先安装 WSL2
2. 在 WSL 中下载 Linux x86_64 二进制文件：
   ```bash
   wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-x86_64-unknown-linux-gnu.tar.gz
   tar xzf termide-0.31.0-x86_64-unknown-linux-gnu.tar.gz
   ./termide
   ```

### Android (Termux)

TermIDE 可在 [Termux](https://termux.dev) 中运行。请使用**静态 ARM64 musl**
构建 —— glibc 的 `aarch64-unknown-linux-gnu` 构建无法在 Android 的 Bionic libc 上运行：

```bash
pkg install git openssh   # termide 会调用的工具（以及所需的 LSP 服务器）
wget https://github.com/termide/termide/releases/latest/download/termide-0.31.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.31.0-aarch64-unknown-linux-musl.tar.gz
./termide
```

Android 上没有系统剪贴板（无 X11/Wayland），资源监视器可能因 `/proc` 受限而显示不完整；
编辑器、文件管理器、git 和内置终端均可正常使用。

## 验证安装

安装完成后，验证是否正常工作：

```bash
termide --version
```

## 下一步

- 阅读[用户界面指南](ui.md)了解应用程序布局
- 了解[文件管理器](file-manager.md)键盘快捷键
- 探索[终端](terminal.md)和[编辑器](editor.md)功能
- 使用[主题](themes.md)自定义您的体验
