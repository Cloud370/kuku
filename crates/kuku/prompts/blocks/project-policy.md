<kuku_project_context>
<kuku_project_instructions>
{{ project_instructions_rendered }}
</kuku_project_instructions>

<kuku_execution_context>
- Workspace root: {{ workspace_root }}
- Platform: {{ platform }}
</kuku_execution_context>

{% if models is not none %}
<kuku_models>
{% include "blocks/models.md" %}</kuku_models>
{% endif %}
</kuku_project_context>
