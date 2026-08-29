
### Fluxcope rebase: integration change
Date: 2026/09/17 10:34

- Replaying MCP process modes on master: retained Fluxcope package/release metadata and upgraded network dependencies; combined master clap 4.6 with MCP derive support. Preserved master lockfile pending incremental dependency reconciliation.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:36

- Merged isolated configuration modes with Fluxcope startup and master private-filesystem protections. Retained owner-only atomic settings writes, master clap 4.6, and the feature configuration lease dependency; preserved master network lockfile rather than restoring obsolete dependency versions.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:38

- Resolved all five src/ca.rs conflict blocks: retained rcgen 0.14 Certificate/KeyPair APIs, Fluxcope CA DN, byte/PEM accessors and PEM filename confinement; integrated feature serialized initialization, incomplete-authority refusal, bounded 2 MiB reads and unchanged feature privacy/concurrency tests. Reused private_fs ensure/open/write for owner/regular-file/no-follow/single-link protections instead of duplicate CA helpers; kept directory fsync after persistence. No validation/staging/rebase performed. Existing upstream private_fs::open_file already chmods validated files to 0600, so later f19a261 permission migration must account for that existing equivalent behavior; no separate migration introduced.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:38

- Resolved src/runtime/mod.rs conflicts at registry commit: retained ProxyStartup/SettingsSession, bound std listener and actual endpoint identity, endpoint-specific log path (removed invalid settings.path()); converted retained listener to Tokio for Hudsucker 0.25 Proxy::builder, preserved rcgen Issuer/rustls provider connector and Hyper 1 certificate server. Installed aws_lc_rs default provider at runtime startup; no validation/staging performed.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:38

- Resolved src/logging/writer.rs conflict: endpoint-isolated path/rotation retained; open_log_file performs private_fs::ensure_directory then private_fs::open_file in one spawn_blocking closure. Removed redundant feature Unix OpenOptions/directory helper and unreachable weaker non-Unix branch; upstream owner UID, 0700/0600, NOFOLLOW, singly-linked regular-file and NONBLOCK checks now protect per-instance logs. No validation/staging performed.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:39

- Per Main clarification, removed newly introduced global install_default from runtime::run. Master has no global provider install; runtime preserves its explicit aws_lc_rs provider passed to RcgenAuthority and with_rustls_connector without adding process-global side effects. Final runtime smoke remains Main-owned.
