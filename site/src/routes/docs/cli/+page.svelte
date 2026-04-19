<svelte:head>
  <title>CLI Reference - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">CLI Reference</h1>
<p class="text-lg text-text-muted mb-8">All commands in the <code>keplor</code> binary.</p>

<h2 id="run">keplor run</h2>
<p>Start the HTTP ingestion server.</p>
<pre><code>$ keplor run [OPTIONS]

Options:
  -c, --config &lt;PATH&gt;    Config file [default: keplor.toml]</code></pre>
<p>If the config file doesn't exist, defaults are used. Binds to <code>0.0.0.0:8080</code> and creates <code>keplor.db</code>.</p>

<h2 id="migrate">keplor migrate</h2>
<p>Apply database migrations without starting the server.</p>
<pre><code>$ keplor migrate --db /var/lib/keplor/keplor.db
migrations applied to /var/lib/keplor/keplor.db</code></pre>

<h2 id="query">keplor query</h2>
<p>Query stored events from the command line.</p>
<pre><code>$ keplor query [OPTIONS]

Options:
  --user-id &lt;ID&gt;       Filter by user
  --model &lt;NAME&gt;       Filter by model
  --provider &lt;KEY&gt;     Filter by provider
  --source &lt;NAME&gt;      Filter by source
  --limit &lt;N&gt;          Max results [default: 20]
  -d, --db &lt;PATH&gt;      Database path [default: keplor.db]</code></pre>
<p>Example:</p>
<pre><code>$ keplor query --provider openai --limit 5
ID                           PROVIDER       MODEL                   TOKENS       COST ($)
--------------------------------------------------------------------------------------------
01J5XQKR...                  openai         gpt-4o                     700     0.00625000
01J5XPBN...                  openai         gpt-4o-mini                340     0.00012750

2 event(s)</code></pre>

<h2 id="stats">keplor stats</h2>
<p>Print storage statistics.</p>
<pre><code>$ keplor stats
=== Keplor Storage Statistics ===
Database:             keplor.db
Total events:         12,847
Database size:        4.2 MB</code></pre>

<h2 id="gc">keplor gc</h2>
<p>Delete events older than a threshold.</p>
<pre><code>$ keplor gc --older-than-days 30
GC complete: deleted 4,291 events (cutoff: 30 days ago)</code></pre>
<p>Schedule via cron for automatic cleanup:</p>
<pre><code># crontab -e
0 3 * * * /usr/local/bin/keplor gc --older-than-days 90 --db /var/lib/keplor/keplor.db</code></pre>

<h2 id="archive">keplor archive</h2>
<p>Manually archive old events to S3/R2 (requires <code>--features s3</code>).</p>
<pre><code>$ keplor archive --config keplor.toml --older-than-days 7
Archived 4,291 events in 3 files (1.2 MB compressed)</code></pre>
<p>Useful for one-off archival outside the automatic hourly cycle.</p>

<h2 id="archive-status">keplor archive_status</h2>
<p>Show archive manifest &mdash; what&rsquo;s been archived to S3/R2.</p>
<pre><code>$ keplor archive_status --config keplor.toml
=== Archive Status ===
User        Day          Events  Compressed
alice       2026-04-12     847    42.1 KB
alice       2026-04-13     923    45.3 KB
bob         2026-04-12     512    28.7 KB
Total: 3 archive files, 2,282 events</code></pre>
