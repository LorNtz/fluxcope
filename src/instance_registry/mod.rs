use std::{
    collections::HashSet,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    net::SocketAddr,
    os::unix::{
        fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    time::Instant,
};

use chrono::{DateTime, Utc};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{
    instance::{InstanceIdentity, RunId, endpoint_hash, local_proxy_url},
    settings::{ConfigMode, PersistenceMode, SettingsSession},
};

const DESCRIPTOR_SCHEMA_VERSION: u16 = 1;
const DESCRIPTOR_RPC_VERSION: u16 = 1;
const MAX_DESCRIPTOR_BYTES: u64 = 64 * 1024;
const MAX_SCAN_FILES: usize = 256;
const MAX_RECONCILE_DIRECTORY_ENTRIES: usize = MAX_SCAN_FILES + 2;
const SOCKET_HASH_HEX_BYTES: usize = 24;
const REGISTRY_INDEX_VERSION: u16 = 1;
const REGISTRY_INDEX_FILENAME: &str = ".registry-index.json";
const REGISTRY_INDEX_TEMP_FILENAME: &str = ".registry-index.tmp";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryIndex {
    version: u16,
    descriptors: Vec<String>,
}

impl Default for RegistryIndex {
    fn default() -> Self {
        Self {
            version: REGISTRY_INDEX_VERSION,
            descriptors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceDescriptor {
    schema_version: u16,
    rpc_version: u16,
    binary_version: String,
    pid: u32,
    proxy_endpoint: SocketAddr,
    local_proxy_url: String,
    run_id: RunId,
    started_at: DateTime<Utc>,
    socket_path: PathBuf,
    config_mode: ConfigMode,
    persistence: PersistenceMode,
    config_source: Option<PathBuf>,
}

impl InstanceDescriptor {
    pub(crate) fn proxy_endpoint(&self) -> SocketAddr {
        self.proxy_endpoint
    }

    pub(crate) fn local_proxy_url(&self) -> &str {
        &self.local_proxy_url
    }

    pub(crate) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub(crate) fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub(crate) fn config_mode(&self) -> ConfigMode {
        self.config_mode
    }

    pub(crate) fn persistence(&self) -> PersistenceMode {
        self.persistence
    }

    pub(crate) fn config_source(&self) -> Option<&Path> {
        self.config_source.as_deref()
    }

    pub(crate) fn binary_version(&self) -> &str {
        &self.binary_version
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DiscoveryDiagnostic {
    code: &'static str,
    message: String,
}

impl DiscoveryDiagnostic {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > 512 {
            let mut boundary = 512;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
        }
        Self { code, message }
    }
    pub(crate) fn from_parts(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

    pub(crate) fn code(&self) -> &'static str {
        self.code
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

pub(crate) struct RegistryScan {
    pub(crate) candidates: Vec<InstanceDescriptor>,
    pub(crate) rejected: Vec<DiscoveryDiagnostic>,
    pub(crate) omitted: usize,
}

pub(crate) struct RegistryMutationLock {
    _file: File,
}

impl RegistryMutationLock {
    pub(crate) fn acquire(wirelens_home: &Path) -> io::Result<Self> {
        Self::open(wirelens_home, true)
    }

    pub(crate) fn try_acquire(wirelens_home: &Path) -> io::Result<Self> {
        Self::open(wirelens_home, false)
    }

    fn open(wirelens_home: &Path, blocking: bool) -> io::Result<Self> {
        ensure_owner_only_directory(wirelens_home)?;
        let run_dir = wirelens_home.join("run");
        ensure_owner_only_directory(&run_dir)?;
        let path = run_dir.join(".registry-mutation.lock");
        let nofollow = (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32;
        let open_existing = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(nofollow)
                .open(&path)
        };
        let file = match open_existing() {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(nofollow)
                    .open(&path)
                {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => open_existing()?,
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "registry mutation lock must be an owner-owned regular file",
            ));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        if blocking {
            file.lock_exclusive()?;
        } else {
            file.try_lock_exclusive()?;
        }
        let path_metadata = fs::symlink_metadata(&path)?;
        let locked_metadata = file.metadata()?;
        if !path_metadata.file_type().is_file()
            || path_metadata.dev() != locked_metadata.dev()
            || path_metadata.ino() != locked_metadata.ino()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "registry mutation lock path changed while acquiring it",
            ));
        }
        Ok(Self { _file: file })
    }
}

fn read_registry_index(instances_root: &Path) -> io::Result<RegistryIndex> {
    let path = instances_root.join(REGISTRY_INDEX_FILENAME);
    let nofollow = (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(nofollow)
        .open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > MAX_DESCRIPTOR_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "registry index must be an owner-only bounded regular file",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    Read::by_ref(&mut file)
        .take(MAX_DESCRIPTOR_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DESCRIPTOR_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "registry index exceeds 64 KiB",
        ));
    }
    let mut index: RegistryIndex = serde_json::from_slice(&bytes).map_err(json_to_io)?;
    if index.version != REGISTRY_INDEX_VERSION
        || index
            .descriptors
            .iter()
            .any(|name| !is_descriptor_name(name))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "registry index is invalid",
        ));
    }
    index.descriptors.sort_unstable();
    index.descriptors.dedup();
    Ok(index)
}

