# Keyhome agent guide

Keyhome is a Tauri v2 remote desktop app with FIDO2 key authentication over Iroh P2P. The app is built and functional — spikes are done, the core flow works end to end.

## Current focus

The project is past the spike/validation phase. The main work now is **performance optimization** — improving frame rate (FPS), reducing latency, and tuning the H.264 encoding pipeline.

## Key files

- `src-tauri/src/commands.rs` — all backend logic (FIDO2 key derivation, Iroh networking, screen capture, H.264 encoding, input injection)
- `src/ui/index.html` — the entire frontend (single-file UI)

## Tech notes

- **Rust edition 2024** — requires Rust 1.85+.
- **Iroh 1.0 API** — uses the latest Iroh endpoint, protocol, and ticket APIs. Older Iroh 0.x code will not compile.
- Always run `source ~/.cargo/env` before any `cargo` command.
- On Linux with NVIDIA, set `WEBKIT_DISABLE_DMABUF_RENDERER=1` before running or building.

## Working rules

- Use OpenSpec for capability planning before coding.
- Treat `openspec/` artifacts as the source of truth for requirements and scope.
- Do not build a browser-only version; the core architecture is native Tauri + Rust backend.
- Prefer small, verifiable Rust spikes over broad scaffolding when exploring new capabilities.

## Validated assumptions

The highest-risk assumptions have all been validated via spikes:

1. ~~Native YubiKey flows usable from a Tauri backend.~~ ✅ Validated (spikes 002, 003 — ctap-hid-fido2 communicates with Google Titan v2)
2. ~~Safe storage or derivation of Iroh identity/addressing material from a YubiKey.~~ ✅ Validated (spike 004 — Titan hmac-secret → Iroh SecretKey → working endpoint, 6.5ms RTT)
3. ~~Iroh session setup and authenticated protocol binding.~~ ✅ Validated (spikes 001, 007 — Iroh endpoints, ALPN routing, and key-derived identity all work)
4. ~~Remote desktop stream/input latency and OS permissions.~~ ✅ Validated (spikes 005, 006 — frame streaming and input injection over Iroh both functional)

## OpenSpec workflow

- Create/modify proposal, design, tasks, and delta specs under `openspec/changes/<change>/`.
- Validate with `openspec validate --all --strict` before reporting specs as done.
- Do not implement active changes until approved.
