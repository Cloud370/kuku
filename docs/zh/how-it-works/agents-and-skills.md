# Agents and Skills

Agents 和 Skills 都能扩展运行时，但它们工作的层级不同。

## Canonical Mental Model

- skill 是加载到当前 conversation 中的指令。
- agent 是从 agent 文件发现的联系人卡片。
- 调用 agent 会在同一个 Session 账本里打开或继续某个 conversation address。
- address 是连续性键。

## The Difference

| | Skill | Agent |
|---|---|---|
| Execution model | 把指令注入当前 conversation | 打开或继续一个委派 conversation |
| State model | 共享当前 conversation | 在同一个 Session 账本中拥有自己的 conversation 线程 |
| Tools | 使用当前 Tool surface | 使用绑定到该 agent 的 Tool surface |
| Continuity | 持续存在于当前 conversation address | 复用同一个 address 时保持连续性 |
| Best for | 工作流指导或打包知识 | 需要隔离上下文的委派工作 |

## Skills

Skill 是从 skill 目录发现的 Markdown 能力。模型在当前 conversation 中需要工作流指导、参考材料或打包行为时，会加载 skill。

Skill 不会创建第二个 conversation。它只是通过增加指令和可选资源来改变当前 conversation。

默认 skill Tool surface 是 `list_skills`、`search_skills` 和 `use_skill`。

`context.skills` 会记录该 conversation turn 的 skill registry 快照和 bootstrap-loaded skill 名称。runtime notice 也可以展示当前已加载的 skills。

## Agents

Agent 是从用户或项目目录发现的联系人卡片。`agent` Tool 通过把消息发送到某个 conversation address 来委派工作，而这个 address 会绑定到一个 agent 身份。

这个被委派的 conversation 具备：

- 自己在共享账本中的 replay 流
- 自己的终止事件（`turn.completed`、`turn.cancelled`、`turn.interrupted`）
- 权限请求会回传到父运行
- 继续委派时有深度预算
- 复用相同 address 时保留连续性

`main` conversation 是 Host 线程的保留地址，不能作为 `agent` Tool 的目标。

嵌套委派只允许 `parent -> child -> grandchild`。如果某个委派 conversation 再继续生成更深层 agent，运行时会用 `blocked: maximum agent delegation depth (2) reached` 阻止它。

## Address and Binding Rules

conversation address 使用小写、斜杠分隔的名字，例如：

- `review`
- `review/api`
- `explore/auth-flow`

规则：

- `main` 是保留名
- `main/...` 非法
- 根 segment 必须命中已发现的 agent contact card
- 复用已有 address 表示继续该 conversation
- 新 address 会写入新的 `conversation.opened`
- address 首次绑定时会写入 `conversation.bound`

如果复用已有 address，绑定身份仍必须匹配。如果 agent 定义现在哈希成了不同的 binding identity，运行时会拒绝这次调用。

## Permissions and Notices

委派 conversation 的权限请求会通过父运行回传决策。Skill 也同样不能绕过权限。

runtime notice 可以提示：

- 已打开的委派 conversations
- 当前 conversation 的收件箱消息
- 已加载 skills
- 待决权限
- 已中断的 turns
- 上下文漂移

权限模型见 [Permissions](permissions.md)。

## Where Each Fact Belongs

- 本页解释运行时关系。
- 使用流程属于 `guides/`。
- 文件格式属于 `reference/`。
- package 与 loader 内部机制属于 [Extension Runtime](../architecture/extension-runtime.md)。
