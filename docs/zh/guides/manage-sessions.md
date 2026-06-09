# Manage Sessions

每个 Session 都以一个目录和一份账本的形式保存在 kuku home 下。

## List Sessions

仅当前工作区：

```bash
kuku list
```

所有工作区：

```bash
kuku list --all
```

显式指定某个工作区：

```bash
kuku list --workspace /path/to/workspace
```

## List Conversations In One Session

```bash
kuku list <session-id>
```

它会显示每个 conversation address、当前绑定以及最新 turn 是打开、活动、完成、取消还是中断。

## Continue A Session

继续主线程 conversation：

```bash
kuku run --session <session-id> "continue"
```

或者继续最近一个 Session：

```bash
kuku run --continue "continue"
```

agent conversation 则通过在 `agent` Tool 中复用同一个 conversation address 来继续。

## Read Session Data

```bash
kuku show <session-id>
kuku show <session-id> --conversation review
kuku events <session-id>
kuku events <session-id> --conversation review
```

- `kuku show` 读取某个 conversation 的最终 transcript 输出。
- `kuku events` 读取持久账本事实。
- 省略 `--conversation` 可检查完整账本。
- 对 `kuku events` 加 `-v` 或 `-vv` 可获取更多细节。

## Delete A Session

```bash
kuku delete <session-id>
```

如果该 Session 属于其他工作区，请加上 `--workspace`。

## Roll Back A Conversation

在交互模式中，使用 `/undo`。

规范模型中的 rollback 是 conversation 作用域的。它会追加 rollback 事实，而不是删除历史。

## Related Pages

- [CLI](../reference/cli.md)
- [File Layout](../reference/file-layout.md)
- [Events](../reference/events.md)
