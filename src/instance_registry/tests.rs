#![cfg(unix)]

use super::{
    InstanceDescriptor, RegistryMutationLock, RegistryPublisher, RegistryScan, RegistryScanner,
    read_registry_index, validate_descriptor_metadata, write_registry_index,
};
use crate::{
    instance::InstanceIdentity,
    settings::{AppSettings, ConfigMode, PersistenceMode, SettingsSession},
};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const ENDPOINT_A: &str = "127.0.0.1:19001";
const ENDPOINT_B: &str = "[::1]:19002";

struct RegistryFixture {
    home: TempDir,
}

impl RegistryFixture {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().expect("temporary Wirelens home"),
        }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn run_dir(&self) -> PathBuf {
        self.home().join("run")
    }

    fn instances_dir(&self) -> PathBuf {
        self.run_dir().join("instances")
    }

    fn index_path(&self) -> PathBuf {
        self.instances_dir().join(".registry-index.json")
    }

    fn replace_index(&self, descriptors: &[String]) {
        let mut index = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(self.index_path())
            .expect("registry index");
        serde_json::to_writer(
            &mut index,
            &serde_json::json!({"version": 1, "descriptors": descriptors}),
        )
        .expect("write registry index");
        index.flush().expect("flush registry index");
        index.sync_all().expect("sync registry index");
    }

    fn prepare(&self, endpoint: &str) -> RegistryPublisher {
        let endpoint = endpoint.parse::<SocketAddr>().expect("proxy endpoint");
        let identity = InstanceIdentity::new(endpoint).expect("instance identity");
        let settings = SettingsSession::temporary(AppSettings::default());
        RegistryPublisher::prepare(self.home(), identity, &settings)
            .expect("registry publisher should prepare")
    }

    fn publish(&self, endpoint: &str) -> RegistryPublisher {
        let mut publisher = self.prepare(endpoint);
        publisher
            .publish()
            .expect("descriptor should publish atomically");
        publisher
    }

    fn scanner(&self) -> RegistryScanner {
        RegistryScanner::new(self.home()).expect("registry scanner")
    }

    fn scan(&self) -> RegistryScan {
        self.scanner().scan_all().expect("registry scan")
    }

    fn rewrite_descriptor(&self, path: &Path, mutate: impl FnOnce(&mut Value)) {
        let bytes = fs::read(path).expect("published descriptor");
        let mut descriptor: Value = serde_json::from_slice(&bytes).expect("descriptor JSON");
        mutate(&mut descriptor);
        let bytes = serde_json::to_vec(&descriptor).expect("mutated descriptor JSON");
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .expect("open descriptor for mutation");
        file.write_all(&bytes).expect("rewrite descriptor");
        file.sync_all().expect("sync descriptor mutation");
    }

    fn rejected_by_code(report: &RegistryScan, code: &str) -> usize {
        report
            .rejected
            .iter()
            .filter(|diagnostic| diagnostic.code() == code)
            .count()
    }
}

