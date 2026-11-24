# Performance

On a small ARM64 Linux device with **371 MiB of RAM**, kuku opened a conversation with more than **1,000 turns**, paged through its history, and continued it in the same Session.

## Idle processes

| Process | Ready state | Idle RSS | Proportional memory (PSS) |
| --- | --- | ---: | ---: |
| `kuku server` 0.1.0 (`84bdfa23`) | HTTP listener | **16.5 MiB** | 14.4 MiB |
| Codex CLI 0.171.0 `app-server` | WebSocket listener | 78.0 MiB | 66.9 MiB |
| Pi 1.5.2 | RPC `get_state` response | 110.3 MiB | 107.8 MiB |
| Claude Code 2.1.283 | Stream JSON `initialize` acknowledgment | 179.5 MiB | 177.4 MiB |

The medians come from three fresh processes per program on the same WSL2 Linux x86-64 host, using isolated data directories with no prompts or model requests, sampled one second after each reached its ready state. kuku was a glibc release build (`opt-level = 2`, embedded Web UI), Codex used its native musl CLI binary, Pi ran `--mode rpc --no-session --offline`, and Claude Code ran print mode with stream JSON I/O, session persistence disabled, and nonessential network traffic disabled. These numbers only cover the idle modes listed above.

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

These figures measure the SDK process. Active Agents, loaded context, and tool subprocesses add memory in a real workload. The wall times come from three paired fresh-process runs on Linux with a warm OS page cache, the same size-optimized release profile, and a fixed SDK model adapter. The read counts come from a separate syscall trace. The synthetic history contains repeated logs, which compress especially well. First sealing took about 3 s. Sealed history pages decompress the frames they traverse. Linux process-kill tests on both sides of publication preserved all 198,454 original committed events. [Evidence and conditions](https://github.com/Cloud370/kuku/issues/151#issuecomment-7075736730) · [Opt-in Session probe](../contributing/testing-advanced.md#long-session-profile)

### TUI resume

In a separate WSL2 run with the same logical history and a warm OS page cache, selecting the sealed **509 MiB** Session displayed its first transcript page in **50 ms** with a server peak of **20 MiB RSS**. An earlier build took **3.61 s** and peaked at **1.09 GiB RSS** on the same fixture.

The server reads bounded event pages with SDK history visibility and Turn numbering. An unsealed journal needs an initial streaming scan to build the projection. On the unsealed fixture, selection to first transcript took **2.15 s** and the server peaked at **18.5 MiB RSS**.

## Low-memory ARM64 device

We tested a four-core Snapdragon 410 device with 371 MiB of RAM reported by Linux and 256 MiB of configured swap. We opened the Session in the terminal and fetched older pages while a follow-up sent through the HTTP API was active. The reply from a local simulated model appeared in the terminal. The Linux OOM kill counter stayed at zero. [Tested build](https://github.com/Cloud370/kuku/commit/327212582ee4025f8d9360934e494801fcbe7fef)

The stress sample was one Session with **1,071 turns**, **2,142 visible messages**, **27,279 tool calls**, and **189 context compactions**. The user messages and final assistant replies contain about **206,000 tokens** with `o200k_base`. The complete journal held about 198,000 events, including tool results, model responses, and context records. Tool results and context records accounted for about 62% of the bytes. The journal occupied **509 MiB** before sealing and **65 MiB** on disk after sealing. [Tokenizer](https://github.com/openai/tiktoken)

| What we measured | Result |
| --- | ---: |
| Peak resident memory of the server | **21.6 MiB** |
| Peak resident memory of the terminal client | **12.7 MiB** |
| Older conversation pages during the active request, 40 entries per page | **72–153 ms** per page |
| Listing the Session with its journal out of the page cache | **316 KiB read from storage**, 0 writes |
| Listing it again with the journal cached | **0 storage reads or writes** |

For the first listing, we asked Linux to evict only the test journal's cached pages. Storage I/O comes from the server process's Linux `read_bytes` and `write_bytes` counters. The page timings are from four regular transcript pages in one run. [Linux I/O counters](https://docs.kernel.org/6.17/filesystems/proc.html#proc-pid-io-display-the-io-accounting-fields)

## Download and installation

Each platform publishes one archive. Linux and macOS use `.tar.gz`. Windows uses `.zip`, including for Windows PowerShell 5.1. The installer shows the manifest’s download size when available and displays progress in interactive terminals. It retries transient download failures, and verifies SHA-256 before installing. A reinstall from a matching cache entry transfers zero package bytes. [Install kuku](../start/install.md)

| Local Linux format comparison | Download size | Extraction median |
| --- | ---: | ---: |
| gzip, the release format | 11.35 MiB | 151 ms |
| xz, for comparison | 8.05 MiB | 410 ms |

Both archives contained the same Linux glibc release binary. Extraction timing used a warm OS page cache. The single published gzip archive works in minimal Linux environments with the usual gzip tools.
