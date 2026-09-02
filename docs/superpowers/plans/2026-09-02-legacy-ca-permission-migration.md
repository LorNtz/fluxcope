# Legacy CA Permission Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use supo-subagent-driven-development (recommended) or supo-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Wirelens securely load legacy owner-controlled CA files created with mode `0644` by tightening them to `0600` without changing the CA bytes.

**Architecture:** Keep migration inside the existing locked CA load path. The secure reader opens each output with `O_NOFOLLOW`, validates the opened handle's type and owner, applies `0600` through that handle when needed, revalidates the handle, and then performs the existing bounded read.

**Tech Stack:** Rust 2024, `std::fs`, Unix `MetadataExt`/`PermissionsExt`, `rustix`, `anyhow`, built-in Rust tests, `tempfile`.

## Global Constraints

- Preserve the existing CA certificate, private key, and PEM bytes.
- Never follow symlinks or repair files not owned by the effective user.
- Require a regular file with final mode `0600` before reading its contents.
- Preserve the existing two-MiB authority-file limit and CA initialization lock.
- Keep non-Unix behavior unchanged.
- Do not refactor unrelated certificate, registry, or settings code.

---

### Task 1: Migrate safe legacy authority output permissions

**Files:**
- Modify: `src/ca.rs:235-271`
- Test: `src/ca.rs:310-455`

**Interfaces:**
- Consumes: `create_or_load_ca(cert_dir: &Path, pem_filename: &str) -> anyhow::Result<CaData>` and the existing `read_owner_only_file(path: &Path) -> io::Result<Vec<u8>>` secure-reader boundary.
- Produces: unchanged function signatures; on Unix, owner-controlled regular authority outputs with legacy permission bits are tightened to `0600` before their bounded contents are returned.

- [ ] **Step 1: Add the failing legacy-permission regression**

Add this Unix-only test inside `src/ca.rs`'s existing `tests` module:

```rust
#[cfg(unix)]
#[test]
fn legacy_authority_permissions_are_tightened_before_loading() {
    use std::os::unix::fs::PermissionsExt as _;

    let cert_dir = tempfile::tempdir().expect("temporary CA directory");
    let pem_filename = "wirelens-ca.pem";
    let original = create_or_load_ca(cert_dir.path(), pem_filename)
        .expect("initial authority creation should succeed");

    for name in [CA_CERT_FILE, CA_KEY_FILE, pem_filename] {
        fs::set_permissions(
            cert_dir.path().join(name),
            fs::Permissions::from_mode(0o644),
        )
        .expect("legacy permissions should be installed");
    }

    let loaded = create_or_load_ca(cert_dir.path(), pem_filename)
        .expect("legacy authority should be migrated and loaded");

    assert_eq!(loaded.cert_der(), original.cert_der());
    assert_eq!(loaded.key_der(), original.key_der());
    assert_eq!(loaded.cert_pem(), original.cert_pem());
    for name in [CA_CERT_FILE, CA_KEY_FILE, pem_filename] {
        assert_eq!(
            fs::metadata(cert_dir.path().join(name))
                .expect("migrated authority metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "{name} must be tightened to owner-only",
        );
    }
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test legacy_authority_permissions_are_tightened_before_loading --all-features -- --nocapture
```

Expected: FAIL at the second `create_or_load_ca` call with `authority output must be an owner-only regular file`.

- [ ] **Step 3: Implement handle-based permission migration**

In `read_owner_only_file`, retain the existing no-follow open and replace the Unix metadata/mode rejection block with this sequence:

```rust
#[cfg(unix)]
let metadata = {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mut metadata = file.metadata()?;
    let effective_uid = rustix::process::geteuid().as_raw();
    if !metadata.file_type().is_file() || metadata.uid() != effective_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "authority output must be a regular file owned by the effective user",
        ));
    }
    if metadata.mode() & 0o777 != 0o600 {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        metadata = file.metadata()?;
    }
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "authority output must be an owner-only regular file",
        ));
    }
    metadata
};
#[cfg(not(unix))]
let metadata = file.metadata()?;
```

Keep the current `O_NOFOLLOW` open, size checks, capacity allocation, and `take(...).read_to_end(...)` unchanged.

- [ ] **Step 4: Run the focused CA suite and verify GREEN**

Run:

```bash
cargo test ca::tests --all-features -- --nocapture
```

Expected: PASS, including the new migration regression and existing persistence/concurrency/security tests.

- [ ] **Step 5: Smoke-test the reported startup path**

Build and launch Wirelens against the existing `/Users/didi/.wirelens/certificate/` directory. Confirm the proxy reaches readiness, then stop it cleanly. Assert with `stat` that `ca_cert.der`, `ca_key.der`, and `wirelens-ca.pem` are regular files owned by UID `601`, have mode `0600`, and retain their pre-migration checksums.

- [ ] **Step 6: Run required reviews and final verification**

Dispatch the repository-required design/maintainability and performance/resource reviewers without asking them to run commands. Adopt material findings, then run:

```bash
cargo fmt --all -- --check
cargo check --all-targets --all-features --locked
cargo test --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
git diff --check -- src/ca.rs
```

Expected: all commands pass; the full test count is at least the current 848-test baseline plus the new regression.

- [ ] **Step 7: Record and commit the fix**

Append the required timestamped implementation/review/verification record to local-only `what_i_just_did.md`, then commit only the production/test change:

```bash
git add src/ca.rs
git commit -m "fix: migrate legacy CA file permissions"
```
