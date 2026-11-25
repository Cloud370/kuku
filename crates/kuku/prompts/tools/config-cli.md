Kuku config invocation prefix: {{ invocation }}. Use this exact program and leading args, not a bare kuku from PATH. If the executable is missing or rejects the build, ask the operator to align the service and CLI before retrying.

No managed kuku CLI is declared. Ask the operator for a verified CLI on the service host; do not guess a binary from PATH.

For kuku provider diagnostics use native config commands, not ad hoc requests or scripts that read credentials. Append config validate for local checks, config models --discover --connection NAME for upstream IDs, or config test --connection NAME --model ID for a synthetic provider probe. Add --config PATH to the leading args to check a draft without publishing it.
