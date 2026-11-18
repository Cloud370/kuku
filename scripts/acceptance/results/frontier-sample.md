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

| Harness | Original task passes | Mean inference USD/attempt | Inference USD/success² | Mean agent + verifier seconds/attempt |
|---|---:|---:|---:|---:|
| kuku | 3/4 | 0.04984 | 0.06646 | 266 |
| Pi 1.4.0 | 13/16 | 0.04340 | 0.05342 | 872 |
| Codex CLI 0.164.0 | 15/16 | 0.03041 | 0.03243 | 834 |
| Claude Code 2.1.308 | 14/16 | 0.05676 | 0.06486 | 819 |
| OpenCode 1.19.4 | 11/16 | 0.02498 | 0.03634 | 812 |
| OpenCode 2.2.7 | 14/16 | 0.04684 | 0.05354 | 370 |
| DeepSeek Harness Standard 0.2.1-rc.3 | 13/16 | 0.06908 | 0.08502 | 877 |

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

## Natural turn completion

Two fresh sessions at `25ef0eb` used the ordinary SDK turn behavior. Neither session had a request-count or step limit. Each had a $1.50 budget, the original task timeout, the same Baseten pin, maximum reasoning effort and 131,072 output tokens per request. The ordinary single pre-stream retry was enabled. These trials appear under `round-6`.

| Task | Original grader | SDK finish | Upstream attempts | Reported inference USD | SDK + verifier seconds | Cached input³ | Proxy first output p50 | Generation TPS | Mean / peak SDK RSS |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|
| Async cancellation | 6/6 | HTTP 429 | 18 | 0.06509 | 248.4 + 25.9 | 91.6% | 1.65 s | 186.0 | 34.3 / 36.7 MiB |
| Large-file Vim edit | 5/5 | completed | 13 | 0.03732 | 191.8 + 42.3 | 73.1% | 1.33 s | 174.0 | 34.6 / 34.8 MiB |

³ Cost, cache and timing measurements cover 17 completed async streams and all 13 Vim streams. The delayed receipts for all 30 confirmed Baseten and matched the stream charges. The 18th async attempt received an upstream HTTP 429 before streaming. The ledger held $0.4718592 against that unpriced request. The SDK's HTTP classifier considered temporary 429 errors retryable, but the runtime filtered out that failure kind. The async session also repeated the same-version overwrite failures described above.

The SDK used 4.33 CPU seconds for async cancellation and 1.87 for Vim. Its recorded context and request preparation took 0.185 and 0.069 seconds in total, respectively. The five async commands reported 1.66 seconds of execution, and the eleven Vim commands reported 64.82 seconds. These measurements give different scopes for interpreting the wall time above.

On the Vim task, the successful local attempt cost $0.03732. The four published Pi attempts averaged $0.08018 (one pass), and the four Codex CLI attempts averaged $0.04673 (three passes). The local model mostly used shell commands on this attempt. The single trial cannot isolate the effect of a particular SDK change. [Per-task comparisons](frontier-v1.1-sample.json)

Through this cohort, OpenRouter streams reported **$0.585663908** across the pilot, including the reconciled canary. The ledger also counted **$2.359296** in conservative holds for four routing failures and the unpriced 429, totaling **$2.944959908** against its $50 allowance. The separate direct DeepSeek ledger carried **$0.168339876** in estimated charges against $30.

## Follow-up on natural turns

The `round-7` async and Vim trials used the debug SDK example at `b0b3cf2` and ran concurrently. The regex trial ran afterward at `bb5012c`, which added retained Session downloads. Each used the same input files, container image and original grader as its earlier Baseten trial. They requested the same DeepSeek model, `max` reasoning effort and 131,072 output tokens. The Baseten pin, $1.50 cap per case and normal pre-stream retry were in effect. The tasks ran with natural turn completion and original timeouts of 900 seconds for async and regex and 1,200 seconds for Vim. [Machine-readable attempts](frontier-v1.1-sample.json)

| Task | Original checks | SDK finish | Upstream requests | Inference USD | SDK + verifier seconds |
|---|---:|---|---:|---:|---:|
| Async cancellation | 6/6 | completed | 35 | 0.073209452 | 283.3 + 26.0 |
| Large-file Vim edit | 4/5 | completed | 12 | 0.032008052 | 164.0 + 39.3 |
| Log regex | 1/1 | completed | 18 | 0.062855860 | 245.8 + 21.7 |

