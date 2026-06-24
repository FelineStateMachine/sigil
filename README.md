# keyhome

A native remote desktop app secured by FIDO2 hardware keys. Plug in a Google Titan or YubiKey, tap it, and connect to your home machine over an end-to-end encrypted Iroh tunnel. No passwords, no addresses shared — the key derives the Iroh identity.

## Status

Early prototype. Core functionality works (identity derivation, streaming, input). Current frame rate is ~28 fps with ffmpeg+NVENC (was ~1–2 fps with xcap+openh264 software encoding). The bottleneck has moved from host-side encoding to network/relay throughput and client-side decode.

Codec and encoder backend are configurable in the info panel — H.264, H.265, and AV1 are all supported, with backends for NVENC, VAAPI, QSV, AMF, VideoToolbox, and software. The frontend uses WebCodecs for hardware-accelerated decode of all three codecs.

## How it works

You plug in a FIDO2 security key (Google Titan, YubiKey) and tap it. The key produces a `hmac-secret` that becomes the 32-byte seed for an Iroh `SecretKey`. Both the host and client derive the same Iroh node ID from the same key — no addresses are exchanged, no passwords are stored.

1. **One-time registration.** Tap the key → `hmac-secret` → 32 bytes → OS keyring.
2. **Host starts.** Read from keyring → Iroh `SecretKey` → `Endpoint`.
3. **Client connects.** Tap the key → derive the host's node ID → dial via relay.

The connection is end-to-end encrypted by Iroh. The key never leaves the hardware token; only the derived secret is persisted in the OS keyring.

## Architecture

```
+--------------------------- keyhome (Tauri v2) ---------------------------+
|                                                                         |
|  Frontend (vanilla JS, no npm)                                          |
|  - connection state, video canvas, input capture                        |
|  - WebCodecs decode (H.264/H.265/AV1, hardware-accelerated)             |
|                                                                         |
|  Tauri IPC                                                              |
|                                                                         |
|  Rust backend                                                           |
|  - FIDO2 CTAP via ctap-hid-fido2       - Screen capture+encode: ffmpeg  |
|  - Iroh Endpoint (P2P, relay, E2EE)      (NVENC/VAAPI/QSV/AMF/VT)       |
|  - OS keyring for identity             - Fallback: xcap + openh264      |
|  - Input injection via enigo           - Codec: H.264/H.265/AV1         |
|                                                                         |
+----------------------------------+--------------------------------------+
                                   |
                                   | Iroh (E2EE, relay-assisted)
                                   v
+------------------------------ Host -------------------------------------+
|  - Iroh Endpoint (identity from keyring)                                |
|  - ffmpeg screen capture → H.264/H.265/AV1 encode (hardware)            |
|  - Fallback: xcap capture → openh264 software encode                    |
|  - enigo mouse/keyboard injection                                       |
+-------------------------------------------------------------------------+
```

| Layer | Crate / Tool |
|---|---|
| App shell | Tauri v2 |
| Backend | Rust |
| P2P transport | Iroh (relay-assisted, end-to-end encrypted) |
| Hardware auth | FIDO2 CTAP via `ctap-hid-fido2` |
| Screen capture + encoding | `ffmpeg` subprocess (NVENC/VAAPI/QSV/AMF/VideoToolbox) |
| Fallback capture + encoding | `xcap` + `openh264` (software) |
| Video codecs | H.264, H.265, AV1 (configurable) |
| Client-side decode | WebCodecs (hardware-accelerated, all three codecs) |
| Input injection | `enigo` |
| Identity storage | OS keyring |
| Frontend | Vanilla JS (no npm, no build step) |

## Spikes

Seven spikes were completed as viability evidence. They live in `spikes/`.

| # | Spike | What it proved |
|---|---|---|
| 1 | `001-iroh-native-ping` | Iroh connectivity between two native endpoints |
| 2 | `002-yubikey-hmac-detection` | YubiKey HMAC-Secret detection and invocation |
| 3 | `003-fido2-hid-enumeration` | FIDO2 HID device enumeration via `ctap-hid-fido2` |
| 4 | `004-hmac-iroh-derivation` | HMAC-Secret → Iroh `SecretKey` derivation |
| 5 | `005-frame-stream` | Frame streaming over Iroh |
| 6 | `006-input-injection` | Mouse/keyboard injection via `enigo` |
| 7 | `007-titan-no-copy` | Titan key derives identity without copying secrets off-device |

## Requirements

- **OS:** Linux, macOS, or Windows
- **Hardware:** A FIDO2 security key (Google Titan, YubiKey)
- **System dependencies:** See [docs/SETUP.md](docs/SETUP.md)

## License

MIT
