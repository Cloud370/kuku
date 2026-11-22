# Performance

In our Linux probes, the idle `kuku` server used **16.5 MiB RSS**, and the SDK's `Session::open` reopened a sealed **509 MiB** Session in **157 ms**.

## Idle processes

| Process | Ready state | Idle RSS | Proportional memory (PSS) |
| --- | --- | ---: | ---: |
| `kuku server` 0.1.0 (`84bdfa23`) | HTTP listener | **16.5 MiB** | 14.4 MiB |
| Codex CLI 0.171.0 `app-server` | WebSocket listener | 78.0 MiB | 66.9 MiB |
| Pi 1.5.2 | RPC `get_state` response | 110.3 MiB | 107.8 MiB |
| Claude Code 2.1.283 | Stream JSON `initialize` acknowledgment | 179.5 MiB | 177.4 MiB |

The medians are from three fresh processes per program on the same WSL2 Linux x86-64 host. We used isolated data directories, sent no prompts or model requests, and sampled one second after each ready state. kuku was a glibc release built with `opt-level = 2` and embedded Web UI. Codex used the native musl CLI executable. Pi ran `--mode rpc --no-session --offline`. Claude Code ran print mode with stream JSON input and output, session persistence disabled, and nonessential network traffic disabled. These figures describe the listed idle modes.

## Reopening a large Session

| Measurement | Plain journal | Sealed journal |
| --- | ---: | ---: |
| Journal on disk | 509 MiB | 65 MiB |
| Reopen median | 2.62 s | **157 ms** |
| Peak RSS at reopen | 13.7 MiB | 14.1 MiB |
| Peak RSS through message rewind | 23.2 MiB | 24.7 MiB |
| Journal reads at reopen | 4,073 | **12** |
| Journal bytes read at reopen | 509 MiB | **274 KiB** |
| Steady short turn | 192 ms | 188 ms |
| SDK's latest 40 events (`events_before`) | 0.49 ms | 1.68 ms |

These figures measure the SDK process. Active Agents, loaded context, and tool subprocesses add memory in a real workload. The wall times come from three paired fresh-process runs on Linux with a warm OS page cache, the same size-optimized release profile, and a fixed SDK model adapter. The read counts come from a separate syscall trace. The synthetic history contains repeated logs, which compress especially well. First sealing took about 3 s. Sealed history pages decompress the frames they traverse. Linux process-kill tests on both sides of publication preserved all 198,454 original committed events. [Evidence and conditions](https://github.com/Cloud370/kuku/issues/151#issuecomment-7075736730) · [Opt-in Session probe](../contributing/testing.md#long-session-profile)

TUI resume also fetches a transcript page from the server. That endpoint currently replays the full journal to resolve Agent ownership and rewind visibility before returning the page.

## Download and installation

Each platform publishes one archive. Linux and macOS use `.tar.gz`. Windows uses `.zip`, including for Windows PowerShell 5.1. The installer shows the manifest’s download size when available and displays progress in interactive terminals. It retries transient download failures, and verifies SHA-256 before installing. A reinstall from a matching cache entry transfers zero package bytes. [Install kuku](../start/install.md)

| Local Linux format comparison | Download size | Extraction median |
| --- | ---: | ---: |
| gzip, the release format | 11.35 MiB | 151 ms |
| xz, for comparison | 8.05 MiB | 410 ms |

Both archives contained the same Linux glibc release binary. Extraction timing used a warm OS page cache. The single published gzip archive works in minimal Linux environments with the usual gzip tools.
