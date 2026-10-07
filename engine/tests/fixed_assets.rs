mod common;

use common::*;
use ledgerzero_engine::amount::Amount;
use ledgerzero_engine::domain::*;
use ledgerzero_engine::engine::*;
use ledgerzero_engine::error::ErrorCode;
use serde_json::Value;

fn linked_entry(fx: &Fx, date: &str, description: &str, mut lines: Vec<NewLine>) -> NewEntry {
    for line in &mut lines {
        line.attribution_entity_id = fx
            .engine
            .get_account(line.account_id)
            .unwrap()
            .associated_entity_id;
    }
    fx.entry(date, description, lines)
}

fn setup() -> (Fx, FixedAsset) {
    let mut fx = fixture();
    fx.engine
        .prepare_import_properties(
            id(),
            fx.actor,
            PrepareImportProperties {
                subject_id: fx.entity,
                entity_namespace: "properties".into(),
                properties: vec![ImportPropertyIdentity {
                    external_key: "p1".into(),
                    name: "Building".into(),
                    effective_from: date("2026-01-01"),
                }],
            },
        )
        .unwrap();
    let property = fx
        .engine
        .find_import_entity("properties", "p1")
        .unwrap()
        .entity_id;
    let mut account = |name: &str, account_type| {
        fx.engine
            .create_account(
                id(),
                fx.actor,
                NewAccount {
                    chart_id: fx.chart,
                    name: name.into(),
                    code: None,
                    account_type,
                    resource_type_id: fx.usd,
                    parent_account_id: None,
                    associated_entity_id: Some(property),
                    validation_rules: Value::Null,
                    metadata: Value::Null,
                },
            )
            .unwrap()
    };
    let cost = account("Building cost", AccountType::Asset);
    let accumulated = account("Accumulated depreciation", AccountType::Asset);
    let expense = account("Depreciation", AccountType::Expense);
    let land = account("Land", AccountType::Asset);
    let asset = FixedAsset {
        asset_id: id(),
        entity_id: fx.entity,
        property_entity_id: property,
        external_namespace: "assets".into(),
        external_key: "a1".into(),
        name: "Building".into(),
        cost_account_id: cost,
        accumulated_depreciation_account_id: accumulated,
        depreciation_expense_account_id: expense,
        land_account_id: Some(land),
        in_service_date: date("2026-01-01"),
        cost: amt("1000"),
        land: amt("200"),
        depreciable_basis: amt("1000"),
        business_use_percent: amt("100"),
        useful_life_years: amt("27.5"),
        method: "SL".into(),
        convention: "MM".into(),
        accumulated_depreciation_as_of: date("2026-01-01"),
        accumulated_depreciation: amt("0"),
        schedules: vec![DepreciationSchedule {
            year: 2026,
            amount: amt("35"),
            status: "PROJECTED".into(),
            basis: "TAX_EQUALS_BOOK".into(),
        }],
        status: "ACTIVE".into(),
        source_metadata: Value::Null,
        linked_entry_id: None,
        revision: 0,
    };
    (fx, asset)
}