fn write_registry_index<O>(
    instances_root: &Path,
    index: &mut RegistryIndex,
    observer: O,
) -> io::Result<()>
where
    O: FnOnce(),
{
    validate_owner_only_directory(instances_root)?;
    index.descriptors.sort_unstable();
    index.descriptors.dedup();
    if index.descriptors.len() > MAX_SCAN_FILES {
        return Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "instance registry descriptor capacity of 256 reached",
        ));
    }
    let temporary_path = instances_root.join(REGISTRY_INDEX_TEMP_FILENAME);
    let index_path = instances_root.join(REGISTRY_INDEX_FILENAME);
    match fs::remove_file(&temporary_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = File::open(instances_root)?;
    let result = (|| {
        let mut temporary = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary_path)?;
        temporary.set_permissions(fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(&mut temporary, index).map_err(json_to_io)?;
        temporary.flush()?;
        temporary.sync_all()?;
        fs::rename(&temporary_path, &index_path)?;
        observer();
        parent.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn ensure_registry_index(instances_root: &Path) -> io::Result<()> {
    match read_registry_index(instances_root) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_registry_index(instances_root, &mut RegistryIndex::default(), || {})
        }
        Err(error) => Err(error),
    }
}

fn reconcile_registry_index<O>(
    run_root: &Path,
    instances_root: &Path,
    mut observer: O,
) -> io::Result<()>
where
    O: FnMut(),
{
    let mut relevant = Vec::with_capacity(MAX_SCAN_FILES + 1);
    let mut visited = 0usize;
    for entry in fs::read_dir(instances_root)? {
        observer();
        visited = visited.saturating_add(1);
        if visited > MAX_RECONCILE_DIRECTORY_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "registry reconciliation directory entry bound exceeded",
            ));
        }
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_descriptor_name(&name) {
            continue;
        }
        relevant.push(name);
        if relevant.len() > MAX_SCAN_FILES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "registry reconciliation exceeds 256 descriptor entries",
            ));
        }
    }
    relevant.sort_unstable();
    let scanner = RegistryScanner::new_from_run_root(run_root)?;
    let mut index = read_registry_index(instances_root)?;
    if index.descriptors.len() > MAX_SCAN_FILES {
        return Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "instance registry descriptor capacity of 256 reached",
        ));
    }
    let mut changed = false;
    for name in relevant {
        if index.descriptors.binary_search(&name).is_ok() {
            continue;
        }
        if scanner.read_descriptor(&instances_root.join(&name)).is_ok() {
            index.descriptors.push(name);
            index.descriptors.sort_unstable();
            changed = true;
        }
    }
    if changed {
        write_registry_index(instances_root, &mut index, || {})?;
    }
    Ok(())
}

pub(crate) struct RegistryPublisher {
    identity: InstanceIdentity,
    descriptor: InstanceDescriptor,
    descriptor_path: PathBuf,
    socket_path: PathBuf,
    listener: Option<UnixListener>,
    published: bool,
    wirelens_home: PathBuf,
    cleanup_attempted: bool,
}

