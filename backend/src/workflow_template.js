import { React, createRoot } from "./workflow-react.js";

const WORKFLOW_ID = "__WORKFLOW_ID__";
const WORKFLOW_DEPLOYMENT_ID = "__WORKFLOW_DEPLOYMENT_ID__";
const WORKFLOW_NAME = __WORKFLOW_NAME_JSON__;
const e = React.createElement;

async function api(path, options = {}) {
  const response = await fetch(path, {
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", ...(options.headers || {}) },
    ...options,
  });
  const body = await response.json().catch(() => ({}));
  return { ok: response.ok, status: response.status, body };
}

function App() {
  const query = new URLSearchParams(window.location.search);
  const bookId = query.get("book_id");
  const entityId = query.get("entity_id");
  const [accounts, setAccounts] = React.useState([]);
  const [authorized, setAuthorized] = React.useState(null);
  const [form, setForm] = React.useState({
    entry_date: new Date().toISOString().slice(0, 10),
    description: "",
    amount: "",
    direction: "debit",
    primary_account_id: "",
    offset_account_id: "",
    memo: "",
  });
  const [message, setMessage] = React.useState("");
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    document.title = `${WORKFLOW_NAME} — FPA`;
    if (!bookId || !entityId) return;
    api(`/api/books/${bookId}/workflows/mine?entity_id=${entityId}`).then((result) => {
      setAuthorized(result.ok && result.body.some((workflow) => workflow.workflow_id === WORKFLOW_ID));
    });
    api(`/api/books/${bookId}/charts?entity_id=${entityId}`).then(async (chartsResult) => {
      if (!chartsResult.ok) return;
      const active = chartsResult.body.find((chart) => chart.is_active);
      if (!active) return;
      const accountsResult = await api(`/api/books/${bookId}/accounts?chart_id=${active.chart_id}`);
      if (accountsResult.ok) setAccounts(accountsResult.body.filter((account) => account.is_active));
    });
  }, [bookId, entityId]);

  function field(name) {
    return (event) => setForm((current) => ({ ...current, [name]: event.target.value }));
  }

  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    setMessage("");
    const primaryDebit = form.direction === "debit";
    const response = await api(`/api/books/${bookId}/entries`, {
      method: "POST",
      body: JSON.stringify({
        entry_id: crypto.randomUUID(),
        entity_id: entityId,
        entry_date: form.entry_date,
        description: form.description,
        source: "WORKFLOW",
        workflow: {
          workflow_id: WORKFLOW_ID,
          workflow_deployment_id: WORKFLOW_DEPLOYMENT_ID,
          workflow_execution_id: crypto.randomUUID(),
        },
        lines: [
          { line_id: crypto.randomUUID(), account_id: form.primary_account_id, attribution_entity_id: accounts.find((account) => account.account_id === form.primary_account_id)?.associated_entity_id || null, debit_amount: primaryDebit ? form.amount : null, credit_amount: primaryDebit ? null : form.amount, memo: form.memo || null },
          { line_id: crypto.randomUUID(), account_id: form.offset_account_id, attribution_entity_id: accounts.find((account) => account.account_id === form.offset_account_id)?.associated_entity_id || null, debit_amount: primaryDebit ? null : form.amount, credit_amount: primaryDebit ? form.amount : null, memo: form.memo || null },
        ],
      }),
    });
    setBusy(false);
    if (response.ok) {
      setMessage(`Posted entry ${response.body.id}.`);
      setForm((current) => ({ ...current, description: "", amount: "", memo: "" }));
    } else {
      setError(`${response.body.error_code || "ERROR"}: ${response.body.message || "Request failed"}`);
    }
  }

  if (!bookId || !entityId) return e("main", { className: "box" }, e("h1", null, WORKFLOW_NAME), e("p", { className: "error" }, "Launch this workflow from FPA My workflows."));
  if (authorized === false) return e("main", { className: "box" }, e("h1", null, WORKFLOW_NAME), e("p", { className: "error" }, "You are not authorized to run this workflow."), e("a", { href: "/" }, "Return to FPA"));

  const accountOptions = [e("option", { key: "", value: "" }, "Choose an account…"), ...accounts.map((account) => e("option", { key: account.account_id, value: account.account_id }, `${account.code ? `${account.code} — ` : ""}${account.name}`))];
  return e("main", { className: "box" },
    e("a", { href: "/" }, "← Return to FPA"),
    e("h1", null, WORKFLOW_NAME),
    authorized === null && e("p", { className: "muted" }, "Checking access…"),
    e("form", { onSubmit: submit },
      e("label", null, "Entry date", e("input", { type: "date", required: true, value: form.entry_date, onChange: field("entry_date") })),
      e("label", null, "Description", e("input", { required: true, value: form.description, onChange: field("description") })),
      e("label", null, "Amount", e("input", { type: "number", min: "0.01", step: "0.01", required: true, value: form.amount, onChange: field("amount") })),
      e("label", null, "Primary-account side", e("select", { value: form.direction, onChange: field("direction") }, e("option", { value: "debit" }, "Debit"), e("option", { value: "credit" }, "Credit"))),
      e("label", null, "Primary account", e("select", { required: true, value: form.primary_account_id, onChange: field("primary_account_id") }, ...accountOptions)),
      e("label", null, "Offset account", e("select", { required: true, value: form.offset_account_id, onChange: field("offset_account_id") }, ...accountOptions)),
      e("label", null, "Memo (optional)", e("input", { value: form.memo, onChange: field("memo") })),
      e("button", { type: "submit", disabled: busy || authorized !== true || accounts.length < 2 }, busy ? "Posting…" : "Post entry")
    ),
    message && e("p", { className: "success" }, message),
    error && e("p", { className: "error" }, error)
  );
}

createRoot(document.getElementById("root")).render(e(App));
