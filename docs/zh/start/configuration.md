# Configuration

## Config File Location

默认情况下，kuku 使用：

```text
~/.kuku/config.toml
```

如果设置了 `KUKU_HOME`，kuku 会使用：

```text
$KUKU_HOME/config.toml
```

参见 [File Layout](../reference/file-layout.md)。

## Provider Setup

默认配置定义了两个 provider：

- `provider.anthropic`，其中 `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai`，其中 `api_key = "$OPENAI_API_KEY"`

你可以把 secret 放在环境变量里，也可以在 `config.toml` 中直接写入字面量 `api_key`。

## Model Tiers

默认安装定义了三个 tier：

- `strong`
- `balanced`
- `light`

每个 tier 都会映射到一个 provider、一个 model、一个 thinking level，以及 token limits。`default_model = "balanced"` 表示当你不传 `--model` 时使用这个 tier。

## Common Changes

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

## Discovery, Handoff, Plugins, and Updates

主要的非 provider 分区有：

- `[discovery]`，用于 Agent 和 Skill 自动发现
- `[handoff]`，用于长 Session 摘要阈值
- `[plugin]`，用于 package hook 执行
- `[update]`，用于 release 来源和 channel 设置

准确的键定义见 [Config](../reference/config.md)。

## Update Channels

默认的 update 分区使用：

```toml
[update]
source = "github"
channel = "stable"
```

这会控制 kuku 应当跟随哪个 release manifest。参见 [Update Manifest](../reference/update-manifest.md)。
