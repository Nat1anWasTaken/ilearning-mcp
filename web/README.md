# Dashboard

The ilearning-mcp dashboard, built on
[`@nat1an/mcp-backbone-dashboard`](https://github.com/Nat1anWasTaken/mcp-backbone/tree/main/packages/dashboard).
The package provides the shell, sign-in page, connected clients, activity
log, consent screen, connect guide and UI components; this app adds the
iLearning-specific parts:

- `src/login.tsx`, `src/link-steps.tsx`: the two-step iLearning sign-in
  (`POST /auth/ilearning/start`, then `/auth/ilearning/complete`), also used to
  re-link.
- `src/link-health.tsx`: the banner shown when the iLearning link stops working.
- `src/overview/`: the profile and iLearning link cards on the overview.
- `src/icons-page.tsx`: the Icons page, serving `public/icon-files`.

It talks to the Rust server through the API described in
[`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md).

```sh
pnpm install
pnpm dev      # http://localhost:5173, proxies /api, /auth, /oauth, /.well-known, /mcp and /files to :8080 (BACKEND_URL)
pnpm build    # type-checks and writes dist/, which the server serves
pnpm lint
pnpm format   # or format:check
```
