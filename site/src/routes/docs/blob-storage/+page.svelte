<script lang="ts">
  import { base } from '$app/paths';
  import Pre from '$lib/components/Pre.svelte';
</script>

<svelte:head>
  <title>Blob Storage - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">Blob Storage</h1>
<p class="text-lg text-text-muted mb-8">Offload request/response bodies to S3, R2, or MinIO.</p>

<h2 id="how-it-works">How it works</h2>
<p>Keplor stores two kinds of data:</p>
<table>
  <thead><tr><th>Data</th><th>Where</th><th>Size</th></tr></thead>
  <tbody>
    <tr><td>Event metadata (timestamps, tokens, cost, user IDs, indexes)</td><td>Always SQLite</td><td>~300 bytes/event</td></tr>
    <tr><td>Request/response bodies (compressed with zstd)</td><td>SQLite <strong>or</strong> external store</td><td>~500&ndash;2000 bytes/event</td></tr>
  </tbody>
</table>
<p>By default, everything lives in SQLite. When you configure <code>[blob_storage]</code>, the compressed body bytes move to an S3-compatible object store. Event metadata stays in SQLite for fast queries &mdash; only the heavy payloads move.</p>
<p>All query, stats, rollup, and quota endpoints work identically regardless of where blobs live. The only difference is that viewing full request/response bodies fetches from the external store (~50&ndash;200ms) instead of local SQLite (~1ms).</p>

<h3>Build with S3 support</h3>
<Pre code="$ cargo build --release --features mimalloc,s3" />
<p>Or with Docker:</p>
<Pre code={'# Dockerfile already includes mimalloc.\n# To add S3, edit the build line:\nRUN cargo build --release --locked --target x86_64-unknown-linux-musl \\\n    -p keplor-cli --features mimalloc,s3'} />

<h2 id="smart-routing">Smart routing</h2>
<p>You don't have to choose upfront. Smart routing starts with SQLite and offloads to S3/R2 automatically when the database grows past a threshold:</p>

<Pre code={'[storage]\nblob_offload_threshold_mb = 500   # embedded until 500 MB, then auto-offload\n\n[blob_storage]\nbucket = "keplor-blobs"\nendpoint = "https://<account-id>.r2.cloudflarestorage.com"\nregion = "auto"\naccess_key_id = "..."\nsecret_access_key = "..."'} />

<table>
  <thead><tr><th>Threshold</th><th>Behavior</th></tr></thead>
  <tbody>
    <tr><td><code>0</code> (default)</td><td>All new blobs go to external store when <code>[blob_storage]</code> is set</td></tr>
    <tr><td><code>&gt; 0</code></td><td>Embedded until SQLite exceeds threshold, then auto-offload to external store</td></tr>
    <tr><td>No <code>[blob_storage]</code></td><td>Always embedded in SQLite regardless of threshold</td></tr>
  </tbody>
</table>

<h3>How the threshold works</h3>
<p>Keplor checks <code>db_size_bytes()</code> on each batch flush (~every 50ms). When SQLite exceeds <code>blob_offload_threshold_mb</code>, three things happen:</p>
<ol>
  <li><strong>New blobs route to S3/R2</strong> &mdash; an internal flag flips and all new blobs go to the external store</li>
  <li><strong>Old blobs drain automatically</strong> &mdash; a background task (every 60s) migrates existing embedded blobs to S3/R2 in batches of 100, then sets <code>data = NULL</code> in SQLite</li>
  <li><strong>VACUUM runs after drain</strong> &mdash; once all embedded blobs are drained, Keplor runs <code>VACUUM</code> to reclaim disk space and actually shrink the database file</li>
</ol>
<p>When the DB shrinks below the threshold (via drain + VACUUM or GC), the flag flips back and new blobs go to SQLite again.</p>
<Pre code={'SQLite < threshold         SQLite >= threshold\n+--------------------+     +--------------------------+\n| New blobs -> SQLite| --> | New blobs -> S3/R2       |\n|                    |     | Drain old blobs -> S3/R2 |\n|                    |     | SET data = NULL          |\n|                    | <-- | VACUUM -> DB shrinks     |\n+--------------------+     +--------------------------+'} />
<p>The hybrid reader always works: it checks the SQLite <code>data</code> column first &mdash; if <code>NULL</code>, falls back to the external store. Mixed embedded + external blobs coexist seamlessly.</p>

<h2 id="r2">Cloudflare R2</h2>
<p>R2 is the recommended choice for most deployments: 10 GB free storage, zero egress fees, S3-compatible API.</p>
<ol>
  <li>Create a bucket in the Cloudflare dashboard (e.g. <code>keplor-blobs</code>)</li>
  <li>Create an R2 API token with Object Read &amp; Write permissions</li>
  <li>Add to <code>keplor.toml</code>:</li>
