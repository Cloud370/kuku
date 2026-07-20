use kuku_server::api::{ContextCatalog, TierCatalogEntry, TierSummary};

#[test]
fn catalog_fixture_is_typed_and_searchable_fields_are_public() {
    let catalog: ContextCatalog = serde_json::from_value(serde_json::json!({
        "api_version": 1,
        "revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "tiers": [{"tier": {
            "tier_id": "tier:balanced",
            "label": "Balanced",
            "purpose": "General work",
            "provider": "fixture-provider",
            "model": "fixture-model",
            "think": null,
            "is_default": true
        }}],
        "skills": [],
        "agents": [],
        "tools": []
    }))
    .unwrap();
    assert_eq!(1, catalog.tiers.len());
    let TierCatalogEntry {
        tier: TierSummary { tier_id, .. },
    } = &catalog.tiers[0];
    assert_eq!("tier:balanced", tier_id);
}
