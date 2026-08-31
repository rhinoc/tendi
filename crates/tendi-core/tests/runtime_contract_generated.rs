use serde_json::json;
use tendi_core::generated::runtime_contract::{
    validate_event, validate_request, validate_response, validate_result, CommandName,
    RuntimeEventEnvelope,
};

#[test]
fn generated_request_validator_enforces_schema_constraints() {
    assert!(validate_request(
        "sessions_search",
        &json!({ "query": "session" }),
    )
    .is_ok());
    assert!(validate_request(
        "sessions_search",
        &json!({ "query": "", "candidates": null }),
    )
    .is_err());
    assert!(validate_request(
        "sessions_search",
        &json!({ "query": "session", "unexpected": true }),
    )
    .is_err());
}

#[test]
fn generated_result_validator_enforces_required_fields() {
    assert!(validate_result(
        "sessions_scan_start",
        &json!({ "generation": 1, "started": true }),
    )
    .is_ok());
    assert!(validate_result("sessions_scan_start", &json!({ "started": true })).is_err());
}

#[test]
fn generated_event_validator_checks_event_payload() {
    let valid = RuntimeEventEnvelope {
        id: 1,
        event: "analytics://revision".to_string(),
        payload: serde_json::from_value(json!({ "scopeKey": "workspace:test", "revision": 1 }))
            .expect("payload object"),
        scope_key: None,
        domain: None,
        operation_id: None,
        base_revision: None,
        revision: None,
        source_version: None,
    };
    assert!(validate_event(&valid).is_ok());

    let mut invalid = valid;
    invalid.payload.insert("unexpected".to_string(), json!(true));
    assert!(validate_event(&invalid).is_err());
}

#[test]
fn generated_response_validator_enforces_the_json_rpc_union() {
    assert!(validate_response(&json!({
        "jsonrpc": "2.0",
        "id": "request-1",
        "result": {}
    }))
    .is_ok());
    assert!(validate_response(&json!({
        "jsonrpc": "2.0",
        "id": "request-2",
        "error": { "code": -32600, "message": "invalid" }
    }))
    .is_ok());
    assert!(validate_response(&json!({
        "jsonrpc": "2.0",
        "id": "request-3"
    }))
    .is_err());
    assert!(validate_response(&json!({
        "jsonrpc": "2.0",
        "id": "request-4",
        "result": {},
        "error": { "code": -32600, "message": "invalid" }
    }))
    .is_err());
}

#[test]
fn generated_valid_and_invalid_fixtures_cover_rust_boundaries() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../runtime-schema/examples/contract-fixtures.json"
    ))
    .expect("contract fixtures are valid JSON");

    for (name, request) in fixtures["validRequests"].as_object().expect("valid requests").iter() {
        assert!(CommandName::parse(name).is_some(), "unknown fixture command: {name}");
        assert!(validate_request(name, request).is_ok(), "valid request rejected: {name}");
    }
    for (name, request) in fixtures["invalidRequests"].as_object().expect("invalid requests").iter() {
        assert!(validate_request(name, request).is_err(), "invalid request accepted: {name}");
    }
    for (name, result) in fixtures["validResults"].as_object().expect("valid results").iter() {
        validate_result(name, result).unwrap_or_else(|error| panic!("valid result rejected: {name}: {error}"));
    }
    for (name, event) in fixtures["validEvents"].as_object().expect("valid events").iter() {
        let event: RuntimeEventEnvelope = serde_json::from_value(event.clone()).expect("valid event envelope");
        assert!(validate_event(&event).is_ok(), "valid event rejected: {name}");
    }
    for (name, event) in fixtures["invalidEvents"].as_object().expect("invalid events").iter() {
        let event: Result<RuntimeEventEnvelope, _> = serde_json::from_value(event.clone());
        assert!(event.is_err() || validate_event(&event.expect("checked above")).is_err(), "invalid event accepted: {name}");
    }
}
