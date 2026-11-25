# 运行任务

## 从 shell 配置

[安装 kuku](../start/install.md)，在项目目录打开 shell。如果 TUI 里的模型已可用，沿用同一数据目录，然后直接[提交任务](#提交任务)。

要新建一套脚本配置，先在 `MY_GATEWAY_KEY` 中设置密钥，再运行以下命令。将示例中的基础 URL 和模型 ID 换成实际的值。带引号的 heredoc 会在配置中保留变量引用。

```bash
export KUKU_HOME="$HOME/.kuku-script"

kuku config set --patch "$(cat <<'TOML'
default_model = { connection = "work", model = "your-model-id" }

[provider.work]
format = "openai-responses"
base_url = "https://gateway.example/v1"
api_key = "$MY_GATEWAY_KEY"
TOML
)"
kuku config validate
```

发出第一个任务前，先导出密钥，使受管本地服务能读取它。新数据目录会报告 `status: "valid"` 和 `service.state: "stopped"`，第一次运行任务会启动服务。关于所选路径、运行中的服务和模型发现，见 [CLI 配置命令](../reference/cli.md#kuku-config)。

## 提交任务

将提示词存入 `task.txt`，在当前目录运行。

```bash
cat > task.txt <<'EOF'
检查项目文件。
告诉我怎样运行测试。
EOF
kuku run --json --prompt-file task.txt > result.json
jq -r '.response' result.json
session_id="$(jq -er '.session_id' result.json)"
```

要通过管道输入，并实时取得逐行 JSON，可运行 `cat task.txt | kuku run --stream-json --prompt-file -`。短任务可以把提示词作为位置参数，例如 `kuku run "检查这个项目"`。用 `--workspace PATH` 指定其他目录，或用 `--connection work --model your-model-id` 选择精确模型。[CLI 参数与退出码](../reference/cli.md#kuku-run)

## 处理结果

在脚本中检查 `kuku run` 的退出码。运行完成后可读取最终 `response`。遇到 `waiting`（退出码 3），先从 `question.id` 或 `permission.id` 读取请求 ID，再在同一工作区和数据目录运行 `kuku reply`。

```bash
request_id="$(jq -er '.question.id' result.json)"
kuku reply "$session_id" "$request_id" --answer "使用项目的测试配置"
```

如果等待授权，从 `.permission.id` 取 ID，并选择 `--allow` 或 `--deny`。若结果中的 Agent 不是 `main`，请传入 `--agent`。遇到 `failed`（退出码 1），读取结果的 `error` 和 stderr，再[排查 CLI 运行](debug-cli.md)。按 `Ctrl+C` 请求取消当前运行，并等待它启动的命令清理完毕。

## 继续已有任务

```bash
kuku run --session "$session_id" --json "总结刚才的发现"
kuku show "$session_id"
```

继续使用同一个工作区和数据目录。用 `kuku list` 查找 Session，也可用 `kuku events "$session_id"` 读取持久事件。在 TUI 里通过 `/resume` 找回相同的 Session。[管理 Session](manage-sessions.md)

## 在 TUI 里运行

运行 `kuku`，按 Enter 提交任务。Agent 运行时，Enter 在下一个安全边界发送补充输入，Alt+Enter 则将它排到下一轮。Shift+Enter 或 Ctrl+J 插入换行，Esc 打断执行并保留草稿。用 `/new` 新开 Session，或通过 `/agents` 选择接收者。[终端界面](../reference/cli.md#终端界面)