#[test]
fn register_profiles_idempotency_and_replay() {
    let (mut fx, asset) = setup();
    let op = id();
    assert_eq!(
        fx.engine
            .create_fixed_asset(op, fx.actor, asset.clone())
            .unwrap(),
        asset.asset_id
    );
    assert_eq!(
        fx.engine
            .create_fixed_asset(op, fx.actor, asset.clone())
            .unwrap(),
        asset.asset_id
    );
    let mut changed = asset.clone();
    changed.name = "Changed".into();
    assert_eq!(
        fx.engine
            .create_fixed_asset(op, fx.actor, changed)
            .unwrap_err()
            .error_code,
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(fx.engine.list_fixed_assets(fx.entity), vec![&asset]);
    let profile = PropertyProfile {
        property_entity_id: asset.property_entity_id,
        address: "1 Main Street".into(),
        property_type: "RESIDENTIAL".into(),
        source_year: 2026,
        fair_rental_days: 300,
        personal_use_days: 10,
        source_metadata: Value::Null,
    };
    let op = id();
    assert_eq!(
        fx.engine
            .upsert_property_profiles(op, fx.actor, vec![profile.clone()])
            .unwrap(),
        op
    );
    fx.engine
        .upsert_property_profiles(op, fx.actor, vec![profile.clone()])
        .unwrap();
    assert_eq!(fx.engine.list_property_profiles(), vec![&profile]);
    assert_eq!(
        &EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        fx.engine.state()
    );
}

#[test]
fn invalid_accounts_amounts_schedules_and_profiles_do_not_mutate() {
    let (mut fx, asset) = setup();
    let state = fx.engine.state().clone();
    let mut invalids = Vec::new();
    let mut invalid = asset.clone();
    invalid.cost_account_id = fx.cash;
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.depreciation_expense_account_id = asset.cost_account_id;
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.cost = amt("-1");
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.accumulated_depreciation = amt("1001");
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.schedules.push(invalid.schedules[0].clone());
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.schedules[0].year = 2025;
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.method = "MACRS".into();
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.convention = "HY".into();
    invalids.push(invalid);
    let mut invalid = asset.clone();
    invalid.accumulated_depreciation_as_of = date("2025-12-31");
    invalids.push(invalid);
    for invalid in invalids {
        assert!(fx
            .engine
            .create_fixed_asset(id(), fx.actor, invalid)
            .is_err());
        assert_eq!(fx.engine.state(), &state);
    }
    let profile = PropertyProfile {
        property_entity_id: asset.property_entity_id,
        address: "1 Main Street".into(),
        property_type: "RESIDENTIAL".into(),
        source_year: 2026,
        fair_rental_days: 365,
        personal_use_days: 1,
        source_metadata: Value::Null,
    };
    assert!(fx
        .engine
        .upsert_property_profiles(id(), fx.actor, vec![profile])
        .is_err());
    assert_eq!(fx.engine.state(), &state);
}

#[test]
fn acquisition_improvement_disposal_are_atomic_and_replayable() {
    let (mut fx, mut asset) = setup();
    let entry = linked_entry(
        &fx,
        "2026-01-15",
        "Acquire building",
        vec![
            debit(asset.cost_account_id, "1000"),
            debit(asset.land_account_id.unwrap(), "200"),
            credit(fx.cash, "1200"),
        ],
    );
    asset.linked_entry_id = Some(entry.entry_id);
    let state = fx.engine.state().clone();
    let mut mismatch = asset.clone();
    mismatch.cost = amt("1100");
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, entry.clone(), mismatch)
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    let mut unbalanced = entry.clone();
    unbalanced.lines[2].credit_amount = Some(amt("1"));
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, unbalanced, asset.clone())
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    assert_eq!(
        fx.engine
            .record_fixed_asset_transaction(fx.actor, entry.clone(), asset.clone())
            .unwrap(),
        entry.entry_id
    );
    let length = fx.engine.audit_log().len();
    fx.engine
        .record_fixed_asset_transaction(fx.actor, entry.clone(), asset.clone())
        .unwrap();
    assert_eq!(fx.engine.audit_log().len(), length);
    assert!(fx.engine.post_entry(fx.actor, entry).is_err());

    let improvement = linked_entry(
        &fx,
        "2026-02-01",
        "Improve building",
        vec![debit(asset.cost_account_id, "100"), credit(fx.cash, "100")],
    );
    asset.cost = amt("1100");
    asset.depreciable_basis = amt("1100");
    asset.linked_entry_id = Some(improvement.entry_id);
    fx.engine
        .record_fixed_asset_transaction(fx.actor, improvement.clone(), asset.clone())
        .unwrap();
    assert_eq!(
        fx.engine.get_fixed_asset(asset.asset_id).unwrap().revision,
        1
    );
    assert_eq!(
        fx.engine.get_balance(asset.cost_account_id).unwrap().net,
        amt("1100")
    );
    let mut stale_entry = improvement;
    stale_entry.entry_id = id();
    asset.linked_entry_id = Some(stale_entry.entry_id);
    let state = fx.engine.state().clone();
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, stale_entry, asset.clone())
        .is_err());
    assert_eq!(fx.engine.state(), &state);

    let disposal = linked_entry(
        &fx,
        "2026-03-01",
        "Dispose building",
        vec![
            credit(asset.cost_account_id, "1100"),
            credit(asset.land_account_id.unwrap(), "200"),
            debit(fx.cash, "1300"),
        ],
    );
    asset.revision = 1;
    asset.cost = Amount::ZERO;
    asset.land = Amount::ZERO;
    asset.depreciable_basis = Amount::ZERO;
    asset.schedules.clear();
    asset.status = "DISPOSED".into();
    asset.linked_entry_id = Some(disposal.entry_id);
    fx.engine
        .record_fixed_asset_transaction(fx.actor, disposal, asset.clone())
        .unwrap();
    assert_eq!(
        fx.engine.get_fixed_asset(asset.asset_id).unwrap().revision,
        2
    );
    assert_eq!(
        &EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        fx.engine.state()
    );
}