| Task | Matching receipts | Cached input | Proxy first output p50 | Generation TPS | SDK CPU seconds | Mean / peak SDK RSS | Container block I/O read / written |
|---|---:|---:|---:|---:|---:|---:|---:|
| Async cancellation | 35/35 | 91.6% | 1.64 s | 149.9 | 10.39 | 35.2 / 37.0 MiB | 496 kB / 5.54 MB |
| Large-file Vim edit | 12/12 | 77.5% | 1.36 s | 152.7 | 1.82 | 35.2 / 36.0 MiB | 94.6 MB / 132 MB |
| Log regex | 18/18 | 84.3% | 1.53 s | 160.4 | 3.86 | 35.0 / 35.4 MiB | 11.7 MB / 1.59 MB |

The delayed receipts confirmed Baseten and matched the stream charges on all 65 requests. First output is the proxy's first meaningful delta. Provider-native TTFT was unavailable. The latest Docker block I/O samples include the tool processes and use Docker's rounded values. The WSL2 kernel had `CONFIG_TASKSTATS` disabled and lacked `/proc/PID/io`, so per-process byte I/O is unknown. The SDK's `getrusage` counters reported zero input and output blocks in all three runs. The closed Sessions retained 834,879, 337,833 and 538,502 file bytes for async, Vim and regex, respectively.

- In round 6, async wrote `run.py` on request 2, received three overwrite refusals and read the same digest three times before overwriting it on request 12. In round 7, the first edit on request 8 used the exact digest confirmed by the full write on request 3, with zero intervening file reads. Later partial edits advanced the digest. The session encountered four `snapshot_stale` results and read back the current digest six times. Overall, it used 35 upstream requests and 10.39 CPU seconds, compared with 18 requests and 4.33 CPU seconds in round 6, which stopped on a pre-stream 429.
- Vim wrote `apply_macros.vim` on request 9 and received the same digest when it read it on request 11. The grader confirmed byte-equivalent CSV output and macro efficiency. It assigned reward zero because the script's `wq` line lacked the literal colon required by its format check. Round 6 passed all five checks using shell tools for the write. The same-digest read required by an overwrite in the earlier comparison occurred in round 5, on requests 37 through 39.
- Regex wrote `regex.txt` on request 2 and completed after 18 requests. Its original Baseten round-4 attempt passed the verifier after reaching the 22-request cap. The old trace contains two `fetch_url` calls and a rejected search against a temporary download path outside the workspace. The follow-up used local file and command tools throughout, so it measured natural turn completion while deterministic Session tests cover retained downloads.

The published Pi and Codex trials used the same model in the Runta/Baseten environment. Costs and total times are means per scored attempt. Each local trial represents one observation under its recorded revision and conditions. [Official trial aggregates](frontier-v1.1-sample.json)

| Task | Pi 1.4.0 pass / USD / seconds | Codex CLI 0.164.0 pass / USD / seconds |
|---|---:|---:|
| Log date ranges | 4/4 / 0.006900 / 652.3 | 4/4 / 0.011625 / 761.3 |
| Async cancellation | 4/4 / 0.051175 / 728.0 | 4/4 / 0.031325 / 831.3 |
| Large-file Vim edit | 1/4 / 0.080175 / 1,459.5 | 3/4 / 0.046725 / 1,049.0 |
| Log regex | 4/4 / 0.035350 / 648.3 | 4/4 / 0.031950 / 696.3 |

The three new trials added **$0.168073364** in provider-reported inference costs. The pilot OpenRouter ledger then accounted **$3.113033272**, including the historical holds above.

