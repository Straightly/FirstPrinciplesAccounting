import { React, createRoot } from "./workflow-react.js";

const WORKFLOW_ID = "__WORKFLOW_ID__";
const WORKFLOW_DEPLOYMENT_ID = "__WORKFLOW_DEPLOYMENT_ID__";
const WORKFLOW_NAME = __WORKFLOW_NAME_JSON__;
const e = React.createElement;

async function api(path, options = {}) {
  const response = await fetch(path, { credentials: "same-origin", headers: { "Content-Type": "application/json" }, ...options });
  const body = await response.json().catch(() => ({}));
  return { ok: response.ok, body };
}

function App() {
  const query = new URLSearchParams(window.location.search);
  const bookId = query.get("book_id");
  const [context, setContext] = React.useState(null);
  const [accounts, setAccounts] = React.useState([]);
  const [resources, setResources] = React.useState([]);
  const [fileText, setFileText] = React.useState("");
  const [rows, setRows] = React.useState([]);
  const [choices, setChoices] = React.useState({});
  const [mappings, setMappings] = React.useState({});
  const [mappingFinished, setMappingFinished] = React.useState(false);
  const [version, setVersion] = React.useState("");
  const [prepared, setPrepared] = React.useState(null);
  const [error, setError] = React.useState("");
  const [message, setMessage] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const accountOps = React.useRef({});

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
    return prepared?.resolved_entities.find((item) => item.external_entity_key === row.attribution_entity_key)?.entity_id || null;
  }

  function eligible(account, row, resourceId) {
    return account.chart_id === context.chart_id && account.account_type === row.proposed_account_type &&
      account.resource_type_id === resourceId &&
      (version !== "1.1" || (account.associated_entity_id || null) === associatedId(row));
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
    setError(""); setMessage(""); setRows([]); setChoices({}); setMappings({}); setMappingFinished(false); setPrepared(null); setVersion(""); accountOps.current = {};
    const file = event.target.files?.[0];
    if (!file) return;
    if (file.size > 1000000) { setError("The import file exceeds 1 MB."); return; }
    const text = await file.text();
    try {
      const parsed = JSON.parse(text);
      if (!Array.isArray(parsed.balance_rows) || !parsed.balance_rows.length) throw new Error("No balance rows found.");
      if (!["1.0", "1.1"].includes(parsed.schema_version)) throw new Error("Unsupported import schema version.");
      setFileText(text);
      setRows(parsed.balance_rows);
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
        if (choice === "__create__" && plannedNames.has(row.proposed_account_name)) {
          throw new Error(`Multiple rows propose the same new account name: ${row.proposed_account_name}. Select existing accounts or revise the file before creating accounts.`);
        }
        if (choice === "__create__") plannedNames.add(row.proposed_account_name);
        const resource = types.find((item) => item.code === row.resource_code);
        if (!resource) throw new Error(`No resource type exists for ${row.resource_code}.`);
        const matching = listed.filter((account) => eligible(account, row, resource.resource_type_id));
        if (choice !== "__create__" && !matching.some((account) => account.account_id === choice)) {
          throw new Error(`The selected account for ${row.external_account_key} is no longer eligible.`);
        }
        const sameName = listed.find((account) => account.name === row.proposed_account_name && account.chart_id === context.chart_id);
        if (choice === "__create__" && sameName && !eligible(sameName, row, resource.resource_type_id)) {
          throw new Error(`Account name already exists but is not suitable: ${row.proposed_account_name}. Choose another account or resolve the name conflict.`);
        }
        return { row, resource, accountId: choice === "__create__" ? sameName?.account_id : choice };
      });
      const resolved = { ...mappings };
      for (const plan of plans) {
        const key = plan.row.external_account_key;
        if (plan.accountId) { resolved[key] = plan.accountId; continue; }
        accountOps.current[key] ||= crypto.randomUUID();
        const created = await api(`/api/books/${bookId}/accounts`, { method: "POST", body: JSON.stringify({
          op_id: accountOps.current[key], chart_id: context.chart_id, name: plan.row.proposed_account_name,
          code: null, account_type: plan.row.proposed_account_type, resource_type_id: plan.resource.resource_type_id,
          parent_account_id: null, associated_entity_id: version === "1.1" ? associatedId(plan.row) : null,
          validation_rules: {}, metadata: {},
        }) });
        if (!created.ok) throw new Error(`${key}: ${created.body.message || "Account creation failed. Check create_account permission."}`);
        resolved[key] = created.body.id;
        setMappings({ ...resolved });
      }
      setMappings(resolved);
      if (!await loadAccounts(context.chart_id)) throw new Error("Accounts were created, but the account list could not be refreshed. Retry Finish mapping before posting.");
      setMappingFinished(true);
      setMessage("Mapping complete. Review the resolved accounts, then post opening balances when ready.");
    } catch (cause) { setError(`${cause.message} Any accounts already created remain in the book. Correct the issue and retry Finish mapping; matching accounts will be reused.`); }
    setBusy(false);
  }

  async function submit(event) {
    event.preventDefault(); setBusy(true); setError(""); setMessage("");
    const result = await api(`/api/books/${bookId}/opening-import`, {
      method: "POST",
      body: JSON.stringify({ file_content: fileText, chart_id: context.chart_id, account_mappings: mappings,
        workflow: { workflow_id: WORKFLOW_ID, workflow_deployment_id: WORKFLOW_DEPLOYMENT_ID, workflow_execution_id: crypto.randomUUID() } }),
    });
    setBusy(false);
    if (result.ok) setMessage(`Opening balances posted as entry ${result.body.id}. Verify property balances against the reviewed source file.`);
    else setError(`${result.body.error_code || "ERROR"}: ${result.body.message || "Import failed"}`);
  }

  if (!bookId) return e("main", { className: "box" }, e("h1", null, WORKFLOW_NAME), e("p", { className: "error" }, "Launch this workflow from FPA My workflows."));
  return e("main", { className: "box" },
    e("a", { href: "/" }, "Return to FPA"), e("h1", null, WORKFLOW_NAME),
    e("p", null, "Import a reviewed opening-balance JSON package. Version 1.1 first prepares property identities, then posts one atomic journal entry."),
    e("p", { className: "error" }, "Re-importing with a new import ID can double the balances. Back up the book before importing real data."),
    context && e("p", { className: "muted" }, `Active chart: ${context.chart_name}`),
    e("label", null, "Reviewed JSON file", e("input", { type: "file", accept: ".json,application/json", onChange: loadFile })),
    version === "1.1" && e("section", null,
      e("h2", null, "Phase 1: prepare property identities"),
      e("p", null, "Validate the full package and atomically create or reuse its property IDs and ownership links. No balances post in this phase."),
      e("button", { type: "button", onClick: prepare, disabled: busy || !context || !rows.length }, busy ? "Preparing..." : "Prepare identities"),
      prepared && e("div", null, e("p", { className: "success" }, `Prepared ${prepared.resolved_entities.length} properties.`),
        ...prepared.resolved_entities.map((item) => e("p", { key: item.external_entity_key }, `${item.name}: ${item.entity_id}`)))
    ),
    e("p", null, "Create new is the default for every row. Finish mapping creates missing accounts; it does not post balances. Creating accounts requires separate create_account permission."),
    context && e("button", { type: "button", onClick: () => loadAccounts(context.chart_id) }, "Refresh account choices"),
    e("form", { onSubmit: submit },
      e("h2", null, version === "1.1" ? "Phase 2: map and post" : "Map and post"),
      ...rows.map((row, index) => e("label", { key: `${row.external_account_key}-${index}` },
        `${row.external_account_key}: ${row.proposed_account_name} (${row.proposed_account_type}, ${row.resource_code}; ${row.attribution_entity_key || "book-wide"}; Dr ${row.debit}, Cr ${row.credit})`,
        e("select", { value: choices[row.external_account_key] || "__create__", onChange: (event) => { setChoices((current) => ({ ...current, [row.external_account_key]: event.target.value })); setMappingFinished(false); } },
          e("option", { value: "__create__" }, "Create new"),
          ...accounts.filter((account) => account.account_type === row.proposed_account_type &&
            account.resource_type_id === resources.find((item) => item.code === row.resource_code)?.resource_type_id &&
            (version !== "1.1" || (account.associated_entity_id || null) === associatedId(row)))
            .map((account) => e("option", { key: account.account_id, value: account.account_id }, `${account.code ? `${account.code} - ` : ""}${account.name}`))
        ),
        mappings[row.external_account_key] && e("small", { className: "muted" }, `Mapped to: ${accounts.find((account) => account.account_id === mappings[row.external_account_key])?.name || mappings[row.external_account_key]}`)
      )),
      e("button", { type: "button", onClick: finishMapping, disabled: busy || !context || !rows.length || (version === "1.1" && !prepared) }, busy ? "Working..." : "Finish mapping"),
      mappingFinished && e("p", { className: "success" }, `${Object.keys(mappings).length} account rows mapped. No balances posted yet.`),
      e("button", { type: "submit", disabled: busy || !mappingFinished || rows.some((row) => !mappings[row.external_account_key]) }, busy ? "Importing..." : "Post opening balances")
    ),
    error && e("p", { className: "error" }, error), message && e("p", { className: "success" }, message)
  );
}

createRoot(document.getElementById("root")).render(e(App));