#[test]
fn opening_entry_allows_aggregated_controls_and_schedule_review_is_versioned() {
    let (mut fx, mut asset) = setup();
    let opening = linked_entry(
        &fx,
        "2026-01-15",
        "Aggregated opening",
        vec![
            debit(asset.cost_account_id, "2000"),
            debit(asset.land_account_id.unwrap(), "400"),
            credit(asset.accumulated_depreciation_account_id, "100"),
            credit(fx.capital, "2300"),
        ],
    );
    fx.engine.post_entry(fx.actor, opening.clone()).unwrap();
    asset.linked_entry_id = Some(opening.entry_id);
    fx.engine
        .create_fixed_asset(id(), fx.actor, asset.clone())
        .unwrap();
    let mut second = asset.clone();
    second.asset_id = id();
    second.external_key = "a2".into();
    fx.engine
        .create_fixed_asset(id(), fx.actor, second)
        .unwrap();
    asset.schedules[0].status = "REVIEWED".into();
    asset.schedules.push(DepreciationSchedule {
        year: 2027,
        amount: Amount::ZERO,
        status: "PROJECTED".into(),
        basis: "TAX_EQUALS_BOOK".into(),
    });
    let op = id();
    fx.engine
        .update_fixed_asset(op, fx.actor, asset.clone())
        .unwrap();
    fx.engine
        .update_fixed_asset(op, fx.actor, asset.clone())
        .unwrap();
    assert_eq!(
        fx.engine.get_fixed_asset(asset.asset_id).unwrap().revision,
        1
    );
    let state = fx.engine.state().clone();
    assert!(fx
        .engine
        .update_fixed_asset(id(), fx.actor, asset.clone())
        .is_err());
    asset.revision = 1;
    asset.cost = amt("1001");
    assert!(fx.engine.update_fixed_asset(id(), fx.actor, asset).is_err());
    assert_eq!(fx.engine.state(), &state);
    assert_eq!(
        &EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        fx.engine.state()
    );
}

