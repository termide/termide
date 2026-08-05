# 架构

本文档描述 TermIDE 的技术架构。

## 概述

TermIDE 是一个使用 Rust 语言基于 `ratatui` TUI 框架构建的基于终端的 IDE。它采用自适应的**垂直拆分面板布局**：每个面板高度可独立调整，并支持一键切换聚焦面板的全屏预设。

```
┌─────────────────────────────────────────────────────────┐
│ 菜单栏     [CPU] [RAM] [时钟]                            │
├───────────────────┬─────────────────────────────────────┤
│ ┌[≡] 📁 文件 ──┐ │ ┌[≡] 📝 编辑器: main.rs ───────────┐│
│ │ src/          │ │ │                                  ││
│ │ tests/        │ │ │  fn main() {                     ││
│ │ Cargo.toml    │ │ │      // code here                ││
│ │               │ │ │  }                               ││
│ ├[≡] 💻 终端 ──┤ │ │                                  ││
│ │ $ cargo build │ │ │                                  ││
│ │ Compiling...  │ │ │                                  ││
│ └───────────────┘ │ └──────────────────────────────────┘│
├───────────────────┴─────────────────────────────────────┤
│ 状态: file.rs:42  行 10, 列 5        磁盘: 83%          │
└─────────────────────────────────────────────────────────┘
```

左侧列上方为两个共享列、各自高度可调的面板；右侧由一个面板独占整列。按 `Alt+F11` 把活动列中所有非聚焦面板缩为标题行（一行预设），再次按下恢复之前的高度。

## 核心架构组件

### 1. 布局系统

#### 1.1 LayoutManager

**位置：** `crates/layout/src/lib.rs`

`LayoutManager` 持有拆分布局的状态。它管理：

**组成部分：**
- `panel_groups: Vec<PanelGroup>` - 面板组的水平排列
- `focus: usize` - 当前焦点（活动面板组的索引）

**主要职责：**
- 基于宽度阈值自动堆叠添加面板
- 管理水平导航（Alt+Left/Right）
- 管理组内垂直焦点（Alt+Up/Down）
- 在相邻组之间智能堆叠/拆分面板（Alt+Backspace、F11）
- 终端尺寸变化时按比例重新分配宽度
- 关闭面板并清理空组

**焦点管理：**
焦点是一个简单的 `usize` 索引，指示当前活动的面板组。获得焦点的组接收键盘/鼠标输入，并在 UI 中高亮显示。

#### 1.2 PanelGroup

**位置：** `crates/layout/src/panel_group.rs`

`PanelGroup` 表示同列中的多个面板，每个面板高度可调。

**结构：**
```rust
pub struct PanelGroup {
    panels: Vec<Box<dyn Panel>>,         // 此组中的面板
    expanded_index: usize,               // 聚焦面板（活动边框）
    pub width: Option<u16>,              // 列宽（None = 自动分配）
    split_heights: Option<Vec<u16>>,     // 缓存的每面板高度
    fullscreen_cache: Option<Vec<u16>>,  // 进入全屏预设前的高度，用于撤销
}
```

**布局行为：**
- 组内所有面板均可见。每个面板的最低高度为一行（标题栏）。
- `split_heights` 是按面板缓存的高度。`None` 表示「无缓存——首次使用时按等量分配」。终端列高变化时高度按比例重缩放。
- 按下 `Alt+F11`（或绑定 `toggle_fullscreen_panel`）切换全屏预设：聚焦面板占满整列高度，其他面板各自缩为一行。预设前的高度保存到 `fullscreen_cache`，再次按下即可恢复。
- 预设激活时，通过 `Alt+Up` / `Alt+Down`（`prev_panel` / `next_panel`）切换焦点会把预设重新应用到新焦点 —— 视觉上等同于经典的手风琴视图。
- `Alt+Shift+=` / `Alt+Shift+-`（`panel_grow_vertical` / `panel_shrink_vertical`）以 1 行为步长增大/缩小聚焦面板，向有空闲的相邻面板让出/获取行数（级联）。

