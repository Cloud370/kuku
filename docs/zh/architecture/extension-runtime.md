# Extension Runtime

扩展通过 package、hook、Skill 以及未来的外部 Tool 来源，挂接在核心运行时周围。

## Package model

package 会从用户和项目的 package 目录中发现，这些目录位于 `.kuku/packages/` 和 `~/.kuku/packages/` 下。

一个 package 可以打包：

- hook 可执行文件
- Skills
- 面向未来外部 Tool 集成的 MCP 配置

package manifest 是 hook 注册的事实来源。

## Hook model

hook 作为外部进程运行。kuku 传入最小化的结构化输入，读取 stdout，并解释退出码。

这让扩展边界保持：

- 与语言无关
- 可审计
- 与 SDK 进程隔离

## Runtime integration points

当前已实现的生命周期 hook 覆盖这些位置：

- `session.start`
- `session.end`
- `tool.pre_execute`
- `tool.post_execute`
- `model.pre_request`
- `model.post_response`

hook 可以在特定位置添加上下文、修改部分输入或输出，或阻止某些操作。它们不会替代核心 Session 模型。

## Permission boundary

扩展不会绕过权限门。package 可以影响 loop 周围的行为，但硬性防护和运行时权限检查仍然生效。

## Skills inside packages

启用 package 加载时，package 内打包的 Skill 会由与独立 Skill 相同的 skill registry 发现。运行时模型保持不变；package 化只改变分发方式，以及它与 hook 的共址关系。关闭 `plugin.enabled` 后，运行时会同时移除 package 提供的 hook 和 package 提供的 Skill。

## Forward path

下一个主要扩展边界是外部 Tool 来源，例如基于 MCP 的 Tool provider。这项工作建立在同样的运行时假设之上：稳定的 Tool schema、运行时权限，以及以文件为后盾的 Session 真相。

host 边界见 [Host Apps](host-apps.md)，当前实现顺序见 [Evolution](evolution.md)。