impl RegistryPublisher {
    pub(crate) fn prepare(
        wirelens_home: &Path,
        identity: InstanceIdentity,
        settings: &SettingsSession,
    ) -> io::Result<Self> {
        let _scanner = RegistryScanner::initialize(wirelens_home)?;
        let run_dir = wirelens_home.join("run");
        let instances_dir = run_dir.join("instances");

        let descriptor_path = instances_dir.join(descriptor_name(identity.proxy_endpoint()));
        let socket_path = run_dir.join(socket_name(identity.proxy_endpoint(), identity.run_id()));
        let listener = UnixListener::bind(&socket_path)?;
        let preparation = (|| {
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
            validate_socket(
                &socket_path,
                &run_dir,
                identity.proxy_endpoint(),
                identity.run_id(),
            )
            .map_err(diagnostic_to_io)?;
            File::open(&run_dir)?.sync_all()
        })();
        if let Err(error) = preparation {
            let _ = fs::remove_file(&socket_path);
            return Err(error);
        }

        let context = settings.ui_context();
        let descriptor = InstanceDescriptor {
            schema_version: DESCRIPTOR_SCHEMA_VERSION,
            rpc_version: DESCRIPTOR_RPC_VERSION,
            binary_version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            proxy_endpoint: identity.proxy_endpoint(),
            local_proxy_url: identity.local_proxy_url().to_owned(),
            run_id: identity.run_id().clone(),
            started_at: identity.started_at(),
            socket_path: socket_path.clone(),
            config_mode: context.config_mode,
            persistence: context.persistence,
            config_source: settings.source_path().map(Path::to_path_buf),
        };

        Ok(Self {
            identity,
            descriptor,
            descriptor_path,
            socket_path,
            listener: Some(listener),
            published: false,
            wirelens_home: wirelens_home.to_path_buf(),
            cleanup_attempted: false,
        })
    }

