# Environment Variables

## 运行时变量

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | 覆盖默认运行时 home 目录 |
| `KUKU_TUI_STATUSLINE` | 用逗号分隔的字段列表覆盖本次终端的已保存状态栏布局。[终端偏好](config.md#终端偏好) |
| `KUKU_TOKEN` | 用于显式连接服务的客户端令牌 |
| `KUKU_SERVER_PASSWORD` | 通过 `kuku server` 显式启动服务时使用的令牌 |
| `ANTHROPIC_API_KEY` | `provider.anthropic` 常用的 API key 来源 |
| `OPENAI_API_KEY` | `provider.openai` 常用的 API key 来源 |
| `DEEPSEEK_API_KEY` | `provider.deepseek` 常用的 API key 来源 |
| `OPENROUTER_API_KEY` | `provider.openrouter` 常用的 API key 来源 |
| `KUKU_PROVIDER_TRACE` | `1` 启用完整 trace，`0` 关闭 provider trace |

`config.toml` 也可以在字符串字段中使用 `$NAME` 引用环境变量。对于 `api_key`，`$NAME` 明确选择环境变量来源；`file:<绝对路径>` 明确选择私有文件。两种来源不会互相回退；其他字符串字段在配置加载时解析。

环境变量凭据必须非空。模型凭据和 `KUKU_PROVIDER_TRACE` 在服务端环境中设置。新客户端接入时，已运行的服务继续使用自己的环境变量。[配置指南](../start/configuration.md)

## Provider Trace 日志

预发布版和未打标签的开发版默认记录协议诊断，稳定版默认关闭。诊断 JSONL 记录包含 Session、轮次和请求关联信息、模型和线路、HTTP 状态、SSE 顺序和结束标记，以及失败类别。思考条目记录类型、格式、索引和请求内的 ID 别名，未知类型和格式也用请求内别名表示。诊断模式省略请求和响应正文、响应内容、头信息及不透明的思考数据。

`KUKU_PROVIDER_TRACE=1` 记录完整的请求正文、头信息和原始 SSE。凭据头的值会脱敏，完整 trace 仍可能包含提示、工具结果和模型输出。嵌入式宿主可通过 `SessionOptions::provider_trace` 或 `Query::provider_trace` 显式选择模式。

Provider trace 以轮转的 JSONL 分段保存在 `$KUKU_HOME/logs/provider-trace/` 下。它们遵循[日志保留规则](config.md#logs)，写入时还会遵守 64 MiB 或配置的日志总预算中较小的上限。每段上限为 8 MiB 或 trace 预算中较小的值，较旧的分段优先清除。

## 未设置 `KUKU_HOME` 时的默认行为

如果 `KUKU_HOME` 未设置，kuku 使用：

```text
~/.kuku
```

## Hook 进程变量

当 kuku 运行 package hook 时，会为 hook 进程设置这些变量：

| Variable | Meaning |
|---|---|
| `KUKU_SESSION_DIR` | 当前 Session 目录的绝对路径 |
| `KUKU_WORKSPACE` | 当前 workspace 的绝对路径 |
| `KUKU_PACKAGE_DIR` | package 根目录的绝对路径 |

hook 进程还会继承 `PATH`、`HOME`、Windows 下的 `USERPROFILE`、`LANG` 和 `LC_ALL`。

其他 secret 不会自动透传。
