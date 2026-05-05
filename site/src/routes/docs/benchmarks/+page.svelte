<script lang="ts">
  // Numbers below are produced by the criterion benches in
  // `keplordb/crates/keplordb/benches/{engine_bench.rs, compact_bench.rs}`.
  // Reproduce with `cargo bench -p keplordb` from the keplordb workspace.
</script>

<svelte:head>
  <title>Benchmarks - Keplor</title>
  <meta
    name="description"
    content="Storage engine benchmarks for keplordb — single-thread and concurrent write throughput, columnar query latency, rollup lookups, WAL fsync cost, and pre/post-compaction aggregate speedup."
  />
</svelte:head>

<h1>Benchmarks</h1>
<p class="text-lg text-text-muted mb-8">
  Storage engine numbers, measured against the embedded keplordb engine that backs every keplor instance.
</p>

<h2 id="caveat">What you are looking at</h2>
<p>
  Keplor's HTTP layer (40K+ rps, 5 ms p99 fire-and-forget) sits on top of an
  embedded columnar log engine called <a href="https://keplordb.pages.dev" target="_blank" rel="noopener">keplordb</a>.
  The numbers on this page measure that engine in-process, with no HTTP socket in the path —
  they are the upper bound the wire format works against, not what an end-to-end benchmark
  client will see.
</p>
<p>
  All write benchmarks are <em>durable</em>: every accepted event passes through the WAL
  and reaches the disk before the call returns. There is no fire-and-forget fast path
  in these numbers.
</p>

<h2 id="setup">Methodology</h2>
<table>
  <thead><tr><th>Component</th><th>Value</th></tr></thead>
  <tbody>
    <tr><td>Harness</td><td><code>criterion 0.8.2</code>, default sample sizes, release + debuginfo profile</td></tr>
    <tr><td>CPU</td><td>11th Gen Intel Core i5-1135G7 @ 2.40 GHz, 4 cores / 8 threads</td></tr>
    <tr><td>Storage</td><td>NVMe SSD, ext4, mount with default sync semantics</td></tr>
    <tr><td>Kernel</td><td>Linux 6.17</td></tr>
    <tr><td>Toolchain</td><td><code>rustc 1.95.0</code></td></tr>
    <tr><td>Allocator</td><td><code>mimalloc</code> (engine default)</td></tr>
    <tr><td>Workload</td><td>Synthetic <code>LogEvent&lt;5,5,3&gt;</code> records, low-cardinality dims (matches production interns)</td></tr>
  </tbody>
</table>
<p>
  Reproduce: <code>cd keplordb &amp;&amp; TMPDIR=/var/tmp cargo bench -p keplordb</code>. The
  <code>TMPDIR</code> override is required if your <code>/tmp</code> is tmpfs — concurrent
  ingest fills it.
</p>

<h2 id="writes">Writes</h2>
<p>Single-thread durable ingest. One <code>append_batch()</code> per iteration; WAL fsync is part of the measured path.</p>
<table>
  <thead><tr><th>Batch size</th><th>Median time</th><th>Throughput</th></tr></thead>
  <tbody>
    <tr><td>64</td><td>166 µs</td><td>387 K events/s</td></tr>
    <tr><td>256</td><td>1.27 ms</td><td>202 K events/s</td></tr>
    <tr><td>1 024</td><td>5.87 ms</td><td>175 K events/s</td></tr>
    <tr><td>4 096</td><td>7.16 ms</td><td><strong>572 K events/s</strong></td></tr>
  </tbody>
</table>
<p>
  Throughput is non-monotonic in batch size because the WAL fsync interval and the segment
  rotation threshold interact — a 1 024-event batch straddles a rotation boundary that a
  4 096-event batch absorbs cleanly. In production this matters less; ingest is concurrent.
</p>

