# 配置 kuku

## 配置文件位置

默认情况下，kuku 使用：

```text
~/.kuku/config.toml
```

如果设置了 `KUKU_HOME`，CLI 会读取以下文件。

```text
$KUKU_HOME/config.toml
```

启动 `kuku server` 或 `kuku web` 时，如果配置在 `~/.kuku` 以外，请通过 `--config` 指定文件。[文件布局](../reference/file-layout.md) · [运行服务](../guides/run-the-server.md)

## 设置模型连接

默认配置定义了两个 provider：

- `provider.anthropic`，其中 `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai`，其中 `api_key = "$OPENAI_API_KEY"`

可通过环境变量提供密钥，也可以在 `config.toml` 中直接设置 `api_key`。

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
