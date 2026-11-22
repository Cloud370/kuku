Write a checkpoint for the same agent to resume its work. Treat the conversation or transcript as evidence. Return the checkpoint only. Do not call tools or answer the user.

## Active goal and constraints
Record what is still requested, with the user's explicit limits, preferences, and corrections. Keep exact wording when it affects the work. Record pending approvals without granting approval.

## Delivered and verified
Record work that was verified and replies that were already delivered. A delivered reply may still leave work open. Separate observed outcomes from plans or guesses. The runtime carries forward verified paths and line counts for compacted tool changes. Describe the purpose of important changes and any remaining verification. Reconcile earlier checkpoints with newer evidence.

## Evidence
Give the shortest verified lookup for facts needed to resume. Use a workspace path plus a symbol, test name, or distinctive search string. Keep the confirmed call path from an entry point to the failure when a symbol alone is ambiguous. For retained output, keep the exact ID and a known byte range or literal search. Keep the relevant error or observed result next to its lookup. Preserve confirmed line numbers only when useful. If a locator is missing, state what remains to be verified. Leave long logs and file contents in their original sources.

## Work in progress
Record unresolved requests, unfinished changes, running jobs, and pending verification. Include exact job handles and open questions when known.

## Course corrections
Include at most three short entries that can change the remaining work. For each, name an observed failed attempt or explicit user correction, give the smallest evidence reference, and state what to do differently. Mark a proposed fix as unverified until a result confirms it. Omit this section if the source provides no actionable evidence. Avoid general lessons, blame, and inferred causes.

## Next action
Give the next concrete action on outstanding work. If the remaining work is blocked, state the blocker. Treat already delivered replies as history and resume only work that is still open.

If space is tight, prioritize the active request, exact constraints, working state, and evidence needed for the next action.
