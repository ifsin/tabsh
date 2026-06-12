# tabsh

A web-based terminal. Open a shell in your browser, with URL-driven routing and a WebAssembly renderer built on the [Rio](https://rioterm.com) terminal engine.

## Building

Requires: Go, Rust + wasm-pack, Node.js + npm.

```bash
make
```

This chains wasm-pack → Vite → Go and outputs `./tabsh` at the repo root.

```bash
make clean   # remove all build artifacts
```

For frontend iteration only:

```bash
cd client/web && npm run build
cd server && go build -o ../tabsh .
```

## Running

```bash
./tabsh                          # listen on 127.0.0.1:7681
./tabsh -bind 0.0.0.0            # listen on all interfaces
./tabsh -port 8080               # custom port
./tabsh -config path/to/config.json
```

## Configuration

Default config path: `~/.config/tabsh/config.json`

```json
{
  "apps": [
    {
      "id": "zsh",
      "command": "/bin/zsh",
      "args": [],
      "cwd": ""
    }
  ]
}
```
