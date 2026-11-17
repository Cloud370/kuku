# Testing

本页维护验证入口、真实测试配置和证据要求。[开发指南](development.md)维护工作循环，[演进说明](../architecture/evolution.md#验证与测量)定义重构所需的具体 case 与测量。

## 选择验证范围

按改动行为选择检查。持续维护的选择入口使用 Python 3.10+，将有界证据写入已忽略的 `benchmarks/acceptance/` 目录：

```bash
python3 scripts/acceptance/run.py contract
python3 scripts/acceptance/run.py protocol
python3 scripts/acceptance/run.py integration
```

`contract` 启用 `kuku/test_support`，运行 SDK、CLI 和 server 测试，包含受 feature 控制的运行时套件。`protocol` 选择 provider 适配、配置、错误、集成、追踪及缓存通知固定输入，不发起付费请求。`integration` 依次执行只读格式检查、启用测试 feature 的 workspace／all-target Clippy 和 `contract`。CI 在 Linux、macOS 和 Windows 运行同一契约选择并保留有界证据。旧有嵌入宿主编译检查单独保留；真实模型和 Linux benchmark 容器是需明确选择的本地检查。

编辑时使用针对性检查，在运行时集成批次收尾执行一次所需 Rust 集成检查。等价的直接命令为：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features kuku/test_support -- -D warnings
cargo test -p kuku -p kuku-cli -p kuku-server --features kuku/test_support
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

## 测试归属与证据

单元测试保留在 `#[cfg(test)] mod tests`；集成测试按领域位于 `tests/` 下，共用 helper 位于 `tests/common/mod.rs`。复用已有调用程序与固定输入。重构应提供一个持续维护的入口或小型 case 清单，选择契约、重放、真实调用及 benchmark 检查，不另建评测平台。

每次选择运行生成一个 `result.json`，包含提交、未提交状态及已跟踪差异的 hash、所选命令／feature、退出状态、耗时和证据位置。每个命令最多保留 64 KiB 脱敏输出，同时记录原始字节数、hash 和明确的截断标记。超时会在 POSIX 清理所属进程组，在 Windows 请求终止进程树；未完整采集视为失败。父进程已经退出后的 Windows 后代清理尚未经本机验证。真实 SDK 检查独立核对调用程序的持久事实、设置、用量与清理，仅命令成功不能通过。

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

真实尝试先构建公开 `session_task` 示例，再选择 `benchmark --mode live`，提供 `--sdk-binary`、`--config`、`--connection` 及下方的预算参数。调用程序的其他有界参数放在 `--` 后。二进制及其解析出的 Linux 动态库复制到独立运行时挂载；证据记录这一必要宿主适配的 hash。Agent 执行时只能访问原任务指令和环境，不提供 solution，隐藏测试在执行结束后才复制进容器。Session 数据保留在该次尝试的 `session-data/`；预期任务产物列于清单。

原 reward、收集／通过／失败测试数、SDK 退出状态与清理结果分别记录。预期任务产物在判定前保存，每个上限 32 MiB；证据缺失或过大不能通过真实验收记录。SDK 检查器流式读取原始 `events.jsonl`，要求已提交游标有序、准备请求与响应对应、工具调用／结果配对、已接纳结果视图不变，以及 `KUKUOUT1` 输出引用的帧和字节数有效并已完成写入。SDK 统计／设置／报告必须匹配这些事实，运行时与持久关闭屏障均须成功。请求与费用按 Agent、Turn 和请求标识关联，后续 Turn 可以复用 `req_1`。整个 Session 的用量与所选 Agent 的快照分别记录。任务成功仍由原 benchmark 判定器决定。

检查器每个事件最多读取 1 MiB，累计最多 100,000 个事件，SDK 报告上限 4 MiB；只保留有界 hash／元数据，不复制上下文历史。诊断上限为 4,096 个准备请求及 8,192 个工具调用／provider 报告。无需再次调用模型即可复核已有证据：`inspect --sdk-result PATH --session-data DIR --container-data /kuku/data --connection NAME`；报告使用宿主路径时省略 `--container-data`。此只读选择不要求凭据或费用预留。

SDK 运行时，入口分别采样 SDK RSS／RSS 高水位、CPU 时间和线程数，以及整个容器的 CPU、内存、进程数和 I/O。采样与输出日志均有界；未采样值保持 `null`。这些样本不能证明空闲 Agent 占用、冷打开／恢复、追加／上下文准备或并发 Agent 测量。后者从相关 SDK 检查记录，不把整容器数据归因于 SDK。

## 真实协议验收

用户选定直连 DeepSeek 的 `deepseek-flash` 作为低成本 benchmark 主力，第二个网关使用 OpenRouter 的 [`deepseek/deepseek-v4.1-flash-1017`](https://openrouter.ai/deepseek/deepseek-v4.1-flash-1017)。两家服务的文档提供下表中的三条协议路线。这些是测试连接输入，不是新冻结的 SDK 配置名称。

| 服务 | 线路协议 | 基础 URL | 请求后缀 | 来源 |
|---|---|---|---|---|
| DeepSeek | Chat Completions | `https://api.deepseek.com` | `/chat/completions` | [首次调用](https://api-docs.deepseek.com/) |
| DeepSeek | Responses | `https://api.deepseek.com` | `/responses` | [Responses 兼容说明](https://api-docs.deepseek.com/guides/responses_api/) |
| DeepSeek | Messages | `https://api.deepseek.com/anthropic` | `/v1/messages` | [Messages 兼容说明](https://api-docs.deepseek.com/guides/anthropic_api/) |
| OpenRouter | Chat Completions | `https://openrouter.ai/api/v1` | `/chat/completions` | [快速开始](https://openrouter.ai/docs/quickstart) |
| OpenRouter | Responses | `https://openrouter.ai/api/v1` | `/responses` | [Responses 参考](https://openrouter.ai/docs/api/api-reference/responses/create-a-response) |
| OpenRouter | Messages | `https://openrouter.ai/api/v1` | `/messages` | [Messages 参考](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message) |

阶段 1 从 DeepSeek 开始，最迟在阶段 2 交付时完成两家服务上的三种协议验证。每种都要覆盖流式输出、真实工具调用及结果续接、多轮上下文和可获得的用量／设置报告。反复运行的 benchmark 主要使用 DeepSeek；OpenRouter 使用一个固定小任务检查兼容性，不把每个 benchmark 在所有模型／协议组合上重跑。选定组合覆盖同一模型家族的两条接入路径，不代表已覆盖不同模型家族。其他模型家族保留为后续可选验证，不要求首批交付前另购或另选两个模型。委派任务包含直连 DeepSeek 和经 OpenRouter 调用的 Agent，保持独立上下文。

仅验证协议时使用实际支持的最低已开启思考档。DeepSeek 当前的[思考指南](https://api-docs.deepseek.com/guides/thinking_mode/)将该档定义为 `low`；OpenRouter 则核对所选上游支持的设置，按所选 API 编码，不假定所有端点接受同一字段和值。benchmark 的思考档按 case 和预算选择，明确记录，同一对比中保持固定；修改档位另记一轮运行，不在失败后静默升档。这些设置只针对被测模型，不改变执行重构的 Codex 会话及其开发 Agent 的模型策略。

为使 OpenRouter 检查可重复，先选择账号路由策略允许的上游，再通过 `provider.order` 选择一个准确的上游端点，设置 `provider.allow_fallbacks = false`、`provider.require_parameters = true`，不使用自动选模或模型回退。其[路由文档](https://openrouter.ai/docs/guides/routing/provider-selection)说明，默认路由可能回退，也可能把不支持的参数传给会忽略它的供应商。记录请求模型、网关、指定路线及可获得的实际上游回报；无法确认时保持未知。这些是测试连接选项，不是全局运行时调度规则。手工指定供应商顺序会覆盖 OpenRouter 的自动粘性路由；固定端点仍可复用自己的缓存。

日常多轮使用建议采用稳定的 `session_id` 或 `x-session-id`、参数支持过滤，以及候选供应商的成本约束，保留自动粘性路由。传输关联由已有 Session／Agent／连接身份派生，跨 Turn、重试和重开保持稳定；不同 Agent 使用不同关联。不新增核心身份，也不在每个 Step 生成新关联。这属于 provider 适配行为，不另建调度器或共享 Agent 对话历史。遵循[提示缓存契约](https://openrouter.ai/docs/guides/best-practices/prompt-caching)：明确会话关联可从首次成功请求建立亲和，供应商失败可能改变路线。文档所述空闲 60 分钟过期指路由亲和，不保证上游缓存存活同样长。稳定路由不能替代不可变的 provider 可见前缀。

OpenRouter 当前默认路由综合考虑有效价格，包括观察到的缓存使用和输入／输出比例，以及性能与可靠性，不承诺单次请求绝对最低价。选择供应商白名单或价格上限前，应按实际工作负载比较未缓存输入、缓存读／写及输出费用。`provider.max_price` 限制可选单价，不是会话总费用上限，运行预算仍需单独约束。可选的 `provider.only` 可缩小候选范围而不手工排序。多轮检查可获得的缓存 token 用量、最终费用和实际路线。这些配置建议需要用所选端点验证，不保证一定节约费用。

罕见或不确定出现的错误、批次，以及可能被网关规范化的供应商特有重放数据，使用针对性的原生协议固定输入补充验证。OpenRouter 覆盖属于网关覆盖，不证明同样请求可直连 OpenAI 或 Anthropic 官方端点。具备凭据时增加官方直连检查，但不把开通新账号作为启动重构的前提。准确记录跑过的路径，缺少的真实覆盖标记为未验证。

DeepSeek Responses 无状态，不支持 `previous_response_id`；其 Messages 兼容层会忽略 `service_tier`、`cache_control` 等选项。HTTP 成功不能证明这些设置已生效。显式不支持的选择必须可见，并区分端点能力和线路协议本身。

## 凭据与运行产物

使用 Git 外的本地凭据文件 `/code/.secrets/kuku-acceptance.env`，目录权限为 `700`，文件权限为 `600`，由用户在本机编辑密钥。`KUKU_TEST_DEEPSEEK_API_KEY` 保存直连 DeepSeek 的 key，`KUKU_TEST_OPENROUTER_API_KEY` 保存单独的 OpenRouter key。每家服务的 key 共用于其三个协议。这些是测试输入名称，不是 Codex 内建设置或新的 SDK API。[配置参考](../reference/config.md)说明如何引用环境变量凭据。

调用程序明确加载该文件，不回显内容或启用 shell 跟踪；另一终端的导出变量不一定延续到新会话。此交接需要同一文件系统与适当访问权。文件权限不能向使用同一用户身份运行的 Agent 隐藏凭据，其他主机／云环境需要另行配置凭据。

付费测试前记录用户授权的请求／费用范围、模型／设置、输出限制、超时和重试策略。执行包括重试在内的适用限额。配置中的上限不构成再次使用已完成测试额度的新授权。不要从编码 Agent 的模型推断凭据，也不要失败后静默增加思考档或预算。

`benchmarks/acceptance/budget.json` 是本次重构唯一的本地费用台账。按用户的任务授权初始化一次，不通过重建台账清零费用：

```bash
python3 scripts/acceptance/run.py budget init --authorization TASK_AUTHORIZATION --limit-usd 5 --opening-spent-usd 0
python3 scripts/acceptance/run.py budget show
```

`live` 和 `benchmark --mode live` 都要求 `--reserve-usd` 与 `--reservation-reason`。启动付费进程前，入口先计算 `--config` 的 hash，不复制其中可能存在的内联凭据，再锁定台账并预留保守上界，覆盖最大请求数、输入／输出 token、provider 单价和全部重试。`live` 要求 `--sdk-result` 和 `--session-data`，执行 `--` 后给出的公开 SDK／CLI 调用程序，显式从 `--secrets` 加载凭据；它不是 HTTP 连通性探测。调用程序必须实际执行预留估算所用的请求／token／重试上限。单价上限或账本预留本身不能约束模型循环；输出上限由调用者显式选择，不为适应预留额度而降低。

未完成、失败或缺失用量的运行按完整预留记账，中断及重启后仍然保留。null／缺失用量保持未知；非空请求的零输入不能证明没有费用。部分回报的计数／费用单独标识，不能作为完整总量。入口不会自动重跑付费命令或释放预留。检查证据后，才能通过 `budget settle --entry RUN_ID --cost-usd AMOUNT --basis provider_reported|token_estimate --evidence PATH` 释放多余预留。区分 provider 回报费用、按 token 估算和保守上界。再次请求前运行 `budget show`；授权余额不足会阻止启动。重复尝试需要新运行标识，并在同一授权内另作预留。

脱敏的本地预检与路由报告位于已有忽略开发输出目录中的 `benchmarks/provider-preflight.json`。重复调用前先检查其范围和覆盖。该文件未提交，不保证其他 checkout 中存在，直接 API 结果不代表重写运行时已验收。运行时间和逐请求细节保存在此类产物中，不写入架构或贡献者规范正文。

## 文档检查

回读修改的页面，检查本地链接／锚点、双语结构与含义、技术字面值和 `git diff --check`。入口或导航变化时，一起复核 `README.md`、`docs/index.md`、`docs/en/index.md`、`docs/zh/index.md`。结构检查通过不代表描述属实。

## 零警告

上述 workspace／all-target Clippy 命令要求零警告。必要的 lint 放行在对应代码位置解释。代码级规则见[代码风格](code-style.md)。
