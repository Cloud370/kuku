# Web client

The browser client is a workbench where each Session has its own page and address. It keeps the conversational grammar people already know, and grows a structured work log around it as work accumulates. Several Agents share one transcript: they speak, hand off work and ask questions in the same ordered log, and the reader changes perspective rather than location. This page owns the target design. [Interactive clients](interactive-clients.md) owns the shared client contract and delivery order across terminal and browser.

## Position

| The client is | The client is not |
|---|---|
| A reader and steering surface for Agent work | A chat product with a prompt box |
| A single transcript where several Agents speak, hand off and ask | A dashboard of metrics and charts |
| A place to review diffs, command results and test reports | An editor or file browser |
| Reachable from a shareable URL for any Session, Agent and entry | A replacement for the terminal's editing and keybinding depth |

The browser earns its existence through reading, overview, navigation and recall: long output, diffs, tables and test reports read better here than in a terminal; every Agent is visible in one transcript without a column per Agent; every entry has a stable address; a tab can be left open while work continues on the server. Session usage is available on demand rather than occupying the page. Those capabilities set the scope. Anything that only reproduces terminal editing belongs to the terminal.

## What is retained and what is replaced

The interface is rewritten. These properties of the current application are kept because they encode agreed contracts, not presentation:

- The HTTP and NDJSON Session protocol, including command identity, receipts, replay and observation cursors.
- Session-wide observation: one ordered stream spanning Agents, with the producing Agent on every entry.
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
| Every message names its speaker and its addressee | Per-Agent chat channels or direct-message threads |

The submitted message sits on a tinted surface on the same canvas. That surface is the universal "I spoke" signal, so recognition costs nothing, while the layout stays a work log rather than a bubble stream. Its byline names the recipient, and Agent prose is not placed in a container at all: it is typeset as a document, so a long reply reads as an article rather than as text trapped in a box.

Structure is progressive. A short Session looks like a clean conversation. As turns and tool calls accumulate, chapter headers, compressed work lines and the turn spine appear; every added structure is collapsible, and none of it is required to follow a single exchange.

### Turn chapter

A Turn is the unit of work. Its header carries the turn number, terminal state and tool call count, and remains pinned while its body scrolls. Turns are joined by a continuous vertical spine with a node per turn, which gives the log a reading position and makes a long Session navigable without a tree view. The spine is the only persistent structural ornament; no other container draws a border by default.

The header never counts seconds. An active turn shows its state and tool count; elapsed time and token usage settle when the turn completes, so the page stays still while work is busy. Statistics use tabular numerals and appear at metadata contrast; field order follows the terminal's status line so the two clients teach the same reading order.

### Work, not cards

Tool calls are compressed before they are detailed. In the room, consecutive calls by one Agent within one turn collapse into a single work line: the Agent, a sentence describing the burst, the call count and the duration. A work line is not a card, never occupies the reading column, and carries no decorative chrome. Parallel work is visible as several quiet lines, not as several columns.

Expanding a work line reveals compact rows: what was read, what command ran, which file changed, what the result was. Writes and edits show a path, a bounded diff preview and a line delta. Failures, running calls and calls awaiting authorization stay visibly distinct without shouting. In a lens the same calls appear individually in sequence, because the lens is the place to inspect process.

Detail lives in a peek surface, opened on demand. It shows arguments, the structured outcome, the diff, the command facts or the retained output, with retained output paged by offset. One gesture — click, Enter or Space — opens and closes it, and opening it never moves the reading position.

### Sending

The composer names its target and keeps a draft per Agent. A new Session's identity and the first command's identity are generated before the first send, so a lost reply is retried against the same identities and creates one Session with one delivery. Enter steers a busy Agent; an explicit action queues for the next turn. Draft, unconfirmed send, queued input and admitted input are four distinguishable states of the same message, shown on the message itself rather than in a separate log.

The target follows the view: reading the room leaves the previous target in place, and selecting an Agent's lens makes that Agent the target. The target is never implicit for a message that is not addressed to another Agent.

