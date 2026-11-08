# tmux-workspace-expose

用卡片浏览 tmux 的 Session、Window 和 Pane。在切换前就能看见窗口的内容、分屏位置，以及哪些 Agent 正在等你处理。

![tmux-workspace-expose 实机演示](assets/recording.gif)

[总览截图](assets/screenshot.png) · [设置截图](assets/custom-example.png)

## 功能概览

- **Session 总览**显示每个 Session 及其 Window 缩略图。进入 **Window 总览**后，每张卡片都显示 Pane 的可见画面和分屏线。
- **可滚动网格**分别限制 Session 和 Window 每屏显示的行数、列数。超出一屏时可用方向键或鼠标滚轮查看。
- **工作区管理**支持模糊搜索、重命名、确认关闭，以及一键直达当前 Session 的 Window 总览。
- **中英文界面**可以在设置里切换，弹窗布局、配色、刷新和 Agent 排序也能直接修改。
- **Agent 状态**可在卡片上显示运行、等待和需要处理的状态，并把需要关注的会话排到前面。

## 安装

需要 Bash、Cargo 和 tmux。本项目在 Linux 的 Alacritty 和 tmux 3.4 上验证过。

```bash
git clone https://github.com/Nobody-Cares-TXSL/tmux-workspace-expose.git
cargo install --locked --path ./tmux-workspace-expose
```

