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

交互式启动 `kuku` 时，如果本地配置文件不存在，或现有配置的有效默认 tier provider 凭据缺失、不可读或为空，TUI 会在打开 Session 前提供共享设置流程。流程会保留并预填现有配置，只使用运行时相同的环境变量、文件或明文凭据解析方式检查默认路由对应的 provider；不会验证认证。可读且非空的默认凭据会跳过设置，格式错误的配置会报错而不会被覆盖。本地连接也可运行 `kuku init`（或 `kuku-cli init`）打开相同流程；该命令要求 TTY，非交互运行不会写入文件。流程遵循 `--data-dir`、`KUKU_HOME` 和 `--config`。`kuku init` 遇到正在运行的受管服务时，会使用经认证的活动配置路径；显式指定冲突的 `--config` 会报错。首次运行引导在活动服务、无法检查服务状态或远程连接时不会打开；配置流程不会为了配置而启动或重启服务。

选择已有命名连接或 preset，再编辑当前默认 tier 的模型 ID。模型 ID 不能以 `$` 开头，因为配置加载器会将其解释为环境变量引用。第 3 步会显示环境变量名称及是否检测到非空值；这只是存在性检查，不是认证验证，也不会显示变量值。所选环境引用或 preset 对应变量缺失或为空且私有存储安全时，默认选择 `Paste API key (save locally)`；检测到非空变量时，默认保留 Environment。若存储不可用，则保持 Environment，并在 Review 中提示在启动前设置变量。已有文件或明文凭据继续使用粘贴保存的 key 来源。你可以显式选择 Environment，设置流程不会擅自切换该选择；两个来源之间不会自动回退。

粘贴输入会被遮蔽；安全存储可用时，密钥以未加密明文保存在工作区之外 `<data-dir>/credentials/` 下的私有文件中。首次运行凭据未就绪时，Review 默认选择返回粘贴输入修复；存储不可用时，则显示在启动前设置环境变量的安全指引。空白粘贴不会离开凭据输入页。`Save and exit` 仍是明确的次要选项；如果默认凭据仍缺失，下次启动会再次提供首次运行设置。凭据就绪时，首次运行默认选择 `Save and continue`。设置不会发起模型请求，也不会验证 provider 认证。

首次创建配置时，仍会初始化 `strong`、`balanced` 和 `light` 三个默认 tier。编辑已有配置只修改选中的默认 tier 和实际更改的连接字段；注释、自定义 endpoint、provider options、其他 provider 和 tier 都会保留。已有内联密钥不会显示，只在显式保存时迁移到私有存储。已位于当前服务凭据目录中的文件引用在输入留空时会保留；其他有效的私有文件引用只在显式保存时迁移到当前数据目录，原文件不会删除。格式错误的配置不会被覆盖。取消或返回不会保存配置或密钥文件。

生成的配置以 `balanced` 为默认 tier，并将所选 provider 和模型用于 `strong`、`balanced`、`light` 三个 tier。编辑 `config.toml` 或运行 `kuku config set` 可以修改持久默认值和各个 tier。在 TUI 中，普通的 `/model` 和 `/thinking` 选项只作用于所选 Agent 的 Session。若要持久保存已解析的连接和模型，请选择 **Set current model as default**，并确认配置路径和 tier。只有没有现有 tier 匹配时才会写入名为 `user-default` 的普通 tier；不会创建新的优先级，也不会改变当前 Session。保存的默认值适用于新建 Session 和没有单独选择模型的 Session；重新打开未显式指定模型的 Session 时可能会使用更新后的默认值。

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

默认安装定义了三个 tier：

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
