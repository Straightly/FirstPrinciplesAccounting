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
  const [error, setError] = React.useState("");
  const [message, setMessage] = React.useState("");
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    document.title = `${WORKFLOW_NAME} - FPA`;
    if (!bookId) return;
    api(`/api/books/${bookId}/opening-import/context?workflow_deployment_id=${WORKFLOW_DEPLOYMENT_ID}`).then(async (result) => {
      if (!result.ok) { setError(result.body.message || "Opening-import workflow role is required."); return; }
      setContext(result.body);
      const listed = await api(`/api/books/${bookId}/accounts?chart_id=${result.body.chart_id}`);
      if (!listed.ok) { setError(listed.body.message || "A separate account-list role is required."); return; }
      setAccounts(listed.body.filter((account) => account.is_active));
    });
  }, [bookId]);

  async function loadFile(event) {
    setError(""); setMessage(""); setRows([]); setMappings({});
    const file = event.target.files?.[0];
    if (!file) return;
    if (file.size > 1000000) { setError("The import file exceeds 1 MB."); return; }
    const text = await file.text();
    try {
      const parsed = JSON.parse(text);
      if (!Array.isArray(parsed.balance_rows) || !parsed.balance_rows.length) throw new Error("No balance rows found.");
      setFileText(text);
      setRows(parsed.balance_rows);
    } catch (cause) { setFileText(""); setError(`Cannot inspect file: ${cause.message}`); }
  }

  async function submit(event) {
    event.preventDefault(); setBusy(true); setError(""); setMessage("");
    const result = await api(`/api/books/${bookId}/opening-import`, {
      method: "POST",
      body: JSON.stringify({ file_content: fileText, chart_id: context.chart_id, account_mappings: mappings,
        workflow: { workflow_id: WORKFLOW_ID, workflow_deployment_id: WORKFLOW_DEPLOYMENT_ID, workflow_execution_id: crypto.randomUUID() } }),
    });
    setBusy(false);
    if (result.ok) setMessage(`Opening balances posted as entry ${result.body.id}. Verify the balances against the reviewed source file.`);
    else setError(`${result.body.error_code || "ERROR"}: ${result.body.message || "Import failed"}`);
  }

  if (!bookId) return e("main", { className: "box" }, e("h1", null, WORKFLOW_NAME), e("p", { className: "error" }, "Launch this workflow from FPA My workflows."));
  return e("main", { className: "box" },
    e("a", { href: "/" }, "Return to FPA"), e("h1", null, WORKFLOW_NAME),
    e("p", null, "Import a reviewed version 1.0 opening-balance JSON package. Posting is one atomic journal entry."),
    e("p", { className: "error" }, "Re-importing with a new import ID can double the balances. Back up the book before importing real data."),
    context && e("p", { className: "muted" }, `Active chart: ${context.chart_name}`),
    e("form", { onSubmit: submit },
      e("label", null, "Reviewed JSON file", e("input", { type: "file", accept: ".json,application/json", required: true, onChange: loadFile })),
      ...rows.map((row, index) => e("label", { key: `${row.external_account_key}-${index}` },
        `${row.external_account_key}: ${row.proposed_account_name} (${row.proposed_account_type}, ${row.resource_code}; Dr ${row.debit}, Cr ${row.credit})`,
        e("select", { required: true, value: mappings[row.external_account_key] || "", onChange: (event) => setMappings((current) => ({ ...current, [row.external_account_key]: event.target.value })) },
          e("option", { value: "" }, "Choose an existing account"),
          ...accounts.filter((account) => account.account_type === row.proposed_account_type).map((account) => e("option", { key: account.account_id, value: account.account_id }, `${account.code ? `${account.code} - ` : ""}${account.name}`))
        )
      )),
      e("button", { type: "submit", disabled: busy || !context || !rows.length || !accounts.length || rows.some((row) => !mappings[row.external_account_key]) }, busy ? "Importing..." : "Post opening balances")
    ),
    error && e("p", { className: "error" }, error), message && e("p", { className: "success" }, message)
  );
}

createRoot(document.getElementById("root")).render(e(App));