    pub(crate) fn publish(&mut self) -> io::Result<()> {
        let _mutation_lock = RegistryMutationLock::acquire(&self.wirelens_home)?;
        let parent = self.descriptor_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "descriptor path has no parent")
        })?;
        validate_owner_only_directory(parent)?;
        match fs::symlink_metadata(&self.descriptor_path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "endpoint descriptor already exists and requires a liveness probe",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let descriptor_name = self
            .descriptor_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "descriptor has no valid filename",
                )
            })?
            .to_owned();
        let mut index = read_registry_index(parent)?;
        index
            .descriptors
            .retain(|name| fs::symlink_metadata(parent.join(name)).is_ok());
        if !index.descriptors.contains(&descriptor_name)
            && index.descriptors.len() >= MAX_SCAN_FILES
        {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "instance registry descriptor capacity of 256 reached",
            ));
        }
        let temporary_path = parent.join(format!(
            ".{}.{}.tmp",
            descriptor_name,
            self.identity.run_id()
        ));
        let mut linked = false;
        let result = (|| {
            let mut temporary = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary_path)?;
            temporary.set_permissions(fs::Permissions::from_mode(0o600))?;
            serde_json::to_writer(&mut temporary, &self.descriptor).map_err(json_to_io)?;
            temporary.flush()?;
            temporary.sync_all()?;
            fs::hard_link(&temporary_path, &self.descriptor_path)?;
            linked = true;
            fs::remove_file(&temporary_path)?;
            index.descriptors.push(descriptor_name);
            write_registry_index(parent, &mut index, || {})
        })();
        if result.is_ok() {
            self.published = true;
        } else {
            let _ = fs::remove_file(&temporary_path);
            if linked {
                let _ = fs::remove_file(&self.descriptor_path);
            }
        }
        result
    }

    fn remove_stale_for_replacement_internal<O, G>(
        &mut self,
        expected: &InstanceDescriptor,
        observer: O,
        guard: G,
    ) -> io::Result<bool>
    where
        O: FnOnce(),
        G: FnOnce() -> bool,
    {
        if expected.proxy_endpoint != self.identity.proxy_endpoint() {
            return Ok(false);
        }
        let _mutation_lock = RegistryMutationLock::acquire(&self.wirelens_home)?;
        observer();
        let scanner = RegistryScanner::new(&self.wirelens_home)?;
        let current = match scanner.parse_descriptor(&self.descriptor_path) {
            Ok(descriptor) => descriptor,
            Err(_) => return Ok(false),
        };
        if current.proxy_endpoint != expected.proxy_endpoint
            || current.run_id != expected.run_id
            || current.socket_path != expected.socket_path
        {
            return Ok(false);
        }
        if !guard() {
            return Ok(false);
        }
        fs::remove_file(&self.descriptor_path)?;
        match fs::remove_file(&current.socket_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if let Some(parent) = self.descriptor_path.parent() {
            let descriptor_name = self
                .descriptor_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "descriptor has no valid filename",
                    )
                })?;
            let mut index = read_registry_index(parent)?;
            index.descriptors.retain(|name| name != descriptor_name);
            write_registry_index(parent, &mut index, || {})?;
        }
        Ok(true)
    }

    pub(crate) fn remove_stale_for_replacement(
        &mut self,
        expected: &InstanceDescriptor,
    ) -> io::Result<bool> {
        self.remove_stale_for_replacement_internal(expected, || {}, || true)
    }

    pub(crate) fn remove_stale_for_replacement_if<G>(
        &mut self,
        expected: &InstanceDescriptor,
        guard: G,
    ) -> io::Result<bool>
    where
        G: FnOnce() -> bool,
    {
        self.remove_stale_for_replacement_internal(expected, || {}, guard)
    }

    #[cfg(test)]
    pub(crate) fn remove_stale_for_replacement_with_observer<F>(
        &mut self,
        expected: &InstanceDescriptor,
        observer: F,
    ) -> io::Result<bool>
    where
        F: FnOnce(),
    {
        self.remove_stale_for_replacement_internal(expected, observer, || true)
    }

    fn cleanup_internal<F>(&mut self, observer: F) -> io::Result<()>
    where
        F: FnOnce(),
    {
        if self.cleanup_attempted {
            return Ok(());
        }
        let _mutation_lock = RegistryMutationLock::acquire(&self.wirelens_home)?;
        if !self.published {
            let _ = fs::remove_file(&self.socket_path);
            self.cleanup_attempted = true;
            return Ok(());
        }
        let scanner = RegistryScanner::new(&self.wirelens_home)?;
        let descriptor = match scanner.parse_descriptor(&self.descriptor_path) {
            Ok(descriptor) => descriptor,
            Err(_) => {
                self.cleanup_attempted = true;
                return Ok(());
            }
        };
        observer();
        if descriptor.proxy_endpoint == self.identity.proxy_endpoint()
            && descriptor.run_id == *self.identity.run_id()
        {
            fs::remove_file(&self.descriptor_path)?;
            fs::remove_file(&self.socket_path)?;
            if let Some(parent) = self.descriptor_path.parent() {
                let descriptor_name = self
                    .descriptor_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "descriptor has no valid filename",
                        )
                    })?;
                let mut index = read_registry_index(parent)?;
                index.descriptors.retain(|name| name != descriptor_name);
                write_registry_index(parent, &mut index, || {})?;
            }
        }
        self.cleanup_attempted = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn cleanup_with_observer<F>(&mut self, observer: F) -> io::Result<()>
    where
        F: FnOnce(),
    {
        self.cleanup_internal(observer)
    }

    pub(crate) fn take_listener(&mut self) -> io::Result<UnixListener> {
        self.listener.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "private control listener was already taken",
            )
        })
    }

    pub(crate) fn identity(&self) -> &InstanceIdentity {
        &self.identity
    }

    pub(crate) fn descriptor_path(&self) -> &Path {
        &self.descriptor_path
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub(crate) fn wirelens_home(&self) -> &Path {
        &self.wirelens_home
    }
}

impl Drop for RegistryPublisher {
    fn drop(&mut self) {
        let _ = self.cleanup_internal(|| {});
    }
}

pub(crate) struct RegistryScanner {
    run_root: PathBuf,
    instances_root: PathBuf,
    effective_uid: u32,
}

impl RegistryScanner {
    pub(crate) fn initialize(wirelens_home: &Path) -> io::Result<Self> {
        Self::initialize_internal(wirelens_home, || {})
    }

    fn initialize_internal<O>(wirelens_home: &Path, observer: O) -> io::Result<Self>
    where
        O: FnMut(),
    {
        ensure_owner_only_directory(wirelens_home)?;
        let run_root = wirelens_home.join("run");
        let instances_root = run_root.join("instances");
        ensure_owner_only_directory(&run_root)?;
        ensure_owner_only_directory(&instances_root)?;
        let _mutation_lock = RegistryMutationLock::acquire(wirelens_home)?;
        ensure_registry_index(&instances_root)?;
        reconcile_registry_index(&run_root, &instances_root, observer)?;
        Self::new_from_run_root(&run_root)
    }

