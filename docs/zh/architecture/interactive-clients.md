# 交互客户端重构计划 — server、Rust TUI 与 WebUI

首个交付目标是一个能日常使用的终端会话。从 CLI 发起任务，关掉终端，再开一个 TUI 窗口，找到同一个 Agent，看到进度并继续发消息。浏览器也能接入同一 Session。

## 从哪里改起

上一轮重构已让 CLI 和 server 都通过公开 Session SDK 执行。已有测试证明 HTTP 客户端断开后，Agent 仍会完成工具调用并提交结果。现在 CLI 依然自己打开 Session。server 拒绝给运行中的 Agent 投递新输入，列出 Agent 时读取的是离线日志快照。SDK 目前在 Turn 开始时接纳队列输入。本轮改动集中在这几处。[CLI 执行入口](../../../crates/kuku-cli/src/commands/run.rs) · [server 运行管理](../../../crates/kuku-server/src/run_manager.rs) · [Agent 列表](../../../crates/kuku-server/src/routes/sessions.rs) · [SDK 驱动](../../../crates/kuku/src/session/runtime/driver.rs) · [断开连接测试](../../../crates/kuku-server/tests/session_execution.rs)

## 职责与客户端契约

| 归属 | 职责 |
|---|---|
| Session SDK | 每个 Session 一个 writer，每个 Agent 一条执行链。负责输入接纳、准备模型请求、工具收尾、等待、取消和恢复。嵌入方可直接调用 SDK。 |
| server | 为产品客户端持有打开的 Session。在服务端机器解析工作目录和模型配置、读取凭据、执行工具。通过 HTTP 提供持久命令、Agent 实时状态和可续读的观察接口。所属工作收尾后可以释放空闲运行资源。 |
| 客户端 | 本地和远程使用同一份 HTTP 契约。CLI 与 TUI 共用一个 Rust 客户端处理投递、回执、观察和重连，WebUI 按相同契约接入。选中的 Agent、滚动位置、草稿和快捷键归各自窗口。 |

默认复用该用户的本地 server，找不到时自动启动一个独立的回环服务。两个客户端使用相同服务配置同时启动，即使位于不同工作目录，也应连接到同一实例。明确指定远程 server 时连接该地址，并显示相应的连接或认证错误。

命令按 Session 和 Agent 寻址。持久回执表示服务端已经收下命令。丢失连接后以同一命令 ID 重发相同意图，应得到原回执。复用 ID 却更换接收者、正文或投递时机时报告冲突。打断绑定到用户看到的那次执行，防止旧窗口误停后来的 Turn。回答问题或授权要带对应待决请求的标识。

连接活跃 Agent 时取得实时状态和已提交事件的游标。新客户端可以读历史，再从快照边界接收新提交。流式片段使用有界缓冲，断线后从已提交事件补齐结果。列出、预览或切换 Agent 都只改变显示。最后一个窗口退出后，活动任务继续在 server 上执行。server 自身重启时恢复已提交的状态和待决请求，按 SDK 契约标明中断或结果未知的副作用。[Session SDK](../reference/sdk.md) · [现有事件接口](../../../crates/kuku-server/src/routes/events.rs)

## 终端交互

