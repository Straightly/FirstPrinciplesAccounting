use ledgerzero_engine::domain::{AccountType, ResourceKind};
use ledgerzero_engine::engine::{
    AccountingEngine, ChartTemplate, CopyChart, EngineState, NewChart, NewResourceType,
};
use ledgerzero_engine::types::FixedClock;
use ledgerzero_engine::ErrorCode;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

struct Fixture {
    engine: AccountingEngine,
    book_id: Uuid,
    actor: Uuid,
    entity_id: Uuid,
    usd_id: Uuid,
    inventory_id: Uuid,
}

fn fixture() -> Fixture {
    let book_id = Uuid::new_v4();
    let actor = Uuid::new_v4();
    let mut engine = AccountingEngine::new(book_id, Box::new(FixedClock::new(1_752_000_000_000)));
    let entity_id = engine
        .create_entity(Uuid::new_v4(), actor, "Acme Corporation")
        .unwrap();
    let usd_id = engine
        .create_resource_type(
            Uuid::new_v4(),
            actor,
            NewResourceType {
                name: "US Dollar".into(),
                kind: ResourceKind::Currency,
                code: "USD".into(),
                unit_of_measure: "USD".into(),
                precision: 2,
                metadata: Value::Null,
            },
        )
        .unwrap();
    let inventory_id = engine
        .create_resource_type(
            Uuid::new_v4(),
            actor,
            NewResourceType {
                name: "Widget".into(),
                kind: ResourceKind::Inventory,
                code: "WIDGET".into(),
                unit_of_measure: "each".into(),
                precision: 0,
                metadata: Value::Null,
            },
        )
        .unwrap();
    Fixture {
        engine,
        book_id,
        actor,
        entity_id,
        usd_id,
        inventory_id,
    }
}

fn corporate_spec(fx: &Fixture, name: &str) -> NewChart {
    NewChart {
        entity_id: fx.entity_id,
        name: name.into(),
        description: Some("Corporate starter".into()),
        activate: true,
        starter_template: ChartTemplate::Corporate,
        resource_type_id: Some(fx.usd_id),
    }
}

