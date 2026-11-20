# Web client

The browser client is a workbench for one Session at a time: it shows what the selected Agent did, what it is doing now, and what needs a human decision. It is not a chat product and not an editor. It keeps the conversational grammar people already know, and grows a structured work log around it as work accumulates. This page owns the target design. [Interactive clients](interactive-clients.md) owns the shared client contract and delivery order across terminal and browser.

## Position

| The client is | The client is not |
|---|---|
| A reader and steering surface for Agent work | A chat product with a prompt box |
| A place to review diffs, command results and test reports | An editor or file browser |
| A way to see several Agents and pending decisions at once | A dashboard of metrics and charts |
| Reachable from a shareable URL for any Session, Agent and entry | A replacement for the terminal's editing and keybinding depth |

The browser earns its existence through reading, overview, navigation and recall: long output, diffs, tables and test reports read better here than in a terminal; several Agents and their pending questions fit on one screen; every entry has a stable address; a tab can be left open while work continues on the server. Those four capabilities set the scope. Anything that only reproduces terminal editing belongs to the terminal.

## What is retained and what is replaced

The interface is rewritten. These properties of the current application are kept because they encode agreed contracts, not presentation:

- The HTTP and NDJSON Session protocol, including command identity, receipts, replay and observation cursors.
- Durable unconfirmed-command recovery, including receipt lookup, replay-only resubmission and the conflict, missing and unknown outcomes.
- Observation de-duplication: barrier cursors, execution-scoped live preview, and replacement of previews by committed entries.
- Same-origin asset delivery through the unified binary, ticket exchange for the local handoff, and explicit token authentication for remote servers.
- The shared protocol fixture under `crates/kuku-server/tests/common/fixtures` as the cross-client contract check.

The component tree, state organization, styling system and build plumbing are replaced. The current application's client-side lint and commit hooks are dropped in favour of the repository-level workflow.

## Interaction model

### Conversational grammar, workbench structure

Familiarity is a requirement. A new user who has only used a chat assistant must be able to send a first message and understand the first reply without explanation. The page therefore keeps the grammar of a conversation and replaces only its containers:

| Kept | Not used |
|---|---|
| Composer pinned to the bottom, Enter sends, Shift+Enter inserts a newline | Message bubbles with tails, avatar columns, opposing alignment |
| The submitted message appears immediately below the log | One card per message |
| Replies stream in place and the view follows them | Typing indicators, decorative animation |
| Reasoning is available above the reply, collapsed by default | Anything hidden that the user must take on faith |

The submitted message sits on a distinct inset surface. That surface is the universal "I spoke" signal, so recognition costs nothing, while the layout stays a work log rather than a bubble stream. Agent prose is not placed in a container: it is typeset as a document, so a long reply reads as an article rather than as text trapped in a box.

Structure is progressive. A short Session looks like a clean conversation. As turns and tool calls accumulate, chapter headers, grouped tool activity and the turn spine appear; every added structure is collapsible, and none of it is required to follow a single exchange.

### Turn chapter

A Turn is the unit of work. Its header carries the turn number, terminal state, elapsed time, tool call count and token usage when recorded, and remains pinned while its body scrolls. Turns are joined by a continuous vertical spine with a node per turn, which gives the log a reading position and makes a long Session navigable without a tree view. The spine is the only persistent structural ornament; no other container draws a border by default.

Statistics use tabular numerals so values never shift as they update. The header shows the live elapsed time for the active turn and settles it on completion. Field order follows the terminal's status line so the two clients teach the same reading order.

### Tool activity

Tool calls are one-line rows in a quiet list, not cards. Each row states what happened in a sentence: what was read, what command ran, which file changed. Consecutive successful reads, searches and listings within one turn collapse into one row with counts. Writes and edits render as file rows with a path, a bounded diff preview and a line delta. Failures, running calls and calls awaiting authorization stay visibly distinct without shouting.

