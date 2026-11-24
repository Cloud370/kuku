# 配置 kuku

## 配置文件位置

kuku 默认读取 `~/.kuku/config.toml`；如果设置了 `KUKU_HOME`，则读取 `$KUKU_HOME/config.toml`。

用 `--config PATH` 可以临时指定配置文件，运行 `kuku server status` 可查看本地服务当前生效的路径。如果受管服务已经在跑，等它空闲后运行 `kuku server restart --config PATH` 即可切换配置。[文件布局](../reference/file-layout.md) · [运行服务](../guides/run-the-server.md)

## 交互式配置

当配置缺失、为空，或者默认模型的凭据不可用时，运行 `kuku` 会在欢迎页引导你通过 `/model` 配好第一条线路；也可以直接在终端运行 `kuku init` 进入同一向导。向导不会覆盖已有配置，遇到格式错误的配置文件只会报错而保留原文件。

向导提供两种方式：

- **Edit config file**：直接给出配置文件路径，不发任何模型请求。
- **Find and test models**：拉取模型列表并做简短的文本、思考档位和可选图片试跑（单次上限 4,096 tokens、超时 30 秒，可能消耗 API 余额）。

你可以选预设、复用已有连接，或填写自定义 URL、代理方式（跟随环境代理、直连、HTTP、SOCKS5、SOCKS5H）和 API Key。对于自定义 Base URL，选好模型后向导会自动探测协议（优先 `Responses`，其次 `Chat Completions`、`Messages`），结果不明确时再让你手动挑；如果目录不可读，也可以直接手填模型 ID。

选择一次 **Find and test models**，即可允许本次设置查询模型目录并发送简短的文本或图片试跑。API Key 试跑可能消耗 API 余额，请求输出上限为 4096 token。ChatGPT 试跑使用你的订阅方案。每次试跑的超时为 30 秒。选择 **Edit config file** 会显示配置路径，供应商请求数为零。

在模型列表里可以直接打字搜索、按 `Space` 多选、`Enter` 确认。试跑结果可以在 **Model tests** 里逐项查看，也可以进 **Settings** 调整思考档位、输出上限、能力标记与费率。想确认工具调用是否正常，点 **Settings → Test tools** 测一次合成工具调用——能正常回文本不代表支持工具调用；如果某条 Chat 线路需要走 Responses 才能调用工具，可在 **Settings → API type → Responses** 切换并重测再保存。

选择自定义连接、已有连接或预设。使用 API Key 时，填写供应商 URL、网络路径和密钥。自定义连接以 URL 的域名和非默认端口命名。同一域名和端口下的连接会用编号区分。已有连接和预设沿用其协议，标准请求 URL 可直接识别。选择模型后，新自定义 Base URL 会并发验证候选协议。优先使用 Responses，确认上游不支持时依次尝试采用 Chat Completions、Messages 的结果。无法确定时会显示 API 类型选项。网络可选择跟随代理环境变量、直连，或使用 HTTP、SOCKS5、SOCKS5H。裸域名有多个标准候选路径时，设置会并发查询，逐条显示进度和结果，找到可用目录就自动打开模型列表。目录无法读取时，可以直接填写已知的模型 ID。

