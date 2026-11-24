# 升级 kuku

## 更新二进制

源码安装用户直接重装当前版本：

```bash
cargo install --locked --git https://github.com/Cloud370/kuku --force kuku-app
```

发行版用户用对应平台的安装脚本指向新版本的 `latest.json` 即可。[安装已发布版本](../start/install.md#安装已发布版本)

## 受管服务切换

照常启动新二进制即可。按 `KUKU_HOME` 隔离：

- 同构建版本：直接复用运行中的受管服务。
- 更高版本（或同版本的不同构建）：等服务空闲后自动替换。
- 旧版客户端遇到新版服务：提示当前正在运行的较新版本，不会降级。

需要多版本并行运行时，各自指定不同的 `KUKU_HOME`。

升级时如果后台还有任务，新开的 TUI 会等待任务完成后自动接入；已经打开的旧窗口若因版本切换断开，会提示当前运行版本，重连时仍沿用该窗口自己的程序。随时可用 `kuku server status` 查看运行中的版本和活跃任务。正常替换会完整保留已提交历史、待决请求和排队输入。[运行服务](run-the-server.md) · [Session 模型](../how-it-works/sessions.md)

## 核对配置

```bash
kuku --help
kuku config validate
```

用 `kuku config show` 核对默认连接与精确模型 ID，再通过 `kuku config validate` 确认路径和加载状态。较早 alpha 版本使用的命名默认模型需要改为当前的连接与模型字段。[最小配置](../reference/config.md#最小配置)

## 核对帮助版本

运行 `kuku server status` 查看运行中的服务版本。新 Turn 使用该服务附带的 `kuku-docs` 页面。恢复已加载 Skill 的 Turn 时会沿用其中记录的资料版本，回退到旧构建时也是如此。[Skill 版本](../reference/skill-format.md#运行时快照)
