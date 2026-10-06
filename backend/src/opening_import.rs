use ledgerzero_engine::amount::Amount;
use ledgerzero_engine::domain::{AccountType, EntrySource, WorkflowContext};
use ledgerzero_engine::engine::{
    AccountingEngine, ImportPropertyIdentity, NewEntry, NewLine, PrepareImportProperties,
};
use ledgerzero_engine::types::Date;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

const MAX_FILE_BYTES: usize = 1_000_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub file_content: String,
    pub chart_id: Uuid,
    pub account_mappings: BTreeMap<String, Uuid>,
    pub workflow: WorkflowContext,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareRequest {
    pub file_content: String,
    pub chart_id: Uuid,
    pub workflow: WorkflowContext,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    schema_version: String,
    import_id: String,
    source_balance_date: Date,
    opening_entry_date: Date,
    declared_accounting_method: String,
    declared_balance_basis: String,
    resource_codes: Vec<String>,
    source_material: Vec<Source>,
    balance_rows: Vec<Row>,
    control_totals: BTreeMap<String, Totals>,
    entity_namespace: Option<String>,
    related_entities: Option<Vec<RelatedEntity>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelatedEntity {
    external_entity_key: String,
    name: String,
    category: String,
    relationship: String,
    effective_from: Date,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    source_id: String,
    sha256: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    external_account_key: String,
    proposed_account_name: String,
    proposed_account_type: AccountType,
    resource_code: String,
    debit: String,
    credit: String,
    source_id: Option<String>,
    source_reference: Option<String>,
    property_reference: Option<String>,
    note: Option<String>,
    attribution_entity_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Totals {
    debit: String,
    credit: String,
}

// serde_json::Value alone overwrites duplicate object keys before validation.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut object = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueValue>()? {
                    if object.insert(key.clone(), value.0).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON key: {key}"
                        )));
                    }
                }
                Ok(UniqueValue(Value::Object(object)))
            }

            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(value)))
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(json!(value)))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(json!(value)))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(UniqueValue(json!(value)))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

fn decimal(value: &str, precision: u8) -> Result<Amount, String> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || (whole.len() > 1 && whole.starts_with('0'))
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (value.contains('.') && fraction.is_empty())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > precision as usize
    {
        return Err("amount is not a valid non-negative decimal for its resource".into());
    }
    Amount::from_str(value)
}