Cargo 包名为 `tmux-workspace-expose`，安装后的命令为 `tmux-expose`。确保 tmux 服务能从 PATH 找到它。使用 [TPM](https://github.com/tmux-plugins/tpm) 时，在 tmux 配置中加入：

```tmux
set -g @plugin 'Nobody-Cares-TXSL/tmux-workspace-expose'
```

重新加载配置后，按 tmux 前缀键再按大写 `I` 安装插件。已有上游插件配置时，替换原来的 `cesarferreira/tmux.expose` 项。

更新时，TPM 会更新插件脚本。要同步更新 `tmux-expose` 二进制，请从更新后的项目目录再次运行 `cargo install --locked --path .`。

手动加载时，用下面的方式执行克隆目录中的插件脚本，并把示例路径改成实际位置：

```tmux
run-shell 'bash ~/src/tmux-workspace-expose/tmux.expose.tmux'
```

如果 tmux 找不到二进制，可在加载插件前指定路径：

```tmux
set -g @tmux-expose-command '~/.cargo/bin/tmux-expose'
```

## 快捷键

| 按键 | 动作 |
| --- | --- |
| `Alt+e` | 打开 Session 总览，默认再次按下关闭弹窗 |
| `Alt+w` | 直接打开当前 Session 的 Window 总览 |
| `Ctrl+,` / `Shift+F12` | 打开设置 |
| 方向键 / 鼠标滚轮 | 选择卡片、逐行滚动 |
| `Tab` / `Shift+Tab` | 在 Window 总览中向前或向后选择 |
| `Enter` | 进入选中的 Session，或切换到选中的 Window |
| `q` | 浏览时关闭弹窗 |
| `Esc` | 返回 Session 总览，或取消当前输入 |
| `/` | 开始模糊搜索 Session |
| `r` | 重命名选中的 Session 或 Window，按 `Enter` 保存 |
| `d` | 显示关闭确认，按 `Enter` 关闭，按 `Esc` 取消 |

在搜索框输入 `pf` 可以匹配 `project-frontend`。开启 Vim 导航时，用 `hjkl` 移动；搜索时按 `Esc` 会结束编辑并保留筛选结果。

如果同一个 Window 链接到了多个 Session，关闭它会移除所有链接。

### Alacritty 的 Ctrl+, 映射

tmux 3.4 可直接用 `Shift+F12` 打开设置。如需在 Alacritty 使用 `Ctrl+,`，在 `alacritty.toml` 中加入：

```toml
[[keyboard.bindings]]
key = ","
mods = "Control"
chars = "\u001B[24;2~"
```

## 设置项与取值

打开设置后，按 `Tab` 或点击页签切换页面，用 `↑` / `↓` 或鼠标选中字段。可以直接输入，也可以用 `←` / `→` 调整。按 `Enter` 保存，按 `Esc` 放弃修改。

### 网格

| 设置项 | 取值 / 默认 | 作用 |
| --- | --- | --- |
| Session 列数 | 1–12 / 2 | 总览每行最多显示多少张 Session 卡片。 |
| Session 行数 | 1–12 / 2 | 总览一屏最多显示几行。 |
| Window 列数 | 1–12 / 2 | Window 卡片每行的上限，Session 卡片里的缩略图也使用这个上限。 |
| Window 行数 | 1–12 / 2 | Window 卡片每屏的行数上限。 |
| 卡片间距 | 0–3 / 1 | 相邻卡片之间留多少终端字符格。 |

例如 Session 设为 2 列、2 行时，一屏最多显示 4 张卡片。终端过窄或过矮时，一屏会显示更少，剩余卡片仍可滚动查看。

### 弹窗

| 设置项 | 取值 / 默认 | 作用 |
| --- | --- | --- |
| 启动键 | `M-e` | 打开 Session 总览的按键。`M-` 表示 Alt，`C-` 表示 Ctrl。`prefix` 按键表也可用单个字母。`M-w` 和 `C-c` 保留。 |
| 按键表 | `root`、`prefix` / `root` | `root` 直接按键，`prefix` 先按 tmux 前缀键。 |
| 宽度 | 1–100% 或 1–1000 个字符格 / `100%` | 决定弹窗横向占用的空间，例如 `80%`。 |
| 高度 | 1–100% 或 1–1000 个字符格 / `100%` | 决定弹窗纵向占用的空间。 |
| 停靠 | `center`、`top`、`bottom`、`left`、`right` / `center` | 决定弹窗居中或靠哪一边。 |
| 弹窗背景 | 颜色或 `default` / `default` | 设置 tmux 弹窗的背景。 |
| 弹窗边框 | 颜色或 `default` / `default` | 设置 tmux 弹窗外框的颜色。 |
| Vim 导航 | `on`、`off` / `off` | 开启后支持 `hjkl` 和保留筛选的搜索模式。 |

宽高、停靠、背景和边框在下次打开弹窗时生效。独立的设置快捷键始终打开全尺寸设置页，方便把过小的尺寸改回来。

### 配色

颜色可以输入名称、`colour0` 到 `colour255`，或 `#RRGGBB`。

| 设置项 | 默认 | 作用 |
| --- | --- | --- |
| 选中卡片 | `yellow` | 当前选中项的高亮颜色。 |
| 当前 Session | `green` | 当前连接的 Session 卡片颜色。 |
| 普通卡片 | `white` | 其他 Session 卡片的标题颜色。 |
| Agent 需处理 | `#ff5555` | Agent 遇到需要人工处理事项时的标记颜色。 |
| Agent 等待 | `#ffb86c` | Agent 等待回复时的标记颜色。 |
| Agent 运行中 | `#8be9fd` | Agent 工作时的标记颜色。 |

### 行为

| 设置项 | 取值 / 默认 | 作用 |
| --- | --- | --- |
| 界面语言 | `zh`、`en` / `zh` | 切换简体中文或英文，保存后立即更新。 |
| Agent 优先排序 | `on`、`off` / `on` | `on` 把需要处理或等待回复的 Session 排到前面，`off` 保留 tmux 的列表顺序。Agent 标记仍会显示。 |
| 布局刷新 | 1–60000 毫秒 / `1500` | 多久重新读取 Session、Window、Pane 的结构和 Agent 状态。 |
| 后台 Pane 刷新 | 1–60000 毫秒 / `1500` | 未选中 Session 的 Pane 预览多久更新一次。 |
| 活跃 Pane 刷新 | 1–60000 毫秒 / `300` | 当前选中 Window 的活跃 Pane 预览多久更新一次。 |
| 每批最多抓取 | 1–32 / `4` | 一批最多同时抓取多少个 Pane 的画面。 |

网格设置保存在 `$XDG_CONFIG_HOME/tmux-expose/grid.conf`，其余保存在同目录下的 `appearance.conf`。未设置 `XDG_CONFIG_HOME` 时使用 `~/.config`。已保存的值优先于对应的启动参数；`--columns` 仍可覆盖当次启动的 Session 列数。

终端缩放后，弹窗会按新尺寸打开并恢复选中项。读取布局或画面失败时，插件会保留可用的预览并重试。

## Agent 状态提醒

在对应 Pane 中运行以下命令可以手动标记。需要自动更新时，由 Agent 的 hook 调用相应命令。

```bash
tmux-expose agent-status working
tmux-expose agent-status waiting
tmux-expose agent-status attention
tmux-expose agent-status clear
```

例如，在一个 Pane 里标记 `waiting`，再按 `Alt+e`，对应的 Session 卡片会提示它在等待回复。处理完后，在原 Pane 运行 `tmux-expose agent-status clear` 清除标记。`attention` 适合权限确认、缺少输入等需要人介入的事件。Agent 状态由显式命令写入，插件不会从终端文本猜测状态。

## 来源与许可证

本项目基于 Cesar Ferreira 的 [tmux.expose](https://github.com/cesarferreira/tmux.expose) 二次开发，保留上游版权声明和 [MIT 许可证](LICENSE)。
