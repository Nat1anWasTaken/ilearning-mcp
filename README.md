# ilearning-mcp

A remote [MCP](https://modelcontextprotocol.io) server that lets AI agents
(claude.ai, ChatGPT, Claude Code, …) use **CYCU i-learning 2.0** (Moodle) on
behalf of each signed-in student.

- **Sign in** to the dashboard with your iLearning account: open the iLearning
  mobile-app launch page, sign in with Microsoft 365, and paste back the
  「若應用程式沒有自動開啟，請點選這裡」 link. Your iLearning account is your
  dashboard account; the token is stored encrypted and renewed automatically
  around its expiry when possible.
- **Connect** an AI client by adding `https://<host>/mcp` as a custom connector.
  It authenticates with OAuth 2.1 (dynamic client registration or client ID
  metadata documents, PKCE), and you approve it on a consent screen.
- **Use** courses, course contents and files (PDF, Word, PowerPoint, Excel
  text extraction), deadlines, assignments and submission, grades, forums,
  notifications and messages. Write tools are annotated so clients confirm
  them with you.
- **Audit** every tool call, and rename or disconnect connected clients.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design and the
dashboard API contract.

## Development

Requirements: Rust (the pinned toolchain in `rust-toolchain.toml` installs
itself), Node 22 with pnpm, Docker.

```sh
docker compose -f docker-compose.dev.yml up -d   # Postgres on localhost:5443
cp .env.example .env                             # then fill it in, see below
cargo run -p ilearning-mcp                       # API + MCP on :8080, migrations run at startup
cd web && pnpm install && pnpm dev               # dashboard on :5173, proxies to :8080
```

Set `PUBLIC_URL=http://localhost:5173` while using the Vite dev server, so that
OAuth redirects and cookies stay on one origin.

Tests (the integration tests use the dev database from `DATABASE_URL`; none of
them contact iLearning):

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd web && pnpm lint && pnpm build
```

### Configuration

| Variable | Purpose |
| --- | --- |
| `PUBLIC_URL` | External origin. OAuth metadata, the MCP resource URL and file download links derive from it. |
| `DATABASE_URL` | Postgres connection string. |
| `ENCRYPTION_KEY` | 32 bytes, base64 (`openssl rand -base64 32`). It encrypts iLearning tokens at rest and signs file download links. |
| `ILEARNING_URL` | The Moodle site (default `https://ilearning.cycu.edu.tw`). |
| `ALLOWED_MOODLE_USERNAMES` | Comma-separated Moodle usernames allowed to sign in. Leave it empty to allow anyone with an iLearning account. |
| `TOKEN_DURATION_DAYS` | Assumed lifetime of a fresh iLearning token (default 84, Moodle's default). |
| `BIND_ADDR`, `WEB_DIST` | Listen address (default `0.0.0.0:8080`) and dashboard build directory (default `web/dist`). |

## Deployment

The production stack is `docker-compose.yml`: the app image (the multi-stage
`Dockerfile` builds both the dashboard and the server) plus Postgres 17. It is
deployed as a Dokploy compose service built from this repository.

1. Put the variables above, plus `POSTGRES_PASSWORD`, in the service's
   environment. `DATABASE_URL` is derived from `POSTGRES_PASSWORD` in the
   compose file.
2. Point a domain at service `app`, port `8080`. TLS terminates at the
   Cloudflare tunnel in front of Traefik.
3. In claude.ai, open Settings, then Connectors, then Add custom connector.
   Enter `https://<host>/mcp` and leave the OAuth fields empty. In ChatGPT,
   add it as a connector in developer mode.

### Token renewal

iLearning tokens expire (Moodle's default is 12 weeks). When the expiry is
known, the server renews the token shortly before it: it opens an iLearning
web session with the private token, which signs you out of iLearning in your
browser once, since the site allows one web login at a time. When renewal is
not possible, the dashboard asks you to re-link.
