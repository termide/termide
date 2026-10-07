# 可分离实例

可分离实例在启动它的终端消失后仍继续运行。关闭 SSH 连接，几小时后回来，再次
接入——编辑器、shell、LSP 服务器和长时间运行的任务都还在您离开时的状态。

这与 `tmux` 和 `screen` 提供的保证相同，但您与 TermIDE 之间不再有第二层复用器：
没有与 TermIDE 自身绑定争抢的前缀键，也没有需要为颜色或鼠标上报单独配置的第二层。

> 仅限 Unix（Linux、macOS、BSD）。Windows 没有 `fork`/`setsid`，其 ConPTY 模型
> 需要另一种宿主，因此这些参数在 Windows 上不可用。

## 快速开始

```bash
termide --detached            # 启动实例，打印其 ID
termide --list-instances       # 查看正在运行的实例
termide --attach              # 接入最近的实例
termide --attach my-project   # 接入指定的实例
```

`Alt+D` 可再次分离，让一切继续运行——菜单中的**选项 → 分离实例**亦然。该菜单项
只在可分离实例中显示；在普通实例中没有可分离的对象，因此直接不显示，而不是显示
出来再拒绝。

实例以启动它的项目目录命名，因此在 `~/src/my-project` 中启动会得到 `my-project`。
在同一目录中再启动一个实例，它就成为 `my-project-2`。

## 让每个实例都可分离

唯一的难点是要在启动时记得加 `--detached`：以普通方式启动的 termide 之后无法再
分离。运行中的进程绑定在其终端的 PTY 上——文件描述符已打开，子进程已继承它们，
控制终端已分配——没有任何办法把它迁移到另一个终端。`tmux` 无法接管已在运行的
程序也是同样的原因。

如果您大部分时间都这样工作，可以永久开启：

```toml
[general]
always_detachable = true
```

或在设置（`Alt+P`）→ 常规中勾选**始终可分离实例（Unix）**。此后每个 `termide`
都在自己的宿主中启动，`Alt+D` 在任何地方都可用。

启用前值得了解：

- **关闭终端不再意味着"停止 termide"。** 实例会存活下来，它的 LSP 服务器、监视器
  和 shell 也一样。通过 SSH 时这正是目的，在本机则可能令人意外——请不时查看
  `--list-instances`。
- **作为 `$EDITOR` 启动时不受影响。** 带文件参数时（`EDITOR=termide git commit`）
  该选项被忽略：git 会等待编辑器退出，而分离会让它误以为编辑已经完成。
- 多出的一层 PTY 在大量输出时会稍微降低吞吐量，与 tmux 一样。

## 典型的远程工作流

```bash
ssh server
cd ~/src/my-project
termide --detached
termide --attach
# … 工作、启动构建、在终端面板中运行代理 …
# 按 Alt+D，或者直接关闭 SSH 连接
```

之后，在任意机器上：

```bash
ssh server
termide --attach my-project
```

不分离直接关闭 SSH 连接是安全的。实例会察觉客户端已离开并继续运行；下一次
`--attach` 会重新接上它。

## 什么会保留，以及为什么

一切都会保留。实例不是被保存再恢复——它从未停止过。

`termide --detached` 会启动一个小型宿主进程，它拥有一个 PTY，并在其中运行一个
普通的 TermIDE。您的 shell、LSP 服务器、监视器和后台任务都是该 TermIDE 的子进程，
因此客户端的来去对它们没有影响。接入就是把一个终端连接到宿主；分离就是断开它。

这与 `~/.local/share/termide/projects/` 中已保存的项目布局不同——后者记录哪些面板曾经
打开，以便一个*新的* TermIDE 重新打开它们。它仍像以前一样工作，在您第一次启动
实例时也仍然适用。

## 从另一个终端重新接入

您可以从一个与启动时完全不同的终端接入——不同的尺寸、不同的模拟器、不同的
`TERM`。接入时，TermIDE 会与当前正在查看它的终端重新协商备用屏幕、鼠标上报、
括号粘贴和键盘协议，根据客户端的 `TERM` 重新检测颜色支持，并完整重绘。

接入状态下调整终端大小的行为与平常相同；布局会像本地实例一样重新分配。

## 命令

| 命令 | 作用 |
|---------|--------------|
| `termide --detached` | 启动可分离实例并打印其 ID |
| `termide --detached file.rs` | 同上，并照常打开文件 |
| `termide --detached --restore`（`-r`） | 同上，并重新打开上次运行的项目 |
| `termide --attach` | 接入最近的实例 |
| `termide --attach <ID>` | 接入指定名称的实例 |
| `termide --attach <ID> --force`（`-f`） | 接入，并从已接入的客户端手中接管实例 |
| `termide --kill <ID>` | 结束实例及其中运行的一切 |
| `termide --list-instances` | 列出实例：ID、pid、运行时长、状态、项目 |

`--list-instances` 还会清理宿主进程已消失的实例，因此崩溃永远不会留下幽灵条目。

加载 shell 补全后（`termide --completions <shell>`，参见
[安装](installation.md#shell-补全)），在 `--attach` 和 `--kill` 之后按 Tab 会给出此表中的 ID。

## 分离

| 方式 | 何时使用 |
|-----|----------------|
| `Alt+D` | 常规方式。可在 `[general.keybindings]` 节中以 `detach_instance` 重新绑定。 |
| 关闭终端 | 安全。实例会察觉并继续运行。 |
| `Ctrl+Z` | **不**起作用，也不可能起作用：termide 以原始模式读取按键，因此该按键永远到不了 tty 行规程，无法变成 SIGTSTP。`Alt+D` 才是实现您本意的绑定。 |
| 连按三次 `Ctrl+\` | 仅限紧急情况——TermIDE 自身已停止响应时使用。由客户端处理，因此即使应用无响应也有效。 |

结束实例与结束任何 TermIDE 相同：在接入状态下退出它（`Alt+Q`）。这也会停止宿主
进程，并移除该实例。

如果无法接入后再退出——实例卡住了，或者只是不再需要——可以从外部结束它：

```bash
termide --kill my-project
```

实例中的 TermIDE 会收到 SIGTERM，三秒后仍未退出则收到 SIGKILL；如果宿主进程也没有
退出，`--kill` 会一并结束它。该实例中未保存的更改会丢失，其中的 shell 和任务也随之
结束。此时已接入的客户端会被告知实例已结束。

## 同一时间只允许一个客户端

当另一个客户端已接入时，对同一实例的第二次 `--attach` 会被拒绝，而不是把屏幕镜像
给两者。分离第一个客户端（或关闭其终端）后，下一次接入会立即成功。

如果第一个客户端无法触及——留在已锁屏的桌面上，或挂在卡住的 SSH 连接之后——可以
接管实例：

```bash
termide --attach my-project --force   # 或 -f
```

另一个客户端会被分离，并提示实例已被接管；实例会为新终端重绘。其中的一切不受影响。
完全停止读取的客户端会被直接断开，不会收到提示。

由较旧版本 TermIDE 启动的实例不理解 `--force`：请用常规方式分离其客户端，或用
`--kill` 结束该实例。

## 实例状态存放在哪里

套接字在 Linux 和 BSD 上位于 `$XDG_RUNTIME_DIR/termide/`，在 macOS 上位于
`~/Library/Application Support/termide/run/`——macOS 没有 `XDG_RUNTIME_DIR`。该目录
仅所有者可访问（`0700`），因此本机上的其他账户无法接入您的实例。

那里没有任何需要手动清理的东西：套接字在宿主消失后最多存活到下一次
`--list-instances` 或 `--detached`，它们会将其清除。
