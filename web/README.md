# Dashboard

React + Vite + shadcn/ui (Base UI) dashboard for ilearning-mcp. It talks to the
Rust server through the API described in [`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md).

```sh
pnpm install
pnpm dev      # http://localhost:5173, proxies /api, /auth, /oauth, /.well-known, /mcp and /files to :8080
pnpm build    # type-checks and writes dist/, which the server serves
pnpm lint
```

UI primitives live in `src/components/ui` and are managed with the shadcn CLI
(`pnpm dlx shadcn@latest add <component>`).
