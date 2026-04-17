# Phase 10 — Virtual keys, budgets, and admin UI

**Status:** not started
**Depends on:** phases 6, 7, 9
**Unlocks:** phase 11

## Goal

Value-added layer on top of the observational proxy. Still optional — pure observational mode must keep working.

## Prompt

### 1. Virtual API keys

New tables:
```sql
CREATE TABLE virtual_keys (
  id TEXT PRIMARY KEY,
  user_id TEXT,
  hash BLOB NOT NULL,              -- argon2id
  budget_nanodollars INTEGER,
  spent_nanodollars INTEGER NOT NULL DEFAULT 0,
  model_allowlist TEXT,             -- JSON array
  rate_limit_rpm INTEGER,
  rate_limit_tpm INTEGER,
  expires_at INTEGER,
  created_at INTEGER NOT NULL,
  disabled INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE provider_keys (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  encrypted_key BLOB NOT NULL,      -- ChaCha20-Poly1305 with KEPLOR_MASTER_KEY
  created_at INTEGER NOT NULL,
  rotated_at INTEGER
) STRICT;
```

- Provisioned via `keplor keys create`. Hash stored with argon2id.
- Proxy pre-hook: if `auth.virtual_keys_enabled = true` in config, intercept requests carrying a `sk-keplor-*` prefixed key, look up, enforce budget (reject 402 if exceeded), enforce model allowlist, apply rate limit (token-bucket in a `DashMap`), then **SWAP the Authorization header** with the real provider key from the keystore before forwarding.
- Real keys stored encrypted-at-rest using ChaCha20-Poly1305 with a master key from env `KEPLOR_MASTER_KEY` (32 random bytes).
- `keplor keys rotate --provider openai` swaps the stored real key without invalidating virtual keys.

### 2. Budget enforcement modes (per virtual key)

- `soft`: log + alert, never reject.
- `hard`: reject with HTTP 402 `{error: "budget_exceeded", ...}` formatted to match the provider's own error shape.
- `hard_estimate`: pre-estimate token count via `tiktoken-rs` BEFORE forwarding; reject if `(spent + estimated_cost) > budget`.

### 3. Admin UI (single-file, no SPA, no build step)

- Embedded via `include_str!` for HTML/CSS/JS in `keplor-cli/assets/`.
- Server-rendered with `askama` or `maud`; HTMX for interactivity.
- Pages:
  - `/` — overview: 24h volume, top models, top users, recent errors.
  - `/events` — paginated event list with filters (user, model, route, status, date range). Click → event detail with full request/response (decompressed on-the-fly).
  - `/keys` — virtual key management.
  - `/usage` — charts (sparklines, no JS chart lib — inline SVG).
  - `/settings` — view config, show version, reload.
- Served at `/admin/*`, gated by basic auth configured via `admin.username` + `admin.password_hash` (argon2id) or env equivalents.
- Total asset footprint < 100 KB gzipped.

### 4. Event detail view

- Show parsed, pretty-printed request + response.
- Show raw wire bytes (collapsible) — for SSE, render chunk-by-chunk with timing between chunks.
- "Replay" button re-runs the event against live provider and diffs the response.
- "Export" button: single-event JSONL download for eval pipelines.

### 5. Admin API (programmatic access)

```
GET    /api/v1/events
GET    /api/v1/events/{id}
GET    /api/v1/usage/summary
POST   /api/v1/keys
POST   /api/v1/events/{id}/feedback
```

Feedback schema:
```json
{"score": -1..1, "label": "string", "comment": "string"}
```

Stored in a new `event_feedback` table for later dataset export.

All under `/api/v1/`, bearer-token auth.

### 6. Tests

- End-to-end admin-UI rendering snapshots via `insta`.
- API contract tests.
- Budget-enforcement tests (soft/hard/hard_estimate).
- Virtual-key rotation test.

### 7. Docs

Document in `docs/admin.md`.

## Acceptance criteria

- [ ] Virtual keys fully functional with budget + rate limit
- [ ] Admin UI accessible, renders correctly, < 100 KB gzipped
- [ ] Admin API documented in `docs/admin.md`
- [ ] All tests green
- [ ] Binary size delta from admin UI reported (should be < 500 KB)
