// src/ca.rs
use crate::private_fs;
use anyhow::{Context, Result};
use fs2::FileExt;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, IsCa, KeyPair,
    KeyUsagePurpose,
};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
};

// File names (not paths) - the directory is added separately
const CA_CERT_FILE: &str = "ca_cert.der";
const CA_KEY_FILE: &str = "ca_key.der";

/// Represents the CA certificate data - either DER bytes or PEM for export
pub struct CaData {
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
    cert_pem: String,
}

impl CaData {
    /// Create new CaData from a Certificate
    fn from_cert(cert: &Certificate, key: &KeyPair) -> Result<Self> {
        let cert_der = cert.der().to_vec();
        let key_der = key.serialize_der();
        let cert_pem = cert.pem();
        Ok(CaData {
            cert_der,
            key_der,
            cert_pem,
        })
    }

    /// Load CaData from DER files on disk
    fn from_disk(cert_dir: &Path, pem_filename: &str) -> Result<Self> {
        let cert_path = cert_dir.join(CA_CERT_FILE);
        let key_path = cert_dir.join(CA_KEY_FILE);
        let pem_path = cert_dir.join(pem_filename);

        let cert_der = read_owner_only_file(&cert_path)
            .with_context(|| format!("failed to read CA certificate {}", cert_path.display()))?;
        let key_der = read_owner_only_file(&key_path)
            .with_context(|| format!("failed to read CA private key {}", key_path.display()))?;
        let cert_pem = String::from_utf8(read_owner_only_file(&pem_path)?)
            .with_context(|| format!("failed to read CA PEM {}", pem_path.display()))?;

        Ok(CaData {
            cert_der,
            key_der,
            cert_pem,
        })
    }

    /// Save to disk
    fn save(&self, cert_dir: &Path, pem_filename: &str) -> Result<()> {
        private_fs::ensure_directory(cert_dir)
            .with_context(|| format!("failed to create CA directory {}", cert_dir.display()))?;
        let cert_path = cert_dir.join(CA_CERT_FILE);
        let key_path = cert_dir.join(CA_KEY_FILE);
        let pem_path = cert_dir.join(pem_filename);
        private_fs::write_file(&cert_path, &self.cert_der)
            .with_context(|| format!("failed to persist CA certificate {}", cert_path.display()))?;
        private_fs::write_file(&key_path, &self.key_der)
            .with_context(|| format!("failed to persist CA private key {}", key_path.display()))?;
        private_fs::write_file(&pem_path, self.cert_pem.as_bytes())
            .with_context(|| format!("failed to persist CA PEM {}", pem_path.display()))?;
        File::open(cert_dir)?.sync_all()?;
        Ok(())
    }

    /// Get DER-encoded certificate bytes
    pub fn cert_der(&self) -> Vec<u8> {
        self.cert_der.clone()
    }

    /// Get DER-encoded private key bytes
    pub fn key_der(&self) -> Vec<u8> {
        self.key_der.clone()
    }

    /// Get PEM-encoded certificate (for export)
    pub fn cert_pem(&self) -> String {
        self.cert_pem.clone()
    }
}

/// Create or load CA certificate from disk
pub fn create_or_load_ca(cert_dir: &Path, pem_filename: &str) -> Result<CaData> {
    anyhow::ensure!(
        Path::new(pem_filename).components().count() == 1
            && matches!(
                Path::new(pem_filename).components().next(),
                Some(std::path::Component::Normal(_))
            ),
        "CA PEM filename must be a single filename"
    );
    private_fs::ensure_directory(cert_dir)
        .with_context(|| format!("failed to prepare CA directory {}", cert_dir.display()))?;
    let lock = private_fs::open_file(&cert_dir.join(".ca-init.lock"), true).with_context(|| {
        format!(
            "failed to open CA initialization lock in {}",
            cert_dir.display()
        )
    })?;
    lock.lock_exclusive()
        .context("failed to lock CA initialization")?;

    let result = (|| {
        let cert_path = cert_dir.join(CA_CERT_FILE);
        let key_path = cert_dir.join(CA_KEY_FILE);
        let pem_path = cert_dir.join(pem_filename);
        if path_present(&cert_path)? || path_present(&key_path)? || path_present(&pem_path)? {
            log::info!("Loading existing CA certificate from {:?}", cert_dir);
            return CaData::from_disk(cert_dir, pem_filename)
                .with_context(|| format!("failed to load CA from {}", cert_dir.display()));
        }

        let (cert, key) = create_ca()?;
        let data = CaData::from_cert(&cert, &key)?;
        data.save(cert_dir, pem_filename)?;
        log::info!("CA certificate saved to {:?}", cert_dir);
        Ok(data)
    })();
    let unlock_result = FileExt::unlock(&lock).context("failed to unlock CA initialization");
    match (result, unlock_result) {
        (Ok(data), Ok(())) => Ok(data),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn path_present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_owner_only_file(path: &Path) -> io::Result<Vec<u8>> {
    let file = private_fs::open_file(path, false)?;
    let metadata = file.metadata()?;
    const MAX_AUTHORITY_FILE_BYTES: u64 = 2 * 1024 * 1024;
    if metadata.len() > MAX_AUTHORITY_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "authority output is too large",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_AUTHORITY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_AUTHORITY_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "authority output is too large",
        ));
    }
    Ok(bytes)
}


