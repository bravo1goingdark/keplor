<script lang="ts">
  import { base } from '$app/paths';
</script>

<svelte:head>
  <title>Configuration - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">Configuration</h1>
<p class="text-lg text-text-muted mb-8">TOML config file with environment variable overrides.</p>

<h2 id="config-file">Config file</h2>
<p>Keplor looks for <code>keplor.toml</code> by default. Override with <code>--config</code>:</p>
<pre><code>$ keplor run --config /etc/keplor/keplor.toml</code></pre>

<h2 id="example">Full example</h2>
<pre><code># keplor.toml

[server]
listen_addr = "0.0.0.0:8080"
shutdown_timeout_secs = 25
request_timeout_secs = 30
max_connections = 10000

[storage]
data_dir = "/var/lib/keplor"        # KeplorDB data directory
retention_days = 90                 # legacy global GC (prefer [retention] tiers)
gc_interval_secs = 3600             # how often GC runs (0 = disabled)
wal_checkpoint_secs = 300           # rotate active WAL into a sealed segment
max_db_size_mb = 0                  # cap data dir size (0 = unlimited)
size_check_interval_ms = 1000       # cache for db_size_bytes (0 = disabled)
wal_max_events = 500000             # events per WAL shard before forced rotation
wal_sync_interval = 64              # fsync every N batched writes
wal_sync_bytes = 262144             # fsync after N buffered bytes
wal_shard_count = 4                 # per-tier WAL shards (raise on >8 cores)
mmap_cache_capacity = 256           # mmap'd-segment LRU size
rollup_replay_days = 7              # days of segments replayed on open
rollup_loop_secs = 60               # rollup-refresh cadence

[auth]
api_keys = ["prod-svc:sk-prod-abc123", "staging:sk-staging-def456"]

# Extended format with tier assignment:
# [[auth.api_key_entries]]
# id = "pro-user"
# secret = "sk-pro-key"
# tier = "pro"

[retention]
default_tier = "free"

[[retention.tiers]]
name = "free"
days = 7

[[retention.tiers]]
name = "pro"
days = 90

# [[retention.tiers]]
# name = "team"
# days = 180

[cors]
allowed_origins = []     # empty = same-origin only; ["*"] = allow all

[pipeline]
batch_size = 64
max_body_bytes = 10485760
channel_capacity = 32768
flush_interval_ms = 50              # BatchWriter cadence (1-10000)
write_timeout_secs = 10             # max wait per durable write (1-300)

[idempotency]
enabled = true
ttl_secs = 300
max_entries = 100000

[rate_limit]
enabled = false
requests_per_second = 100.0
burst = 200

[pricing]
refresh_interval_secs = 86400       # 24h. set 0 to disable. range 60-604800.
# source_url = "..."                # override only when mirroring through a CDN

# Optional: archive old events to S3/R2 (requires --features s3)
# [archive]
# bucket = "keplor-archive"
# endpoint = "https://&lt;account-id&gt;.r2.cloudflarestorage.com"
# region = "auto"
# access_key_id = "..."
# secret_access_key = "..."
# prefix = "events"
# archive_after_days = 30
# archive_threshold_mb = 500
# archive_batch_size = 10000

# [tls]
# cert_path = "/etc/keplor/cert.pem"
# key_path = "/etc/keplor/key.pem"</code></pre>

<h2 id="server">[server]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>listen_addr</code></td><td>string</td><td><code>"0.0.0.0:8080"</code></td><td>Bind address</td></tr>
    <tr><td><code>shutdown_timeout_secs</code></td><td>u64</td><td><code>25</code></td><td>Graceful shutdown timeout (drain BatchWriter + WAL checkpoint). Raise to 60+ for sustained 20K+ rps deployments.</td></tr>
    <tr><td><code>request_timeout_secs</code></td><td>u64</td><td><code>30</code></td><td>Per-request timeout (range 1&ndash;300; returns 408 on expiry)</td></tr>
    <tr><td><code>max_connections</code></td><td>usize</td><td><code>10000</code></td><td>Maximum concurrent connections. <code>/health</code> and <code>/metrics</code> bypass this so observability survives saturation.</td></tr>
  </tbody>
