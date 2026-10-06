# 自定义命令

> **注意：** 本文介绍 `命令` 菜单中的**用户自定义命令**。键盘快捷键与界面操作请参阅 [ui.md](ui.md#键盘导航和面板管理)。

命令是一条具名的 shell 命令行，TermIDE 可以从 `命令` 菜单或通过它自己的快捷键运行它：在新的终端面板中运行、在后台静默运行，或在后台运行并在结束时于窗口中显示其输出。命令定义在 `commands.toml` 文件中，可以手动编写，也可以通过 `添加命令...` 表单创建。

## 命令存放位置

有两个文件，菜单会同时显示两者的命令：

| 范围 | 文件 |
|------|------|
| 全局（所有项目） | TermIDE 配置目录中的 `commands.toml` |
| 项目 | `<项目>/.termide/commands.toml` |

配置目录因平台而异：

| 平台 | 全局文件 |
|------|----------|
| Linux | `~/.config/termide/commands.toml`（或 `$XDG_CONFIG_HOME/termide/commands.toml`） |
| macOS | `~/Library/Application Support/termide/commands.toml` |
| Windows | `%APPDATA%\termide\commands.toml` |

项目文件属于当前项目根目录，因此可以提交到仓库，与所有参与该仓库的人共享。

两个文件互不覆盖：标识符相同的项目命令和全局命令是两个独立的菜单项。项目命令排在前面，并以粗体显示。

## 文件格式

每个 TOML 表就是一条命令；表名即命令的标识符。

```toml
[build]
command = "cargo build --release"

[test]
name = "Run tests"
command = "cargo nextest run"
group = "cargo"
key = "Ctrl+Shift+T"

[clippy]
command = "cargo clippy --workspace -- -D warnings"
mode = "report"
group = "cargo"

[dev-server]
name = "Start dev server"
command = "npm run dev > /tmp/dev-server.log 2>&1"
mode = "background"
```

| 字段 | 必填 | 说明 |
|------|------|------|
| `command` | 是 | 要运行的 shell 命令行 |
| `name` | 否 | 菜单中的标签；缺省时显示标识符 |
| `mode` | 否 | `terminal`（默认）、`background` 或 `report`，参见[执行模式](#执行模式) |
| `group` | 否 | 将命令放入以此命名的子菜单 |
| `key` | 否 | 运行该命令的快捷键，例如 `Ctrl+Shift+D`，参见[快捷键](#快捷键) |
| `params` | 否 | 运行前需要询问的值，参见[参数](#参数) |

未知的 `mode` 会回退为 `terminal`。如果某个文件无法解析，其中的命令都不会出现；原因会写入日志。

## 命令菜单

菜单栏中的 `命令` 菜单自上而下列出：

1. `添加命令...` — 打开新建命令的表单。
2. 没有分组的项目命令，然后是项目分组（粗体）。
3. 没有分组的全局命令，然后是全局分组。

在每一部分中，命令和分组分别按标识符和分组名排序。分组会打开一个包含其命令的子菜单；再次点击分组标题会将其关闭。命令的快捷键显示在快捷键列中。当终端支持 emoji 时，每个标签前会加上表示其模式的图标：💻 terminal、⚙ background、📋 report。

选中命令时可用的按键：

| 按键 | 操作 |
|------|------|
| `Enter` | 运行命令 |
| `F2` | 重命名（修改标识符） |
| `F4` | 在命令表单中编辑 |
| `Delete` / `F8` | 确认后删除 |

菜单每次打开时都会重新读取两个文件，因此手动添加到文件中的命令无需重启即可显示。

## 添加与编辑命令

`添加命令...` 和 `F4` 打开同一个表单：

| 字段 | 含义 |
|------|------|
| `Group:` | 分组名；会提示已有的分组。留空表示菜单顶层 |
| `Menu item:` | 菜单标签（`name`） |
| `命令:` | shell 命令行（`command`）；创建时必填 |
| `Mode:` | `Terminal`、`Background` 或 `Report`；用 `←` / `→` 或 `1`–`3` 切换 |
| `Hotkey:` | 可选的快捷键（`key`） |
| `Project command` | 勾选：命令保存到项目的 `.termide/commands.toml`；不勾选：保存到全局文件 |

新命令的标识符由 `Menu item:` 生成，标签为空时则由 `命令:` 生成；字符 `/ \ : * ? " < > | .` 会被替换为 `-`。如果已存在同一标识符的命令，新命令会使用一个空闲的变体（`build-2`），而不会替换它。编辑时清空 `Menu item:` 会删除标签，菜单随即显示标识符。编辑时切换 `Project command` 会把命令移到另一个文件。表单不编辑 `params`；它们会按文件中的原样保留。重命名（`F2`）为另一个命令的标识符会被拒绝。

通过表单保存、重命名或删除会就地编辑 `commands.toml`：其他命令、它们的顺序以及周围的注释保持不变，只重写有变化的字段，新命令添加到末尾。重命名的命令会移到文件末尾。

## 执行模式

每条命令都在聚焦面板的工作目录中运行（例如文件管理器中显示的目录，或终端的当前目录）；如果该面板没有工作目录，则在项目根目录中运行。

| 模式 | 运行方式 | 输出 |
|------|----------|------|
| `terminal` | 打开新的终端面板，并将命令行输入到其 shell 中 | 显示在终端中；命令结束后 shell 保持打开 |
| `background` | 不打开面板，执行 `sh -c "<command>"` | 丢弃 |
| `report` | 不打开面板，执行 `sh -c "<command>"` | 捕获，并在命令结束时显示在窗口中 |

后台命令和报告命令会出现在[操作](operations.md)面板中，该面板会在此类命令启动时打开。进程退出后条目随之消失；在那里取消会连同其子进程一起终止该进程。后台命令在完成时不会给出其他信号，因此如果需要其输出，请将其重定向到文件。在 Unix 上，如果安装了 `direnv`，后台命令和报告命令还会获得 `direnv export json` 为工作目录返回的环境变量。

### 报告窗口

报告命令结束后，会弹出一个窗口，标题为命令的标签加上 `✓`（退出码 0）或 `✗`（其他退出码），显示其输出：先是标准输出，然后是标准错误。输出中的缩进和内部空行会保留，制表符展开为四个空格，每个流前后的空行会被丢弃；没有输出的命令显示 `(no output)`。窗口在当前所在的项目中打开；在另一个项目中启动的命令会在标题中加上该项目的路径。如果多个报告命令同时结束，只显示最后一个的窗口。

| 按键 | 操作 |
|------|------|
| `↑` / `↓`（`k` / `j`） | 滚动一行 |
| `PageUp` / `PageDown` | 滚动一页 |
| `Home` / `End` | 跳到开头 / 结尾 |
| 鼠标滚轮 | 滚动 |
| `Enter` / `Escape` | 关闭窗口 |

## 参数

命令可以声明参数。运行前，TermIDE 会显示标题为 `Parameters: <id>` 的表单，每个参数一个字段，并带有 `Run` 和 `Cancel` 按钮。每个值都以环境变量 `TERMIDE_PARAM_<NAME>` 的形式传给命令：名称转为大写，`-` 替换为 `_`。

```toml
[deploy]
name = "Deploy"
command = "./deploy.sh \"$TERMIDE_PARAM_TARGET\" \"$TERMIDE_PARAM_DRY_RUN\""
mode = "report"
key = "Ctrl+Shift+D"

[[deploy.params]]
name = "target"
label = "Target environment"
type = "select"
options = ["staging", "production"]
default = "staging"

[[deploy.params]]
name = "dry-run"
label = "Dry run"
type = "bool"
default = true
```

| 字段 | 说明 |
|------|------|
| `name` | 必填；对应的变量为 `TERMIDE_PARAM_<NAME>` |
| `label` | 表单中的字段标签；默认为 `name` |
| `type` | `text`（默认）、`number`、`bool` 或 `select` |
| `options` | `select` 的可选项 |
| `default` | 初始值：字符串、数字或布尔值 |

| 类型 | 字段 | 传递的值 |
|------|------|----------|
| `text` | 文本输入框 | 输入的文本 |
| `number` | 文本输入框 | 输入的文本；不会检查是否为数字 |
| `bool` | 复选框，用 `Space` / `Enter` 或点击切换 | `true` 或 `false` |
| `select` | 用 `←` / `→` 切换的选项；初始为 `default`，否则为第一个选项 | 选中的选项 |

无论命令以何种方式启动——从 `命令` 菜单、通过快捷键或从命令面板——都会显示该表单。变量在所有模式下都会传给命令；在 `terminal` 模式下，它们设置在新终端的 shell 中，命令结束后仍然保留。

## 快捷键

`key` 使用与 `config.toml` 中键绑定相同的写法（参见 [keybindings.md](keybindings.md)），例如 `Ctrl+Shift+D` 或 `Alt+F5`。`Ctrl+Shift+<字母>` 只有在支持 Kitty 键盘协议的终端中才能到达 TermIDE（参见[通用层 vs 增强层](keybindings.md#通用层-vs-增强层)）。命令快捷键与 TermIDE 全局快捷键一起检查，因此无论哪个面板拥有焦点都能生效。标识符相同的项目命令和全局命令各自保留独立的快捷键。

带快捷键的命令也会以 `Run command: <label>` 的形式列在命令面板（`Ctrl+P`）中。

表单的 `Hotkey:` 字段接受 `Ctrl`、`Alt` 和 `Shift` 与字母、数字、`F1`–`F12` 或具名按键（`Enter`、`Tab`、`Space`、`Home`、`PageUp`、方向键等）的组合，并拒绝已被其他命令或 TermIDE 全局快捷键占用的快捷键（`Hotkey is already in use`）。手动写入文件的快捷键不会被检查：如果它与全局快捷键冲突，以全局快捷键为准，该命令将永远无法通过键盘运行。它会在下次打开 `命令` 菜单或从 TermIDE 保存该文件后生效。

## 提示

- 命令通过 `sh -c`（或终端的 shell）运行，因此管道、`&&`、重定向和环境变量都与在 shell 中一样可用。在 Windows 上，`background` 和 `report` 命令需要 `PATH` 中有 `sh`。
- 对想要查看结果的简短检查（`git status`、代码检查工具）使用 `report`，对不需要其输出的进程使用 `background`，对任何交互式或需要观察的长时间运行任务使用 `terminal`。
- 将项目专用的命令保存在仓库的 `.termide/commands.toml` 中，个人命令保存在全局文件中。