#[test]
fn publisher_binds_before_publication_and_publishes_owner_only_descriptor() {
    let fixture = RegistryFixture::new();
    let mut publisher = fixture.prepare(ENDPOINT_A);

    assert!(publisher.socket_path().exists());
    assert!(
        fs::metadata(publisher.socket_path())
            .expect("socket metadata")
            .file_type()
            .is_socket()
    );
    assert!(!publisher.descriptor_path().exists());
    assert_eq!(
        fs::metadata(fixture.run_dir())
            .expect("run directory")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(fixture.instances_dir())
            .expect("instances directory")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(publisher.socket_path())
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    publisher.publish().expect("publish descriptor");

    let descriptor_metadata = fs::metadata(publisher.descriptor_path()).expect("descriptor");
    assert!(descriptor_metadata.is_file());
    assert_eq!(descriptor_metadata.permissions().mode() & 0o777, 0o600);
    assert!(
        fs::read_dir(fixture.instances_dir())
            .expect("instances directory")
            .all(|entry| !entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp")),
        "atomic publication must not leave temporary files"
    );

    let report = fixture.scan();
    assert!(report.rejected.is_empty());
    assert_eq!(report.omitted, 0);
    assert_eq!(report.candidates.len(), 1);
    let descriptor = &report.candidates[0];
    assert_eq!(
        descriptor.proxy_endpoint(),
        publisher.identity().proxy_endpoint()
    );
    assert_eq!(descriptor.local_proxy_url(), "http://127.0.0.1:19001");
    assert_eq!(descriptor.run_id(), publisher.identity().run_id());
    assert_eq!(descriptor.started_at(), publisher.identity().started_at());
    assert_eq!(descriptor.pid(), std::process::id());
    assert_eq!(descriptor.socket_path(), publisher.socket_path());
    assert_eq!(descriptor.config_mode(), ConfigMode::Temporary);
    assert_eq!(descriptor.persistence(), PersistenceMode::Ephemeral);
    assert_eq!(descriptor.config_source(), None);
    assert_eq!(descriptor.binary_version(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn endpoint_and_run_derived_registry_names_are_stable_and_bounded() {
    let fixture = RegistryFixture::new();
    let first = fixture.prepare(ENDPOINT_A);
    let second = fixture.prepare(ENDPOINT_A);

    let descriptor_name = first
        .descriptor_path()
        .file_name()
        .expect("descriptor name")
        .to_string_lossy();
    let first_socket_name = first
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy();
    let second_socket_name = second
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy();

    assert_eq!(first.descriptor_path(), second.descriptor_path());
    assert_ne!(first.socket_path(), second.socket_path());
    assert_eq!(descriptor_name.len(), 69);
    assert!(descriptor_name.ends_with(".json"));
    assert!(first_socket_name.len() <= 69);
    assert!(first_socket_name.ends_with(".sock"));
    assert!(second_socket_name.len() <= 69);
}

#[test]
fn scanner_rejects_symlink_descriptor() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    let descriptor_path = publisher.descriptor_path().to_path_buf();
    let target_path = fixture.instances_dir().join("descriptor-target");
    fs::rename(&descriptor_path, &target_path).expect("move descriptor target");
    symlink(&target_path, &descriptor_path).expect("descriptor symlink");

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_symlink"),
        1
    );
    assert!(report.candidates.is_empty());
}

#[test]
fn metadata_validation_rejects_a_descriptor_owned_by_another_user() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    let metadata = fs::metadata(publisher.descriptor_path()).expect("descriptor metadata");
    let other_uid = metadata.uid().wrapping_add(1);

    let diagnostic = validate_descriptor_metadata(&metadata, other_uid)
        .expect_err("wrong owner must be rejected");

    assert_eq!(diagnostic.code(), "descriptor_wrong_owner");
}

#[test]
fn scanner_rejects_group_or_world_accessible_descriptor() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    fs::set_permissions(
        publisher.descriptor_path(),
        fs::Permissions::from_mode(0o640),
    )
    .expect("weaken descriptor permissions");

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_permissions"),
        1
    );
    assert!(report.candidates.is_empty());
}

#[test]
fn scanner_refuses_a_group_or_world_accessible_registry_directory() {
    let fixture = RegistryFixture::new();
    let _publisher = fixture.publish(ENDPOINT_A);
    fs::set_permissions(fixture.instances_dir(), fs::Permissions::from_mode(0o750))
        .expect("weaken registry permissions");

    assert!(
        RegistryScanner::new(fixture.home()).is_err(),
        "scanner must not trust a registry writable or searchable by other users"
    );
}

#[test]
fn scanner_rejects_descriptor_larger_than_64_kib_before_json_parsing() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    OpenOptions::new()
        .write(true)
        .open(publisher.descriptor_path())
        .expect("open descriptor")
        .set_len(64 * 1024 + 1)
        .expect("enlarge descriptor");

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_too_large"),
        1
    );
    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_invalid_json"),
        0,
        "the size limit must be enforced before parsing"
    );
}

