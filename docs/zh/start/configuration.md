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

交互式启动 `kuku` 时，如果本地配置文件不存在，TUI 会在打开 Session 前提供简短设置流程。它使用与本地服务相同的解析路径，遵循 `--data-dir`、`KUKU_HOME` 和 `--config`，并显示该路径与工作区。所选配置路径只要已存在（包括格式错误的文件），或连接是远程连接，就会跳过设置。

选择 provider、字面模型 ID 和凭据环境变量的**名称**。模型 ID 不能以 `$` 开头，因为配置加载器会将其解释为环境变量引用。设置流程不会显示或保存密钥值，也不会发起模型请求。它只检查指定变量是否有非空值；不会验证凭据或模型 ID。如果变量不存在或为空，可以保存并退出，再在启动 `kuku` 的 shell 中设置变量并重新启动。取消不会创建配置文件。

生成的配置以 `balanced` 为默认 tier，并将所选 provider 和模型用于 `strong`、`balanced`、`light` 三个 tier。编辑 `config.toml` 或运行 `kuku config set` 可以修改持久默认值和各个 tier。在 TUI 中，`/model` 只修改所选 Agent 当前 Session 的配置，不会改变已保存的配置默认值。

## 设置模型连接

默认配置定义了两个 provider：

- `provider.anthropic`，其中 `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai`，其中 `api_key = "$OPENAI_API_KEY"`

可通过环境变量提供密钥，也可以在 `config.toml` 中直接设置 `api_key`。

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