</table>

<h2 id="storage">[storage]</h2>
<p>Storage is a KeplorDB data directory &mdash; one append-only segment tree per retention tier under <code>{`{data_dir}/{tier}/`}</code>.</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>data_dir</code></td><td>string</td><td><code>"./keplor_data"</code></td><td>KeplorDB data directory</td></tr>
    <tr><td><code>retention_days</code></td><td>u64</td><td><code>90</code></td><td>Legacy global GC threshold (0 = disabled). Prefer <code>[retention]</code> tiers.</td></tr>
    <tr><td><code>gc_interval_secs</code></td><td>u64</td><td><code>3600</code></td><td>How often GC runs in seconds (0 = disabled). Segment-granular.</td></tr>
    <tr><td><code>wal_checkpoint_secs</code></td><td>u64</td><td><code>300</code></td><td>Rotate active WAL into a sealed segment (0 = disabled)</td></tr>
    <tr><td><code>max_db_size_mb</code></td><td>u64</td><td><code>0</code></td><td>Cap data dir size in MB (0 = unlimited). Returns HTTP 507 when exceeded.</td></tr>
    <tr><td><code>size_check_interval_ms</code></td><td>u64</td><td><code>1000</code></td><td>How long the cached <code>db_size_bytes</code> result is reused before walking every per-tier engine. Range 0&ndash;60000 (0 = disable cache).</td></tr>
    <tr><td><code>wal_max_events</code></td><td>u32</td><td><code>500000</code></td><td>Events per WAL shard before forced rotation</td></tr>
    <tr><td><code>wal_sync_interval</code></td><td>u32</td><td><code>64</code></td><td>fsync every N batched writes</td></tr>
    <tr><td><code>wal_sync_bytes</code></td><td>u64</td><td><code>262144</code></td><td>fsync after N buffered bytes</td></tr>
    <tr><td><code>wal_shard_count</code></td><td>usize</td><td><code>4</code></td><td>Per-tier WAL shard count (range 1&ndash;64; raise on machines with &gt;8 cores)</td></tr>
    <tr><td><code>mmap_cache_capacity</code></td><td>usize</td><td><code>256</code></td><td>Mmap&rsquo;d-segment LRU size</td></tr>
    <tr><td><code>rollup_replay_days</code></td><td>u32</td><td><code>7</code></td><td>Days of historical segments replayed into the in-memory rollup store on open</td></tr>
    <tr><td><code>rollup_loop_secs</code></td><td>u64</td><td><code>60</code></td><td>Cadence of the in-process rollup-refresh loop (range 5&ndash;3600)</td></tr>
  </tbody>
</table>

<h2 id="auth">[auth]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>api_keys</code></td><td>string[]</td><td><code>[]</code></td><td>Simple API keys (<code>id:secret</code> or bare secret). Assigned to <code>default_tier</code>. Empty = open access.</td></tr>
    <tr><td><code>api_key_entries</code></td><td>object[]</td><td><code>[]</code></td><td>Extended keys with explicit tier: <code>{"{ id, secret, tier }"}</code></td></tr>
  </tbody>
</table>

<h2 id="retention">[retention]</h2>
<p>Per-tier retention policies. GC runs one pass per tier, dropping segments whose events are entirely older than the tier&rsquo;s <code>days</code> threshold.</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>default_tier</code></td><td>string</td><td><code>"free"</code></td><td>Tier assigned to simple-format keys and unauthenticated requests</td></tr>
    <tr><td><code>tiers</code></td><td>object[]</td><td>free (7d), pro (90d)</td><td>Named tiers with retention days. <code>days = 0</code> keeps events forever.</td></tr>
  </tbody>
</table>
<p>Tiers are just names &mdash; add <code>"team"</code>, <code>"enterprise"</code>, or any custom tier via config. No code changes needed.</p>

