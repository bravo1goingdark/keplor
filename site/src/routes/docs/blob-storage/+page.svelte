<script lang="ts">
  import { base } from '$app/paths';
  import Pre from '$lib/components/Pre.svelte';
</script>

<svelte:head>
  <title>Event Archival - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">Event Archival</h1>
<p class="text-lg text-text-muted mb-8">Archive old events to S3, R2, or MinIO as compressed JSONL.</p>

<h2 id="how-it-works">How it works</h2>
<p>As events age past a configurable threshold, Keplor archives them to an S3-compatible object store and tombstones them in KeplorDB. Segment GC reclaims the disk space on the next retention sweep, keeping the data dir lean.</p>
<table>
  <thead><tr><th>Data</th><th>After archival</th></tr></thead>
  <tbody>
    <tr><td>Recent events (within <code>archive_after_days</code>)</td><td>Stay in KeplorDB &mdash; fully queryable</td></tr>
    <tr><td>Old events (past threshold)</td><td>Compressed JSONL in S3/R2 &mdash; tombstoned locally; reclaimed by segment GC</td></tr>
    <tr><td>Daily rollups</td><td>Always preserved &mdash; aggregation queries unaffected</td></tr>
    <tr><td>Archive manifests</td><td>Tracked in <code>{`{data_dir}/archive_manifests.jsonl`}</code> for audit</td></tr>
  </tbody>
</table>
<p>All query, stats, rollup, and quota endpoints continue working on data that remains in the local store. The <code>has_archived_data</code> flag in query responses indicates when archived data exists for the queried time range. Pass <code>?include_archived=true</code> on <code>GET /v1/events</code> to merge archived events into the response (one S3 round-trip per overlapping manifest; uncached).</p>

<h3>Build with S3 support</h3>
<Pre code="$ cargo build --release --features keplor-cli/s3,keplor-cli/mimalloc" />
<p>Or with Docker:</p>
<Pre code={'# Dockerfile already includes mimalloc.\n# To add S3, edit the build line:\nRUN cargo build --release --locked --target x86_64-unknown-linux-musl \\\n    -p keplor-cli --features keplor-cli/mimalloc --features keplor-cli/s3'} />

<h2 id="archive-lifecycle">Archive lifecycle</h2>
<p>Every <code>archive_interval_secs</code> (default 1 hour), Keplor checks whether archival should run based on age and/or data-dir size triggers. When triggered:</p>
<ol>
  <li><strong>Force rollup</strong> for affected days (preserves daily aggregations after tombstoning)</li>
  <li><strong>Query events</strong> older than <code>archive_after_days</code>, ordered by <code>(user_id, timestamp)</code></li>
  <li><strong>Group by user + day</strong>, serialize to JSONL, compress with zstd, upload to S3/R2</li>
  <li><strong>Record manifest</strong> in the JSONL sidecar (<code>archive_manifests.jsonl</code>) and the in-memory index</li>
  <li><strong>Tombstone archived events</strong> in KeplorDB; segment GC reclaims their disk space on the next sweep</li>
</ol>

<Pre code={'S3/R2 key format:\n{prefix}/user_id={alice}/day=2026-04-15/{archive_id}.jsonl.zstd'} />

<p>Each chunk (user + day) is archived independently. If one upload fails, the remaining chunks continue. Failed events stay in KeplorDB and are retried on the next cycle.</p>

<h2 id="r2">Cloudflare R2</h2>
<p>R2 is the recommended choice for most deployments: 10 GB free storage, zero egress fees, S3-compatible API.</p>
<ol>
  <li>Create a bucket in the Cloudflare dashboard (e.g. <code>keplor-archive</code>)</li>
  <li>Create an R2 API token with Object Read &amp; Write permissions</li>
  <li>Add to <code>keplor.toml</code>:</li>
</ol>
<Pre code={'[archive]\nbucket = "keplor-archive"\nendpoint = "https://<account-id>.r2.cloudflarestorage.com"\nregion = "auto"\naccess_key_id = "your-r2-access-key"\nsecret_access_key = "your-r2-secret-key"\nprefix = "events"\narchive_after_days = 30'} />