#[test]
fn scanner_rejects_malformed_descriptor_without_aborting_the_scan() {
    let fixture = RegistryFixture::new();
    let good = fixture.publish(ENDPOINT_A);
    let bad = fixture.publish(ENDPOINT_B);
    fs::write(bad.descriptor_path(), b"{not-json").expect("malformed descriptor");
    fs::set_permissions(bad.descriptor_path(), fs::Permissions::from_mode(0o600))
        .expect("owner-only descriptor");

    let report = fixture.scan();

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), good.identity().run_id());
    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_invalid_json"),
        1
    );
}

#[test]
fn scanner_rejects_unknown_descriptor_schema_version() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    fixture.rewrite_descriptor(publisher.descriptor_path(), |descriptor| {
        let current = descriptor["schema_version"]
            .as_u64()
            .expect("numeric schema version");
        descriptor["schema_version"] = Value::from(current + 1);
    });

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_schema_version"),
        1
    );
}

#[test]
fn scanner_rejects_unsupported_rpc_version() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    fixture.rewrite_descriptor(publisher.descriptor_path(), |descriptor| {
        let current = descriptor["rpc_version"]
            .as_u64()
            .expect("numeric RPC version");
        descriptor["rpc_version"] = Value::from(current + 1);
    });

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_rpc_version"),
        1
    );
}

#[test]
fn scanner_rejects_socket_path_outside_the_owner_registry_root() {
    use std::os::unix::net::UnixListener;

    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    let escaped_path = fixture.home().join("escaped.sock");
    let _escaped_listener = UnixListener::bind(&escaped_path).expect("escaped socket");
    fixture.rewrite_descriptor(publisher.descriptor_path(), |descriptor| {
        descriptor["socket_path"] = Value::from(escaped_path.to_string_lossy().into_owned());
    });

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "socket_path_escape"),
        1
    );
}

#[test]
fn scanner_requires_socket_basename_to_match_endpoint_and_run_identity() {
    use std::os::unix::net::UnixListener;

    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    let wrong_socket = publisher
        .socket_path()
        .with_file_name("wrong-identity.sock");
    let _wrong_listener = UnixListener::bind(&wrong_socket).expect("wrong identity socket");
    fixture.rewrite_descriptor(publisher.descriptor_path(), |descriptor| {
        descriptor["socket_path"] = Value::from(wrong_socket.to_string_lossy().into_owned());
    });

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "socket_name_mismatch"),
        1
    );
}

#[test]
fn scanner_rejects_regular_file_at_the_declared_socket_path() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    fs::remove_file(publisher.socket_path()).expect("remove listener pathname");
    let mut replacement = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(publisher.socket_path())
        .expect("regular socket replacement");
    replacement
        .write_all(b"not a socket")
        .expect("replacement data");

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "socket_not_socket"),
        1
    );
}

#[test]
fn scanner_reports_a_missing_indexed_descriptor_and_ignores_its_untracked_rename() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    let mut wrong_name = publisher
        .descriptor_path()
        .file_name()
        .expect("descriptor filename")
        .to_string_lossy()
        .into_owned();
    let replacement = if wrong_name.starts_with('0') {
        "1"
    } else {
        "0"
    };
    wrong_name.replace_range(..1, replacement);
    let wrong_name = fixture.instances_dir().join(wrong_name);
    fs::rename(publisher.descriptor_path(), &wrong_name).expect("rename descriptor");

    let report = fixture.scan();

    assert_eq!(
        RegistryFixture::rejected_by_code(&report, "descriptor_open"),
        1
    );
}