**关键操作：**
- `add_panel()` / `insert_panel()` / `remove_panel()` - 修改面板列表并重新平衡高度缓存。
- `set_expanded()` / `next_panel()` / `prev_panel()` - 移动焦点；预设激活时重新应用预设。
- `toggle_fullscreen()` - 打开或关闭全屏预设。
- `grow_focused()` / `shrink_focused()` - 调整聚焦面板的高度。
- `resize_panel_divider()` - 把增量应用到给定面板上方的分隔线（鼠标拖动处理器使用）。

#### 1.3 自动堆叠

通过 `LayoutManager::add_panel()` 添加新面板时：

```rust
let new_width_if_split = available_width / (num_groups + 1);

if new_width_if_split < config.auto_stack_threshold {
    // 在活动组中垂直堆叠（带高度缓存的拆分布局）
    active_group.add_panel(panel);
} else {
    // 创建新的水平组
    let new_group = PanelGroup::new(panel);
    panel_groups.push(new_group);
}
```

**默认阈值：** `auto_stack_threshold = 80` 字符

这确保面板始终有足够的空间保持可用性。

### 2. 面板系统

#### 2.1 Panel Trait

**位置：** `crates/core/src/lib.rs`

所有面板实现 `Panel` trait，该 trait 定义了交互式终端面板的接口：

```rust
pub trait Panel {
    /// 渲染面板内容
    fn render(
        &mut self,
        area: Rect,                // 可用渲染区域
        buf: &mut Buffer,          // Ratatui 缓冲区
        is_focused: bool,          // 此面板是否有焦点？
        panel_index: usize,        // 面板索引用于标识
        state: &AppState,          // 共享应用状态
    );

    /// 处理键盘输入
    fn handle_key(&mut self, key: KeyEvent) -> Result<()>;

    /// 处理鼠标输入
    fn handle_mouse(&mut self, mouse: MouseEvent, panel_area: Rect) -> Result<()>;

    /// 获取面板标题（显示在标题栏中）
    fn title(&self) -> String;

    /// 检查是否为欢迎面板（打开其他面板时自动关闭）
    fn is_welcome_panel(&self) -> bool { false }

    /// 获取要打开的文件（用于请求打开文件的面板）
    fn take_file_to_open(&mut self) -> Option<PathBuf> { None }

    /// 获取新面板的工作目录
    fn get_working_directory(&self) -> Option<PathBuf> { None }

    /// 获取模态框请求（用于打开模态框的面板）
    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> { None }
}
```

#### 2.2 面板实现

**文件管理器** (`crates/panel-file-manager/src/lib.rs`)
- 浏览文件和目录
- 文件操作（创建、删除、复制、移动）
- Git 状态集成
- 剪贴板支持
- 批量操作
- 拖放选择

**编辑器** (`crates/panel-editor/src/lib.rs`)
- 带撤销/重做的文本编辑
- 通过 tree-sitter 实现语法高亮（15+ 种语言）
- 带内嵌查找栏的搜索和替换
- 行号、光标位置、自动换行
- 单词导航（Ctrl+Left/Right）、段落/符号导航（Ctrl+Up/Down）
- 带括号分割缩进的自动缩进
- 自动关闭括号和引号
- 行号中的 Git 差异可视化
- LSP 集成（补全、悬停、跳转到定义）
- 文件保存，支持另存为和可执行复选框

**终端** (`crates/panel-terminal/src/lib.rs`)
- 完整的 PTY（伪终端）支持
- Shell 集成
- 回滚缓冲区
- 跨滚动缓冲区和可见缓冲区的文本搜索（`Searchable` trait）
- ANSI 颜色支持
- 调整大小处理

**日志查看器** (`crates/panel-misc/src/journal.rs`)
- 应用日志查看器
- 面板信息
- 系统资源监控

**Git 状态** (`crates/panel-git-status/src/lib.rs`)
- 仓库状态概览
- 文件暂存/取消暂存
- 分支切换
- 创建提交

