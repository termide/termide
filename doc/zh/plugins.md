# 插件

插件是项目打开期间 termide 在后台运行的程序，termide 通过该程序的标准输入和标准输出以 JSON-RPC 与它通信。插件能获知项目的编辑器和仓库中发生的事，读取和编辑打开的缓冲区，添加带有自己快捷键的命令，在编辑器中标记行，在状态栏中显示项目，在 termide 的窗口中提问，打开自己的面板和终端，并为编码代理提供工具。

插件是全局的：它们安装在你的配置目录中，项目只能关闭你安装的插件，或开启你允许它开启的插件。打开的仓库永远无法决定运行哪个程序。

## 安装

把插件的目录复制到与 `config.toml` 同级的 `plugins/` 中：

| 平台 | 目录 |
|---|---|
| Linux | `~/.config/termide/plugins/<name>/` |
| macOS | `~/Library/Application Support/termide/plugins/<name>/` |
| Windows | `%APPDATA%\termide\plugins\<name>\` |

目录名就是插件名：由字母、数字、`-` 和 `_` 组成。目录中包含一个 `plugin.toml`：

```toml
description = "Formats on save"
command = "./run.py"          # 相对于本目录
args = ["--quiet"]
[env]
LOG_LEVEL = "info"
```

`command` 可以是绝对路径、相对于插件目录的路径（包含 `/`），或者像 `python3` 这样的裸名称。裸名称只在 `PATH` 的绝对路径项中查找。插件以自己的目录作为工作目录运行，并从 `initialize` 得知项目的根目录。

插件在 termide 启动时读取。安装、删除或修改插件后，在命令面板（`Ctrl+P`）中执行 **插件：重新读取插件目录** 即可无需重启地载入它：所有插件都会重新启动，新安装插件的语言功能对此后打开的编辑器生效。**插件：显示状态** 列出已安装的插件及其在当前项目中的状态，**插件：重启 <名称>** 会重新启动某个插件。

## 开启和关闭插件

全局 `config.toml` 为每个插件设置模式：

```toml
[plugins.formatter]
mode = "on"       # 默认值：在每个项目中运行，项目可以关闭它
[plugins.deploy]
mode = "opt-in"   # 只在开启它的项目中运行
[plugins.experiment]
mode = "off"      # 在任何地方都不运行

