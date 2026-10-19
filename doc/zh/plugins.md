# 插件

插件是项目打开期间 termide 在后台运行的程序，termide 通过该程序的标准输入和标准输出以 JSON-RPC 与它通信。插件可以响应文件的打开和保存，向命令面板和命令菜单添加命令，在状态栏中显示项目，并为编码代理提供工具。

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

插件在 termide 启动时读取。安装或删除插件后请重启 termide。

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

项目文件中的 `mode` 和全局文件中的 `enabled` 会被忽略，并在日志中留下警告。对作用于项目内容的插件——例如运行项目脚本的插件——请设为 `opt-in`，这样任何仓库都无法未经你同意就开启它们。

每个打开的项目都为它运行的每个插件拥有一份独立副本，包括停放在后台的项目。崩溃的插件会在 1、2、4…… 秒后重启。一分钟内崩溃三次后，它将保持关闭，直到其设置发生变化或 termide 重启，状态栏会对此作出提示。

## 编写插件

每条消息是一行 JSON-RPC 2.0。插件写到标准错误的任何内容都会进入 termide 的日志。

**termide → 插件**

| 方法 | 类型 | 参数 | 结果 |
|---|---|---|---|
| `initialize` | 请求 | `protocolVersion`（1）、`termide.version`、`plugin.name`、`plugin.dir`、`project.root`、`settings` | 插件提供的内容，见下文 |
| `initialized` | 通知 | | |
| `file/opened`、`file/saved` | 通知，仅在订阅时发送 | `path` | |
| `command/execute` | 请求 | `id` | 任意 |
| `tool/call` | 请求 | `name`、`arguments` | `text`、`isError` |
| `shutdown` | 请求 | | `null` |
| `exit` | 通知 | | |

`initialize` 的结果，以及之后为替换它而发送的 `contributions/changed` 通知的参数：

```json
{
  "subscriptions": ["file/saved"],
  "commands": [{ "id": "check", "title": "Check the project" }],
  "tools": [{
    "name": "lookup",
    "description": "Look a word up in the project glossary",
    "inputSchema": { "type": "object", "properties": { "word": { "type": "string" } } },
    "readOnly": true
  }]
}
```

命令以 `<plugin>: <title>` 的形式出现在命令面板（`Plugins` 类别）中以及命令菜单的末尾。工具以 `plugin-<plugin>__<tool>` 的名称提供给插件所属项目的代理会话。代理的权限规则把它们当作 MCP 工具对待：除非标为 `readOnly`，运行前都会询问。

**插件 → termide**

| 方法 | 类型 | 参数 |
|---|---|---|
| `contributions/changed` | 通知 | 与 `initialize` 的结果相同 |
| `window/notify` | 请求 | `message`、`level`（`"info"` 或 `"error"`） |
| `status/set` | 请求 | `id`、`text`（空文本会移除该项目） |

后台项目的插件不会显示在屏幕上：它的通知会写入日志。

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
