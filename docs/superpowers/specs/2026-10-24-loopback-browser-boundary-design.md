# Loopback Browser Boundary - Design (2026-10-24)

> **Status:** spec.
> **Tracks:** the note in `docs/tasks.md` section 7 - "the REST API binds to
> `127.0.0.1:8080` with permissive CORS for the local UI and has no
> authentication; acceptable under the local-only threat model".
> **Epic edge:** the origin policy and its tests are device-free. Proving the
> bundled shells still work on each OS is part of the clean-machine install
> test (`docs/launch-readiness.md`, section 2B5).

## Intent

"Local-only" was true of the **bind address** and false of the **browser**.
The API answers `Access-Control-Allow-Origin: *` on every route, so any page
the user visits can read the topology, start a model download, or drive a chat
completion against the local orchestrator; the telemetry WebSocket has no
origin check at all, and browsers do not apply CORS to WebSocket handshakes, so
that one is readable cross-origin even with a strict CORS layer. A globally
distributed desktop app cannot ship with that hole.

This epic draws the browser boundary: only the bundled desktop UI (and a
localhost dev server) may use the API **from a browser**, while native clients
keep working unchanged. No authentication is added, and nothing here protects
against a hostile program running as the same user - that is a different threat
and a different epic.

## Decisions (D-G1 - D-G4)

### D-G1 - CORS allows the shell and localhost, not the web

A single predicate decides whether an `Origin` is acceptable:

- `tauri://localhost` - the bundled shell on Linux and macOS.
- `http(s)://tauri.localhost` - the bundled shell on Windows and Android.
- `http(s)://localhost[:port]`, `http(s)://127.0.0.1[:port]`, `[::1]` - the
  `tauri dev` server and local tooling. The port is not pinned: Vite moves it
  when 1420 is taken, and a localhost page is not an attack surface CORS can
  protect (a local program can call the API without a browser).

Everything else - including lookalikes such as `http://localhost.evil.com` and
`http://tauri.localhost.evil.com` - is refused. Comparison is on the parsed
host, never a prefix or suffix match.

**No `Origin` header means a native client** (curl, the Android worker, the
in-process tests). Those requests are untouched: CORS is enforced by browsers,
and refusing origin-less requests would break every non-browser caller while
stopping no attacker.

### D-G2 - The layer is narrow and the preflight is honest

`allow_origin(predicate)`, `allow_methods([GET, POST])`,
`allow_headers([CONTENT_TYPE])`, plus `allow_private_network(true)` (the
Private Network Access preflight some Chromium builds send when a page asks for
a loopback address). Nothing else is exposed. A refused origin gets no
`Access-Control-Allow-Origin` header at all, which is what makes the browser
block the response; the request still reaches the handler (as it does in any
CORS misconfiguration), so this is not a substitute for the origin check on
upgrades.

### D-G3 - The telemetry upgrade checks the origin itself

CORS does not apply to WebSocket handshakes, so `ws::handler` inspects the
`Origin` header before upgrading and answers `403` to an origin D-G1 rejects.
A missing `Origin` still upgrades: native test clients and tooling send none.

### D-G4 - The pairing listener stops advertising CORS entirely

`POST /api/pairing/enroll` on `127.0.0.1:8081` exists for the phone, which
reaches it through `adb reverse` with a native HTTP client and sends no
`Origin`. The permissive layer there only made the route reachable from a
browser; the new layer is the same predicate as D-G1, so a web page cannot even
complete the preflight. Session minting was already kept off this listener
(spec 2026-10-20, D-B8).

## Acceptance criteria

1. `GET /v1/models` with `Origin: tauri://localhost` returns that origin in
   `Access-Control-Allow-Origin`; the same holds for `http://tauri.localhost`
   and `http://localhost:1420`.
2. A preflight (`OPTIONS` + `Access-Control-Request-Method: POST`) from
   `https://evil.example` carries no `Access-Control-Allow-Origin`, and a
   non-preflight request from that origin carries none either.
3. A request with no `Origin` is unaffected (`200`).
4. A WebSocket upgrade with `Origin: https://evil.example` is refused with
   `403`; with the shell origin, or with no origin, it upgrades (`101`).
5. The origin parser unit-tests the lookalike hosts and the `null` origin, so
   the boundary cannot regress into a prefix match.

## Out of scope

- Authentication or per-session tokens (the API stays a loopback service; the
  desktop shell and native clients must keep working without a handshake).
- A Content-Security-Policy for the webview (`tauri.conf.json` still says
  `"csp": null`; it needs runtime verification of the shell's inline styles and
  is tracked separately).
- Rate limiting and request size limits.
