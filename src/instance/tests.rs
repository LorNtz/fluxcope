use super::{InstanceIdentity, RunId, endpoint_hash, fluxcope_home_from_env};
use std::{
    ffi::OsStr,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
};

#[test]
fn unspecified_ipv4_endpoint_uses_loopback_local_url() {
    let identity =
        InstanceIdentity::new("0.0.0.0:8989".parse().expect("endpoint")).expect("identity");

    assert_eq!(identity.local_proxy_url(), "http://127.0.0.1:8989");
    assert_eq!(identity.proxy_endpoint(), "0.0.0.0:8989".parse().unwrap());
    assert_eq!(identity.run_id().decoded_len(), 16);
}

#[test]
fn local_proxy_url_is_canonical_for_concrete_and_unspecified_addresses() {
    let cases = [
        (
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9100),
            "http://127.0.0.1:9100",
        ),
        (
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)), 9101),
            "http://192.0.2.7:9101",
        ),
        (
            SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 9102),
            "http://[::1]:9102",
        ),
        (
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 9103),
            "http://[::1]:9103",
        ),
    ];

    for (endpoint, expected) in cases {
        let identity = InstanceIdentity::new(endpoint).expect("identity");
        assert_eq!(identity.local_proxy_url(), expected, "endpoint {endpoint}");
    }
}

#[test]
fn generated_run_id_is_canonical_base64url_and_round_trips_through_json() {
    let identity =
        InstanceIdentity::new("127.0.0.1:8989".parse().expect("endpoint")).expect("identity");
    let run_id = identity.run_id();

    assert_eq!(run_id.as_str().len(), 22);
    assert!(
        run_id
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    );
    assert!(!run_id.as_str().contains('='));

    let encoded = serde_json::to_string(run_id).expect("serialize run ID");
    let decoded: RunId = serde_json::from_str(&encoded).expect("deserialize run ID");
    assert_eq!(&decoded, run_id);
    assert_eq!(decoded.decoded_len(), 16);
}
#[test]
fn run_id_deserializes_from_owned_json_value() {
    let value = serde_json::json!("AAAAAAAAAAAAAAAAAAAAAA");

    let run_id: RunId = serde_json::from_value(value).expect("owned JSON run ID");

    assert_eq!(run_id.as_str(), "AAAAAAAAAAAAAAAAAAAAAA");
}

#[test]
fn run_id_parser_rejects_wrong_lengths_padding_alphabet_and_noncanonical_bits() {
    let invalid = [
        "",
        "AAAAAAAAAAAAAAAAAAAA",
        "AAAAAAAAAAAAAAAAAAAAAAA",
        "AAAAAAAAAAAAAAAAAAAAAA==",
        "/////////////////////w",
        "AAAAAAAAAAAAAAAAAAAAAB",
    ];

    for value in invalid {
        assert!(
            value.parse::<RunId>().is_err(),
            "invalid run ID was accepted: {value:?}"
        );
    }

    let valid = "AAAAAAAAAAAAAAAAAAAAAA"
        .parse::<RunId>()
        .expect("canonical 128-bit run ID");
    assert_eq!(valid.as_str(), "AAAAAAAAAAAAAAAAAAAAAA");
    assert_eq!(valid.decoded_len(), 16);
}

#[test]
fn endpoint_hash_is_stable_bounded_and_uses_the_canonical_socket_address() {
    let ipv4: SocketAddr = "127.0.0.1:8989".parse().expect("IPv4 endpoint");
    let expanded_ipv6: SocketAddr = "[0:0:0:0:0:0:0:1]:8989"
        .parse()
        .expect("expanded IPv6 endpoint");
    let compressed_ipv6: SocketAddr = "[::1]:8989".parse().expect("IPv6 endpoint");

    let ipv4_hash = endpoint_hash(ipv4);
    assert_eq!(ipv4_hash, endpoint_hash(ipv4));
    assert_eq!(endpoint_hash(expanded_ipv6), endpoint_hash(compressed_ipv6));
    assert_ne!(ipv4_hash, endpoint_hash(compressed_ipv6));
    assert_ne!(
        ipv4_hash,
        endpoint_hash("127.0.0.1:8990".parse().expect("other endpoint"))
    );
    assert_eq!(ipv4_hash.len(), 64);
    assert!(ipv4_hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(ipv4_hash, ipv4_hash.to_ascii_lowercase());
}

#[test]
fn per_user_fluxcope_home_is_platform_independent_and_never_uses_cwd() {
    let home =
        fluxcope_home_from_env(Some(OsStr::new("test-user-home"))).expect("per-user Fluxcope home");

    assert_eq!(home, PathBuf::from("test-user-home").join(".fluxcope"));
    assert!(fluxcope_home_from_env(None).is_err());
    assert_ne!(home, PathBuf::from("."));
}
