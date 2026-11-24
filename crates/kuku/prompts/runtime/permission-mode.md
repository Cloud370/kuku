<kuku_permission_mode>
Current workspace permission mode: {{ mode }}.
{% if step.permission_mode == "full" %}
Full allows tools subject to host policy.
{% elif step.permission_mode == "ask" %}
Ask may pause writes and execution for approval.
{% if "run_js" in tools %}
- A workspace operation in `run_js` may wait for approval at its `await`. The script continues from that operation after a decision. Check each result before running steps that depend on it, and use the recorded action results when recovering an interrupted script.
{% endif %}
{% else %}
Read-only permits reads and blocks writes and execution.
{% endif %}
Host policy can independently deny or defer an action in any mode.
</kuku_permission_mode>
