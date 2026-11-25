# 配置 kuku

## 配置文件位置

运行 `kuku config show` 查看选中的配置，凭据会脱敏。运行 `kuku config validate` 查看绝对路径、诊断结果，以及本地服务是否已加载该版本。[CLI 参考](../reference/cli.md#kuku-config)说明路径选择顺序，也可用 `--config PATH` 指定其他文件。

运行 `kuku server status` 可查看本地服务当前生效的路径。要切换受管服务的配置文件，等它空闲后运行 `kuku server restart --config PATH`。[文件布局](../reference/file-layout.md) · [运行服务](../guides/run-the-server.md)

## 交互式配置

在项目目录运行 `kuku`。启动时若需要配置模型，kuku 会自动打开设置页。退出设置后，可按 `Ctrl+L` 或使用 `/model` 重新打开。设置页按 `F1` 可打开与构建版本对应的配置指南。若 kuku 报告配置文件格式错误，请修正对应文件或用 `--config PATH` 选择其他配置。

向导提供两种方式：

- **Edit config file**：直接给出配置文件路径，不发任何模型请求。
- **Find and test models**：拉取模型列表并做简短的文本、思考档位和可选图片试跑（单次上限 4,096 tokens、超时 30 秒，可能消耗 API 余额）。

ChatGPT 设置读取所选账号的模型目录。可以在 **Settings** 里手动发起生成测试。

首次配置时在模型列表里搜索并保存一个精确的模型 ID。试跑结果可在 **Model tests** 里查看，也可以进 **Settings** 调整思考强度、输出上限、能力标记与费率。通过 **Settings → Test tools** 检查工具调用。若某条 Chat 线路需要 Responses 才能调用工具，可在 **Settings → API type → Responses** 切换并重测。

选择自定义连接、已有连接或预设。使用 API Key 时，填写供应商 URL、网络路径和密钥。自定义连接以 URL 的域名和非默认端口命名。同一域名和端口下的连接会用编号区分。已有连接和预设沿用其协议，标准请求 URL 可直接识别。选择模型后，新自定义 Base URL 会并发验证候选协议。优先使用 Responses，确认上游不支持时依次尝试采用 Chat Completions、Messages 的结果。无法确定时会显示 API 类型选项。网络可选择跟随代理环境变量、直连，或使用 HTTP、SOCKS5、SOCKS5H。裸域名有多个标准候选路径时，设置会并发查询，逐条显示进度和结果，找到可用目录就自动打开模型列表。目录无法读取时，可以直接填写已知的模型 ID。

稍后可以打开 `/model`，选择 **Add model or connection** 添加模型。选中已有模型再按 `e` 可修改设置。

## 选择模型

在 `/model` 里选中连接和模型，按 Enter 可切换当前 Agent。要改新 Session 的默认值，选中模型后按 `d`，再确认配置变更。这两项可独立设置。[终端操作](../reference/cli.md#斜杠命令) · [配置参考](../reference/config.md)

保存没有配置思考档位的模型时，若支持 `medium`，设置优先选它。其余情况取已知顺序中间偏低的一档、唯一可用的档位，或在档位顺序未知时采用有效的上游声明默认值。能力信息不足时跟随供应商默认。手动设置优先。

要管理添加过的服务档位，选中路由后按 `e`，再打开 **Settings → Model capabilities → Custom service tiers**。[模型和服务档位快捷键](../reference/cli.md#斜杠命令) · [配置参考](../reference/config.md)

## Provider 与凭据

设置时可选择输入密钥或使用环境变量。填写自己设置的变量名，例如 `MY_GATEWAY_KEY`，配置会写入 `api_key = "$MY_GATEWAY_KEY"`。如果粘贴了密钥，kuku 会将其存入数据目录下的 `credentials/`，以仅所有者可读的明文文件保存，并在 TOML 里记录 `file:` 路径。[环境变量](../reference/environment-variables.md) · [文件布局](../reference/file-layout.md)

手写配置可以从[单模型 TOML 示例](../reference/config.md#最小配置)开始。请在运行服务的主机上设置引用的环境变量。本地服务启动后改了环境变量时，等它空闲后运行 `kuku server restart`。

### 在无头主机登录订阅

在首次配置或 `/model` 中添加连接，选择 **Preset · ChatGPT subscription**。选择网络模式，可选填账号别名。在自己的浏览器打开 **Open authorization link**，或用 **Ctrl+Y** 复制分隔线之间的完整授权地址。授权后，把地址栏里的回调 URL 粘贴到 kuku。如果浏览器无法连接回调页面，从错误页的地址栏复制。回调输入提示会显示光标，粘贴的 URL 不回显。OpenAI 拒绝授权码时，请使用 kuku 显示的新授权链接。登录后账号会保存，设置界面随即查询该账号的模型。

也可以在运行 kuku 的主机上执行 `kuku auth login chatgpt [名称]`。省略名称时，登录会保存 `chatgpt` 或 `chatgpt-2` 这样的短连接名。重新授权时可以使用已保存的连接名或账号别名。

默认跟随环境代理设置，也可以选直连或手动代理。令牌交换、续期、模型发现和推理都沿用所选模式。手动代理失败时，请求停止。手动设置 SOCKS 代理请使用 `socks5h://`，由代理解析供应商地址。浏览器使用自身的网络设置。

添加或编辑 ChatGPT 模型时，用所选账号的访问令牌刷新模型目录。设置界面会显示账号别名，并在可用时显示脱敏邮箱。同一个邮箱有多个工作区时，可以用不同别名区分。OpenAI 的公开登录 API 没有定义 Plus、Pro 等套餐名称字段。

在模型编辑器中，**Settings → Service tier** 沿用 OpenAI 为该账号和模型列出的档位。目录未声明的档位标为待验证。选择 **Settings → Test model**，可以用当前模型、档位和思考设置发起文本试跑，使用你的 ChatGPT 订阅额度。**Model tests** 会显示 OpenAI 为试跑回报的实际档位，或者显示请求失败时返回的错误。

ChatGPT 的方案和应用额度可在[管理用量](https://chatgpt.com/settings/usage)查看。kuku 可以显示 ChatGPT 返回的额度错误，以及已完成请求回报的 token 数。其他订阅渠道提供剩余额度接口时，可以显示该渠道实际回报的数值。

### 认证失败后的恢复

在 TUI 里遇到认证失败时，按 `F2` 选择 **Repair credentials**。如果是受管本地服务，输入新密钥并确认 **Save credentials and restart local service** 即可只更新该 Provider 的 `api_key` 并重启服务（密钥同样存到 `$KUKU_HOME/credentials/`，保存时不做在线校验）。若服务当时正忙于其他任务，可等空闲后重试重启。重启完成后，终端会恢复原来的 Session、Agent 和草稿，你可以主动重试原输入（不会自动重发）。

远程或 token 认证的服务需要在服务端修复凭据并重启，TUI 不会为这类连接写本地密钥文件。

## 常用配置命令

```bash
kuku config show
kuku config validate
kuku config models
kuku config models --discover --connection work
kuku config test --connection work --model MODEL_ID
```

把 `work` 换成已配置的连接名。从脚本创建或修改配置的示例见[运行任务](../guides/run-a-task.md)。

## 其他配置分区

除 Provider 之外的主要分区：

- `[discovery]`：Agent 与 Skill 自动发现
- `[handoff]`：长 Session 摘要阈值
- `[logs]`：可观测性日志保留策略（默认常开，无关闭开关；默认 `max_age_days = 14`、`max_total_size_mb = 512`）
- `[plugin]`：Package 扩展加载
- `[update]`：发布清单来源与更新通道（默认 `source = "github"`、`channel = "stable"`）

具体配置键见[配置参考](../reference/config.md)，更新清单格式见[更新清单](../reference/update-manifest.md)。