    #[cfg(test)]
    pub(crate) fn initialize_with_reconciliation_observer<O>(
        wirelens_home: &Path,
        observer: O,
    ) -> io::Result<Self>
    where
        O: FnMut(),
    {
        Self::initialize_internal(wirelens_home, observer)
    }

    pub(crate) fn new(wirelens_home: &Path) -> io::Result<Self> {
        Self::new_from_run_root(&wirelens_home.join("run"))
    }

    fn new_from_run_root(run_root: &Path) -> io::Result<Self> {
        validate_owner_only_directory(run_root)?;
        let instances_root = run_root.join("instances");
        validate_owner_only_directory(&instances_root)?;
        Ok(Self {
            run_root: fs::canonicalize(run_root)?,
            instances_root: fs::canonicalize(instances_root)?,
            effective_uid: rustix::process::geteuid().as_raw(),
        })
    }

    pub(crate) fn scan_all(&self) -> io::Result<RegistryScan> {
        self.scan_all_internal(|| {})
    }

    fn scan_all_internal<O>(&self, mut observer: O) -> io::Result<RegistryScan>
    where
        O: FnMut(),
    {
        let index = read_registry_index(&self.instances_root)?;
        let omitted = index.descriptors.len().saturating_sub(MAX_SCAN_FILES);
        let mut scan = RegistryScan {
            candidates: Vec::new(),
            rejected: Vec::new(),
            omitted,
        };
        for name in index.descriptors.into_iter().take(MAX_SCAN_FILES) {
            observer();
            self.inspect(&self.instances_root.join(name), &mut scan);
        }
        Ok(scan)
    }

    #[cfg(test)]
    pub(crate) fn scan_all_with_enumeration_observer<O>(
        &self,
        observer: O,
    ) -> io::Result<RegistryScan>
    where
        O: FnMut(),
    {
        self.scan_all_internal(observer)
    }