[plugins.formatter.settings]
line_width = 100  # 在 `initialize` 中交给插件
```

项目的 `.termide/config.toml` 可以在插件模式允许的范围内开关它，并补充其设置：

```toml
[plugins.deploy]
enabled = true
[plugins.formatter]
enabled = false
```

| 全局 `mode` | 项目未设置 | `enabled = false` | `enabled = true` |
|---|---|---|---|
| `on`（或无条目） | 运行 | 关闭 | 运行 |
| `opt-in` | 关闭 | 关闭 | 运行 |
| `off` | 关闭 | 关闭 | 关闭 |

设置（`Alt+P`）中的 **插件** 标签页列出已安装的插件：全局存储时，在插件上按 `Enter` 切换其模式；存储在项目中时，切换项目的开关。

项目文件中的 `mode` 和全局文件中的 `enabled` 会被忽略，并在日志中留下警告。对作用于项目内容的插件——例如运行项目脚本的插件——请设为 `opt-in`，这样任何仓库都无法未经你同意就开启它们。

每个打开的项目都为它运行的每个插件拥有一份独立副本，包括停放在后台的项目。崩溃的插件会在 1、2、4…… 秒后重启。一分钟内崩溃三次后，它将保持关闭，直到其设置发生变化、它被重启、插件目录被重新读取或 termide 重启，其所属项目的状态栏会对此作出提示。

## 编写插件

每条消息是一行 JSON-RPC 2.0。插件写到标准错误的任何内容都会进入 termide 的日志。用 Python 编写时，仓库中的 `examples/plugins/sdk/termide_plugin.py` 会替你处理协议，`examples/plugins/` 中有用它写成的插件。

**termide → 插件**

| 方法 | 类型 | 参数 | 结果 |
|---|---|---|---|
| `initialize` | 请求 | `protocolVersion`（1）、`termide.version`、`plugin.name`、`plugin.dir`、`project.root`、`settings` | 插件提供的内容，见下文 |
| `initialized` | 通知 | | |
| `command/execute` | 请求 | `id` | 任意 |
| `tool/call` | 请求 | `name`、`arguments`、`progress` | `text`、`isError` |
| `$/cancelRequest` | 通知 | termide 已放弃的请求的 `id` | |
| `shutdown` | 请求 | | `null` |
| `exit` | 通知 | | |

插件可订阅的事件，即关于其自身项目的通知：

| 事件 | 参数 | 时机 |
|---|---|---|
| `file/opened` | `path` | 文件在编辑器中打开 |
| `file/saved` | `path` | 编辑器写入了它的文件 |
| `file/closed` | `path` | 当前项目的某个编辑器关闭 |
| `editor/focused` | `path` | 另一个文件的编辑器成为活动面板 |
| `editor/changed` | `path`、`version` | 缓冲区发生变化，并在输入停顿 300 毫秒后发送 |
| `git/changed` | `root` | 仓库的索引、HEAD 或引用发生变动：提交、检出、暂存 |
| `agent/started`、`agent/finished` | `panel`，结束时另有 `ok` | 代理面板的一次运行开始或结束 |

`file/will_save` 是请求而不是事件：订阅了它的插件，会在其项目的编辑器中按下 `Ctrl+S`、写入文件之前收到 `{ "path", "version" }`，并可以回答 `{ "edits": [...] }`（同 `editor/edit`）或 `null`——即保存时格式化。termide 依次询问这些插件，每个三秒，若缓冲区期间未改变就应用其编辑，然后写入文件。关闭时保存和“另存为”不会询问。

只有当前项目的某个插件请求了编辑器事件时，termide 才会监视这些事件。

`initialize` 的结果，以及之后为替换它而发送的 `contributions/changed` 通知的参数：

```json
{
  "subscriptions": ["file/saved"],
  "commands": [{ "id": "check", "title": "Check the project", "key": "Ctrl+Alt+K" }],
  "tools": [{
    "name": "lookup",
    "description": "Look a word up in the project glossary",
    "inputSchema": { "type": "object", "properties": { "word": { "type": "string" } } },
    "readOnly": true
  }]
}
```

命令以 `<plugin>: <title>` 的形式出现在命令面板（`Plugins` 类别）中以及命令菜单的末尾。命令的 `key` 可以在任何地方运行它，前提是 termide 没有在任何地方绑定该键（包括用户自己的命令），并且它是 `Ctrl` 或 `Alt` 加另一个修饰键（`Ctrl+Alt+K`、`Ctrl+Shift+K`、`Alt+Shift+K`），或 F 键加一个修饰键（`Shift+F7`）：这些是终端中的程序很少需要的键。不符合条件的键不会被绑定，并在日志中留下一行记录。

工具以 `plugin-<plugin>__<tool>` 的名称提供给插件所属项目的代理会话。代理的权限规则把它们当作 MCP 工具对待：除非标为 `readOnly`，运行前都会询问。工具运行期间，插件可以带上调用中的 `progress` 令牌发送 `tool/progress` `{ "token", "text" }`：即到目前为止的完整输出，会随到随显示在对话记录中。用户停止的调用，或超过十分钟的调用，之后会收到 `$/cancelRequest`；超过五分钟的命令也是如此。

**插件 → termide**

| 方法 | 类型 | 参数 |
|---|---|---|
| `contributions/changed` | 通知 | 与 `initialize` 的结果相同 |
| `window/notify` | 请求 | `message`、`level`（`"info"` 或 `"error"`） |
| `status/set` | 请求 | `id`、`text`（空文本会移除该项目） |

后台项目的插件不会显示在屏幕上：它的通知会写入日志。termide 每次刷新屏幕时最多从所有插件接收 256 条消息，其余的等待下一次刷新。

### 补全、悬停提示与修复

插件可以像语言服务器那样在编辑器中作答：与服务器并列，或为没有服务器的语言作答。它在贡献中的 `providers` 里按 LSP id（`rust`、`python`……——`*` 表示全部）列出语言：

```json
"providers": { "completion": ["python"], "hover": ["*"], "codeActions": ["python"],
               "definition": ["python"], "references": ["python"] }
