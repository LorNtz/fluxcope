// src/ca.rs
use rcgen::{Certificate, BasicConstraints, IsCa, KeyUsagePurpose, CertificateParams, DistinguishedName};

pub fn create_ca() -> Certificate { // Returns (Cert PEM, Private Key PEM)
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(rcgen::DnType::CommonName, "Proxy TUI CA");
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];

    let cert = Certificate::from_params(params).unwrap();
    // (cert.serialize_pem().unwrap(), cert.serialize_private_key_pem())
    cert
}
