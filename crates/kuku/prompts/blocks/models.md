{% for model in models %}
{% set thinking = model.reasoning_effort or "provider default" %}
{% set thinking_options = model.reasoning_choices | join(", ") if model.reasoning_choices else "use default" %}
{% set service = model.service_tier or "provider default" %}
{% set service_options = "unknown" if model.service_choices is none else (model.service_choices | join(", ") if model.service_choices else "none") %}
{% set modalities = "input_modalities " ~ model.input_modalities_text ~ ", output_modalities " ~ model.output_modalities_text %}
connection={{ model.connection_text }}, model={{ model.model_text }}{% if model.purpose %} — {{ model.purpose }}{% endif %} (thinking {{ thinking }}, thinking options {{ thinking_options }}, service {{ service }}, service options {{ service_options }}, {{ modalities }}, route input {{ model.effective_input_modalities | join(", ") }}, Agent output {{ model.output }}, image input {{ model.image_input_label }})
{% else %}
No registered text models are available.
{% endfor %}
