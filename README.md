# FluxGate

FluxGate is a local-first desktop AI gateway for Codex, Claude, and OpenAI-compatible clients. It is built with Tauri, Rust, React, and SQLite.

## Features

- Native endpoints for OpenAI Chat Completions, OpenAI Responses/Codex, and Anthropic Messages
- Local provider credentials, priorities, client targeting, and remote model discovery
- Multi-target model aliases with ordered provider/model queues, per-target enable controls, and automatic legacy alias migration
- Automatic retry, alias-level ordered failover, circuit breaking, and health recovery
- Request rectifier with same-provider retry for unsupported thinking/signature fields, Anthropic thinking budgets, and text-only image fallback
- SSE streaming passthrough for native upstream protocols
- Basic non-streaming request/response conversion from Codex or Claude to OpenAI Chat Completions upstreams
- Request logs, Token and cache accounting, cost estimates, trends, provider statistics, and model statistics
- Codex and Claude client takeover with automatic configuration backups

## Development

Prerequisites: Node.js 20+, Rust 1.77+, and the Tauri 2 platform prerequisites for Windows.

```powershell
npm install
npm run tauri dev
```

The browser-only UI preview uses local demo data:

```powershell
npm run dev
```

Production frontend and Rust checks:

```powershell
npm run build
Set-Location src-tauri
cargo check
```

## Local endpoints

The default base URL is `http://127.0.0.1:18080`. Port `15721` is deliberately avoided because cc-switch commonly occupies it.

| Client | Endpoint |
| --- | --- |
| Codex / Responses | `/v1/responses` |
| Claude Messages | `/v1/messages` |
| OpenAI compatible | `/v1/chat/completions` |
| Health check | `/health` |

When takeover is enabled, FluxGate backs up Codex or Claude configuration beside the original file with a `.fluxgate.bak` suffix. Disabling takeover restores that backup.

## Data

Settings, provider credentials, aliases, and request logs are stored in the operating system application-data directory as `fluxgate.db`. Nothing is synchronized to a remote control plane.
