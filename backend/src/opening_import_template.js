import { React, createRoot } from "./workflow-react.js";

const WORKFLOW_ID = "__WORKFLOW_ID__";
const WORKFLOW_DEPLOYMENT_ID = "__WORKFLOW_DEPLOYMENT_ID__";
const WORKFLOW_NAME = __WORKFLOW_NAME_JSON__;
const e = React.createElement;

async function api(path, options = {}) {
  try {
    const response = await fetch(path, { credentials: "same-origin", headers: { "Content-Type": "application/json" }, ...options });
    const text = await response.text();
    let body;
    try { body = JSON.parse(text); } catch { body = { message: text || `Request failed (${response.status}).` }; }
    return { ok: response.ok, body };
  } catch (cause) { return { ok: false, body: { message: cause.message || "Network request failed." } }; }
}

function table(label, headers, rows) {
  return e("div", { style: { overflowX: "auto" } }, e("table", { "aria-label": label },
    e("thead", null, e("tr", null, ...headers.map((header) => e("th", { key: header, scope: "col" }, header)))),
    e("tbody", null, ...rows.map((row, index) => e("tr", { key: index }, ...row.map((cell, column) => e("td", { key: column }, cell ?? "Not supplied")))))));
}

function sourceRef(item, sources = []) {
  const metadata = item.source_metadata || item;
  const source = sources.find((entry) => entry.source_id === metadata.source_id);
  return [metadata.source_id, source?.label, metadata.source_reference].filter(Boolean).join(" / ") || "Not supplied";
}

function schedules(asset) {
  return e("div", null, ...(asset.schedules || []).map((schedule, index) => e("p", { key: index },
    `${schedule.year}: ${schedule.amount} (${schedule.status}; ${schedule.basis})`)));
}

function assetTable(label, assets, propertyName, accountName, sources, editAsset, busy) {
  return table(label, ["Asset", "Property", "Cost / land / basis", "In service", "Depreciation method", "Starting accumulated depreciation", "Schedules (not posted)", "Linked accounts", "Source reference", ...(editAsset ? ["Review"] : [])], assets.map((asset) => [
    `${asset.name} (${asset.external_asset_key || asset.external_key})`,
    propertyName(asset.attribution_entity_key || asset.property_entity_id),
    `${asset.cost} / ${asset.land} / ${asset.depreciable_basis}`,
    asset.in_service_date,
    `${asset.method}; ${asset.convention}; ${asset.useful_life_years} years; ${asset.business_use_percent}% business use`,
    `${asset.accumulated_depreciation} as of ${asset.accumulated_depreciation_as_of}`,
    schedules(asset),
    [asset.cost_account_key || asset.cost_account_id, asset.land_account_key || asset.land_account_id,
      asset.accumulated_depreciation_account_key || asset.accumulated_depreciation_account_id,
      asset.depreciation_expense_account_key || asset.depreciation_expense_account_id].filter(Boolean).map(accountName).join(" / "),
    sourceRef(asset, sources),
    ...(editAsset ? [e("button", { type: "button", disabled: busy || asset.status !== "ACTIVE", onClick: () => editAsset(asset) }, `Review ${asset.name}`)] : []),
  ]));
}

