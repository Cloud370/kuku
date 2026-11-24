# 测试指南

在待测 checkout 运行[验收脚本](../../../scripts/acceptance/run.py)。脚本需要 Python 3.10+，会记录实际选择的检查和证据。

## 选择验证范围

| 改动 | 检查 |
| --- | --- |
| 仅文档 | [文档检查](#文档检查) |
| Rust 行为和公开契约 | `python3 scripts/acceptance/run.py contract` |
| Provider 编码、流式处理、回放或设置 | `python3 scripts/acceptance/run.py protocol` |
| 准备集成的 Rust 改动 | `python3 scripts/acceptance/run.py integration` |
| TUI 输入、Session 导航、服务发现、内嵌浏览器或打包 | [安装客户端验收](#安装客户端验收) |
| 验收脚本或夹具工具 | `python3 -m unittest discover -s scripts/acceptance -p 'test_*.py'` |

开发期间先跑能验证改动的最小检查。运行时改动集成前，格式化修改过的 Rust 文件，然后跑一次 `integration`。它依次做只读格式检查、全工作区所有目标的 Clippy 零警告检查和 `contract`。`contract` 启用 `kuku/test_support`，覆盖受该 feature 控制的运行时、客户端和 TUI 测试。`protocol` 使用可控 Provider 夹具，不产生付费调用。

只跑受 feature 控制的 Provider 和查询集成测试时，使用以下命令。

```bash
cargo test -p kuku --features test_support --test provider_integration --test query_runtime
```

合并前，确认待合版本通过就绪 PR 选定的检查。[CI 计划](../../../scripts/acceptance/ci_plan.py) 根据 PR 改动选择原生平台和安装验收场景。[发布流程](release.md)要求对候选版本运行 `platform=all` 完整矩阵。

## 安装客户端验收

可用当前源码构建运行可控场景，也可以用已构建的二进制复跑单个场景。

```bash
python3 scripts/acceptance/run.py interactive
python3 scripts/acceptance/run.py interactive --binary PATH_TO_BINARY --cases statusline
```

这些检查通过 PTY 或 ConPTY 驱动真实客户端与服务，模型请求由本地可控 Provider 接收。原生冒烟、浏览器交接、发行包、安装升级、桌面输入和回放见[安装验收操作](testing-advanced.md#安装验收操作)。

## 测试归属与证据

单元测试靠近源码，集成测试在 `tests/` 下按领域组织。复用现有夹具，在拥有该行为的层验证不变量。例如 Provider 测试验证重试时序，Session 测试验证中断和恢复。

在 `benchmarks/acceptance/<run-id>/result.json` 查看源码版本、feature、实际命令、退出状态和证据路径。检查有界日志、存在时的失败摘要，以及终端场景的 `installed-interactive.log`。采集到的构建耗时在 `target/cargo-timings/`。每个 worktree 使用独立的 Cargo target，引用安装验收前核对二进制版本。

报告实际运行、通过、失败或跳过的内容。分别说明可控场景覆盖、原生平台结果和人工体验。通过持久化记录和夹具断言核对工具效果与清理。对外分享有界证据，保护凭据、私有请求和 Session 内容。

Benchmark、资源测量和[真实 Provider 验收](testing-advanced.md#真实协议验收)需要显式选择。操作和付费前提见[专项验收](testing-advanced.md)。

## 文档检查

回读改动，检查本地链接和锚点，并核对中英文含义一致。运行 `git diff --check`。如果调整入口或导航，同时检查 [README](../../../README.md)、[文档入口](../../index.md)、[英文首页](../../en/index.md)和[中文首页](../index.md)。
