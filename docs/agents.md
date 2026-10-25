# Using AI Upcycler as a local model server for agents

AI Upcycler keeps its built-in chat and also exposes the same host engine as an
OpenAI-compatible endpoint for coding agents. Point opencode, Kilo Code, or any
client that speaks the OpenAI chat-completions format at the loopback API and
the agent can use tools against the model running on your machine.

This guide covers the host-only agent path. The Android cluster still serves
plain chat; distributed tool calls are a later step.

## What you need

- The desktop app running on the host (Windows, macOS, or Linux).
- A tool-capable GGUF and a `llama-server` build that supports `--jinja`
  (the bundled engine does; the app passes the flag explicitly).
- For an 8 GB host: **Qwen2.5-Coder-3B-Instruct Q4_K_M** (about 1.9 GB). A 7B
  Q4 model does not fit alongside the OS on a 5.9 GB usable-RAM laptop; treat
  16 GB as the practical floor for 7B.
- The built-in chat keeps working unchanged. Agents are a second front door,
  not a replacement.

## 1. Get the model

The Models page downloads a GGUF from Hugging Face and registers its model id.
Use the custom download form with:

| Field | Value |
|---|---|
| Repository | `Qwen/Qwen2.5-Coder-3B-Instruct-GGUF` |
| File | `qwen2.5-coder-3b-instruct-q4_k_m.gguf` |

The model id is the file name without `.gguf`:
`qwen2.5-coder-3b-instruct-q4_k_m`. `/v1/models` shows the exact id; use that
string in agent configuration.

The host engine serves **one** GGUF: the file named by `AI_UPCYCLER_MODEL`, or
`qwen2.5-0.5b-instruct-q4_k_m.gguf` in the models directory when the variable
is unset. Point `AI_UPCYCLER_MODEL` at the coder model, otherwise the agent
will talk to the small default model even though the 3B appears in the catalog.

## 2. Start the app with the engine paths

| Variable | Purpose | Default |
|---|---|---|
| `AI_UPCYCLER_MODELS_DIR` | Model library and download destination | The desktop app's data directory `models/` |
| `AI_UPCYCLER_MODEL` | GGUF the host engine loads | `<models dir>/qwen2.5-0.5b-instruct-q4_k_m.gguf` |
| `AI_UPCYCLER_LLAMA_SERVER` | `llama-server` binary | `<parent of models dir>/engine/llama-server[.exe]` |
| `AI_UPCYCLER_API_PORT` | Loopback API port | `8080` |
| `AI_UPCYCLER_CONTEXT` | Context window passed to `llama-server` | `8192` |

The engine's default location is the `engine/` directory next to the models
directory's parent. For example, this layout needs no binary override:

```text
D:\ai-upcycler\
  engine\llama-server.exe
  models\qwen2.5-coder-3b-instruct-q4_k_m.gguf
```

```powershell
$env:AI_UPCYCLER_MODELS_DIR = "D:\ai-upcycler\models"
$env:AI_UPCYCLER_MODEL = "D:\ai-upcycler\models\qwen2.5-coder-3b-instruct-q4_k_m.gguf"
```

If the models directory is somewhere else, set the binary explicitly:

```powershell
$env:AI_UPCYCLER_MODELS_DIR = "D:\ai-upcycler-models"
$env:AI_UPCYCLER_MODEL = "D:\ai-upcycler-models\qwen2.5-coder-3b-instruct-q4_k_m.gguf"
$env:AI_UPCYCLER_LLAMA_SERVER = "E:\AI Upcycler\runtime\engine\llama-server.exe"
```

Start the desktop app after setting these variables; the engine is discovered
at startup. Changing them requires a restart.

The desktop dashboard and built-in chat currently call `127.0.0.1:8080`
directly. If you move the API port, the agent endpoint moves with it, but the
dashboard and chat in this build still expect 8080. Keep the app on 8080 when
you need the built-in UI, or move the conflicting process to another port.

## 3. Verify the endpoint

List the registered models:

```bash
curl http://127.0.0.1:8080/v1/models
```

Send a tool request:

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen2.5-coder-3b-instruct-q4_k_m",
    "messages": [{"role": "user", "content": "Read README.md"}],
    "tools": [{
      "type": "function",
      "function": {
        "name": "read",
        "description": "Read a file",
        "parameters": {
          "type": "object",
          "properties": {"path": {"type": "string"}},
          "required": ["path"]
        }
      }
    }]
  }'
```

A working tool turn has `finish_reason: "tool_calls"` and a `tool_calls` array
on the assistant message. A plain text answer with `finish_reason: "stop"`
means the model did not choose a tool; it is not a server error.

Small models sometimes write the call as a single JSON object in `content`
instead of the native `tool_calls` field. When the request carries tools and
that object has a `name` matching one of the requested tools and an `arguments`
object (or a JSON string that parses), the host backend promotes it to a
`tool_calls` turn. It does not parse prose, unknown tool names, or multiple
calls; those remain text. Tool requests are buffered until the turn ends so the
server can make that decision. Requests without tools and
`tool_choice: "none"` keep streaming unchanged.

For streaming, add `"stream": true`. Tool-call fragments arrive as
`choices[0].delta.tool_calls`, and the last chunk before `[DONE]` carries the
finish reason.

## 4. opencode (V2)

Add a custom provider to `opencode.json` (project) or
`~/.config/opencode/opencode.json` (global):

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "model": "ai-upcycler/qwen2.5-coder-3b-instruct-q4_k_m",
  "providers": {
    "ai-upcycler": {
      "name": "AI Upcycler",
      "package": "@opencode/ai/providers/openai-compatible",
      "settings": {
        "baseURL": "http://127.0.0.1:8080/v1"
      },
      "models": {
        "qwen2.5-coder-3b-instruct-q4_k_m": {
          "modelID": "qwen2.5-coder-3b-instruct-q4_k_m",
          "name": "Qwen2.5 Coder 3B (local)",
          "capabilities": {
            "tools": true,
            "input": ["text"],
            "output": ["text"]
          },
          "limit": {
            "context": 16384,
            "output": 2048
          }
        }
      }
    }
  }
}
```

