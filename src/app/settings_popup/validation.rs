use crate::{
    mapping::validate_proxy_settings,
    settings::{AppSettings, ProxySettings},
};

pub(super) fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    if settings.server.port == 0 {
        return Err("server.port must be between 1 and 65535".to_string());
    }

    if let Some(proxy) = &settings.proxy {
        validate_proxy(proxy)?;
    }

    Ok(())
}

fn validate_proxy(proxy: &ProxySettings) -> Result<(), String> {
    validate_proxy_settings(proxy)
        .into_iter()
        .next()
        .map_or(Ok(()), |diagnostic| Err(diagnostic.message))
}
