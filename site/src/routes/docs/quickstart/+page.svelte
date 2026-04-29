<script lang="ts">
  import { base } from '$app/paths';
  import Pre from '$lib/components/Pre.svelte';

  const healthResp = `$ curl http://localhost:8080/health
{"status":"ok","version":"0.1.0","db":"connected","queue_depth":0,"queue_capacity":32768,"queue_utilization_pct":0}`;

  const ingestReq = `$ curl -X POST http://localhost:8080/v1/events \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "gpt-4o",
    "provider": "openai",
    "usage": {"input_tokens": 500, "output_tokens": 200},
    "latency": {"total_ms": 1200, "ttft_ms": 180},
    "user_id": "alice",
    "source": "my-app"
  }'`;

  const ingestResp = `{
  "id": "01J5XQKR...",
  "cost_nanodollars": 6250000,
  "model": "gpt-4o",
  "provider": "openai"
}`;

  const queryCmd = `$ keplor query --user-id alice
ID                           PROVIDER       MODEL                   TOKENS       COST ($)
--------------------------------------------------------------------------------------------
01J5XQKR...                  openai         gpt-4o                     700     0.00625000

1 event(s)`;

  const statsCmd = `$ keplor stats --data-dir ./keplor_data
=== Keplor Storage Statistics ===
Data dir:             ./keplor_data
Total events:         1
Total bytes on disk:  47.3 KB
Tiers:                free, pro, team`;
</script>

<svelte:head>
  <title>Quickstart - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">Quickstart</h1>
<p class="text-lg text-text-muted mb-8">From zero to ingesting events in under 2 minutes.</p>

<h2 id="install">1. Install</h2>
<p>Build from source:</p>
<Pre code="$ git clone https://github.com/bravo1goingdark/keplor.git
$ cd keplor
$ cargo build --release --features keplor-cli/mimalloc
$ cp target/release/keplor /usr/local/bin/" />
<p class="text-sm text-text-muted">Add <code>--features keplor-server/simd-json</code> for the AVX-accelerated JSON parser, or <code>RUSTFLAGS="-C target-cpu=native"</code> to autovectorize for the host CPU. See <a href="{base}/docs/configuration#perf-build">build-time perf knobs</a>.</p>

<h2 id="start">2. Start the server</h2>
<Pre code="$ keplor run" />
<p>Binds to <code>0.0.0.0:8080</code> and creates <code>./keplor_data/</code> &mdash; the KeplorDB data directory with one segment tree per retention tier. No config needed.</p>
<p>Verify:</p>
<Pre code={healthResp} />

<h2 id="first-event">3. Send your first event</h2>
<Pre code={ingestReq} />
<p>Response (<code>201 Created</code>):</p>
<Pre code={ingestResp} />
<p>Cost is auto-computed. <code>6250000</code> nanodollars = <strong>$0.00625</strong>.</p>
<p class="text-sm text-text-muted">For hot-path traffic that doesn&rsquo;t need per-event flush confirmation, append <code>?durable=false</code> &mdash; the request returns <code>202 Accepted</code> immediately, p50 drops from ~5 ms to ~1.5 ms.</p>

<h2 id="query">4. Query it back</h2>
<Pre code={'$ curl "http://localhost:8080/v1/events?user_id=alice&limit=5"'} />
<p>Or from the CLI:</p>
<Pre code={queryCmd} />

<h2 id="stats">5. Check stats</h2>
<Pre code={statsCmd} />

<h2 id="next">Next steps</h2>
<p>
  <a href="{base}/docs/integration">Integration Guide</a> &mdash; Python, Node.js, LiteLLM, S3/R2 setup, tiered retention.<br />
  <a href="{base}/docs/api-reference">API Reference</a> &mdash; full endpoint docs.<br />
  <a href="{base}/docs/configuration">Configuration</a> &mdash; auth, archival, retention tiers, tuning.<br />
  <a href="{base}/docs/cli">CLI Reference</a> &mdash; all commands.
</p>
