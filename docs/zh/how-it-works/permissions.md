# Permissions

Permissions 是运行时强制执行的机制。

项目指令可以建议行为，但它们本身不会授予或拒绝 Tool 访问权限。

## Decision sources

运行时会按固定顺序评估权限：

1. hard guard
2. project policy deny
3. session grants
4. project policy allow
5. default behavior

hard guard 永远优先。

## What the model can and cannot do

- 模型可以请求一次 Tool 调用。
- 运行时决定这次调用是否被允许。
- 即使被拒绝，该调用仍会成为 Session 历史中的一个事件。

这让权限系统可以被审计，并与 Prompt 表述分离。

## Common modes

不同 Host 暴露权限的方式可以不同，但运行时支持的底层选择相同：

- 单次批准
- Session 级批准
- 项目级批准
- 拒绝

Subagent 的权限请求会回传给父级 run。硬性保护规则在这里同样适用。

## Policy files and runtime facts

项目权限状态由文件承载，并由运行时评估。它不是普通的对话上下文。

## Failure shape

如果 Tool 调用被拒绝，kuku 会记录权限决策并写入一个被阻止的 Tool 结果。如果调用被允许，执行会继续，结果会按正常方式写入。

## Mental model

Permissions 是模型意图和实际副作用之间执行边界的一部分。这也是 kuku 将运行时与 Host UI 和 Prompt 指令分离的主要原因之一。

面向维护者的边界说明见 [Module Contracts](../architecture/module-contracts.md)。
