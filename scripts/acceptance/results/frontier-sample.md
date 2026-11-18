# FrontierHarness small-sample results

This study compares kuku with Pi 1.4.0 on four tasks from FrontierHarness Eval v1.1.0. Three short tasks exercise async cancellation, regular expressions and log aggregation. The fourth requires transforming a million CSV rows with Vim macros. Across the 22 published harness configurations using this model, 45 of 88 attempts passed that task after audit. Each published Pi task has four attempts. [Dataset](https://hub.harborframework.com/datasets/frontier-harness-eval/frontier-harness-eval) · [Published trials](https://frontierharness.org/live/)

## Diagnostic runs

The first four trials used the direct DeepSeek endpoint, maximum reasoning effort and an 8,192-token output allowance. The original graders passed the async cancellation and log aggregation results. Together the four attempts cost an estimated **$0.09028** at the published peak rate.

- The regular-expression session finished its first response with `length`. Its entire 8,192-token completion went to reasoning before it called a tool. A fresh trial of the same SDK with a 32,768-token allowance passed.
- The initial Vim script produced byte-identical output and passed four of five checks. Its exit command omitted a colon required by the task. A generic prompt revision clarified command invocation and asked the agent to check explicit artifact formats. On a fresh trial the original grader passed all five checks. That session still failed the SDK completion check. It reached a 131,072-byte serialized-request guard before its next request, and the example's top-level error was empty.
- The SDK example had coupled a token setting to a byte setting. The comparison uses separate limits for model context, generated tokens and encoded request bytes. The raw task reward and the SDK's ability to finish are both recorded.

The initial four runs and the output-allowance repeat used SDK commit `8470623`. The format-guidance repeat used SDK commit `ab865ba`. Six direct-endpoint runs cost an estimated **$0.16834** in total. These observations motivated a single configuration shared by all four subsequent tasks. Each rerun used a new session with the original task instructions.

## Shared configuration and comparison

The four scored attempts used DeepSeek-V4.1-Flash-1017 through OpenRouter, pinned to Baseten with automatic fallback disabled. Each got 131,072 output tokens per request, up to 22 model requests and the original task timeout. The meter confirmed the model and received prices from completed streams. The independent generation lookup was unavailable in this cohort. Baseten is the requested route. [All attempts, provider evidence and 22 comparison harnesses](frontier-v1.1-sample.json)

| Task | Original grader | SDK finished | SDK + verifier seconds | Inference USD | Cached input | TTFT p50¹ | Output TPS p50¹ | Mean / peak SDK RSS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Log date ranges | 2/2 checks | yes | 42.4 + 12.7 | 0.01085 | 80.6% | 1.44 s | 203 | 34.1 / 34.3 MiB |
| Async cancellation | 6/6 checks | no | 310.9 + 27.1 | 0.07315 | 88.8% | 1.87 s | 212 | 34.3 / 36.2 MiB |
| Log regex | 1/1 check | no | 210.9 + 115.4 | 0.04781 | 87.4% | 1.68 s | 195 | 35.1 / 37.5 MiB |
| Large-file Vim edit | 0/5 checks | no | 331.6 + 12.9 | 0.06756 | 85.5% | 1.68 s | 167 | 35.1 / 35.2 MiB |

¹ TTFT measures request dispatch to the first meaningful output observed at the proxy. Stream TPS divides completion tokens by the interval from the first meaningful delta through end of stream. Reasoning tokens contribute to completion tokens. Native provider TTFT was unavailable. Memory, CPU, I/O units, sampling coverage and per-request metric coverage are included in the JSON report.

| Harness | Original task passes | Mean inference USD/attempt | Inference USD/success² |
|---|---:|---:|---:|
| kuku | 3/4 | 0.04984 | 0.06646 |
| Pi 1.4.0 | 13/16 | 0.04340 | 0.05342 |
| Codex CLI 0.164.0 | 15/16 | 0.03041 | 0.03243 |
| Claude Code 2.1.308 | 14/16 | 0.05676 | 0.06486 |
| OpenCode 1.19.4 | 11/16 | 0.02498 | 0.03634 |
| OpenCode 2.2.7 | 14/16 | 0.04684 | 0.05354 |
| DeepSeek Harness Standard 0.2.1-rc.3 | 13/16 | 0.06908 | 0.08502 |

² Total model cost of all scored attempts divided by the number of successes. Official harness rates use the site's audit adjustments and four trials per task. The kuku row uses the original verifier once per task and excludes setup and diagnostic attempts. Only log aggregation also met kuku's separate SDK completion check. Different runtimes and replica counts limit speed and ranking claims. The local regex session used two external searches; local results have no official FrontierHarness audit.

The 22-request limit stopped three kuku sessions. Async cancellation had written the target by request two and spent its final requests cleaning up scratch tests. The regex session wrote its target on request nineteen. The large-file session worked on a scratch Vim script and reached the limit before writing the requested script. These outcomes separate delivery and turn completion from the correctness of the files actually produced.

The JSON round labels identify distinct observations. `first`, `output32k` and `guidance` are direct-provider diagnostics. `round-1` was a local request-validation failure with zero upstream calls. `round-2` was an OpenRouter routing failure; an independent diagnostic confirmed that the account policy excluded the official DeepSeek endpoint. `round-3` was the Baseten log canary. Its three completed streams reported $0.00596856; the initial meter provisionally accounted $1.4155776 before reconciling those charges. `round-4` is the four-task comparison above. Historical meter upper bounds are preserved as recorded.

## Request-limit check

The second local cohort repeated only async cancellation and large-file editing. Both received up to 48 requests, using the same valid task inputs, model, output allowance, endpoint pin, SDK build and per-case dollar cap. Route diagnostics were updated in the host meter. The async solution passed 6/6 checks. Its 47th request encountered a connection error before the provider returned HTTP headers. The example had disabled the normal SDK transient retry. The Vim script passed 5/5 checks, but the SDK reached its 48-request limit. These two trials cost $0.27792 combined. Both outcomes appear under `round-5`; they do not replace the four-task result.

Subsequent OpenRouter generation receipts confirmed Baseten and matched the streamed price on all 94 completed requests. The proxy measured time to the first meaningful output, including reasoning or tool arguments. Generation TPS divides the total native completion tokens by the sum of the receipt generation durations. Provider-native TTFT was unavailable.

| Task | Matched receipts | Inference USD | Cached input | Proxy first output p50 | Generation TPS | SDK + verifier seconds | Mean / peak SDK RSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| Async cancellation | 46/46 | 0.15325 | 96.2% | 2.06 s | 155.5 | 730.4 + 26.2 | 36.4 / 41.2 MiB |
| Large-file Vim edit | 48/48 | 0.12467 | 94.8% | 1.89 s | 150.0 | 947.3 + 30.7 | 35.9 / 37.9 MiB |

The last sampled container block I/O was 0 B read and 32.6 MB written for async cancellation, and 0 B read and 1.39 GB written for Vim. Docker rounds these values and includes task tools. The SDK's own `getrusage` recorded zero input and output blocks in both runs. This host's WSL2 kernel did not expose `/proc/PID/io`, so logical read and write bytes for the SDK process are unknown.

Both sessions also exposed redundant file reads. In the async session, `write_file` confirmed writing a scratch file on request 11 but refused to overwrite it on request 16. Reading it on request 17 returned the same hash. In the Vim session, the agent wrote its complete script on request 22, received the same refusal on request 37 and read back the unchanged hash on request 38. Pi's edit tool and Codex's patch implementation read current content within the edit operation. [Pi edit](https://github.com/earendil-works/pi/blob/d5b3de22d0e24a3e9c38a9410d3329b099717283/packages/coding-agent/src/core/tools/edit.ts#L178) · [Codex patch](https://github.com/openai/codex/blob/6ee9bc1ba7b6375faf040042728add58f796e79c/codex-rs/apply-patch/src/file_update.rs#L25)

## Turn completion in peer implementations

The source checkouts below illustrate design choices. Their revisions can differ from the versions used by the published leaderboard.

- Pi continues for tool results or queued follow-up, then returns control. [Source at d5b3de22](https://github.com/earendil-works/pi/blob/d5b3de22d0e24a3e9c38a9410d3329b099717283/packages/agent/src/agent-loop.ts#L295)
- Oh My Pi records steps in its turn telemetry. Its ordinary completion path follows remaining tool calls and pending messages. [Source at e1ae2cc9](https://github.com/can1357/oh-my-pi/blob/e1ae2cc998efd1ca151b46ebefc0e0dab6c23c63/packages/agent/src/agent-loop.ts#L772)
- Codex evaluates both the model’s follow-up signal and pending user input before ending the turn. [Source at 6ee9bc1b](https://github.com/openai/codex/blob/6ee9bc1ba7b6375faf040042728add58f796e79c/codex-rs/core/src/session/turn.rs#L718)
- DeepSeek Harness completes its ordinary loop when there are no tool calls. [Source at 39163dd3](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/core/agent-loop/src/agent.ts#L593)
- OpenCode applies its last-step behavior when an agent has an explicitly configured step limit. [Source at 63bfe62c](https://github.com/anomalyco/opencode/blob/63bfe62c91fb069c97fcdd7a721aa8f7291cbf49/packages/core/src/session/runner/llm.ts#L263)