#[test]
fn scanner_ignores_descriptor_shaped_files_missing_from_the_bounded_index() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    let mut created = 0;
    for index in 0..=256_u16 {
        let path = fixture.instances_dir().join(format!("{index:064x}.json"));
        if path == publisher.descriptor_path() {
            continue;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .expect("capped descriptor fixture");
        file.write_all(b"{}").expect("descriptor fixture");
        created += 1;
        if created == 256 {
            break;
        }
    }
    assert_eq!(created, 256);

    let report = fixture.scan();

    assert_eq!(report.candidates.len(), 1);
    assert!(report.rejected.is_empty());
    assert_eq!(report.omitted, 0);
}

#[test]
fn registry_mutation_lock_is_exclusive_across_independent_handles() {
    let fixture = RegistryFixture::new();
    fs::create_dir_all(fixture.run_dir()).expect("run directory");
    fs::set_permissions(fixture.run_dir(), fs::Permissions::from_mode(0o700))
        .expect("owner-only run directory");
    let first = RegistryMutationLock::acquire(fixture.home()).expect("registry mutation lock");

    let error = RegistryMutationLock::try_acquire(fixture.home())
        .err()
        .expect("second mutation handle must contend");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);

    drop(first);
    RegistryMutationLock::try_acquire(fixture.home())
        .expect("lock must be available after the owner exits");
}

#[test]
fn endpoint_read_does_not_scan_an_unrelated_invalid_descriptor() {
    let fixture = RegistryFixture::new();
    let good = fixture.publish(ENDPOINT_A);
    let bad = fixture.publish(ENDPOINT_B);
    fs::write(bad.descriptor_path(), b"invalid").expect("invalid unrelated descriptor");
    fs::set_permissions(bad.descriptor_path(), fs::Permissions::from_mode(0o600))
        .expect("owner-only descriptor");

    let report = fixture
        .scanner()
        .read_endpoint(ENDPOINT_A.parse().expect("endpoint"))
        .expect("targeted descriptor read");

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), good.identity().run_id());
    assert!(report.rejected.is_empty());
    assert_eq!(report.omitted, 0);
}

#[test]
fn publisher_refuses_to_overwrite_an_existing_endpoint_descriptor() {
    let fixture = RegistryFixture::new();
    let first = fixture.publish(ENDPOINT_A);
    let mut second = fixture.prepare(ENDPOINT_A);
    let authoritative_run_id = first.identity().run_id().clone();

    let error = second
        .publish()
        .expect_err("Task 5 must probe liveness before replacement");

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    let report = fixture.scan();
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), &authoritative_run_id);
}

#[test]
fn cleanup_holds_registry_lock_from_identity_reread_through_unlink() {
    let fixture = RegistryFixture::new();
    let mut publisher = fixture.publish(ENDPOINT_A);
    let descriptor_path = publisher.descriptor_path().to_path_buf();
    let socket_path = publisher.socket_path().to_path_buf();

    publisher
        .cleanup_with_observer(|| {
            let error = RegistryMutationLock::try_acquire(fixture.home())
                .err()
                .expect("cleanup observer must run while mutation lock is held");
            assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        })
        .expect("locked publisher cleanup");

    assert!(!descriptor_path.exists());
    assert!(!socket_path.exists());
}

#[test]
fn stale_cleanup_refuses_a_descriptor_whose_run_id_changed_before_locked_reread() {
    let fixture = RegistryFixture::new();
    let publisher = fixture.publish(ENDPOINT_A);
    let descriptor_path = publisher.descriptor_path().to_path_buf();
    let replacement =
        InstanceIdentity::new(ENDPOINT_A.parse().expect("endpoint")).expect("replacement identity");
    fixture.rewrite_descriptor(&descriptor_path, |descriptor| {
        descriptor["run_id"] = Value::from(replacement.run_id().as_str());
    });

    drop(publisher);

    assert!(descriptor_path.exists());
}

