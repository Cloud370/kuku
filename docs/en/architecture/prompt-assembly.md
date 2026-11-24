# Prompt assembly

The public Session SDK prepares a model Step for one Agent. The input consists of a stable system prompt, a prelude, that Agent's admitted messages and the tools available to it. The provider adapter converts this canonical request to the selected wire protocol.

Keep stable behavioral guidance short and usable across models with different capabilities. Interpret the latest task in context and use the requested deliverable to decide independently whether to reply, write to the workspace or execute a command. Express general responsibilities and verify them across paraphrases and tasks with the same intent.

Use the user's goal to judge whether further work could change the result. Use the least costly checks that resolve uncertainty about the requested outcome, including explicit delivery requirements. The form of delivery follows the request.

## Start a context segment

On first preparation, the SDK loads the Prompt catalog, Agent instructions, workspace instruction files, global and project Memory, and tool definitions. It renders the system and prelude with the selected resources. The built-in project policy excludes the current date; frozen Session preparation appends it as a durable runtime admission on first use and when the date changes. Static Agent and Skill catalogs omit changing conversation counts and loaded/changed state, while turn-specific notices remain in the suffix. The journal records the catalog, rendered material and tool schemas in `runtime.context_segment`. The workspace is read again when preparing a segment for a different workspace. [Preparation and segment creation](../../../crates/kuku/src/query/frozen.rs) · [Prompt assets](../../../crates/kuku/prompts)

A readable, nonblank `AGENTS.override.md` at the selected workspace root supplies all project instructions from that directory, even if Git ignores it. Otherwise, kuku loads the readable `AGENTS.md` and `CLAUDE.md` in that order. The lookup stays within the selected root. Changes apply on fresh assembly. Resuming a Session reuses its committed prelude.

The execution context includes the workspace root and the platform of the process that runs tools. The platform label includes the operating system and architecture. On Linux it also includes the distribution ID and version when the operating system reports them.

## Prepare the next Step

Newly rendered frozen preludes put selected tool guidance (including delegation and memory-tool guidance) first, then Agent behavior, project policy (workspace instructions, workspace/platform and model tiers), and global/project Memory. Guidance remains conditional on the Agent's tools. Behavior precedes workspace policy because it can be shared across workspaces using the same Agent definition; different behavior still forms a distinct prefix. Recorded segments, snapshots and admitted messages retain their original order on resume, including policy-first segments. Successor segments that reuse a recorded prelude preserve its ordering and identity slot; only a freshly rendered prelude uses the new order.

1. The active cursor reads new committed events for this Agent and retains the previously admitted prefix.
2. Newly selected user input, assistant messages, tool-result views and runtime messages are recorded in `runtime.context_admitted`. A large tool result contributes its bounded model view while the full retained output remains available to the caller.
3. The adapter encodes the request. Before dispatch, `runtime.step_prepared` records the segment, input boundary, source watermark, effective settings and request-body hash. `prefix_hash` hashes the fixed segment boundary—system instructions, tools and prelude—and remains stable while that segment is unchanged; it is a canonical-content diagnostic, not a cache-hit guarantee. `prefix_context_head` and `history_prefix_hash` identify and hash only already-confirmed history before the newly appended suffix; the boundary is zero for a new segment, and the history hash changes as confirmed history grows. A matching preparation confirms the admitted suffix. Reopening reconstructs the confirmed prefix from these records.

Editing a workspace instruction or Memory file cannot change text the model already saw. Handoff or an incompatible route establishes a successor segment. A route change removes incompatible provider-native continuation data. Provider cache hits depend on the chosen endpoint and its reports. [Module contracts](module-contracts.md) · [Reopen and tool-output tests](../../../crates/kuku/tests/context_runtime.rs)

At the provider boundary, Kuku requests automatic prompt caching on OpenRouter Chat only when the model ID starts with `anthropic/claude-`, using a top-level ephemeral marker. The Anthropic Messages adapter retains its separate top-level marker behavior. These markers request caching; model eligibility, minimum prefix size, upstream routing, and actual cache reads or writes remain provider-specific and should be checked in returned usage. This OpenRouter Chat rule does not apply to arbitrary Claude-compatible gateways, and Kuku does not add explicit breakpoints at a shared-prelude boundary. DeepSeek Chat uses provider-side automatic caching without this marker.

