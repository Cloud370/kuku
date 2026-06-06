# Skill Format

## 位置

- 用户级：`~/.kuku/skills/<name>/`
- 项目级：`<workspace>/.kuku/skills/<name>/`

## 必需布局

```text
<skill>/
├── SKILL.md
├── references/
├── scripts/
├── examples/
└── assets/
```

只有 `SKILL.md` 是必需的。

## `SKILL.md` 形状

`SKILL.md` 使用 YAML frontmatter，后面跟 Markdown 指令。

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

| Field | Rule |
|---|---|
| `name` | 1-64 个字符，小写字母、数字、连字符；必须与目录名匹配 |
| `description` | 1-1024 个字符；说明 Skill 做什么以及何时使用 |

可选：

| Field | Type | Meaning |
|---|---|---|
| `allowed-tools` | string[] | 仅为遗留元数据；不是运行时强制策略 |
| `disallowed-tools` | string[] | 仅为遗留元数据；不是运行时强制策略 |
| `max-turns` | integer | 激活时的轮次上限 |
| `model` | string | tier 覆盖 |
| `metadata` | table | 任意元数据 |

Skill 会指导模型行为，但不会改变权限。Tool 访问和权限提示仍然由当前 Session 的运行时策略决定。

## 加载模型

Skill 分三阶段加载：

1. Session 启动时加载 metadata
2. 使用 Skill 时加载完整 `SKILL.md`
3. 按需加载 `references/`、`scripts/`、`examples/` 和 `assets/`

## Path Resolution

当 Skill 被加载时，运行时会在前面加入类似下面的注释：

```markdown
<!-- loaded: /home/user/.kuku/skills/tdd -->
```

Skill 指令中使用的相对路径会相对于这个 Skill 目录解析。
