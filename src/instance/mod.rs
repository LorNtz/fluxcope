use std::{
    ffi::OsStr,
    fmt::{self, Write as _},
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    str::FromStr,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest as _, Sha256};

const RUN_ID_BYTES: usize = 16;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RunId(String);

impl RunId {
    fn generate() -> anyhow::Result<Self> {
        let mut bytes = [0_u8; RUN_ID_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow::anyhow!("failed to generate run ID: {error}"))?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn decoded_len(&self) -> usize {
        RUN_ID_BYTES
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunIdParseError;

impl fmt::Display for RunIdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("run ID must be canonical unpadded base64url encoding of 16 bytes")
    }
}

impl std::error::Error for RunIdParseError {}

impl FromStr for RunId {
    type Err = RunIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 22
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(RunIdParseError);
        }
        let decoded = URL_SAFE_NO_PAD.decode(value).map_err(|_| RunIdParseError)?;
        if decoded.len() != RUN_ID_BYTES || URL_SAFE_NO_PAD.encode(&decoded) != value {
            return Err(RunIdParseError);
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for RunId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = <&str>::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InstanceIdentity {
    proxy_endpoint: SocketAddr,
    local_proxy_url: String,
    run_id: RunId,
    started_at: DateTime<Utc>,
}

impl InstanceIdentity {
    pub(crate) fn new(proxy_endpoint: SocketAddr) -> anyhow::Result<Self> {
        Ok(Self {
            proxy_endpoint,
            local_proxy_url: local_proxy_url(proxy_endpoint),
            run_id: RunId::generate()?,
            started_at: Utc::now(),
        })
    }

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
}

pub(crate) fn endpoint_hash(endpoint: SocketAddr) -> String {
    let digest = Sha256::digest(endpoint.to_string().as_bytes());
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hash, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hash
}

pub(crate) fn local_proxy_url(endpoint: SocketAddr) -> String {
    let connect_ip = match endpoint.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(connect_ip, endpoint.port()))
}

pub(crate) fn wirelens_home_dir() -> io::Result<PathBuf> {
    wirelens_home_from_env(std::env::var_os("HOME").as_deref())
}

pub(crate) fn wirelens_home_from_env(home: Option<&OsStr>) -> io::Result<PathBuf> {
    home.map(PathBuf::from)
        .map(|path| path.join(".wirelens"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "HOME environment variable is not set",
            )
        })
}

pub(crate) mod lock;

pub(crate) use lock::DefaultConfigLease;

#[cfg(test)]
mod tests;