#[test]
fn corporate_starter_creates_exact_tree_and_replays() {
    let mut fx = fixture();
    let chart_id = fx
        .engine
        .create_chart(Uuid::new_v4(), fx.actor, corporate_spec(&fx, "Primary"))
        .unwrap();

    let accounts = fx.engine.list_accounts(chart_id);
    assert_eq!(accounts.len(), 13);
    let by_name: BTreeMap<_, _> = accounts.iter().map(|a| (a.name.as_str(), *a)).collect();
    let expected = [
        ("Assets", AccountType::Asset, None),
        ("Current Assets", AccountType::Asset, Some("Assets")),
        ("Cash", AccountType::Asset, Some("Current Assets")),
        ("Checking Account", AccountType::Asset, Some("Cash")),
        ("Liabilities", AccountType::Liability, None),
        ("Equity", AccountType::Equity, None),
        ("Common Stock", AccountType::Equity, Some("Equity")),
        ("Retained Earnings", AccountType::Equity, Some("Equity")),
        (
            "Additional Paid-in Capital",
            AccountType::Equity,
            Some("Equity"),
        ),
        ("Revenue", AccountType::Revenue, None),
        (
            "Sales or Service Revenue",
            AccountType::Revenue,
            Some("Revenue"),
        ),
        ("Expenses", AccountType::Expense, None),
        ("Operating Expenses", AccountType::Expense, Some("Expenses")),
    ];

    for (name, account_type, parent_name) in expected {
        let account = by_name
            .get(name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(account.account_type, account_type, "type for {name}");
        assert_eq!(account.normal_balance, account_type.normal_balance());
        assert_eq!(account.resource_type_id, fx.usd_id);
        assert_eq!(account.code, None);
        assert!(account.is_active);
        assert_eq!(
            account.parent_account_id,
            parent_name.map(|parent| by_name[parent].account_id),
            "parent for {name}"
        );
        assert_eq!(
            fx.engine
                .get_balance(account.account_id)
                .unwrap()
                .net
                .to_string(),
            "0.00000000"
        );
    }

    let chart = fx
        .engine
        .list_charts(fx.entity_id)
        .into_iter()
        .find(|chart| chart.chart_id == chart_id)
        .unwrap();
    assert!(chart.is_active, "the entity's first chart must be active");

    let replayed = EngineState::replay(fx.book_id, fx.engine.audit_log()).unwrap();
    assert_eq!(&replayed, fx.engine.state());
}

#[test]
fn corporate_starter_is_idempotent_and_conflicts_on_changed_input() {
    let mut fx = fixture();
    let op_id = Uuid::new_v4();
    let first = fx
        .engine
        .create_chart(op_id, fx.actor, corporate_spec(&fx, "Primary"))
        .unwrap();
    let event_count = fx.engine.audit_log().len();

    let replay = fx
        .engine
        .create_chart(op_id, fx.actor, corporate_spec(&fx, "Primary"))
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(fx.engine.audit_log().len(), event_count);
    assert_eq!(fx.engine.list_accounts(first).len(), 13);

    let changed = fx
        .engine
        .create_chart(op_id, fx.actor, corporate_spec(&fx, "Changed"))
        .unwrap_err();
    assert_eq!(changed.error_code, ErrorCode::IdempotencyConflict);
}

#[test]
fn corporate_starter_rejects_invalid_resource_without_partial_events() {
    let mut fx = fixture();
    let before = fx.engine.audit_log().len();
    let mut missing = corporate_spec(&fx, "Missing currency");
    missing.resource_type_id = None;
    assert_eq!(
        fx.engine
            .create_chart(Uuid::new_v4(), fx.actor, missing)
            .unwrap_err()
            .error_code,
        ErrorCode::InvalidInput
    );
    assert_eq!(fx.engine.audit_log().len(), before);

    let mut non_currency = corporate_spec(&fx, "Inventory chart");
    non_currency.resource_type_id = Some(fx.inventory_id);
    assert_eq!(
        fx.engine
            .create_chart(Uuid::new_v4(), fx.actor, non_currency)
            .unwrap_err()
            .error_code,
        ErrorCode::InvalidInput
    );
    assert_eq!(fx.engine.audit_log().len(), before);

    let mut unknown = corporate_spec(&fx, "Unknown currency");
    unknown.resource_type_id = Some(Uuid::new_v4());
    assert_eq!(
        fx.engine
            .create_chart(Uuid::new_v4(), fx.actor, unknown)
            .unwrap_err()
            .error_code,
        ErrorCode::InvalidInput
    );
    assert_eq!(fx.engine.audit_log().len(), before);
    assert!(fx.engine.list_charts(fx.entity_id).is_empty());
}

#[test]
fn copying_starter_chart_copies_source_once_without_injecting_another_set() {
    let mut fx = fixture();
    let source = fx
        .engine
        .create_chart(Uuid::new_v4(), fx.actor, corporate_spec(&fx, "Primary"))
        .unwrap();
    let copied = fx
        .engine
        .copy_chart(
            Uuid::new_v4(),
            fx.actor,
            CopyChart {
                source_chart_id: source,
                name: "Primary copy".into(),
                description: None,
                activate: false,
            },
        )
        .unwrap();
    assert_eq!(fx.engine.list_accounts(source).len(), 13);
    assert_eq!(fx.engine.list_accounts(copied).len(), 13);
}

#[test]
fn empty_chart_remains_available_and_rejects_meaningless_resource_input() {
    let mut fx = fixture();
    let empty = fx
        .engine
        .create_chart(
            Uuid::new_v4(),
            fx.actor,
            NewChart {
                entity_id: fx.entity_id,
                name: "Manual".into(),
                description: None,
                activate: false,
                starter_template: ChartTemplate::Empty,
                resource_type_id: None,
            },
        )
        .unwrap();
    assert!(fx.engine.list_accounts(empty).is_empty());

    let before = fx.engine.audit_log().len();
    let err = fx
        .engine
        .create_chart(
            Uuid::new_v4(),
            fx.actor,
            NewChart {
                entity_id: fx.entity_id,
                name: "Invalid manual".into(),
                description: None,
                activate: false,
                starter_template: ChartTemplate::Empty,
                resource_type_id: Some(fx.usd_id),
            },
        )
        .unwrap_err();
    assert_eq!(err.error_code, ErrorCode::InvalidInput);
    assert_eq!(fx.engine.audit_log().len(), before);
}
