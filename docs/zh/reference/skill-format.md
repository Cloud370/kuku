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

- 默认 Session 的首次模型上下文列出所有可用 Skill 的完整名称。描述共用约 8,192 tokens 的预算；放得下就保留完整描述，缩短时会标记，不会为了预算省略名称。正文仍按需加载。
- 新 Turn 的首个请求会追加一条相对上次模型可见目录的新增、版本变更和移除通知，不回写已冻结的前缀。同一 Turn 沿用当前目录；只有排序变化时不发通知。
- `list_skills`、`search_skills` 与 `use_skill` 均读取当前 Turn 的快照，不中途重读磁盘。
- 恢复既有 Turn 时沿用其持久化的快照。
- 磁盘上的修改在下一次新 Turn 生效。

追加到当前 Turn 的输入，在交付前用这个 Turn 的目录校验。如果选择了本轮开始后安装的 Skill，只拒绝这条追加，不中断正在运行的 Turn。可改为在新 Turn 发送。

只有对应版本的正文实际在 Agent 的上下文中时，才算已加载。`use_skill` 遇到已经可见的同一版本，只报告复用，不重复返回正文。如果文件更新或正文已经不在上下文中，Agent 可以重新加载适用的版本。

输入显式选择 Skill 时，压缩会保留该版本的完整正文供本轮后续步骤使用，不会自动重载此前 Turn 选中的 Skill。

## 路径解析

加载 Skill 时，运行时会在正文前插入目录标记：

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

可通过目录标记定位用户或项目 Skill 的配套文件。工作区文件工具的相对参数从 Agent 的工作区解析，并遵循正常的路径授权规则。

## Skill 资料

kuku 将内置 Skill 安装为服务数据目录中的真实目录。版本包含 `SKILL.md` 和 `references/docs/` 下的内置文档。正在执行或恢复的 Turn 沿用原目录，新 Turn 可发现新版本。

每个 Turn 首次加载内置 Skill 时都会校验资料文件，并从同一版本的归档恢复缺失或损坏的文件。普通文件工具仍遵循原有路径和授权规则；浏览 Skill 目录不会扫描内置资料。

通过 `use_skill` 取得指引和真实目录；用 `read_file` 读取资料，或用 `find_files` 和 `search_text` 找文件。已知绝对路径时也可以直接读。相对脚本和资源路径以 Skill 目录为基准，命令仍默认在 Agent 工作区执行。内置文件对文件编辑工具只读。
