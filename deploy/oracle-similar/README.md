# Oracle similarity runner

The Worker serves cached results. Oracle exports versioned fingerprint and CLIP
pages, runs the same Rust similarity algorithms natively, then publishes the
complete result only while that catalog generation is still current.

Run local checks without network access:

```sh
python3 deploy/oracle-similar/test_runner.py
```

Build `cargo build --release --bin vitrine-similar` on Linux aarch64 for Oracle.
The current server has Docker but no Rust toolchain; a temporary Rust build
container can produce the binary without installing Rust on the host. Use the
Rust 1.95 toolchain and the repository's lockfile. Do not deploy a macOS binary.

Deployment layout (production installation and activation are separate):

| Source | Oracle destination |
| --- | --- |
| `tools/clip_embed.py` | `/home/ubuntu/vitrine-similar/clip_embed.py` |
| `tools/similar_compute.py` | `/home/ubuntu/vitrine-similar/similar_compute.py` |
| Linux aarch64 `target/release/vitrine-similar` | `/home/ubuntu/vitrine-similar/vitrine-similar` |
| `vitrine-similar.service` | `/etc/systemd/system/vitrine-similar.service` |
| `vitrine-similar.timer` | `/etc/systemd/system/vitrine-similar.timer` |

The service runs as `ubuntu` and reads the existing
`/opt/hanabi-container/hanabi.env` directly. It does not copy the token or use an
`EnvironmentFile`, so credentials are absent from arguments and process
environment. Systemd creates `/var/lib/vitrine-similar` for temporary input,
output, and the single-run lock. Input/output files are removed after each run;
the next locked run also removes files left by an interrupted process.

The service first checks the catalog generation. If the previous successful scan
checked that generation and its similarity result is current, it makes no further
requests. Otherwise it runs the existing OpenCLIP ViT-B-32/openai scanner for missing images, then computes groups. Completed upload batches survive interruption and are skipped on retry. The scanner uses the existing virtualenv and cached model with four Torch threads and up to four prefetched downloads; no missing images means no model load. A partial scan still publishes completed work and marks the service failed so the next scheduled run retries missing keys.

The timer runs once daily at 00:15 UTC (08:15 Beijing/Singapore), with up to
15 seconds jitter and a persistent missed-run catch-up. New works enter similarity
review after the next scheduled run. Unchanged catalogs require only one small
version request. Full export uses ordered `after`/`next_cursor` pages rather than
OFFSET; counts are requested only for an export and cached by catalog generation.
The runner records the pre-scan generation, so writes during scanning still cause
a check on the next run. HTTP operations have a 30-second timeout and at most
three attempts; a 409 discards that run and waits for the next timer. D1 daily
quota responses stop retries and persist a cooldown until the stated reset. Native computation has a 20-minute timeout. Systemd
bounds the whole run to 30 minutes, four CPUs, and 2 GiB RAM. A failed calculation
or stale publication preserves the previous successful cache.

After installation, verify unit syntax and the next calendar trigger. Verify
`result_fresh` and the authenticated admin response when D1 is available; do not
run a full production scan solely for deployment verification. The local test
suite covers the complete scan/export/publish path with disposable data.
For rollback, stop/disable the timer and service, restore the previous runner
and binary if replacing them, and restore the prior Worker if its API changed.
Stopping the runner preserves the published cache, but future catalog changes
need an active runner to refresh it.
