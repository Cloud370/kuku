# Web UI 开发

当修改 `apps/web/**`、内嵌 Web 资源或 Web API 契约时，使用本页。

## 从这里开始

1. 阅读 [Development](development.md) 与 [Testing](testing.md)。
2. 安装 `apps/web/.nvmrc` 记录的 Node 版本。
3. 在 `apps/web` 中运行 `npm ci`。
4. 使用 `npm run dev` 启动 UI。它连接到 `127.0.0.1:17777` 上的 `kuku-server`。

简明命令表也在 [apps/web/README.md](../../../apps/web/README.md)。

## 本地工作流

除非另有说明，以下命令都在 `apps/web` 中运行。

```bash
npm ci
npm run typecheck
npm run lint
npm run test
npm run build
```

使用 `npm run test:storybook` 验证组件故事。使用 `npm run test:e2e` 运行会启动内嵌应用二进制的浏览器流程。CI 使用单 worker 运行这些流程，因为每个测试都会启动真实服务进程；本地资源充足时可以自行提高并发。

当改动影响 API 契约时，先重新生成再测试：

```bash
npm run generate:api
git diff -- src/api/generated
```

不要直接编辑 `src/api/generated/**`。

## 内嵌应用

生产环境的 `kuku web` 服务由 `kuku-app` 提供内嵌资源。构建二进制前先构建资源：

```bash
cd apps/web
npm run build
cd ../..
cargo build -p kuku-app --features embedded-web-assets
```

## 浏览器诊断产物

浏览器 E2E 覆盖行为、可访问性和响应式边界，但不提交图像基线。不要提交 `apps/web/test-results/`、Playwright HTML 报告、trace、截图或视频。这些都是失败时生成的诊断产物，已被 Git 忽略。

## CI 职责

| 工作流 | 触发条件 | 职责 |
| --- | --- | --- |
| `CI` | `main` 上非纯 Web UI 的改动与 pull request | Rust 格式、clippy 与跨平台测试 |
| `WebUI CI` | 相关 Web、内嵌应用、契约、server 或工作流改动 | 契约生成、Web 静态检查与内嵌 Chromium 流程 |
| `Release` | 版本 tag | 构建、smoke、打包、校验和发布产物 |

`WebUI CI` 仅在任务失败时上传浏览器报告。它是自动 Web 门禁；alpha 阶段的日常改动不应再增加手工审批工作流。

## Agent 检查清单

1. 同步生成的 API 文件，不要手工编辑。
2. 为行为改动添加或更新聚焦的单元测试。
3. 运行相关静态检查与浏览器测试。
4. 不要把生成的浏览器诊断产物提交到仓库。
