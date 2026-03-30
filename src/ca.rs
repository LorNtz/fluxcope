// src/ca.rs
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, IsCa, KeyUsagePurpose,
};
use std::fs;
use std::path::PathBuf;

// File names (not paths) - the directory is added separately
const CA_CERT_FILE: &str = "ca_cert.der";
const CA_KEY_FILE: &str = "ca_key.der";
const CA_CERT_PEM: &str = "ca_cert.pem";

// Default directory for certificate storage
const CA_DIR: &str = ".certificate";

/// Represents the CA certificate data - either DER bytes or PEM for export
pub struct CaData {
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
    cert_pem: String,
}

impl CaData {
    /// Create new CaData from a Certificate
    fn from_cert(cert: &Certificate) -> Self {
        let cert_der = cert.serialize_der().unwrap();
        let key_der = cert.serialize_private_key_der();
        let cert_pem = cert.serialize_pem().unwrap();
        CaData {
            cert_der,
            key_der,
            cert_pem,
        }
    }

    /// Load CaData from DER files on disk
    fn from_disk(cert_dir: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let cert_path = cert_dir.join(CA_CERT_FILE);
        let key_path = cert_dir.join(CA_KEY_FILE);
        let pem_path = cert_dir.join(CA_CERT_PEM);

        let cert_der = fs::read(cert_path)?;
        let key_der = fs::read(key_path)?;
        let cert_pem = fs::read_to_string(pem_path)?;

        Ok(CaData {
            cert_der,
            key_der,
            cert_pem,
        })
    }

    /// Save to disk
    fn save(&self, cert_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(cert_dir)?;
        fs::write(cert_dir.join(CA_CERT_FILE), &self.cert_der)?;
        fs::write(cert_dir.join(CA_KEY_FILE), &self.key_der)?;
        fs::write(cert_dir.join(CA_CERT_PEM), &self.cert_pem)?;
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
pub fn create_or_load_ca() -> CaData {
    let cert_dir = PathBuf::from(CA_DIR);
    let cert_path = cert_dir.join(CA_CERT_FILE);
    let key_path = cert_dir.join(CA_KEY_FILE);

    // Try to load existing certificate
    if cert_path.exists() && key_path.exists() {
        log::info!("Loading existing CA certificate from {:?}", cert_dir);
        match CaData::from_disk(&cert_dir) {
            Ok(data) => {
                log::info!("Successfully loaded existing CA certificate");
                return data;
            }
            Err(e) => {
                log::warn!("Failed to load existing CA certificate: {}", e);
                log::info!("Creating new CA certificate...");
            }
        }
    }

    // Create new certificate
    let cert = create_ca();
    let data = CaData::from_cert(&cert);

    // Save to disk
    if let Err(e) = data.save(&cert_dir) {
        log::error!("Failed to save CA certificate: {}", e);
    } else {
        log::info!("CA certificate saved to {:?}", cert_dir);
    }

    data
}

fn create_ca() -> Certificate {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Proxy TUI CA");
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];

    Certificate::from_params(params).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_certificate_persistence() {
        let cert_dir = PathBuf::from(".proxy-tui_test");

        // Clean up before test
        let _ = fs::remove_dir_all(&cert_dir);

        // First run - should create new certificate
        let ca1 = create_or_load_ca_custom(&cert_dir);
        let cert_pem1 = ca1.cert_pem();

        // Verify files were created
        assert!(
            cert_dir.join(CA_CERT_FILE).exists(),
            "Certificate DER file should exist"
        );
        assert!(
            cert_dir.join(CA_KEY_FILE).exists(),
            "Key DER file should exist"
        );
        assert!(
            cert_dir.join(CA_CERT_PEM).exists(),
            "Certificate PEM file should exist"
        );

        // Second run - should load existing certificate
        let ca2 = create_or_load_ca_custom(&cert_dir);
        let cert_pem2 = ca2.cert_pem();

        // Certificates should match
        assert_eq!(
            cert_pem1, cert_pem2,
            "Loaded certificate should match original"
        );

        // DER bytes should also match
        assert_eq!(ca1.cert_der(), ca2.cert_der(), "DER bytes should match");
        assert_eq!(ca1.key_der(), ca2.key_der(), "Key DER bytes should match");

        // Clean up
        let _ = fs::remove_dir_all(&cert_dir);
    }

    fn create_or_load_ca_custom(cert_dir: &PathBuf) -> CaData {
        let cert_path = cert_dir.join(CA_CERT_FILE);
        let key_path = cert_dir.join(CA_KEY_FILE);

        if cert_path.exists() && key_path.exists() {
            return CaData::from_disk(cert_dir).unwrap();
        }

        let cert = create_ca();
        let data = CaData::from_cert(&cert);
        data.save(cert_dir).unwrap();
        data
    }
}