</ol>
<Pre code={'[blob_storage]\nbucket = "keplor-blobs"\nendpoint = "https://<account-id>.r2.cloudflarestorage.com"\nregion = "auto"\naccess_key_id = "your-r2-access-key"\nsecret_access_key = "your-r2-secret-key"'} />

<h2 id="s3">AWS S3</h2>
<Pre code={'[blob_storage]\nbucket = "keplor-blobs"\nendpoint = "https://s3.us-east-1.amazonaws.com"\nregion = "us-east-1"\naccess_key_id = "AKIA..."\nsecret_access_key = "..."'} />
<p>Standard S3 pricing applies. Consider S3 Intelligent-Tiering for infrequently accessed blobs.</p>

<h2 id="minio">MinIO (self-hosted)</h2>
<Pre code={'[blob_storage]\nbucket = "keplor-blobs"\nendpoint = "http://localhost:9000"\nregion = "us-east-1"\naccess_key_id = "minioadmin"\nsecret_access_key = "minioadmin"\npath_style = true    # required for MinIO'} />
<p>Any S3-compatible service works: DigitalOcean Spaces, Backblaze B2, Wasabi, etc.</p>

<h2 id="config-ref">Configuration reference</h2>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>bucket</code></td><td>string</td><td></td><td>Bucket name (required)</td></tr>
    <tr><td><code>endpoint</code></td><td>string</td><td></td><td>S3 endpoint URL (required)</td></tr>
    <tr><td><code>region</code></td><td>string</td><td></td><td>Region (<code>"auto"</code> for R2, <code>"us-east-1"</code> for AWS)</td></tr>
    <tr><td><code>access_key_id</code></td><td>string</td><td></td><td>Access key (required)</td></tr>
    <tr><td><code>secret_access_key</code></td><td>string</td><td></td><td>Secret key (required)</td></tr>
    <tr><td><code>prefix</code></td><td>string</td><td><code>""</code></td><td>Key prefix (e.g. <code>"blobs/"</code>)</td></tr>
    <tr><td><code>path_style</code></td><td>bool</td><td><code>false</code></td><td>Path-style addressing (required for MinIO)</td></tr>
  </tbody>
</table>
<p>Related <code>[storage]</code> field:</p>
<table>
  <thead><tr><th>Key</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>blob_offload_threshold_mb</code></td><td>u64</td><td><code>0</code></td><td>Auto-offload when SQLite exceeds this size (0 = always external when configured)</td></tr>
  </tbody>
</table>

<h2 id="dedup">Deduplication</h2>
<p>Blobs are keyed by their SHA-256 hash (64-char hex string). Identical payloads &mdash; e.g. the same system prompt sent thousands of times &mdash; produce the same key. S3 PUTs are naturally idempotent: same key = same content = safe overwrite. No coordination needed.</p>
<p>Reference counts are tracked in SQLite. When multiple events reference the same blob, the refcount increments. The blob is only deleted from external storage when all referencing events are gone.</p>

<h2 id="gc">GC &amp; cleanup</h2>
<p>When events are deleted (via tiered retention GC, the <code>DELETE /v1/events</code> API, or manual <code>keplor gc</code>), Keplor:</p>
<ol>
  <li>Decrements the blob's reference count in SQLite</li>
  <li>If the count reaches zero, deletes the metadata row from SQLite</li>
  <li>After the SQLite transaction commits, deletes the blob from the external store</li>
</ol>
<p>Failed external deletes are logged as warnings but don't block GC. Orphaned blobs in S3 waste storage but don't cause correctness issues &mdash; they can be cleaned up manually via S3 lifecycle rules.</p>

<h2 id="migration">Migration</h2>

<h3>Embedded to external</h3>
<p>When you add <code>[blob_storage]</code> to a running instance:</p>
<ul>
  <li><strong>New blobs:</strong> Go to the external store immediately</li>
  <li><strong>Old blobs:</strong> Readable via the hybrid reader (no downtime)</li>
  <li><strong>With threshold:</strong> Old blobs are drained automatically (100/tick, every 60s), then VACUUM reclaims the space</li>
  <li><strong>Without threshold:</strong> Old blobs stay in SQLite until they expire via GC</li>
</ul>
<p>No manual migration script needed when using smart routing &mdash; the drain loop handles it.</p>

<h3>External back to embedded</h3>
<p>Remove the <code>[blob_storage]</code> section and restart. New blobs go back to SQLite. Existing external blobs become unreadable (their <code>data</code> column is <code>NULL</code> and no external store is configured). Only do this after all external-stored events have expired via GC.</p>

<h2 id="next">Next steps</h2>
<p>
  <a href="{base}/docs/configuration#blob-storage">Configuration reference</a> &mdash; all <code>[blob_storage]</code> fields.<br />
  <a href="{base}/docs/configuration#storage">Storage config</a> &mdash; <code>blob_offload_threshold_mb</code> and other storage settings.<br />
  <a href="{base}/docs/integration">Integration guide</a> &mdash; full setup with retention tiers and auth.
</p>
