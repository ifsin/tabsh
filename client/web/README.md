# tabsh — frontend

TypeScript/Preact frontend for tabsh, built with Vite.

## Development

1. Start the Go server: `./tabsh` (from repo root, listens on `:7681`)
2. Start the dev server: `npm start` (listens on `:9000`, proxies `/_ws` and `/_fav` to `:7681`)

## Production build

```bash
npm run build
```

Bundles everything to `dist/` and copies the output to `../../server/embedded/` so the Go binary can embed it.