<h2 id="archive">[archive] (optional, --features s3)</h2>
<p>Archive old events to an S3-compatible object store (Cloudflare R2, MinIO, AWS S3) as zstd-compressed JSONL files. Archived events are tombstoned in KeplorDB; segment GC reclaims their disk space on the next sweep. Daily rollups are preserved.</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>bucket</code></td><td>string</td><td></td><td>S3 bucket name</td></tr>
    <tr><td><code>endpoint</code></td><td>string</td><td></td><td>S3 endpoint URL</td></tr>
    <tr><td><code>region</code></td><td>string</td><td></td><td>Region (<code>"auto"</code> for R2, <code>"us-east-1"</code> for AWS)</td></tr>
    <tr><td><code>access_key_id</code></td><td>string</td><td></td><td>Access key</td></tr>
    <tr><td><code>secret_access_key</code></td><td>string</td><td></td><td>Secret key</td></tr>
    <tr><td><code>prefix</code></td><td>string</td><td><code>""</code></td><td>Key prefix in bucket (e.g. <code>"events"</code>)</td></tr>
    <tr><td><code>path_style</code></td><td>bool</td><td><code>false</code></td><td>Path-style addressing (required for MinIO)</td></tr>
    <tr><td><code>archive_after_days</code></td><td>u64</td><td><code>30</code></td><td>Archive events older than this many days</td></tr>
    <tr><td><code>archive_after_hours</code></td><td>u64</td><td><code>0</code></td><td>Sub-day archival (hours). Overrides <code>archive_after_days</code> when non-zero. Set to <code>1</code> for hourly offload.</td></tr>
    <tr><td><code>archive_threshold_mb</code></td><td>u64</td><td><code>0</code></td><td>Also archive when the data dir exceeds this size (MB). 0 = age-only.</td></tr>
    <tr><td><code>archive_batch_size</code></td><td>usize</td><td><code>10000</code></td><td>Maximum events per JSONL archive file</td></tr>
    <tr><td><code>archive_interval_secs</code></td><td>u64</td><td><code>3600</code></td><td>How often the archive loop runs (seconds). Default: 1 hour.</td></tr>
  </tbody>
</table>
<p>Archival runs every <code>archive_interval_secs</code> (default 1 hour). Events are grouped by <code>(user_id, day)</code>, serialized to JSONL, compressed with zstd, and uploaded to S3/R2. S3 connectivity is verified at startup. See <a href="{base}/docs/blob-storage">Event Archival</a> for the full lifecycle.</p>
<p><strong>Important:</strong> Set <code>archive_after_days</code> lower than your shortest retention tier&rsquo;s <code>days</code> value, or GC will delete events before they can be archived.</p>

<h2 id="cors">[cors]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>allowed_origins</code></td><td>string[]</td><td><code>[]</code></td><td>CORS origin allowlist. Empty = same-origin only. <code>["*"]</code> = allow all (not recommended).</td></tr>
  </tbody>
</table>

<h2 id="pipeline">[pipeline]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Range</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>batch_size</code></td><td>usize</td><td><code>64</code></td><td>1 &ndash; 100,000</td><td>Events per batch flush</td></tr>
    <tr><td><code>max_body_bytes</code></td><td>usize</td><td><code>10485760</code></td><td>1 &ndash; 100 MB</td><td>Max request body size</td></tr>
    <tr><td><code>channel_capacity</code></td><td>usize</td><td><code>32768</code></td><td>1 &ndash; &infin;</td><td>Batch writer queue depth. Raise for bursty traffic.</td></tr>
    <tr><td><code>flush_interval_ms</code></td><td>u64</td><td><code>50</code></td><td>1 &ndash; 10,000</td><td>BatchWriter flush cadence. Lower = fresher reads, more segment files. Raise to 250+ for sustained-write workloads where operators tolerate ~half-second read staleness.</td></tr>
    <tr><td><code>write_timeout_secs</code></td><td>u64</td><td><code>10</code></td><td>1 &ndash; 300</td><td>Max wait per durable ingest write before returning 500. Bounds worst-case request latency under back-pressure.</td></tr>
  </tbody>
</table>

<h2 id="idempotency">[idempotency]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>enabled</code></td><td>bool</td><td><code>true</code></td><td>Enable idempotency key support</td></tr>
    <tr><td><code>ttl_secs</code></td><td>u64</td><td><code>300</code></td><td>Cache TTL for idempotency keys (seconds)</td></tr>
    <tr><td><code>max_entries</code></td><td>usize</td><td><code>100000</code></td><td>Maximum cached idempotency keys (LRU eviction)</td></tr>
  </tbody>
