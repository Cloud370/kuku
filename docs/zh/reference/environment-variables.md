# Environment Variables

## 运行时变量

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | 覆盖默认运行时 home 目录 |
| `ANTHROPIC_API_KEY` | `provider.anthropic` 常用的 API key 来源 |
| `OPENAI_API_KEY` | `provider.openai` 常用的 API key 来源 |
| `KUKU_PROVIDER_TRACE` | 设为 `1` 时写入 provider 请求和响应 trace 日志 |

`config.toml` 也可以在字符串字段中使用 `$NAME` 引用环境变量。`api_key` 保留环境变量引用并在之后解析；其他字符串字段在配置加载时解析。

## Provider Trace 日志

`KUKU_PROVIDER_TRACE=1` 会为真实 API 请求开启 provider 诊断日志。开启后，kuku 会把 JSONL trace 文件写入：

```text
$KUKU_HOME/logs/provider-trace/<yyyy-mm-dd>/<session-id>.jsonl
```

Provider trace 记录包含请求 header、请求 body、响应 header 和流式响应事件。`authorization`、`x-api-key`、`api-key`、cookie 和代理凭证等 secret header value 会在写入前打码。请求和响应 body 仍可能包含 prompt 文本、tool result、模型输出或其他任务数据，所以只应在调试时开启。

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
