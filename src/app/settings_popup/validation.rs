use crate::settings::{AppSettings, mapping_ops::validate_mapping_candidate};

pub(super) fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    if settings.server.port == 0 {
        return Err("server.port must be between 1 and 65535".to_string());
    }

    validate_proxy(settings.proxy.as_ref())?;

    Ok(())
}

fn validate_proxy(proxy: Option<&crate::settings::ProxySettings>) -> Result<(), String> {
    validate_mapping_candidate(proxy)
        .diagnostics
        .into_iter()
        .next()
        .map_or(Ok(()), |diagnostic| Err(diagnostic.message))
}
