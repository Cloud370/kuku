# Testing

本页维护验证入口、真实测试配置和证据要求。[开发指南](development.md)维护工作循环，[演进说明](../architecture/evolution.md#验证与测量)定义重构所需的具体 case 与测量。

## 选择验证范围

按改动行为选择检查。服务端测试夹具复用一个本地 HTTP 客户端，就绪等待共用用例截止时间，不把单个阶段的启动耗时当作性能断言。失败快照只保留有界执行状态，排除配置和长对话文本。每项不变量只保留一处主要覆盖：重试上限与退避交给已有虚拟时钟 provider 测试，Session 测试负责中断与显式恢复，不再用另一份真实等待退避的集成测试重复完整组合。持续维护的选择入口使用 Python 3.10+，将有界证据写入已忽略的 `benchmarks/acceptance/` 目录：

```bash
python3 scripts/acceptance/run.py contract
python3 scripts/acceptance/run.py protocol
python3 scripts/acceptance/run.py integration
```

Windows workspace 选择器串行运行独立 Rust 测试用例（`--test-threads=1`），各用例内部验证的并发保持不变。Shell 语法和输出检查复用子进程启动预算；进程超时测试先观察真实子进程就绪，再推进 Tokio 时钟触发截止时间，并检查实际进程清理。

`contract` 启用 `kuku/test_support`，运行 workspace 测试，包含受 feature 控制的运行时套件、共享客户端和 TUI 测试。某个测试程序失败后会继续运行其余测试程序，任一测试失败仍返回失败。`protocol` 选择 provider 适配、配置、错误、集成、追踪及缓存通知固定输入，无需付费请求。`integration` 依次执行只读格式检查、启用测试 feature 的 workspace／all-target Clippy 和 `contract`。草稿 PR 与 main 推送只运行一套 Linux workspace 契约与 Python 工具检查，加上格式检查和 Clippy。进入评审的 PR 在 Linux 运行完整 workspace 契约、安装终端和浏览器验收。macOS 和 Windows 运行原生安装冒烟测试，覆盖首启、模型编辑、终端缩放、粘贴和终端恢复。Python 工具检查在三个平台运行。Linux 浏览器和终端验收共用同一个已安装程序。真实模型调用和 Linux benchmark 容器需要在本地明确选择。

契约测试采集保留原有 64 KiB 输出尾部和完整输出 hash。独立失败摘要在 16 KiB UTF-8 字节范围内保留失败测试名、panic 位置与少量上下文，以及 Cargo 失败 target。`failure_summary.truncated` 标记遗漏内容；失败仍由子进程退出状态判定。

契约测试、release 安装和 PTY 构建通过 Cargo 原生 `--timings` 在 `target/cargo-timings/` 生成报告。CI 将其保存为 `build-timings-<os>` artifact，以便分别评估编译与测试执行时间。判断耗时变化是否来自源码前，先比较缓存状态、feature 与 profile。

两个 workflow 都支持目标为 `main` 或 `feat/interactive-clients` 的 PR，以及 main 推送。WebUI 检查的路径范围限于 `apps/web/**` 和自身 workflow，Rust 改动不再触发重复的安装浏览器构建。PR 检查统一使用 GitHub 的 merge-ref。记录实际 commit/tree，只有 tree 相同才复用源分支证据。同一 PR 的旧运行会被取消。Python 工具检查失败不会阻止 Rust 契约与安装验收作业收集各自结果。失败后保留 Rust 依赖缓存；Cargo 仍校验构建指纹。PTY helper 共享契约测试的 workspace/`kuku/test_support` feature 解析，并记录编译身份。CI 每轮只构建一次 Web 资源。各原生任务用这些资源安装对应平台的程序，也可复用同一合并提交、OS、架构和编译器的缓存程序。验收前校验安装程序的版本。

待评审 PR 的安装验收复用 debug 程序及合约阶段已编译的 Rust 产物。浏览器验收仍使用嵌入 Web 资源的二进制。安装程序按准确提交和工具链缓存，独立 PTY helper 按自身源码、依赖锁文件、平台和编译器缓存。手动发布验收继续在原生平台验证 release 构建。

手动 CI 默认 `platform=all`。排查单个平台时可选 `linux`、`macos` 或 `windows`；运行标题标明所选范围，不同平台的运行不会互相取消。手动触发会在所选平台运行完整契约和安装验收，包括 Windows 桌面和 Linux 浏览器交接。

草稿 PR 通过不代表安装验收通过。合并前要求目标版本的待评审 PR 检查通过。发布候选版本和需要完整跨平台排查的改动，以 `platform=all` 手动触发全量检查。安装验收使用带嵌入 Web 资源的程序；选择 Windows 桌面检查时必须完成桌面准备。独立安装场景使用隔离服务，单个场景失败后继续其余场景，保留原始错误并让整体返回失败。现有每条命令的超时界限保持不变。每个场景打印耗时，最多 16 KiB 的失败详情在最后一个场景之后再次输出，避免后续输出挤掉失败终端画面。受控模型只有核实工具结果后才能回复成功，模拟成功消息本身不能证明工具成功。命令可能在前台完成，也可能返回 Job 句柄；固定模型通过 `wait_job` 等待并核对持久化命令事实后才能报告成功。安装恢复场景还会明确等到自动转为 Job，再通过正常历史滚动检查重开的启动卡片和完成通知。需要执行持续活跃的测试会保持模型请求，完成相关断言后再释放响应，不依赖固定响应延迟。

选择入口将 `CARGO_TARGET_DIR` 固定为当前 checkout 的 `target/`，不改变宿主环境。每个 worktree 使用独立 target，即使串行构建也不共享：workspace 产物可能错误复用另一源码树的缓存。本地 target 可以使用第三方依赖缓存作为种子，但首次构建前先执行一次 `cargo clean --target-dir target -p kuku -p kuku-cli -p kuku-server -p kuku-client -p kuku-tui -p kuku-app` 清除 workspace 产物。记录提交／工作树、target 目录、feature、profile 和二进制 hash；共享 target 构建的冻结二进制本身不能证明对应的源码版本。

编辑时使用针对性检查，在运行时集成批次收尾执行一次所需 Rust 集成检查。等价的直接命令为：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features kuku/test_support -- -D warnings
cargo test --workspace --no-fail-fast --features kuku/test_support
```

相关写入者停止修改后，先格式化修改的 Rust 范围，再运行只读格式检查。编辑中可用 `cargo check` 快速反馈。不因随后要提交或推送就重复通过的检查；代码、配置或集成变化使证据失效时再补测。纯文档改动使用下方的文档检查。

不带 `test_support` 的旧默认测试命令不会选择所有套件。只检查受 feature 控制的运行时集成时使用：

```bash
cargo test -p kuku --features test_support --test provider_integration --test query_runtime --test cache_runtime_notices
```

显式选择受影响的 feature 套件，并报告实际运行范围。被忽略的真实测试需要显式选择，并提供对应凭据与启用配置。跳过或未选择的测试不算覆盖。测试归属变化时，同步更新实际入口、CI 选择和本参考页。

| 证据 | 能说明什么 |
|---|---|
| 保留下游真实组件的针对性契约测试 | 确定性的状态、授权、持久化、取消和资源行为；仅替换不确定边界 |
| 录制的协议／重放 case | 无需再次付费调用即可验证精确请求／上下文和工具结果行为；固定输入的变化按行为变化审阅 |
| 公开 SDK／CLI 的真实任务 | 装配后的用户路径能够使用配置中的模型及真实工具 |
| 原 benchmark 判定器及资源测量 | 记录条件下的任务结果与费用／性能 |
| CI 平台矩阵 | 实际运行的平台与配置结果 |

独立检查改动文件、命令退出状态、持久事实和资源清理，不依赖 Agent 最终措辞。直接 HTTP 探测验证连通性或供应商行为，不代表 SDK 已集成。真实模型成功不证明罕见失败场景的保证。

## 安装客户端验收

终端输入、服务发现、内嵌浏览器和发行打包变更，使用安装客户端验收入口。

```bash
python3 scripts/acceptance/run.py interactive
python3 scripts/acceptance/run.py interactive --native-smoke
python3 scripts/acceptance/run.py interactive --archive PATH_TO_PLATFORM_ARCHIVE --browser
python3 scripts/acceptance/run.py interactive --archive NEW_ARCHIVE --upgrade-from OLD_ARCHIVE
python3 scripts/acceptance/run.py interactive --desktop --archive PATH_TO_LINUX_ARCHIVE
python3 scripts/acceptance/run.py interactive --desktop --archive PATH_TO_WINDOWS_ARCHIVE --windows-terminal PATH_TO_WINDOWS_TERMINAL_EXE
```

默认使用仓库指定的 Node 版本构建 Web 资源，再安装当前源码中带 `embedded-web-assets` 的 `kuku`。`--archive` 测试对应平台的 `.tar.gz` 或 `.zip` 发行包。`--binary` 复制已经构建或安装的可执行文件。临时安装前缀、空白数据目录和工作区都在源码树外。`--scratch-base` 可另选源码树外的临时目录。选择入口将二进制 hash、原生终端类型、所选场景和结果写入现有的证据目录。

要复测安装包升级，提供两个包含不同可执行文件、均支持持久 Session 的平台归档。选择入口在工具运行期间调用平台安装器替换程序，验证重启被推迟、原工具仅执行一次，以及新服务能恢复同一 Session。

终端场景在 Linux 和 macOS 通过 Unix PTY 驱动安装好的 `kuku`，在 Windows 使用 ConPTY。公共终端模拟器在 DEC 同步刷新期间保留上一帧，提交后才提供新帧；[协议测试](../../../apps/kuku/tests/terminal_state.rs)覆盖分片更新、调整大小和光标报告。用带内嵌资源的二进制和 `--browser` 选项可加入 Chromium 场景。按 [CI 浏览器准备步骤](../../../.github/workflows/ci.yml) 安装 Playwright Python 包和 Chromium。场景使用可控的本地模型端点，实际运行客户端和服务，无需模型凭据或付费请求。各层需要验证的行为见[交互客户端计划](../architecture/interactive-clients.md#开发时如何测试)。场景通过原生终端操作 Ctrl+C 和菜单补全，分别保持模型请求与工具处于运行中以核对显示状态，并在服务断开时检查连接指示。本地模型端点报告已知的输入、缓存读取和输出 token 数，供安装后的终端核对上下文占用、累计 token 和缓存命中率，并检查实测 TPS、Agent 地址与工作目录。

维护中的生命周期场景从缺失凭据修复走到首个已认证回复，保存默认模型并检查新建和重开 Session 实际使用的模型，以及用键盘 ↑/Delete 撤回精确排队输入，同时保留其草稿和另一条排队输入。场景也会重开只读回退内容并核对下一次 provider 请求不含被排除的内容，跨过最近 48 条已提交消息选择精确回退目标，并用受控的 401 验证遮蔽凭据更新和安全服务重启。认证修复场景确认只有显式 Continue 才会用替换凭据重试保留的原输入，且仅重试一次。键盘覆盖与现有鼠标撤回检查并存。

原生冒烟场景在 60 和 120 列下验证输入框操作，包括斜杠补全、Ctrl+T、Agent 与模型的思考设置，以及服务请求仍在运行时的内联退出确认。用 `interactive --native-smoke --binary PATH_TO_BINARY` 或 `--archive PATH_TO_PLATFORM_ARCHIVE` 在目标平台检查候选版本。终端画面保存在 `installed-interactive.log`。[输入框场景](../../../scripts/acceptance/interactive_composer.py)

对照终端布局时，为两个版本使用同一套固定输入和窗口网格尺寸。安装场景检查 `/statusline` 的预览、保存、重开和恢复默认，并验证带着未发送草稿在多个工作中的 Agent 之间切换。工具密集对话在同一轮至少执行 20 次调用，包括成功操作、缺失文件和等待释放的命令。场景在工具运行和代码围栏未闭合时分别捕获宽窄终端画面，再核对完成的中文报告、保留的工具事实和未发送草稿。有界 `installed-interactive.log` 保存终端画面和检查结果。[终端场景](../../../scripts/acceptance/interactive.py)

本地对比流式表现时，用同一回放运行两个原生发行版二进制，每次使用不同临时目录。回放记录原文哈希、首段文字出现时间、突发输出期间的按键回显，以及 60 和 120 列下后段数据的绘制样本。绘制计时从本地固定端点刷新数据开始。Rust 测量直接记录 Markdown 排版耗时，并报告复用的字节数和行数。[原生回放](../../../scripts/acceptance/interactive_stream.py)

```bash
python3 scripts/acceptance/interactive.py --stream-profile --binary KUKU_BINARY --bridge NATIVE_PTY_HELPER --directory SCRATCH_DIR
cargo test --release -p kuku-tui stream_layout_profile -- --ignored --nocapture
```

在装有 Docker 的 Linux 上用 `--desktop` 加入 Xfce Terminal 和 IBus 拼音场景。选择入口使用 Ubuntu 软件包构建镜像，并在仅有回环网络的隔离 Xvfb 会话中运行。场景通过图形界面验证中文组字、可编辑的多行粘贴、**Messages → Copy** 和终端字体缩放。首次构建时 Docker 需要访问 Ubuntu 镜像和软件仓库。安装后的可执行文件和选择入口的原生测试 helper 都须能在镜像中运行。`desktop/result.json` 记录包版本、终端网格尺寸、提交与复制的固定文本及有界截图的 hash。

桌面场景复用工具密集对话，捕获宽窄视图并核对复制出的报告原文。截图与 `desktop/result.json` 一起保留，有采集时也保存动作菜单、模型选择器及状态栏编辑器截图。[桌面场景](../../../scripts/acceptance/desktop)

Windows Terminal 场景需要专用的活动测试桌面、独立剪贴板、美式键盘布局和 Windows Terminal 默认快捷键。在运行场景的 Python 环境安装 Pillow，然后通过 `--windows-terminal` 指定已有的 `WindowsTerminal.exe`。场景使用鼠标左键拖选回复中的一段文字，松开后核对 Windows 剪贴板。启用微软拼音后可验收候选组字和提交。微软拼音不可用时，场景仍记录复制、可编辑的多行粘贴和字体缩放的结果，并将 GUI 覆盖标为不完整。[Windows CI 任务](../../../.github/workflows/ci.yml) 准备便携版 Terminal，尝试配置拼音，并将预检结果随场景证据保存。macOS 的人工实操单独记录平台证据。

显式选择 `--live` 后，使用平台安装器、发行包和 Chromium，通过安装后的客户端发起有预算限制的 DeepSeek 调用。此模式要求从显式凭据文件读取 DeepSeek 密钥，并使用已有[预算账本](#凭据与运行产物)。经授权可用 `--ledger` 选择另一份现有账本。使用唯一的运行 ID 和已授权的账本。

```bash
python3 scripts/acceptance/run.py --run-id LIVE_RUN_ID interactive --archive PATH_TO_PLATFORM_ARCHIVE --live --secrets PATH_TO_CREDENTIAL_FILE
```

`live-result.json` 记录实际模型路线、请求和任务结果。场景核对实际项目修改及测试输出、Agent 切换、工具运行期间更换客户端，以及在终端、浏览器和脚本客户端之间接续工作。[真实调用场景](../../../scripts/acceptance/interactive_live.py)

通过 `--intent deepseek` 或 `--intent openrouter` 在原生终端中检查真实模型执行的动作。用例验证终端显示的回复措辞、跨轮次的文件内容，以及独立委派。委派用例核对首次创建请求成功，以及 main 收到子 Agent 的结果后才向用户展示。可用 `--intent-case delivery` 或 `--intent-case delegation` 单独运行其中一组。

```bash
python3 scripts/acceptance/run.py --run-id INTENT_RUN_ID interactive \
  --intent openrouter --intent-case delegation --binary PATH_TO_BINARY \
  --intent-model openai/gpt-6.1-terra --intent-effort high --intent-upstream openai \
  --revision EXPECTED_GIT_COMMIT --scratch-base PATH_OUTSIDE_CHECKOUT \
  --secrets PATH_TO_CREDENTIAL_FILE --ledger PATH_TO_AUTHORIZED_LEDGER
```

可用 `--archive` 代替 `--binary`。验收在预留预算前核对安装后二进制的提交版本。直连 DeepSeek 时默认使用 `deepseek-flash`，通过 OpenRouter 时默认使用 `openai/gpt-6-luna` 和 `openai` 端点，默认思考档为 `low`。用 `--intent-model` 和 `--intent-effort` 覆盖模型和请求档位，用 `--intent-upstream` 指定准确的 OpenRouter 端点标签。验收限制请求数和输出，包括重试。`intent/result.json` 记录用例检查、终端画面路径和用量。意图验收要求确认路由，使用 OpenRouter 时会根据生成回执核对模型、上游 provider 和可用的费用。检查结果后在所选账本结算预留额度。见[意图验收场景](../../../scripts/acceptance/interactive_intent.py)和[凭据与运行产物](#凭据与运行产物)。

分别记录确定性检查、平台执行和人工体验。人工验收时，在源码树外安装候选版本，用已配置的模型读取文件、完成修改并运行检查，再核对实际文件、diff 和工具结果。从浏览器和另一个终端恢复同一 Session。分别记录 Linux、Windows Terminal 的输入法、粘贴、复制与缩放检查，以及实际 macOS 覆盖。模型调用证据遵循下方规则，无法执行的场景标为待验证。

## 工作区执行验收

用 `workspace` 通过公开 SDK 示例或已安装的终端客户端执行同一项大日志任务。验收运行本机日志夹具，核对生成报告的准确内容和观察到的命令事实，随后重开 Session，验证用命名值复现报告时无需再次运行夹具。产品使用隔离的 `PATH`。Python 在产品进程之外驱动验收。见[工作区场景](../../../scripts/acceptance/workspace.py)。

```bash
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py workspace --sdk target/debug/examples/session_task
python3 scripts/acceptance/run.py workspace --binary PATH_TO_BINARY
```

Windows 构建使用带 `.exe` 后缀的路径。可以用 `--archive` 代替 `--binary` 验收本地发行包。选择器默认构建本机夹具，也可以用 `--bridge` 复用已有构建。用 `--scratch-base` 可指定源码树之外的临时目录。使用 `--sdk` 时可以加上 `--handoff-only`，预存一份命名报告，验证独立审查 Agent 能读取共享引用并报告发现。

默认使用本地确定性 provider。工作区的正式真实模型验收先跑直连 DeepSeek 的 `deepseek-flash`、`high`，再用 OpenRouter 的 `openai/gpt-6.1-terra`、`high` 和 `openai` 端点交叉验证。选择对应的 `--provider` 时会应用这些默认值。为每家服务传入对应的 `--secrets` 和 `--ledger`。下面通过公开 SDK 调用程序显式记录每条路线。

```bash
python3 scripts/acceptance/run.py --run-id WORKSPACE_DEEPSEEK_RUN workspace \
  --sdk target/debug/examples/session_task --provider deepseek --model deepseek-flash \
  --effort high --secrets PATH_TO_CREDENTIAL_FILE --ledger PATH_TO_DEEPSEEK_LEDGER
python3 scripts/acceptance/run.py --run-id WORKSPACE_OPENROUTER_RUN workspace \
  --sdk target/debug/examples/session_task --provider openrouter --model openai/gpt-6.1-terra \
  --effort high --upstream openai --secrets PATH_TO_CREDENTIAL_FILE --ledger PATH_TO_OPENROUTER_LEDGER
```

`workspace/result.json` 记录任务检查、费用、发送的模型和思考档，以及路由证据。无法核实时，`workspace` 选择器允许 `upstream_confirmed: null`。OpenRouter 未返回生成回执时，只能报告请求已固定到哪个端点，实际上游仍未知。思考档同样按请求值报告，除非 provider 回报实际值。确认路由不一致会使本次验收失败。见[凭据与运行产物](#凭据与运行产物)。

### 命令输出与长任务验收

以[命令输出提案](../architecture/workspace-execution.md#命令输出处理提案)为行为契约。复用现有工具、Session 和安装客户端夹具。固定 provider 验证模型实际能看到什么，真实 provider 验证模型实际怎么选。

| 检查 | 必要证据 |
|---|---|
| 原生调用 | 默认命令 schema 接受字面程序参数。用原生 Linux 和 Windows 验证空格、引号与 Unicode，单独记录 macOS 覆盖。核对实际授权的调用、运行结果与 TUI 标题。 |
| 短、长和失败输出 | 核对首条送给 provider 的结果与实际捕获。短结果直接显示。长结果的形态和方法提示足以在下一 Step 取到需要的内容。失败时保住决定性诊断和独立的捕获完整性标志。 |
| 原生搜索和规则覆盖 | 用字节级夹具验证跨帧匹配、授权、取消和截断。按每条诊断引用读回原始捕获。每个启用的规则族覆盖成功、失败和格式不匹配，并验证通用回退以及重开后原先接纳的精确视图。 |
| 后台和长任务 | 覆盖快速完成、一秒边界、取消和单次终态。观察持续有进展的慢 Job，以及安静且存在具体阻碍的 Job。验证结果已经明确时可以立即收束，同时仅观察到安静时，有用的工作仍可继续。 |

真实对照固定模型、思考档、工具、预算和原环境及 verifier。覆盖短程直接操作、适合 `run_js` 的任务，以及包含 Job 或昂贵验证的长流程。至少保留两个结构不同的任务用于独立验证，调 Prompt 时不使用。先固定输出视图比较 Prompt 选型，再固定 Prompt 比较提取提示，最后验收组合效果。

记录工具和脚本首次调用的正确性、适用任务中的 `run_js` 选用率、探索性补读、重试和重复检查。记录原 verifier 结果、墙钟耗时、provider 累计输入与缓存用量、峰值 RSS 和保留磁盘字节。用通用有界预览作为摘要字节数基线，用 provider 用量支持 token 结论。结果已明确，或具体阻碍让继续执行失去验证价值时，结束当次运行并保存观察与原因。实际执行用时和最长运行上限分别记录。

## 测试归属与证据

单元测试保留在 `#[cfg(test)] mod tests`；集成测试按领域位于 `tests/` 下，共用 helper 位于 `tests/common/mod.rs`。复用已有调用程序与固定输入。重构应提供一个持续维护的入口或小型 case 清单，选择契约、重放、真实调用及 benchmark 检查，不另建评测平台。

每次选择运行生成一个 `result.json`，包含提交、未提交状态及已跟踪差异的 hash、所选命令／feature、退出状态、耗时和证据位置。每个命令最多保留 64 KiB 脱敏输出，同时记录原始字节数、hash 和明确的截断标记。超时会在 POSIX 清理所属进程组，在 Windows 请求终止进程树；未完整采集视为失败。验收工具的[孤儿进程输出测试](../../../scripts/acceptance/test_run.py)覆盖 POSIX 进程组，在 Windows 上跳过。真实 SDK 检查独立核对调用程序的持久事实、设置、用量与清理，仅命令成功不能通过。

报告已选择／通过／失败／跳过的覆盖，区分基础设施失败、运行时失败和 Agent 未完成任务。详细输出保存在有界产物中，任务或 PR 只携带简明结论与证据引用。不打印 key 或私有请求正文。无需凭据即可检查选择入口自身：`python3 -m unittest discover -s scripts/acceptance -p 'test_*.py'`。

路由对比保持模型、思考档、工作负载和预算固定。区分冷请求与连续会话，保持每个会话关联稳定，比较回报费用、缓存／未缓存输入、输出、实际路线、首段输出时间及完成延迟。供应商名称本身不能证明端点身份或缓存保留；少量合成对话不证明全局最优路线。

## 原始 benchmark 任务

[`scripts/acceptance/cases.json`](../../../scripts/acceptance/cases.json)维护少量 case 的选择和上游完整提交。先选择 Terminal-Bench 2.0 的 `cancel-async-tasks`，再扩展到 `log-summary-date-ranges`、`openssl-selfsigned-cert` 和 `regex-log`。原任务、Docker 构建上下文、solution 和判定器保持不变。[Harbor 任务契约](https://docs.harborframework.com/tasks/overview)允许直接使用这些独立任务，无需安装另一套评测框架。

列出所选任务，在花费模型预算前先检查一个环境：

```bash
python3 scripts/acceptance/run.py list
python3 scripts/acceptance/run.py --run-id cancel-environment benchmark --case cancel-async-tasks --mode preflight
```

Linux／Docker 预检将固定源码检出到已忽略的 `benchmarks/sources/`，记录任务文件／判定器 SHA-256 hash 和构建镜像 ID，并创建两个全新容器。未修改环境必须实际收集并运行测试且失败；原 oracle 必须使用同一个原始 `tests/test.sh`，全部已收集测试均通过，不存在 skipped、pending 或其他结果。没有已收集测试报告的零 reward 属于基础设施失败。SDK／oracle 执行后，宿主先将判定器输出目录替换为空目录，再运行判定器，避免把先前残留报告作为本轮证据。判定器仍安装原有的固定版本依赖；下载失败不算任务失败，重试要使用独立运行标识。入口沿用任务原有的单 CPU／2 GiB 限额和工作目录，移除自己创建的容器并报告清理失败。

真实尝试先构建公开 `session_task` 示例，再选择 `benchmark --mode live`，提供 `--sdk-binary`、`--config`、`--connection` 及下方的预算参数。调用程序的其他有界参数放在 `--` 后。二进制、解析出的 Linux 动态库和宿主 CA bundle 复制到独立的只读运行时挂载。证据记录各文件的 hash 和 CA 来源；`SSL_CERT_FILE` 让 SDK 使用保留的信任证书，任务镜像没有安装 CA 根证书时仍校验证书。Agent 执行时只能访问原任务指令和环境，不提供 solution，隐藏测试在执行结束后才复制进容器。Session 数据保留在该次尝试的 `session-data/`；预期任务产物列于清单。

原 reward、收集／通过／失败测试数、SDK 退出状态与清理结果分别记录。预期任务产物在判定前保存，每个上限 32 MiB；证据缺失或过大不能通过真实验收记录。SDK 检查器流式读取原始 `events.jsonl`，要求已提交游标有序、准备请求与响应对应、工具调用／结果配对、已接纳结果视图不变，以及 `KUKUOUT1` 输出引用的帧和字节数有效并已完成写入。SDK 统计／设置／报告必须匹配这些事实，运行时与持久关闭屏障均须成功。请求与费用按 Agent、Turn 和请求标识关联，后续 Turn 可以复用 `req_1`。整个 Session 的用量与所选 Agent 的快照分别记录。任务成功仍由原 benchmark 判定器决定。

检查器每个事件最多读取 1 MiB，累计最多 100,000 个事件，SDK 报告上限 4 MiB；只保留有界 hash／元数据，不复制上下文历史。诊断上限为 4,096 个准备请求及 8,192 个工具调用／provider 报告。未确认的上下文后缀跨 segment 最多保留 16,384 个工具块指纹，只有匹配的准备请求头才确认其中的工具视图。重试会替换候选后缀，已确认的前缀及持久调用／结果匹配仍严格校验。无需再次调用模型即可复核已有证据：`inspect --sdk-result PATH --session-data DIR --container-data /kuku/data --connection NAME`；报告使用宿主路径时省略 `--container-data`。此只读选择不要求凭据或费用预留。

工具参数格式错误会产生 `invalid_response`，并丢弃整批工具调用。检查器核对该响应未接纳任何工具调用，并确认同一 Agent 和 Turn 准备了替代请求。两次准备请求的用量均计入总量。

调用程序报告 `streaming` 时，检查器核对计数、首次观察时间及持久请求／模型字节数的上界。这些数据来自同一个 SDK 订阅，不是可重放的线路计时。所选协议检查要求非空模型输出且没有丢弃更新时，可对 `inspect`、`live` 或真实 benchmark 增加 `--require-streaming`。旧调用程序明确保持未报告。模型的 `no_agents` 设置禁用其协作工具，不禁止宿主在同一 Session 创建其他独立 Agent。

SDK 运行时，入口分别采样 SDK RSS／RSS 高水位、CPU 时间和线程数，以及整个容器的 CPU、内存、进程数和 I/O。采样与输出日志均有界；未采样值保持 `null`，不把整容器数据归因于 SDK。下方专门的资源调用程序测量其余生命周期边界。

### 共同观察 TUI 运行（可选）

在交互终端中提交任务并观察 Agent 的执行。有 tmux 时，可以用命名会话让其他人接入同一个 TUI，也可以抓取当前画面。在待测 checkout 或 worktree 中运行：

```sh
cargo build --locked -p kuku-app --bin kuku
tmux new-session -d -s kuku-review -c "$PWD" \
  "$PWD/target/debug/kuku" --workspace "$PWD"
tmux set-option -t kuku-review mouse on
tmux attach-session -t kuku-review
```

从已接入的 tmux 客户端用 `tmux switch-client -t kuku-review` 切换。在 TUI 中用滚轮或 Page Up 和 Page Down 翻页，按 Ctrl+End 回到最新输出。普通接入方式会传递这些输入。并发跑多个案例时，各用一个会话名。不同 worktree 需要独立受管服务时，为各自的 TUI 指定 `--data-dir`，并用 `--config` 选择配置。

通过 Session 日志或 `kuku events SESSION_ID` 查看任务进度、工具结果和请求边界。运行事件命令时沿用相同的连接和工作区选项。用 `tmux capture-pane -p -t kuku-review` 核对画面呈现。将有用的画面文本随运行证据留存。

跑上游 benchmark 时，将 kuku 服务放在固定的任务容器里。若容器内已可运行且已配置 kuku，可以用宿主机的 tmux 接入容器内的 TUI。将 `benchmark_container` 设为正在运行的任务容器的名称：

```sh
tmux new-session -d -s kuku-benchmark \
  docker exec -it "$benchmark_container" kuku --workspace /app
tmux set-option -t kuku-benchmark mouse on
tmux attach-session -t kuku-benchmark
```

宿主机上的 TUI 也可用 `--remote` 连到容器服务，并用 `--workspace` 指定容器内的路径。粘贴原题面，等 Agent 和 Job 都结束后再运行原 verifier。判分隔离和验收证据沿用上面的原始 benchmark 流程。[终端操作](../reference/cli.md#终端界面) · [远程连接](../guides/run-the-server.md#显式启动服务)

## FrontierHarness 公开小样本

[题目清单](../../../scripts/acceptance/cases.json)将 `frontier` 套件固定到 FrontierHarness Eval v1.1.0。历史小样本覆盖日志汇总、异步取消、正则表达式和 Vim 大文件编辑。本地对照还包含 Nginx 配置、SQLite 覆盖率构建，以及需要持续运行到判分结束的 gRPC 服务。每题使用上游题面、镜像构建上下文和原始判分器。在有 Git、Docker、Python 3.10+ 和仓库指定 Rust 工具链的 Linux 上，先用空提交确认判分器能识别失败。构建和评分依赖需要网络，这一步没有模型费用。[数据集来源](https://hub.harborframework.com/datasets/frontier-harness-eval/frontier-harness-eval)

```sh
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py --run-id smoke-regex benchmark \
  --suite frontier --case regex-log --mode preflight
```

实跑时，将 `KUKU_TEST_OPENROUTER_API_KEY=...` 写入仓库外的私有文件。计量程序在宿主机持有上游密钥，并从共用美元账本中为每题预留额度。以下命令每题最多预留 $0.75，经 OpenRouter 固定到 DeepSeek 官方端点，模型为 `deepseek/deepseek-v4.1-flash-1017`，推理强度为 `max`。首次运行时设定总额，复测沿用同一账本，每次使用新的运行 ID。示例显式设置 131,072 tokens，用于复现历史对照。省略 `--max-output-tokens` 时采用固定路由公布的生成容量。路由未公布容量时须显式指定。计量程序会在预留费用前确认单次请求按最高用量计算仍能落在美元预算内。默认由单题美元额度和题目时限约束运行。复现历史限次对照组时可另加记录中的 `--max-requests`。结算按实际用量或明确标记的保守估计计算。

```sh
ledger="$PWD/benchmarks/acceptance/frontier-budget.json"
keys="$HOME/.config/kuku/frontier.env"
python3 scripts/acceptance/run.py budget init --ledger "$ledger" \
  --authorization local-frontier-sample --limit-usd 3
printf '%s\n' log-summary-date-ranges cancel-async-tasks regex-log large-scale-text-editing | \
  xargs -P 4 -I '{}' python3 scripts/acceptance/run.py --run-id 'sample-{}' benchmark \
    --suite frontier --case '{}' --mode live --provider openrouter --upstream deepseek \
    --max-output-tokens 131072 --case-cap-usd 0.75 \
    --sdk-binary target/debug/examples/session_task --ledger "$ledger" --secrets "$keys"
```

如果账户的数据策略排除了所选上游，可以使用与策略兼容的固定端点（如 `--upstream baseten`）另起一轮，并记录路由变化。OpenRouter 可能在流结束时已返回费用，而 generation 回执稍后才可用。给下方报告命令加上 `--refresh-receipts "$keys"` 可查询这些试验已有的回执。之后可离线复用已核对的回执，每笔费用只统计一次。

对照直连 DeepSeek 时，使用 `--provider deepseek` 和含 `KUKU_TEST_DEEPSEEK_API_KEY` 的私有文件。默认模型为 `deepseek-flash`。要在同一题目和判分器下测试 Pro，使用 `--model deepseek-v4-pro`。计量程序会记录所选模型，并在启动前校验价格和额度。

下载官方快照生成可分享的报告。每题按时间顺序传入历次运行，保留所有失败。结果分别给出上游评分、SDK 完成状态、模型成本和判分耗时。Pi 官方数据使用同一模型版本，每题四次，经过审计，使用 Runta 和 Baseten。官方每通过一次的耗时，用所有计分运行的 Agent 与判分总耗时除以审计后的通过次数。失败和超时的耗时也计入总耗时。对比缓存时同时查看实际服务商、输入 token 中已缓存的比例、用量覆盖情况和首个请求。报告只导出允许公开的字段，原始 Session 与私有凭据保存在本机。[官方结果](https://frontierharness.org/live/) · [OpenRouter 路由](https://openrouter.ai/docs/guides/routing/provider-selection)

```sh
mkdir -p benchmarks/acceptance/public
curl -fsSL https://frontierharness.org/api/live -o benchmarks/acceptance/public/live.json
python3 scripts/acceptance/frontier_report.py --official-live benchmarks/acceptance/public/live.json \
  --attempt log-summary-date-ranges benchmarks/acceptance/sample-log-summary-date-ranges \
  --attempt cancel-async-tasks benchmarks/acceptance/sample-cancel-async-tasks \
  --attempt regex-log benchmarks/acceptance/sample-regex-log \
  --attempt large-scale-text-editing benchmarks/acceptance/sample-large-scale-text-editing \
  --output benchmarks/acceptance/public/results.json
```

SDK 在写入结果前采集 Linux `getrusage(RUSAGE_SELF)`，得到本进程峰值 RSS、CPU 时间、文件系统输入和输出块计数。宿主采样器按相邻样本的时间间隔估算平均 RSS，并报告覆盖窗口。最后一次 Linux `/proc/PID/io` 读数可能包含已等待结束的子进程，分别记录逻辑读写量、系统调用数和存储字节。容器快照还包含工具进程。判分在 SDK 采样结束后进行。计量程序分开记录到首个有效流式输出的时间与供应商原生指标，并给出 TPS 与缓存命中统计的分母。

[小样本结果与 Session 诊断](../../../scripts/acceptance/results/frontier-sample.md)保留探索阶段的各次尝试及统一配置下的复测。

### 严格的本地 Kuku–Pi 对照

每道入选题先跑一个 Pi 对照。优先选能拉开 harness 差异的题，再用工作负载不同的第二题检验改动能否泛化。快速收集反馈时，只要宿主机和供应商有余量，两道题的 Kuku 和 Pi 共四次试验可以并发。启动负载较高时，可以把各次启动间隔几秒。每次使用唯一的运行 ID，四次共用费用台账。SQLite 构建这类 CPU 密集任务可以在同一道题的两家运行中都加 `--cpus 2`。报告会校验 CPU 限额一致。结果需注明是否并发及 CPU 限额。正式比较耗时和 CPU 时，两家在同一台空闲机器上依次运行。复测时交替先跑的一方，保留每次结果，包括失败。下面的对照使用无界面入口。TUI 观察另记一轮。

使用支持 cgroup v2 和 `memory.peak` 的 Linux Docker，以及与 Pi 兼容的 Linux Node.js 可执行文件（Pi 1.5.2 要求 22.19 或更高）。每次从原题固定镜像启动新容器。两家使用原始题面和判分器、直连 DeepSeek Chat Completions 的 `deepseek-flash` 和 `max` 推理档，以及相同的请求、时间和费用上限。严格模式默认 1 CPU、2 GiB 内存，关闭 swap。资源验收要求内核限额符合配置，能采集 harness RSS，且 agent 执行阶段没有 cgroup OOM 事件。

将对照版本装进 Git 忽略目录，用 release 模式构建 Kuku runner。保留 npm lockfile，之后可以用 `npm ci` 复现依赖树。将 `KUKU_TEST_DEEPSEEK_API_KEY=...` 放进 `keys` 所指的私有文件，付费执行前用已授权的总额初始化台账。

```sh
pi_ref="$PWD/benchmarks/tools/pi-1.5.2"
npm install --prefix "$pi_ref" --registry=https://registry.npmjs.org --save-exact \
  --ignore-scripts --no-audit --no-fund @earendil-works/pi-coding-agent@1.5.2
cargo build --release --locked -p kuku --example session_task

ledger="$PWD/benchmarks/acceptance/paired-budget.json"
keys="$HOME/.config/kuku/frontier.env"
case_id=large-scale-text-editing
python3 scripts/acceptance/run.py budget init --ledger "$ledger" \
  --authorization local-paired-frontier --limit-usd 1.50

paired_run() {
  run_id="$1"
  shift
  python3 scripts/acceptance/run.py --run-id "$run_id" benchmark \
    --suite frontier --case "$case_id" --mode live --provider deepseek --require-resource-evidence \
    --max-output-tokens 131072 --case-cap-usd 0.75 --ledger "$ledger" --secrets "$keys" "$@"
}
paired_run vim-kuku --sdk-binary target/release/examples/session_task || test "$?" -eq 1
paired_run vim-pi --pi-prefix "$pi_ref" || test "$?" -eq 1

python3 scripts/acceptance/benchmark_compare.py \
  --kuku benchmarks/acceptance/vim-kuku --pi benchmarks/acceptance/vim-pi \
  > benchmarks/acceptance/vim-comparison.json
```

导出工具先核对题目文件摘要、Docker 镜像 ID、模型路由和运行限额。对照报告包含原始评分和测试数、agent 与判分耗时、模型请求数与 token 用量、估算美元费用，以及资源验收、harness RSS、容器内存峰值、CPU 秒数和 OOM 事件。容器内存峰值取启动到判分前的内核高水位，覆盖计入 cgroup 的 harness、工具子进程和采样开销。harness RSS 统计主进程的驻留页面，包含共享映射。CPU 秒数取 agent 执行前后两次内核读数的差值。分享对照 JSON 时附上轮次、主机和构建模式。需要精确复现依赖树时附上 Pi lockfile。凭据、原始 Prompt、工具输出和 Session 文件留在本机。

对照报告累计宿主计量代理接纳的所有请求的转发耗时和序列化请求体字节数，包含重试。转发耗时涵盖上游连接和响应传输。有任一请求缺少相应读数时，该项总量留空。首个输出时间的中位数按已观测的请求计算，并附样本数。

运行器有逐条响应的工具调用记录时，对照报告会分别统计零次、一次和多次调用的响应数。这项指标衡量模型如何组批，实际执行是否重叠需另用工具开始和结算记录核对。

对照 Prompt 版本时复用同一份 release `session_task`，给每条 Frontier 真实试跑传入包含覆盖文件的 `--prompts-dir`。选择器为 Prompt 资源留存快照，在试跑记录中写入 SHA-256 摘要，并以只读方式挂载快照。缺少的资源由 SDK 提供嵌入的默认版本。使用 `benchmark_compare.py --baseline RUN_DIRECTORY --candidate RUN_DIRECTORY` 核对 SDK 二进制、运行时包、两份不同的 Prompt 快照，以及双方一致的题目、provider 设置和资源限额。

## 资源测量

```bash
python3 scripts/acceptance/run.py metrics
python3 scripts/acceptance/run.py metrics --profile release
python3 scripts/acceptance/run.py metrics --profile release --cpu-count 1 --address-space-mib 512
```

此本地选择在当前 checkout 的独立 target 中构建 [`resource_probe.rs`](../../../crates/kuku/examples/resource_probe.rs)，使用既有固定 SSE／配置 helper，通过公开 `Session` 执行。它不加载付费凭据或费用台账。默认规模为 32 个空闲 Agent、64 次真实队列输入追加、5 次关闭／重开、4 MiB 命令输出与 1 MiB 保留配额，以及两个独立推进的 Agent。输出子进程复用调用程序二进制，不增加另一套 SDK 执行器。

`caller.json` 记录工作规模、样本数组及单位、nearest-rank 追加／重开百分位数、首次 Session 打开时间、输出字节／配额／读取边界、并发与持久路径。仅在实际准备事实存在时报告 SDK 的 `context_prepare_us` 和 `request_prepare_us`；输入到完成的延迟另行测量，不能替代这两项。首次打开和重开不清空操作系统文件缓存。

工作区负载通过公开 SDK 读取器和嵌入式脚本分别分页处理同一份本机程序日志。它记录首次和重复调用的延迟、worker 数量，以及峰值与任务结束后的进程采样。两个 Session 共享同一个 `WorkspaceExecutionHost`，提交的脚本数超过 worker 并发额度。保留检查跟踪磁盘增长，回收普通捕获，并验证命名基线在重开和尝试超配额后仍可读取。默认重复六次任务，使用两个脚本 worker，允许四个脚本宿主操作并发。用 `--repeated-tasks`、`--script-workers` 或 `--script-host-operations` 可改变负载。

Linux `/proc` 和 Windows 进程 API 分别采样 SDK 调用进程和工具后代。固定 provider 位于调用进程内，计入进程占用，并单独记录基线。CPU 使用秒，RSS 使用 KiB。未采样阶段和不可用计数保持未知。保留输出和事件文件的逻辑字节数单独记录。运行包含构建 profile、target 目录、二进制及源码哈希、进程限制和清理结果。`measurements_complete` 与 `missing_measurements` 区分本地任务成功和测量缺失。

在 Linux 上，`--cpu-count` 限定调用进程及其后代可用的 CPU。`--address-space-mib` 限定各个进程的虚拟地址空间。Rust 调用程序可以跨平台运行。此选择器当前在 Linux 和 Windows 提供进程采样。

用 `--pressure-rounds 3` 可以在同一个调用进程里对比多轮峰值和空闲阶段。在 Linux 上，如果能读取 `/proc/<pid>/smaps_rollup`，选择器也会记录匿名内存与文件映射内存。

## 真实协议验收

用户选定直连 DeepSeek 的 `deepseek-flash` 用于 benchmark 和三种线路协议检查。第二模型家族使用 OpenRouter 的 [`openai/gpt-6-luna`](https://openrouter.ai/openai/gpt-6-luna)，固定标准 OpenAI 提供商，通过 OpenAI Responses API 调用。这条 OpenRouter 路线用固定小任务检查协议兼容。下表保留两家服务公开的通用路线，不要求穷举每个模型／服务／协议组合。这些是测试连接输入，不是新冻结的 SDK 配置名称。

| 服务 | 线路协议 | 基础 URL | 请求后缀 | 来源 |
|---|---|---|---|---|
| DeepSeek | Chat Completions | `https://api.deepseek.com` | `/chat/completions` | [首次调用](https://api-docs.deepseek.com/) |
| DeepSeek | Responses | `https://api.deepseek.com` | `/responses` | [Responses 兼容说明](https://api-docs.deepseek.com/guides/responses_api/) |
| DeepSeek | Messages | `https://api.deepseek.com/anthropic` | `/v1/messages` | [Messages 兼容说明](https://api-docs.deepseek.com/guides/anthropic_api/) |
| OpenRouter | Chat Completions | `https://openrouter.ai/api/v1` | `/chat/completions` | [快速开始](https://openrouter.ai/docs/quickstart) |
| OpenRouter | Responses | `https://openrouter.ai/api/v1` | `/responses` | [Responses 参考](https://openrouter.ai/docs/api/api-reference/responses/create-a-response) |
| OpenRouter | Messages | `https://openrouter.ai/api/v1` | `/messages` | [Messages 参考](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message) |

阶段 1 从 DeepSeek 开始。阶段 2 要求直连 DeepSeek 的 Chat Completions、Responses 和 Messages 覆盖，以及选定的 OpenRouter／OpenAI Responses 兼容路线。每条所选路线都要覆盖流式输出、真实工具调用及结果续接、多轮上下文和可获得的用量／设置报告。benchmark 保持使用 DeepSeek；OpenRouter 面向 OpenAI 模型家族，不重跑 benchmark 集，也不要求其余 OpenRouter 协议组合。委派任务包含直连 DeepSeek 和 OpenRouter／OpenAI Agent，保持独立上下文。

仅验证协议时使用实际支持的最低已开启思考档。DeepSeek 当前的[思考指南](https://api-docs.deepseek.com/guides/thinking_mode/)将该档定义为 `low`；OpenRouter 则核对所选上游支持的设置，按所选 API 编码，不假定所有端点接受同一字段和值。benchmark 的思考档按 case 和预算选择，明确记录，同一对比中保持固定；修改档位另记一轮运行，不在失败后静默升档。这些设置只针对被测模型，不改变执行重构的 Codex 会话及其开发 Agent 的模型策略。

所选 OpenRouter 检查使用 `POST https://openrouter.ai/api/v1/responses`，遵循标准 OpenAI Responses 请求语义。模型的[公开端点元数据](https://openrouter.ai/api/v1/models/openai/gpt-6-luna/endpoints)将 `openai` 标识为标准 OpenAI 端点标签；`openai/flex` 和 `openai/fast` 是不同的服务路线。通过以下请求片段固定所选路线：

```json
{
  "model": "openai/gpt-6-luna",
  "provider": {
    "order": ["openai"],
    "allow_fallbacks": false,
    "require_parameters": true
  }
}
```

先核对账号路由策略允许该端点，不使用自动选模或模型回退。OpenRouter 的[路由文档](https://openrouter.ai/docs/guides/routing/provider-selection)说明，默认路由可能回退，也可能把不支持的参数传给会忽略它的供应商。记录请求模型、网关、指定路线及可获得的实际上游回报；无法确认时保持未知。这些是测试连接选项，不是全局运行时调度规则。手工指定供应商顺序会覆盖 OpenRouter 的自动粘性路由；固定端点仍可复用自己的缓存。

日常多轮使用建议采用稳定的 `session_id` 或 `x-session-id`、参数支持过滤，以及候选供应商的成本约束，保留自动粘性路由。传输关联由已有 Session／Agent／连接身份派生，跨 Turn、重试和重开保持稳定；不同 Agent 使用不同关联。不新增核心身份，也不在每个 Step 生成新关联。这属于 provider 适配行为，不另建调度器或共享 Agent 对话历史。遵循[提示缓存契约](https://openrouter.ai/docs/guides/best-practices/prompt-caching)：明确会话关联可从首次成功请求建立亲和，供应商失败可能改变路线。文档所述空闲 60 分钟过期指路由亲和，不保证上游缓存存活同样长。稳定路由不能替代不可变的 provider 可见前缀。

OpenRouter 当前默认路由综合考虑有效价格，包括观察到的缓存使用和输入／输出比例，以及性能与可靠性，不承诺单次请求绝对最低价。选择供应商白名单或价格上限前，应按实际工作负载比较未缓存输入、缓存读／写及输出费用。`provider.max_price` 限制可选单价，不是会话总费用上限，运行预算仍需单独约束。可选的 `provider.only` 可缩小候选范围而不手工排序。多轮检查可获得的缓存 token 用量、最终费用和实际路线。这些配置建议需要用所选端点验证，不保证一定节约费用。

罕见或不确定出现的错误、批次，以及可能被网关规范化的供应商特有重放数据，使用针对性的原生协议固定输入补充验证。OpenRouter 覆盖属于网关覆盖，不证明同样请求可直连 OpenAI 或 Anthropic 官方端点。具备凭据时增加官方直连检查，但不把开通新账号作为启动重构的前提。准确记录跑过的路径，缺少的真实覆盖标记为未验证。

用固定流录制验证模型服务的思考活动、可见正文、交替到达的参数片段和工具派发。检查参数片段对应正确的调用、工具在派发后才执行，以及协议原生思考数据在工具续接与 Session 重开后仍可用。核对对话只呈现思考活动状态和可见回复，私有思考原文保持私有。

- OpenAI Responses 按 `output_index` 和 `item_id` 关联参数片段，用 `call_id` 配对工具结果。无状态重放核对输出顺序、加密思考项和助手消息阶段。[流式事件](https://developers.openai.com/api/reference/resources/responses/streaming-events) · [思考](https://developers.openai.com/api/docs/guides/reasoning)
- Anthropic Messages 按内容块索引分别装配工具输入和思考。加入省略正文但带签名的思考块，核对续接保留原块和签名。[流式消息](https://platform.claude.com/docs/en/build-with-claude/streaming) · [思考](https://platform.claude.com/docs/en/build-with-claude/thinking)
- DeepSeek Chat Completions 在携带 `tools` 的请求中核对早先各轮的 `reasoning_content` 重放，包含没有工具调用的轮次。DeepSeek Responses 验证无状态路径上的原生思考项与 `reasoning_text` 事件。[思考模式](https://api-docs.deepseek.com/guides/thinking_mode/) · [Responses](https://api-docs.deepseek.com/guides/responses_api/)
- OpenRouter Chat Completions 核对续接中 `reasoning_details` 的顺序及其签名或加密数据。OpenRouter Responses 保留有序输出项。通过实际回报的上游元数据确认真实请求由哪家供应商处理。[思考](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens) · [供应商路由](https://openrouter.ai/docs/guides/routing/provider-selection)

DeepSeek Responses 无状态，不支持 `previous_response_id`；其 Messages 兼容层会忽略 `service_tier`、`cache_control` 等选项。HTTP 成功不能证明这些设置已生效。显式不支持的选择必须可见，并区分端点能力和线路协议本身。

## 凭据与运行产物

使用 Git 外的本地凭据文件 `/code/.secrets/kuku-acceptance.env`，目录权限为 `700`，文件权限为 `600`，由用户在本机编辑密钥。`KUKU_TEST_DEEPSEEK_API_KEY` 保存直连 DeepSeek 的 key，`KUKU_TEST_OPENROUTER_API_KEY` 保存单独的 OpenRouter key。每家服务的 key 共用于其所选路线。这些是测试输入名称，不是 Codex 内建设置或新的 SDK API。[配置参考](../reference/config.md)说明如何引用环境变量凭据。

调用程序明确加载该文件，不回显内容或启用 shell 跟踪；另一终端的导出变量不一定延续到新会话。此交接需要同一文件系统与适当访问权。文件权限不能向使用同一用户身份运行的 Agent 隐藏凭据，其他主机／云环境需要另行配置凭据。

付费测试前记录用户授权的请求／费用范围、模型／设置、输出限制、超时和重试策略。执行包括重试在内的适用限额。配置中的上限不构成再次使用已完成测试额度的新授权。不要从编码 Agent 的模型推断凭据，也不要失败后静默增加思考档或预算。

使用所选 API 服务获授权的费用账本。DeepSeek 与 OpenRouter 分别授权时，各用独立账本。通用预算命令默认使用 `benchmarks/acceptance/budget.json`。使用独立账本时应显式指定相应文件。按授权范围初始化一次并保留费用历史。

```bash
python3 scripts/acceptance/run.py budget init --ledger PATH_TO_LEDGER \
  --authorization TASK_AUTHORIZATION --limit-usd 5 --opening-spent-usd 0
python3 scripts/acceptance/run.py budget show --ledger PATH_TO_LEDGER
```

`live` 和 `benchmark --mode live` 都要求 `--reserve-usd` 与 `--reservation-reason`。启动付费进程前，入口先计算 `--config` 的 hash，不复制其中可能存在的内联凭据，再锁定台账并预留保守上界，覆盖最大请求数、输入／输出 token、provider 单价和全部重试。`live` 要求 `--sdk-result` 和 `--session-data`，执行 `--` 后给出的公开 SDK／CLI 调用程序，显式从 `--secrets` 加载凭据；它不是 HTTP 连通性探测。调用程序必须实际执行预留估算所用的请求／token／重试上限。单价上限或账本预留本身不能约束模型循环；输出上限由调用者显式选择，不为适应预留额度而降低。

未完成、失败或缺失用量的运行按完整预留记账，中断及重启后仍然保留。null／缺失用量保持未知；非空请求的零输入不能证明没有费用。部分回报的计数／费用单独标识，不能作为完整总量。入口不会自动重跑付费命令或释放预留。检查证据后，才能通过 `budget settle --entry RUN_ID --cost-usd AMOUNT --basis provider_reported|token_estimate --evidence PATH` 释放多余预留。区分 provider 回报费用、按 token 估算和保守上界。再次请求前运行 `budget show`；授权余额不足会阻止启动。重复尝试需要新运行标识，并在同一授权内另作预留。

脱敏的本地预检与路由报告位于已有忽略开发输出目录中的 `benchmarks/provider-preflight.json`。重复调用前先检查其范围和覆盖。该文件未提交，不保证其他 checkout 中存在，直接 API 结果不代表重写运行时已验收。运行时间和逐请求细节保存在此类产物中，不写入架构或贡献者规范正文。

## 文档检查

回读修改的页面，检查本地链接／锚点、双语结构与含义、技术字面值和 `git diff --check`。入口或导航变化时，一起复核 `README.md`、`docs/index.md`、`docs/en/index.md`、`docs/zh/index.md`。结构检查通过不代表描述属实。

## 零警告

上述 workspace／all-target Clippy 命令要求零警告。必要的 lint 放行在对应代码位置解释。代码级规则见[代码风格](code-style.md)。
