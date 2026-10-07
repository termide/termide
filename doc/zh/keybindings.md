# 键盘快捷键

Termide 在与绑定匹配前对每个按键事件进行规范化。规范化在派发边界
执行一次,其结果与原始事件一起作为 `KeyChord` 通过面板管道向下传递。
面板和模态对话框根据需要选择形式:

- **`canonical`**(规范) — 用于快捷键匹配、Vim 命令解释、设置中
  的按键捕获。
- **`raw`**(原始) — 用于文本输入(`InsertChar`)、终端面板的
  PTY 透传、搜索缓冲区输入。

在编辑器中输入的文本或发送给终端面板内程序的文本**永远不会**被
规范化重写,西里尔字母、移位字符和 locale 相关字符原样到达目标。
唯一的例外是发送给终端的其他布局字母上的 `Ctrl` 组合键:西里尔字母没有
控制码,因此 `Ctrl+С` 会按其物理键作为 `Ctrl+C`(`^C`)发出。

文本输入框(模态框输入、查找栏、智能体提示框)也按规范形式匹配自身的
`Ctrl` 快捷键——全选、复制、剪切、粘贴、撤销——而输入其中的字母保持原样。

## 规范化修复的问题

| 问题 | 行为 |
| --- | --- |
| 西里尔字母与拉丁字母位于同一物理键(`й`/`q`、`ь`/`m`、…) | 映射到拉丁,使绑定 `Alt+M` 在 QWERTY 与 ЙЦУКЕН 布局下都生效。 |
| `REPORT_ALTERNATE_KEYS` 移位符号重写 | crossterm 将 `Shift+Ctrl+=` 重写为 `Char('+') + Ctrl`(Shift 被剥离,字符被替换)。规范化器逆向: `Char('+') + Ctrl` → `Char('=') + Ctrl + Shift`。 |
| Caps Lock 在字母上的伪 Shift | 当 `REPORT_EVENT_TYPES` 标记了 `KeyEventState::CAPS_LOCK` 时,在匹配前丢弃字母上的 Shift 位。 |
| VTE `Ctrl+/` 折叠为 `Ctrl+7` | 仅当 Kitty 协议**未**激活: `Ctrl+7` → `Ctrl+/`。 |

## 通用层 vs 增强层

某些和弦无法被所有终端编码。Termide 将默认值分为两层,如果当前
终端无法传递任何已配置的增强层和弦,会在启动时发出警告。

### 通用层(在任何 VT100+ 终端上工作)

- `Alt+字母`, `Ctrl+字母`(字母 → ASCII 控制码 0x01–0x1A)。
- `F1`–`F12` 和带**单个**修饰符的 `F1`–`F12`(`Shift+F*`、
  `Alt+F*`、`Ctrl+F*`)。
- 方向键带**单个**修饰符(`Shift+Up`、`Ctrl+Up`、`Alt+Up`)。
- `Home`、`End`、`PgUp`、`PgDn` + 单个修饰符。
- `Enter`、`Tab`、`Esc`、`Backspace`、`Delete`、`Insert` + 单个修饰符。
- `Alt+数字`。
- `Alt+标点`(`Alt+/`、`Alt+,`、`Alt+.`、…)。

### 增强层(需要 Kitty 键盘协议)

- `Ctrl+标点`(`Ctrl+/`、`Ctrl+-`、`Ctrl+=`、`Ctrl+,`、`Ctrl+.`)。
- `Ctrl+Shift+字母`。
- `Ctrl+Alt+任意键`。
- `Alt+Shift+字母` 和 `Alt+Shift+方向键` —— VTE 在传统模式下
  对 `Alt+Shift+l` 发出 `\eL`,与 `Alt+L` 无法区分;
  `Alt+Shift+...` 绑定不能匹配。
- `Super` / `Meta` / `Hyper` 修饰符。

