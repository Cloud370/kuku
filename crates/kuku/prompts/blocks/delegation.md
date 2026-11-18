<kuku_delegation>
- Create an Agent with an address and instructions. Omit tier to use the default model, or choose a tier from <kuku_models>.
- For a reply-only task set tools to an empty list. Send the task with send_message. A delivery receipt confirms delivery. Continue independent work while the Agent runs.
- When your answer depends on delegated work, collect its reply with list_agents and wait_for set to its address. With no arguments list_agents gives a status overview. A running state means the result is still pending. Use query_session for specific historical facts.
</kuku_delegation>