在 Session 里随时可以输入 `/model` 或按 `Ctrl+L` 添加/修改连接，选择 **Use in this Session** 切换当前 Agent 的线路，或 **Set as default** 存为新 Session 的默认线路。[终端控制](../reference/cli.md#斜杠命令) · [配置参考](../reference/config.md)

## Provider 与凭据

默认配置包含两个 Provider：

- `provider.anthropic`：`api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai`：`api_key = "$OPENAI_API_KEY"`

密钥推荐使用环境变量引用（`api_key = "$VARIABLE_NAME"`）或独立密钥文件（`api_key = "file:<绝对路径>"`）。在交互向导里粘贴的密钥会以仅所有者可读的明文文件存到 `$KUKU_HOME/credentials/`，TOML 里只写 `file:` 路径；文件未加密。[配置参考](../reference/config.md) · [文件布局](../reference/file-layout.md)

环境变量由服务端进程读取，而不是终端客户端。如果本地服务启动后你才在终端里改了环境变量，等服务空闲后运行 `kuku server restart` 才会生效；连远程服务时请在服务端机器上配置密钥。`config.toml` 里其他字符串字段同样支持在加载时展开 `$ENV_VAR_NAME`（例如 `base_url = "$KUKU_ANTHROPIC_BASE_URL"`）。

### 在无头主机登录订阅

在 `kuku init` 或 `/model` 的 **Add model** 中选择 **Preset · ChatGPT subscription**。填入代理，可选填账号别名。用自己的浏览器打开完整授权链接，再把地址栏里的回调 URL 粘贴到 kuku。如果浏览器无法连接回调页面，从错误页的地址栏复制。粘贴的 URL 不回显。登录后账号会保存，设置界面随即查询该账号的模型。

也可以在运行 kuku 的主机上执行 `kuku auth login chatgpt [名称] --proxy URL`。重新授权时使用已保存的账号别名。省略名称时，从授权身份派生别名。

登录前设置该账号的代理。kuku 发出的订阅请求都使用该连接的代理，包括令牌交换、续期、模型发现和推理。代理缺失或失败时，请求停止。之后可以更新连接的代理。浏览器使用自身的网络设置。

添加或编辑 ChatGPT 模型时，用所选账号的访问令牌刷新模型目录。设置界面会显示账号别名，并在可用时显示脱敏邮箱。同一个邮箱有多个工作区时，可以用不同别名区分。OpenAI 的公开登录 API 没有定义 Plus、Pro 等套餐名称字段。

在模型编辑器中，通过 **Settings → Service tier** 请求 Fast 或 Ultrafast。档位是否可用取决于账号和模型，因此选择页会将两档标为待验证。**Model tests** 会显示 OpenAI 为试跑回报的实际档位，或者显示请求失败时返回的错误。

ChatGPT 的方案和应用额度可在[管理用量](https://chatgpt.com/settings/usage)查看。kuku 可以显示 ChatGPT 返回的额度错误，以及已完成请求回报的 token 数。其他订阅渠道提供剩余额度接口时，可以显示该渠道实际回报的数值。

### 认证失败后的恢复

在 TUI 里遇到认证失败时，按 `F2` 选择 **Repair credentials**。如果是受管本地服务，输入新密钥并确认 **Save credentials and restart local service** 即可只更新该 Provider 的 `api_key` 并重启服务（密钥同样存到 `$KUKU_HOME/credentials/`，保存时不做在线校验）。若服务当时正忙于其他任务，可等空闲后重试重启。重启完成后，终端会恢复原来的 Session、Agent 和草稿，你可以主动重试原输入（不会自动重发）。

远程或 token 认证的服务需要在服务端修复凭据并重启，TUI 不会为这类连接写本地密钥文件。

## 模型档（Tier）

命名 Tier 把连接、模型、思考档位和 token 限额打包在一起（例如 `strong`、`balanced`、`light`）。`default_model = "balanced"` 指定默认档；运行时可以用 `--tier <name>` 切档，或用 `--connection <name> --model <model-id>` 精确指定线路。

## 常用配置命令

```bash
kuku config show
kuku config validate
kuku config set model.balanced.think high
```

## 其他配置分区

除 Provider 之外的主要分区：

- `[discovery]`：Agent 与 Skill 自动发现
- `[handoff]`：长 Session 摘要阈值
- `[logs]`：可观测性日志保留策略（默认常开，无关闭开关；默认 `max_age_days = 14`、`max_total_size_mb = 512`）
- `[plugin]`：Package 扩展加载
- `[update]`：发布清单来源与更新通道（默认 `source = "github"`、`channel = "stable"`）

具体配置键见[配置参考](../reference/config.md)，更新清单格式见[更新清单](../reference/update-manifest.md)。
