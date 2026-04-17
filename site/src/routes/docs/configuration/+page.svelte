<svelte:head>
  <title>Configuration - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">Configuration</h1>
<p class="text-lg text-text-muted mb-8">TOML config file with environment variable overrides.</p>

<h2>Config file</h2>
<p>Keplor looks for <code>keplor.toml</code> by default. Override with <code>--config</code>:</p>
<pre><code>$ keplor run --config /etc/keplor/keplor.toml</code></pre>

<h2>Full example</h2>
<pre><code># keplor.toml

[server]
listen_addr = "0.0.0.0:8080"
shutdown_timeout_secs = 25

[storage]
db_path = "/var/lib/keplor/keplor.db"

[auth]
api_keys = ["sk-keplor-prod-abc123", "sk-keplor-prod-def456"]

[pipeline]
batch_size = 128
max_body_bytes = 10485760</code></pre>

<h2>[server]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>listen_addr</code></td><td>string</td><td><code>"0.0.0.0:8080"</code></td><td>Bind address</td></tr>
    <tr><td><code>shutdown_timeout_secs</code></td><td>u64</td><td><code>25</code></td><td>Graceful shutdown timeout</td></tr>
  </tbody>
</table>

<h2>[storage]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>db_path</code></td><td>string</td><td><code>"keplor.db"</code></td><td>SQLite database path</td></tr>
  </tbody>
</table>

<h2>[auth]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>api_keys</code></td><td>string[]</td><td><code>[]</code></td><td>Valid API keys. Empty = open access (warning logged).</td></tr>
  </tbody>
</table>

<h2>[pipeline]</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Range</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>batch_size</code></td><td>usize</td><td><code>64</code></td><td>1 &ndash; 100,000</td><td>Events per batch flush</td></tr>
    <tr><td><code>max_body_bytes</code></td><td>usize</td><td><code>10485760</code></td><td>1 &ndash; 100MB</td><td>Max request body size</td></tr>
  </tbody>
</table>

<h2>Environment variables</h2>
<p>Override any key with <code>KEPLOR_</code> prefix and underscore nesting:</p>
<pre><code>$ KEPLOR_SERVER_LISTEN_ADDR=0.0.0.0:9090 \
  KEPLOR_STORAGE_DB_PATH=/tmp/keplor.db \
  KEPLOR_AUTH_API_KEYS=sk-key1,sk-key2 \
  keplor run</code></pre>
<p>Environment variables take precedence over the config file.</p>

<h2>SQLite pragmas</h2>
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
  </tbody>
</table>

<h2>Validation</h2>
<p>Config is validated at startup. Invalid values produce immediate errors:</p>
<pre><code>Error: invalid config: pipeline.batch_size must be > 0</code></pre>
