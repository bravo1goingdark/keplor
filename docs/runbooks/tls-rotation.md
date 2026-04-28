# TLS Rotation

**Status: stub — depends on SIGHUP-driven TLS reload.** The current
`sighup_reload_loop` in `crates/keplor-server/src/server.rs` only
hot-swaps the API key set; `tls_config` is built once in
`PipelineServer::new()` (lines 161-198) and is never rebuilt. There
is no in-process rustls cert/key rotation today.

The manual workaround is **graceful restart** with new cert files,
which drops in-flight TLS connections at the OS level (clients see a
TLS connection reset and must retry). Keep restart windows short and
stagger across replicas if you run more than one.

## Trigger

- Cert nearing expiry (alert at 21 days; hard fail at 7).
- ACME / cert-manager renewal completed and wrote new cert files to
  disk; running process still serves the old chain.
- Suspected key compromise.
- Cipher suite or chain change (intermediate CA replacement).

## Verify

1. Cert files on disk match what's expected:
   ```
   openssl x509 -in /etc/keplor/tls/cert.pem -noout -dates -subject -issuer
   ```
2. Compare the live cert (what the running process is serving) with the
   on-disk cert:
   ```
   echo | openssl s_client -connect localhost:8443 -servername your.host \
     2>/dev/null | openssl x509 -noout -dates -fingerprint -sha256
   openssl x509 -in /etc/keplor/tls/cert.pem -noout -fingerprint -sha256
   ```
   Different fingerprints → on-disk cert is fresh, process needs a
   restart.
3. Cert path agrees with `keplor.toml`:
   ```
   grep -A2 '^\[tls\]' /etc/keplor/keplor.toml
   ```

## Fix

1. Stage the new cert + key. Both files must be readable by the
   `keplor` service user:
   ```
   sudo cp new-cert.pem /etc/keplor/tls/cert.pem
   sudo cp new-key.pem  /etc/keplor/tls/key.pem
   sudo chown keplor:keplor /etc/keplor/tls/{cert,key}.pem
   sudo chmod 0640 /etc/keplor/tls/key.pem
   ```
2. Validate the new cert/key parse before restart — `keplor` fails
   `PipelineServer::new` on invalid PEM, and a failed restart leaves you
   with no listener:
   ```
   openssl x509 -in /etc/keplor/tls/cert.pem -noout -text >/dev/null
   openssl rsa  -in /etc/keplor/tls/key.pem  -check -noout      \
     || openssl ec -in /etc/keplor/tls/key.pem -check -noout
   ```
3. Graceful restart — `axum::serve` calls `with_graceful_shutdown` on
   SIGINT/SIGTERM, drains the batch writer, and runs a final WAL
   checkpoint:
   ```
   sudo systemctl restart keplor
   ```
   Existing TLS connections are dropped (no in-process reload exists).
   Clients with retries (LiteLLM, gateway-style integrations) will
   reconnect; raw HTTP clients that don't retry will see one failed
   request.
4. Verify the new cert is being served:
   ```
   echo | openssl s_client -connect localhost:8443 -servername your.host \
     2>/dev/null | openssl x509 -noout -dates -fingerprint -sha256
   ```
5. **TODO: requires SIGHUP-driven TLS reload.** When implemented, the
   procedure above collapses to: copy new files, send SIGHUP, verify
   fingerprint changed without dropping connections. Track the work
   alongside `sighup_reload_loop` extensions in `server.rs`.

## Post-mortem template

1. Timeline (UTC)
2. Detection: cert-monitor alert, customer report, scheduled rotation
3. Customer impact: connection resets during restart, request retries
4. Root cause: expired / compromised / scheduled
5. Resolution: when new cert in production, when restart completed
6. Action items: rotation cadence, cert-monitor alert thresholds,
   ship in-process TLS reload to remove the restart requirement