<h3 id="writes-concurrent">Concurrent writes</h3>
<p>T threads sharing one <code>Arc&lt;Engine&gt;</code>, each pushing N-event batches.</p>
<table>
  <thead><tr><th>Threads</th><th>Batch</th><th>Throughput</th></tr></thead>
  <tbody>
    <tr><td>2</td><td>256</td><td>225 K events/s</td></tr>
    <tr><td>2</td><td>1 024</td><td>827 K events/s</td></tr>
    <tr><td>4</td><td>256</td><td>245 K events/s</td></tr>
    <tr><td>4</td><td>1 024</td><td><strong>1.20 M events/s</strong></td></tr>
    <tr><td>8</td><td>256</td><td>965 K events/s</td></tr>
    <tr><td>8</td><td>1 024</td><td>685 K events/s</td></tr>
  </tbody>
</table>
<p>
  Scales to ~1.2 M events/s at 4 threads × 1 024-event batches; 8 threads regresses because
  fsync contention dominates on this disk. Sharded WAL (<a href="#wal">below</a>) is the
  knob to turn when you outgrow this.
</p>

<h3 id="writes-rotation">Rotation overhead</h3>
<table>
  <thead><tr><th>Case</th><th>Median time</th><th>Throughput</th></tr></thead>
  <tbody>
    <tr><td>1 shard, 1 024-event batches with forced rotation every 4 096 events</td><td>28.4 ms</td><td>361 K events/s</td></tr>
  </tbody>
</table>

<h2 id="queries">Queries</h2>
<p>
  Aggregate and scan over a 1 M-event, 40-segment dataset. Throughput is reported in
  <code>elem/s</code> — events <em>scanned</em>, not result rows. Zone-map skipping plus
  AVX2 SIMD aggregates means the engine can rip through billions of elements per second
  to find the matching handful.
</p>
<table>
  <thead><tr><th>Query</th><th>Median time</th><th>Throughput</th></tr></thead>
  <tbody>
    <tr><td><code>aggregate_all</code> (full scan)</td><td>768 µs</td><td>1.30 G elem/s</td></tr>
    <tr><td><code>aggregate_user</code></td><td>260 µs</td><td>3.84 G elem/s</td></tr>
    <tr><td><code>aggregate_time_range</code></td><td>270 µs</td><td>3.71 G elem/s</td></tr>
    <tr><td><code>aggregate_user_and_time</code></td><td>97 µs</td><td>10.31 G elem/s</td></tr>
    <tr><td><code>query_recent_100</code></td><td>45 µs</td><td>22.36 G elem/s</td></tr>
    <tr><td><code>query_recent_1000</code></td><td>439 µs</td><td>2.28 G elem/s</td></tr>
    <tr><td><code>query_recent_user_100</code></td><td>64 µs</td><td>15.54 G elem/s</td></tr>
  </tbody>
</table>

<h2 id="rollups">Rollups</h2>
<p>In-memory rollup store lookups (the structure that backs <code>GET /v1/rollups</code> on a hot dataset).</p>
<table>
  <thead><tr><th>Query</th><th>Median time</th></tr></thead>
  <tbody>
    <tr><td><code>single_user_single_day</code></td><td>7.2 µs</td></tr>
    <tr><td><code>single_org_single_day</code></td><td>8.0 µs</td></tr>
    <tr><td><code>all_buckets_single_day</code></td><td>24.5 µs</td></tr>
  </tbody>
</table>

<h2 id="wal">WAL</h2>
<p>WAL in isolation, varying fsync interval. <code>memory_only</code> is the no-fsync baseline — the speed-of-light for the in-memory path.</p>
<table>
  <thead><tr><th>Case</th><th>Median time</th><th>Throughput</th></tr></thead>
  <tbody>
    <tr><td>fsync every event</td><td>2.44 ms / 1 024 ev</td><td>419 K events/s</td></tr>
    <tr><td>fsync every 64 events</td><td>2.33 ms / 1 024 ev</td><td>440 K events/s</td></tr>
    <tr><td>fsync every 1 024 events</td><td>2.80 ms / 1 024 ev</td><td>365 K events/s</td></tr>
    <tr><td><code>memory_only</code> (no fsync)</td><td>369 µs / 1 024 ev</td><td><strong>2.77 M events/s</strong></td></tr>
  </tbody>
</table>

