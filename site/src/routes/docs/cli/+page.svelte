<svelte:head>
  <title>CLI Reference - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">CLI Reference</h1>
<p class="text-lg text-text-muted mb-8">All commands in the <code>keplor</code> binary.</p>

<h2 id="run">keplor run</h2>
<p>Start the HTTP ingestion server.</p>
<pre><code>$ keplor run [OPTIONS]

Options:
  -c, --config &lt;PATH&gt;    Config file [default: keplor.toml]
      --json-logs        Emit structured JSON logs (for log aggregation)</code></pre>
<p>If the config file doesn&rsquo;t exist, defaults are used. Binds to <code>0.0.0.0:8080</code> and creates the KeplorDB data directory at <code>./keplor_data</code> with one segment tree per retention tier.</p>

<h2 id="migrate">keplor migrate</h2>
<p>Verify or initialise a data directory. Opens the store, creating per-tier engines if missing, and refuses to mount a directory written under a mismatched schema id. Idempotent &mdash; safe to run repeatedly.</p>
<pre><code>$ keplor migrate --data-dir /var/lib/keplor
data dir ready at /var/lib/keplor (schema id verified)</code></pre>

<h2 id="query">keplor query</h2>
<p>Query stored events from the command line.</p>
<pre><code>$ keplor query [OPTIONS]

Options:
  --user-id &lt;ID&gt;          Filter by user
  --model &lt;NAME&gt;          Filter by model
  --provider &lt;KEY&gt;        Filter by provider
  --source &lt;NAME&gt;         Filter by source
  --limit &lt;N&gt;             Max results [default: 20]
  -d, --data-dir &lt;DIR&gt;    KeplorDB data directory [default: ./keplor_data]</code></pre>
<p>Example:</p>
<pre><code>$ keplor query --provider openai --limit 5
ID                           PROVIDER       MODEL                   TOKENS       COST ($)
--------------------------------------------------------------------------------------------
01J5XQKR...                  openai         gpt-4o                     700     0.00625000
01J5XPBN...                  openai         gpt-4o-mini                340     0.00012750

2 event(s)</code></pre>

<h2 id="stats">keplor stats</h2>
<p>Print per-tier storage statistics.</p>
<pre><code>$ keplor stats --data-dir /var/lib/keplor
=== Keplor Storage Statistics ===
Data dir:             /var/lib/keplor
Total events:         12,847
Total bytes on disk:  4.2 MB
Per tier:
  free  segments=128  events=8,401  bytes=2.7 MB
  pro   segments=42   events=4,123  bytes=1.4 MB
  team  segments=4    events=323    bytes=120 KB</code></pre>

<h2 id="gc">keplor gc</h2>
<p>Delete events older than a threshold. Segment-granular &mdash; drops whole segments whose events are entirely older than the cutoff.</p>
<pre><code>$ keplor gc --older-than-days 30 --data-dir /var/lib/keplor
GC complete: tier=free segments_deleted=24 bytes_freed=1.1 MB
GC complete: tier=pro  segments_deleted=8  bytes_freed=312 KB</code></pre>
<p>Schedule via cron for automatic cleanup if you&rsquo;ve disabled the in-process GC loop:</p>
<pre><code># crontab -e
0 3 * * * /usr/local/bin/keplor gc --older-than-days 90 --data-dir /var/lib/keplor</code></pre>

<h2 id="rollup">keplor rollup</h2>
<p>Force a WAL checkpoint &mdash; rotates the active WAL into a sealed segment so post-write queries see the events immediately. The legacy <code>--days</code> flag is accepted for compatibility but is now a no-op (rollups are accumulated on every write).</p>
<pre><code>$ keplor rollup --data-dir /var/lib/keplor
WAL checkpointed: 3 tiers flushed</code></pre>

<h2 id="archive">keplor archive</h2>
<p>Manually archive old events to S3/R2 (requires <code>--features s3</code>).</p>
<pre><code>$ keplor archive --config keplor.toml --older-than-days 7
Archived 4,291 events in 3 files (1.2 MB compressed)</code></pre>
<p>Useful for one-off archival outside the automatic hourly cycle.</p>

<h2 id="archive-status">keplor archive-status</h2>
<p>Show archive manifest &mdash; what&rsquo;s been archived to S3/R2.</p>
<pre><code>$ keplor archive-status --data-dir /var/lib/keplor
=== Archive Status ===
User        Day          Events  Compressed
alice       2026-04-12     847    42.1 KB
alice       2026-04-13     923    45.3 KB
bob         2026-04-12     512    28.7 KB
Total: 3 archive files, 2,282 events, 116.1 KB</code></pre>