Then run `/models` and select the model, or start a run with:

```bash
opencode run --model ai-upcycler/qwen2.5-coder-3b-instruct-q4_k_m "Read README.md and summarize it"
```

Set `AI_UPCYCLER_CONTEXT=16384` when you start the app for this config. At
8192, opencode's system prompt plus tool definitions can exceed the window and
`llama-server` returns HTTP 400. On the 5.9 GB usable-RAM test host, the 3B at
16k used about 2.45 GB and left little headroom; close other apps. For the
built-in chat only, 8192 is the safer default.

The endpoint needs no API key on loopback. Do not add one; the app does not
authenticate requests. The `package` path above is the V2 runtime package. V1
configurations used `npm` and `options`; migrate them before using this guide.

If the API port is not 8080, change both `AI_UPCYCLER_API_PORT` and
`settings.baseURL`.

## 5. Kilo Code

Kilo Code can use the same OpenAI-compatible endpoint. Choose its
OpenAI-compatible provider, set the base URL to
`http://127.0.0.1:8080/v1`, and use the model id from `/v1/models`. Tool
support depends on the client sending the standard `tools` field and handling
`tool_calls`; the server side is the same contract as opencode.

## 6. What "host-only" means

A request that carries a non-empty `tools` array is served by the host
`llama-server` and never by the cluster. The cluster's ChatML renderer knows
only system/user/assistant messages, so pretending it could call tools would
produce text that looks like a call and is not one.

When no host engine is available, the API returns `400` with a message naming
the missing host engine. The request is refused before a session or user turn
is stored, so a failed agent loop does not leave half-written conversations
behind.

Plain chat requests still use the cluster when one is available; the built-in
chat UI is unchanged.

## 7. Troubleshooting

| Symptom | Check |
|---|---|
| `400` mentioning the host engine | Set `AI_UPCYCLER_MODEL` and `AI_UPCYCLER_LLAMA_SERVER` (or use the default `engine/` layout), then restart. Confirm both files exist. |
| `404` for the model | The request id must match an id from `/v1/models`. The id is the file name without `.gguf`. |
| Text instead of `tool_calls` | If the whole answer is one JSON object whose `name` matches a requested tool, the host backend promotes it. Prose, unknown names, and multiple calls stay text. Keep `--jinja` enabled, use a tool-capable model, and check `finish_reason`. The 0.5B default model is not a reliable tool caller. |
| `400` on an agent turn | The prompt plus tools exceeded the context. Set `AI_UPCYCLER_CONTEXT` and `limit.context` to 16384, or lower `limit.output` / shorten the task. |
| Tool arguments are cut off | The engine stopped at the output or context limit. Raise `limit.output` / `AI_UPCYCLER_CONTEXT`, or ask for a smaller edit. |
| Port 8080 is already in use | Set `AI_UPCYCLER_API_PORT` to a free port and update the agent's base URL. |
| Very slow or out of memory | Use the 3B Q4 model on an 8 GB host, close other apps, and keep the context at 8192 for plain chat. opencode may need 16384 for its prompt plus tools; that leaves less headroom. A 7B Q4 model needs about 16 GB of usable RAM. |
| Streaming stops after a tool call | Check the final chunk's `finish_reason`. A `length` reason means the model was truncated; the client should not execute a partial call. |

## 8. Limits and safety

- The API binds to `127.0.0.1` only and has no authentication. Do not expose
  the port through a proxy, tunnel, or firewall rule. Anyone who can reach it
  can use your model and, through an agent, your files.
- One host engine serves one GGUF in this phase. There is no in-app switch
  between the host model and the cluster's active model.
- Distributed tool calling is not implemented. Cluster workers serve plain
  chat; tool requests stay on the host.
- `response_format`, vision, audio, and multiple choices are out of scope.
- The JSON-call promotion is a compatibility shim for small models. It only
  accepts one JSON object whose name matches a requested tool; it is not a
  general tool-calling guarantee. Tool requests are buffered, so their text is
  not token-streamed.
- A 3B model is useful for single-file, scoped edits. Multi-file refactors can
  loop or hallucinate. The JSON-call promotion covers simple single-tool turns;
  the 7B class is the practical agent floor, but it needs a 16 GB host.
- Performance numbers on this page are deliberately absent: they depend on the
  CPU, the quantisation, and the context. Measure on your own machine before
  relying on them.

## Related documents

- `docs/api.md` section 2.3 for the endpoint surface.
- `docs/superpowers/specs/2026-10-24-agent-api-compatibility-design.md` for the
  request/response contract and the host-only decision.
- `docs/launch-readiness.md` for the remaining distribution and hardware work.
- `docs/tasks.md` section 6 for the on-device verification checklist.
