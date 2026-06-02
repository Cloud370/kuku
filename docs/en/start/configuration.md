# Configuration

## Config File Location

By default, kuku uses:

```text
~/.kuku/config.toml
```

If `KUKU_HOME` is set, kuku uses:

```text
$KUKU_HOME/config.toml
```

See [File Layout](../reference/file-layout.md).

## Provider Setup

The default config defines two providers:

- `provider.anthropic` with `api_key = "$ANTHROPIC_API_KEY"`
- `provider.openai` with `api_key = "$OPENAI_API_KEY"`

You can keep secrets in environment variables or store a literal `api_key` in `config.toml`.

For other string settings, you can also use `$ENV_VAR_NAME` in `config.toml`. For example, `base_url = "$KUKU_ANTHROPIC_BASE_URL"` resolves during config load.

## Model Tiers

The default install defines three tiers:

- `strong`
- `balanced`
- `light`

Each tier maps to one provider, one model, a thinking level, and token limits. `default_model = "balanced"` selects the tier used when you do not pass `--model`.

## Common Changes

Show the current config:

```bash
kuku config show
```

Validate the file:

```bash
kuku config validate
```

Set one value:

```bash
kuku config set model.balanced.think high
```

## Discovery, Handoff, Plugins, and Updates

The main non-provider sections are:

- `[discovery]` for agent and skill auto-discovery
- `[handoff]` for long-session summarization thresholds
- `[plugin]` for package hook execution
- `[update]` for release source and channel settings

The exact keys live in [Config](../reference/config.md).

## Update Channels

The default update section uses:

```toml
[update]
source = "github"
channel = "stable"
```

This controls which release manifest kuku should follow. See [Update Manifest](../reference/update-manifest.md).
