# ovk-prototype server — Rust + HTMX on the rapids

The backend for the OVK red-team panel prototype. It talks to OVK **exclusively
through OVK's flow queue** (the "rapids") and never modifies the OVK repo:

- **appends** `content:query` entries when a draft is submitted,
- **answers** them with a resident panel listener: fan-out to OVK's 20 personas
  (files read-only; `llm.sh` is the one model seam), atomic `reply:<id>` batch,
  aggregation via OVK's `panel-aggregate.sh` (a pure function), then a
  `prediction:<item>` append,
- **consumes** everything back with its own durable cursor (`ovk-prototype`),
  crash-safe at-least-once, and can **replay the whole UI state from the queue**
  (an empty local DB triggers re-ingestion from origin),
- **records outcomes**: `outcome:<item>` entries in OVK's exact OutcomeRecord
  format, closing the calibration loop; the UI joins predicted vs actual and
  shows the mean signed error,
- **enriches** locally in SQLite: red-team role mapping for OVK's archetypes
  (the Outlaw voice ships locked — score counts, text withheld), EN/ET strings,
  per-run operator notes, per-run engine choice.

The UI is server-rendered HTML swapped by HTMX (vendored, no CDN), bilingual
EN/ET (`?lang=et`, persisted in a cookie).

## Run

```sh
cd server
cargo run
```

Then open http://127.0.0.1:8787. The server discovers the flow daemon socket by
running `ground flow status <queue>` inside the OVK checkout, and starts the
daemon if it isn't running.

## Reply engines

Submissions choose an engine per run (radio in the form):

- **model** (default) — personas answer through OVK's `llm.sh` seam (routed by
  `panel/llm.env`, e.g. the Ollama box). The listener probes reachability per
  run and, if the model is down, degrades to mock **with an honest label**
  ("mock (model unreachable)") — never silently.
- **mock** — deterministic archetype-flavored replies, free, instant.

## Configuration (env, all optional)

| Var | Default | Meaning |
|---|---|---|
| `OVK_ROOT` | `../../ovk` | OVK checkout (read-only from our side) |
| `OVK_QUEUE` | `panel` | flow queue name |
| `OVK_FLOW_SOCKET` | discovered | explicit daemon socket path (skips discovery) |
| `OVK_CURSOR` | `ovk-prototype` | durable cursor of the UI ingester |
| `OVK_LISTENER` | `1` | resident panel listener (`0` disables) |
| `OVK_RUNNER` | `append` | `append`: listener answers · `panel-run`: invoke OVK's one-shot panel-run.sh instead (disable the listener too, or they race) |
| `REPLY_ENGINE` | `model` | default engine when the form doesn't say (`mock` \| `model`) |
| `BIND` | `127.0.0.1:8787` | HTTP bind address |
| `DATA_DIR` | `data` | where the SQLite enrichment store lives |

## Layout

```
src/config.rs    env config
src/flow.rs      native client for the ground flow daemon's Unix-socket
                 newline-JSON protocol; base64 bodies; cursors move by
                 keyword (origin|head|next|prev) so commits walk sequentially
src/consumer.rs  resident rapids ingester; cursor position = last fully
                 processed ordinal; replays from origin when the DB is empty
src/listener.rs  resident panel listener answering content:query entries
src/seeds.rs     archetype → red-team role mapping (EN/ET, Outlaw locked)
src/i18n.rs      EN/ET string tables
src/store.rs     SQLite enrichment store (runs, replies, personas, run_meta)
src/routes.rs    axum handlers + askama templates -> HTMX fragments
templates/       page shells and fragments
static/          vendored htmx
```

## Semantics worth knowing

- Reply→run association is positional at ingest time, then **re-anchored when
  the prediction arrives**: `prediction:<N>` explicitly tags its item, and all
  replies between the previous prediction and this one belong to it. (Handles
  a query answered late, after newer queries already landed.)
- A fresh listener cursor **fast-forwards past the backlog** — the listener
  answers the future, not history — and skips any query that already has a
  prediction (e.g. one served by a manual `panel-run.sh`).
- The listener degrades per run, the ingester replays on demand, and neither
  can wedge on an unknown content type — the cursor always advances.
- macOS caps Unix-socket paths at ~104 bytes; for deeply nested sockets the
  client transparently connects via a short symlink in the OS temp dir. (The
  daemon has the same limit — start it with a short/relative `--dir`.)

## Not here yet (deliberately)

Accounts/multi-tenancy/billing, SSE instead of polling, real stratified
population modeling, media/social-listening inputs, hosted flow hub over TCP.
