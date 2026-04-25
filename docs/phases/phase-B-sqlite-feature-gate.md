# Phase B — SQLite feature-gate + docs cutover

Status: **complete.** Dated 2026-04-25.

## Scope

Phase A landed the KeplorDB cutover but left `rusqlite` linked into
every build because `keplor migrate-from-sqlite` still needed
`SqliteStore` as a read source. Phase B finishes the job by gating the
legacy SQLite path behind an opt-in Cargo feature so default release
builds drop the dep entirely, then refreshing the storage docs to
match the new layout.

## What changed

### Type extraction (prep)

The shared row DTOs (`ArchiveManifest`, `GcStats`, `EventSummary`,
`QuotaSummary`, `RollupRow`, `AggregateRow`) were defined inside
`crates/keplor-store/src/store.rs` (the SQLite impl) but used on every
runtime path through `KdbStore`. Moved them to a new
`crates/keplor-store/src/types.rs` so the SQLite module can be
compiled out without those types disappearing from the public API.

`kdb_store.rs`, `kdb_store/manifests.rs`, and `archive.rs` now import
from `crate::types::*`. No behavioural change.

### Feature gates

`crates/keplor-store/Cargo.toml`:

```toml
[features]
default              = ["batch"]
batch                = ["dep:tokio", "dep:metrics"]
s3                   = ["dep:object_store", "dep:zstd"]
migrate-from-sqlite  = ["dep:rusqlite"]   # NEW

[dependencies]
rusqlite = { workspace = true, optional = true }   # was unconditional
```

`#[cfg(feature = "migrate-from-sqlite")]` gates added in
`crates/keplor-store/src/lib.rs` for:

- `mod migrations`
- `pub mod store`
- `pub use store::Store as SqliteStore`

And in `crates/keplor-store/src/error.rs`:

- `StoreError::Sqlite(#[from] rusqlite::Error)` — only present with the
  feature on.

`crates/keplor-cli/Cargo.toml`:

```toml
[features]
migrate-from-sqlite  = ["keplor-store/migrate-from-sqlite"]   # NEW
```

In `crates/keplor-cli/src/main.rs` the gate covers:

- The `Cli::MigrateFromSqlite` clap variant
- The dispatch arm in `main()`
- `fn migrate_from_sqlite`, `fn run_migration`, `struct MigrateStats`,
  `fn read_checkpoint`, `fn write_checkpoint`
- `mod migration_tests` (combined `#[cfg(all(test, feature = "..."))]`)

### Verification

| Build | `rusqlite` linked | Binary size (release) |
|--|--|--|
| `cargo build -p keplor-cli --release` (default) | no | **9.91 MB** |
| `cargo build -p keplor-cli --release --features migrate-from-sqlite` | yes | 11.87 MB |

Default build is back under the CLAUDE.md "<10 MB" target.
`cargo tree -p keplor-cli` shows zero `rusqlite`/`libsqlite3-sys`
entries on the default build, both appear with the feature on.

`cargo clippy --workspace --all-targets -- -D warnings` clean both
ways. `cargo test -p keplor-store -p keplor-cli` passes both ways
(33 default tests, 49 with the feature on — the extra 16 are the
SQLite migrations + store unit tests + the migration end-to-end
tests in `keplor-cli`).

### Docs

- `docs/architecture.md` — replaced the SQLite-centric "Database
  schema" section with a KeplorDB layout description (per-tier engines,
  WAL shards, segments, manifests sidecar) and added a paragraph on
  the feature-gated migration path. Crate-layout one-liner updated.
- `docs/operations.md` — backup/restore section rewritten for the
  data-directory layout (filesystem snapshot / stop-and-tar /
  rsync-after-checkpoint), upgrade flow updated, troubleshooting table
  refreshed, pre-deploy checklist now warns against shipping the
  `migrate-from-sqlite` feature in production runtime builds.

## Compatibility notes

- The on-disk layout did not change in Phase B — only build-time
  surface. Existing KeplorDB data dirs work unchanged.
- Operators upgrading from a SQLite-era release must build the
  migration binary explicitly:
  `cargo build --release -p keplor-cli --features migrate-from-sqlite`.
  After the one-shot `keplor migrate-from-sqlite` import, redeploy the
  default (smaller) binary.
- The legacy SQLite store still passes its own integration tests when
  the feature is on, so it remains a viable read source indefinitely.

## Out of scope

- Transparent merge of KeplorDB segments (still a keplordb-side
  open task).
- Removing the `body_*` fields from `LlmEvent` (deferred from the
  pre-Phase-A blob removal — see
  `MEMORY/project_blob_removal_archival.md`).
