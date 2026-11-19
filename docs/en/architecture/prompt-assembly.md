# Prompt assembly

The public Session SDK prepares a model Step for one Agent. The input consists of a stable system prompt, a prelude, that Agent's admitted messages and the tools available to it. The provider adapter converts this canonical request to the selected wire protocol.

Keep stable behavioral guidance short and usable across models with different capabilities. Interpret the latest task in context and use the requested deliverable to decide independently whether to reply, write to the workspace or execute a command. Express general responsibilities and verify them across paraphrases and tasks with the same intent.

Use the user's goal to judge whether further work could change the result. Choose checks that resolve a relevant uncertainty, taking their cost into account. The form of delivery follows the request.

## Start a context segment

On first preparation, the SDK loads the Prompt catalog, Agent instructions, workspace instruction files, global and project Memory, and tool definitions. It renders the system and prelude with the selected resources. The journal records the catalog, rendered material and tool schemas in `runtime.context_segment`. The workspace is read again when preparing a segment for a different workspace. [Preparation and segment creation](../../../crates/kuku/src/query/frozen.rs) · [Prompt assets](../../../crates/kuku/prompts)

## Prepare the next Step

1. The active cursor reads new committed events for this Agent and retains the previously admitted prefix.
2. Newly selected user input, assistant messages, tool-result views and runtime messages are recorded in `runtime.context_admitted`. A large tool result contributes its bounded model view while the full retained output remains available to the caller.
3. The adapter encodes the request. Before dispatch, `runtime.step_prepared` records the segment, input boundary, source watermark, effective settings and request-body hash. A matching preparation confirms the admitted suffix. Reopening reconstructs the confirmed prefix from these records.

Editing a workspace instruction or Memory file cannot change text the model already saw. Handoff or an incompatible route establishes a successor segment. A route change removes incompatible provider-native continuation data. Provider cache hits depend on the chosen endpoint and its reports. [Module contracts](module-contracts.md) · [Reopen and tool-output tests](../../../crates/kuku/tests/context_runtime.rs)