```

| 请求 | 参数 | 回答 |
|---|---|---|
| `editor/completion` | `path`、`line`、`col` | `{ "items": [...] }`，LSP 的 `CompletionItem`（`label`、`insertText`、`detail`、`kind`……） |
| `editor/hover` | `path`、`line`、`col` | `{ "markdown" }` |
| `editor/code_actions` | `path`、`line`、`col`、`end_line`、`end_col`、`diagnostics`（`line`、`col`、`end_line`、`end_col`、`message`、`source`） | `{ "actions": [{ "title", "edits" }] }`，编辑同 `editor/edit` |
| `editor/definition`、`editor/references` | `path`、`line`、`col` | `{ "locations": [{ "path", "line", "col", "end_line"?, "end_col"? }] }`，路径为绝对路径 |

termide 会与服务器一起询问文件所属项目的插件，并给出一个合并的回答：补全项合在一起，悬停提示上下排列，修复合成一个列表，跳转位置（`F12`）和引用也合在一起。插件有 1.5 秒；更晚的回答不会进入这次结果。修复只修改被询问的那个文件，作用于其打开的缓冲区，标题以插件名开头，以便看清是谁的建议。每个插件最多取 500 个补全项、50 个修复和 20 000 个字符的悬停内容。

### 编辑器

插件可以访问其所属项目的编辑器，且只能访问这些编辑器：

| 方法 | 参数 | 结果 |
|---|---|---|
| `editor/active` | | 活动编辑器的 `path`、`line`、`col`、`selection`（`{ line, col, end_line, end_col }` 或 `null`）、`modified`、`version`；没有活动编辑器或插件所属项目在后台时为 `null` |
| `editor/read` | `path` | 打开的缓冲区的 `text`、`version`、`modified`，包括未保存的更改 |
| `editor/edit` | `path`、`edits`：`[{ "line", "col"?, "end_line"?, "end_col"?, "text" }]`、`version`（可选） | `{ "version" }` |
| `editor/open` | `path`、`line`、`col`（可选） | `null`；在当前项目中打开该文件 |

位置按 LSP 的方式计数：行号从 0 开始，列以 UTF-16 码元计；没有结束位置的编辑即为插入。`editor/edit` 只修改打开的缓冲区——termide 不会替插件写文件，插件可以自己写——并且这些编辑构成一个撤销步骤，因此 `Ctrl+Z` 可以撤回它们。带上 `version` 时，如果缓冲区在该版本（由 `editor/read` 或 `editor/changed` 报告）之后发生了变化，编辑会以 `stale` 被拒绝，这样根据文本计算出编辑的插件就不会把它们应用到另一份文本上。仅用于查看文件的编辑器的缓冲区同样会被拒绝。`editor/read` 最多交出 16 MB，`editor/edit` 一次最多接受 10 000 处编辑。

### 终端

`terminal/run` `{ "command", "cwd" }` 在插件所属项目（必须是当前项目）中打开一个终端面板，并在其中键入 `command`，让用户看到运行的内容；`cwd`（绝对路径）默认为项目根目录。命令只能是一行，且不能含控制字符；无法打开终端时返回错误。

### 编辑器中的标记

插件可以像语言服务器那样标记文件中的行，使用 `editor/diagnostics`（请求）`{ "path", "items" }`：标记显示在编辑器边栏和文字下方，其消息显示在下面的行中，也会出现在诊断面板里，与语言服务器的标记并列，并以插件名标注。每次调用都会替换该插件在此文件上的标记；`items` 为空则清除它们。`path` 为绝对路径。

```json
{ "path": "/home/me/proj/src/main.rs", "items": [
  { "line": 11, "message": "TODO left behind", "severity": "info" },
  { "line": 40, "col": 8, "end_col": 15, "severity": "error",
    "message": "unsafe call", "code": "S101" }
] }
```

行号从 0 开始，列按 LSP 的方式计数（UTF-16 码元）。没有 `col` 时标记整行；只有 `col` 时标记该处的单词。`severity` 为 `error`、`warning`（默认）、`info` 或 `hint`。之后打开的文件会显示此前设置的标记；插件停止时其标记会被移除。

### 窗口

插件可以在 termide 自己的窗口中向用户提问。请求会在用户关闭窗口时得到答复：带上回答；若窗口被关闭（`Esc`）则为 `null`。

| 方法 | 参数 | 回答 |
|---|---|---|
| `window/confirm` | `title`、`message` | `true`、`false` 或 `null` |
| `window/choose` | `title`、`items`（字符串） | 所选项的索引，或 `null` |
| `window/input` | `title`、`prompt`、`default`（可选） | 文本，或 `null` |
| `window/form` | `title`、`fields`：`[{ "name", "label", "type", "options"?, "default"? }]`，`type` 为 `text`、`number`、`bool` 或 `select` | `{ name: value }`（开关为布尔值，数字为数字），或 `null` |
| `window/message` | `title`、`message` | `null` |

每个窗口都由 termide 加上标题 `🧩 <插件> · <title>`，使它绝不会被当成 termide 自己的提问；插件给出的文字会去掉控制字符与不可见字符。没有隐藏输入：要求输入密码的窗口正是钓鱼插件会显示的东西，因此机密应放在插件的 `settings` 或其环境中。窗口会排在屏幕上已有的窗口之后，并且只在其插件所属项目为当前项目时显示；每个插件一次只能有一个窗口，期间再请求会以 busy 被拒绝。插件停止或其项目关闭时，它的窗口会被撤下。

### 面板

插件可以打开自己的面板，按它发送的 Markdown 绘制：标题、列表、表格、代码和链接，与 Markdown 查看器相同，带光标、选择、搜索和复制。指向 `plugin:<动作>` 的链接就是按钮：跟随它（在链接上按 `Enter` 或点击）会把动作发回插件，插件再以更新面板作答。

| 方法 | 类型 | 方向 | 参数 / 结果 |
|---|---|---|---|
| `panel/open` | 请求 | 插件 → termide | `title`、`markdown`、`menu`（可选）；结果为 `{ "panel" }`，即其 id |
| `panel/update` | 请求 | 插件 → termide | `panel`、`markdown`，以及可选的 `title` 和 `menu` |
| `panel/close` | 请求 | 插件 → termide | `panel` |
| `panel/action` | 通知 | termide → 插件 | `panel`、`action` |
| `panel/closed` | 通知 | termide → 插件 | `panel`：用户关闭了它 |

`menu` 是面板 `[≡]` 菜单中它自己的条目，`[{ "label", "action" }]`（最多 12 个）：选中的条目会像 `plugin:` 链接一样作为 `panel/action` 发送。表明面板出自谁之手的部分由 termide 决定：图标总是 🧩，标题总是以插件名开头（`deploy: Report`）；插件给出的标题和标签会去掉控制字符与不可见字符并截断长度。

面板在插件所属的项目中打开，该项目必须是当前项目；面板不会随布局保存。更新会尽量让读者停留在原处。插件停止时，它的面板保留并说明这一点；重新启动的插件不会再拿回它们。

```python
# 位于下方示例的循环中
elif method == "command/execute":            # 命令“显示报告”
    send({"jsonrpc": "2.0", "id": id, "result": None})
    send({"jsonrpc": "2.0", "id": "open", "method": "panel/open",
          "params": {"title": "Report", "markdown": "# Report\n\n[Refresh](plugin:refresh)"}})
