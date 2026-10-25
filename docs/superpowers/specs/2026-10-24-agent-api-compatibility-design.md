# Agent API Compatibility - Design (2026-10-24)

> **Status:** spec.
> **Tracks:** the product gap behind "this app is a local model server for
> agents" - the API is OpenAI-shaped, but an agent cannot edit a file through it.
> **Epic edge:** host-only tool calling is device-free. Distributed tool calling
> is deferred; the cluster keeps serving plain chat.

## Intent

The core already exposes `POST /v1/chat/completions` with streaming and
buffered replies, but its message model is `{role, content}` only. A coding
agent such as opencode sends tool definitions on every turn and edits files
only when the model answers with `tool_calls`. Today those definitions are
silently dropped by serde, so the model never sees them and the agent appears
to "do nothing". This spec defines the smallest OpenAI-compatible surface that
makes a local model usable by an agent, on the host backend only.

## Decisions (D-A1 - D-A7)

### D-A1 - The supported subset is explicit

Requests accept:

- `tools`: an array of function tools (`{type: "function", function: {name,
  description, parameters}}`), passed through unchanged.
- `tool_choice`: `"auto"`, `"none"`, `"required"`, or a named function object.
- `stop`: a string or an array of strings.
- `top_p` and `seed`.

Messages accept:

- `role`, `content` (nullable; `null` becomes `""`), `tool_calls` on assistant
  turns, `tool_call_id` on `tool` turns, and `name`.

Responses expose:

- `message.tool_calls` in buffered replies.
- `choices[0].delta.tool_calls` in streaming replies.
- `finish_reason: "tool_calls"` when the turn produced a call.

Non-goals: `response_format`, multi-modal content, auth, and distributed tool
calls. `parallel_tool_calls` is tolerated but not interpreted.

### D-A2 - Tool requests are host-only, and fail loudly

A request carrying `tools` is served by the host `llama-server`, never by the
cluster. Layer-split workers sample their own tokens today and their ChatML
renderer knows only system/user/assistant; pretending they can call tools would
produce text that looks like a call and is not one.

When no host engine is available, the API returns `400` with a message naming
the missing host engine. A silent text answer is worse: the agent loops,
retries, and the user sees a model that "ignores" its tools.

### D-A3 - Null content is legal; tool calls are not persisted

`content` stays a `String` at the API boundary, with a deserializer that maps
`null` or a missing field to `""`. Assistant tool-call turns are commonly sent
with `content: null`, and rejecting them would break every agent loop.

Assistant text is persisted to SQLite as today. `tool_calls` are not persisted:
the built-in chat does not render them, and the agent resends the full turn on
the next request. This keeps the store schema unchanged.

`GenerationRequest` carries the sampling and tool fields; `GenerationChunk`
carries `tool_calls` and `finish_reason` alongside `content`.

### D-A4 - Streaming mirrors llama-server

The host backend already proxies `llama-server`'s SSE stream. It now forwards
`choices[0].delta.tool_calls` unchanged (index, id, type, function name and
argument fragments) and the backend's `finish_reason`. The final chunk before
`[DONE]` carries the reason; `"stop"` remains the default.

Buffered replies merge argument fragments by `index`, emit the assembled
`message.tool_calls`, and set `finish_reason: "tool_calls"` when at least one
call is present.

### D-A5 - Metrics and sessions are unchanged

Tool-call deltas are tokens, so they count toward `completion_tokens` exactly
like content deltas. Session resolution, title derivation and telemetry are
untouched. The assistant message persisted for a tool-call turn is its text
content (possibly empty).

### D-A6 - Host engine requirements

`llama-server` is spawned with `--jinja` explicitly and a bounded context
(`-c`, default 8192 on the current 8 GB host). opencode's system prompt plus
tool definitions can exceed 8192; on a host with headroom, set
`AI_UPCYCLER_CONTEXT=16384` and a matching client `limit.context`. Tool calling
depends on the model's chat template, not on the API: the tested family is
Qwen2.5-Coder (Q4_K_M). A model whose template cannot render tools returns plain
text, which the agent reports as a failure to call tools.

No authentication is added. The API remains loopback-only.

### D-A7 - The opencode client contract

opencode uses the AI SDK's OpenAI-compatible provider:

- `npm: "@ai-sdk/openai-compatible"`.
- `options.baseURL: "http://127.0.0.1:8080/v1"` (or the port chosen with
  `AI_UPCYCLER_API_PORT`).
- A model id that matches `GET /v1/models`.
- `small_model` set to the same local model, so session titles never reach a
  cloud provider.

Kilo Code speaks the same surface and is expected to work without further
server changes; it is verified after opencode.

### D-A8 - Small-model text calls are promoted, narrowly (2026-10-25)

The 3B Qwen2.5-Coder does not emit the template's `<tool_call>` tags even when
the prompt asks for them. It writes a single JSON object such as
`{"name":"read","arguments":{"path":"README.md"}}` in `content` and stops. The
host backend now promotes that text to a `tool_calls` turn only when all of
these hold:

- the request carries tools and `tool_choice` is not `"none"`;
- the whole content is one JSON object, bare or in a JSON code fence;
- `name` matches a requested tool (or the one named by `tool_choice`);
- `arguments` is a JSON object or a JSON string that itself parses.

Prose, unknown names, multiple calls, and malformed arguments stay text. Native
`tool_calls` from the engine win. Tool requests are buffered until `[DONE]` so
the backend can make this decision; requests without tools and
`tool_choice: "none"` keep the streaming path unchanged. This is a compatibility
shim for small models, not a general tool-calling guarantee: the 3B still
misreads multi-step tasks, and the 7B class remains the practical agent floor.

The 16k context run on the 5.9 GB host used about 2.45 GB for the 3B and left
little headroom. 8192 is still the engine default; opencode users need
`AI_UPCYCLER_CONTEXT=16384` and a matching `limit.context` when the client's
system prompt plus tools exceed 8192.

## Compatibility

- Requests without `tools` behave exactly as before, including cluster routing.
- Clients that do not know the new fields ignore them.
- Old workers are unaffected: they never receive a tool request.
- `AI_UPCYCLER_API_PORT` (already configurable) lets the core and an agent
  server share a machine.

## Testing

- Parser unit tests: content delta, tool-call delta, `finish_reason`, `[DONE]`,
  malformed payloads.
- Message deserialization: `content: null`, missing content, tool-call turn,
  `tool` turn with `tool_call_id`.
- API tests with a fake backend: buffered `message.tool_calls`, streaming
  `delta.tool_calls`, and the 400 when tools are requested without a host
  engine.
- A real opencode run on a scratch repository (manual, recorded in the PR).

## Out of scope

- Distributed tool calling (template rendering and parsing on the split path).
- Authentication and non-loopback binds.
- `response_format`, vision, audio.
- Persisting or rendering tool calls in the built-in chat.