    pub(crate) fn read_endpoint(&self, endpoint: SocketAddr) -> io::Result<RegistryScan> {
        let path = self.instances_root.join(descriptor_name(endpoint));
        let mut scan = RegistryScan {
            candidates: Vec::new(),
            rejected: Vec::new(),
            omitted: 0,
        };
        match fs::symlink_metadata(&path) {
            Ok(_) => self.inspect(&path, &mut scan),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(scan)
    }

    fn remove_stale_batch_internal<O>(
        &self,
        expected: &[InstanceDescriptor],
        deadline: Instant,
        cancelled: &CancellationToken,
        observer: O,
    ) -> io::Result<usize>
    where
        O: FnOnce(),
    {
        let wirelens_home = self.run_root.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "registry run root has no Wirelens home parent",
            )
        })?;
        let _mutation_lock = RegistryMutationLock::acquire(wirelens_home)?;
        if cancelled.is_cancelled() || Instant::now() >= deadline {
            return Ok(0);
        }
        let mut index = read_registry_index(&self.instances_root)?;
        let mut seen = HashSet::with_capacity(expected.len().min(MAX_SCAN_FILES));
        let mut removed = 0usize;
        let mut deferred_error = None;
        for stale in expected {
            let identity = (
                stale.proxy_endpoint(),
                stale.run_id().as_str(),
                stale.socket_path(),
            );
            if !seen.insert(identity) {
                continue;
            }
            let descriptor_path = self
                .instances_root
                .join(descriptor_name(stale.proxy_endpoint()));
            let current = match self.parse_descriptor(&descriptor_path) {
                Ok(descriptor) => descriptor,
                Err(_) => continue,
            };
            if current.proxy_endpoint != stale.proxy_endpoint
                || current.run_id != *stale.run_id()
                || current.socket_path != stale.socket_path()
            {
                continue;
            }
            if cancelled.is_cancelled() || Instant::now() >= deadline {
                break;
            }
            if let Err(error) = fs::remove_file(&descriptor_path) {
                if error.kind() != io::ErrorKind::NotFound {
                    deferred_error = Some(error);
                    break;
                }
                continue;
            }
            let stop_after_descriptor = cancelled.is_cancelled() || Instant::now() >= deadline;
            if !stop_after_descriptor {
                match fs::remove_file(&current.socket_path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        deferred_error = Some(error);
                    }
                }
            }
            let name = descriptor_name(current.proxy_endpoint);
            index.descriptors.retain(|candidate| candidate != &name);
            removed = removed.saturating_add(1);
            if stop_after_descriptor || deferred_error.is_some() {
                break;
            }
        }
        if removed > 0 {
            write_registry_index(&self.instances_root, &mut index, observer)?;
        }
        if let Some(error) = deferred_error {
            return Err(error);
        }
        Ok(removed)
    }

    pub(crate) fn remove_stale_batch_if_current<O>(
        &self,
        expected: &[InstanceDescriptor],
        deadline: Instant,
        cancelled: &CancellationToken,
        observer: O,
    ) -> io::Result<usize>
    where
        O: FnOnce(),
    {
        self.remove_stale_batch_internal(expected, deadline, cancelled, observer)
    }

    fn inspect(&self, path: &Path, scan: &mut RegistryScan) {
        match self.read_descriptor(path) {
            Ok(descriptor) => scan.candidates.push(descriptor),
            Err(diagnostic) => scan.rejected.push(diagnostic),
        }
    }

    fn parse_descriptor(&self, path: &Path) -> Result<InstanceDescriptor, DiscoveryDiagnostic> {
        let nofollow = (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(nofollow)
            .open(path)
            .map_err(|error| {
                if fs::symlink_metadata(path)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    DiscoveryDiagnostic::new("descriptor_symlink", "descriptor is a symlink")
                } else {
                    DiscoveryDiagnostic::new(
                        "descriptor_open",
                        format!("failed to open descriptor: {error}"),
                    )
                }
            })?;
        let metadata = file.metadata().map_err(|error| {
            DiscoveryDiagnostic::new(
                "descriptor_metadata",
                format!("failed to inspect descriptor: {error}"),
            )
        })?;
        validate_descriptor_metadata(&metadata, self.effective_uid)?;

        let capacity = usize::try_from(metadata.len()).unwrap_or(0);
        let mut bytes = Vec::with_capacity(capacity);
        Read::by_ref(&mut file)
            .take(MAX_DESCRIPTOR_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                DiscoveryDiagnostic::new(
                    "descriptor_read",
                    format!("failed to read descriptor: {error}"),
                )
            })?;
        if bytes.len() as u64 > MAX_DESCRIPTOR_BYTES {
            return Err(DiscoveryDiagnostic::new(
                "descriptor_too_large",
                "descriptor exceeds 64 KiB",
            ));
        }
        let descriptor: InstanceDescriptor = serde_json::from_slice(&bytes).map_err(|error| {
            DiscoveryDiagnostic::new(
                "descriptor_invalid_json",
                format!("invalid descriptor JSON: {error}"),
            )
        })?;
        Ok(descriptor)
    }

    fn read_descriptor(&self, path: &Path) -> Result<InstanceDescriptor, DiscoveryDiagnostic> {
        let descriptor = self.parse_descriptor(path)?;
        self.validate_descriptor(path, descriptor)
    }

    fn validate_descriptor(
        &self,
        path: &Path,
        descriptor: InstanceDescriptor,
    ) -> Result<InstanceDescriptor, DiscoveryDiagnostic> {
        if descriptor.schema_version != DESCRIPTOR_SCHEMA_VERSION {
            return Err(DiscoveryDiagnostic::new(
                "descriptor_schema_version",
                "unsupported descriptor schema version",
            ));
        }
        if descriptor.rpc_version != DESCRIPTOR_RPC_VERSION {
            return Err(DiscoveryDiagnostic::new(
                "descriptor_rpc_version",
                "unsupported descriptor RPC version",
            ));
        }
        if descriptor.binary_version.is_empty() || descriptor.binary_version.len() > 128 {
            return Err(DiscoveryDiagnostic::new(
                "descriptor_binary_version",
                "invalid descriptor binary version",
            ));
        }
        if descriptor.local_proxy_url != local_proxy_url(descriptor.proxy_endpoint) {
            return Err(DiscoveryDiagnostic::new(
                "descriptor_local_proxy_url",
                "local proxy URL does not match endpoint",
            ));
        }
        if path.file_name().and_then(|name| name.to_str())
            != Some(descriptor_name(descriptor.proxy_endpoint).as_str())
        {
            return Err(DiscoveryDiagnostic::new(
                "descriptor_name_mismatch",
                "descriptor filename does not match endpoint",
            ));
        }
        validate_socket(
            &descriptor.socket_path,
            &self.run_root,
            descriptor.proxy_endpoint,
            &descriptor.run_id,
        )?;
        Ok(descriptor)
    }
}