<h2 id="s3">AWS S3</h2>
<Pre code={'[archive]\nbucket = "keplor-archive"\nendpoint = "https://s3.us-east-1.amazonaws.com"\nregion = "us-east-1"\naccess_key_id = "AKIA..."\nsecret_access_key = "..."\nprefix = "events"\narchive_after_days = 30'} />
<p>Standard S3 pricing applies. Consider S3 Intelligent-Tiering for infrequently accessed archives.</p>

<h2 id="minio">MinIO (self-hosted)</h2>
<Pre code={'[archive]\nbucket = "keplor-archive"\nendpoint = "http://localhost:9000"\nregion = "us-east-1"\naccess_key_id = "minioadmin"\nsecret_access_key = "minioadmin"\npath_style = true    # required for MinIO\narchive_after_days = 30'} />
<p>Any S3-compatible service works: DigitalOcean Spaces, Backblaze B2, Wasabi, etc.</p>

<h2 id="config-ref">Configuration reference</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>bucket</code></td><td>string</td><td></td><td>S3 bucket name (required)</td></tr>
    <tr><td><code>endpoint</code></td><td>string</td><td></td><td>S3 endpoint URL (required)</td></tr>
    <tr><td><code>region</code></td><td>string</td><td></td><td>Region (<code>"auto"</code> for R2, <code>"us-east-1"</code> for AWS)</td></tr>
    <tr><td><code>access_key_id</code></td><td>string</td><td></td><td>Access key (required)</td></tr>
    <tr><td><code>secret_access_key</code></td><td>string</td><td></td><td>Secret key (required)</td></tr>
    <tr><td><code>prefix</code></td><td>string</td><td><code>""</code></td><td>Key prefix in bucket (e.g. <code>"events"</code>)</td></tr>
    <tr><td><code>path_style</code></td><td>bool</td><td><code>false</code></td><td>Path-style addressing (required for MinIO)</td></tr>
    <tr><td><code>archive_after_days</code></td><td>u64</td><td><code>30</code></td><td>Archive events older than this many days</td></tr>
    <tr><td><code>archive_after_hours</code></td><td>u64</td><td><code>0</code></td><td>Sub-day archival (hours). Overrides <code>archive_after_days</code> when non-zero. Set to <code>1</code> for hourly offload.</td></tr>
    <tr><td><code>archive_threshold_mb</code></td><td>u64</td><td><code>0</code></td><td>Also archive when the data dir exceeds this size (MB). 0 = age-only.</td></tr>
    <tr><td><code>archive_batch_size</code></td><td>usize</td><td><code>10000</code></td><td>Maximum events per JSONL archive file</td></tr>
    <tr><td><code>archive_interval_secs</code></td><td>u64</td><td><code>3600</code></td><td>How often the archive loop runs (seconds). Default: 1 hour.</td></tr>
  </tbody>
</table>

<h2 id="retention-warning">Archive vs. retention</h2>
<p>If <code>archive_after_days</code> is greater than the shortest retention tier&rsquo;s <code>days</code> value, GC will delete events before they can be archived. Keplor warns about this at startup. Always set <code>archive_after_days</code> lower than your shortest tier&rsquo;s retention.</p>

<h2 id="gc">GC &amp; cleanup</h2>
<p>Archival runs <strong>before</strong> GC in the combined loop to prevent data loss. Daily rollups are force-refreshed before tombstoning, so aggregation queries remain accurate even after events are archived. Segment GC reclaims the on-disk space the next time it runs (segment-granular).</p>
<p>S3 connectivity is verified at startup with a HEAD probe. Bad credentials or unreachable endpoints fail immediately rather than silently misbehaving hours later on the first archive cycle.</p>

<h2 id="cli">CLI commands</h2>
<p>Archive manually (outside the automatic cycle):</p>
<Pre code="$ keplor archive --config keplor.toml --older-than-days 14" />
<p>Check archive status:</p>
<Pre code="$ keplor archive-status --data-dir /var/lib/keplor" />

<h2 id="next">Next steps</h2>
<p>
  <a href="{base}/docs/configuration#archive">Configuration reference</a> &mdash; all <code>[archive]</code> fields.<br />
  <a href="{base}/docs/configuration#storage">Storage config</a> &mdash; <code>max_db_size_mb</code> and other storage settings.<br />
  <a href="{base}/docs/integration">Integration guide</a> &mdash; full setup with retention tiers and auth.
</p>