#[test]
fn exact_stale_descriptor_can_be_removed_then_replaced_after_a_private_probe() {
    let fixture = RegistryFixture::new();
    let stale_publisher = fixture.publish(ENDPOINT_A);
    let stale = fixture.scan().candidates[0].clone();
    let stale_socket = stale.socket_path().to_path_buf();
    let mut replacement = fixture.prepare(ENDPOINT_A);

    assert!(
        replacement
            .remove_stale_for_replacement(&stale)
            .expect("identity-safe stale removal")
    );
    assert!(!stale_publisher.descriptor_path().exists());
    assert!(!stale_socket.exists());

    replacement.publish().expect("replacement publication");
    let report = fixture.scan();
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(
        report.candidates[0].run_id(),
        replacement.identity().run_id()
    );
}

#[test]
fn stale_replacement_refuses_a_changed_descriptor_after_the_probe() {
    let fixture = RegistryFixture::new();
    let stale_publisher = fixture.publish(ENDPOINT_A);
    let stale: InstanceDescriptor = fixture.scan().candidates[0].clone();
    let descriptor_path = stale_publisher.descriptor_path().to_path_buf();
    let replacement_identity =
        InstanceIdentity::new(ENDPOINT_A.parse().expect("endpoint")).expect("replacement identity");
    let replacement_run_id = replacement_identity.run_id().as_str().to_owned();
    let mut replacement = fixture.prepare(ENDPOINT_A);

    let removed = replacement
        .remove_stale_for_replacement_with_observer(&stale, || {
            fixture.rewrite_descriptor(&descriptor_path, |descriptor| {
                descriptor["run_id"] = Value::from(replacement_run_id);
            });
        })
        .expect("locked stale replacement check");

    assert!(!removed);
    assert!(descriptor_path.exists());
}

#[test]
fn scanner_uses_a_bounded_index_instead_of_enumerating_untracked_directory_junk() {
    let fixture = RegistryFixture::new();
    let published = fixture.publish(ENDPOINT_A);
    let descriptor_path = published.descriptor_path().to_path_buf();
    let mut created = 0usize;
    for index in 0..=300_u16 {
        let path = fixture.instances_dir().join(format!("{index:064x}.json"));
        if path == descriptor_path {
            continue;
        }
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .expect("untracked descriptor-shaped junk");
        created += 1;
        if created == 300 {
            break;
        }
    }
    assert_eq!(created, 300);
    let mut enumerated = 0usize;

    let report = fixture
        .scanner()
        .scan_all_with_enumeration_observer(|| enumerated += 1)
        .expect("bounded indexed scan");

    assert_eq!(enumerated, 1);
    assert_eq!(report.omitted, 0);
    assert!(report.rejected.is_empty());
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), published.identity().run_id());
}

#[test]
fn registry_refuses_a_257th_descriptor_before_enumeration_can_grow_unbounded() {
    let fixture = RegistryFixture::new();
    let mut overflow = fixture.prepare("127.0.0.1:20256");
    let overflow_path = overflow.descriptor_path().to_path_buf();
    let mut created = 0usize;
    let mut tracked = Vec::new();
    for index in 0..=256_u16 {
        let path = fixture.instances_dir().join(format!("{index:064x}.json"));
        if path == overflow_path {
            continue;
        }
        tracked.push(
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("descriptor filename")
                .to_owned(),
        );
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .expect("bounded descriptor slot");
        file.sync_all().expect("sync descriptor slot");
        created += 1;
        if created == 256 {
            break;
        }
    }
    assert_eq!(created, 256);
    let index_path = fixture.instances_dir().join(".registry-index.json");
    let mut index_file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(index_path)
        .expect("registry index");
    serde_json::to_writer(
        &mut index_file,
        &serde_json::json!({"version": 1, "descriptors": tracked}),
    )
    .expect("seed full registry index");
    index_file.flush().expect("flush registry index");
    index_file.sync_all().expect("sync registry index");

    let error = overflow
        .publish()
        .expect_err("registry descriptor capacity");

    assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
    assert_eq!(
        error.to_string(),
        "instance registry descriptor capacity of 256 reached"
    );
    assert!(!overflow_path.exists());
}