### Attention and pending work

All work that needs a human surfaces where it arose:

- A waiting question or permission request appears inline in the room, attributed to the Agent asking it, under one warning treatment. The participant strip carries a count per Agent, so one glance answers who is blocked. Answering in one tab settles it everywhere, and other tabs observe the result.
- Input still queued for an Agent sits directly above the composer, with its delivery timing and a withdraw action.
- Unconfirmed commands appear inline under the message they belong to. The send trail — sending, receipt, queued, admitted — is shown in one small line, without explanation.

Because the room carries every ask, a user never needs to visit each Agent to find out who is waiting.

### Reading position

The log follows new output while the user is at the bottom. Scrolling up suspends following and preserves the anchor as new entries and previews arrive. A return-to-latest control sits against the composer, not in a floating corner. The room and each lens keep their own anchor, and each Agent keeps its own draft; switching perspective restores both without issuing a model request.

### Several Agents: the room and the lens

The Session is one transcript, and Agents are its speakers. Every message carries a byline with a speaker and an addressee — `You → Lead`, `Lead → Reviewer`, `Reviewer → you`. Agent-to-agent work is first-class speech in the log, not a copied summary or a reference row to follow elsewhere. Parallel work interleaves in commit order; a burst from an Agent that has not spoken stays one quiet work line, so presence does not cost a column.

The room is the default view. It shows all speech in full, compresses all work, and places every pending decision inline at the point where it arose. Decisions are made in the room because the room is where every Agent is already present.

A lens is a change of perspective, not a destination. Selecting an Agent filters the same transcript to that Agent's complete log: its reasoning, its tool rows and results, and the messages it sent and received. Messages from other Agents appear as quoted context so the lens reads as that Agent's day rather than as an unexplained fragment. The lens is a selector over the same entries, not a second log and not a second subscription. `Escape` returns to the room.

The participant strip replaces the Agent rail: Room first, then each Agent with an identity dot, name, current state and need count. The left rail belongs to Sessions alone, so Sessions and Agents never compete for the same surface. Selecting a participant is navigation of the view, not creation: it never issues a model request.

A side-by-side room and lens for wide screens may be added later; it stays two panes and is never the default. A time-aligned lane view for a Team remains out of scope.

### Layouts

One component set serves every width; layouts change, components do not.

| Width | Shape |
|---|---|
| Wide | Session rail, one bounded room or lens column, and on-demand drawers over the margin |
| Medium | Room or lens column; the Session rail collapses and the participant strip stays above the transcript; drawers overlay |
| Narrow | Single column; Session list and drawers become sheets; the participant strip scrolls horizontally; composer pinned above the keyboard |

There is no Agent rail at any width. The narrow layout is designed rather than deferred: touch targets, safe-area insets and the visual viewport are handled from the start, while polished phone-specific gestures wait for a later stage.

### Addresses

The URL identifies what is being read: workspace, Session, and optionally one Agent's lens or one entry. The room is the default when no view is named. Opening a link restores that view; a shared link to a failed command is as useful as a shared screenshot. Presentation preferences — theme, language, density, collapsed state — stay local and never enter the URL.

### Finding work

A workspace's Sessions are listed in the left rail with a recognizable title, last activity and any waiting Agent, and selecting one navigates rather than loading a preview. Search is locale-aware and covers titles and first prompts. There is no separate dashboard: the list is a navigation aid, not a metric surface.

### Usage on demand

Session facts — elapsed time, input and output tokens, cache hit rate, cache reads, tool calls, failures and per-Agent share — open in a drawer from the Session header. The drawer is hidden by default, overlays rather than narrowing the reading column, and closes back to the same anchor. Values come from projected turn facts; a missing measurement is marked as missing rather than inferred. Cache hit rate uses the requests that report both input tokens and cache-read tokens, and both sums use the same sample.

## Visual system

### Direction

