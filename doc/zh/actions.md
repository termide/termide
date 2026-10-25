# 自定义命令

> **注意：** 本文介绍 `命令` 菜单中的**用户自定义命令**。键盘快捷键与界面操作请参阅 [ui.md](ui.md#键盘导航和面板管理)。

命令是一条具名的 shell 命令行，Termide 可以从 `命令` 菜单或通过它自己的快捷键运行它：在新的终端面板中运行、在后台静默运行，或在后台运行并在结束时于窗口中显示其输出。命令定义在 `commands.toml` 文件中，可以手动编写，也可以通过 `添加命令...` 表单创建。

## 命令存放位置

有两个文件，菜单会同时显示两者的命令：

| 范围 | 文件 |
|------|------|
| 全局（所有项目） | Termide 配置目录中的 `commands.toml` |
| 项目 | `<项目>/.termide/commands.toml` |

配置目录因平台而异：

| 平台 | 全局文件 |
|------|----------|
| Linux | `~/.config/termide/commands.toml`（或 `$XDG_CONFIG_HOME/termide/commands.toml`） |
| macOS | `~/Library/Application Support/termide/commands.toml` |
| Windows | `%APPDATA%\termide\commands.toml` |

项目文件属于当前项目根目录，因此可以提交到仓库，与所有参与该仓库的人共享。

两个文件互不覆盖：标识符相同的项目命令和全局命令是两个独立的菜单项。项目命令排在前面，并以粗体显示。

由于项目文件随仓库而来，项目命令只有在你看过之后才会运行。第一次启动某个项目命令时——无论通过菜单、命令面板、其快捷键还是 `F5`——会弹出一个窗口，显示它要运行的内容（先是其 `build`，然后是命令本身）并询问你；同意后即运行，并按其当前内容批准它，批准记录保存在 termide 数据目录中该项目名下（`trust/<项目路径>/approved.toml`）。如果仓库更改了该命令、其模式、`build`、调试器或快捷键，会再次询问。项目命令获批准之前，只有带 `Ctrl` 或 `Alt` 的组合键或 F 键会询问：按下它会弹出与菜单相同的窗口。用于输入文本的键——普通字母、`Shift`+字母、`Enter`——只在命令获批准后才会绑定：从菜单启动一次，此后该键即生效。你在 `Add command...` 或 **设置调试** 表单中编写的命令在保存时即获批准。

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
| `debugger` | 否 | `lldb-dap`、`gdb-dap`、`debugpy`、`dlv-dap`、`lldb`、`gdb`、`dlv` 或 `pdb`：命令在该调试器下运行，参见[调试](#调试) |
| `build` | 否 | 与 `debugger` 一起使用：先运行的 shell 命令；只有它成功后调试器才会启动 |
| `console` | 否 | 使用调试适配器时：`terminal`（默认）在终端面板中运行程序，程序在那里读取键盘输入；`panel` 在调试面板中显示其输出 |

未知的 `mode` 会回退为 `terminal`。如果某个文件无法解析，其中的命令都不会出现；原因会写入日志。

## 命令菜单

菜单栏中的 `命令` 菜单自上而下列出：

1. `添加命令...` — 打开新建命令的表单。
2. 没有分组的项目命令，然后是项目分组（粗体）。
3. 没有分组的全局命令，然后是全局分组。

在每一部分中，命令和分组分别按标识符和分组名排序。分组会打开一个包含其命令的子菜单；再次点击分组标题会将其关闭。命令的快捷键显示在快捷键列中。当终端支持 emoji 时，每个标签前会加上表示其模式的图标：💻 terminal、⚙ background、📋 report，🐞 表示带 `debugger` 的命令。

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

带 `debugger` 的命令无论其 `mode` 如何，都在项目根目录中运行，显示在调试面板或终端面板中，参见[调试](#调试)。其他每条命令都在聚焦面板的工作目录中运行（例如文件管理器中显示的目录，或终端的当前目录）；如果该面板没有工作目录，则在项目根目录中运行。

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

命令可以声明参数。运行前，Termide 会显示标题为 `Parameters: <id>` 的表单，每个参数一个字段，并带有 `Run` 和 `Cancel` 按钮。每个值都会替换命令行中对应的[占位符](#占位符) `{name}`，同时以环境变量 `TERMIDE_PARAM_<NAME>` 的形式传给命令：名称转为大写，`-` 替换为 `_`。

```toml
[deploy]
name = "Deploy"
command = "./deploy.sh {target} {dry-run}"
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
| `name` | 必填；对应的占位符为 `{name}`，变量为 `TERMIDE_PARAM_<NAME>` |
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

## 占位符

`command`（以及 `build`）中的 `{name}` 会在命令运行前被替换：替换为参数 `name` 的值，或替换为当前聚焦面板或项目提供的值。同名参数优先。

| 占位符 | 值 |
|--------|----|
| `{file}` | 编辑器中的文件；在文件管理器中为光标所在的条目 |
| `{files}` | 文件管理器中选中的文件，每个为一个独立的词；未选中时为光标所在的条目；在编辑器中为其文件 |
| `{line}`、`{col}` | 编辑器光标位置，从 1 开始 |
| `{dir}` | 当前聚焦面板的工作目录 |
| `{project}` | 项目根目录 |

路径为绝对路径。没有可用的文件时（例如聚焦的是终端，或文件位于远程服务器上），使用 `{file}`、`{files}`、`{line}` 或 `{col}` 的命令不会运行，并会显示提示；提示会在参数表单打开之前出现。

```toml
[p4-edit]
name = "p4 edit"
command = "p4 edit {file}"
key = "Alt+E"
```

每个值都会按读取该命令行的程序进行引用，因此包含空格、引号或 `;` 的路径始终是一个参数，绝不会被当作命令执行：

- `background` 和 `report` 命令通过 `sh -c` 运行：使用 POSIX 引用。
- `terminal` 命令按终端的 shell 引用：sh、bash、zsh 等使用 POSIX，fish 使用其自身的规则，PowerShell 使用 `'…'`，cmd 使用 `"…"`。cmd 没有字面引用，因此值中的 `%name%` 在那里仍会被展开。
- 调试命令的命令行先拆分为词，再把值放入其中，每个值为一个词（`{files}` 为多个）；其 `build` 使用 POSIX 引用。

占位符可以写在任何位置：作为独立的词（`p4 edit {file}`）、写在引号中（`"{file}"`），或位于更长的引号字符串中（`"at {file}"`、`'at {file}'`）。值会按其所在位置进行引用，因此在任何情况下都按字面理解：名为 `$(rm -rf ~).c` 的文件只是一个名称。引号中的 `{files}` 会成为一个词，与引号中的其他内容一样。为 shell 转义过的占位符（sh 中的 `\{file}`、PowerShell 中的 `` `{file} ``、cmd 中的 `^{file}`）保持原样。只有上面列出的名称和命令的参数会被替换：其他花括号保持原样，因此 `awk '{print $1}'`、`docker inspect -f '{{.Name}}'`、花括号展开和 `${VAR}` 都不受影响。

## 快捷键

`key` 使用与 `config.toml` 中键绑定相同的写法（参见 [keybindings.md](keybindings.md)），例如 `Ctrl+Shift+D` 或 `Alt+F6`。`Ctrl+Shift+<字母>` 只有在支持 Kitty 键盘协议的终端中才能到达 Termide（参见[通用层 vs 增强层](keybindings.md#通用层-vs-增强层)）。命令快捷键与 Termide 全局快捷键一起检查，因此无论哪个面板拥有焦点都能生效。标识符相同的项目命令和全局命令各自保留独立的快捷键。

带快捷键的命令也会以 `Run command: <label>` 的形式列在命令面板（`Ctrl+P`）中。

表单的 `Hotkey:` 字段接受 `Ctrl`、`Alt` 和 `Shift` 与字母、数字、`F1`–`F12` 或具名按键（`Enter`、`Tab`、`Space`、`Home`、`PageUp`、方向键等）的组合，并拒绝已被其他命令或 Termide 全局快捷键占用的快捷键（`Hotkey is already in use`）。手动写入文件的快捷键不会被检查：如果它与全局快捷键冲突，以全局快捷键为准，该命令将永远无法通过键盘运行。它会在下次打开 `命令` 菜单或从 Termide 保存该文件后生效。

## 调试

带 `debugger` 的命令会调试一个程序：设置好在编辑器中放置的断点（参见[断点](editor.md#断点与调试)），并将程序运行到第一个断点。此时 `command` 就是要调试的对象。支持两类调试器：

- **调试适配器**，由 Termide 通过 Debug Adapter Protocol 驱动，显示在[调试面板](#调试面板)中：`lldb-dap`、`gdb-dap`、`debugpy`、`dlv-dap`；
- **命令行调试器**，在新的终端面板中启动，在其中用调试器自己的命令单步、打印和继续：`lldb`、`gdb`、`dlv`、`pdb`。

| `debugger` | `command` 是 | 运行方式 |
|------------|--------------|----------|
| `lldb-dap` | 程序及其参数 | LLVM 的 `lldb-dap`（在 macOS 上也会通过 `xcrun` 查找；旧版 LLVM 中为 `lldb-vscode`）：Rust、C、C++ |
| `gdb-dap` | 程序及其参数 | `gdb -i dap`，需要 GDB 14 或更高版本 |
| `debugpy` | 脚本及其参数 | `python3 -m debugpy.adapter`；需要 `pip install debugpy` |
| `dlv-dap` | 要构建的包及其参数，例如 `./cmd/app --flag` | `dlv dap`，即 Delve 的适配器，通过本地 TCP 端口连接；在无法读取 `/proc/net/tcp` 的环境（Android 上的 Termux）中会加上 `--only-same-user=false`，因为 Delve 的同用户检查在那里会拒绝所有连接 |
| `lldb` | 程序及其参数 | `lldb -s <script> -- <command>` |
| `gdb` | 程序及其参数 | `gdb -q -x <script> --args <command>` |
| `dlv` | Delve 的子命令及其参数，例如 `debug ./cmd/app` | `dlv --init <script> <command>` |
| `pdb` | 脚本及其参数 | `python3 -m pdb -c 'break <file>:<line>' … -c continue <command>` |

```toml
[debug-app]
name = "Debug app"
command = "target/debug/app --verbose"
debugger = "lldb-dap"
build = "cargo build"
key = "Ctrl+Alt+D"
```

`debugger_args` 以单词数组的形式给出调试器自身的选项，追加到 Termide 启动它时使用的选项之后：

```toml
[debug-go]
command = "./cmd/app"
debugger = "dlv-dap"
debugger_args = ["--log", "--log-output=dap"]
```

它们放在调试器读取其选项的位置，即程序之前：适配器在其自身参数之后获得它们（`dlv dap --listen=… --log …`、`gdb -i dap …`），`lldb` 和 `gdb` 在 `--` 和 `--args` 之前，`dlv` 在其子命令之前，`pdb` 在其 `-c` 之前。程序的参数仍写在 `command` 中。

对于调试适配器，`command` 会像 shell 那样拆分成单词（引号和反斜杠有效），但并不由 shell 运行：变量、`~` 和通配符不会展开。`build` 先运行，其输出进入调试面板；只有它成功后适配器才会启动。命令行调试器从脚本中读取断点——每个断点一条 `breakpoint set` 或 `break`，然后是 `run`（lldb、gdb）或 `continue`（Delve）——该脚本写入系统临时目录下的 `termide-debug`；调试器运行期间设置的断点只有在下次启动时才会传给它，因此在此期间请直接在调试器中设置断点。

只有项目根目录下文件中的断点才会传入，每个断点交给对应其语言的调试器：Python 文件交给 debugpy 和 pdb，Go 文件交给 Delve 和 dlv-dap，其余交给 lldb 和 gdb。调试命令在项目根目录中运行，因此像 `target/debug/app` 这样的相对程序路径无论焦点在哪里含义都相同。参数的用法与其他命令相同；对于调试适配器，它们作为环境变量传给构建命令和适配器。

在编辑器中按 `F5`（`start_debug`），或在命令面板中选择 **开始调试**，即可开始调试：只有一个启动项时立即启动，有多个时以列表形式供选择。启动项包括项目文件和全局文件中的调试命令；当聚焦的编辑器中是 Python、Go 或 Rust 文件时，还包括该文件本身，它不需要任何命令：

- Python 脚本在安装了 debugpy 时于调试面板中在 debugpy 下运行，否则在终端中在 pdb 下运行；
- Go 文件所在目录的包在安装了 Delve 时于调试面板中在 `dlv dap` 下运行，否则在终端中在 `dlv debug` 下运行；
- Cargo 项目中的 Rust 文件会提供通过 `cargo metadata` 找到的二进制文件：以它为主文件的那个二进制，否则为其所在包的二进制，再否则为工作区中的所有二进制。每个二进制都用 `cargo build -p <package> --bin <name>` 构建，并在工作区根目录中运行，所用调试器为 `lldb-dap`、GDB 的适配器或终端中的 `lldb`，取最先安装的那个。

当文件可以调试但缺少相应的调试器（Rust 没有 LLDB 或 GDB，Go 没有 Delve）时，会弹出窗口说明需要安装什么。当完全没有可启动的对象时，则会打开 **设置调试** 表单（见下文）。调试会话处于活动状态时，`F5` 改为继续运行程序。

**设置调试…** 位于编辑器的 `[≡]` 菜单和命令面板中，它打开一个用于自定义启动项的表单：名称、程序及其参数、先执行的构建命令、调试器及其选项（像 shell 那样拆分），并根据编辑器中的文件预先填好——对于 Rust 二进制，是它在 `target/debug` 下的路径和对应的 `cargo build`。**Run** 会将其作为调试命令保存到项目的 `.termide/commands.toml` 中并启动；此后 `F5` 会提供它，也可以在该文件或命令菜单中编辑它，例如添加参数。

### 调试器设置

各个调试器如何启动在 `config.toml` 中设置，与调试的对象分开。项目的 `.termide/config.toml` 可以设置 `native` 和 `console`；调试器的 `command` 和 `args` 只取自全局文件，因为它们指定 termide 启动的程序：

```toml
[debug]
native = ["lldb-dap", "gdb-dap", "lldb"]  # 没有命令时用于 Rust、C、C++：取第一个已安装的
console = "terminal"                      # 或 "panel"：适配器默认在何处运行程序

[debug.debuggers.lldb-dap]
command = "/opt/homebrew/opt/llvm/bin/lldb-dap"  # 代替 PATH（或 xcrun）中的那个

[debug.debuggers.dlv-dap]
args = ["--log"]                                 # 每次启动都使用的选项
```

- `command` 指定调试器的程序：路径（`~/` 表示主目录），或在 `PATH` 中查找的名称。对于 `debugpy` 和 `pdb`，它是要运行的 Python。不存在的路径会使该调试器被视为未安装，而不会回退到另一个。
- `args` 是每次启动时调试器的选项，取代其默认选项；启动项的 `debugger_args` 放在它们之后。不设置则保留默认值，`[]` 则去掉默认值。目前唯一的默认值是在无法读取 `/proc/net/tcp` 的环境（Android 上的 Termux）中为 `dlv-dap` 加上的 `--only-same-user=false`：Delve 的同用户检查在那里会拒绝所有连接。在这类系统中为 `dlv-dap` 设置 `args` 会取代它，因此如果仍然需要，请把它再列出来。
- Termide 与适配器通信所需的参数（Delve 的 `dap --listen=…`、GDB 的 `-i dap`、`-m debugpy.adapter`）以及它为命令行调试器准备的脚本总会传入，无法更改。
- `native` 用于在没有命令的情况下调试 Rust 文件时按顺序尝试；它接受 `lldb-dap`、`gdb-dap`、`lldb` 和 `gdb`。
- `console` 适用于没有设置自己的 `console` 的启动项。

设置窗口的 **调试** 选项卡编辑的是同样的内容：先是顺序和控制台，然后是 Termide 所知的每个调试器及其程序和选项，默认值会标明为默认值。每个调试器在单独的页面中打开；其选项按 shell 的拆分方式输入（引号使值保持完整，逗号是选项的一部分），清空表示没有选项，**重置** 会恢复 `PATH` 中的程序和默认选项；在项目设置中则恢复全局设置。

### 调试面板

通过调试适配器启动时会打开调试面板（或复用已打开的面板）。其顶行显示会话及其状态——构建中、启动中、运行中、已暂停及原因、已退出及退出码，或失败及原因——以及按键。下方有三个区域：

- **调用栈** — 暂停的线程的各个帧，最内层在前，每帧带有其文件和行号。在帧上按 `Enter` 或点击它，会显示其变量，并在编辑器中显示其所在行。
- **变量** — 所选帧的各个作用域；第一个获取代价较低的作用域会自动展开。`Enter`、`Space` 或 `→` 展开带成员的变量（首次展开时获取），`←` 将其折叠或转到其父级；再次点击某行会切换其展开状态。
- **输出** — 程序的标准输出和标准错误、适配器的消息、构建输出，以及求值的表达式及其值。它会跟随新输出；`↑`/`PgUp` 向回滚动，`Enter` 回到末尾。

| 按键 | 操作 |
|------|------|
| `c` / `F5` | 继续 |
| `n` | 单步跳过 |
| `s` | 单步进入 |
| `o` | 单步跳出 |
| `p` | 暂停正在运行的程序 |
| `x` | 停止：结束会话和程序 |
| `r` | 重新启动：再次运行该启动项——先执行其 `build`，使用相同的参数——如果会话仍在运行则先结束它 |
| `e` | 在所选帧中对表达式求值 |
| `Tab` / `Shift+Tab` | 下一个 / 上一个区域 |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | 在区域内移动 |

程序暂停时，打开了该文件的编辑器会把光标移到该行，只在必要时滚动，并在边栏中以高亮单元格上的 `→` 标出该行；尚未打开的文件会在新的编辑器中打开，焦点仍留在调试面板上。会话运行期间设置或移除的断点会立即传给适配器。关闭面板或开始另一次启动会结束该会话及其程序。

### 程序的终端

默认情况下（`console = "terminal"`），程序在一个单独的终端面板中运行，该面板在其他面板旁边打开：程序在那里读取键盘输入——聚焦该面板即可向它输入——其输出、颜色和全屏界面也像在任何终端中一样显示在那里，而调试面板保留适配器的消息。debugpy 和 Delve 会请求 Termide 在该面板中启动程序；lldb-dap 则自己启动程序，并通过 lldb 的 `target.input-path`、`output-path` 和 `error-path` 设置获知该面板的终端设备，所有版本的 lldb-dap 都支持这些设置。在 lldb-dap 使用的终端中按 `Ctrl+C` 不会传给程序：请在调试面板中按 `p` 暂停它。GDB 的适配器没有这种模式，因此其程序总是输出到调试面板，且不读取任何输入；智能体启动的会话也是如此，智能体从会话中读取其输出。`console = "panel"` 会让任何适配器都这样运行，适用于不读取输入的程序。

## 提示

- 命令通过 `sh -c`（或终端的 shell）运行，因此管道、`&&`、重定向和环境变量都与在 shell 中一样可用。在 Windows 上，`background` 和 `report` 命令需要 `PATH` 中有 `sh`。
- 对想要查看结果的简短检查（`git status`、代码检查工具）使用 `report`，对不需要其输出的进程使用 `background`，对任何交互式或需要观察的长时间运行任务使用 `terminal`。
- 将项目专用的命令保存在仓库的 `.termide/commands.toml` 中，个人命令保存在全局文件中。