主界面用单列滚动对话、标明接收者的固定多行输入框，以及一行状态。工具输出默认显示摘要，按需展开。打开新窗口时留白，首次发送才创建 Session。用类似 `/resume` 的命令打开 Session 列表、预览历史。进入 Session 后只用一个 Agent 选择器，切换后显示该 Agent 的对话，下次主动发送才投递给它。两个窗口可以分别看不同 Agent。[Pi 交互参考](https://github.com/earendil-works/pi/blob/e56d35ce5eceedfa16eaf13a5f01a1f7b7271253/packages/coding-agent/docs/usage.md)

| 操作 | 目标行为 |
|---|---|
| `Enter` 发送 | 空闲 Agent 开始工作。忙时先持久收件，等在途响应及已派发工具收尾，在当前 Turn 的下一次模型请求准备前插入。若当前 Turn 已自然结束，该输入转入下一 Turn。 |
| `Alt+Enter` 排到下一轮 | 当前 Turn 完整结束后再接纳这条输入。终端拦截该组合键时，命令菜单提供同一操作。 |
| `Esc` 打断 | 选择器打开时先关闭选择器。在对话界面，停止当前选中 Agent 的这次执行，草稿留在输入框，界面显示清理进度或失败原因。需要用户再按 `Enter` 才发送草稿。关闭窗口会断开观察连接。 |

由 SDK 在每个边界决定接纳哪些输入。同一边界上符合条件的输入按回执顺序接纳。已准备的 Step 在供应商重试时保持一致。即使先排了一条下一轮消息，后来发送的插话仍能进当前 Turn。界面显示待回答或待授权状态，只有匹配的回复会解除等待。[Step 准备逻辑](../../../crates/kuku/src/query/frozen.rs)

## 实现顺序

1. **把已验收的 SDK 基线合入 `main`。** 从合并点开一条客户端分支，复用当前工作目录及其 Cargo 缓存。收回旧 worktree 前核对提交覆盖、在途进程和 Git 之外的验收证据。
2. **打通共享执行路径。** 在 SDK 补齐 Step 和 Turn 边界的输入接纳，沿用 server 的 Session 管理、事件读取与待决回复。让 CLI 通过可复用 Rust 客户端运行。两个客户端能发任务、断开、找到同一个 Agent、插话并读到同一份持久结果。替换接口时在同一改动中更新调用方、清除旧路径。客户端用例跑通后，这一阶段即可独立合入。[核心方向](core-direction.md)
3. **交付终端。** 用 Ratatui 排版、Crossterm 收键盘与终端事件，选择支持软换行的多行编辑器。server 契约实现期间可以先做小型界面原型。在交互终端，裸 `kuku` 进入 TUI。显式脚本命令保留机器可读输出。验证客户端命令与 Skill 调用都能正常使用。[Crossterm 事件](https://docs.rs/crossterm/latest/crossterm/event/struct.EventStream.html) · [Ratatui 多行编辑器](https://docs.rs/ratatui-textarea/latest/ratatui_textarea/)
4. **接入现有 WebUI。** 更换 HTTP 路由时同步调整浏览器调用方。重连、插话、下一轮发送和打断使用相同的 Session／Agent 命令。验收从 TUI 发起的任务能在浏览器继续操作，再切回终端。浏览器布局可根据终端日常使用中的反馈继续设计。[现有 WebUI API](../../../apps/web/src/api)

## 开发时如何测试

| 层次 | 验证内容 |
|---|---|
| 画面与输入 | 用 Ratatui 的内存后端分别渲染普通和窄终端，直接驱动按键。断言接收者、草稿、滚动、中文宽字符、折行，以及等待和排队状态。[TestBackend](https://docs.rs/ratatui/latest/ratatui/backend/struct.TestBackend.html) |
| SDK 与 server | 使用真实 server、临时工作目录和可控制返回时机的模型服务。先卡住工具，先发下一轮消息，再发当前轮插话。收到回执后断开第一个客户端，重连并放行工具。核对实际模型请求和 Turn ID，下一 Step 只接纳插话，下一 Turn 才接纳后续消息，同一命令重发只出现一次。另测旧窗口打断与新 Turn 的竞争，以及关联回复。[现有模型测试设施](../../../crates/kuku-server/tests/common/mock_provider.rs) |
| 真实终端 | 分别在 Unix PTY 和 Windows 原生 ConPTY 上，用同一份本地配置启动两个真实客户端，核实只启动一个 server。切换 Agent、粘贴草稿、调整窗口、按 `Esc` 打断。断言草稿仍可见，再按 `Enter` 后才收到它的首个回执。退出再连接，检查持久结果、终端模式及光标恢复。[portable-pty](https://docs.rs/portable-pty/latest/portable_pty/) · [vt100 屏幕解析器](https://docs.rs/vt100/latest/vt100/struct.Parser.html) |
| 实际使用 | 在 Linux 终端和 Windows Terminal 手测中文输入法、复制及滚动。macOS 运行 CI 自动用例，单独记录人工体验覆盖。在浏览器实测一次连回同一 server 的流程。[测试指南](../contributing/testing.md) |
