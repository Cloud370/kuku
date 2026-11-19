# FrontierHarness small-sample results

This study compares Kuku with Pi 1.4.0 and Codex CLI 0.164.0 on four tasks from FrontierHarness Eval v1.1.0 using DeepSeek-V4.1-Flash-1017. Three short tasks exercise async cancellation, regular expressions and log aggregation. The fourth requires transforming a million CSV rows with Vim macros. Across the 22 published harness configurations using this model, 45 of 88 attempts passed the Vim task after audit. [Dataset](https://hub.harborframework.com/datasets/frontier-harness-eval/frontier-harness-eval) · [Published trials](https://frontierharness.org/live/)

## Latest four-task comparison

| Harness | Tasks passed / attempts | Mean inference USD per attempt | Total inference USD / passes | Mean agent + verifier time |
|---|---:|---:|---:|---:|
| Kuku, latest attempt per task | 3/4 | 0.05799 | 0.07732 | 254 s |
| Pi 1.4.0 | 13/16 | 0.04340 | 0.05342 | 872 s |
| Codex CLI 0.164.0 | 15/16 | 0.03041 | 0.03243 | 834 s |

The latest local attempts come from round 7 for Vim, round 9 for async cancellation, round 11 for log regex and round 12 for log aggregation. All four turns completed. Vim lost its script format check after passing in round 6. Async cancellation accounted for $0.12508 of the latest $0.23197 in inference charges. Each published peer ran four attempts per task. Different execution hosts and missing peer first-token and throughput measurements prevent attributing the observed wall-time differences to a harness alone. [Attempt data and measurement definitions](frontier-v1.1-sample.json)

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
| Log date ranges | 2/2 checks | yes | 42.4 + 12.7 | 0.01085 | 80.6% | 1.44 s | 203 | 34.1 / 34.9 MiB |
| Async cancellation | 6/6 checks | no | 310.9 + 27.1 | 0.07315 | 88.8% | 1.87 s | 212 | 34.3 / 36.2 MiB |
| Log regex | 1/1 check | no | 210.9 + 115.4 | 0.04781 | 87.4% | 1.68 s | 195 | 35.1 / 38.2 MiB |
| Large-file Vim edit | 0/5 checks | no | 331.6 + 12.9 | 0.06756 | 85.5% | 1.68 s | 167 | 35.1 / 36.0 MiB |

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
| Large-file Vim edit | 5/5 | completed | 13 | 0.03732 | 191.8 + 42.3 | 73.1% | 1.33 s | 174.0 | 34.6 / 35.3 MiB |

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
| Log regex | 18/18 | 84.3% | 1.53 s | 160.4 | 3.86 | 35.0 / 36.3 MiB | 11.7 MB / 1.59 MB |

The delayed receipts confirmed Baseten and matched the stream charges on all 65 requests. First output is the proxy's first meaningful delta. Provider-native TTFT was unavailable. The latest Docker block I/O samples include the tool processes and use Docker's rounded values. The WSL2 kernel had `CONFIG_TASKSTATS` disabled and lacked `/proc/PID/io`, so per-process byte I/O is unknown. The SDK's `getrusage` counters reported zero input and output blocks in all three runs. The closed Sessions retained 834,879, 337,833 and 538,502 file bytes for async, Vim and regex, respectively.

- In round 6, async wrote `run.py` on request 2, received three overwrite refusals and read the same digest three times before overwriting it on request 12. In round 7, the first edit on request 8 used the exact digest confirmed by the full write on request 3, with zero intervening file reads. Later partial edits advanced the digest. The session encountered four `snapshot_stale` results and read back the current digest six times. Overall, it used 35 upstream requests and 10.39 CPU seconds, compared with 18 requests and 4.33 CPU seconds in round 6, which stopped on a pre-stream 429.
- In round 7, Kuku wrote `apply_macros.vim` with a final line of `wq` on request 9. Request 10 ran the script on the million-row input and verified that the resulting CSV matched the expected file byte for byte. Request 11 read back the script, including line 7. Request 12 reported completion with `wq` still in place. The task explicitly requires `:wq` or `:x`. Round 6 also used `wq` in a draft, then changed it to `:wq` before delivery and passed all five checks. [Original instructions](https://github.com/harbor-framework/terminal-bench-2-1/blob/7131e4375048a0e408a8fb404b5f499d726b695b/tasks/large-scale-text-editing/instruction.md)
- Regex wrote `regex.txt` on request 2 and completed after 18 requests. Its original Baseten round-4 attempt passed the verifier after reaching the 22-request cap. The old trace contains two `fetch_url` calls and a rejected search against a temporary download path outside the workspace. The follow-up used local file and command tools throughout, so it measured natural turn completion while deterministic Session tests cover retained downloads.

An offline replay ran the original Vim grader in two fresh containers. The retained 166-byte round-7 script reproduced 4/5 checks and reward 0. Inserting one colon before the final `wq` gave 5/5 and reward 1. Only `test_apply_macros_well_formed` changed. Both runs produced the same CSV and used 51 macro keystrokes. The replay used zero model calls. The scored agent attempt is the original 4/5. [Original grader](https://github.com/harbor-framework/terminal-bench-2-1/blob/7131e4375048a0e408a8fb404b5f499d726b695b/tasks/large-scale-text-editing/tests/test_outputs.py#L146-L156)

The published Pi and Codex trials used the same model in the Runta/Baseten environment. On Vim, all three failed Pi attempts recorded an agent timeout. One of the three passing Codex attempts also recorded an agent timeout and its artifact passed the verifier. Public trial records show outcomes and timing without the command transcripts. Costs and total times are means per scored attempt. Each local trial represents one observation under its recorded revision and conditions. [Official live data](https://frontierharness.org/api/live) · [Per-task aggregates](frontier-v1.1-sample.json)

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

The eight streams had 83.6% cached input, proxy first output of 1.32 seconds at p50 and 155.3 generation tokens per second. The SDK used 0.73 CPU seconds and averaged 34.9 MiB RSS with a 35.4 MiB peak. The latest container block I/O was 6.41 MB read and 700 kB written. The SDK's `getrusage` reported zero input and output blocks. Per-process byte I/O and provider-native TTFT were unavailable. The closed Session retained 212,568 file bytes.

Replay the command above with `--case log-summary-date-ranges` at `1895e01` and omit `--max-output-tokens`. Append its result as `--attempt log-summary-date-ranges RESULT_DIR round-8` when regenerating the JSON. The four follow-up trials added **$0.177278256** in provider-reported charges. The pilot OpenRouter ledger accounted **$3.122238164**, including the earlier conservative holds.

## Exact edits and QuickJS tool selection

Six fresh attempts used the same pinned Baseten model, max reasoning and original graders. Async cancellation and log regex kept their 131,072-token output allowance. The four log aggregation trials used 262,144. Each had a $1.50 case cap. All 84 streams received matching provider receipts. The six attempts added **$0.235461352** in inference charges. [Attempt records](frontier-v1.1-sample.json)

| Case / round | Checks | Model requests | QuickJS calls / host actions | Inference USD | SDK + verifier seconds | Cache hit | First output p50 | Generation TPS p50 | SDK mean / peak RSS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Async / 9 | 6/6 | 30 | 0 / 0 | 0.12508 | 429.5 + 25.7 | 94.8% | 1.64 s | 208.2 | 36.0 / 39.4 MiB |
| Log aggregation / 9, control | 2/2 | 7 | 0 / 0 | 0.00779 | 25.8 + 13.5 | 81.3% | 1.24 s | 156.2 | 34.7 / 35.6 MiB |
| Log aggregation / 10, composition guidance | 2/2 | 11 | 1 / 1 | 0.01579 | 51.7 + 12.6 | 84.9% | 1.40 s | 147.5 | 36.0 / 37.1 MiB |
| Log aggregation / 11, output field guidance | 2/2 | 8 | 5 / 5 | 0.01191 | 45.8 + 13.7 | 83.3% | 1.43 s | 142.4 | 35.7 / 36.6 MiB |
| Log aggregation / 12, complete capture guidance | 2/2 | 10 | 0 / 0 | 0.01201 | 48.2 + 12.1 | 85.6% | 1.28 s | 133.8 | 35.0 / 35.6 MiB |
| Log regex / 11 | 1/1 | 18 | 2 / 15 | 0.06287 | 274.5 + 20.8 | 90.9% | 1.55 s | 147.3 | 35.2 / 37.8 MiB |

First output is measured at the proxy. TPS uses native completion tokens and generation duration from the provider receipt. Provider-native TTFT and per-process I/O bytes are unavailable on this host. Container block I/O, harness CPU time and memory sampling coverage are included in the JSON. Peak RSS uses the largest observed process RSS, sampled high-water mark or final self-reported high-water mark. In round 12 the sampler observed 36,436 KiB and the SDK reported 35,504 KiB at exit. Raw observations and the self-usage figure are preserved. Historical peaks use the same reconciliation.

- **Async edits.** The four previously rejected edits each targeted text that still occurred exactly once. A loopback model replayed the original arguments and relevant earlier file operations through both SDK binaries. The old binary rejected all four. The new binary committed all four, with output bytes matching independent single replacements. In the fresh paid run every `edit_file` succeeded and requests fell from 35 to 30. Cost rose from $0.07321 to $0.12508. Output tokens rose from 31,323 to 71,495, including 26,991 reasoning tokens on the new run's first request before any tool executed.
- **Log aggregation.** The control used direct tools. Composition guidance produced one QuickJS call that invoked Python. The command produced 123 bytes, but the model's script returned fields that were undefined. The next model request received only `{"exit":0}` and repeated the command. Replaying the identical command with `result.output_preview` returned all 123 bytes in one execution. In the output-field trial, the model used the correct field but initially counted a clipped preview. The next request saw `capture_complete: true` plus the counts and received no preview truncation flag. On the same 737,299-byte stdout, an offline replay counted 260 entries from the preview and 94,547 from the retained output iterator. The latter count matched an independent scan. Both replays used the public SDK with a loopback model and add no scored attempts. Round 12 clarified the difference between complete capture and preview in the model-facing tool description. That trial used only direct tools, so its full task pass gives no live evidence about the new script advice. The four log runs offer no evidence of lower cost from raising QuickJS call frequency.
- **Log regex.** The model used two QuickJS calls for 15 host actions, including grouped file inspections and cleanup. Both the round-7 direct-tool attempt and this one passed in 18 requests and cost approximately $0.06286. More script calls alone did not reduce the number of model requests in this comparison.

Each row is a single stochastic attempt. In this Chat Completions route the next tool message carries `model_content`. The SDK stores nested action facts separately. The output-field diagnosis above uses the admitted tool message and its persisted host receipt. [Wire encoding](../../../crates/kuku/src/provider/openai_compat.rs) · [Script result](../../../crates/kuku/src/tool/workspace_script/runtime.rs)

Repeat using the earlier Frontier command with the recorded commit and output allowance, a new run ID, and your own budget ledger. The frozen official snapshot and original attempt order are preserved in the report.

## Turn completion in peer implementations

The source checkouts below illustrate design choices. Their revisions can differ from the versions used by the published leaderboard.

- Pi continues for tool results or queued follow-up, then returns control. [Source at d5b3de22](https://github.com/earendil-works/pi/blob/d5b3de22d0e24a3e9c38a9410d3329b099717283/packages/agent/src/agent-loop.ts#L295)
- Oh My Pi records steps in its turn telemetry. Its ordinary completion path follows remaining tool calls and pending messages. [Source at e1ae2cc9](https://github.com/can1357/oh-my-pi/blob/e1ae2cc998efd1ca151b46ebefc0e0dab6c23c63/packages/agent/src/agent-loop.ts#L772)
- Codex evaluates both the model’s follow-up signal and pending user input before ending the turn. [Source at 6ee9bc1b](https://github.com/openai/codex/blob/6ee9bc1ba7b6375faf040042728add58f796e79c/codex-rs/core/src/session/turn.rs#L718)
- DeepSeek Harness completes its ordinary loop when there are no tool calls. [Source at 39163dd3](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/core/agent-loop/src/agent.ts#L593)
- OpenCode applies its last-step behavior when an agent has an explicitly configured step limit. [Source at 63bfe62c](https://github.com/anomalyco/opencode/blob/63bfe62c91fb069c97fcdd7a721aa8f7291cbf49/packages/core/src/session/runner/llm.ts#L263)