**Git 日志** (`crates/panel-git-log/src/lib.rs`)
- 带 ASCII 图形的提交历史
- 查看差异
- 复制提交哈希

**Git 差异** (`crates/panel-git-diff/src/lib.rs`)
- 并排或内联差异视图
- 语法高亮的差异

**诊断** (`crates/panel-diagnostics/src/lib.rs`)
- LSP 诊断显示
- 错误/警告导航

**操作** (`crates/panel-operations/src/lib.rs`)
- 后台文件操作跟踪
- 复制/移动/删除的进度显示

**大纲** (`crates/panel-outline/src/lib.rs`)
- 基于 tree-sitter 查询的代码结构导航
- 与活动编辑器同步的符号列表
- 按 Enter 导航到符号
- 光标跟踪和实时更新

**图片** (`crates/panel-image/src/lib.rs`)
- 原生图片渲染（Kitty、iTerm2、Sixel 协议）
- 回退到 Unicode 块字符

**帮助** (`crates/panel-misc/src/help.rs`)
- 基于快捷键配置动态生成的帮助内容
- 带全宽布局的伪图形表格
- 支持键盘和鼠标滚动
- 打开其他面板时自动关闭

### 3. 事件处理

#### 3.1 事件循环

**位置：** `crates/app/src/app/mod.rs`

主事件循环结构：

```rust
while !state.should_quit {
    match event_handler.next()? {
        Event::Key(key) => self.handle_key_event(key)?,
        Event::Mouse(mouse) => self.handle_mouse_event(mouse)?,
        Event::Resize(w, h) => state.update_terminal_size(w, h),
        Event::Tick => {
            // 周期性更新
            self.update_panels_tick()?;
            self.system_monitor.update(&mut self.state);
        }
    }
    self.render(terminal)?;
}
```

**事件类型：**
- **Key** - 键盘输入（快捷键、文字输入）
- **Mouse** - 鼠标点击、拖拽、滚动
- **Resize** - 终端大小变化
- **Tick** - 周期性定时器（资源监控、面板更新）

#### 3.2 键盘处理器

**位置：** `crates/app/src/app/key_handler.rs`

按优先级处理键盘输入：

1. **模态框优先捕获输入**（如果已打开）
2. **全局快捷键**（Alt+M、Alt+H、Alt+Q 等）
3. **面板管理**（Alt+Left/Right、Alt+Up/Down、Alt+X 等）
4. **活动面板**（通过 `panel.handle_key()`）

**西里尔文支持：**
通过 `termide_keyboard::translate_hotkey()` 进行键盘布局翻译，使快捷键在俄语键盘布局下也能工作。

#### 3.3 鼠标处理器

**位置：** `crates/app/src/app/mouse_handler.rs`

处理鼠标输入：

**面板标题栏：**
- 点击 `[≡]` 按钮 → 打开面板操作上下文菜单（关闭 / 拆分 / 合并 / 移动）
- 点击标题区域 → 激活面板（双击文件管理器 → 目录选择器）
- **拖拽标题区域** → 双模式手势：
  - 在源组所在列内释放 → 垂直 resize：被拖面板上方的分隔线跟随光标。
  - 在另一列或两列之间释放 → 移动面板：ghost 跟随光标，drop 区高亮；释放在另一个面板的标题上则插入到该组，释放在两组之间则创建新组。`Escape` 取消。

**面板内容：**
- 点击转发到 `panel.handle_mouse()`
- 每个面板处理自己的鼠标交互

**菜单栏：**
- 点击菜单项进行激活

#### 3.4 模态框处理器

**位置：** `crates/app/src/app/modal_handler.rs` 和 `crates/modal/src/`

处理交互式模态对话框：