fn create_ca() -> Result<(Certificate, KeyPair)> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Fluxcope CA");
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];

    let key = KeyPair::generate().context("failed to generate CA key")?;
    let certificate = params
        .self_signed(&key)
        .context("failed to generate CA certificate")?;
    Ok((certificate, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_certificate_persistence() {
        let cert_dir = tempfile::tempdir().expect("temporary CA directory should be created");
        let pem_filename = "fluxcope-ca.pem";

        // First run - should create new certificate
        let ca1 = create_or_load_ca(cert_dir.path(), pem_filename)
            .expect("first CA creation should succeed");
        let cert_pem1 = ca1.cert_pem();

        // Verify files were created
        assert!(
            cert_dir.path().join(CA_CERT_FILE).exists(),
            "Certificate DER file should exist"
        );
        assert!(
            cert_dir.path().join(CA_KEY_FILE).exists(),
            "Key DER file should exist"
        );
        assert!(
            cert_dir.path().join(pem_filename).exists(),
            "Certificate PEM file should exist"
        );

        // Second run - should load existing certificate
        let ca2 =
            create_or_load_ca(cert_dir.path(), pem_filename).expect("persisted CA should load");
        let cert_pem2 = ca2.cert_pem();

        // Certificates should match
        assert_eq!(
            cert_pem1, cert_pem2,
            "Loaded certificate should match original"
        );

        // DER bytes should also match
        assert_eq!(ca1.cert_der(), ca2.cert_der(), "DER bytes should match");
        assert_eq!(ca1.key_der(), ca2.key_der(), "Key DER bytes should match");
    }

    #[test]
    fn incomplete_persisted_authority_is_fatal() {
        let cert_dir = tempfile::tempdir().expect("temporary CA directory should be created");
        let pem_filename = "fluxcope-ca.pem";
        fs::write(cert_dir.path().join(CA_CERT_FILE), b"incomplete")
            .expect("partial CA file should be written");

        let error = create_or_load_ca(cert_dir.path(), pem_filename)
            .err()
            .expect("partial persisted CA should fail");

        assert!(error.to_string().contains("failed to load CA"));
        assert!(!cert_dir.path().join(CA_KEY_FILE).exists());
    }

    #[test]
    fn concurrent_initialization_publishes_one_complete_shared_authority() {
        use std::sync::{Arc, Barrier};

        let cert_dir = tempfile::tempdir().expect("temporary CA directory");
        let cert_path = Arc::new(cert_dir.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(3));
        let callers = (0..2)
            .map(|_| {
                let cert_path = Arc::clone(&cert_path);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    create_or_load_ca(&cert_path, "wirelens-ca.pem")
                })
            })
            .collect::<Vec<_>>();

        barrier.wait();
        let authorities = callers
            .into_iter()
            .map(|caller| {
                caller
                    .join()
                    .expect("CA initialization thread should not panic")
                    .expect("concurrent CA initialization should succeed")
            })
            .collect::<Vec<_>>();

        assert_eq!(authorities[0].cert_der(), authorities[1].cert_der());
        assert_eq!(authorities[0].key_der(), authorities[1].key_der());
        assert_eq!(authorities[0].cert_pem(), authorities[1].cert_pem());
        assert_eq!(
            fs::read(cert_dir.path().join(CA_CERT_FILE)).expect("persisted certificate"),
            authorities[0].cert_der()
        );
        assert_eq!(
            fs::read(cert_dir.path().join(CA_KEY_FILE)).expect("persisted private key"),
            authorities[0].key_der()
        );
        assert_eq!(
            fs::read_to_string(cert_dir.path().join("wirelens-ca.pem"))
                .expect("persisted PEM certificate"),
            authorities[0].cert_pem()
        );
        assert!(!authorities[0].cert_der().is_empty());
        assert!(!authorities[0].key_der().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn shared_authority_directory_lock_and_outputs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("temporary root");
        let cert_dir = root.path().join("authority");
        create_or_load_ca(&cert_dir, "wirelens-ca.pem").expect("CA initialization");

        assert_eq!(
            fs::metadata(&cert_dir)
                .expect("CA directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in [
            CA_CERT_FILE,
            CA_KEY_FILE,
            "wirelens-ca.pem",
            ".ca-init.lock",
        ] {
            let path = cert_dir.join(name);
            let metadata = fs::symlink_metadata(&path).expect("authority file metadata");
            assert!(metadata.is_file(), "{name} must be a regular file");
            assert!(
                !metadata.file_type().is_symlink(),
                "{name} must not be a symlink"
            );
            assert_eq!(
                metadata.permissions().mode() & 0o777,
                0o600,
                "{name} must be owner-only"
            );
        }
    }
}