pub(crate) fn validate_descriptor_metadata(
    metadata: &Metadata,
    effective_uid: u32,
) -> Result<(), DiscoveryDiagnostic> {
    if !metadata.file_type().is_file() {
        return Err(DiscoveryDiagnostic::new(
            "descriptor_not_regular",
            "descriptor is not a regular file",
        ));
    }
    if metadata.uid() != effective_uid {
        return Err(DiscoveryDiagnostic::new(
            "descriptor_wrong_owner",
            "descriptor is owned by another user",
        ));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(DiscoveryDiagnostic::new(
            "descriptor_permissions",
            "descriptor mode must be 0600",
        ));
    }
    if metadata.len() > MAX_DESCRIPTOR_BYTES {
        return Err(DiscoveryDiagnostic::new(
            "descriptor_too_large",
            "descriptor exceeds 64 KiB",
        ));
    }
    Ok(())
}

fn descriptor_name(endpoint: SocketAddr) -> String {
    format!("{}.json", endpoint_hash(endpoint))
}

fn socket_name(endpoint: SocketAddr, run_id: &RunId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(endpoint.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(run_id.as_str().as_bytes());
    let digest = hasher.finalize();
    let mut name = String::with_capacity(SOCKET_HASH_HEX_BYTES + 5);
    for byte in digest.iter().take(SOCKET_HASH_HEX_BYTES / 2) {
        use std::fmt::Write as _;
        write!(&mut name, "{byte:02x}").expect("writing to a String cannot fail");
    }
    name.push_str(".sock");
    name
}

fn is_descriptor_name(name: &str) -> bool {
    name.len() == 69
        && name.ends_with(".json")
        && name[..64]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_socket(
    socket_path: &Path,
    run_root: &Path,
    endpoint: SocketAddr,
    run_id: &RunId,
) -> Result<(), DiscoveryDiagnostic> {
    let metadata = fs::symlink_metadata(socket_path).map_err(|error| {
        DiscoveryDiagnostic::new("socket_missing", format!("invalid socket path: {error}"))
    })?;
    if !metadata.file_type().is_socket() {
        return Err(DiscoveryDiagnostic::new(
            "socket_not_socket",
            "declared socket path is not a Unix socket",
        ));
    }
    let canonical_root = fs::canonicalize(run_root).map_err(|error| {
        DiscoveryDiagnostic::new("socket_path_escape", format!("invalid run root: {error}"))
    })?;
    let canonical_socket = fs::canonicalize(socket_path).map_err(|error| {
        DiscoveryDiagnostic::new("socket_missing", format!("invalid socket path: {error}"))
    })?;
    if !canonical_socket.starts_with(&canonical_root) {
        return Err(DiscoveryDiagnostic::new(
            "socket_path_escape",
            "socket path escapes the registry root",
        ));
    }
    if canonical_socket.file_name().and_then(|name| name.to_str())
        != Some(socket_name(endpoint, run_id).as_str())
    {
        return Err(DiscoveryDiagnostic::new(
            "socket_name_mismatch",
            "socket filename does not match endpoint and run ID",
        ));
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o777 != 0o600 {
        return Err(DiscoveryDiagnostic::new(
            "socket_permissions",
            "socket must be owner-only",
        ));
    }
    Ok(())
}

fn ensure_owner_only_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "directory has no parent")
            })?;
            fs::create_dir_all(parent)?;
            match builder.create(path) {
                Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "registry directory must be a real directory owned by the effective user",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    validate_owner_only_directory(path)
}

fn validate_owner_only_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "registry directory must be a real owner-only directory",
        ));
    }
    Ok(())
}

fn diagnostic_to_io(diagnostic: DiscoveryDiagnostic) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, diagnostic.message)
}

fn json_to_io(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests;
