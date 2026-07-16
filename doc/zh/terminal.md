# 终端

终端面板提供功能完整的终端模拟器，支持伪终端（PTY），确保与大多数控制台应用程序的兼容性，如 `bash`、`fish`、`htop` 和 `mc`。

## 主要功能

- **交互式 Shell**：启动系统默认 shell（`fish`、`zsh`、`bash` 等）执行命令
- **兼容性**：支持 `xterm-256color` 和大多数标准 ANSI 控制序列，确保正确显示颜色和文本样式
- **进程管理**：关闭有运行中进程的终端面板时，应用程序会在终止进程前请求确认

## 交互操作

| 快捷键 | 操作 |
|------------------------|--------------------------------------------|
| `Ctrl+/`               | 打开目录切换器                            |
| `Ctrl+F`               | 打开滚动缓冲区文本搜索                    |
| `Ctrl+C`               | **有选中内容时**复制到剪贴板；否则作为 `SIGINT` 发送给 shell |
| `Ctrl+V`               | 从剪贴板粘贴文本                           |
| `Shift+Enter`          | 插入换行（多行输入）                       |
| `Shift+PageUp`         | 向上滚动输出历史                           |
| `Shift+PageDown`       | 向下滚动输出历史                           |
| `Shift+Home`           | 转到输出历史的开头                         |
| `Shift+End`            | 转到当前行（历史末尾）                     |

**键盘布局支持：**

TermIDE 支持西里尔文键盘布局的常用快捷键。使用俄语/西里尔文布局时，粘贴（`Ctrl+V`）无需切换到拉丁布局即可使用——按下同一物理按键上的西里尔字母会被自动识别。

所有其他组合键直接传递给终端中运行的应用程序。

**带修饰键的方向键与 Home/End** 按标准 xterm CSI 序列 `1;{mod}{final}`
进行编码（`{mod}` 是 xterm 修饰键参数：`2` = Shift、`3` = Alt、`5` = Ctrl、
`6` = Ctrl+Shift 等；`{final}` 为 `A`/`B`/`C`/`D`/`H`/`F`）。这样
`Ctrl+Left` / `Ctrl+Right` 在 bash/zsh readline 中触发 `backward-word` /
`forward-word`；`Shift+Home` / `Shift+End` 在支持的 shell 中选中到行
首/行尾。未带修饰的方向键保持原路径，包括 application-cursor-mode 切换
（`\x1bOA` 与 `\x1b[A`）。`Alt+Left` / `Alt+Right` 仍然被全局面板组切换
快捷键占用，不会转发给终端。

## 文本搜索

按 `Ctrl+F` 打开停靠在面板顶部的内嵌搜索栏（与编辑器、文件管理器一致），其下带
分隔线。搜索功能覆盖整个滚动缓冲区和可见屏幕：

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

## 鼠标支持

- **文本选择**：在普通 shell/滚动缓冲区视图中，按住鼠标左键拖动以选择文本，然后用 `Ctrl+C` 复制。当终端内的应用程序（例如编辑器或智能体）启用 xterm 鼠标跟踪时，鼠标归该应用程序所有（由它绘制自己的选区）；此时改用 `Alt+拖动` 进行 TermIDE 本地选择，再用 `Ctrl+C` 复制
- **双击**：选中光标下的单词；**三击**：选中整行
- **滚轮**：滚动终端输出历史
- **Ctrl+点击 URL/路径**：在浏览器或文件管理器中打开链接
- **Ctrl+点击十六进制颜色**：显示颜色预览弹窗（如 `#ff0000`、`#abc`）——按住按钮时可见，松开时消失
- **应用交互**：如果控制台应用程序（如 `htop` 或 `mc`）支持鼠标输入，终端会将鼠标事件传递给它
