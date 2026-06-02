# Manage Sessions

Each session is stored as a directory under kuku home.

## List Sessions

Current workspace only:

```bash
kuku list
```

All workspaces:

```bash
kuku list --all
```

One workspace explicitly:

```bash
kuku list --workspace /path/to/workspace
```

## Continue a Session

```bash
kuku run --session <session-id> "continue"
```

Or continue the latest session:

```bash
kuku run --continue "continue"
```

## Read Session Data

```bash
kuku show <session-id>
kuku events <session-id>
```

Use `kuku events -v` or `-vv` when you need more detail.

## Delete a Session

```bash
kuku delete <session-id>
```

If the session belongs to another workspace, add `--workspace`.

## Roll Back a Turn

In interactive mode, use `/undo`.

## Related Pages

- [CLI](../reference/cli.md)
- [File Layout](../reference/file-layout.md)
- [Events](../reference/events.md)
