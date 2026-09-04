# ecp-demo

A public, read-only web front for [ecp](https://github.com/coseto6125/egent-code-plexus).
Paste a public GitHub repository; the service clones it at depth 1, indexes it
with `ecp admin index`, and then runs the read-only `ecp` subcommands against its
code graph. Every result shows the exact command an agent would type.

The tool list, the JSON schemas and the argv rules come from
`ecp admin mcp tools --format json` at startup, so the page is the same surface
an AI agent gets over MCP, and this crate has no Rust dependency on ecp.

## Run locally

```sh
cargo run --release            # needs `ecp`, `git`, `curl` on PATH
open http://localhost:8080
```

`ecp` must print the full tool list from `ecp admin mcp tools --format json`
(egent-code-plexus PR #749; the next release after 0.13.0).

## Guards

Public endpoint, anonymous callers, so:

- read-only subcommand allowlist (`tools.rs::ALLOWED`);
- `--repo`, `--graph`, `--batch` are server-owned and rejected on the translated argv;
- repo size ceiling checked via the GitHub API before the clone and on the checkout after it; the checkout's `.ecp/` and every symlink are removed before indexing;
- one build at a time, bounded queue, LRU eviction that never touches a repo with a run in flight;
- per-run timeout (SIGKILL), bounded wait for a query slot, output cap, per-IP rate limits keyed on the proxy-appended `x-forwarded-for` hop;
- every `ecp` child runs with a scrubbed environment.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `PORT` | `8080` | listen port |
| `ECP_DEMO_BIN` / `ECP_DEMO_GIT` / `ECP_DEMO_CURL` | `ecp` / `git` / `curl` | programs to spawn |
| `ECP_DEMO_REPOS` | `/data/repos` | where checkouts live |
| `ECP_DEMO_TIMEOUT_SECS` | `15` | per-query wall clock |
| `ECP_DEMO_QUEUE_WAIT_SECS` | `5` | wait for a query slot before 503 |
| `ECP_DEMO_CONCURRENCY` | `2` | concurrent queries |
| `ECP_DEMO_MAX_OUTPUT_BYTES` | `262144` | stdout cap |
| `ECP_DEMO_RATE_PER_MIN` | `60` | queries + list polls per address |
| `ECP_DEMO_ADD_RATE_PER_HOUR` | `10` | repositories added per address |
| `ECP_DEMO_TRUSTED_HOPS` | `1` | `x-forwarded-for` hops appended by trusted proxies (0 = use the socket peer) |
| `ECP_DEMO_MAX_REPO_KB` | `524288` | checkout size ceiling after the depth-1 clone (indexing needs roughly 2–3× the checkout in RAM; on a 512 MB instance, ~50 MB is the practical limit) |
| `ECP_DEMO_MAX_REPOS` | `6` | checkouts kept before LRU eviction |
| `ECP_DEMO_QUEUE_LIMIT` | `3` | builds queued at once |
| `ECP_DEMO_CLONE_TIMEOUT_SECS` / `ECP_DEMO_INDEX_TIMEOUT_SECS` | `120` / `300` | build stage timeouts |
| `GITHUB_TOKEN` | unset | raises the GitHub API quota for the size precheck (never passed to `ecp`) |

## Deploy

`Dockerfile` builds this service and installs `ecp` (a release tarball with
`--build-arg ECP_VERSION=x.y.z`, or a git build with `--build-arg ECP_REF=<ref>`).
`.github/workflows/demo.yml` pushes the image to GHCR on every push to `main`
and pings the live service every 10 minutes. `render.yaml` is a Render
Blueprint on the free plan that pulls that image.

Once: connect the repository in Render (New → Blueprint), set `GITHUB_TOKEN`
in the service, and set the repository variable `ECP_DEMO_URL` (and, optionally,
the secret `RENDER_DEPLOY_HOOK_URL` so each image push redeploys).

## Tests

```sh
cargo test
```

`tests/api.rs` drives the router with stub `ecp`/`git`/`curl` scripts that log
every spawn; `tests/fixtures/tools.json` is the captured tool list.