<h3 id="wal-shards">Sharded WAL</h3>
<p>4 producer threads, varying shard count. More shards = more parallel fsync streams; the optimum is hardware-dependent.</p>
<table>
  <thead><tr><th>Shards</th><th>Median time</th><th>Throughput</th></tr></thead>
  <tbody>
    <tr><td>1</td><td>2.09 ms</td><td>490 K events/s</td></tr>
    <tr><td>2</td><td>1.83 ms</td><td>560 K events/s</td></tr>
    <tr><td>4</td><td>3.15 ms</td><td>325 K events/s</td></tr>
    <tr><td>8</td><td>2.18 ms</td><td>469 K events/s</td></tr>
  </tbody>
</table>

<h2 id="compaction">Compaction</h2>
<p>
  Aggregate latency before and after compacting 1 000 tiny segments (10 events each)
  into one. Same query, same dataset.
</p>
<table>
  <thead><tr><th>State</th><th>Median time</th><th>Throughput</th></tr></thead>
  <tbody>
    <tr><td>Before (1 000 tiny segments)</td><td>15.6 ms</td><td>640 K elem/s</td></tr>
    <tr><td>After (1 segment)</td><td>10.7 µs</td><td><strong>931 M elem/s</strong></td></tr>
  </tbody>
</table>
<p>
  ≈ 1 450× speed-up. Aggregate latency is roughly O(segment count) on the cold path —
  compaction is what gets you back onto the SIMD/zone-map fast path. Schedule it.
</p>

<h2 id="http-tier">HTTP tier (sharded BatchWriter)</h2>
<p>
  These engine numbers are the floor of the stack — what the WAL and the
  segment scanner can do in process. Above the engine, keplor's HTTP
  ingest tier funnels every event through a sharded
  <code>BatchWriter</code> with <code>flush_shards</code> parallel append
  loops + a decoupled rotator. Numbers below were measured against the
  same 4-core/8-thread laptop, no PGO, single keplor process, with
  <code>wal_shard_count = 8</code> and <code>flush_shards = 8</code>:
</p>
<table>
  <thead><tr><th>Path</th><th>Single funnel (legacy)</th><th>Sharded (current)</th><th>Δ</th></tr></thead>
  <tbody>
    <tr><td>Fire-and-forget zero-error</td><td>~41 K rps</td><td><strong>~55 K rps</strong></td><td>+34 %</td></tr>
    <tr><td>Durable peak (c=512), p99</td><td>20.2 K rps, 33.6 ms</td><td><strong>44.9 K rps, 21.4 ms</strong></td><td>+122 % rps, &minus;36 % p99</td></tr>
    <tr><td>Batch endpoint zero-error</td><td>25.6 K events/s</td><td><strong>51.2 K events/s</strong></td><td>+100 %</td></tr>
  </tbody>
</table>
<p>
  Each append loop calls <code>append_batch_durable</code>
  independently; keplordb's internal round-robin spreads the calls
  across separate WAL shards, so N concurrent flushes fsync N different
  files with no lock contention. The rotator runs once per
  <code>flush_interval</code> and rotates WAL → segments for read
  visibility, replacing the per-batch <code>wal_checkpoint</code> that
  previously serialised every flush behind a tier-global lock-loop.
</p>

<h2 id="caveats">Caveats</h2>
<p>
  These numbers are floor-of-the-stack and exclude HTTP framing, JSON deserialisation,
  cost computation, and rate-limiting middleware. End-to-end ingest through
  <code>POST /v1/events</code> is bounded by both the engine and the wire path; treat the
  service-level <a href="../../">numbers on the homepage</a> (40K+ rps, 5 ms p99
  fire-and-forget) as the realistic ceiling.
</p>
<p>
  fsync behaviour varies by kernel, filesystem, and underlying block device. The numbers
  here are from a consumer NVMe SSD on ext4; a server-grade NVMe with a battery-backed
  controller will fsync faster, and a cloud block volume (EBS gp3, GCP pd-balanced) will
  fsync slower. Re-run on your target hardware before sizing.
</p>