**模态框类型**（crate `termide-modal`）：
- **Input** — 文本输入（文件名、目录名等）
- **Confirm** — 是/否确认
- **Select** / **EditableSelect** — 从选项中选择（可编辑）
- **Choice** — 水平选择按钮
- **Info** — 信息显示，**支持内容滚动**（脚本报告、系统信息）；右侧边框带滚动条，支持 `↑↓/PageUp/PageDown/Home/End` 及鼠标滚轮
- **InfoAction** — 带附加操作按钮的信息窗口
- **Settings** — 采用 **侧边栏布局** 的全屏配置模态。拆分为 `crates/modal/src/settings/` 下的若干子模块：
  - `settings.rs` — `SettingsModal` 结构、渲染、键盘/鼠标处理
  - `settings/fields.rs` — 声明性字段数据（`FieldType`、`FieldDescriptor`、`ContentRow`，以及辅助函数 `fields_for_tab`、`get_field_value`、`toggle_field`、`cycle_enum_*`）
  - `settings/kb.rs` — 键绑定表和宏（`kb_get!`/`kb_set!`、`KB_SECTIONS`、`kb_binding_names`、`get/set_kb_value`、`format_key_event`）
- **Progress** — 长时间操作的进度条
- **Commit** / **Conflict** / **RenamePattern** / **Sessions** / **DirectoryPicker** / **SaveAs** / **BookmarkAdd** / **Calendar** / **CommandPalette** / **ScriptCreate** — 针对具体场景的专用对话框

共用工具集中在 `crates/modal/src/base.rs`（`render_modal_block`、`render_modal_frame`、`button_style`、`CursorNavigation` trait）。

**输入捕获：**
模态框打开时，键盘输入首先传递给模态框。Escape 关闭模态框。

### 4. 渲染管线

#### 4.1 主渲染

**位置：** `crates/ui-render/src/layout.rs`

渲染流程：

```rust
fn render_main_area(frame, layout_manager, state) {
    // 1. 计算列宽（按比例分配，受 min_panel_width 约束）。
    let horizontal_chunks = calculate_horizontal_layout();

    // 2. 对每一列，从该组缓存的高度（或等量分配的回退值）
    //    推导出垂直约束。
    for group in groups {
        let vertical_chunks =
            termide_layout::compute_vertical_constraints(group, area_height);

        // 3. 渲染每个面板。height >= 2 的面板渲染完整内容和边框
        //    （包含底部）；被压到 1 行的面板回退为仅渲染标题。
        let mut prev_was_accordion = false;
        for (idx, panel) in group.panels().enumerate() {
            let area = vertical_chunks[idx];
            let omit_bottom_border = area.height < 2;
            if !omit_bottom_border {
                render_expanded_panel(panel, area, omit_bottom_border, ...);
            } else {
                render_collapsed_panel(panel, area, ...);
            }
            // 顶部行的角字符根据上下文选择：└┘ 用于
            // accordion 形式的最后一个面板（视觉上封闭组），
            // ├┤ 当当前与上一个均为 accordion（连贯性），
            // 否则使用普通的 ┌┐。
            patch_top_corners(area, idx, prev_was_accordion, &group);
            prev_was_accordion = area.height < 2;
        }
    }

    // 4. 渲染模态框（如果打开）。
    if let Some(modal) = state.active_modal {
        render_modal(modal, ...);
    }
}
```

`height >= 2` 的每个面板都绘制**完整**边框；两个相邻面板之间显示两行连续的边框（上方面板底部 + 下方面板顶部）—— 聚焦面板因此在所有四边都被强调色完整地框住。`height == 1` 的 accordion 面板仍只绘制顶部边框，其顶部角字符根据上方内容以及它是否为组内最后一个面板，在 `┌┐`、`├┤` 和 `└┘` 之间切换。

#### 4.2 面板渲染

**位置：** `crates/ui-render/src/panel.rs`

**完整面板（height ≥ 2 行）：**
- 带 `[≡]` 操作按钮、emoji 与标题的边框（例如 `[≡] 📁 文件`）
- 完整的内容区域
- 内容超出区域时可滚动
- `height >= 2` 的面板始终渲染底部边框；两个相邻面板之间出现两行连续的边框，使聚焦面板被强调色完整框住