Detail lives in a peek surface, opened on demand. It shows arguments, the structured outcome, the diff, the command facts or the retained output, with retained output paged by offset. One gesture — click, Enter or Space — opens and closes it, and opening it never moves the reading position.

### Sending

The composer belongs to the selected Agent and keeps that Agent's draft. A new Session's identity and the first command's identity are generated before the first send, so a lost reply is retried against the same identities and creates one Session with one delivery. Enter steers a busy Agent; an explicit action queues for the next turn. Draft, unconfirmed send, queued input and admitted input are four distinguishable states of the same message, shown on the message itself rather than in a separate log.

### Attention and pending work

All work that needs a human is collected rather than scattered:

- Waiting questions and permission requests raise one indicator in the header with a count, and open a single decision surface that names the Agent and the exact request. Answering in one tab settles it everywhere, and other tabs observe the result.
- Input still queued for an Agent sits directly above the composer, with its delivery timing and a withdraw action.
- Unconfirmed commands appear inline under the message they belong to. The send trail — sending, receipt, queued, admitted — is shown in one small line, without explanation.

A user should never need to visit each Agent to find out who is blocked.

### Reading position

The log follows new output while the user is at the bottom. Scrolling up suspends following and preserves the anchor as new entries and previews arrive. A return-to-latest control sits against the composer, not in a floating corner. Switching Agents restores that Agent's scroll position and draft.

### Several Agents

The default layout gives one Agent the reading surface and lists all Agents in a rail with name, state, a one-line account of what it is doing, and a marker when it needs a person. Selecting an Agent is navigation, not creation: it never issues a model request, and each Agent keeps its own log, draft and position.

Work that crosses Agents appears as a reference in the log — a row naming the other Agent and the action — rather than as copied content. Following it navigates. Creation ancestry is not shown as hierarchy, because ancestry is not ownership or authority.

A side-by-side split for two Agents is a later addition and stays limited to two panes. A time-aligned lane view for a Team is out of scope until a Team workflow requires it.

### Layouts

One component set serves every width; layouts change, components do not.

| Width | Shape |
|---|---|
| Wide | Agent rail, log, and a peek surface as a side panel |
| Medium | Log with a collapsible rail and a peek surface as an overlay |
| Narrow | Single column log; rail and peek become bottom sheets; composer pinned above the keyboard |

The narrow layout is designed rather than deferred: touch targets, safe-area insets and the visual viewport are handled from the start, while polished phone-specific gestures wait for a later stage.

### Addresses

The URL identifies what is being read: workspace, Session, Agent, and optionally one entry. Opening a link restores that view; a shared link to a failed command is as useful as a shared screenshot. Presentation preferences — theme, language, density, collapsed state — stay local and never enter the URL.

### Finding work

A workspace's Sessions are listed with a recognizable title, last activity and any waiting Agent, and selecting one navigates rather than loading a preview. Search is locale-aware and covers titles and first prompts. There is no separate dashboard: the list is a navigation aid, not a metric surface.

## Visual system

### Direction

Calm, precise, restrained. One canvas, dividers instead of nested boxes, one accent, and colour reserved for state. Hierarchy comes from size, weight and space, not from shadows or borders. The surface stays quiet by default so that waiting, failure and success are the only things that stand out.

Both themes share one hue family with the terminal: cool blue-grey neutrals and a teal accent. The light theme is a cool off-white rather than a warm paper, so the two clients read as one product in either theme.

### Token layers

Colour, type, spacing, radius and motion are defined once and consumed by role. Components never contain a literal colour value.

| Layer | Contents | Consumer |
|---|---|---|
| Primitive | Raw palette ramps, neutral and accent and state hues per theme | Semantic layer only |
| Semantic | Roles such as `surface`, `panel`, `text.primary`, `text.muted`, `accent`, `state.running`, `state.waiting`, `state.success`, `state.error`, `border.subtle`, `code.background` | Every component and story |
| Component | A short list of composed values where a role alone is insufficient, such as the inset user surface | One component family |