#[test]
fn fixed_asset_permissions_are_allowed_in_both_role_paths() {
    let (mut fx, _) = setup();
    let permissions: Vec<String> = [
        "list_account_balances",
        "create_fixed_asset",
        "list_fixed_assets",
        "update_fixed_asset",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let role = fx
        .engine
        .create_role(
            id(),
            fx.actor,
            NewRole {
                entity_id: fx.entity,
                name: "Asset register".into(),
                description: None,
                permissions: permissions.clone(),
            },
        )
        .unwrap();
    fx.engine
        .assign_role_to_user(id(), fx.actor, role, fx.actor)
        .unwrap();
    for permission in &permissions {
        assert!(fx
            .engine
            .user_has_permission(fx.actor, fx.entity, permission));
    }
    let role = fx
        .engine
        .create_role(
            id(),
            fx.actor,
            NewRole {
                entity_id: fx.entity,
                name: "Asset register incremental".into(),
                description: None,
                permissions: Vec::new(),
            },
        )
        .unwrap();
    for permission in permissions {
        fx.engine
            .add_role_permission(id(), fx.actor, role, permission)
            .unwrap();
    }
}

#[test]
fn disposal_requires_zero_controls_and_matching_removal_lines() {
    let (mut fx, mut asset) = setup();
    asset.accumulated_depreciation = amt("100");
    let acquisition = linked_entry(
        &fx,
        "2026-01-15",
        "Acquire depreciated asset",
        vec![
            debit(asset.cost_account_id, "1000"),
            debit(asset.land_account_id.unwrap(), "200"),
            credit(asset.accumulated_depreciation_account_id, "100"),
            credit(fx.cash, "1100"),
        ],
    );
    asset.linked_entry_id = Some(acquisition.entry_id);
    fx.engine
        .record_fixed_asset_transaction(fx.actor, acquisition, asset.clone())
        .unwrap();
    let state = fx.engine.state().clone();
    let unrelated = linked_entry(
        &fx,
        "2026-02-01",
        "Not a disposal",
        vec![debit(fx.cash, "1"), credit(fx.capital, "1")],
    );
    let mut disposed = asset.clone();
    disposed.status = "DISPOSED".into();
    disposed.linked_entry_id = Some(unrelated.entry_id);
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, unrelated.clone(), disposed.clone())
        .is_err());
    disposed.cost = Amount::ZERO;
    disposed.land = Amount::ZERO;
    disposed.depreciable_basis = Amount::ZERO;
    disposed.accumulated_depreciation = Amount::ZERO;
    disposed.schedules.clear();
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, unrelated, disposed.clone())
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    let removal = linked_entry(
        &fx,
        "2026-02-01",
        "Remove asset controls",
        vec![
            credit(asset.cost_account_id, "1000"),
            credit(asset.land_account_id.unwrap(), "200"),
            debit(asset.accumulated_depreciation_account_id, "100"),
            debit(fx.cash, "1100"),
        ],
    );
    disposed.linked_entry_id = Some(removal.entry_id);
    fx.engine
        .record_fixed_asset_transaction(fx.actor, removal, disposed)
        .unwrap();
    for account in [
        asset.cost_account_id,
        asset.land_account_id.unwrap(),
        asset.accumulated_depreciation_account_id,
    ] {
        assert_eq!(fx.engine.get_balance(account).unwrap().net, Amount::ZERO);
    }
    assert_eq!(
        &EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        fx.engine.state()
    );
}

#[test]
fn asset_money_and_transaction_lines_respect_currency_precision() {
    let (mut fx, asset) = setup();
    let state = fx.engine.state().clone();
    let setters: [fn(&mut FixedAsset); 5] = [
        |a| a.cost = amt("1000.001"),
        |a| a.land = amt("200.001"),
        |a| a.depreciable_basis = amt("999.999"),
        |a| a.accumulated_depreciation = amt("0.001"),
        |a| a.schedules[0].amount = amt("35.001"),
    ];
    for set in setters {
        let mut invalid = asset.clone();
        set(&mut invalid);
        assert!(fx
            .engine
            .create_fixed_asset(id(), fx.actor, invalid)
            .is_err());
        assert_eq!(fx.engine.state(), &state);
    }
    let split = linked_entry(
        &fx,
        "2026-01-15",
        "Subcent split netting to valid cost",
        vec![
            debit(asset.cost_account_id, "1000.005"),
            credit(asset.cost_account_id, "0.005"),
            debit(asset.land_account_id.unwrap(), "200"),
            credit(fx.cash, "1200"),
        ],
    );
    let mut invalid = asset.clone();
    invalid.linked_entry_id = Some(split.entry_id);
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, split, invalid)
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    fx.engine
        .create_fixed_asset(id(), fx.actor, asset.clone())
        .unwrap();
    let state = fx.engine.state().clone();
    let mut invalid = asset;
    invalid.schedules[0].amount = amt("35.001");
    assert!(fx
        .engine
        .update_fixed_asset(id(), fx.actor, invalid)
        .is_err());
    assert_eq!(fx.engine.state(), &state);
}