**折叠面板（height = 1 行，仅在压到最小时）：**
- 仅标题栏：`─[≡] 📁 文件 ─────`
- 占用 1 行
- 点击标题以聚焦面板；`Alt+F11` 或 `Alt+Shift+=` 可放大它

**图标模式：**
面板标题根据面板类型显示 emoji 图标（📁 文件管理器、💻 终端、📝 编辑器等）。通过 `[general]` 中的 `icon_mode` 配置：
- `auto`（默认）— 终端支持时显示 emoji，否则仅显示 `[≡]`
- `emoji` — 始终显示 emoji 图标
- `unicode` — 无图标、无箭头，仅 `[≡]`

**Drag Overlay：**
拖拽面板顶部边框时，`render_drag_overlay()`（在 `src/ui.rs`）在主面板渲染之后、dropdowns/modals 之前运行。它高亮 drop 目标（`IntoGroup` 是目标面板的顶部边框，`NewGroup` 是组间的垂直线）并在光标下绘制 ghost 图标。命中测试重用 `termide_layout` 中的自由函数 `calculate_panel_rects` / `compute_drop_target`，因此鼠标处理器和渲染器对几何一致。

**边框渲染：**
边框和按钮由 `panel_rendering.rs` 绘制，然后面板的 `render()` 方法在内部区域绘制内容。

### 5. 状态管理

#### 5.1 AppState

**位置：** `crates/state/src/`（拆分为 `batch.rs`、`layout.rs`、`operations.rs`、`pending_action.rs`、`ui.rs`）

中央状态容器：

```rust
pub struct AppState {
    pub theme: Theme,                    // 当前主题
    pub terminal: TerminalInfo,          // 宽度、高度
    pub config: Config,                  // 用户配置
    pub should_quit: bool,               // 退出标志
    pub batch_operation: Option<BatchOp>, // 待处理的批量操作
    pub active_modal: Option<ActiveModal>, // 当前模态框
    pub error_message: Option<String>,   // 要显示的错误
    pub fs_watcher: Option<Watcher>,     // 文件系统监视器
    // ... 其他字段
}
```

**线程安全：**
大部分状态为单线程（TUI 在主线程运行）。文件系统监视器使用通道进行跨线程通信。

#### 5.2 配置

**位置：** `crates/config/src/lib.rs`

从 TOML 加载的用户配置：

```rust
pub struct Config {
    pub general: GeneralSettings,         // 主题、语言、icon_mode、vim_mode、快捷键
    pub editor: EditorSettings,           // 制表符大小、自动换行、git diff、自动缩进
    pub file_manager: FileManagerSettings, // 扩展视图宽度、快捷键
    pub git_status: GitStatusSettings,    // 快捷键
    pub terminal: TerminalSettings,       // 快捷键
    pub lsp: LspSettings,                // LSP 服务器、补全、悬停
    pub logging: LoggingSettings,         // 日志级别、资源监控间隔
    pub vfs: VfsSettings,                // VFS 连接超时
}
```

**默认位置：**
- Linux: `~/.config/termide/config.toml`
- macOS: `~/Library/Application Support/termide/config.toml`
- Windows: `%APPDATA%\\termide\\config.toml`

### 6. 主题系统

**位置：** `crates/theme/src/lib.rs`

**内置主题：** 38 款主题（Dracula、Nord、Monokai、Matrix、Pip-Boy 等）

**自定义主题：** 从 `~/.config/termide/themes/*.toml` 加载

**主题结构：**
```rust
pub struct Theme {
    pub fg: Color,                // 前景色
    pub bg: Color,                // 背景色
    pub accented_fg: Color,       // 聚焦元素
    pub disabled: Color,          // 禁用/非聚焦
    pub selected_bg: Color,       // 选择背景
    // ... 语法高亮颜色
}
```

**加载优先级：**
1. 用户主题（在配置目录中）
2. 内置主题
3. 回退到默认

### 7. 国际化

**位置：** `crates/i18n/`

通过编译时加载的基于 TOML 的翻译文件实现语言支持：

