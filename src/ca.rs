// src/ca.rs
use crate::private_fs;
use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, IsCa, KeyPair,
    KeyUsagePurpose,
};
#[cfg(test)]
use std::fs;
use std::io::Read;
use std::path::Path;

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

        let mut cert_der = Vec::new();
        private_fs::open_file(&cert_path, false)?
            .read_to_end(&mut cert_der)
            .with_context(|| format!("failed to read CA certificate {}", cert_path.display()))?;
        let mut key_der = Vec::new();
        private_fs::open_file(&key_path, false)?
            .read_to_end(&mut key_der)
            .with_context(|| format!("failed to read CA private key {}", key_path.display()))?;
        let mut cert_pem = String::new();
        private_fs::open_file(&pem_path, false)?
            .read_to_string(&mut cert_pem)
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
    private_fs::ensure_directory(cert_dir)?;
    let cert_path = cert_dir.join(CA_CERT_FILE);
    let key_path = cert_dir.join(CA_KEY_FILE);
    let pem_path = cert_dir.join(pem_filename);

    if cert_path.exists() || key_path.exists() || pem_path.exists() {
        log::info!("Loading existing CA certificate from {:?}", cert_dir);
        let data = CaData::from_disk(cert_dir, pem_filename)
            .with_context(|| format!("failed to load CA from {}", cert_dir.display()))?;
        log::info!("Successfully loaded existing CA certificate");
        return Ok(data);
    }

    let (cert, key) = create_ca()?;
    let data = CaData::from_cert(&cert, &key)?;

    data.save(cert_dir, pem_filename)?;
    log::info!("CA certificate saved to {:?}", cert_dir);

    Ok(data)
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
}
