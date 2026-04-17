# START HERE

This file is the entry point for every Claude Code session.

## First session ever

```
Read CLAUDE.md, then docs/architecture.md, then
docs/phases/phase-00-bootstrap.md. Confirm you understand
the project and the phase-0 scope. Before writing any files,
show me the directory tree you plan to create. Then begin
phase 0.
```

## Every subsequent session

```
Read CLAUDE.md and docs/progress.md to see where we are.
Then read the current phase file in docs/phases/.
Begin that phase.
```

## Phase order

1. `phase-00-bootstrap.md` — workspace, CI, crate skeleton
2. `phase-01-core-types.md` — keplor-core: Event, Usage, Cost, errors
3. `phase-02-pricing.md` — LiteLLM catalog + cost engine
4. `phase-03-storage.md` — SQLite + zstd (no dicts yet)
5. `phase-04-proxy-core.md` — hyper reverse proxy with body tee
6. `phase-05-openai-anthropic.md` — first two providers
7. `phase-06-cli-mvp.md` — CLI + config + Docker + smoke test → v0.1.0-alpha.1
8. `phase-07-remaining-providers.md` — 9 more providers
9. `phase-08-compression.md` — trained dicts + dedup → 30-80× ratios
10. `phase-09-remote-sinks.md` — ClickHouse + S3 + Postgres + OTLP
11. `phase-10-admin-ui.md` — virtual keys, budgets, admin UI
12. `phase-11-hardening.md` — chaos, benchmarks, docs, v1.0.0

## Tips

- One phase per fresh Claude Code session. Don't pile them up.
- Review the phase retrospective in `docs/progress.md` before starting the
  next phase.
- If Claude tries to skip ahead, stop it and redirect to the current phase.
- Human review at every phase boundary. Don't auto-merge.
