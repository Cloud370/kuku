# 专项验收

日常检查按[测试指南](testing.md)选择。改动需要对应证据时再运行本页场景。使用待测 checkout，每次付费尝试使用唯一的 `--run-id`。[验收脚本](../../../scripts/acceptance/run.py)用各子命令的 `--help` 列出参数。

## 安装验收操作

[安装验收入口](testing.md#安装客户端验收)在未指定 `--binary` 或 `--archive` 时，使用仓库的 Node 版本，并安装内嵌 Web 资源的构建。安装位置、服务数据目录和工作区都在 checkout 之外，可通过 `--scratch-base` 选择位置。运行记录包含二进制哈希、终端类型和实际选择的场景。

| 目标 | 运行 `python3 scripts/acceptance/run.py interactive` 时附加的参数 |
| --- | --- |
| 原生终端冒烟 | `--binary PATH_TO_BINARY --native-smoke` |
| 可选开发构建的浏览器交接 | `--binary PATH --browser`，使用通过 `embedded-web-assets` 构建的二进制 |
| 工具执行期间升级 | `--archive NEW_ARCHIVE --upgrade-from OLD_ARCHIVE` |
| Linux 桌面和 IBus | `--archive ARCHIVE --desktop` |
| Windows Terminal | `--archive ARCHIVE --desktop --windows-terminal WINDOWS_TERMINAL_EXE` |

用 `--desktop-only` 替换 `--desktop` 可以只运行图形场景。需要在托管桌面上重跑 Windows Terminal 验收时，手动触发 CI 并选择 `platform=windows-desktop`。该任务使用调试版客户端运行桌面场景。

浏览器验收按 [CI](../../../.github/workflows/ci-platform.yml) 准备 Playwright Chromium。升级前后的包都要支持持久化 Session。核对重启延后、工具只完成一次，以及新服务能重新打开 Session。图形验收在 Linux 上需要 Docker，使用隔离的 Xvfb、Xfce Terminal 和 IBus。Windows 需要专用活动桌面、独立剪贴板、Pillow，以及夹具预期的键盘布局和快捷键。启用 Microsoft Pinyin 以覆盖拼音组合输入。分别记录实际通过的检查与缺失的 IME 覆盖。[安装场景](../../../scripts/acceptance/interactive.py) · [桌面夹具](../../../scripts/acceptance/desktop)

比较终端画面时保持夹具输入和窗口尺寸一致。生成一个多轮结算场景，再用另一个二进制回放，回放不发模型请求。

```bash
python3 scripts/acceptance/interactive.py --binary PATH_TO_BINARY --bridge PATH_TO_PTY_HELPER \
  --directory REVIEW_RUN --cases release-review
python3 scripts/acceptance/interactive.py --binary PATH_TO_OTHER_BINARY --bridge PATH_TO_PTY_HELPER \
  --replay-review REVIEW_RUN/release-review --renderings comparison.json
```

测量流式延迟和布局时，各构建使用不同临时目录和相同夹具。原生终端测量记录首个可见文本和突发输入回显，Rust 测量直接记录 Markdown 布局。[流式夹具](../../../scripts/acceptance/interactive_stream.py)

```bash
python3 scripts/acceptance/interactive.py --stream-profile --binary KUKU_BINARY --bridge NATIVE_PTY_HELPER --directory SCRATCH_DIR
cargo test --release -p kuku-tui stream_layout_profile -- --ignored --nocapture
```

### 付费安装验收

使用已授权的[账本与凭据文件](#凭据与运行产物)。发行包真实交接场景核对实际文件修改和测试结果、工具执行期间更换客户端，以及终端、浏览器和脚本客户端之间的交接。证据见 `live-result.json`。[真实场景](../../../scripts/acceptance/interactive_live.py)

```bash
python3 scripts/acceptance/run.py --run-id LIVE_RUN_ID interactive \
  --archive ARCHIVE --live --secrets CREDENTIAL_FILE --ledger AUTHORIZED_LEDGER
```

通过 `--intent deepseek` 或 `--intent openrouter` 验证真实模型在原生终端的回复、编辑和独立委派。选择场景，固定需要的路由，并在预留预算前核对安装版本。用 `--archive` 替换 `--binary` 可验收发行包。

```bash
python3 scripts/acceptance/run.py --run-id INTENT_RUN_ID interactive \
  --intent openrouter --intent-case delegation --binary PATH_TO_BINARY \
  --intent-model MODEL --intent-effort EFFORT --intent-upstream UPSTREAM \
  --revision EXPECTED_GIT_COMMIT --scratch-base PATH_OUTSIDE_CHECKOUT \
  --secrets CREDENTIAL_FILE --ledger AUTHORIZED_LEDGER
```

`intent/result.json` 记录场景检查、画面、用量和已确认的路由。脚本限制请求数和输出，重试也计入。付费 `release-review` 显式设置授权的 `--intent-max-requests` 和 `--intent-max-output-tokens`。如需保留 Session 用于回放，用[原生 intent 脚本](../../../scripts/acceptance/interactive_intent.py)运行 `--case release-review --keep-session`，并传入对应的 `--max-requests` 和 `--max-output-tokens`。以结果中的 `scratch_path` 作为回放输入。

单独记录人工验收。在源码树外安装候选版本，核对一次真实文件改动和测试结果，再从浏览器和另一终端恢复同一 Session。按实际体验的平台记录 IME、粘贴、复制及缩放。

## 工作区执行验收

用 SDK 或已安装 TUI 跑同一个原生日志夹具。核对精确报告、命令事实，以及重开后能复现报告的命名值。默认使用可控 Provider，产品进程使用隔离的 `PATH`。[工作区夹具](../../../scripts/acceptance/workspace.py)

```bash
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py workspace --sdk target/debug/examples/session_task
python3 scripts/acceptance/run.py workspace --binary PATH_TO_BINARY
```

用 `--archive` 验收本地发行包。与 `--sdk` 一起使用 `--handoff-only`，可验证如何把命名报告交给独立审查者。Windows 上使用 `.exe` 路径。真实运行选择 `--provider deepseek` 或 `--provider openrouter`，传入对应的 `--secrets` 和 `--ledger`，并选择模型和思考等级。需要精确 OpenRouter 上游时设置 `--upstream`。在 `workspace/result.json` 中，Provider 未确认的实际路由和思考等级保持未知。确认的路由与预期不符时验收失败。

### 命令输出与长任务验收

按[命令输出契约](../architecture/workspace-execution.md#命令输出处理提案)，复用工具、Session 和安装客户端夹具。

| 边界 | 核对 |
| --- | --- |
| 原生命令 | 参数里的空格、引号和 Unicode 保持字面值。对照授权参数、实际效果和 TUI 标题。 |
| 输出形态与失败 | 比较模型实际看到的视图和保留输出。大结果要有可用的检索入口，失败结果要保留关键诊断和采集完整性。 |
| 搜索与输出规则 | 验证跨帧匹配、授权、取消、截断和通用回退。从原始输出读取诊断引用，并验证重开后的接纳视图。 |
| Job 与长任务 | 覆盖快速结束、后台晋升、取消和唯一的最终结果。检查一个持续推进的慢 Job 和一个因具体阻碍而安静的 Job。 |

用可控夹具验证模型能收到什么，用有预算的真实任务验证模型如何选择。固定模型、思考等级、工具、预算、环境和判定器。对比短的直接任务、适合 `run_js` 的任务和较长的 Job 或检查任务。保留两种不同任务用于检验 Prompt 调整的泛化。先分别改变 Prompt 和输出视图，再比较组合改动。记录原始判定结果、重试、重复检查、Provider 用量与缓存读取、延迟、峰值 RSS 和保留输出字节。

## 共同观察 TUI 运行（可选）

在待测 checkout 使用独立的 tmux server。以下配置启用颜色并使用 tmux 默认键位。

```sh
cargo build --locked -p kuku-app --bin kuku
tmux -L kuku-review -f /dev/null new-session -d -s kuku-review -c "$PWD"
tmux -L kuku-review set-option -g default-terminal tmux-256color
tmux -L kuku-review set-option -t kuku-review mouse on
tmux -L kuku-review respawn-pane -k -t kuku-review \
  env -u NO_COLOR "$PWD/target/debug/kuku" --workspace "$PWD"
tmux -L kuku-review capture-pane -p -t kuku-review
tmux -L kuku-review attach -t kuku-review
```

等待 `Ready` 后，一方操作复现，另一方读取同一个 pane，并用 `kuku events SESSION_ID --limit 40` 核对相关事件。对比前后版本时分别使用独立窗口、夹具副本、数据目录和明确的服务配置，保持终端尺寸一致。颜色或 Alt 键异常时检查 `TERM`、`NO_COLOR` 和 `tmux list-keys -T root`。[终端操作](../reference/cli.md#终端界面) · [远程连接](../guides/run-the-server.md#显式启动服务)

## 原始 benchmark 任务

[用例清单](../../../scripts/acceptance/cases.json)负责记录用例 ID、源码版本和上游镜像。使用原始任务指令、环境和判定器。Linux 上需要 Git 和 Docker。花费模型预算前，用原始判定器检查未修改环境和官方解答。判定器依赖或镜像出错归为基础设施失败。

```bash
python3 scripts/acceptance/run.py list
python3 scripts/acceptance/run.py --run-id cancel-environment benchmark --case cancel-async-tasks --mode preflight
```

运行真实 Terminal-Bench 前构建 `session_task`，然后传入 `--sdk-binary`、`--config`、`--connection` 和[预算参数](#凭据与运行产物)。任务成败由原始判定器决定，分别记录判定结果、SDK 退出状态、产物和清理结果。分析超时时分别核对 Agent 时限、监控进程结果、持久化 Session 证据和判定结果。

用 `inspect` 核对已有持久化证据，该步骤不发起网络请求。

```bash
python3 scripts/acceptance/run.py inspect --sdk-result SDK_REPORT --session-data SESSION_DATA
```

容器生成的报告可通过 `--container-data` 指定报告里的挂载前缀。检查器核对有序记录、调用与结果配对、接纳视图、保留输出引用和关闭证据。若验收要求 SDK 观察到非空且无积压的流式输出，在 `inspect`、`live` 或真实 benchmark 上加 `--require-streaming`。

### FrontierHarness 公开小样本

用固定的 `frontier` 套件预检，验证未修改的提交无法通过原始判定器。准备 Docker 镜像和判定器依赖可能需要联网。

```bash
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py --run-id smoke-regex benchmark --suite frontier --case regex-log --mode preflight
```

真实试跑从 `--sdk-binary`、`--pi-prefix`、`--codex-prefix` 中选一个。验收脚本生成配置，用宿主计量器和 `--case-cap-usd` 限制花费。固定上游并记录模型、思考等级、输出、请求与时间上限，以及授权的共享账本。每个用例用新运行 ID。历史小样本和复现方式见[样本报告](../../../scripts/acceptance/results/frontier-sample.md)。

### 严格的本地 Kuku–Pi 对照

使用支持 cgroup v2 和 `memory.peak` 的 Linux Docker。用全新容器、原始任务和判定器，固定模型和思考等级，并对齐请求、时间、CPU、内存与花费上限。严格对照默认使用一个 CPU、2 GiB 内存且无交换分区。比较时间时在同一空闲主机运行，多次试验交替启动顺序。单独记录 TUI 观察运行。

示例使用锁定的 Pi 1.5.2、兼容的 Linux Node.js（22.19+）和已有的授权账本。保留 npm 锁文件，使用新运行 ID，开跑前填入单次授权上限。

```sh
pi_ref="$PWD/benchmarks/tools/pi-1.5.2"
npm install --prefix "$pi_ref" --registry=https://registry.npmjs.org --save-exact \
  --ignore-scripts --no-audit --no-fund @earendil-works/pi-coding-agent@1.5.2
cargo build --release --locked -p kuku --example session_task

case_id=large-scale-text-editing
ledger=PATH_TO_AUTHORIZED_LEDGER
keys=PATH_TO_CREDENTIAL_FILE
case_cap=AUTHORIZED_CASE_CAP_USD

paired_run() {
  run_id="$1"
  shift
  python3 scripts/acceptance/run.py --run-id "$run_id" benchmark \
    --suite frontier --case "$case_id" --mode live --provider deepseek --require-resource-evidence \
    --max-output-tokens 131072 --case-cap-usd "$case_cap" --ledger "$ledger" --secrets "$keys" "$@"
}
paired_run vim-kuku --sdk-binary target/release/examples/session_task
paired_run vim-pi --pi-prefix "$pi_ref"

python3 scripts/acceptance/benchmark_compare.py \
  --kuku benchmarks/acceptance/vim-kuku --pi benchmarks/acceptance/vim-pi \
  > benchmarks/acceptance/vim-comparison.json
```

导出工具先核对匹配的输入，再报告判定结果、时间、请求和花费、harness RSS、容器峰值内存、CPU 及 OOM 事件。缺失的测量保持未知。随结果记录资源限制、构建 profile、SDK 哈希和全部尝试。

### 本地严格 Kuku–Codex 对照

固定一个隔离的 Codex npm 安装。把选定的 `deepseek-flash` 模型目录条目放到该安装前缀中的 `deepseek-flash-models.json`。[DeepSeek 的 Codex 接入说明](https://api-docs.deepseek.com/quick_start/agent_integrations/codex/#option-2-edit-the-configuration-file-manually)

```sh
codex_ref="$PWD/benchmarks/tools/codex-0.164.0"
npm install --prefix "$codex_ref" --registry=https://registry.npmjs.org --save-exact \
  --ignore-scripts --no-audit --no-fund @openai/codex@0.164.0
```

沿用对照参数，并给两组都加 `--wire openai-responses`。Kuku 选 `--sdk-binary`，Codex 选 `--codex-prefix "$codex_ref"`。用 `benchmark_compare.py --kuku RUN_DIRECTORY --codex RUN_DIRECTORY` 导出。记录二进制、npm 锁文件和模型目录的哈希。

| 比较内容 | 只改变一个输入 | 导出参数 |
| --- | --- | --- |
| Prompt | `--prompts-dir` | `--baseline RUN_DIRECTORY --candidate RUN_DIRECTORY` |
| 思考等级 | 直连 DeepSeek 的 `--reasoning-effort` | `--compare-effort --baseline RUN_DIRECTORY --candidate RUN_DIRECTORY` |
| SDK 构建 | `--sdk-binary` | `--compare-sdk --baseline RUN_DIRECTORY --candidate RUN_DIRECTORY` |

其余设置保持一致。测试命令时限或请求字节上限时，对照组固定相同的 `--max-command-seconds` 或 `--max-request-bytes`。只有核对已提交的上下文容量切换和保留输出引用，才报告该运行实际触发了上下文恢复。[对照导出脚本](../../../scripts/acceptance/benchmark_compare.py)

## 资源测量

```bash
python3 scripts/acceptance/run.py metrics --profile release
python3 scripts/acceptance/run.py metrics --profile release --cpu-count 1 --address-space-mib 512
```

[资源测量程序](../../../crates/kuku/examples/resource_probe.rs)使用可控 Provider 运行公开 SDK 负载，覆盖空闲 Agent、入队、重开、输出保留和共享脚本 worker 压力。可用负载见 `metrics --help`。Linux 上可将 CPU 亲和度限制应用于调用方和子进程，地址空间限制按进程生效。进程采样支持 Linux 和 Windows。

用 `caller.json` 比较构建、阶段、进程限制和负载。分别报告 SDK、工具子进程和磁盘占用。请求准备耗时取自持久化的准备事实。未采样的数值保持未知。除了任务结果，还要看 `measurements_complete`。

### 大型会话压测

```bash
python3 scripts/acceptance/run.py metrics --profile release --long-session-mib 500
```

合成负载不产生付费请求。它生成大型日志，再用另一个进程重开以验证历史、锚点和回退。调试夹具时先用较小规模。在相同负载、文件系统缓存条件、构建 profile 和进程限制下比较 `fixture.json` 与 `caller.json`。压测会保留生成的数据，先确认剩余磁盘空间。

## 真实协议验收

选择能回答验证问题且已授权的连接、模型和协议。每条路由用能确认行为的最小负载。例如，检查真实工具续接时，可以只做一次流式工具调用并回传结果。验证跨 Provider 委派时，再在相关连接上使用独立 Agent。[连接配置](../reference/config.md)

用可控的 `protocol` 套件验证流式参数组装、错误处理、回放和协议原生工具续接。真实调用记录已确认的上游、请求设置，以及 Provider 回报的设置和用量。缺少回报的值保持未知。比较路由时固定模型、思考等级、负载和预算，分别测量冷请求与持续 Session。

使用 `live` 时传入 `--config`、`--sdk-result`、`--session-data`，在 `--` 后运行 SDK 或 CLI 调用方。用 `--secrets` 传入凭据文件，用 `--ledger` 传入授权账本。验收脚本核对调用方记录的持久化证据。遵循[付费运行前提](#凭据与运行产物)。

## 凭据与运行产物

凭据使用 Git 外的独立私有文件。本地约定是 `/code/.secrets/kuku-acceptance.env`，目录权限为 `700`，文件权限为 `600`。`KUKU_TEST_DEEPSEEK_API_KEY` 和 `KUKU_TEST_OPENROUTER_API_KEY` 分别属于相应服务。载入凭据时避免打印内容或打开 shell 跟踪。[凭据配置](../reference/config.md)

付费尝试前记录已授权的花费、请求数、输出上限、超时和重试策略。重试也受同样限制。使用该服务的授权账本，不同授权使用不同账本。每次授权只初始化一次账本，保留花费历史。

```bash
python3 scripts/acceptance/run.py budget init --ledger LEDGER \
  --authorization AUTHORIZATION_REF --limit-usd AUTHORIZED_USD --opening-spent-usd 0
python3 scripts/acceptance/run.py budget show --ledger LEDGER
```

`live` 和 Terminal-Bench 真实运行要求 `--reserve-usd` 和 `--reservation-reason`。预留额按保守费率覆盖所有获准请求。Frontier 真实运行使用宿主计量器和 `--case-cap-usd`。付费 `interactive` 和 `workspace` 要显式传入凭据和账本。SDK 和转发计量器负责执行各自的限制，花费上限本身不能限制生成长度。

失败、未完成或缺少用量回报的尝试保留全部预留额，直到有证据可以结算。缺少用量不代表花费为零。核对证据后再释放未用的预留额，并记录结算依据。

```bash
python3 scripts/acceptance/run.py budget settle --ledger LEDGER --entry RUN_ID \
  --cost-usd AMOUNT --basis provider_reported --evidence PATH_TO_EVIDENCE
```

适用时用 `--basis token_estimate` 或 `conservative_upper_bound`。再次请求前确认剩余授权，每次复跑使用新的运行 ID。Provider 请求内容、凭据和 Session 日志保持私有。
