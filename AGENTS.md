# AGENTS.md

## Project Overview

**tabsh** is a web-based terminal. A Go server handles HTTP and WebSocket connections and manages PTYs. A Rust/WASM crate (built on the Rio terminal engine) does the terminal rendering in the browser via a canvas. A Preact frontend ties it together.

## Structure

```
server/          Go backend — HTTP server, WebSocket handler, PTY lifecycle
client/
  wasm/          Rust crate compiled to WebAssembly via wasm-pack
  web/           TypeScript/Preact frontend, built with Vite
vendor/          Vendored Rust crates (Rio terminal engine: sugarloaf, rio-backend, etc.)
shims/           Shell init scripts (zshrc, bashrc, fish, etc.)
```

## Essential Commands

```bash
make             # full build: wasm-pack → Vite → go build → ./tabsh
make clean       # remove all build artifacts

cd client/web
npm run build    # frontend only (also copies output to server/embedded/)
npm start        # dev server on :9000, proxies /_ws and /_fav to :7681

./tabsh                   # run server on 127.0.0.1:7681
./tabsh -bind 0.0.0.0     # expose on all interfaces
```

## Architecture

### URL routing

- `/:appId/:cwd?cmd=...` — SPA route; appId selects the shell, cwd is the working directory, cmd runs on connect
- Any path not prefixed with `/_` falls through to index.html (SPA fallback)

### WebSocket protocol (binary frames, 1-byte type prefix)

**Client → Server**

| Byte | Meaning |
|------|---------|
| `0x00` | PTY input |
| `0x01` | Resize (JSON: `{cols, rows}`) |
| `0x02` | INIT (JSON: `{sessionId, cols, rows, cwd, appId, cmd}`) |
| `0x03` | QUIT |
| `0x04` | CLEAR |

**Server → Client**

| Byte | Meaning |
|------|---------|
| `0x00` | PTY output |
| `0x01` | REATTACHED |
| `0x02` | STATE (JSON: title, cmd, cwd, favicon, or error) |

### Build pipeline

1. `wasm-pack build client/wasm` → outputs JS + WASM to `client/web/src/wasm/`
2. `vite build` in `client/web/` → bundles TS/SCSS, outputs to `client/web/dist/`
3. Vite's embed plugin copies `dist/` → `server/embedded/` and `src/favicon.png` → `server/embedded/default.ico`
4. `go build` in `server/` embeds `server/embedded/` via `//go:embed` and compiles to `./tabsh`

### Frontend → WASM interface

The WASM crate exports three symbols used by the frontend:
- `init()` — initializes the WASM module
- `init_terminal(canvas, cols, rows)` → `TabshTerminal`
- `TabshTerminal` methods: `on_pty_data`, `on_key`, `resize`, `resize_message`, `pop_title`, `pop_cwd`, `pop_bell`