fn stable_uuid(import_id: Uuid, key: &str) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(import_id.as_bytes());
    digest.update(key.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn sorted_unique(values: impl IntoIterator<Item = String>) -> bool {
    let values: Vec<String> = values.into_iter().collect();
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_key(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn nonempty_optional(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|value| !value.is_empty())
}

pub fn build_entry(
    request: &ImportRequest,
    entity_id: Uuid,
    engine: &AccountingEngine,
) -> Result<NewEntry, String> {
    if serde_json::from_str::<Value>(&request.file_content)
        .ok()
        .and_then(|raw| {
            raw.get("schema_version")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some("1.1")
    {
        return build_v11_entry(request, entity_id, engine);
    }
    if request.file_content.len() > MAX_FILE_BYTES || request.file_content.starts_with('\u{feff}') {
        return Err("import file is too large or has a byte-order mark".into());
    }
    let unique: UniqueValue = serde_json::from_str(&request.file_content)
        .map_err(|error| format!("invalid import JSON: {error}"))?;
    let raw = unique.0;
    if let Some(rows) = raw.get("balance_rows").and_then(Value::as_array) {
        if rows.iter().any(|row| {
            [
                "source_id",
                "source_reference",
                "property_reference",
                "note",
            ]
            .iter()
            .any(|key| row.get(*key).is_none())
        }) {
            return Err("every row must include nullable provenance fields".into());
        }
    }
    let package: Package =
        serde_json::from_value(raw).map_err(|error| format!("invalid import package: {error}"))?;
    if package.schema_version != "1.0" {
        return Err("unsupported import schema version".into());
    }
    if package.entity_namespace.is_some()
        || package.related_entities.is_some()
        || package
            .balance_rows
            .iter()
            .any(|row| row.attribution_entity_key.is_some())
    {
        return Err("v1.0 package contains v1.1 identity fields".into());
    }
    let import_id = Uuid::parse_str(&package.import_id)
        .map_err(|_| "import_id must be a lowercase UUID".to_string())?;
    if import_id.is_nil() || package.import_id != import_id.to_string() {
        return Err("import_id must be a non-nil lowercase UUID".into());
    }
    if package.source_balance_date >= package.opening_entry_date {
        return Err("opening_entry_date must follow source_balance_date".into());
    }
    if !["cash", "accrual", "other"].contains(&package.declared_accounting_method.as_str())
        || package.declared_balance_basis.trim().is_empty()
    {
        return Err("accounting method or balance basis is invalid".into());
    }
    if package.resource_codes.len() != 1
        || package.resource_codes[0].is_empty()
        || package.source_material.is_empty()
        || package.balance_rows.len() < 2
        || !sorted_unique(
            package
                .source_material
                .iter()
                .map(|source| source.source_id.clone()),
        )
        || !sorted_unique(
            package
                .balance_rows
                .iter()
                .map(|row| row.external_account_key.clone()),
        )
    {
        return Err("v1 needs one resource and nonempty sorted unique sources and rows".into());
    }
    let code = &package.resource_codes[0];
    if !code
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase())
        || !code.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
    {
        return Err("invalid resource code".into());
    }
    if package.control_totals.len() != 1 || !package.control_totals.contains_key(code) {
        return Err("control totals must match the declared resource".into());
    }
    let resource = engine
        .list_resource_types()
        .into_iter()
        .find(|resource| resource.code == *code)
        .ok_or_else(|| "unknown resource code".to_string())?;
    let chart = engine
        .list_charts(entity_id)
        .into_iter()
        .find(|chart| chart.chart_id == request.chart_id && chart.is_active)
        .ok_or_else(|| "selected chart is not active in the target entity".to_string())?;
    let source_ids: BTreeSet<&str> = package
        .source_material
        .iter()
        .map(|source| source.source_id.as_str())
        .collect();
    for source in &package.source_material {
        if !valid_key(&source.source_id)
            || source.label.is_empty()
            || source.sha256.len() != 64
            || !source
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("invalid source material metadata".into());
        }
    }
    if request.account_mappings.len() != package.balance_rows.len() {
        return Err("every row must have exactly one account mapping".into());
    }
    let mut lines = Vec::with_capacity(package.balance_rows.len());
    let mut debit_total = Amount::ZERO;
    let mut credit_total = Amount::ZERO;
    for row in &package.balance_rows {
        if !valid_key(&row.external_account_key)
            || row.proposed_account_name.is_empty()
            || row.resource_code != *code
            || row
                .source_id
                .as_deref()
                .is_some_and(|id| !source_ids.contains(id))
            || !nonempty_optional(&row.source_reference)
            || !nonempty_optional(&row.property_reference)
            || !nonempty_optional(&row.note)
        {
            return Err("invalid row key, name, resource, or source reference".into());
        }
        let account_id = request
            .account_mappings
            .get(&row.external_account_key)
            .ok_or_else(|| format!("missing mapping for {}", row.external_account_key))?;
        let account = engine
            .get_account(*account_id)
            .ok_or_else(|| "mapped account does not exist".to_string())?;
        if !account.is_active
            || account.chart_id != chart.chart_id
            || account.entity_id != entity_id
            || account.resource_type_id != resource.resource_type_id
            || account.account_type != row.proposed_account_type
        {
            return Err(format!(
                "incompatible mapped account for {}",
                row.external_account_key
            ));
        }
        let debit = decimal(&row.debit, resource.precision)?;
        let credit = decimal(&row.credit, resource.precision)?;
        if debit.is_zero() == credit.is_zero() {
            return Err("each row needs exactly one positive debit or credit".into());
        }
        debit_total = debit_total.checked_add(debit)?;
        credit_total = credit_total.checked_add(credit)?;
        lines.push(NewLine {
            line_id: stable_uuid(import_id, &row.external_account_key),
            account_id: *account_id,
            attribution_entity_id: None,
            debit_amount: (!debit.is_zero()).then_some(debit),
            credit_amount: (!credit.is_zero()).then_some(credit),
            memo: row.note.clone(),
            metadata: json!({
                "external_account_key": row.external_account_key,
                "source_id": row.source_id,
                "source_reference": row.source_reference,
                "property_reference": row.property_reference,
            }),
        });
    }
    let totals = &package.control_totals[code];
    if debit_total != credit_total
        || debit_total != decimal(&totals.debit, resource.precision)?
        || credit_total != decimal(&totals.credit, resource.precision)?
    {
        return Err("balance rows and control totals do not balance".into());
    }
    Ok(NewEntry {
        entry_id: import_id,
        entity_id,
        entry_date: package.opening_entry_date,
        description: "Opening-balance import".into(),
        lines,
        prices: Vec::new(),
        source: EntrySource::Workflow,
        metadata: json!({
            "kind": "opening_balance_import",
            "schema_version": package.schema_version,
            "import_id": package.import_id,
            "source_balance_date": package.source_balance_date,
            "declared_accounting_method": package.declared_accounting_method,
            "declared_balance_basis": package.declared_balance_basis,
            "source_material": package.source_material.iter().map(|source| json!({
                "source_id": source.source_id,
                "sha256": source.sha256,
                "label": source.label,
            })).collect::<Vec<_>>(),
        }),
        workflow: Some(WorkflowContext {
            workflow_id: request.workflow.workflow_id,
            workflow_deployment_id: request.workflow.workflow_deployment_id,
            workflow_execution_id: stable_uuid(import_id, "opening-import-execution"),
        }),
    })
}

fn parse_v11(
    file_content: &str,
    chart_id: Uuid,
    subject_id: Uuid,
    engine: &AccountingEngine,
) -> Result<(Package, Uuid, PrepareImportProperties), String> {
    if file_content.len() > MAX_FILE_BYTES || file_content.starts_with('\u{feff}') {
        return Err("import file is too large or has a byte-order mark".into());
    }
    let raw = serde_json::from_str::<UniqueValue>(file_content)
        .map_err(|error| format!("invalid import JSON: {error}"))?
        .0;
    let rows = raw
        .get("balance_rows")
        .and_then(Value::as_array)
        .ok_or("missing balance_rows")?;
    if rows.iter().any(|row| {
        [
            "source_id",
            "source_reference",
            "property_reference",
            "note",
            "attribution_entity_key",
        ]
        .iter()
        .any(|key| row.get(*key).is_none())
    }) {
        return Err(
            "every v1.1 row must include nullable provenance and attribution fields".into(),
        );
    }
    if raw.get("entity_namespace").is_none() || raw.get("related_entities").is_none() {
        return Err("v1.1 requires entity_namespace and related_entities".into());
    }
    let package: Package =
        serde_json::from_value(raw).map_err(|error| format!("invalid import package: {error}"))?;
    if package.schema_version != "1.1" {
        return Err("unsupported import schema version".into());
    }
    let import_id =
        Uuid::parse_str(&package.import_id).map_err(|_| "invalid import_id".to_string())?;
    if import_id.is_nil() || package.import_id != import_id.to_string() {
        return Err("import_id must be a non-nil lowercase UUID".into());
    }
    if package.source_balance_date >= package.opening_entry_date {
        return Err("opening date must follow source balance date".into());
    }
    if !["cash", "accrual", "other"].contains(&package.declared_accounting_method.as_str())
        || package.declared_balance_basis.trim().is_empty()
    {
        return Err("invalid accounting method or balance basis".into());
    }
    if package.resource_codes.len() != 1
        || package.source_material.is_empty()
        || package.balance_rows.len() < 2
        || !sorted_unique(
            package
                .source_material
                .iter()
                .map(|source| source.source_id.clone()),
        )
        || !sorted_unique(
            package
                .balance_rows
                .iter()
                .map(|row| row.external_account_key.clone()),
        )
    {
        return Err("v1.1 needs one resource and sorted unique sources and rows".into());
    }
    let code = &package.resource_codes[0];
    if code.is_empty()
        || !code.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
        || package.control_totals.len() != 1
        || !package.control_totals.contains_key(code)
    {
        return Err("invalid resource or control totals".into());
    }
    let resource = engine
        .list_resource_types()
        .into_iter()
        .find(|resource| resource.code == *code)
        .ok_or("unknown resource code")?;
    if !engine
        .list_charts(subject_id)
        .into_iter()
        .any(|chart| chart.chart_id == chart_id && chart.is_active)
    {
        return Err("selected chart is not active for the book subject".into());
    }
    let namespace = package
        .entity_namespace
        .as_deref()
        .ok_or("missing entity_namespace")?
        .to_owned();
    if !valid_key(&namespace) || namespace.len() > 120 {
        return Err("invalid entity_namespace".into());
    }
    let related = package
        .related_entities
        .as_ref()
        .ok_or("missing related_entities")?;
    if related.is_empty()
        || !sorted_unique(
            related
                .iter()
                .map(|entity| entity.external_entity_key.clone()),
        )
    {
        return Err("related_entities must be nonempty and sorted by unique key".into());
    }
    let mut properties = Vec::new();
    for entity in related {
        if !valid_key(&entity.external_entity_key)
            || entity.name.trim().is_empty()
            || entity.category != "PROPERTY"
            || entity.relationship != "OWNS"
        {
            return Err(
                "v1.1 related entity must be a named PROPERTY owned by the book subject".into(),
            );
        }
        properties.push(ImportPropertyIdentity {
            external_key: entity.external_entity_key.clone(),
            name: entity.name.clone(),
            effective_from: entity.effective_from.clone(),
        });
    }
    let keys: BTreeSet<&str> = related
        .iter()
        .map(|entity| entity.external_entity_key.as_str())
        .collect();
    let source_ids: BTreeSet<&str> = package
        .source_material
        .iter()
        .map(|source| source.source_id.as_str())
        .collect();
    for source in &package.source_material {
        if !valid_key(&source.source_id)
            || source.label.is_empty()
            || source.sha256.len() != 64
            || !source
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("invalid source material metadata".into());
        }
    }
    let mut debit_total = Amount::ZERO;
    let mut credit_total = Amount::ZERO;
    for row in &package.balance_rows {
        if !valid_key(&row.external_account_key)
            || row.proposed_account_name.is_empty()
            || row.resource_code != *code
            || row
                .source_id
                .as_deref()
                .is_some_and(|id| !source_ids.contains(id))
            || row
                .attribution_entity_key
                .as_deref()
                .is_some_and(|key| !keys.contains(key))
            || !nonempty_optional(&row.source_reference)
            || !nonempty_optional(&row.property_reference)
            || !nonempty_optional(&row.note)
        {
            return Err("invalid balance row identity or provenance".into());
        }
        let debit = decimal(&row.debit, resource.precision)?;
        let credit = decimal(&row.credit, resource.precision)?;
        if debit.is_zero() == credit.is_zero() {
            return Err("each row needs exactly one positive debit or credit".into());
        }
        debit_total = debit_total.checked_add(debit)?;
        credit_total = credit_total.checked_add(credit)?;
    }
    let totals = &package.control_totals[code];
    if debit_total != credit_total
        || debit_total != decimal(&totals.debit, resource.precision)?
        || credit_total != decimal(&totals.credit, resource.precision)?
    {
        return Err("balance rows and control totals do not balance".into());
    }
    Ok((
        package,
        import_id,
        PrepareImportProperties {
            subject_id,
            entity_namespace: namespace,
            properties,
        },
    ))
}

pub fn preparation_spec(
    request: &PrepareRequest,
    subject_id: Uuid,
    engine: &AccountingEngine,
) -> Result<(Uuid, PrepareImportProperties), String> {
    let (_, import_id, spec) =
        parse_v11(&request.file_content, request.chart_id, subject_id, engine)?;
    Ok((stable_uuid(import_id, "prepare-identities"), spec))
}

fn build_v11_entry(
    request: &ImportRequest,
    subject_id: Uuid,
    engine: &AccountingEngine,
) -> Result<NewEntry, String> {
    let (package, import_id, spec) =
        parse_v11(&request.file_content, request.chart_id, subject_id, engine)?;
    for property in &spec.properties {
        let entity = engine
            .find_import_entity(&spec.entity_namespace, &property.external_key)
            .ok_or("property identities must be prepared before posting")?;
        if entity.name != property.name
            || entity.category != ledgerzero_engine::domain::EntityCategory::Property
            || !engine
                .list_entity_relationships()
                .iter()
                .any(|relationship| {
                    relationship.from_entity_id == subject_id
                        && relationship.to_entity_id == entity.entity_id
                        && relationship.kind
                            == ledgerzero_engine::domain::EntityRelationshipKind::Owns
                        && relationship.effective_from == property.effective_from
                        && relationship.effective_to.is_none()
                })
        {
            return Err("prepared property identity or relationship conflicts with package".into());
        }
    }
    if request.account_mappings.len() != package.balance_rows.len() {
        return Err("every row must have exactly one account mapping".into());
    }
    let resource = engine
        .list_resource_types()
        .into_iter()
        .find(|resource| resource.code == package.resource_codes[0])
        .ok_or("unknown resource")?;
    let mut lines = Vec::new();
    for row in &package.balance_rows {
        let account_id = *request
            .account_mappings
            .get(&row.external_account_key)
            .ok_or_else(|| format!("missing mapping for {}", row.external_account_key))?;
        let account = engine
            .get_account(account_id)
            .ok_or("mapped account does not exist")?;
        let attribution = if let Some(key) = row.attribution_entity_key.as_deref() {
            let entity = engine
                .find_import_entity(&spec.entity_namespace, key)
                .ok_or("property identities must be prepared before posting")?;
            Some(entity.entity_id)
        } else {
            None
        };
        if !account.is_active
            || account.chart_id != request.chart_id
            || account.entity_id != subject_id
            || account.account_type != row.proposed_account_type
            || account.resource_type_id != resource.resource_type_id
            || account.associated_entity_id != attribution
        {
            return Err(format!(
                "incompatible mapped account for {}",
                row.external_account_key
            ));
        }
        let debit = decimal(&row.debit, resource.precision)?;
        let credit = decimal(&row.credit, resource.precision)?;
        lines.push(NewLine { line_id: stable_uuid(import_id, &row.external_account_key), account_id,
            attribution_entity_id: attribution, debit_amount: (!debit.is_zero()).then_some(debit),
            credit_amount: (!credit.is_zero()).then_some(credit), memo: row.note.clone(),
            metadata: json!({"external_account_key":row.external_account_key,"source_id":row.source_id,
                "source_reference":row.source_reference,"property_reference":row.property_reference,
                "attribution_entity_key":row.attribution_entity_key}) });
    }
    Ok(NewEntry {
        entry_id: import_id,
        entity_id: subject_id,
        entry_date: package.opening_entry_date,
        description: "Opening-balance import".into(),
        lines,
        prices: Vec::new(),
        source: EntrySource::Workflow,
        metadata: json!({"kind":"opening_balance_import","schema_version":"1.1","import_id":package.import_id,
            "entity_namespace":spec.entity_namespace,"source_balance_date":package.source_balance_date,
            "declared_accounting_method":package.declared_accounting_method,"declared_balance_basis":package.declared_balance_basis,
            "source_material":package.source_material.iter().map(|source| json!({"source_id":source.source_id,"sha256":source.sha256,"label":source.label})).collect::<Vec<_>>() }),
        workflow: Some(WorkflowContext {
            workflow_id: request.workflow.workflow_id,
            workflow_deployment_id: request.workflow.workflow_deployment_id,
            workflow_execution_id: stable_uuid(import_id, "opening-import-execution"),
        }),
    })
}
