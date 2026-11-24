# Prompt 装配

公开 Session SDK 为一个 Agent 准备一个模型 Step。输入包括稳定的系统提示、前置消息、该 Agent 已接纳的消息和可用工具。provider 适配器把这份规范请求转换成所选线路协议。

稳定的行为指引保持简短，适用于能力各异的模型。结合上下文理解最新任务，再按要求交付的成果分别决定是否回复、写入工作区或执行命令。用通用职责表达规则，用同一意图的不同措辞和任务核验其适用范围。

根据用户的目标判断继续工作能否改变结果。用成本最低且可靠的检查解决对所需结果的疑点，覆盖明确的交付条件。交付形式由用户的请求决定。

## 创建上下文段

首次准备时，SDK 加载 Prompt 目录、Agent 指令、工作区指令文件、全局与项目 Memory，以及工具定义。使用选定的资源渲染系统提示和前置消息。内置项目策略不包含当前日期；冻结的 Session 准备会在首次使用以及日期变化时将日期作为持久化运行时消息追加。静态 Agent 和 Skill 目录不包含变化中的对话数量、已加载或变更状态；每轮通知仍保留在后缀中。日志在 `runtime.context_segment` 中记录 Prompt 目录、渲染结果和工具 schema。为不同工作目录准备上下文段时，会重新读取对应目录。[Step 准备与上下文段创建](../../../crates/kuku/src/query/frozen.rs) · [Prompt 资源](../../../crates/kuku/prompts)

所选工作区根目录下可读取且含有非空白内容的 `AGENTS.override.md` 是该目录唯一的项目指令来源，即使 Git 忽略了它也会加载。否则 kuku 按顺序加载可读取的 `AGENTS.md`、`CLAUDE.md`。查找仅限所选根目录。更改在重新组装上下文时生效，恢复 Session 时沿用已提交的前置消息。

执行环境包含工作区根目录和实际执行工具的进程平台。平台包含操作系统与架构，Linux 上还会带上系统提供的发行版 ID 和版本。

## 准备下一个 Step

新渲染的冻结前置消息先放入按需选择的工具指引（包括委派和 Memory 工具指引），然后依次放入 Agent 行为、项目策略（工作区指令、工作区与平台、模型层级）以及全局和项目 Memory。工具指引仍取决于该 Agent 的可用工具。行为位于工作区策略之前，因为使用相同 Agent 定义的不同工作区可以共享行为；行为不同时，前缀仍然不同。恢复时保留已记录上下文段、快照和已接纳消息的原始顺序，包括项目策略在前的旧上下文段。复用已记录前置消息的后继上下文段也保留原有顺序和身份槽位；只有重新渲染前置消息时才使用新顺序。

1. 活跃游标读取该 Agent 新提交的事件，保留先前已接纳的前缀。
2. 新选入的用户输入、模型消息、工具结果视图和运行时消息记入 `runtime.context_admitted`。大工具输出通过有界视图进入模型，调用者仍可读取保留下来的完整范围。
3. 适配器编码请求。派发之前，`runtime.step_prepared` 记录上下文段、输入边界、源事件游标、有效设置和请求正文哈希。`prefix_hash` 对固定上下文段边界（系统指令、工具和前置消息）计算哈希；上下文段不变时该值保持稳定。它是规范内容诊断，不保证缓存命中。`prefix_context_head` 与 `history_prefix_hash` 标识并哈希新后缀之前已确认的历史；新上下文段的边界为零，确认历史增长时，该哈希也会变化。匹配的准备记录确认新增消息。重开 Session 后按这些记录恢复已确认的前缀。

之后编辑工作区指令或 Memory 文件，模型已经看过的文本保持原样。交接或不兼容的路线变更会建立后继上下文段。切换路线会移除不兼容的 provider 原生续接数据。模型服务的缓存命中取决于具体端点和它返回的用量。[模块契约](module-contracts.md) · [重开与工具输出测试](../../../crates/kuku/tests/context_runtime.rs)

在 provider 边界，只有当 OpenRouter Chat 的模型 ID 以 `anthropic/claude-` 开头时，Kuku 才会通过顶层 ephemeral 标记请求自动提示缓存。Anthropic Messages 适配器保留其独立的顶层标记行为。这些标记只是缓存请求；模型资格、最小前缀长度、上游路由以及实际缓存读写仍取决于具体 provider，应检查返回的用量。此 OpenRouter Chat 规则不适用于任意 Claude 兼容网关，Kuku 也不会在共享前置消息边界添加显式缓存断点。DeepSeek Chat 使用 provider 自身的自动缓存，不使用此标记。

## Prompt 条件模板

