# Environment Variables

## Runtime Variables

| Variable | Meaning |
|---|---|
| `KUKU_HOME` | Override the default runtime home directory |
| `ANTHROPIC_API_KEY` | Common API key source for `provider.anthropic` |
| `OPENAI_API_KEY` | Common API key source for `provider.openai` |

`config.toml` can also reference any environment variable by using `$NAME` in a provider `api_key` field.

## Default Behavior Without `KUKU_HOME`

If `KUKU_HOME` is unset, kuku uses:

```text
~/.kuku
```

## Hook Process Variables

When kuku runs a package hook, it sets these variables for the hook process:

| Variable | Meaning |
|---|---|
| `KUKU_SESSION_DIR` | Absolute path to the current session directory |
| `KUKU_WORKSPACE` | Absolute path to the current workspace |
| `KUKU_PACKAGE_DIR` | Absolute path to the package root |

The hook process also inherits `PATH`, `HOME`, `USERPROFILE` on Windows, `LANG`, and `LC_ALL`.

Other secrets are not passed through automatically.
