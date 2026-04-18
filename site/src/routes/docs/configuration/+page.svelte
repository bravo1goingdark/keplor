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
db_path = "/var/lib/keplor/keplor.db"
retention_days = 90          # legacy global GC (prefer [retention] tiers)
wal_checkpoint_secs = 300
max_db_size_mb = 0           # 0 = unlimited
read_pool_size = 4           # SQLite read connections (1-64)
gc_interval_secs = 3600     # how often GC runs (0 = disabled)

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
batch_size = 128
max_body_bytes = 10485760

[idempotency]
enabled = true
ttl_secs = 300
max_entries = 100000

[rate_limit]
enabled = false
requests_per_second = 100.0
burst = 200

# Optional: offload blobs to S3/R2 (requires --features s3)
# [blob_storage]
# bucket = "keplor-blobs"
# endpoint = "https://&lt;account&gt;.r2.cloudflarestorage.com"
# region = "auto"
# access_key_id = "..."
# secret_access_key = "..."

# [tls]
# cert_path = "/etc/keplor/cert.pem"
# key_path = "/etc/keplor/key.pem"</code></pre>

<h2 id="server">[server]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>listen_addr</code></td><td>string</td><td><code>"0.0.0.0:8080"</code></td><td>Bind address</td></tr>
    <tr><td><code>shutdown_timeout_secs</code></td><td>u64</td><td><code>25</code></td><td>Graceful shutdown timeout (drain + checkpoint)</td></tr>
    <tr><td><code>request_timeout_secs</code></td><td>u64</td><td><code>30</code></td><td>Per-request timeout</td></tr>
    <tr><td><code>max_connections</code></td><td>usize</td><td><code>10000</code></td><td>Maximum concurrent connections</td></tr>
  </tbody>
</table>

<h2 id="storage">[storage]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>db_path</code></td><td>string</td><td><code>"keplor.db"</code></td><td>SQLite database path</td></tr>
    <tr><td><code>retention_days</code></td><td>u64</td><td><code>90</code></td><td>Legacy global GC threshold (0 = disabled). Prefer <code>[retention]</code> tiers.</td></tr>
    <tr><td><code>wal_checkpoint_secs</code></td><td>u64</td><td><code>300</code></td><td>WAL truncation interval (0 = disabled)</td></tr>
    <tr><td><code>max_db_size_mb</code></td><td>u64</td><td><code>0</code></td><td>Max database size in MB (0 = unlimited). Returns HTTP 507 when exceeded.</td></tr>
    <tr><td><code>read_pool_size</code></td><td>usize</td><td><code>4</code></td><td>Number of SQLite read connections (range: 1&ndash;64)</td></tr>
    <tr><td><code>gc_interval_secs</code></td><td>u64</td><td><code>3600</code></td><td>How often GC runs in seconds (0 = disabled)</td></tr>
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
<p>Per-tier retention policies. GC runs one pass per tier, deleting events older than the tier's <code>days</code> threshold.</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>default_tier</code></td><td>string</td><td><code>"free"</code></td><td>Tier assigned to simple-format keys and unauthenticated requests</td></tr>
    <tr><td><code>tiers</code></td><td>object[]</td><td>free (7d), pro (90d)</td><td>Named tiers with retention days. <code>days = 0</code> keeps events forever.</td></tr>
  </tbody>
</table>
<p>Tiers are just names &mdash; add <code>"team"</code>, <code>"enterprise"</code>, or any custom tier via config. No code changes needed.</p>

<h2 id="blob-storage">[blob_storage] (optional, --features s3)</h2>
<p>Offload request/response body blobs to an S3-compatible object store (Cloudflare R2, MinIO, AWS S3). Event metadata stays in SQLite; only the heavy compressed payloads move.</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>bucket</code></td><td>string</td><td>Bucket name</td></tr>
    <tr><td><code>endpoint</code></td><td>string</td><td>S3 endpoint URL</td></tr>
    <tr><td><code>region</code></td><td>string</td><td>Region (<code>"auto"</code> for R2, <code>"us-east-1"</code> for AWS)</td></tr>
    <tr><td><code>access_key_id</code></td><td>string</td><td>Access key</td></tr>
    <tr><td><code>secret_access_key</code></td><td>string</td><td>Secret key</td></tr>
    <tr><td><code>prefix</code></td><td>string</td><td>Optional key prefix (e.g. <code>"blobs/"</code>)</td></tr>
    <tr><td><code>path_style</code></td><td>bool</td><td>Use path-style addressing (required for MinIO)</td></tr>
  </tbody>
</table>

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
    <tr><td><code>max_body_bytes</code></td><td>usize</td><td><code>10485760</code></td><td>1 &ndash; 100MB</td><td>Max request body size</td></tr>
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
  KEPLOR_STORAGE_DB_PATH=/tmp/keplor.db \
  KEPLOR_AUTH_API_KEYS=sk-key1,sk-key2 \
  keplor run</code></pre>
<p>Environment variables take precedence over the config file.</p>

<h2 id="pragmas">SQLite pragmas</h2>
<p>Applied automatically at startup:</p>
<table>
  <thead><tr><th>Pragma</th><th>Value</th><th>Purpose</th></tr></thead>
  <tbody>
    <tr><td><code>journal_mode</code></td><td>WAL</td><td>Concurrent reads during writes</td></tr>
    <tr><td><code>synchronous</code></td><td>NORMAL</td><td>Fsync on checkpoint only</td></tr>
    <tr><td><code>mmap_size</code></td><td>256 MB</td><td>Memory-mapped I/O</td></tr>
    <tr><td><code>busy_timeout</code></td><td>5000 ms</td><td>Retry on lock contention</td></tr>
    <tr><td><code>cache_size</code></td><td>64 MB</td><td>Page cache</td></tr>
    <tr><td><code>temp_store</code></td><td>MEMORY</td><td>Temp tables in RAM</td></tr>
    <tr><td><code>wal_autocheckpoint</code></td><td>1000 pages</td><td>Auto-checkpoint WAL to bound growth</td></tr>
  </tbody>
</table>

<h2 id="validation">Validation</h2>
<p>Config is validated at startup. Invalid values produce immediate errors:</p>
<pre><code>Error: invalid config: pipeline.batch_size must be > 0</code></pre>
