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
  const [fileText, setFileText] = React.useState("");
  const [rows, setRows] = React.useState([]);
  const [mappings, setMappings] = React.useState({});
  const [version, setVersion] = React.useState("");
  const [prepared, setPrepared] = React.useState(null);
  const [error, setError] = React.useState("");
  const [message, setMessage] = React.useState("");
  const [busy, setBusy] = React.useState(false);

  async function loadAccounts(chartId) {
    const listed = await api(`/api/books/${bookId}/accounts?chart_id=${chartId}`);
    if (!listed.ok) { setError(listed.body.message || "A separate account-list role is required."); return; }
    setAccounts(listed.body.filter((account) => account.is_active));
  }

  React.useEffect(() => {
    document.title = `${WORKFLOW_NAME} - FPA`;
    if (!bookId) return;
    api(`/api/books/${bookId}/opening-import/context?workflow_deployment_id=${WORKFLOW_DEPLOYMENT_ID}`).then(async (result) => {
      if (!result.ok) { setError(result.body.message || "Opening-import workflow role is required."); return; }
      setContext(result.body);
      await loadAccounts(result.body.chart_id);
    });
  }, [bookId]);

  async function loadFile(event) {
    setError(""); setMessage(""); setRows([]); setMappings({}); setPrepared(null); setVersion("");
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
    if (result.ok) { setPrepared(result.body); setMessage("Identity preparation complete. Map every row, then post the opening entry."); }
    else setError(`${result.body.error_code || "ERROR"}: ${result.body.message || "Identity preparation failed"}`);
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
    version === "1.1" && e("p", null, "Each property row needs an existing account associated with that property. Create missing accounts in FPA with separate create_account permission, then refresh this list."),
    context && e("button", { type: "button", onClick: () => loadAccounts(context.chart_id) }, "Refresh account choices"),
    e("form", { onSubmit: submit },
      e("h2", null, version === "1.1" ? "Phase 2: map and post" : "Map and post"),
      ...rows.map((row, index) => e("label", { key: `${row.external_account_key}-${index}` },
        `${row.external_account_key}: ${row.proposed_account_name} (${row.proposed_account_type}, ${row.resource_code}; ${row.attribution_entity_key || "book-wide"}; Dr ${row.debit}, Cr ${row.credit})`,
        e("select", { required: true, value: mappings[row.external_account_key] || "", onChange: (event) => setMappings((current) => ({ ...current, [row.external_account_key]: event.target.value })) },
          e("option", { value: "" }, "Choose an existing account"),
          ...accounts.filter((account) => account.account_type === row.proposed_account_type &&
            (version !== "1.1" || account.associated_entity_id === (prepared?.resolved_entities.find((item) => item.external_entity_key === row.attribution_entity_key)?.entity_id || null)))
            .map((account) => e("option", { key: account.account_id, value: account.account_id }, `${account.code ? `${account.code} - ` : ""}${account.name}`))
        )
      )),
      e("button", { type: "submit", disabled: busy || !context || !rows.length || !accounts.length || (version === "1.1" && !prepared) || rows.some((row) => !mappings[row.external_account_key]) }, busy ? "Importing..." : "Post opening balances")
    ),
    error && e("p", { className: "error" }, error), message && e("p", { className: "success" }, message)
  );
}

createRoot(document.getElementById("root")).render(e(App));