采用 [MiniJinja](https://docs.rs/minijinja/latest/minijinja/syntax/) 作为 PromptCatalog 资源共用的运行时渲染器。`system.md`、`agents/`、`blocks/`、`runtime/`、`tools/` 和 `memory/` 中的内置模板和宿主覆盖模板使用同一种语法。Rust 提供结构化事实并决定渲染结果放在哪一层。模板负责发给模型的措辞、条件和排版。

首批迁移包括模型与图像能力、项目策略与模型选择、工具指引、委派、工作区权限和会话交接。这些模板在相关事实解析后渲染。已经提交的上下文段继续使用已记录的 Prompt。

MiniJinja 支持从字符串加载模板，并且可以限制一次渲染的工作量。Tera 也支持运行时模板。Askama 在构建时生成 Rust 代码，适合打包的模板，运行时覆盖需要单独的实现。kuku 保持一套渲染器。 [MiniJinja 环境](https://docs.rs/minijinja/latest/minijinja/struct.Environment.html) · [Tera](https://docs.rs/tera/latest/tera/) · [Askama](https://askama.rs/en/stable/)

### 编写模板

变量用 `{{ value }}` 插值，分支和列表采用标准的 `if`、`elif`、`else` 和 `for`。条件可组合布尔值、成员判断和有类型的比较。排版可用 `join` 等过滤器，共用措辞可用宏或引用目录内的模板。需要展示模板语法本身时，用 raw 块保留原文。

模板文件名用短横线分词，事实字段用下划线分词。多行分支的控制标记独占一行。插值写成 `{{ value }}`，文字段落之间留一行空行。只做格式整理时保留原有措辞和 `<kuku_delegation>` 等运行时标签。

由渲染器选择能力说明。任务行为用选中的自然语言表达。下面的例子直接告诉模型自己能否收到图像，只有工具可用时才渲染 `view_image` 指引。

```jinja
{% if agent.image_input.supported %}
你可以在 kuku 里看到图片附件。
{% if "view_image" in tools %}
任务依赖工作区图片的视觉内容时，用 view_image 查看这张图。
{% endif %}
{% else %}
你在 kuku 里看不到任何图片。将文件名和元数据只作为文件信息。
不要猜测画面内容。引用此前的观察时，注明来源。
{% endif %}
```

候选模型也由同一个渲染器呈现，Rust 提供数据即可。

```jinja
{% for model in models %}
- {{ model.tier }} 可以接收 {{ model.effective_input_modalities | join("、") }}。
{% endfor %}
```

### 事实与放置位置

| 事实 | 含义 |
| --- | --- |
| `agent.model.input_modalities`、`agent.model.output_modalities` | 已解析的模型声明。缺少声明时为 `none`。 |
| `agent.image_input.supported` | 当前 Agent 的实际适配器能否传入图像。能力未确认或适配器只传文本时为 `false`。 |
| `agent.image_input.status` | 机器可读的原因，例如 `supported`、`unconfirmed`、`unsupported_by_model` 或 `unsupported_on_route`。 |
| `tools` | 经过选择和策略过滤后最终提供给模型的工具名。 |
| `models` | 候选模型路线，包含声明模态和实际可用模态。 |
| `step.permission_mode` | 当前 Step 的实际权限模式。 |
| `host` | 预留给 SDK 宿主提供只读的扩展值。 |

事实沿用现有的来源。例如由模型解析器合并手填和发现的模态，由工具注册表解析实际可用的工具。模板需要一个新的运行时判断时，增加对应的结构化字段。授权和派发等有副作用的决策由运行时负责。

模板的位置决定它接收的数据。系统提示和前置消息使用 Agent 模型、工具、工作区等上下文段事实。Step 内的模板还可使用权限模式等变化中的事实。经常变化的提示放在 Step 后缀。各处都使用同一种模板语法。运行时生成角色指令时，目录内的 Agent preset 模板可以读取 preset 名、配置的层级、配置的工具名和工作区。后续 Step 复用这些角色指令。直接传入的 Agent 指令作为文字插入。

### 渲染与验收

- 每份已加载的目录编译一次模板。事实稳定时复用编译结果及已渲染的上下文段。到了配置变更边界，用新事实渲染受影响的上下文段。恢复 Session 时沿用已经提交的文本。
- `include` 和 `import` 按名称引用已加载目录里的模板。工作区指令、Memory 内容、Skill 正文、用户消息和工具输出作为插入值传递。模板指令只在目录资源的原文中解析，插入值按文字保留。
- 按纯文本渲染并启用严格未定义模式。语法错误或必需字段未定义时，在派发前报告模板名和位置。已知的空值按字段契约用 `none`、空集合或 `false` 表示。循环和引用片段也计入渲染工作量和输出上限。 [环境配置](https://docs.rs/minijinja/latest/minijinja/struct.Environment.html) · [严格未定义模式](https://docs.rs/minijinja/latest/minijinja/enum.UndefinedBehavior.html)
- 资源迁移时同步转换现有工具标记和手工替换。加载器把旧记录里的工具标记转换为统一的模板语法。同一迁移后的资源只有一条渲染路径。验收覆盖内置及覆盖模板、能力条件的两条分支、插入值中的模板语法原样保留，以及连续 Step 的请求前缀保持稳定。