function App() {
  const query = new URLSearchParams(window.location.search);
  const bookId = query.get("book_id");
  const [context, setContext] = React.useState(null);
  const [accounts, setAccounts] = React.useState([]);
  const [resources, setResources] = React.useState([]);
  const [fileText, setFileText] = React.useState("");
  const [rows, setRows] = React.useState([]);
  const [packageData, setPackageData] = React.useState(null);
  const [choices, setChoices] = React.useState({});
  const [overrides, setOverrides] = React.useState({});
  const [mappings, setMappings] = React.useState({});
  const [mappingFinished, setMappingFinished] = React.useState(false);
  const [version, setVersion] = React.useState("");
  const [prepared, setPrepared] = React.useState(null);
  const [error, setError] = React.useState("");
  const [message, setMessage] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [posted, setPosted] = React.useState(null);
  const [register, setRegister] = React.useState(null);
  const [profiles, setProfiles] = React.useState(null);
  const [balances, setBalances] = React.useState({});
  const [reviewBalances, setReviewBalances] = React.useState([]);
  const [reviewed, setReviewed] = React.useState(false);
  const [editingAsset, setEditingAsset] = React.useState(null);
  const [assetName, setAssetName] = React.useState("");
  const [scheduleAmounts, setScheduleAmounts] = React.useState([]);
  const assetOp = React.useRef(null);
  const [verificationErrors, setVerificationErrors] = React.useState([]);
  const accountOps = React.useRef({});
  const createdAccounts = React.useRef({});
  const hasIdentities = version === "1.1" || version === "1.2";

  async function loadAccounts(chartId) {
    const listed = await api(`/api/books/${bookId}/accounts?chart_id=${chartId}`);
    if (!listed.ok) { setError(listed.body.message || "A separate account-list role is required."); return; }
    setAccounts(listed.body.filter((account) => account.is_active));
    return listed.body.filter((account) => account.is_active);
  }

  async function loadResources() {
    const listed = await api(`/api/books/${bookId}/resource-types`);
    if (!listed.ok) { setError(listed.body.message || "A separate resource-list role is required."); return; }
    setResources(listed.body);
    return listed.body;
  }

  function associatedId(row) {
    return prepared?.resolved_entities?.find((item) => item.external_entity_key === row.attribution_entity_key)?.entity_id || null;
  }

  function eligible(account, row, resourceId) {
    return account.chart_id === context.chart_id && account.account_type === row.proposed_account_type &&
      account.resource_type_id === resourceId &&
      (!hasIdentities || (account.associated_entity_id || null) === associatedId(row));
  }

  function parentChoice(row) {
    return overrides[row.external_account_key]?.parent ?? (row.parent_external_account_key ? `definition:${row.parent_external_account_key}` : "");
  }

  function changeOverride(key, field, value) {
    setOverrides((current) => ({ ...current, [key]: { ...current[key], [field]: value } }));
    setMappingFinished(false);
  }

  function propertyName(key) {
    return packageData?.related_entities?.find((item) => item.external_entity_key === key)?.name ||
      prepared?.resolved_entities?.find((item) => item.entity_id === key)?.name ||
      profiles?.find((item) => item.property_entity_id === key)?.address || key;
  }

  function accountName(key) {
    return accounts.find((item) => item.account_id === (mappings[key] || key))?.name ||
      overrides[key]?.name || rows.find((item) => item.external_account_key === key)?.proposed_account_name ||
      reviewBalances.find((item) => item.account_id === key)?.account_name || key;
  }

  React.useEffect(() => {
    document.title = `${WORKFLOW_NAME} - FPA`;
    if (!bookId) return;
    api(`/api/books/${bookId}/opening-import/context?workflow_deployment_id=${WORKFLOW_DEPLOYMENT_ID}`).then(async (result) => {
      if (!result.ok) { setError(result.body.message || "Opening-import workflow role is required."); return; }
      setContext(result.body);
      await loadAccounts(result.body.chart_id);
      await loadResources();
    });
  }, [bookId]);

  async function loadFile(event) {
    setError(""); setMessage(""); setFileText(""); setRows([]); setPackageData(null); setChoices({}); setOverrides({}); setMappings({}); setMappingFinished(false); setPrepared(null); setVersion("");
    setPosted(null); setRegister(null); setProfiles(null); setBalances({}); setReviewBalances([]); setReviewed(false); setEditingAsset(null); setVerificationErrors([]); accountOps.current = {}; createdAccounts.current = {};
    const file = event.target.files?.[0];
    if (!file) return;
    if (file.size > 1000000) { setError("The import file exceeds 1 MB."); return; }
    try {
      const text = await file.text();
      const parsed = JSON.parse(text);
      if (!Array.isArray(parsed.balance_rows) || !parsed.balance_rows.length) throw new Error("No balance rows found.");
      if (!["1.0", "1.1", "1.2"].includes(parsed.schema_version)) throw new Error("Unsupported import schema version.");
      if (parsed.schema_version === "1.2" && (!Array.isArray(parsed.account_definitions) || !parsed.account_definitions.length ||
        !Array.isArray(parsed.fixed_assets) || !Array.isArray(parsed.property_facts))) {
        throw new Error("Version 1.2 requires account_definitions, fixed_assets, and property_facts arrays.");
      }
      const definitions = parsed.schema_version === "1.2" ? parsed.account_definitions : parsed.balance_rows;
      const keys = new Set();
      for (const definition of definitions) {
        if (!definition.external_account_key || keys.has(definition.external_account_key)) throw new Error("Account definitions must have unique, nonempty external keys.");
        keys.add(definition.external_account_key);
      }
      if (parsed.balance_rows.some((row) => !keys.has(row.external_account_key))) throw new Error("Every balance row must have an account definition.");
      setFileText(text);
      setRows(definitions);
      setPackageData(parsed);
      setVersion(parsed.schema_version);
    } catch (cause) { setFileText(""); setError(`Cannot inspect file: ${cause.message}`); }
  }

  async function prepare() {
    setBusy(true); setError(""); setMessage("");
    const result = await api(`/api/books/${bookId}/opening-import/prepare-identities`, {
      method: "POST",
      body: JSON.stringify({ file_content: fileText, chart_id: context.chart_id,
        workflow: { workflow_id: WORKFLOW_ID, workflow_deployment_id: WORKFLOW_DEPLOYMENT_ID, workflow_execution_id: crypto.randomUUID() } }),
    });
    setBusy(false);
    if (result.ok) { setPrepared(result.body); setMappingFinished(false); setMessage("Identity preparation complete. Review account choices, then finish mapping."); }
    else setError(`${result.body.error_code || "ERROR"}: ${result.body.message || "Identity preparation failed"}`);
  }

  async function finishMapping() {
    setBusy(true); setError(""); setMessage(""); setMappingFinished(false);
    try {
      const listed = await loadAccounts(context.chart_id);
      const types = await loadResources();
      if (!listed || !types) throw new Error("Account and resource lists are required to finish mapping.");
      const plannedNames = new Set();
      const plans = rows.map((row) => {
        const choice = choices[row.external_account_key] || "__create__";
        const name = (overrides[row.external_account_key]?.name ?? row.proposed_account_name).trim();
        if (choice === "__create__" && !name) throw new Error(`Enter a name for ${row.external_account_key}.`);
        if (choice === "__create__" && plannedNames.has(name)) {
          throw new Error(`Multiple definitions propose the same new account name: ${name}. Choose distinct names or existing accounts.`);
        }
        if (choice === "__create__") plannedNames.add(name);
        if (hasIdentities && row.attribution_entity_key && !associatedId(row)) throw new Error(`Prepare the property identity for ${row.attribution_entity_key} first.`);
        const resource = types.find((item) => item.code === row.resource_code);
        if (!resource) throw new Error(`No resource type exists for ${row.resource_code}.`);
        const matching = listed.filter((account) => eligible(account, row, resource.resource_type_id));
        if (choice !== "__create__" && !matching.some((account) => account.account_id === choice)) {
          throw new Error(`The selected account for ${row.external_account_key} is no longer eligible.`);
        }
        const createdId = createdAccounts.current[row.external_account_key];
        const created = listed.find((account) => account.account_id === createdId);
        const sameName = listed.find((account) => account.name === name && account.chart_id === context.chart_id);
        if (choice === "__create__" && sameName && sameName.account_id !== createdId) {
          throw new Error(`Account name already exists: ${name}. Select that existing account explicitly or enter a different name.`);
        }
        if (createdId && choice === "__create__" && (!created || created.name !== name || !eligible(created, row, resource.resource_type_id))) {
          throw new Error(`The account already created for ${row.external_account_key} no longer matches these choices. Select it explicitly, or restore the pre-import backup before restarting setup.`);
        }
        return { row, name, parent: parentChoice(row), resource, created: choice === "__create__" ? created : null, accountId: choice === "__create__" ? createdId : choice };
      });
      const byKey = new Map(plans.map((plan) => [plan.row.external_account_key, plan]));
      const sorted = [];
      const visiting = new Set();
      const visited = new Set();
      // Validate the entire hierarchy before creating anything, then create parents first.
      function visit(plan) {
        const key = plan.row.external_account_key;
        if (visited.has(key)) return;
        if (visiting.has(key)) throw new Error(`Parent hierarchy contains a cycle at ${key}.`);
        visiting.add(key);
        if (!plan.accountId || plan.created) {
          if (plan.parent.startsWith("definition:")) {
            const parentKey = plan.parent.slice("definition:".length);
            const parent = byKey.get(parentKey);
            if (!parent) throw new Error(`Unknown parent account definition: ${parentKey}.`);
            visit(parent);
          } else if (plan.parent && !listed.some((account) => account.account_id === plan.parent && account.chart_id === context.chart_id)) {
            throw new Error(`The selected parent for ${key} is no longer available.`);
          }
        }
        visiting.delete(key); visited.add(key); sorted.push(plan);
      }
      plans.forEach(visit);
      const resolved = {};
      for (const plan of sorted) {
        const key = plan.row.external_account_key;
        const parentId = plan.parent.startsWith("definition:") ? resolved[plan.parent.slice("definition:".length)] : plan.parent || null;
        if (plan.created && (plan.created.parent_account_id || null) !== parentId) {
          throw new Error(`Parent choice changed for an account already created (${key}). Select the existing account explicitly, or restore the pre-import backup.`);
        }
        if (plan.accountId) { resolved[key] = plan.accountId; continue; }
        const spec = {
          chart_id: context.chart_id, name: plan.name,
          code: null, account_type: plan.row.proposed_account_type, resource_type_id: plan.resource.resource_type_id,
          parent_account_id: parentId, associated_entity_id: hasIdentities ? associatedId(plan.row) : null,
          validation_rules: {}, metadata: {},
        };
        const fingerprint = JSON.stringify(spec);
        if (accountOps.current[key] && accountOps.current[key].fingerprint !== fingerprint) {
          throw new Error(`Creation was already attempted for ${key} with different choices. Restore the pre-import backup before changing those choices.`);
        }
        accountOps.current[key] ||= { id: crypto.randomUUID(), fingerprint };
        const created = await api(`/api/books/${bookId}/accounts`, { method: "POST", body: JSON.stringify({ op_id: accountOps.current[key].id, ...spec }) });
        if (!created.ok) throw new Error(`${key}: ${created.body.message || "Account creation failed. Check create_account permission."}`);
        createdAccounts.current[key] = created.body.id;
        resolved[key] = created.body.id;
        setMappings({ ...resolved });
      }
      setMappings(resolved);
      if (!await loadAccounts(context.chart_id)) throw new Error("Accounts were created, but the account list could not be refreshed. Retry Finish mapping before posting.");
      setMappingFinished(true);
      setMessage("Mapping complete. Review the resolved accounts, then post opening balances when ready.");
    } catch (cause) { setError(`${cause.message} Any accounts already created remain in the book. A verified pre-import backup is the recovery point.`); }
    setBusy(false);
  }

  async function submit(event) {
    event.preventDefault();
    if (busy || posted || !mappingFinished || rows.some((row) => !mappings[row.external_account_key])) return;
    setBusy(true); setError(""); setMessage("");
    const result = await api(`/api/books/${bookId}/opening-import`, {
      method: "POST",
      body: JSON.stringify({ file_content: fileText, chart_id: context.chart_id, account_mappings: mappings,
        workflow: { workflow_id: WORKFLOW_ID, workflow_deployment_id: WORKFLOW_DEPLOYMENT_ID, workflow_execution_id: crypto.randomUUID() } }),
    });
    if (result.ok) {
      setPosted(result.body);
      setMessage(`Opening balances posted as entry ${result.body.id}. ${version === "1.2" ? "Asset register and starting schedules imported. Projected depreciation has not been posted." : "Verify balances against the reviewed source file."}`);
      await loadVerification();
    }
    else setError(`${result.body.error_code || "ERROR"}: ${result.body.message || "Import failed"}`);
    setBusy(false);
  }

  async function loadVerification() {
    setReviewed(true);
    const result = await api(`/api/books/${bookId}/opening-import/review`);
    if (!result.ok) {
      setBalances({}); setReviewBalances([]); setRegister(null); setProfiles(null);
      setVerificationErrors([`${result.body.error_code || "REVIEW_ERROR"}: ${result.body.message || "Could not read import review."} Review requires all three permissions: list_accounts, list_account_balances, and list_fixed_assets. Assign the missing permissions and refresh verification; do not re-import.`]);
      return;
    }
    setBalances(Object.fromEntries(result.body.balances.map((item) => [item.account_id, item.balance])));
    setReviewBalances(result.body.balances);
    setRegister(result.body.assets);
    setProfiles(result.body.property_profiles);
    setVerificationErrors([]);
  }

  async function refreshVerification() {
    setBusy(true);
    try { setEditingAsset(null); await loadVerification(); } finally { setBusy(false); }
  }

  function editAsset(asset) {
    setEditingAsset(asset); setAssetName(asset.name); setScheduleAmounts(asset.schedules.map((schedule) => schedule.amount));
    setError(""); setMessage(""); assetOp.current = null;
  }

  async function saveAssetReview(event) {
    event.preventDefault();
    if (busy || !editingAsset) return;
    setError(""); setMessage("");
    if (!assetName.trim()) { setError("Enter an asset name."); return; }
    if (scheduleAmounts.some((amount) => !/^\d+(\.\d+)?$/.test(amount))) { setError("Enter nonnegative decimal schedule amounts without commas."); return; }
    // Send the current revision and preserve all ledger-linked facts unchanged.
    const asset = { ...editingAsset, name: assetName.trim(), schedules: editingAsset.schedules.map((schedule, index) => ({ ...schedule, amount: scheduleAmounts[index] })) };
    const fingerprint = JSON.stringify(asset);
    if (assetOp.current?.fingerprint !== fingerprint) assetOp.current = { fingerprint, id: crypto.randomUUID() };
    setBusy(true);
    try {
      const result = await api(`/api/books/${bookId}/fixed-assets/${asset.asset_id}`, { method: "PATCH", body: JSON.stringify({ op_id: assetOp.current.id, asset }) });
      if (!result.ok) { setError(`${result.body.error_code || "ERROR"}: ${result.body.message || "Asset review update failed."} Review updates require update_fixed_asset. If the record changed elsewhere, refresh verification and review the latest values.`); return; }
      setEditingAsset(null);
      setMessage("Asset name and starting schedules updated. No journal entry or depreciation expense was posted.");
      await loadVerification();
    } finally { setBusy(false); }
  }

  if (!bookId) return e("main", { className: "box" }, e("h1", null, WORKFLOW_NAME), e("p", { className: "error" }, "Launch this workflow from FPA My workflows."));
  return e("main", { className: "box" },
    e("a", { href: "/" }, "Return to FPA"), e("h1", null, WORKFLOW_NAME),
    e("p", null, "Import a reviewed opening-balance JSON package (1.0, 1.1, or 1.2). Versions 1.1 and 1.2 prepare property identities first. Version 1.2 also imports property facts, all declared accounts, and fixed-asset starting schedules."),
    e("p", { className: "error" }, "Re-importing with a new import ID can double the balances. Verify a book backup before importing real data. Identity and account setup occur before final import; restore that backup to recover from partial setup."),
    context && e("p", { className: "muted" }, `Active chart: ${context.chart_name}`),
    context && e("button", { type: "button", disabled: busy, onClick: refreshVerification }, "Review current register and balances"),
    e("label", null, "Reviewed JSON file", e("input", { type: "file", accept: ".json,application/json", onChange: loadFile, disabled: busy })),
    packageData && e("section", null,
      e("h2", null, "Package preview"),
      e("p", null, `Version ${version}; ${rows.length} account definitions; ${packageData.balance_rows.length} opening balance rows; opening date ${packageData.opening_entry_date}.`),
      packageData.related_entities?.length > 0 && table("Property identities", ["Key", "Name", "Category", "Relationship", "Effective from"], packageData.related_entities.map((item) => [item.external_entity_key, item.name, item.category, item.relationship, item.effective_from])),
      packageData.property_facts?.length > 0 && table("Property facts preview", ["Property", "Address", "Type", "Source year", "Rental days", "Personal-use days", "Source reference"], packageData.property_facts.map((item) => [propertyName(item.external_entity_key), item.address, item.property_type, item.source_year, item.fair_rental_days, item.personal_use_days, sourceRef(item, packageData.source_material)])),
      table("Opening balances preview", ["Account", "Property", "Resource", "Debit", "Credit", "Source reference"], packageData.balance_rows.map((row) => [row.external_account_key, row.attribution_entity_key ? propertyName(row.attribution_entity_key) : "Book-wide", row.resource_code, row.debit, row.credit, sourceRef(row, packageData.source_material)])),
      version === "1.2" && e("div", null,
        e("h3", null, "Fixed assets and starting schedules"),
        e("p", null, "Schedules preserve reviewed projections and basis facts. Final depreciation posting is deferred; projected amounts are not new journal activity."),
        assetTable("Fixed assets preview", packageData.fixed_assets, propertyName, accountName, packageData.source_material),
        e("p", null, "Final import requires create_fixed_asset permission. Combined register, property-profile, and balance review requires all three permissions: list_accounts, list_account_balances, and list_fixed_assets. Asset-list permission alone does not grant access to account balances. These are separate grants; the workflow role does not implicitly grant them.")
      ),
      table("Source material", ["Source ID", "Label", "SHA-256"], (packageData.source_material || []).map((source) => [source.source_id, source.label, source.sha256]))
    ),
    hasIdentities && e("section", null,
      e("h2", null, "Phase 1: prepare property identities"),
      e("p", null, "Validate the full package and atomically create or reuse its property IDs and ownership links. No balances post in this phase."),
      e("button", { type: "button", onClick: prepare, disabled: busy || !!posted || !context || !rows.length }, busy ? "Preparing..." : "Prepare identities"),
      prepared && e("div", null, e("p", { className: "success" }, `Prepared ${prepared.resolved_entities.length} properties.`),
        ...prepared.resolved_entities.map((item) => e("p", { key: item.external_entity_key }, `${item.name}: ${item.entity_id}`)))
    ),
    e("p", null, "Create new is the default for every account definition, including zero-opening-balance accounts. Review names and parent hierarchy before Finish mapping. Existing accounts must be selected explicitly. Creating accounts requires separate create_account permission; Finish mapping does not post balances."),
    context && e("button", { type: "button", disabled: busy, onClick: () => loadAccounts(context.chart_id) }, "Refresh account choices"),
    e("form", { onSubmit: submit },
      e("h2", null, hasIdentities ? "Phase 2: map and post" : "Map and post"),
      ...rows.map((row, index) => e("fieldset", { key: `${row.external_account_key}-${index}`, disabled: busy || !!posted },
        e("legend", null, `${row.external_account_key}: ${row.proposed_account_name} (${row.proposed_account_type}, ${row.resource_code}; ${row.attribution_entity_key || "book-wide"})`),
        !packageData.balance_rows.some((balance) => balance.external_account_key === row.external_account_key) && e("p", { className: "muted" }, "Zero opening balance: create/map this account for future transactions."),
        e("label", null, "Account mapping", e("select", { "aria-label": `Account mapping for ${row.external_account_key}`, value: choices[row.external_account_key] || "__create__", onChange: (event) => { setChoices((current) => ({ ...current, [row.external_account_key]: event.target.value })); setMappingFinished(false); } },
          e("option", { value: "__create__" }, "Create new"),
          ...accounts.filter((account) => context && eligible(account, row, resources.find((item) => item.code === row.resource_code)?.resource_type_id))
            .map((account) => e("option", { key: account.account_id, value: account.account_id }, `${account.code ? `${account.code} - ` : ""}${account.name}`))
        )),
        (choices[row.external_account_key] || "__create__") === "__create__" && e("div", null,
          e("label", null, "New account name", e("input", { "aria-label": `New account name for ${row.external_account_key}`, value: overrides[row.external_account_key]?.name ?? row.proposed_account_name,
            onChange: (event) => changeOverride(row.external_account_key, "name", event.target.value) })),
          e("label", null, "Parent account", e("select", { "aria-label": `Parent account for ${row.external_account_key}`, value: parentChoice(row), onChange: (event) => changeOverride(row.external_account_key, "parent", event.target.value) },
            e("option", { value: "" }, "No parent (top level)"),
            ...rows.filter((item) => item.external_account_key !== row.external_account_key).map((item) => e("option", { key: `definition:${item.external_account_key}`, value: `definition:${item.external_account_key}` }, `Import account: ${overrides[item.external_account_key]?.name ?? item.proposed_account_name}`)),
            ...accounts.filter((account) => account.chart_id === context?.chart_id && account.account_id !== createdAccounts.current[row.external_account_key]).map((account) => e("option", { key: account.account_id, value: account.account_id }, `Existing account: ${account.name}`))
          ))
        ),
        mappings[row.external_account_key] && e("small", { className: "muted" }, `Mapped to: ${accounts.find((account) => account.account_id === mappings[row.external_account_key])?.name || mappings[row.external_account_key]}`)
      )),
      e("button", { type: "button", onClick: finishMapping, disabled: busy || !!posted || !context || !rows.length || (hasIdentities && !prepared) }, busy ? "Working..." : "Finish mapping"),
      mappingFinished && !posted && e("p", { className: "success" }, `${Object.keys(mappings).length} account definitions mapped. No balances posted yet.`),
      e("button", { type: "submit", disabled: busy || !!posted || !mappingFinished || rows.some((row) => !mappings[row.external_account_key]) }, busy ? "Importing..." : "Post opening balances")
    ),
    (posted || reviewed) && e("section", null,
      e("h2", null, posted ? "Post-import verification" : "Book register and balances"),
      e("p", null, posted ? `Imported entry: ${posted.id}. Verification reads do not post any activity.` : "Review the book's existing asset register, property profiles, and account balances. No file or new import is required. Review requires all three permissions: list_accounts, list_account_balances, and list_fixed_assets."),
      e("button", { type: "button", disabled: busy, onClick: refreshVerification }, "Refresh verification"),
      ...verificationErrors.map((warning, index) => e("p", { key: index, className: "error", role: "alert" }, warning)),
      table("Imported account balances", ["External key", "Account", "Debit total", "Credit total", "Natural balance"], posted ? rows.map((row) => {
        const balance = balances[mappings[row.external_account_key]];
        return [row.external_account_key, accountName(row.external_account_key), balance?.debit_total ?? "Unavailable", balance?.credit_total ?? "Unavailable", balance?.natural ?? "Unavailable"];
      }) : reviewBalances.map((item) => [item.account_id, item.account_name, item.balance.debit_total, item.balance.credit_total, item.balance.natural])),
      e("h3", null, "Imported asset register"),
      e("p", null, "Starting schedules and projected depreciation are register facts, not final depreciation postings."),
      register && assetTable("Imported asset register", posted && version === "1.2" ? register.filter((asset) => asset.external_namespace === packageData.entity_namespace && packageData.fixed_assets.some((item) => item.external_asset_key === asset.external_key)) : register, propertyName, accountName, packageData?.source_material, editAsset, busy),
      e("p", null, "Review updates require update_fixed_asset and change only names and schedule amounts. Acquisitions and improvements need a balanced transaction linked to the asset register; this import workflow does not provide that entry form."),
      editingAsset && e("form", { onSubmit: saveAssetReview, "aria-label": "Asset name and schedule review" },
        e("h3", null, "Review asset name and schedules"),
        e("p", null, "Cost, land, depreciation basis, account links, and existing balances remain unchanged. Schedule changes do not post depreciation."),
        e("fieldset", { disabled: busy },
          e("label", null, "Asset name", e("input", { required: true, value: assetName, onChange: (event) => setAssetName(event.target.value) })),
          ...editingAsset.schedules.map((schedule, index) => e("label", { key: index }, `${schedule.year} schedule amount (${schedule.status}; ${schedule.basis})`, e("input", {
            "aria-label": `${schedule.year} schedule amount`, required: true, inputMode: "decimal", value: scheduleAmounts[index],
            onChange: (event) => setScheduleAmounts((current) => current.map((amount, position) => position === index ? event.target.value : amount)),
          }))),
          e("button", { type: "submit" }, "Save review changes"),
          e("button", { type: "button", onClick: () => setEditingAsset(null) }, "Cancel review")
        )
      ),
      profiles && table("Imported property profiles", ["Property", "Address", "Type", "Source year", "Rental days", "Personal-use days", "Source reference"], (posted && prepared ? profiles.filter((profile) => prepared.resolved_entities.some((item) => item.entity_id === profile.property_entity_id)) : profiles).map((profile) => [propertyName(profile.property_entity_id), profile.address, profile.property_type, profile.source_year, profile.fair_rental_days, profile.personal_use_days, sourceRef(profile, packageData?.source_material)])),
      register && e("details", null,
        e("summary", null, "Integration details for asset transactions"),
        e("p", null, "For an integrated transaction-entry tool. These links identify APIs, not standalone entry screens."),
        e("p", null, `Transaction API: POST /api/books/${bookId}/fixed-assets/transactions (balanced entry plus linked asset). Review API: PATCH /api/books/${bookId}/fixed-assets/{asset_id}.`),
        table("Asset integration links", ["Asset", "Asset ID", "Cost account ID", "Land account ID", "Accumulated depreciation account ID", "Depreciation expense account ID", "Linked entry", "Revision"], register.map((asset) => [asset.name, asset.asset_id, asset.cost_account_id, asset.land_account_id, asset.accumulated_depreciation_account_id, asset.depreciation_expense_account_id, asset.linked_entry_id, asset.revision]))
      )
    ),
    error && e("p", { className: "error" }, error), message && e("p", { className: "success" }, message)
  );
}

createRoot(document.getElementById("root")).render(e(App));
