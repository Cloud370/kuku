# 配置 kuku

## 配置文件位置

默认情况下，kuku 使用：

```text
~/.kuku/config.toml
```

如果设置了 `KUKU_HOME`，服务默认读取以下文件。

```text
$KUKU_HOME/config.toml
```

本地服务默认读取所选数据目录下的 `config.toml`。通过 `--config` 可以选择其他文件。运行 `kuku server status` 查看生效路径。已运行的受管服务要切换配置文件时，在服务空闲后运行 `kuku server restart --config PATH`。[文件布局](../reference/file-layout.md) · [运行服务](../guides/run-the-server.md)

## TUI 首次运行设置

交互式启动 `kuku` 时，如果本地配置文件不存在或为空，或现有配置的有效默认 tier provider 凭据缺失、不可读或为空，TUI 会在打开 Session 前提供共享设置流程。流程会保留并预填现有配置，只使用运行时相同的环境变量、文件或明文凭据解析方式检查默认路由对应的 provider；不会验证认证。可读且非空的默认凭据会跳过设置，格式错误的配置会报错而不会被覆盖。本地连接也可运行 `kuku init`（或 `kuku-cli init`）打开相同流程；该命令要求 TTY，非交互运行不会写入文件。流程遵循 `--data-dir`、`KUKU_HOME` 和 `--config`。`kuku init` 遇到正在运行的受管服务时，会使用经认证的活动配置路径；显式指定冲突的 `--config` 会报错。首次运行引导在活动服务、无法检查服务状态或远程连接时不会打开；配置流程不会为了配置而启动或重启服务。

选择一次 **Find and test models**，即可允许本次设置查询模型目录并发送简短的文本或图片试跑。每次试跑请求的输出上限为 4096 token，超时为 60 秒，可能消耗 API 余额。选择 **Edit config file** 会显示配置路径，供应商请求数为零。

设置会检查所选模型接受哪些思考档位，优先使用 API 声明的档位。保存页逐个显示所选模型的进度，包括排队的测试、重试倒计时和可用档位。**Model tests** 按模型展示所有测试及其结果。生成请求按所选模型数量共享两到四个并发名额，其余测试仍在运行时可以先保存已有结果。被接受的档位可以在日常使用中发送。API 回报的实际档位或思考活动单独显示。每页底部会提示 Esc 的作用。

选择自定义连接、已有连接或预设，再填写供应商 URL、网络路径和密钥。已有连接和预设沿用其协议，标准请求 URL 可直接识别。新自定义 Base URL 在选择模型后验证协议，无法确定时才出现 API 类型选项。网络可选择跟随代理环境变量、直连，或使用 HTTP、SOCKS5、SOCKS5H。裸域名有多个标准候选路径时，设置会并发查询，逐条显示进度和结果，找到可用目录就自动打开模型列表。目录无法读取时，可以直接填写已知的模型 ID。

模型列表和搜索结果均按供应商返回的顺序显示。打字搜索，方向键移动，Enter 确认单选。按 Space 勾选多个模型，再用 Enter 确认整组。新配置选中多个模型时，可指定其中一个作为默认。确认模型后直接进入保存页，同时在后台试跑。可以查看进度，也可以打开 **Model tests** 查看逐条请求。随时保存即可开始使用，保存时会停止剩余试跑。保存页显示模型、请求地址和测试状态。打开 **Settings** 可修改思考、输出、模型能力和计费方式。**Costs** 可设置费率。在状态栏添加 **Request cost** 可查看费用汇总。[代理与费率配置](../reference/config.md#providername)

在 Session 中打开 `/model` 或按 Ctrl+L。**Add model or connection** 和 **Edit** 打开同一个表单。选用已有连接时可复用保存的凭据。**Use in this Session** 修改所选 Agent 的路由，**Set as default** 将路由保存为新 Session 的默认值。`/thinking` 分别提供 Session 覆盖和模型持久设置。[配置参考](../reference/config.md)

## 设置模型连接

默认配置定义了两个 provider：

- `provider.anthropic`，其中 `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai`，其中 `api_key = "$OPENAI_API_KEY"`

使用明确的 `api_key = "$VARIABLE_NAME"` 环境变量引用，或 `api_key = "file:<绝对路径>"`。交互式设置保存的新密钥位于服务数据目录，TOML 只记录绝对路径引用。文件是未加密明文。详见 [Config](../reference/config.md) 和[文件布局](../reference/file-layout.md)。

环境变量由服务进程读取。如果在受管服务启动后修改了终端里的密钥，等服务空闲后从该终端运行 `kuku server restart`。使用远程连接时，在服务端机器上设置密钥。生效配置文件的修改通过校验后，服务会在打开 Session 时使用新配置。

其他字符串设置也可以在 `config.toml` 中使用 `$ENV_VAR_NAME`。例如，`base_url = "$KUKU_ANTHROPIC_BASE_URL"` 会在加载配置时解析。

### 认证失败后的恢复

终端中的 provider 认证失败后，用 F2 选择 **Repair credentials** 并按 Enter。修复绑定实际失败的连接和模型。在受管本地服务中，在遮蔽输入框里输入替换密钥，按 Enter 检查，再确认 **Save credentials and restart local service** 应用变更。只修改该 provider 的 `api_key`，保留当前模型选择和其他配置。密钥使用与首次设置相同的私有文件存储，不进入主输入框或日志；保存不代表认证已经验证。

其他执行让服务保持忙碌时，**Credentials saved; service not restarted** 页面允许稍后重试安全重启。终端区分“已保存”和“已应用到当前 Session”，不会自动重试失败请求。确认使用同一配置的新服务实例启动后，终端恢复原 Session、Agent 和草稿。再通过 Continue 或界面中的恢复动作，明确重试保留的原输入，不必重新发送一份。

远程或使用 token 的连接须在服务端修复凭据，必要时安全重启，再重连并继续；终端不会为这类连接写入本地凭据。

## 模型档

可选的命名路由示例：

- `strong`
- `balanced`
- `light`

每个模型档指定连接、模型、思考等级和 token 限额。`default_model = "balanced"` 选择默认 Tier。使用 `--tier <name>` 选择其他 Tier，或 `--connection <name> --model <model-id>` 精确指定连接和模型。

## 常用配置命令

显示当前配置：

```bash
kuku config show
```

校验文件：

```bash
kuku config validate
```

设置一个值：

```bash
kuku config set model.balanced.think high
```

## 资源发现、上下文交接、日志、插件与更新

主要的非 provider 分区有：

- `[discovery]`，用于 Agent 和 Skill 自动发现
- `[handoff]`，用于长 Session 摘要阈值
- `[logs]`，用于可观测性日志保留策略
- `[plugin]`，用于 package 扩展加载
- `[update]`，用于 release 来源和 channel 设置

日志默认开启，且没有禁用开关。默认保留策略为 `max_age_days = 14` 和 `max_total_size_mb = 512`。

准确的键定义见 [Config](../reference/config.md)。

## 更新渠道

默认的 update 分区使用：

```toml
[update]
source = "github"
channel = "stable"
```

这会控制 kuku 应当跟随哪个 release manifest。参见 [Update Manifest](../reference/update-manifest.md)。