The primitive values are derived from the terminal palette, so a change here is a change to both clients' character. Semantic role names match the terminal's style constants one-for-one, which keeps the two clients describing the same roles instead of inventing vocabulary.

Tokens are plain custom properties on the root element and on the theme attribute and are exposed to the utility layer through its theme directive (Tailwind's `@theme` today), so adjusting a value is a single-file edit and every usage follows. A dev-only token page renders the full palette, type ramp and spacing scale for both themes, and Storybook switches themes with a toolbar control. A unit test asserts contrast for every text-on-surface pair in both themes so that tuning a value cannot silently break readability.

### Typography

Two families: a UI face and a code face. The UI face is a system stack with explicit Chinese and Japanese fallbacks; no web font is required for the interface. Code uses a monospace stack with a CJK-capable fallback so that Chinese inside code fences keeps its alignment. Sizes are a short scale with two dominant sizes for body and list text, one small size for metadata, and one large size for chapter headings. Measures stay bounded on wide screens.

### Motion

Two animations only: the streaming caret and the phase indicator. State changes fade. Durations stay short, easing is linear or near-linear, and reduced-motion preference removes movement while keeping state legible.

Layout is stable by construction rather than by animation. The composer grows upward without displacing the log, a tool row keeps its height when its state changes, the turn header reserves space for its elapsed time, and metrics use tabular numerals. Smoothness is mostly the absence of movement.

## Internationalization

The interface ships in English and Simplified Chinese, with the structure ready for more.

- Messages are extracted at build time from source; a missing translation fails the build rather than falling back silently.
- Formatting of dates, numbers, durations and collation goes through the internationalization library, never through string concatenation. Session search uses locale-aware collation.
- Server errors are localized by code. The client maps known codes to catalogued messages and falls back to the server's text for unknown ones, so a new server error is readable before it is translated.
- User and model content is never translated. Log text, paths, diffs and code carry automatic direction, and code blocks keep left-to-right flow.
- Typography adapts: taller line height for CJK text, no letter-spacing or uppercasing on headings, and flexible widths instead of fixed ones.

Language is a local preference, not part of the URL.

## Architecture

### Layers

```
SDK facts          CommandExecutionFacts, FileMutationFacts, TurnSummary, AgentProgress
Server projection  typed Session entries: what each event is
Client view        what each client shows, per viewport and preference
```

The boundary is a rule, not a preference. A derivation belongs to the server when both clients would compute the same answer from the same facts and the answer requires tool or SDK semantics: pairing a tool call with its result, classifying a tool, computing a diff summary, turn numbering, turn statistics, retained output references. A derivation belongs to the client when it depends on viewport, time, interaction or preference: grouping thresholds, collapse state, scroll anchors, timers and phase debounce, number styling, breakpoints.

The clients do not share a view layer. The terminal's view layer produces width-aware text rows for a cell buffer, which the browser cannot use; what is worth sharing is meaning, and meaning belongs in the projection.

### Server projection contract

The projection is currently built from untyped JSON in the server's wire layer, and both clients read it by string keys. That is the source of existing drift: the server emits `reasoning_preview` while the browser reads `thinking`, the live `thinking` event carries no content, and turn summaries, job completions and turn numbers are emitted but unused. The first stage replaces this with a typed contract:

- One Rust type per entry variant, serialized as a tagged enum, covering user, assistant, reasoning, tool call and result, question, permission, replies, job completion, turn summary, status and error.
- Tool entries carry a classification, a lifecycle state, and the structured facts that the clients need for their default rendering, including diff, command facts, test reports and retained output references.
- Live events carry their kind and payload, including reasoning text.
- Every history and live entry carries the turn number and turn summary when known.
- Errors carry a stable code from a documented catalogue.
- TypeScript types are generated from these Rust types. The browser keeps no hand-written protocol types.
- The shared protocol fixture is regenerated from the typed contract and remains the cross-client check.

The terminal client migrates to the same types in the same change, so the contract has two real consumers from the start.

### Client state

The Session view is a pure reducer: `(state, frame) → state`, where a frame is a snapshot, a history page, a committed entry, a live event or a connection transition. It is independent of React, deterministic, and replayable, so it is tested by replaying recorded streams and the shared fixture rather than by rendering.

React subscribes through an external store. Request-and-response resources — Session list, model catalogue, skills, path completion, service status — use the query layer with caching and cancellation. Durable command state stays a separate store, because it survives navigation and reloads.

Layout, collapse, theme and language preferences are local and per client. None of them are written back to the server.

### Streaming and rendering

Markdown is parsed incrementally: closed blocks are rendered once and cached, and only the trailing block is re-parsed as text arrives. Code highlighting loads per language and degrades to plain text. Long logs are virtualized with stable measured heights and anchored scrolling. Retained output is fetched by offset and appended, never re-read whole.

### Assets and packaging

The built application is embedded in the unified binary and served from the same origin as the API. A source build without the embedded assets keeps the current fallback behaviour. The development server proxies the API to an explicit local service and prints the address it is proxying to. The build stays reproducible from a lockfile, and its size budget is tracked as part of the release checks, because highlighting grammars and fonts are the parts that grow.

## Accessibility

- Full keyboard operation: navigation, Agent switching, decision surfaces and the composer, with a documented shortcut set.
- Visible focus everywhere; no focus traps; the peek surface returns focus to its origin.
- Semantic landmarks and live regions for new committed entries and for state changes that matter; streaming text is announced politely and throttled.
- Contrast enforced by test in both themes.
- Reduced motion honoured.

## Delivery stages

Each stage is a reviewable result with its own acceptance conditions. Stages may merge when they share one change.

### Stage 0 — typed Session contract

Type the server projection, generate browser types, migrate the terminal client, and regenerate the shared fixture. Acceptance: both clients build against generated types; the shared fixture exercises every entry variant; the browser renders reasoning, turn summaries and job completions from committed data, verified by replaying a recorded stream.

### Stage 1 — shell and reading

Replace the application shell: layout, theme and token system with the token page and contrast test, localization infrastructure, connection gate and ticket handoff, Session and Agent navigation with deep links, the turn chapter, tool rows with grouping, and the peek surface with paged output. Acceptance: a recorded Session with long output, mixed tool outcomes and CJK content renders correctly at wide, medium and narrow widths; every entry is addressable; keyboard and screen-reader checks pass on the documented set.

### Stage 2 — authoring and decisions

Composer with draft persistence per Agent, timing selection, queued input listing and withdrawal, the send trail, unconfirmed-command recovery, question and permission surfaces, interruption and continuation, and the attention indicator. Acceptance: the shared command scenarios pass in the browser, including receipt loss, reload recovery, a second tab answering first, and withdrawal racing admission.

### Stage 3 — breadth

Model and reasoning selection with pending-application state, skills and path completion, background jobs, named values, service status and restart guidance, Session deletion and search. Acceptance: each surface has a recorded-stream replay check and a keyboard path.

### Stage 4 — changes review

A Session-level read-only review of file mutations with commit state, aggregated by the server. Acceptance: the aggregation endpoint is specified in the server reference, and the view reads only projected facts.

## Verification

- Protocol: the shared fixture and recorded real streams replay into the reducer, asserting derived view state rather than pixels.
- Presentation: Storybook stories are built from recorded streams, so the component explorer and the regression source are the same artifacts; visual snapshots cover both themes and all three widths.
- Behaviour: the shared command, observation and recovery scenarios run against a real server with a controllable provider, as described in [Interactive clients](interactive-clients.md#testing-during-development).
- Delivery: the installed binary is checked through the existing acceptance entrypoint, including the browser handoff and a session started in the terminal and continued in the browser.
