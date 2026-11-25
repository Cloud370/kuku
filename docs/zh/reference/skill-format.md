# Skill 格式

## 目录位置与结构

- 用户级：`~/.kuku/skills/<name>/`
- 项目级：`<workspace>/.kuku/skills/<name>/`

```text
<skill>/
├── SKILL.md
├── references/
├── scripts/
├── examples/
└── assets/
```

只有 `SKILL.md` 是必需的。其余目录不会自动预加载，保留在 Skill 目录下供指令或工具按需引用。

## `SKILL.md` 格式

使用 YAML frontmatter 加 Markdown 指令：

```markdown
---
name: tdd
description: Write tests before implementation. Follow red-green-refactor.
---

# TDD

Before writing any code:

1. Write a failing test
2. Write minimal code to pass
3. Refactor
```

## Frontmatter 字段

必填：

| 字段 | 规则 |
|---|---|
| `name` | 1–64 个字符（小写字母、数字、连字符），必须与目录名一致 |
| `description` | 1–1024 个字符，说明用途与适用时机 |

可选：

| 字段 | 类型 | 含义 |
|---|---|---|
| `allowed-tools` | string[] | 解析为建议工具的元数据 |
| `disallowed-tools` | string[] | 解析为排除工具的元数据 |
| `max-turns` | integer | 解析为元数据 |
| `model` | string | 解析为元数据 |
| `metadata` | table | 任意自定义元数据 |

这些元数据用于描述 Skill。Agent 配置、运行时限额和宿主权限策略决定模型选择、执行预算和工具权限。

## 运行时快照

每个新 Turn 开始时，kuku 重新发现 Skill（`plugin.enabled = true` 时包含 Package 里的 Skill）。Session 日志只记指纹；未变更的目录共用一份不可变快照，存于服务数据目录的 `skill-catalogs/`。迁移或备份服务数据时，请将该目录与 Session 日志一并保留。

- `list_skills`、`search_skills` 与 `use_skill` 均读取当前 Turn 的快照，不中途重读磁盘。
- 恢复既有 Turn 时沿用其持久化的快照。
- 磁盘上的修改在下一次新 Turn 生效。

追加到当前 Turn 的输入，在交付前用这个 Turn 的目录校验。如果选择了本轮开始后安装的 Skill，只拒绝这条追加，不中断正在运行的 Turn。可改为在新 Turn 发送。

只有对应版本的正文实际在 Agent 的上下文中时，才算已加载。`use_skill` 遇到已经可见的同一版本，只报告复用，不重复返回正文。如果文件更新或正文已经不在上下文中，Agent 可以重新加载适用的版本。

## 路径解析

加载 Skill 时，运行时会在正文前插入目录标记：

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

可通过目录标记定位用户或项目 Skill 的配套文件。工作区文件工具的相对参数从 Agent 的工作区解析，并遵循正常的路径授权规则。

## Skill 资料

`kuku-docs` 将内置的 `docs/` 树提供在 `references/docs/`。Agent 可直接用只读的 `skill_resources` 工具访问，不要求先加载正文。首次加载或读取资料时，或当前 Turn 因可恢复的权限请求暂停前，kuku 会保留内置资料的版本。暂停的 Turn 在服务升级后仍可访问原来的版本。综合问题可先看 `references/docs/zh/index.md` 或 `references/docs/en/index.md`，具体问题可直接读对应页面。

| 操作 | 参数 | 结果 |
|---|---|---|
| `list` | 可选的目录 `path`、`offset`、`limit` | 递归列出路径，每页最多 20 项 |
| `search` | 不区分大小写的字面 `query`，可选的目录 `path`、`offset`、`limit` | 返回匹配的文件路径与行号，每页最多 20 项 |
| `read` | 文件 `path`，可选的起始行 `offset`（从 1 开始）和 `limit` | 返回有界的 Markdown 源码，每次最多 200 行 |

每次调用都传 `skill_name: "kuku-docs"`。工具遵循正常的读取授权，路径限于其资料目录。结果会给出资料版本和源码修订号。内置页面之间保留相对链接，指向仓库其他文件的链接对应源码提交。

资料在首次访问时解压，完整性校验后才发布。缓存位于服务数据目录下的 `skill-resources/kuku-docs/`，以内容指纹区分版本，可供多个 Session 共用。[使用内置帮助](../guides/use-skills.md#询问-kuku-用法)