Reproduce a follow-up with a clean checkout of its recorded revision, a unique run ID and the existing OpenRouter ledger and private key file. Use the original case ID, `cancel-async-tasks`, `large-scale-text-editing` or `regex-log`. [FrontierHarness acceptance](../../../docs/en/contributing/testing.md#frontierharness-public-sample)

```sh
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py --run-id "$run_id" benchmark \
  --suite frontier --case "$case" --mode live --provider openrouter --upstream baseten \
  --max-output-tokens 131072 --case-cap-usd 1.50 \
  --sdk-binary target/debug/examples/session_task --ledger "$ledger" --secrets "$keys"
```

Append each result directory as `--attempt CASE RESULT_DIR round-7` after its earlier runs when regenerating the JSON from the frozen official snapshot. Use `--refresh-receipts "$keys"` to retrieve receipts that are missing from the local cache. [Report generator](../frontier_report.py)

## Route output allowance control

The `round-8` log aggregation trial used `1895e01`. It reused the round-4 task files, image and original 900-second timeouts, with the same model, Baseten pin, `max` effort and $1.50 case cap. The launch omitted `--max-output-tokens`. The paid benchmark runner selected 262,144 output tokens from the verified route catalog and reserved up to $0.6291456 for each attempt. The meter and all eight prepared SDK requests recorded 262,144. The SDK leaves the optional output field absent for ordinary requests when callers leave it unset. [Round-8 record](frontier-v1.1-sample.json)

The original grader passed **2/2** and the SDK completed its turn and closed the Session. The SDK took 40.7 seconds and the verifier took 14.9 seconds. Round 4 used 10 upstream requests, 42.4 seconds in the SDK and 12.7 seconds in the verifier. The new attempt received 8 matching Baseten receipts totaling **$0.009204892**.

The meter saw nine local attempts. Attempt five failed to establish an HTTPS connection before the meter could forward the request. The meter returned HTTP 503 and charged zero for that attempt. The SDK recorded a retry of prepared request 5, then continued to normal completion. Its snapshot counted nine attempts, including one without usage. The durable log contained eight logical requests and one retry. The original selector compared the nine attempts with the eight logical requests and rejected the evidence. The corrected inspector passed on the retained SDK and Session files with streaming and paid output checks enabled. The JSON preserves the original `sdk_evidence` failure and the grader's reward.

The eight streams had 83.6% cached input, proxy first output of 1.32 seconds at p50 and 155.3 generation tokens per second. The SDK used 0.73 CPU seconds and averaged 34.9 MiB RSS with a 35.1 MiB peak. The latest container block I/O was 6.41 MB read and 700 kB written. The SDK's `getrusage` reported zero input and output blocks. Per-process byte I/O and provider-native TTFT were unavailable. The closed Session retained 212,568 file bytes.

Replay the command above with `--case log-summary-date-ranges` at `1895e01` and omit `--max-output-tokens`. Append its result as `--attempt log-summary-date-ranges RESULT_DIR round-8` when regenerating the JSON. The four follow-up trials added **$0.177278256** in provider-reported charges. The pilot OpenRouter ledger accounted **$3.122238164**, including the earlier conservative holds.

## Turn completion in peer implementations

The source checkouts below illustrate design choices. Their revisions can differ from the versions used by the published leaderboard.

- Pi continues for tool results or queued follow-up, then returns control. [Source at d5b3de22](https://github.com/earendil-works/pi/blob/d5b3de22d0e24a3e9c38a9410d3329b099717283/packages/agent/src/agent-loop.ts#L295)
- Oh My Pi records steps in its turn telemetry. Its ordinary completion path follows remaining tool calls and pending messages. [Source at e1ae2cc9](https://github.com/can1357/oh-my-pi/blob/e1ae2cc998efd1ca151b46ebefc0e0dab6c23c63/packages/agent/src/agent-loop.ts#L772)
- Codex evaluates both the model’s follow-up signal and pending user input before ending the turn. [Source at 6ee9bc1b](https://github.com/openai/codex/blob/6ee9bc1ba7b6375faf040042728add58f796e79c/codex-rs/core/src/session/turn.rs#L718)
- DeepSeek Harness completes its ordinary loop when there are no tool calls. [Source at 39163dd3](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/core/agent-loop/src/agent.ts#L593)
- OpenCode applies its last-step behavior when an agent has an explicitly configured step limit. [Source at 63bfe62c](https://github.com/anomalyco/opencode/blob/63bfe62c91fb069c97fcdd7a721aa8f7291cbf49/packages/core/src/session/runner/llm.ts#L263)