## Conditional Prompt templates (design)

Use [MiniJinja](https://docs.rs/minijinja/latest/minijinja/syntax/) as the common runtime renderer for PromptCatalog assets. Built-in templates and host overrides use the same syntax in `system.md`, `agents/`, `blocks/`, `runtime/`, `tools/`, and `memory/`. Rust supplies structured facts and chooses where to place the result. The template owns model-facing wording, conditions, and layout.

MiniJinja supports loading templates as strings and limiting rendering work. Tera also supports runtime templates. Askama generates Rust at build time, which fits bundled assets and would require a separate path for runtime overrides. Keep one renderer for kuku. [MiniJinja environment](https://docs.rs/minijinja/latest/minijinja/struct.Environment.html) · [Tera](https://docs.rs/tera/latest/tera/) · [Askama](https://askama.rs/en/stable/)

### Authoring

Use `{{ value }}` for interpolation and standard `if`, `elif`, `else`, and `for` blocks. Conditions can combine booleans, membership and comparisons over typed values. Use filters such as `join` for presentation, and macros or catalog includes for reused text. Use raw blocks for literal template syntax.

Have the renderer choose capability statements. Keep behavioral guidance about the task in the selected prose. The image-input example gives the model its effective ability and puts the tool instruction only in a branch where the tool is available.

```jinja
{% if agent.image_input.supported %}
You can see image attachments in kuku.
{% if "view_image" in tools %}
When the task depends on the visual contents of a workspace image, inspect it with view_image.
{% endif %}
{% else %}
You cannot see any images in kuku. Treat filenames and metadata as file information.
Never guess what the image contains. Attribute any prior observation that you relay.
{% endif %}
```

The same renderer can present a list of candidate models without constructing the lines in Rust.

```jinja
{% for model in models %}
- {{ model.tier }} can receive {{ model.effective_input_modalities | join(", ") }}.
{% endfor %}
```

### Facts and placement

| Fact | Meaning |
| --- | --- |
| `agent.model.input_modalities` and `agent.model.output_modalities` | Resolved model declarations. A missing declaration is `none`. |
| `agent.image_input.supported` | Boolean for image input through the current Agent's actual adapter. It is false when support is unconfirmed or the adapter sends text only. |
| `agent.image_input.status` | Machine-readable reason such as `supported`, `unconfirmed`, `unsupported_by_model`, or `unsupported_on_route`. |
| `tools` | Final tool names after selection and policy filtering. |
| `models` | Candidate routes with declared and effective modalities. |
| `step.permission_mode` | Effective permission mode for the current Step. |
| `host` | Read-only values supplied by an SDK host for its own templates. |

Expose facts from their existing owners. For example, the model resolver merges manual declarations with discovered values, and the tool registry resolves the actual tool set. Add a named fact when a template needs a new runtime decision. Keep decisions involving side effects, such as authorization and dispatch, in the runtime.

Choose template data by its placement. System and prelude assets receive segment facts, such as the Agent's model, tools and workspace. A Step-local asset can also use changing facts, such as a permission mode. Put frequently changing guidance in the Step suffix. Every asset uses the same grammar.

### Rendering and verification

- Compile templates once per loaded catalog. Reuse the compiled environment and the rendered segment when its facts stay stable. At a configuration boundary, render the affected segment with the new facts. Replay committed text from existing Session records.
- Resolve includes and imports from the loaded catalog by name. Treat workspace instructions, memory contents, skill bodies, user messages and tool output as input values. Interpret template directives only in the catalog source, before inserting values.
- Render plaintext with strict undefined handling. Report the asset name and location for invalid syntax or an undefined required field before dispatch. Represent known absence explicitly with `none`, an empty collection, or `false`, according to the field contract. Bound render work and output, including loops and includes. [Environment settings](https://docs.rs/minijinja/latest/minijinja/struct.Environment.html) · [Strict undefined](https://docs.rs/minijinja/latest/minijinja/enum.UndefinedBehavior.html)
- Migrate the modality and tool guidance first. Convert existing tool markers and manual substitutions as their assets move to the common renderer. A migrated asset has one render path. Cover built-in and overridden assets, both sides of capability conditions, literal template syntax in inserted data, and stable request prefixes across repeated Steps.
