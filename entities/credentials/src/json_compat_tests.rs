//! Verifies the hand-written `credentials` types parse and re-emit the
//! exact same JSON shape as the `protobuf`-generated `proto_gen` types did
//! - the actual regression net for `credentials.json`
//! (`service_loader::loaders::load_credentials`) staying readable across
//! the migration off protobuf. Deleted once `proto_gen` is removed.

use crate::{credentials as old, entity as credentials};

fn assert_same_json(old_json: &str, new: &credentials::Authentication) {
    let new_json = serde_json::to_string(new).expect("new type serializes");

    let old_value: serde_json::Value = serde_json::from_str(old_json).expect("old JSON parses");
    let new_value: serde_json::Value = serde_json::from_str(&new_json).expect("new JSON parses");
    assert_eq!(
        old_value, new_value,
        "old proto JSON {old_json:?} and new serde JSON {new_json:?} must match"
    );

    let round_tripped: credentials::Authentication =
        serde_json::from_str(old_json).expect("new type parses the old proto's JSON");
    assert_eq!(&round_tripped, new);
}

#[test]
fn basic_credentials_round_trip() {
    let mut old = old::Authentication::new();
    let mut basic = old::BasicCredentials::new();
    basic.username = "alice".into();
    basic.password = "hunter2".into();
    old.set_basic(basic);
    let old_json = protobuf_json_mapping::print_to_string(&old).unwrap();

    let new = credentials::Authentication::Basic(credentials::BasicCredentials {
        username: "alice".into(),
        password: "hunter2".into(),
    });

    assert_same_json(&old_json, &new);
}

#[test]
fn header_credentials_round_trip() {
    let mut old = old::Authentication::new();
    let mut header = old::HeaderCredentials::new();
    header.value = "shhh".into();
    old.set_header(header);
    let old_json = protobuf_json_mapping::print_to_string(&old).unwrap();

    let new = credentials::Authentication::Header(credentials::HeaderCredentials {
        value: "shhh".into(),
    });

    assert_same_json(&old_json, &new);
}

#[test]
fn oauth_credentials_round_trip_with_access_token() {
    let mut old = old::Authentication::new();
    let mut oauth = old::OAuthCredentials::new();
    oauth.clientId = "client-1".into();
    oauth.clientSecret = "secret-1".into();
    oauth.accessToken = Some("token-1".into());
    old.set_oauth(oauth);
    let old_json = protobuf_json_mapping::print_to_string(&old).unwrap();

    let new = credentials::Authentication::Oauth(credentials::OAuthCredentials {
        client_id: "client-1".into(),
        client_secret: "secret-1".into(),
        access_token: Some("token-1".into()),
    });

    assert_same_json(&old_json, &new);
}

#[test]
fn oauth_credentials_round_trip_without_access_token() {
    let mut old = old::Authentication::new();
    let mut oauth = old::OAuthCredentials::new();
    oauth.clientId = "client-1".into();
    oauth.clientSecret = "secret-1".into();
    old.set_oauth(oauth);
    let old_json = protobuf_json_mapping::print_to_string(&old).unwrap();
    assert!(
        !old_json.contains("accessToken"),
        "unset optional field should be omitted by the old proto JSON mapping: {old_json}"
    );

    let new = credentials::Authentication::Oauth(credentials::OAuthCredentials {
        client_id: "client-1".into(),
        client_secret: "secret-1".into(),
        access_token: None,
    });

    assert_same_json(&old_json, &new);
}

#[test]
fn multi_header_credentials_round_trip() {
    let mut old = old::Authentication::new();
    let mut multi = old::MultiHeaderCredentials::new();
    multi.values.insert("X-One".into(), "1".into());
    multi.values.insert("X-Two".into(), "2".into());
    old.set_multiHeader(multi);
    let old_json = protobuf_json_mapping::print_to_string(&old).unwrap();

    let new = credentials::Authentication::MultiHeader(credentials::MultiHeaderCredentials {
        values: [
            ("X-One".to_owned(), "1".to_owned()),
            ("X-Two".to_owned(), "2".to_owned()),
        ]
        .into_iter()
        .collect(),
    });

    assert_same_json(&old_json, &new);
}

#[test]
fn query_and_path_credentials_round_trip() {
    let mut old_query = old::Authentication::new();
    let mut query = old::QueryCredentials::new();
    query.value = "q".into();
    old_query.set_query(query);
    let old_query_json = protobuf_json_mapping::print_to_string(&old_query).unwrap();
    assert_same_json(
        &old_query_json,
        &credentials::Authentication::Query(credentials::QueryCredentials { value: "q".into() }),
    );

    let mut old_path = old::Authentication::new();
    let mut path = old::PathCredentials::new();
    path.value = "p".into();
    old_path.set_path(path);
    let old_path_json = protobuf_json_mapping::print_to_string(&old_path).unwrap();
    assert_same_json(
        &old_path_json,
        &credentials::Authentication::Path(credentials::PathCredentials { value: "p".into() }),
    );
}
