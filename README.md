# ytcast-open

A lightweight YouTube Music cast receiver written in Rust. Makes a Raspberry Pi (or any Linux device) appear as a cast target in YouTube Music. Tap Cast, see your device, play music through MPD.

**3.8MB RAM idle. 2.4MB single binary. Zero runtime dependencies.**

Audio streaming powered by [sabr-rs](https://github.com/mthwJsmith/sabr-rs), my Rust implementation of YouTube's SABR protocol.

## How it works

```
YouTube Music (phone)
    | SSDP multicast discovery
    v
ytcast-open (single Rust binary)
    | DIAL protocol (port 8008) - device appears in cast menu
    | YouTube Lounge API - long-poll session for play/pause/seek/skip/volume
    | SABR protocol - YouTube's native streaming, built-in via sabr-rs
    v
MPD (Music Player Daemon)
    | ALSA output
    v
speakers
```

1. Phone discovers the device via SSDP/DIAL (same open protocol Chromecast uses for discovery)
2. Phone connects via YouTube's Lounge API (reverse-engineered cast control protocol)
3. Phone sends a video ID
4. Built-in SABR implementation streams audio directly from YouTube's CDN
5. MPD plays the stream. Position, duration, and state sync back to the phone in real-time

No Node.js. No Python. No yt-dlp. Just one binary.

## Stream resolution - SABR

All audio is streamed using YouTube's **SABR (Server Adaptive Bit Rate) protocol**, the same protocol the official YouTube app uses. The SABR implementation is built directly into the binary using [sabr-rs](https://github.com/mthwJsmith/sabr-rs), our Rust port of the protocol.

Unlike direct URL approaches that break when YouTube changes client policies, SABR is YouTube's own native streaming protocol. It handles format negotiation, server redirects, retries, and backoff automatically.

**Under the hood:**
1. InnerTube `/player` call using WEB_REMIX client + credential transfer token (ctt) from the cast session for authenticated access. When casting from YouTube Music, the phone sends a per-video ctt automatically — no setup needed. Falls back to IOS/ANDROID clients without ctt (some tracks may be restricted without authentication)
2. Picks the highest-bitrate Opus audio format
3. Opens a SABR session with YouTube's CDN using protobuf requests
4. Parses UMP (Universal Media Protocol) binary response frames
5. Extracts and streams audio segments as chunked HTTP to MPD
6. Reports buffered ranges so the server sends the next segments

The SABR code adds roughly 1200 lines of Rust across 3 files. Proto definitions (18 `.proto` files) are compiled at build time via `prost-build`. This replaced an earlier Node.js SABR proxy (ytresolve) that used ~84MB RSS on its own.

## Features

- Cast from YouTube Music or YouTube (phone, tablet, browser)
- Play, pause, seek, skip, previous, volume - all controlled from phone
- Auto-advance to next track when a song finishes (MPD idle events, not polling)
- Playlist support with queue updates mid-session
- Phone UI stays fully synced (seekbar, album art, controls)
- Graceful disconnect (stops playback when phone disconnects)
- Best-quality Opus audio (itag 251, up to ~160kbps, 48kHz stereo)
- Authenticated streaming via cast session credential transfer tokens (WEB_REMIX + ctt)

## Requirements

- **MPD** (Music Player Daemon) running and accessible
- **Linux** (tested on Raspberry Pi Zero 2W with Debian trixie/aarch64)
- Network access to YouTube servers

That's it. No Node.js, no Python, no external resolvers.

## Usage

```bash
# Set device name (default: "Living Room Pi")
export DEVICE_NAME="Living Room Pi"

# MPD connection (defaults: localhost:6600)
export MPD_HOST=localhost
export MPD_PORT=6600

# Run
./ytcast-open
```

Open YouTube Music on your phone, tap Cast, and select your device.

## Building

```bash
# Native build (with mock MPD for development on Windows/Mac)
cargo build --features mock-mpd

# Release build
cargo build --release

# Cross-compile for Raspberry Pi (aarch64)
docker run --rm -v "$PWD:/src" -w /src rust:1.89-bookworm bash -c \
  "apt-get update -qq && apt-get install -y -qq gcc-aarch64-linux-gnu protobuf-compiler && \
   rustup target add aarch64-unknown-linux-gnu && \
   CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
   cargo build --release --target aarch64-unknown-linux-gnu"
```

## Architecture

| File | Lines | Purpose |
|------|-------|---------|
| `main.rs` | ~750 | Entry point, player command loop, MPD idle events, auto-advance |
| `lounge.rs` | ~1020 | YouTube Lounge API - session management, streaming long-poll, command parsing |
| `messages.rs` | ~510 | Lounge message parsing + `ChunkParser` for HTTP streaming responses |
| `innertube.rs` | ~150 | Stream resolver - InnerTube /player for metadata (WEB_REMIX+ctt or ANDROID fallback) |
| `mpd.rs` | ~340 | Raw MPD TCP client (command + idle connections) + mock mode for dev |
| `ssdp.rs` | ~100 | SSDP multicast discovery responder |
| `dial.rs` | ~290 | DIAL HTTP server on port 8008 + `/stream/:videoId` endpoint |
| `config.rs` | ~50 | JSON config (UUID, screen_id) load/save |
| `sabr/stream.rs` | ~930 | SABR streaming - InnerTube /player, format selection, SABR request loop, UMP parsing |
| `sabr/ump.rs` | ~320 | UMP binary codec (YouTube's custom varint) + streaming parser + tests |
| `sabr/mod.rs` | ~12 | Module glue + protobuf includes |

Single-threaded tokio runtime. Two MPD TCP connections (command + idle). One long-poll HTTP connection to YouTube Lounge API. SABR requests run per-track in a spawned task.

## RAM usage

RSS = Resident Set Size (actual physical RAM used by the process).

| Component | RAM (RSS) |
|-----------|-----------|
| ytcast-open idle | ~3.8MB |
| ytcast-open during SABR streaming | ~5-8MB |
| MPD | ~10MB |
| **Total system** | **~15-20MB** |

The previous architecture used a Node.js SABR proxy (ytresolve, ~84MB) alongside the Rust binary. That's gone now. All SABR streaming is built into the single binary via [sabr-rs](https://github.com/mthwJsmith/sabr-rs).

## Comparison

| | ytcast-open (current) | Previous (Rust + Node.js ytresolve) | Typical yt-dlp setup |
|-|----------------------|-------------------------------------|---------------------|
| Idle RAM | **~3.8MB** | ~60MB (3.8MB + 55MB Node) | 50-90MB per invocation |
| Binary/install | **2.4MB single binary** | 2.4MB + 2MB node_modules | Python + yt-dlp + JS runtime |
| Runtime deps | **None** | Node.js 18+ | Python + JS runtime |
| Startup | ~50ms | ~2s (Node.js) | 2-5s per video |
| Stream method | SABR (built-in Rust) | SABR (Node.js googlevideo) | yt-dlp sig decryption |
| Future-proof | Yes - YouTube's own protocol | Yes | Breaks frequently |

## Credits

- [sabr-rs](https://github.com/mthwJsmith/sabr-rs) - our Rust SABR implementation (extracted as a standalone crate)
- [googlevideo](https://github.com/LuanRT/googlevideo) by LuanRT - TypeScript SABR reference implementation + proto definitions
- [yt-cast-receiver](https://github.com/patrickkfkan/yt-cast-receiver) by patrickkfkan - Node.js cast receiver framework (protocol reference)
- [plaincast](https://github.com/nickvdp/plaincast) - Go cast receiver (Lounge API reference)
- [ytm-mpd](https://github.com/dgalli1/ytm-mpd) by dgalli1 - original YouTube Cast + MPD bridge

## License

MIT
