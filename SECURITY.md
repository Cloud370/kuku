# Security

## Threat Model

kuku is a terminal coding agent that runs locally on your machine. It executes shell commands, reads/writes files, and makes API calls to LLM providers.

### No Sandbox

kuku does **not** sandbox the agent. The permission system exists to help users stay aware of what actions the agent is taking — it prompts for confirmation before executing commands or writing files. It is not designed to provide security isolation.

If you need true isolation, run kuku inside a Docker container or VM.

### Server Mode

Server mode (`kuku-server`) is opt-in. The server binds to `127.0.0.1` by default. It is the user's responsibility to secure the server — place it behind a reverse proxy with TLS and authentication if exposing it beyond localhost.

### Out of Scope

| Category | Rationale |
|----------|-----------|
| **Server access when opted-in** | If you enable server mode, API access is expected behavior |
| **Sandbox escapes** | The permission system is not a sandbox (see above) |
| **LLM provider data handling** | Data sent to your configured LLM provider is governed by their policies |
| **Malicious config files** | Users control their own `~/.kuku/` directory; modifying it is not an attack vector |
| **Skills / subagent behavior** | Skills and subagents execute with the user's own permissions |
| **Prompt injection via files** | The agent reads files in your project; malicious file content is indistinguishable from legitimate project content |

## Reporting a Vulnerability

To report a security issue, use the GitHub Security Advisory ["Report a Vulnerability"](https://github.com/Cloud370/kuku/security/advisories/new) tab.

We will respond within 5 business days to acknowledge your report and outline next steps. After the initial reply, we will keep you informed of progress toward a fix.

We do not accept AI-generated security reports.
