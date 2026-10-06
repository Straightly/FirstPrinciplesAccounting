mod common;

use common::*;
use ledgerzero_engine::domain::AccountType;
use ledgerzero_engine::engine::{
    EngineState, ImportPropertyIdentity, NewAccount, PrepareImportProperties,
};
use ledgerzero_engine::types::FixedClock;
use ledgerzero_engine::AccountingEngine;
use serde_json::Value;

#[test]
fn six_property_preparation_is_atomic_stable_and_balances_stay_separate() {
    let mut fx = fixture();
    let spec = PrepareImportProperties {
        subject_id: fx.entity,
        entity_namespace: "zag-properties".into(),
        properties: (1..=6)
            .map(|n| ImportPropertyIdentity {
                external_key: format!("property-{n}"),
                name: format!("Property {n}"),
                effective_from: date("2026-01-01"),
            })
            .collect(),
    };
    let mut conflicting = spec.clone();
    conflicting.properties[5].external_key = "property-1".into();
    assert!(fx
        .engine
        .prepare_import_properties(id(), fx.actor, conflicting)
        .is_err());
    assert_eq!(fx.engine.list_entities().len(), 1);
    let op_id = id();
    fx.engine
        .prepare_import_properties(op_id, fx.actor, spec.clone())
        .unwrap();
    fx.engine
        .prepare_import_properties(op_id, fx.actor, spec.clone())
        .unwrap();
    fx.engine
        .prepare_import_properties(id(), fx.actor, spec)
        .unwrap();
    assert_eq!(fx.engine.list_entities().len(), 7);
    assert_eq!(fx.engine.list_entity_relationships().len(), 6);

    let mut lines = Vec::new();
    let mut accounts = Vec::new();
    for n in 1..=6 {
        let property = fx
            .engine
            .find_import_entity("zag-properties", &format!("property-{n}"))
            .unwrap()
            .entity_id;
        let account = fx
            .engine
            .create_account(
                id(),
                fx.actor,
                NewAccount {
                    chart_id: fx.chart,
                    name: format!("Property {n} asset"),
                    code: None,
                    account_type: AccountType::Asset,
                    resource_type_id: fx.usd,
                    parent_account_id: None,
                    associated_entity_id: Some(property),
                    validation_rules: Value::Null,
                    metadata: Value::Null,
                },
            )
            .unwrap();
        let mut line = debit(account, "10");
        line.attribution_entity_id = Some(property);
        lines.push(line);
        accounts.push(account);
    }
    lines.push(credit(fx.capital, "60"));
    fx.engine
        .post_entry(fx.actor, fx.entry("2026-01-15", "Six properties", lines))
        .unwrap();
    for account in accounts {
        assert_eq!(
            fx.engine.get_balance(account).unwrap().debit_total,
            amt("10")
        );
    }
    let replayed = AccountingEngine::from_state(
        EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        Box::new(FixedClock::new(1_752_000_000_000)),
    );
    assert_eq!(replayed.list_entities().len(), 7);
    assert_eq!(replayed.list_entity_relationships().len(), 6);
}