#[test]
fn transaction_dates_bound_service_and_accumulated_depreciation() {
    let (mut fx, mut asset) = setup();
    let entry = linked_entry(
        &fx,
        "2026-01-15",
        "Acquire asset",
        vec![
            debit(asset.cost_account_id, "1000"),
            debit(asset.land_account_id.unwrap(), "200"),
            credit(fx.cash, "1200"),
        ],
    );
    asset.linked_entry_id = Some(entry.entry_id);
    let state = fx.engine.state().clone();
    let mut future_service = asset.clone();
    future_service.in_service_date = date("2026-01-16");
    future_service.accumulated_depreciation_as_of = date("2026-01-16");
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, entry.clone(), future_service)
        .is_err());
    let mut future_depreciation = asset.clone();
    future_depreciation.accumulated_depreciation_as_of = date("2026-01-16");
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, entry.clone(), future_depreciation)
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    asset.accumulated_depreciation_as_of = entry.entry_date.clone();
    fx.engine
        .record_fixed_asset_transaction(fx.actor, entry, asset.clone())
        .unwrap();
    let update = linked_entry(
        &fx,
        "2026-02-01",
        "Improve asset",
        vec![debit(asset.cost_account_id, "100"), credit(fx.cash, "100")],
    );
    asset.cost = amt("1100");
    asset.depreciable_basis = amt("1100");
    asset.linked_entry_id = Some(update.entry_id);
    asset.accumulated_depreciation_as_of = date("2026-02-02");
    let state = fx.engine.state().clone();
    assert!(fx
        .engine
        .record_fixed_asset_transaction(fx.actor, update, asset)
        .is_err());
    assert_eq!(fx.engine.state(), &state);
}