```
crates/i18n/
├── src/
│   ├── lib.rs      # 翻译 trait 和运行时
│   └── runtime.rs  # 语言检测和加载
└── i18n/           # 翻译文件
    ├── bn.toml     # 孟加拉语
    ├── de.toml     # 德语
    ├── en.toml     # 英语
    ├── es.toml     # 西班牙语
    ├── fr.toml     # 法语
    ├── hi.toml     # 印地语
    ├── id.toml     # 印尼语
    ├── ja.toml     # 日语
    ├── ko.toml     # 韩语
    ├── pt.toml     # 葡萄牙语
    ├── ru.toml     # 俄语
    ├── th.toml     # 泰语
    ├── tr.toml     # 土耳其语
    ├── vi.toml     # 越南语
    └── zh.toml     # 中文
```

**语言：** 支持 15 种（孟加拉语、中文、英语、法语、德语、印地语、印尼语、日语、韩语、葡萄牙语、俄语、西班牙语、泰语、土耳其语、越南语）

**检测顺序：**
1. `config.language` 设置
2. `LANG` / `LC_ALL` 系统变量
3. 默认为英语

### 8. 关键依赖

**Ratatui** - 终端 UI 框架
- 基于组件的渲染
- 缓冲区系统实现高效更新
- 布局系统（Rect、Constraints）

**Crossterm** - 跨平台终端控制
- 事件处理（键盘、鼠标、调整大小）
- 终端控制（光标、颜色、清屏）
- Raw 模式管理

**Tree-sitter** - 语法高亮
- 15+ 种语言的解析器生成器
- 增量解析提升性能
- 用于语法高亮的查询系统

**Ropey** - 文本缓冲区
- 高效的基于行的文本存储
- UTF-8 感知
- 内部采用 Gap buffer

**Portable-pty** - PTY 实现
- 跨平台伪终端
- Shell 集成
- 调整大小支持

**Sysinfo** - 系统监控
- CPU 使用率
- 内存使用量
- 磁盘空间

## 设计决策

### 为什么采用拆分布局加全屏预设？

**问题：** 终端空间有限，多面板 IDE 常常显得拥挤；但「一个展开、其余折叠」的二元布局又会丢掉用户希望同时看到两三个面板（且高度可调）的场景。

**解决方案：** 每组面板高度可调，并提供一键全屏预设：
- 默认每个面板都可见；用户通过鼠标拖动（面板下边框或标题栏）、`Alt+Shift+=` / `Alt+Shift+-` 或它们的任意组合来调整高度。
- `Alt+F11` 切换「聚焦面板全屏」预设，模拟旧的手风琴视图（一个面板占满整列，其余缩为一行），先前的高度被保存以便瞬间恢复。
- 高度被缓存，并在终端尺寸变化时按比例重缩放。
- 终端过窄时（`min_panel_width`）仍会自动堆叠到同一组中。

### 为什么选择动态面板？

**优势：** 用户可以根据需要打开任意数量的面板：
- 多个编辑器用于不同文件
- 多个终端用于不同任务
- 多个文件管理器用于不同目录

**挑战：** 高效管理多个面板
- 全屏预设按需提供干净的「单面板视图」
- 快捷键提供快速导航
- 欢迎界面自动关闭

### 为什么选择基于 Trait 的面板？

**灵活性：** 无需更改核心代码即可添加新面板类型
- 实现 `Panel` trait
- 添加到面板创建逻辑
- 与现有布局系统协同工作

**多态性：** `Box<dyn Panel>` 允许异构集合
- 单个 `Vec<Box<dyn Panel>>` 容纳所有面板类型
- 统一的渲染和事件处理
- 动态分发的开销对 TUI 来说可忽略不计

## 性能特征

**渲染：** O(n)，n = 可见组中的面板数量
- height ≥ 2 的面板渲染完整内容；被压到 1 行的面板仅渲染标题栏
- 每个 height ≥ 2 的面板都绘制自己的底部边框；连接处显示两行连续的边框，让聚焦面板被强调色完整框住

**事件处理：** 大多数操作 O(1)
- 直接索引访问聚焦面板
- 快捷键绑定使用哈希表查找

