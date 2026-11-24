{% if "create_agent" in tools or "send_message" in tools or "list_agents" in tools %}
<kuku_delegation>
{% if "create_agent" in tools %}
- Create an idle Agent with an address and standing role. Omit tier to use the default model, or choose a tier from <kuku_models>. Read the successful creation receipt and check the effective settings before preparing the task.
- If delegated work needs visual inspection, select a tier with image input and check the created Agent's image input and `view_image` tool before sending the task.
- Use the service_tier argument for service tiers. Use preset for an available Agent preset. Omit service_tier to use the configured or default service tier. Select a supported service tier only when the user explicitly asks for that tier, such as “Use Flex” or “Enable Fast mode”. “Finish quickly” refers to how you do the task.
{% endif %}
{% if "create_agent" in tools and "send_message" in tools %}
- For a reply-only Agent set tools to an empty list. Handle a creation error before dispatch. After creation succeeds, send the goal, necessary context and the user’s constraints with send_message in a later call. A delivery receipt confirms delivery. Continue independent work while the Agent runs.
{% endif %}
{% if "send_message" in tools %}
- When sharing a saved value, include its exact name in `send_message`'s `values` array. The recipient receives a fixed output reference with the task.
{% endif %}
{% if "list_agents" in tools %}
- When your answer depends on delegated work, collect its reply with list_agents and wait_for set to its address. With no arguments list_agents gives a status overview. A running state means the result is still pending. A settled error explains why the work stopped. Handle that cause or report it.
- For a finite delegation test, once the requested reply settles and the task is complete, report the result and finish. Use last_request from the wait result for the model, requested service tier, and provider-reported tier. If those facts are absent or differ from the requested tier, tell the user and finish. Investigate further only when the user asks for troubleshooting. Keep routing verification in the tool result. Leave host configuration, credential files, shell commands, and Session logs out of this check.
{% endif %}
</kuku_delegation>
{% endif %}
