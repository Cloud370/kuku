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

交互式启动 `kuku` 时，如果本地配置文件不存在，TUI 会在打开 Session 前提供共享设置流程。本地连接也可运行 `kuku init`（或 `kuku-cli init`）打开相同流程；该命令要求 TTY，非交互运行不会写入文件。流程遵循 `--data-dir`、`KUKU_HOME` 和 `--config`。若受管服务正在运行，设置会使用经认证的活动配置路径；显式指定冲突的 `--config` 会报错。设置不会为了配置而启动服务。已有配置、活动服务或远程连接会跳过首次运行设置。

选择已有命名连接或 preset，再编辑当前默认 tier 的模型 ID。模型 ID 不能以 `$` 开头，因为配置加载器会将其解释为环境变量引用。凭据来源必须明确选择：环境变量名称或粘贴保存的 key。环境变量模式只检查变量是否非空，不会显示其值。保存 key 模式会遮蔽输入，并将密钥以未加密明文形式保存在 `<data-dir>/credentials/` 下的私有文件中，且位于工作区之外。两个来源之间不会自动回退。设置不发起模型请求。环境变量缺失或为空时，仍可保存并退出，再在服务端环境中设置后重启。

首次创建配置时，仍会初始化 `strong`、`balanced` 和 `light` 三个默认 tier。编辑已有配置只修改选中的默认 tier 和实际更改的连接字段；注释、自定义 endpoint、provider options、其他 provider 和 tier 都会保留。已有内联密钥不会显示，只在显式保存时迁移到私有存储。已位于当前服务凭据目录中的文件引用在输入留空时会保留；其他有效的私有文件引用只在显式保存时迁移到当前数据目录，原文件不会删除。格式错误的配置不会被覆盖。取消或返回不会保存配置或密钥文件。

生成的配置以 `balanced` 为默认 tier，并将所选 provider 和模型用于 `strong`、`balanced`、`light` 三个 tier。编辑 `config.toml` 或运行 `kuku config set` 可以修改持久默认值和各个 tier。在 TUI 中，`/model` 只修改所选 Agent 当前 Session 的配置，不会改变已保存的配置默认值。

## 设置模型连接

默认配置定义了两个 provider：

- `provider.anthropic`，其中 `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai`，其中 `api_key = "$OPENAI_API_KEY"`

使用明确的 `api_key = "$VARIABLE_NAME"` 环境变量引用，或 `api_key = "file:<绝对路径>"`。交互式设置保存的新密钥位于服务数据目录，TOML 只记录绝对路径引用。文件是未加密明文。详见 [Config](../reference/config.md) 和[文件布局](../reference/file-layout.md)。

环境变量由服务进程读取。如果在受管服务启动后修改了终端里的密钥，等服务空闲后从该终端运行 `kuku server restart`。使用远程连接时，在服务端机器上设置密钥。生效配置文件的修改通过校验后，服务会在打开 Session 时使用新配置。

其他字符串设置也可以在 `config.toml` 中使用 `$ENV_VAR_NAME`。例如，`base_url = "$KUKU_ANTHROPIC_BASE_URL"` 会在加载配置时解析。

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