**内存：** 与面板数量线性相关
- 每个面板拥有自己的状态
- 共享的 AppState 很小
- 无过度克隆（使用引用）

**文件操作：** 尽可能异步
- 文件系统监视器使用独立线程
- 防抖避免过多更新

### 异步管道

一些启动和热路径操作过去会阻塞渲染循环，现在改为在工作线程上运行，并从每个
面板的 `tick()` 中轮询。模式在所有情况下都相同：启动一个工作线程，在面板上停放
一个 `mpsc::Receiver`，当 `try_recv()` 返回时换入结果。下表指出每个管道所在位置。

| 管道                             | 工作线程位置                                                     | 轮询于                                                                        |
|----------------------------------|-----------------------------------------------------------------|-------------------------------------------------------------------------------|
| FileManager 初次目录读取         | `crates/panel-file-manager/src/lib.rs` (`start_async_reload`)   | 应用每帧 `check_background_panel_updates` 中的 `check_async_reload`            |
| FileManager 子树展开             | `crates/panel-file-manager/src/lib.rs` (`start_listing`)        | 同一路径的 `poll_pending_expansions`；占位行在解析前显示 `…`                   |
| FileManager 每条目 git 状态      | `crates/panel-file-manager/src/git_status.rs`                   | `check_git_status_async`；若目录读取抢先，`apply_git_statuses` 会重新应用      |
| Git 状态/日志面板刷新            | `crates/panel-git-status/src/lib.rs`、`panel-git-log/src/lib.rs`| 各面板 `tick` 中的 `poll_refresh`                                             |
| Git 子模块发现（RepoManager）    | `crates/git/src/repo_manager.rs` (`spawn_submodule_walk`)       | git 面板 `tick` 中的 `RepoManager::poll`                                       |
| 会话恢复——并行构建面板           | `crates/app/src/layout_session.rs`（每面板 `construct_panel`）  | 启动后同步 join，因此最慢的面板仍决定首帧                                     |
| 监视器仓库注册                   | `crates/watcher/src/lib.rs` (`watch_repository`)                | 主循环中的 `poll_pending`；inotify 每帧按 `INSTALL_CHUNK` 分块安装             |
| 目录大小遍历（宽视图）           | `crates/panel-file-manager/src/utils.rs` (`shared_dir_size_cache`) | 对共享缓存逐帧 `try_recv`；每次遍历有预算限制                              |

SFTP/FTP 后端使用不同的模式——一个专用的 tokio 运行时拥有连接和一个“分块即命令”
actor（见 `crates/vfs/src/sftp.rs`）。同步工作线程驱动分块循环，并在分发之间
轮询暂停/取消标志，因此暂停的传输会让 actor 空闲以服务其他面板的元数据请求。

### 8. 会话管理

**位置：** `crates/session/src/lib.rs`

会话持久化允许保存和恢复面板布局：

**存储位置：**
- Linux: `~/.local/share/termide/sessions/<project_path>/session.toml`
- macOS: `~/Library/Application Support/termide/sessions/<project_path>/session.toml`

**功能特性：**
- 退出时自动保存会话
- 启动时恢复面板布局
- 通过菜单切换会话（在不同项目之间切换）
- 会话保留，自动清理旧会话

**会话文件格式：**
```toml
focused_group = 0

[[panel_groups]]
expanded_index = 1            # 组内聚焦面板
width = 80                    # 列宽（None = 自动分配）
split_heights = [12, 6]       # 每面板高度（无缓存时省略）
fullscreen_cache = [10, 8]    # Alt+F11 关闭时恢复的高度

[[panel_groups.panels]]
type = "file_manager"
path_or_url = "/home/user/project"

[[panel_groups.panels]]
type = "editor"
path = "/home/user/project/main.rs"
```

旧会话中的 `mode = "accordion"` 字段仍会被读取，并在加载时一次性迁移为全屏预设（当前代码不再写入该字段）。

### 9. VFS（远程文件系统）

