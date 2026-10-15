//! The committed OpenAPI document is the generated one. Regenerate with
//! `ML_WRITE_OPENAPI=1 cargo test -p ml-serve --test openapi`.

#[test]
fn the_committed_document_is_the_generated_one() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json");
    let generated = format!("{}\n", ml_serve::spec());
    if std::env::var_os("ML_WRITE_OPENAPI").is_some() {
        std::fs::write(path, &generated).expect("write openapi.json");
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == generated,
        "openapi.json is stale; regenerate it with ML_WRITE_OPENAPI=1"
    );
}

#[test]
fn the_document_names_every_route_and_the_key() {
    let spec: serde_json::Value = serde_json::from_str(ml_serve::spec()).unwrap();
    let paths = spec["paths"].as_object().unwrap();
    for route in [
        "/v1/authorizations",
        "/v1/contexts",
        "/v1/contexts/{ctx}",
        "/v1/contexts/{ctx}/payment",
        "/v1/contexts/{ctx}/settlement",
        "/v1/contexts/{ctx}/compensation",
        "/v1/contexts/{ctx}/delivery",
        "/v1/contexts/{ctx}/expiry",
        "/v1/contexts/{ctx}/evidence",
        "/v1/mandates/{mandate}/revocation",
        "/v1/events",
        "/healthz",
        "/readyz",
    ] {
        assert!(paths.contains_key(route), "missing {route}");
    }
    assert_eq!(
        spec["components"]["securitySchemes"]["api_key"]["scheme"],
        "bearer"
    );
    // Every /v1 operation requires the key; the ops routes do not.
    for (route, item) in paths {
        for (_, operation) in item.as_object().unwrap() {
            let secured = operation["security"].is_array();
            assert_eq!(secured, route.starts_with("/v1/"), "{route}");
        }
    }
}