Calm, precise, restrained. One canvas, one reading column, one spine. The only filled surface in the transcript is the user's own message; the only warning-coloured object is a pending decision. Hierarchy comes from size, weight and space, not from shadows or nested borders. The page stays quiet by default so that waiting, failure and success are the only things that stand out.

Both themes share one hue family with the terminal: cool blue-grey neutrals and a teal accent. The light theme is a cool off-white rather than a warm paper, so the two clients read as one product in either theme.

### Attention budget

Attention is the scarce resource, so emphasis is budgeted rather than accumulated. Each screen has one focal object, and everything else recedes by at least one step. The reading order is fixed:

1. A pending decision.
2. The Agent's answer.
3. The user's message.
4. Evidence: a diff, a command result, a test report.
5. Work and reasoning.
6. Turn metadata and chrome.

The turn header, bylines, tool glyphs and composer hints sit at metadata contrast: they are read when sought, not before. Colour is state, not decoration. The accent marks the current target or active focus; warning marks a need; success and error appear only on the row carrying the fact. Agent identity colours are small dots and are never the only signal, because the name is always present. Emphasis does not stack: a pending decision is not also accent-coloured, a user message is not also bold, and a turn header is not also a card.

### Token layers

Colour, type, spacing, radius and motion are defined once and consumed by role. Components never contain a literal colour value.

| Layer | Contents | Consumer |
|---|---|---|
| Primitive | Raw palette ramps, neutral and accent and state hues per theme | Semantic layer only |
| Semantic | Roles such as `surface`, `surface.user`, `panel`, `text.primary`, `text.muted`, `accent`, `state.running`, `state.waiting`, `state.success`, `state.error`, `agent.identity`, `border.subtle`, `code.background` | Every component and story |
| Component | A short list of composed values where a role alone is insufficient, such as the tinted user surface | One component family |

The primitive values are derived from the terminal palette, so a change here is a change to both clients' character. Semantic role names match the terminal's style constants one-for-one, which keeps the two clients describing the same roles instead of inventing vocabulary.

