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
| `allowed-tools` | string[] | 仅保留的旧字段，运行时不作为权限策略执行 |
| `disallowed-tools` | string[] | 仅保留的旧字段，运行时不作为权限策略执行 |
| `max-turns` | integer | 激活时的轮次上限 |
| `model` | string | 模型档位覆盖 |
| `metadata` | table | 任意自定义元数据 |

Skill 只指导模型行为，不会改变运行时的工具权限。

## 运行时快照

每个 Turn 开始时，kuku 将当前发现的 Skill 目录快照写入 Session 日志（`plugin.enabled = true` 时包含 Package 里的 Skill）：

- `list_skills`、`search_skills` 与 `use_skill` 均读取当前 Turn 的快照，不中途重读磁盘。
- 恢复既有 Turn 时沿用其持久化的快照。
- 磁盘上的修改在下一次新 Turn 生效。

## 路径解析

加载 Skill 时，运行时会在正文前插入目录标记：

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

Skill 指令里的相对路径均相对于该目录解析。
