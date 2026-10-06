mod common;

use common::*;
use ledgerzero_engine::domain::{AccountType, EntityCategory, EntityRelationshipKind};
use ledgerzero_engine::engine::{
    ChartTemplate, EngineState, NewAccount, NewEntityRelationship, NewReferencedEntity,
    ReverseEntry,
};
use ledgerzero_engine::types::FixedClock;
use ledgerzero_engine::AccountingEngine;
use serde_json::{json, Value};

#[test]
fn referenced_entities_are_not_accounting_subjects_and_replay_stably() {
    let mut fx = fixture();
    let op = id();
    let property_spec = NewReferencedEntity {
        name: "Elm House".into(),
        category: EntityCategory::Property,
    };
    let property = fx
        .engine
        .create_referenced_entity(op, fx.actor, property_spec.clone())
        .unwrap();
    assert_eq!(
        fx.engine
            .create_referenced_entity(op, fx.actor, property_spec)
            .unwrap(),
        property
    );
    let unit = fx
        .engine
        .create_referenced_entity(
            id(),
            fx.actor,
            NewReferencedEntity {
                name: "Unit 1".into(),
                category: EntityCategory::Unit,
            },
        )
        .unwrap();
    assert!(fx
        .engine
        .create_chart(
            id(),
            fx.actor,
            ledgerzero_engine::engine::NewChart {
                entity_id: property,
                name: "Not a second chart".into(),
                description: None,
                activate: false,
                starter_template: ChartTemplate::Empty,
                resource_type_id: None,
            }
        )
        .is_err());
    let owns = fx
        .engine
        .create_entity_relationship(
            id(),
            fx.actor,
            NewEntityRelationship {
                from_entity_id: fx.entity,
                to_entity_id: property,
                kind: EntityRelationshipKind::Owns,
                effective_from: date("2026-01-01"),
                effective_to: None,
            },
        )
        .unwrap();
    fx.engine
        .create_entity_relationship(
            id(),
            fx.actor,
            NewEntityRelationship {
                from_entity_id: property,
                to_entity_id: unit,
                kind: EntityRelationshipKind::Contains,
                effective_from: date("2026-01-01"),
                effective_to: None,
            },
        )
        .unwrap();
    assert!(fx
        .engine
        .create_entity_relationship(
            id(),
            fx.actor,
            NewEntityRelationship {
                from_entity_id: property,
                to_entity_id: unit,
                kind: EntityRelationshipKind::Owns,
                effective_from: date("2026-01-01"),
                effective_to: None,
            }
        )
        .is_err());
    let replayed = AccountingEngine::from_state(
        EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        Box::new(FixedClock::new(1_752_000_000_000)),
    );
    assert!(replayed.get_entity(fx.entity).unwrap().is_subject);
    assert!(!replayed.get_entity(property).unwrap().is_subject);
    assert_eq!(
        replayed
            .list_entity_relationships()
            .iter()
            .find(|item| item.relationship_id == owns)
            .unwrap()
            .to_entity_id,
        property
    );
}

#[test]
fn dedicated_account_requires_matching_line_attribution_and_reversal_preserves_it() {
    let mut fx = fixture();
    let property = fx
        .engine
        .create_referenced_entity(
            id(),
            fx.actor,
            NewReferencedEntity {
                name: "Elm House".into(),
                category: EntityCategory::Property,
            },
        )
        .unwrap();
    let dedicated = fx
        .engine
        .create_account(
            id(),
            fx.actor,
            NewAccount {
                chart_id: fx.chart,
                name: "Elm rent".into(),
                code: None,
                account_type: AccountType::Revenue,
                resource_type_id: fx.usd,
                parent_account_id: None,
                associated_entity_id: Some(property),
                validation_rules: Value::Null,
                metadata: Value::Null,
            },
        )
        .unwrap();
    let mut entry = fx.entry(
        "2026-01-10",
        "Rent",
        vec![debit(fx.cash, "100"), credit(dedicated, "100")],
    );
    assert!(fx.engine.post_entry(fx.actor, entry.clone()).is_err());
    entry.lines[1].attribution_entity_id = Some(fx.entity);
    assert!(fx.engine.post_entry(fx.actor, entry.clone()).is_err());
    entry.lines[1].attribution_entity_id = Some(property);
    let entry_id = fx.engine.post_entry(fx.actor, entry).unwrap();
    let reversal_id = fx
        .engine
        .reverse_entry(
            fx.actor,
            ReverseEntry {
                new_entry_id: id(),
                original_entry_id: entry_id,
                entry_date: date("2026-01-11"),
                description: None,
                metadata: Value::Null,
            },
        )
        .unwrap();
    assert_eq!(
        fx.engine.get_entry(reversal_id).unwrap().lines[1].attribution_entity_id,
        Some(property)
    );
    let replayed = AccountingEngine::from_state(
        EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        Box::new(FixedClock::new(1_752_000_000_000)),
    );
    assert_eq!(
        replayed
            .get_account(dedicated)
            .unwrap()
            .associated_entity_id,
        Some(property)
    );
    assert_eq!(
        replayed.get_entry(entry_id).unwrap().lines[1].attribution_entity_id,
        Some(property)
    );
}

#[test]
fn legacy_events_default_to_subject_and_no_attribution() {
    let fx = fixture();
    let mut event = serde_json::to_value(&fx.engine.audit_log()[0]).unwrap();
    event["payload"]["entity"]
        .as_object_mut()
        .unwrap()
        .remove("is_subject");
    event["payload"]["entity"]
        .as_object_mut()
        .unwrap()
        .remove("category");
    let old: ledgerzero_engine::domain::EventRecord = serde_json::from_value(event).unwrap();
    let json = serde_json::to_value(old).unwrap();
    assert_eq!(json["payload"]["entity"]["is_subject"], json!(true));
    assert_eq!(json["payload"]["entity"]["category"], json!("OTHER"));
}