**位置：** `crates/vfs/src/`

一个纯 Rust 的 VFS 层让远程服务器在应用的其余部分看来像本地目录。无需原生
OpenSSL 或 libssh——SFTP 运行在 `russh` + `russh-sftp` 上，FTPS 运行在 `rustls`
上。构建可在 Alpine / musl 上静态完成。

**支持的协议：** `sftp://`、`ftp://`、`ftps://`（URL 解析也识别 `smb://` /
`nfs://`，但尚未提供相应的 provider）。

**关键组件：**
- **`VfsProvider` trait**（`crates/vfs/src/traits.rs`）——供 FileManager、
  file-ops、编辑器使用的抽象文件系统 API：`list_dir`、`read_file`、`write_file`、
  `delete`、`upload`、`upload_with_progress` 等。每次调用返回一个
  `VfsOperation<T>`，调用方轮询其 receiver——没有阻塞变体。
- **`VfsManager`**（`crates/vfs/src/lib.rs`）——按 `(scheme, host, port, user)`
  为键的 provider 缓存；淘汰其 actor 已死的条目。
- **SFTP actor**（`crates/vfs/src/sftp.rs`）——单个 tokio 任务拥有
  `russh-sftp` 会话，处理小的原子命令（`OpenRead` / `ReadChunk` / `WriteChunk` /
  `CloseHandle` / `Stat` / `ListDir` / `MkdirRecursive` / …）。分块循环位于同步
  工作线程一侧，在分发之间轮询暂停/取消。
- **URL 解析**（`crates/vfs/src/url.rs`）——UTF-8 往返并对路径做百分号解码，
  使非 ASCII 文件名得以保留。
- **认证**——SSH agent → `~/.ssh/config` 的 `IdentityFile` → 默认密钥
  （`id_ed25519` / `id_rsa` / `id_ecdsa` / `id_dsa`）→ 密码；这四种都可作为显式的
  `AuthMethod` 变体选择，供不想使用自动链的用户。

**取消安全：** 传输在分块之间取消；部分文件会弹出“删除部分上传？”模态框，使
服务器不会留下滞留的字节。同连接内的重命名保持在服务器端（无需先下载再上传）。
用户视角见 `doc/zh/vfs.md`，取消流程见 `doc/zh/operations.md`。

## 未来架构考虑

**潜在改进：**

1. **异步面板**
   - 长时间运行的操作（搜索、编译）不阻塞 UI
   - 带进度指示器的后台任务

2. **插件系统**
   - 动态加载面板
   - 用户定义的面板类型
   - 脚本集成（Lua、Python）

3. **网络面板**
   - SSH 终端面板
   - 远程文件浏览器
   - 协作编辑

## 调试架构

**日志系统：**
- 所有日志写入配置目录中的 `termide.log`
- 级别：INFO、ERROR、DEBUG
- 时间戳和组件前缀
- 日志轮转防止无限增长

**调试面板：**
- 应用状态实时查看
- 最近的日志条目
- 面板检查
- 性能指标

**Panic 处理：**
- Panic 时恢复终端
- 将 panic 信息写入日志
- 向用户显示错误消息

## 安全考虑

**终端注入：**
- 终端面板中过滤 ANSI 转义序列
- Shell 执行前对用户输入进行清理

**文件操作：**
- 防止符号链接攻击
- 路径遍历检查
- 操作前进行权限检查

**资源限制：**
- 编辑器文件大小限制（100 MB）
- 终端回滚缓冲区限制
- 日志轮转防止磁盘耗尽

## 总结

TermIDE 的架构优先考虑：
- **灵活性** - 动态面板系统适应用户需求
- **高效性** - 可调拆分布局加一键全屏预设，在不放弃多面板视图的前提下最大化可用空间
- **可扩展性** - 基于 Trait 的设计便于扩展
- **健壮性** - 防御性编程防止崩溃
- **性能** - 高效的渲染和事件处理

带全屏预设的拆分布局是 TermIDE 区别于传统多面板终端应用程序的关键创新。