#[test]
fn pending_schedules_include_source_year_and_exclude_historical_years() {
    let (mut fx, mut asset) = setup();
    asset.in_service_date = date("2024-01-01");
    asset.accumulated_depreciation_as_of = date("2026-01-01");
    asset.accumulated_depreciation = amt("800");
    asset.schedules = vec![
        DepreciationSchedule {
            year: 2025,
            amount: amt("800"),
            status: "REVIEWED".into(),
            basis: "TAX_EQUALS_BOOK".into(),
        },
        DepreciationSchedule {
            year: 2026,
            amount: amt("100"),
            status: "PROJECTED".into(),
            basis: "TAX_EQUALS_BOOK".into(),
        },
        DepreciationSchedule {
            year: 2027,
            amount: amt("100"),
            status: "PROJECTED".into(),
            basis: "TAX_EQUALS_BOOK".into(),
        },
    ];
    fx.engine
        .create_fixed_asset(id(), fx.actor, asset.clone())
        .unwrap();
    let state = fx.engine.state().clone();
    let mut combined = asset.clone();
    combined.schedules[2].amount = amt("100.01");
    assert!(fx
        .engine
        .update_fixed_asset(id(), fx.actor, combined)
        .is_err());
    let mut individual = asset.clone();
    individual.schedules[1].amount = amt("200.01");
    individual.schedules[2].amount = Amount::ZERO;
    assert!(fx
        .engine
        .update_fixed_asset(id(), fx.actor, individual)
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    asset.schedules[0].amount = amt("900");
    fx.engine.update_fixed_asset(id(), fx.actor, asset).unwrap();
    assert_eq!(
        &EngineState::replay(fx.book, fx.engine.audit_log()).unwrap(),
        fx.engine.state()
    );
}

#[test]
fn source_year_schedules_are_capped_even_with_zero_accumulated_depreciation() {
    let (mut fx, mut asset) = setup();
    asset.schedules[0].amount = amt("600");
    let mut next = asset.schedules[0].clone();
    next.year = 2027;
    next.amount = amt("400.01");
    asset.schedules.push(next);
    let state = fx.engine.state().clone();
    assert!(fx
        .engine
        .create_fixed_asset(id(), fx.actor, asset.clone())
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    asset.schedules[1].amount = amt("400");
    fx.engine.create_fixed_asset(id(), fx.actor, asset).unwrap();
}

#[test]
fn zero_accumulated_import_does_not_require_opening_contra_line() {
    let (mut fx, mut asset) = setup();
    let opening = linked_entry(
        &fx,
        "2026-01-15",
        "Zero depreciation opening",
        vec![
            debit(asset.cost_account_id, "1000"),
            debit(asset.land_account_id.unwrap(), "200"),
            credit(fx.capital, "1200"),
        ],
    );
    fx.engine.post_entry(fx.actor, opening.clone()).unwrap();
    asset.linked_entry_id = Some(opening.entry_id);
    let state = fx.engine.state().clone();
    let mut nonzero = asset.clone();
    nonzero.accumulated_depreciation = amt("1");
    assert!(fx
        .engine
        .create_fixed_asset(id(), fx.actor, nonzero)
        .is_err());
    assert_eq!(fx.engine.state(), &state);
    fx.engine.create_fixed_asset(id(), fx.actor, asset).unwrap();
    let state = fx.engine.state().clone();
    assert!(fx
        .engine
        .reverse_entry(
            fx.actor,
            ReverseEntry {
                new_entry_id: id(),
                original_entry_id: opening.entry_id,
                entry_date: date("2026-02-01"),
                description: None,
                metadata: Value::Null,
            }
        )
        .is_err());
    assert_eq!(fx.engine.state(), &state);
}

#[test]
fn reversals_reject_current_and_historical_asset_links_after_replay() {
    let (mut fx, mut asset) = setup();
    let acquisition = linked_entry(
        &fx,
        "2026-01-15",
        "Acquisition",
        vec![
            debit(asset.cost_account_id, "1000"),
            debit(asset.land_account_id.unwrap(), "200"),
            credit(fx.cash, "1200"),
        ],
    );
    let acquisition_id = acquisition.entry_id;
    asset.linked_entry_id = Some(acquisition_id);
    fx.engine
        .record_fixed_asset_transaction(fx.actor, acquisition, asset.clone())
        .unwrap();
    let improvement = linked_entry(
        &fx,
        "2026-02-01",
        "Improvement",
        vec![debit(asset.cost_account_id, "100"), credit(fx.cash, "100")],
    );
    let improvement_id = improvement.entry_id;
    asset.cost = amt("1100");
    asset.depreciable_basis = amt("1100");
    asset.linked_entry_id = Some(improvement_id);
    fx.engine
        .record_fixed_asset_transaction(fx.actor, improvement, asset)
        .unwrap();
    let replayed = EngineState::replay(fx.book, fx.engine.audit_log()).unwrap();
    let mut restored = AccountingEngine::from_state(
        replayed,
        Box::new(ledgerzero_engine::types::FixedClock::new(0)),
    );
    for engine in [&mut fx.engine, &mut restored] {
        let state = engine.state().clone();
        for entry in [acquisition_id, improvement_id] {
            let error = engine
                .reverse_entry(
                    fx.actor,
                    ReverseEntry {
                        new_entry_id: id(),
                        original_entry_id: entry,
                        entry_date: date("2026-03-01"),
                        description: None,
                        metadata: Value::Null,
                    },
                )
                .unwrap_err();
            assert!(error
                .message
                .contains("compensating linked asset transaction"));
            assert_eq!(engine.state(), &state);
        }
    }
    let unrelated = fx.entry(
        "2026-03-01",
        "Unrelated",
        vec![debit(fx.cash, "1"), credit(fx.capital, "1")],
    );
    fx.engine.post_entry(fx.actor, unrelated.clone()).unwrap();
    fx.engine
        .reverse_entry(
            fx.actor,
            ReverseEntry {
                new_entry_id: id(),
                original_entry_id: unrelated.entry_id,
                entry_date: date("2026-03-02"),
                description: None,
                metadata: Value::Null,
            },
        )
        .unwrap();
}