termide 自带的增强层默认值(`toggle_comment` 的 `Ctrl+/`,
`switch_directory` 的 `Ctrl+\`,`replace_all` 的 `Ctrl+Alt+R`)被保留,因为它们是编辑器
中的事实标准。在不支持 Kitty 协议的终端上,termide 在启动时记录
警告,列出受影响的绑定;用户可通过设置 → 键绑定重新绑定。

## macOS: Option 不是 Alt

在 macOS 上,所有终端默认都把 `Option` 当作**文本组合**修饰符——Ghostty
`macos-option-as-alt=false`,kitty `macos_option_as_alt no`,iTerm2
Option=Normal,Terminal.app 的 "Use Option as Meta key" 关闭。因此 `Option+F`
以组合后的字形 `ƒ` 到达,不带 ALT 位,所有 `Alt+<字母>` 默认值(约 25 个
全局动作)都无法触发。波及范围取决于终端:在 Ghostty 上,不产生文本的按键
保留 ALT 位,所以 `Option+F9`、`Option+Up/Down` 和 `Option+Backspace` 仍然
可用,而 Terminal.app 连这些键的修饰符也会剥离,只送出一个裸 `Up`。

Termide 的补救措施是 Kitty 的 `REPORT_ALL_KEYS_AS_ESCAPE_CODES` 标志,它让
终端在系统仍在组合字符的同时把 `Option+F` 上报为 `Alt+F`。该标志仅在 macOS
上、在公布 Kitty 键盘协议的终端上于启动时推送,由以下选项控制:

```toml
[general]
report_all_keys = true  # 默认
```

该标志有一个代价:死键和输入法组合不再到达 termide,因此 `Option+E` `E` → `é`
在应用内失效。需要组合输入的用户应设置 `report_all_keys = false`,然后要么重新
绑定受影响的动作,要么在终端自身中把 `Option` 切换为 `Alt`(Ghostty
`macos-option-as-alt = true`,Terminal.app "Use Option as Meta key")。该标志还会
让终端上报单独按下的修饰键;这些事件在事件边界被丢弃。

当 `Alt+<键>` 绑定无法触发时,termide 会在启动时向日志记录一条警告,指出针对
当前终端的补救办法。

### Ghostty 重新绑定了 Option+Left / Option+Right

与上述组合问题无关,Ghostty 自带以下 macOS 默认配置:

```
keybind = alt+arrow_left=esc:b
keybind = alt+arrow_right=esc:f
```

它们实现了 readline 的按词移动约定,并且在按键到达 termide 之前就已触发:
`Option+Left` 以两个字节 `ESC b` 到达,被解析为 `Alt+B`,`Option+Right` 则成为
`Alt+F`。因此在 Ghostty 上,这两个和弦不只是无法到达 `prev_group` /
`next_group`——它们会触发绑定到 `Alt+B` 和 `Alt+F` 的任何动作,默认分别是
*添加书签*和*新建文件管理器*。

termide 无法撤销这一点:Ghostty 在键盘协议介入之前就应用了该 keybind,因此
`Option+Left` 以两个字节 `ESC b` 到达,与真正的 `Alt+B` 无法区分。Ghostty 的
keybind 前缀(`global:`、`all:`、`unconsumed:`、`performable:`)和键表都无法把
绑定限定在某个应用上。

Termide 曾为这两个动作提供 `Alt+A` / `Alt+D` 作为备选,从而在不改动 Ghostty
的情况下绕过该问题。这两个字母后来让给了用户最先想到的动作——`Alt+D` 分离
实例,`Alt+A` 打开编码代理面板——因此修复现在应放在 Ghostty 的配置中。

要恢复 `Option+Left` / `Option+Right`,在 `~/.config/ghostty/config` 中清除这
两个绑定:

```
keybind = alt+arrow_left=unbind
keybind = alt+arrow_right=unbind
```

用 `Cmd+Shift+,` 重新加载或重启 Ghostty。注意这是终端全局的设置,因此也会移除
您 shell 中 readline 的按词移动。可在 shell 一侧恢复——zsh 在 `~/.zshrc` 中:

```zsh
bindkey "^[[1;3D" backward-word   # Option+Left
bindkey "^[[1;3C" forward-word    # Option+Right
```

bash 则在 `~/.inputrc` 中:

```
"\e[1;3D": backward-word
"\e[1;3C": forward-word
```

这些是 Ghostty 在自身绑定移除后为 `Alt+Left` / `Alt+Right` 发送的标准 xterm
序列;运行 `cat -v` 再按键可以确认您的终端实际发出的内容。

`Option+Up` / `Option+Down` 没有这样的 Ghostty 绑定,能正常到达 `prev_panel` /
`next_panel`。

### Alt+Shift+= 和 Alt+Shift+- 在 macOS 上无法到达 termide

`panel_grow_vertical` 和 `panel_shrink_vertical` 是 macOS 上唯一一对无法触发的
默认值。Option 把 `Option+Shift+=` 组合成单个字形——拉丁布局下是 `±`,俄语布局下
是 `«`——终端上报的是该字形而不是按键。这种映射依赖布局,因此无法像其他平台上
`+` → `Shift+=` 那样逆向还原。

请用鼠标进行垂直调整,或通过设置 → 键绑定把这两个动作重新绑定到不含 `Shift`
的和弦。

### Terminal.app 完全无法使用该补救措施

Terminal.app 不实现 Kitty 键盘协议,因此 `report_all_keys` 无从生效,所有
`Alt+<字母>` 绑定都无法触发,直到启用 **Settings → Profiles → Keyboard → "Use
Option as Meta key"**。开启后,`Option+F` 以 `Alt+F` 到达 termide。

即便如此,它的方向键仍需单独处理。Terminal.app 自带的配置文件按键映射会为
`Option+Left` / `Option+Right` 发送 `ESC b` / `ESC f`——与 Ghostty 相同的冲突,
以 `Alt+B` / `Alt+F` 到达——并且会完全丢弃 `Option+Up` / `Option+Down` 的修饰符,
使其以裸 `Up` / `Down` 到达。两者都可在同一 Keyboard 标签页的按键映射列表中
编辑。在 macOS 26.5 上的实测:

```
Option+Left   -> Alt+Char('b')      Option+Up   -> Up   (no ALT)
Option+Right  -> Alt+Char('f')      Option+Down -> Down (no ALT)
```

一旦开启 Option-as-Meta,`Option+Up` / `Option+Down` 会原样到达 termide;只有
水平方向的一对需要上述 Ghostty 式的解绑。

### macOS: `Option+Z` 无法绑定

`Alt+<字母>` 属于通用层,在 macOS 上一旦 Kitty 协议激活即可工作——只有一个例外。
`Option+Z` 是 US 布局上唯一组合出**大写**字形 `Ω`(U+03A9)的组合。终端将其归类
为移位字符并上报为:

```
Char('Ω') + SHIFT      ← no ALT bit at all
```

而 `Option+Q` 和 `Option+T` 则正确地上报为 `Char('q') + ALT` 和
`Char('t') + ALT`。规范化无法挽回这一点:ALT 位从未到达,而把 `Ω` 映射回 `z`
会误伤输入希腊文的用户。

因此 `Alt+Z` 绑定在 macOS 上永远不会匹配。请选择另一个字母——这正是
`detach_instance` 默认为 `Alt+D` 的原因。

### macOS 保留了部分功能键

`F11` 被 macOS 的 Mission Control 绑定为*显示桌面*,永远不会到达终端,因此
*切换堆叠*的 `F11` 备选项开箱即用时不工作;请使用 `Alt+Backspace`。

更广泛地说,除非启用 **系统设置 → 键盘 → "将 F1、F2 等键用作标准功能键"**,
F 键行发送的是媒体键,任何 `F<n>` 绑定都无法到达。对大多数全局动作而言,这只
损失一对绑定中的 F 键那一半——`F9` 不工作时 `Alt+M` 仍能打开菜单。但以下默认值
只绑定到功能键而没有其他备选,在该设置开启之前确实无法触发:

| 动作 | 绑定 | 节 |
|---|---|---|
| 在组内最大化面板 | `Alt+F11` | `general` |
| 删除行 | `F8` | `editor` |
| 查找下一个 | `F3` | `editor` |
| 查找上一个 | `Shift+F3` | `editor` |
| 转到定义 | `F12` | `editor` |
| 查找引用 | `Shift+F12`、`F24` | `editor` |
| 重命名符号 | `F4` | `editor` |
| 查看文件 | `F3` | `git_status` |
| 编辑文件 | `F4` | `git_status` |

termide 在 macOS 上启动时会记录一条列出这些绑定的警告。请启用该设置,或通过
设置 → 键绑定重新绑定这些动作。

## 终端兼容性 (2026)

| 终端 | Kitty 键盘协议 |
| --- | --- |
| kitty | 完整 |
| foot 1.13+ | 完整 |
| WezTerm | 完整 |
| Ghostty | 完整 |
| iTerm2 | 完整 |
| rio | 完整 |
| Windows Terminal Preview 1.25+ | 完整 |
| alacritty | 部分(CSI-u,无增强标志) |
| xterm | 部分(需手动配置) |
| GNOME Terminal / Tilix / VTE | 无(开发中) |
| Konsole | 无(已计划) |
| tmux | 透传(取决于宿主终端) |

如果您的终端不公布 Kitty 协议而您依赖增强层和弦,可以切换到支持
的终端,或在 `config.toml` → `[*.keybindings]` 中将相关动作重新
绑定到通用层备选项。

## 冲突检测

设置 → 键绑定显示内联警告,当您分配的和弦已被另一个动作占用。同一
动作已接受的和弦会被直接拒绝——它只会在该行中添加该动作已响应的
按键的另一种写法——拒绝原因显示在其位置上。检测三类冲突:

- **同一节内** — 同一节中两个动作共享和弦;第二个变得无法到达。
- **跨节遮蔽** — 全局和弦遮蔽面板本地的;面板绑定永远不会触发。
- **跨节并存** — 两个面板本地绑定重叠;只有焦点面板处理事件,
  通常安全但值得注意。

同一节冲突也会在启动时记录。

## 自定义默认值

在 `config.toml` 中覆盖任何绑定。字符串以规范形式解析,因此
`"Alt++"` ≡ `"Alt+Shift+="`,`"Ctrl+Й"` ≡ `"Ctrl+Q"`:

```toml
[general.keybindings]
panel_grow_vertical = "Alt+Shift+="
panel_shrink_vertical = "Alt+Shift+-"
open_projects = "Alt+\\"

[editor.keybindings]
trigger_completion = ["Ctrl+J", "Ctrl+Space"]
toggle_comment = ["Ctrl+/", "Ctrl+."]
replace_all = ["Ctrl+Alt+R", "Alt+R"]

[file_manager.keybindings]
switch_directory = "Ctrl+\\"

[terminal.keybindings]
switch_directory = "Ctrl+\\"
```

注:`Ctrl+/` 和 `Ctrl+\` 通过 `KeyNormalizer` quirk 即使在传统终端
(例如 VTE)上也能工作 —— VTE 将它们作为 `\x1F` 和 `\x1C` 控制字节发送,
crossterm 解析为 `Ctrl+7` / `Ctrl+4`,规范化器将其重写回斜杠 / 反斜杠。

任何动作都支持多个备选项:列在数组中。第一项是帮助面板中显示的
规范字符串。

不用改文件也能配出同样的形状:在 Settings → Keybindings 中,每一行列出
该动作接受的所有按键,`Left`/`Right` 在各按键之间移动光标,`Enter` 为
光标所在的按键捕获一次组合,焦点行末尾画着的 `[+]` 则追加一个新的备选。
参见 [ui.md](ui.md#设置模态框settings)。

### 未绑定的操作

有些操作默认不带绑定。对 `prev_project` / `next_project` 而言，每个仍然空闲
的组合键在某处都会与桌面或 shell 的快捷键冲突：`Ctrl+Alt+Left` /
`Ctrl+Alt+Right` 在多个 Linux 桌面中用于切换工作区，而 `Alt+.` 在 bash 中插入
上一条命令的最后一个参数。`goto_panel_1` … `goto_panel_9` 已将 `Alt+1` …
`Alt+9` 让给 `goto_project_1` … `goto_project_9`，后者切换到对应编号的已打开
项目。请将它们绑定到您系统上空闲的组合键：

| 操作 | 作用 |
|------|------|
| `prev_project` / `next_project` | 按 `项目` 菜单中的排列顺序切换到上一个 / 下一个已打开的项目 |
| `goto_panel_1` … `goto_panel_9` | 将焦点移到对应编号的面板组（列） |

```toml
[general.keybindings]
prev_project = "Shift+F5"
next_project = "Shift+F6"
goto_panel_1 = "Shift+F1"
```

若要把数字键还给面板，请在同一文件中为项目另行绑定。当 `goto_project_N`
未设置时，绑定到 `Alt+N` 的 `goto_panel_N` 会被视为旧版本保存的旧默认值副本，
并让位于新的默认值：

```toml
[general.keybindings]
goto_panel_1 = "Alt+1"
goto_project_1 = "Shift+F1"
```

`goto_project_N` 的按键会显示在 `项目` 菜单中对应项目的旁边。