</table>

<h2 id="rate-limit">[rate_limit]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>enabled</code></td><td>bool</td><td><code>false</code></td><td>Enable per-key rate limiting</td></tr>
    <tr><td><code>requests_per_second</code></td><td>f64</td><td><code>100.0</code></td><td>Token bucket refill rate per API key</td></tr>
    <tr><td><code>burst</code></td><td>usize</td><td><code>200</code></td><td>Maximum burst size per API key</td></tr>
  </tbody>
</table>

<h2 id="pricing">[pricing]</h2>
<p>Pricing catalog daily refresh task. On error, the existing catalog stays in place (the bundled fallback always loads at startup).</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>refresh_interval_secs</code></td><td>u64</td><td><code>86400</code></td><td>Refresh cadence in seconds. Range 60&ndash;604800. Set 0 to disable. ±10% jitter is applied per cycle so multi-replica deployments don&rsquo;t stampede the upstream.</td></tr>
    <tr><td><code>source_url</code></td><td>string</td><td>LiteLLM main branch</td><td>Override only when mirroring through a CDN / private proxy.</td></tr>
  </tbody>
</table>
<p>The <code>keplor_pricing_catalog_age_seconds</code> gauge surfaces silently-stale catalogs to alerting.</p>

<h2 id="tls">[tls]</h2>
<p>Optional. When present, the server listens with HTTPS via rustls.</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>cert_path</code></td><td>string</td><td>Path to PEM-encoded certificate chain</td></tr>
    <tr><td><code>key_path</code></td><td>string</td><td>Path to PEM-encoded private key</td></tr>
  </tbody>
</table>

<h2 id="env">Environment variables</h2>
<p>Override any key with <code>KEPLOR_</code> prefix and underscore nesting:</p>
<pre><code>$ KEPLOR_SERVER_LISTEN_ADDR=0.0.0.0:9090 \
  KEPLOR_STORAGE_DATA_DIR=/tmp/keplor_data \
  KEPLOR_AUTH_API_KEYS=sk-key1,sk-key2 \
  keplor run</code></pre>
<p>Environment variables take precedence over the config file.</p>

<h2 id="perf-build">Build-time perf knobs</h2>
<p>For maximum throughput on a known target host:</p>
<pre><code>RUSTFLAGS="-C target-cpu=native" \
  cargo build --release -p keplor-cli \
    --features keplor-cli/mimalloc \
    --features keplor-server/simd-json</code></pre>
<table>
  <thead><tr><th>Flag</th><th>What it does</th><th>Approximate gain</th></tr></thead>
  <tbody>
    <tr><td><code>RUSTFLAGS=-C target-cpu=native</code></td><td>Autovectorize for the host&rsquo;s instruction set (AVX2/AVX-512)</td><td>5&ndash;10%</td></tr>
    <tr><td><code>--features keplor-cli/mimalloc</code></td><td>Replace the system allocator with mimalloc</td><td>30&ndash;50% under high alloc churn</td></tr>
    <tr><td><code>--features keplor-server/simd-json</code></td><td>Use <code>simd-json</code> for the ingest body parse</td><td>1.5&ndash;3× on 1&ndash;5 KB JSON</td></tr>
    <tr><td>PGO (profile-guided optimization)</td><td>2-stage build with a representative load profile</td><td>+10&ndash;25% on top, plus ~3× p99 reduction on the f&amp;f path</td></tr>
  </tbody>
</table>
<p>For statically-linked deploys also pin <code>--target x86_64-unknown-linux-musl</code>. PGO workflow is documented in <a href="https://github.com/bravo1goingdark/keplor/blob/main/docs/operations.md#build-time-perf-knobs">docs/operations.md</a>.</p>

<h2 id="validation">Validation</h2>
<p>Config is validated at startup. Invalid values produce immediate errors:</p>
<pre><code>Error: invalid config: pipeline.batch_size must be > 0</code></pre>