Tokens are plain custom properties on the root element and on the theme attribute and are exposed to the utility layer through its theme directive (Tailwind's `@theme` today), so adjusting a value is a single-file edit and every usage follows. A dev-only token page renders the full palette, type ramp and spacing scale for both themes, and Storybook switches themes with a toolbar control. A unit test asserts contrast for every text-on-surface pair in both themes so that tuning a value cannot silently break readability.

### Typography

Two families: a UI face and a code face. The UI face is a system stack with explicit Chinese and Japanese fallbacks; no web font is required for the interface. Code uses a monospace stack with a CJK-capable fallback so that Chinese inside code fences keeps its alignment. Sizes are a short scale: the Agent's prose is the largest text on the page, the user's message is one step smaller, metadata is one step below that, and chapter headings use size and weight without competing with the answer. The turn header uses the mono face, so it reads as instrumentation rather than as content. Measures stay bounded on wide screens.

### Motion

Two animations only: the streaming caret and the phase indicator. State changes fade. Durations stay short, easing is linear or near-linear, and reduced-motion preference removes movement while keeping state legible.

Layout is stable by construction rather than by animation. The composer grows upward without displacing the log, a work line keeps its height when its state changes, the turn header reserves space for its settled metadata, and metrics use tabular numerals. Nothing ticks: elapsed time changes only when a turn settles. Smoothness is mostly the absence of movement.

## Internationalization

The interface ships in English and Simplified Chinese, with the structure ready for more.

- Messages are extracted at build time from source; a missing translation fails the build rather than falling back silently.
- Formatting of dates, numbers, durations and collation goes through the internationalization library, never through string concatenation. Session search uses locale-aware collation.
- Server errors are localized by code. The client maps known codes to catalogued messages and falls back to the server's text for unknown ones, so a new server error is readable before it is translated.
- User and model content is never translated. Agent names and addresses are user data and stay verbatim. Log text, paths, diffs and code carry automatic direction, and code blocks keep left-to-right flow.
- Typography adapts: taller line height for CJK text, no letter-spacing or uppercasing on headings, and flexible widths instead of fixed ones.

Language is a local preference, not part of the URL.

## Architecture

### Layers

```
SDK facts          CommandExecutionFacts, FileMutationFacts, TurnSummary, AgentProgress
Server projection  typed Session entries: what each event is, who produced it, who it is for
Client view        what each client shows, per viewport and preference
```

The boundary is a rule, not a preference. A derivation belongs to the server when both clients would compute the same answer from the same facts and the answer requires tool or SDK semantics: pairing a tool call with its result, classifying a tool, computing a diff summary, turn numbering, turn statistics, retained output references, message addressees. A derivation belongs to the client when it depends on viewport, time, interaction or preference: grouping thresholds, collapse state, the room's compression, lens filtering, scroll anchors, timers and phase debounce, number styling, breakpoints.

The clients do not share a view layer. The terminal's view layer produces width-aware text rows for a cell buffer, which the browser cannot use; what is worth sharing is meaning, and meaning belongs in the projection.

### Server projection contract

The projection is currently built from untyped JSON in the server's wire layer, and both clients read it by string keys. That is the source of existing drift: the server emits `reasoning_preview` while the browser reads `thinking`, the live `thinking` event carries no content, and turn summaries, job completions and turn numbers are emitted but unused. The first stage replaces this with a typed contract:

- One Rust type per entry variant, serialized as a tagged enum, covering user, assistant, reasoning, tool call and result, question, permission, replies, job completion, turn summary, status and error.
- Every entry carries the Agent that produced it. Messages carry a speaker and an addressee, which is `you` or an Agent address; agent-to-agent delivery preserves the sender and the receiving conversation, so the room can render the byline without guessing.
- Session-wide observation is the room's source: one ordered stream across Agents. A lens is a client-side selector over those entries, not a separate subscription.
- Tool entries carry a classification, a lifecycle state, and the structured facts that the clients need for their default rendering, including diff, command facts, test reports and retained output references.
- Live events carry their kind, payload, reasoning text and speaker, so streaming prose lands in the room under the right byline.
- Every history and live entry carries the turn number and turn summary when known. Turn summaries carry elapsed time, tool counts, input, output and cache-read tokens, so usage is aggregated from projected facts.
- Errors carry a stable code from a documented catalogue.
- TypeScript types are generated from these Rust types. The browser keeps no hand-written protocol types.
- The shared protocol fixture is regenerated from the typed contract and remains the cross-client check.

The terminal client migrates to the same types in the same change, so the contract has two real consumers from the start.

### Client state

The Session view is a pure reducer: `(state, frame) → state`, where a frame is a snapshot, a history page, a committed entry, a live event or a connection transition. It holds one ordered log for the Session, not one log per Agent, so a room stream and every lens share the same source. It is independent of React, deterministic, and replayable, so it is tested by replaying recorded streams and the shared fixture rather than by rendering.

Focus — the room or one Agent — is view state. A lens is a selector that marks entries involving that Agent as its own and entries from other Agents as quoted context. Per-view scroll anchors and per-Agent drafts also stay local. None of them are written back to the server.

React subscribes through an external store. Request-and-response resources — Session list, model catalogue, skills, path completion, service status — use the query layer with caching and cancellation. Durable command state stays a separate store, because it survives navigation and reloads.

Layout, collapse, theme and language preferences are local and per client.

### Streaming and rendering

Markdown is parsed incrementally: closed blocks are rendered once and cached, and only the trailing block is re-parsed as text arrives. Code highlighting loads per language and degrades to plain text. Streaming prose carries its speaker and is appended in order; if the reader is in another view, the participant strip shows the activity instead of stealing the scroll. Long logs are virtualized with stable measured heights and anchored scrolling. A collapsed work line is measured once, so expanding it does not shift the anchor above it. Retained output is fetched by offset and appended, never re-read whole.

### Assets and packaging

The built application is embedded in the unified binary and served from the same origin as the API. A source build without the embedded assets keeps the current fallback behaviour. The development server proxies the API to an explicit local service and prints the address it is proxying to. The build stays reproducible from a lockfile, and its size budget is tracked as part of the release checks, because highlighting grammars and fonts are the parts that grow.

## Accessibility

- Full keyboard operation: room and lens navigation, participant switching, decision surfaces and the composer, with a documented shortcut set. `Escape` returns from a lens to the room.
- Visible focus everywhere; no focus traps; the peek surface returns focus to its origin.
- Semantic landmarks and live regions for new committed entries and for state changes that matter; streaming text is announced politely and throttled; a need count is announced when it changes.
- Byline names are buttons with accessible names, and the participant strip exposes its selected view and need counts rather than relying on colour.
- Contrast enforced by test in both themes.
- Reduced motion honoured.

## Delivery stages

Each stage is a reviewable result with its own acceptance conditions. Stages may merge when they share one change.

### Stage 0 — typed Session contract

Type the server projection, generate browser types, migrate the terminal client, and regenerate the shared fixture. The contract includes the producing Agent on every entry, speaker and addressee on messages, session-wide ordered observation, and usage fields on turn summaries. Acceptance: both clients build against generated types; the shared fixture exercises every entry variant, including an agent-to-agent message; replaying a recorded multi-agent stream into the reducer produces the room's bylines and correct lens membership; the browser renders reasoning, turn summaries and job completions from committed data.

### Stage 1 — shell and reading

Replace the application shell: layout, theme and token system with the token page and contrast test, localization infrastructure, connection gate and ticket handoff, Session rail, participant strip, the room and lens, deep links, the turn chapter, compressed work lines with expansion, and the peek surface with paged output. Acceptance: a recorded multi-agent Session with long output, mixed tool outcomes, cross-agent handoffs and CJK content renders correctly at wide, medium and narrow widths; every entry is addressable; a lens shows incoming messages as context and its own work in sequence; switching views preserves the anchor and the draft without issuing a request; keyboard and screen-reader checks pass on the documented set.

### Stage 2 — authoring and decisions

Composer with a target selector and per-Agent draft persistence, timing selection, queued input listing and withdrawal, the send trail, unconfirmed-command recovery, inline questions and permission requests with the participant need count, interruption and continuation. Acceptance: the shared command scenarios pass in the browser, including receipt loss, reload recovery, a second tab answering first, and withdrawal racing admission; an ask answered in the room settles in the asker's lens and in every tab.

### Stage 3 — breadth

Model and reasoning selection with pending-application state, skills and path completion, background jobs, named values, service status and restart guidance, Session deletion and search. Acceptance: each surface has a recorded-stream replay check and a keyboard path.

### Stage 4 — review and usage

A Session-level read-only review of file mutations with commit state, aggregated by the server, and the on-demand usage drawer: elapsed time, tokens, cache hit rate, cache reads, tool calls, failures and per-Agent share. Acceptance: the aggregation fields are specified in the server reference; both surfaces read only projected facts, stay closed by default, and return focus and scroll to their origin.

## Verification

- Protocol: the shared fixture and recorded real streams replay into the reducer, asserting room order, bylines, addressees, lens membership and derived view state rather than pixels.
- Presentation: Storybook stories are built from recorded streams, so the component explorer and the regression source are the same artifacts; visual snapshots cover the room and a lens in both themes and all three widths.
- Behaviour: the shared command, observation and recovery scenarios run against a real server with a controllable provider, as described in [Interactive clients](interactive-clients.md#testing-during-development). A two-Agent handoff is answered from the room, inspected in the asker's lens, reloaded, and continued in the terminal.
- Delivery: the installed binary is checked through the existing acceptance entrypoint, including the browser handoff and a session started in the terminal and continued in the browser.
