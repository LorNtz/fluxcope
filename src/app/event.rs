use crate::proxy_handler::CapturedData;

#[derive(Debug)]
pub enum AppEvent {
    NetworkRequest(CapturedData),
    LogMessage(String),
    CertificateDownloadReady(String),
}
