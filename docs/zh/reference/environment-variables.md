# 环境变量

## 运行时变量

| 变量 | 含义 |
|---|---|
| `KUKU_HOME` | 覆盖默认数据目录（默认 `~/.kuku`） |
| `KUKU_TUI_STATUSLINE` | 本次 TUI 启动的状态栏字段列表，逗号分隔（`none` 隐藏）。[终端偏好](config.md#终端偏好) |
| `KUKU_TOKEN` | 连接显式 `--remote` 服务时使用的 Bearer Token |
| `KUKU_SERVER_PASSWORD` | 通过 `kuku server` 启动独立服务时设置的访问令牌 |
| `ANTHROPIC_API_KEY` | `provider.anthropic` 默认使用的密钥变量 |
| `OPENAI_API_KEY` | `provider.openai` 默认使用的密钥变量 |
| `DEEPSEEK_API_KEY` | `provider.deepseek` 默认使用的密钥变量 |
| `OPENROUTER_API_KEY` | `provider.openrouter` 默认使用的密钥变量 |
| `KUKU_PROVIDER_TRACE` | `1` 开启完整请求 trace；`0` 关闭 |

`config.toml` 中任何以 `$` 开头的字符串字段都会在加载时展开对应环境变量；`api_key` 例外，会保留 `$NAME` 引用并在调用 Provider 时解析。`$NAME` 和 `file:<绝对路径>` 互相独立，不会自动回退。

模型凭据和 `KUKU_PROVIDER_TRACE` 由**服务端**进程读取（环境变量密钥不能为空）；新客户端连入已运行的服务时沿用服务端自身的环境。[配置 kuku](../start/configuration.md)

## Provider Trace 日志

预发布版和未打标签的开发版默认记录轻量级协议诊断，稳定版默认关闭。诊断 JSONL 记录 Session/Turn/请求关联、模型与线路、HTTP 状态、SSE 序号与结束标记、失败类别以及脱敏后的思考条目元数据，不包含请求/响应正文、请求头或不透明载荷。

设置 `KUKU_PROVIDER_TRACE=1` 会记录完整请求正文、请求头和原始 SSE 帧。含密钥的请求头会自动脱敏，但提示词、工具结果和回复仍在正文里。嵌入 SDK 时可通过 `SessionOptions::provider_trace` 或 `Query::provider_trace` 显式指定策略。

Trace 以轮转 JSONL 分段存放在 `$KUKU_HOME/logs/provider-trace/`，遵循[日志保留规则](config.md#logs)，同时受写入上限约束：总大小不超过 `min(64 MiB, 日志总预算)`，单段不超过 `min(8 MiB, trace 预算)`，超出时优先淘汰旧分段。

## Hook 进程变量

kuku 执行 Package Hook 时会注入以下变量：

| 变量 | 含义 |
|---|---|
| `KUKU_SESSION_DIR` | 当前 Session 目录的绝对路径 |
| `KUKU_WORKSPACE` | 当前工作区的绝对路径 |
| `KUKU_PACKAGE_DIR` | Package 根目录的绝对路径 |

Hook 进程同时继承 `PATH`、`HOME`、`USERPROFILE`（Windows）、`LANG` 和 `LC_ALL`；其他环境变量和密钥不会自动透传。
