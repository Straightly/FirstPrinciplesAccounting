# LedgerZero (FirstPrincipleAccounting)

A first-principles, AI-native accounting platform. Current public reference:

- [`docs/FPA-Architecture-Diagram.md`](docs/FPA-Architecture-Diagram.md) — current runtime and deployment architecture

The `docs/LedgerZero_*` files are retained as historical design and delivery
records. They are not the active project plan or specification and may contain
superseded statements. Current behavior is established by the source and tests;
the private control-plane project holds the single active specification and
plan.

## Layout

- `engine/` — Rust crate: the `AccountingEngine` (invariants, domain, storage boundary)
- `backend/` — Rust crate: routing server + runtime backend (Axum); the only component with storage access
- `frontend/` — React + Vite application shell (identity, books, accounting administration, workflow/role administration); each workflow is deployed as its own self-contained React app
- `mcp_server/` — Python MCP server + dev-time backend (LLM/workflow generation); no accounting storage access
- `scripts/check.sh` — upgrades dependencies, then builds and tests everything

## Getting started

Prerequisites: Rust (rustup.rs), Node.js 24+, Python 3.11+. The local build
bootstraps `uv` and `cargo-upgrade` into an ignored project-local directory
when needed. Local builds need network access to check for new releases.

```bash
./scripts/check.sh                                 # upgrade, build + test all components
cp server.config.example.toml server.config.toml   # then edit
cargo run -p ledgerzero-backend                    # serves http://localhost:8080
```

Every local `check.sh` or `package.sh` run upgrades Rust, npm, and Python
dependencies before building, including new major versions. If an upgrade
breaks the build or should be held back, adjust the manifests and lockfiles
before committing. The GitHub release workflow does not upgrade dependencies:
it builds from the committed `Cargo.lock`, `package-lock.json`, and `uv.lock`.
The frontend build also emits a self-contained React module for generated
workflow artifacts. Build the frontend before generating or deploying a new
workflow artifact.

`server.config.toml` (gitignored) holds the bootstrap owner email and Google
OAuth client credentials (Impl Spec §5.3). For local development without OAuth
credentials, set `[dev_login] enabled = true` — never on a network-reachable
deployment.

Frontend development with hot reload: `cd frontend && npm run dev` (proxies
`/api` to the backend on :8080).

## Persistent local staging on macOS

Deploy a packaged copy outside the source checkout:

```bash
./scripts/local-staging.sh build-deploy
```

This uses the committed dependency lockfiles, installs separate engine,
backend, launcher, and runtime-frontend artifacts under
`~/Deployments/FPA-Staging/components`, preserves configuration/data outside
the replaceable component directories, starts the backend on
`127.0.0.1:8081`, and checks its health. The engine is a separately installed
dynamic library; component manifests enforce engine, backend, storage, and
runtime-frontend API compatibility, including the Rust dynamic-ABI identity,
before replacement. On macOS the staging command uses a staging-scoped
launchd job so the backend survives the deployment shell. The first run creates
`~/Deployments/FPA-Staging/config/server.config.toml`; add the localhost
Google OAuth credentials there before testing sign-in, then restart with:

```bash
~/Deployments/FPA-Staging/deploy/fpa-stage restart
```

The copied `fpa-stage` command also supports `status`, `logs`, `stop`,
`start`, `deploy-component`, `redeploy-component`, and `recover`. Component
deployment deletes/replaces only the selected component. `recover` deletes
all four component directories and reinstalls their recorded artifacts while
leaving `config/`, `data/`, `logs/`, `artifacts/`, and `deploy/` intact.
Workflow SPAs created through the running application live under persistent
`data/generated-workflows`, not in the replaceable packaged-workflow component.
When an engine API boundary changes, use
`fpa-stage deploy-engine-backend <engine-artifact> <backend-artifact>` so the
compatible pair is preflighted and health-gated together; ordinary fixes still
replace only their affected component.

Run the isolated L3 deployment acceptance drill with:

```bash
./scripts/test-component-deployment.sh
```
