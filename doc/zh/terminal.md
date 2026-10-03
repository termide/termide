# 终端

终端面板提供功能完整的终端模拟器，支持伪终端（PTY），确保与大多数控制台应用程序的兼容性，如 `bash`、`fish`、`htop` 和 `mc`。

## 主要功能

- **交互式 Shell**：启动系统默认 shell（`fish`、`zsh`、`bash` 等）执行命令
- **兼容性**：支持 `xterm-256color` 和大多数标准 ANSI 控制序列，确保正确显示颜色和文本样式
- **宽字符与组合字符**：CJK 文本、emoji 及 emoji 序列（肤色、ZWJ）占用宿主终端分配给它们的两列，组合重音附着在其前一个字符上而不占用单元格。变体选择符是否会加宽符号（`⏱️`）在启动时向宿主终端探测，因为各终端对此并不一致。全屏应用和 `pi` 这类编码代理能保持布局、擦除正确的行，并把光标放在预期位置
- **现代 TUI 兼容性**：响应常见的终端能力查询，并支持 `vim`、`neovim`、`yazi`、`htop`、`lazygit` 等应用程序协商使用的键盘/焦点上报
- **进程管理**：关闭有运行中进程的终端面板时，应用程序会在终止进程前请求确认
- **响铃**：程序发出的响铃会传到您的终端，未获得焦点的终端面板标题会以警告色显示，直到您将其聚焦。在后台打开的项目中，响铃会在 `项目` 菜单中以 🔔 标记该项目
- **面板标题**：显示 `user@host/<directory> (<foreground command>)`。目录从运行中的 shell 读取，因此会跟随面板内的 `cd`；以固定命令启动的面板（例如 SSH 会话）则以该命令作为标题
- **实时工作目录**：面板向应用其余部分报告的目录同样跟随 shell 内的 `cd`——目录切换器、在"此处"打开新面板，以及 git 面板的仓库列表都使用您实际所在的目录。Windows 上请参阅 [Windows 上的工作目录](#windows-上的工作目录)
- **布局恢复**：恢复的终端会在 shell 最后所在的目录中打开，而不是面板最初创建时的目录。如果该目录已不存在，则使用最近的仍然存在的父目录

## 交互操作

| 快捷键 | 操作 |
|------------------------|--------------------------------------------|
| `Ctrl+\`               | 打开目录切换器                            |
| `Ctrl+F`               | 打开滚动缓冲区文本搜索                    |
| `Ctrl+C`               | **有选中内容时**复制到剪贴板；否则作为 `SIGINT` 发送给 shell |
| `Ctrl+V`               | 从剪贴板粘贴文本                           |
| `Shift+Enter`          | 插入换行（多行输入）                       |
| `Shift+PageUp`         | 向上滚动输出历史                           |
| `Shift+PageDown`       | 向下滚动输出历史                           |
| `Shift+Home`           | 转到输出历史的开头                         |
| `Shift+End`            | 转到当前行（历史末尾）                     |

**键盘布局支持：**

TermIDE 支持西里尔文键盘布局的常用快捷键。使用俄语/西里尔文布局时，粘贴（`Ctrl+V`）无需切换到拉丁布局即可使用——按下同一物理按键上的西里尔字母会被自动识别。发送给程序的控制组合键同样如此：`Ctrl+С` 像 `Ctrl+C` 一样中断程序，`Ctrl+В` 像 `Ctrl+D` 一样发送文件结束符。

所有其他组合键直接传递给终端中运行的应用程序。

当终端内的应用程序请求现代键盘上报时，TermIDE 会从传统的 xterm 风格按键编码切换到协商的兼容模式。这有助于应用程序区分含义模糊的组合，例如 `Ctrl+I` 与 `Tab`、`Ctrl+M` 与 `Enter`，以及带修饰键的 `Esc`/`Backspace`。

**带修饰键的方向键与 Home/End** 按标准 xterm CSI 序列 `1;{mod}{final}`
进行编码（`{mod}` 是 xterm 修饰键参数：`2` = Shift、`3` = Alt、`5` = Ctrl、
`6` = Ctrl+Shift 等；`{final}` 为 `A`/`B`/`C`/`D`/`H`/`F`）。这样
`Ctrl+Left` / `Ctrl+Right` 在 bash/zsh readline 中触发 `backward-word` /
`forward-word`；`Shift+Home` / `Shift+End` 在支持的 shell 中选中到行
首/行尾。未带修饰的方向键保持原路径，包括 application-cursor-mode 切换
（`\x1bOA` 与 `\x1b[A`）。`Alt+Left` / `Alt+Right` 仍然被全局面板组切换
快捷键占用，不会转发给终端。

如果 shell 没有为此类序列绑定功能，就会输出序列的最后一个字母：bash 5 和
zsh 对 `Shift+Up` 输出 `A`，对 `Shift+Left` 输出 `D`，以此类推。任何兼容
xterm 的终端都是如此。对于 bash，可在 `~/.inputrc` 中绑定这些按键，例如：

```
"\e[1;2A": previous-history
"\e[1;2B": next-history
"\e[1;2C": forward-char
"\e[1;2D": backward-char
```

## 文本搜索

按 `Ctrl+F` 打开停靠在面板底部的内嵌搜索栏（与编辑器、文件管理器一致），其上带
标题边框（`─ Search ─`）将其与上方网格隔开。搜索功能覆盖整个滚动缓冲区和可见屏幕：

- **实时预览**：输入时高亮显示匹配项；搜索栏显示匹配计数（例如"3/12"）
- **开关**：`[Aa]` 区分大小写、`[.*]` 正则表达式（点击，或将焦点移到按钮行后按
  `Enter` / `Space`）
- **导航**：`◄ Prev` / `Next ►` 按钮、`Enter` 或 `F3` / `Shift+F3` 在匹配项间
  跳转；视口自动滚动到当前匹配项
- **焦点**：`Tab` 在搜索栏与终端网格之间切换焦点，搜索栏打开时仍可滚动网格
- **刷新**：`Ctrl+R` 针对当前滚动缓冲区重新运行查询
- **关闭**：`Escape`

搜索快捷键默认为 `Ctrl+F`（而非 `Ctrl+Shift+F`），因为大多数宿主终端会拦截 `Ctrl+Shift+F` 用于自身搜索。

## Shell 选择

您可以通过 **Windows > Terminal** 子菜单选择要启动的 shell。子菜单列出系统中检测到的所有 shell：

- **Linux/macOS**：来自 `/etc/shells` 的 shell，以及常见路径（`/usr/bin/fish`、`/usr/bin/zsh`、`/bin/bash`、`/bin/sh`）和 NixOS 特定路径
- **Windows**：Git Bash、PowerShell Core（`pwsh`）、Windows PowerShell、命令提示符（`cmd`）和 WSL 发行版

当前配置的默认 shell 以 **●** 标记。选择某个 shell 会打开一个使用该 shell 的新终端，并将其保存为未来终端的默认 shell。

也可以在 `config.toml` 中设置默认 shell：

```toml
[terminal]
default_shell = "/usr/bin/fish"
```

### Windows 上的工作目录

Windows 没有让一个程序得知另一个程序工作目录的调用，因此终端通过以下两种方式之一获知 shell 的目录：

- **从 shell 进程获取**——命令提示符和 Git Bash 会保持其更新，因此 `cd`（包括 `D:` 或 `cd /d D:\work` 这样的驱动器切换）无需任何设置即可被跟随
- **由 shell 自己告知**——PowerShell 的 `Set-Location` 不会更新进程，因此 PowerShell 必须用 `OSC 9;9` 或 `OSC 7` 转义序列告知每个目录。Windows Terminal 要求同样的设置，所以已为它准备好的配置文件在这里同样可用。否则，请将以下内容添加到 `$PROFILE`：

```powershell
function prompt {
  $loc = $executionContext.SessionState.Path.CurrentLocation
  $out = ""
  if ($loc.Provider.Name -eq "FileSystem") {
    $out += "$([char]27)]9;9;`"$($loc.ProviderPath)`"$([char]27)\"
  }
  $out += "PS $loc$('>' * ($nestedPromptLevel + 1)) "
  return $out
}
```

使用 oh-my-posh 时，请改为在其配置中设置 `"pwd": "osc99"`。

当 shell 告知了目录时，以该告知为准，而非进程信息。

## 鼠标支持

- **文本选择**：在普通 shell/滚动缓冲区视图中，按住鼠标左键拖动以选择文本，然后用 `Ctrl+C` 复制。当终端内的应用程序（例如编辑器或智能体）启用 xterm 鼠标跟踪时，鼠标归该应用程序所有（由它绘制自己的选区）；此时改用 `Alt+拖动` 进行 TermIDE 本地选择，再用 `Ctrl+C` 复制
- **双击**：选中光标下的单词；**三击**：选中整行
- **滚轮**：滚动终端输出历史，直到终端内的应用程序启用鼠标跟踪；此后滚轮事件会传递给该应用程序
- **Ctrl+点击 URL/路径**：在浏览器或文件管理器中打开链接
- **Ctrl+点击十六进制颜色**：显示颜色预览弹窗（如 `#ff0000`、`#abc`）——按住按钮时可见，松开时消失
- **应用交互**：如果控制台应用程序（如 `htop` 或 `mc`）启用了 xterm 鼠标跟踪，TermIDE 会在终端内容区域内优先将点击、拖动、移动和滚轮事件交给它

## 应用兼容性说明

- **键盘协商**：TermIDE 会响应现代 TUI 应用常用的键盘能力查询，并在应用请求时支持协商的 `CSI u` / `modifyOtherKeys` 兼容模式
- **焦点上报**：如果应用启用了 xterm 焦点事件（`?1004`），TermIDE 会将宿主终端的焦点获得/失去转发为 `CSI I` / `CSI O`
- **终端标识**：内部 PTY 保持常规的 `xterm-256color` 基线，通过运行时协商而非伪装成另一种终端来增加兼容性
