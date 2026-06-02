# Environment Variables

## 运行时变量

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | 覆盖默认运行时 home 目录 |
| `ANTHROPIC_API_KEY` | `provider.anthropic` 常用的 API key 来源 |
| `OPENAI_API_KEY` | `provider.openai` 常用的 API key 来源 |

`config.toml` 也可以在 provider 的 `api_key` 字段中使用 `$NAME` 来引用任意环境变量。

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