elif msg.get("id") == "open":                # panel/open 的应答
    panel = msg["result"]["panel"]
elif method == "panel/action" and msg["params"]["action"] == "refresh":
    send({"jsonrpc": "2.0", "id": "upd", "method": "panel/update",
          "params": {"panel": panel, "markdown": "# Report\n\nUpdated.\n\n[Refresh](plugin:refresh)"}})
```

### 示例

一个在状态栏中统计保存次数的 Python 插件：

```python
#!/usr/bin/env python3
import json, sys

def send(message):
    print(json.dumps(message), flush=True)

saves = 0
for line in sys.stdin:
    msg = json.loads(line)
    method, id = msg.get("method"), msg.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id, "result": {
            "subscriptions": ["file/saved"],
            "commands": [{"id": "reset", "title": "Reset the save count"}]}})
    elif method == "file/saved":
        saves += 1
        send({"jsonrpc": "2.0", "id": f"s{saves}", "method": "status/set",
              "params": {"id": "saves", "text": f"saved {saves}×"}})
    elif method == "command/execute":
        saves = 0
        send({"jsonrpc": "2.0", "id": id, "result": None})
        send({"jsonrpc": "2.0", "id": "r", "method": "status/set",
              "params": {"id": "saves", "text": ""}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": id, "result": None})
    elif method == "exit":
        break
```

把它保存为 `plugins/save-count/run.py`，设为可执行，并添加内容为 `command = "./run.py"` 的 `plugins/save-count/plugin.toml`。
