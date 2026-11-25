# 环境变量

## 运行时变量

| 变量 | 含义 |
|---|---|
| `KUKU_HOME` | 覆盖默认数据目录（默认 `~/.kuku`） |
| `KUKU_TUI_STATUSLINE` | 本次 TUI 启动的状态栏字段列表，逗号分隔（`none` 隐藏）。[终端偏好](config.md#终端偏好) |
| `KUKU_TOKEN` | 连接显式 `--remote` 服务时使用的 Bearer Token |
| `KUKU_SERVER_PASSWORD` | 通过 `kuku server` 启动独立服务时设置的访问令牌 |
| `KUKU_PROVIDER_TRACE` | `1` 开启完整请求 trace；`0` 关闭 |

## 配置中的引用

变量名可以自己选。`config.toml` 中完整的 `$NAME` 值引用对应的环境变量。在 `[provider.work]` 中写入 `base_url = "$MY_GATEWAY_URL"`，在 `default_model` 中写入 `model = "$MY_MODEL_ID"`。要为这个 ID 设置选项，可用带引号的模型键 `[model.work."$MY_MODEL_ID"]`。

kuku 在加载配置时展开 URL 和模型 ID 引用。API 密钥与自定义请求头保留 `$NAME`，到准备或发出请求时再解析。自定义请求体中的美元符号按字面值保留。API 密钥也可使用 `file:<绝对路径>`。[配置参考](config.md)

请在加载配置的进程环境中设置引用的变量。CLI 或 TUI 发出的模型请求使用**服务端**环境。本地 `kuku config` 命令读取该命令进程的环境。若设置变量时服务已经启动，请等它空闲后重启。[配置指南](../start/configuration.md)

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
