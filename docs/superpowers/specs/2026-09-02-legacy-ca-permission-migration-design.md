# Legacy CA Permission Migration Design

## Problem

Wirelens versions before the multi-instance security hardening created `ca_cert.der`, `ca_key.der`, and the configured CA PEM file with process-default permissions. Existing installations commonly have mode `0644`.

Current startup opens those files safely but rejects every mode other than `0600`. This prevents an existing owner-controlled, regular CA from loading and produces `authority output must be an owner-only regular file`. New installations are unaffected because new authority files are already created with mode `0600`.

## Decision

On Unix, Wirelens will migrate an existing authority output only when the safely opened object is a regular file owned by the effective user. If its permission bits differ from `0600`, Wirelens will apply `0600` through the opened file handle, re-read metadata, and require the final mode to be exactly `0600` before reading data.

Wirelens will preserve the existing authority bytes. It will not regenerate the CA or invalidate trust already installed in browsers and devices.

## Security Invariants

- Open authority outputs with `O_NOFOLLOW` before inspecting or modifying them.
- Reject symlinks, non-regular files, and files not owned by the effective user.
- Change permissions through the verified open file handle rather than a separately resolved path.
- Revalidate file type, owner, and mode after the permission change.
- Preserve the existing two-MiB size limit and bounded read.
- Perform migration while holding the existing CA initialization lock.
- Propagate permission-change and validation failures; do not replace or regenerate files on failure.

Non-Unix behavior remains unchanged.

## Data Flow

1. `create_or_load_ca` prepares the owner-only authority directory and acquires `.ca-init.lock`.
2. `CaData::from_disk` requests each authority output through the secure reader.
3. The reader opens the path without following symlinks and reads metadata from the resulting handle.
4. The reader validates regular-file type and effective-user ownership.
5. For a legacy mode, the reader applies `0600` through the handle and revalidates metadata.
6. The reader enforces the size limit and returns the unchanged bytes.
7. Startup continues with the existing certificate, key, and PEM data.

## Error Handling

Unsafe identity or object-type failures remain fatal. A failed permission change or failed post-change validation remains fatal and keeps the existing contextual startup error chain. Partial or missing authority sets remain fatal under the existing policy.

## Verification

A Unix regression test will:

- create a valid persisted CA;
- change all three output files to legacy mode `0644`;
- load the authority again;
- assert certificate, key, and PEM bytes are unchanged; and
- assert every output is now mode `0600`.

Existing tests continue to cover new authority creation, persistence, concurrent initialization, directory mode, lock mode, and output mode. Final verification includes the focused CA suite, a startup smoke test using the reported legacy directory, the full test suite, formatting, and strict Clippy. Required design and performance reviews follow implementation.

## Non-goals

- Regenerating an existing CA.
- Repairing foreign-owned files, symlinks, devices, sockets, or directories.
- Changing certificate formats or configured paths.
- Broadening permission migration to unrelated Wirelens state.