#[test]
fn stale_cleanup_deduplicates_a_batch_and_syncs_the_registry_once() {
    let fixture = RegistryFixture::new();
    let _publishers = [
        fixture.publish("127.0.0.1:20300"),
        fixture.publish("127.0.0.1:20301"),
        fixture.publish("127.0.0.1:20302"),
    ];
    let mut stale = fixture.scan().candidates;
    stale.push(stale[0].clone());
    let cancelled = CancellationToken::new();
    let mut durable_syncs = 0usize;

    let removed = fixture
        .scanner()
        .remove_stale_batch_if_current(
            &stale,
            Instant::now() + Duration::from_secs(1),
            &cancelled,
            || durable_syncs += 1,
        )
        .expect("batched stale cleanup");

    assert_eq!(removed, 3);
    assert_eq!(durable_syncs, 1);
    assert!(fixture.scan().candidates.is_empty());
}

#[test]
fn initialize_migrates_a_valid_legacy_descriptor_into_the_index() {
    let fixture = RegistryFixture::new();
    let published = fixture.publish("127.0.0.1:20400");
    fs::remove_file(fixture.index_path()).expect("remove post-index registry metadata");

    let scanner = RegistryScanner::initialize(fixture.home()).expect("migrate legacy registry");
    let report = scanner.scan_all().expect("scan migrated registry");

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), published.identity().run_id());
}

#[test]
fn exact_endpoint_read_recovers_a_valid_crash_orphan_missing_from_the_index() {
    let fixture = RegistryFixture::new();
    let published = fixture.publish("127.0.0.1:20401");
    fixture.replace_index(&[]);

    let report = fixture
        .scanner()
        .read_endpoint(published.identity().proxy_endpoint())
        .expect("read deterministic crash orphan");

    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), published.identity().run_id());
}

#[test]
fn reconciliation_rejects_more_than_256_relevant_entries_deterministically() {
    let fixture = RegistryFixture::new();
    let prepared = fixture.prepare("127.0.0.1:20402");
    let mut created = 0usize;
    for index in 0..=257_u16 {
        let path = fixture.instances_dir().join(format!("{index:064x}.json"));
        if path == prepared.descriptor_path() {
            continue;
        }
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .expect("legacy descriptor-shaped entry");
        created += 1;
        if created == 257 {
            break;
        }
    }
    assert_eq!(created, 257);

    for _ in 0..2 {
        let error = RegistryScanner::initialize(fixture.home())
            .err()
            .expect("over-cap reconciliation must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
        assert_eq!(
            error.to_string(),
            "registry reconciliation exceeds 256 descriptor entries"
        );
    }
}

#[test]
fn reconciliation_stops_before_unrelated_directory_junk_can_be_unbounded() {
    let fixture = RegistryFixture::new();
    let _prepared = fixture.prepare("127.0.0.1:20403");
    for index in 0..300_u16 {
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(fixture.instances_dir().join(format!("junk-{index:04}")))
            .expect("unrelated registry junk");
    }
    let mut inspected = 0usize;
    let error =
        RegistryScanner::initialize_with_reconciliation_observer(fixture.home(), || inspected += 1)
            .err()
            .expect("bounded reconciliation rejects an overfull directory");

    assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
    assert!(inspected <= 259);
}

#[test]
fn index_rewrite_syncs_the_parent_handle_opened_before_the_atomic_rename() {
    let fixture = RegistryFixture::new();
    let _prepared = fixture.prepare("127.0.0.1:20404");
    let instances = fixture.instances_dir();
    let moved = fixture.run_dir().join("instances-moved");
    let mut index = read_registry_index(&instances).expect("registry index");

    write_registry_index(&instances, &mut index, || {
        fs::rename(&instances, &moved).expect("move registry directory after index rename");
    })
    .expect("sync already-open parent directory handle");

    assert!(moved.join(".registry-index.json").exists());
    fs::rename(&moved, &instances).expect("restore registry directory");
}
