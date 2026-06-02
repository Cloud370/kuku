# Manage Sessions

每个 Session 都会以目录形式存储在 kuku home 下。

## List Sessions

仅当前工作区：

```bash
kuku list
```

所有工作区：

```bash
kuku list --all
```

显式指定一个工作区：

```bash
kuku list --workspace /path/to/workspace
```

## Continue a Session

```bash
kuku run --session <session-id> "continue"
```

或者继续最新的 Session：

```bash
kuku run --continue "continue"
```

## Read Session Data

```bash
kuku show <session-id>
kuku events <session-id>
```

当你需要更多细节时，使用 `kuku events -v` 或 `-vv`。

## Delete a Session

```bash
kuku delete <session-id>
```

如果该 Session 属于另一个工作区，请加上 `--workspace`。

## Roll Back a Turn

在交互模式中，使用 `/undo`。

## Related Pages

- [CLI](../reference/cli.md)
- [File Layout](../reference/file-layout.md)
- [Events](../reference/events.md)
