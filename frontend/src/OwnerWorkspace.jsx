import { useCallback, useEffect, useMemo, useState } from "react";
import { api, errorText, newId, today } from "./api.js";

const emptyData = {
  entities: [], resourceTypes: [], charts: [], accounts: [], periods: [], prices: [],
  entries: [], audit: [], workflows: [], roles: [], artifacts: [], users: [], balances: {},
};

function newEntryDraft(current = {}) {
  return {
    entry_id: newId(),
    debit_line_id: newId(),
    credit_line_id: newId(),
    entry_date: current.entry_date || today(),
    description: "",
    debit_account: current.debit_account || "",
    credit_account: current.credit_account || "",
    amount: "",
    memo: "",
  };
}

function Button({ children, kind = "primary", ...props }) {
  return <button className={`button ${kind === "primary" ? "" : kind}`} {...props}>{children}</button>;
}
function Field({ label, children }) { return <label className="field">{label}{children}</label>; }
function Panel({ title, children, className = "" }) { return <section className={`panel ${className}`}><h2>{title}</h2>{children}</section>; }
function JsonDetails({ label = "Details", value }) { return <details><summary>{label}</summary><pre><code>{JSON.stringify(value, null, 2)}</code></pre></details>; }

export default function OwnerWorkspace({ me, book, isBookOwner, refreshEpoch, onChanged, setMessage, setError, selectBook }) {
  const [tab, setTab] = useState("books");
  const [data, setData] = useState(emptyData);
  const [loading, setLoading] = useState(false);
  const [createBook, setCreateBook] = useState({ name: "", passphrase: "" });
  const [openPassphrase, setOpenPassphrase] = useState("");
  const [backupLocation, setBackupLocation] = useState("");
  const [restoreLocation, setRestoreLocation] = useState("");
  const [resource, setResource] = useState({ name: "US Dollar", kind: "CURRENCY", code: "USD", unit_of_measure: "USD", precision: "2" });
  const [chart, setChart] = useState({ name: "Primary chart", description: "", activate: true, starter_template: "CORPORATE", resource_type_id: "" });
  const [copyChart, setCopyChart] = useState({ source: "", name: "", description: "", activate: false });
  const [account, setAccount] = useState({ chart_id: "", name: "", code: "", account_type: "ASSET", resource_type_id: "", parent_account_id: "" });
  const [accountEdit, setAccountEdit] = useState({ account_id: "", name: "", code: "" });
  const [period, setPeriod] = useState({ name: "", start_date: today(), end_date: today() });
  const [price, setPrice] = useState({ base_resource_type_id: "", quote_resource_type_id: "", rate: "", as_of: today() });
  const [entry, setEntry] = useState(() => newEntryDraft());
  const [entryError, setEntryError] = useState("");
  const [entryPosting, setEntryPosting] = useState(false);
  const [reverse, setReverse] = useState({ original_entry_id: "", entry_date: today(), description: "" });
  const [workflowArtifact, setWorkflowArtifact] = useState({ workflow_name: "", description: "", kind: "journal" });
  const [role, setRole] = useState({ name: "", description: "", permissions: [] });
  const [roleWorkflow, setRoleWorkflow] = useState({ role_id: "", workflow_id: "" });
  const [roleUser, setRoleUser] = useState({ role_id: "", user_email: "" });
  const [owner, setOwner] = useState({ email: "", currentPassphrase: "", newPassphrase: "", confirm: "", cancelPassphrase: "" });

  const request = useCallback(async (path, options, success) => {
    setError("");
    const result = await api(path, options);
    if (!result.ok) { setError(errorText(result)); return null; }
    if (success) setMessage(typeof success === "function" ? success(result.body) : success);
    return result.body;
  }, [setError, setMessage]);

  const loadData = useCallback(async () => {
    if (!book || !isBookOwner || !book.is_open || book.pending_owner_transfer) { setData(emptyData); return; }
    setLoading(true);
    const base = `/api/books/${book.book_id}`;
    const entity = encodeURIComponent(book.entity_id);
    const paths = [
      ["entities", `${base}/entities`], ["resourceTypes", `${base}/resource-types`],
      ["charts", `${base}/charts?entity_id=${entity}`], ["periods", `${base}/periods?entity_id=${entity}`],
      ["prices", `${base}/prices`], ["entries", `${base}/entries?entity_id=${entity}`],
      ["audit", `${base}/audit-log`], ["workflows", `${base}/workflows?entity_id=${entity}`],
      ["roles", `${base}/roles?entity_id=${entity}`], ["artifacts", `${base}/workflow-artifacts`], ["users", `${base}/users`],
    ];
    const results = await Promise.all(paths.map(async ([key, path]) => [key, await api(path)]));
    const next = { ...emptyData };
    for (const [key, result] of results) {
      if (!result.ok) { setLoading(false); setError(errorText(result)); return; }
      next[key] = result.body;
    }
    const accountResults = await Promise.all(next.charts.map((item) => api(`${base}/accounts?chart_id=${item.chart_id}`)));
    next.accounts = accountResults.flatMap((result) => result.ok ? result.body : []);
    const balanceResults = await Promise.all(next.accounts.map(async (item) => [item.account_id, await api(`${base}/accounts/${item.account_id}/balance`)]));
    next.balances = Object.fromEntries(balanceResults.filter(([, result]) => result.ok).map(([id, result]) => [id, result.body]));
    setData(next);
    setLoading(false);
  }, [book, isBookOwner, setError]);

  useEffect(() => { loadData(); }, [loadData, refreshEpoch]);
  useEffect(() => {
    const active = data.charts.find((item) => item.is_active) || data.charts[0];
    const currency = data.resourceTypes.find((item) => item.kind === "CURRENCY");
    setAccount((current) => ({ ...current, chart_id: current.chart_id || active?.chart_id || "", resource_type_id: current.resource_type_id || data.resourceTypes[0]?.resource_type_id || "" }));
    setChart((current) => ({ ...current, resource_type_id: current.resource_type_id || currency?.resource_type_id || "" }));
    setCopyChart((current) => ({ ...current, source: current.source || active?.chart_id || "" }));
    setPrice((current) => ({ ...current, base_resource_type_id: current.base_resource_type_id || data.resourceTypes[0]?.resource_type_id || "", quote_resource_type_id: current.quote_resource_type_id || data.resourceTypes[1]?.resource_type_id || "" }));
  }, [data.charts, data.resourceTypes]);

  async function changed(path, body, success, method = "POST") {
    const value = await request(path, { method, body: JSON.stringify(body) }, success);
    if (value !== null) { await loadData(); await onChanged("change"); }
    return value;
  }

  async function postEntry(event) {
    event.preventDefault();
    setEntryError("");
    setError("");
    setEntryPosting(true);
    const result = await api(`/api/books/${book.book_id}/entries`, {
      method: "POST",
      body: JSON.stringify({
        entry_id: entry.entry_id,
        entity_id: book.entity_id,
        entry_date: entry.entry_date,
        description: entry.description,
        source: "MANUAL",
        metadata: {},
        prices: [],
        lines: [
          { line_id: entry.debit_line_id, account_id: entry.debit_account, debit_amount: entry.amount, credit_amount: null, memo: entry.memo || null, metadata: {} },
          { line_id: entry.credit_line_id, account_id: entry.credit_account, debit_amount: null, credit_amount: entry.amount, memo: entry.memo || null, metadata: {} },
        ],
      }),
    });
    setEntryPosting(false);
    if (!result.ok) {
      const message = errorText(result);
      setEntryError(message);
      setError(message);
      return;
    }
    setMessage(`Entry ${entry.entry_id} posted.`);
    setEntry(newEntryDraft(entry));
    await loadData();
    await onChanged("change");
  }

  async function createBookSubmit(event) {
    event.preventDefault();
    const value = await request("/api/books", { method: "POST", body: JSON.stringify(createBook) }, "Book created and opened.");
    if (value) { setCreateBook({ name: "", passphrase: "" }); await onChanged("change"); selectBook(value.book_id); }
  }
  async function openBook(event) {
    event.preventDefault();
    const value = await request(`/api/books/${book.book_id}/open`, { method: "POST", body: JSON.stringify({ passphrase: openPassphrase }) }, "Book opened.");
    if (value) { setOpenPassphrase(""); await onChanged("change"); }
  }
  async function closeBook() {
    if (!window.confirm(`Close ${book.name}? Unsaved form input in this browser will remain, but accounting APIs will be unavailable until it is reopened.`)) return;
    const value = await request(`/api/books/${book.book_id}/close`, { method: "POST" }, "Book closed.");
    if (value) await onChanged("change");
  }

  const accountTree = useMemo(() => {
    const byParent = new Map();
    for (const item of data.accounts) {
      const key = item.parent_account_id || "";
      byParent.set(key, [...(byParent.get(key) || []), item]);
    }
    for (const children of byParent.values()) {
      children.sort((left, right) => (left.code || left.name).localeCompare(right.code || right.name));
    }
    const ordered = [];
    const append = (parentId, depth) => {
      for (const item of byParent.get(parentId) || []) {
        ordered.push({ ...item, depth });
        append(item.account_id, depth + 1);
      }
    };
    append("", 0);
    return ordered;
  }, [data.accounts]);
  const accountName = (id) => data.accounts.find((item) => item.account_id === id)?.name || id;
  const userName = (id) => data.users.find((item) => item.user_id === id)?.email || id;

  return <>
    {!book && <Panel title="Administration">
      <p>Select a book, or create/restore one below.</p>
      {me.is_bootstrap_owner && <div className="grid">
        <form onSubmit={createBookSubmit}><h3>Create book</h3><Field label="Book name"><input required value={createBook.name} onChange={(event) => setCreateBook({ ...createBook, name: event.target.value })} /></Field><Field label="Passphrase"><input type="password" minLength="8" required autoComplete="new-password" value={createBook.passphrase} onChange={(event) => setCreateBook({ ...createBook, passphrase: event.target.value })} /></Field><Button type="submit">Create and open</Button></form>
        <form onSubmit={async (event) => { event.preventDefault(); const value = await request("/api/books/restore", { method: "POST", body: JSON.stringify({ location: restoreLocation }) }, "Backup restored. Open it with its original passphrase."); if (value) { setRestoreLocation(""); await onChanged("change"); selectBook(value.book_id); } }}><h3>Restore book</h3><Field label="Server backup folder"><input required placeholder="/path/to/backup" value={restoreLocation} onChange={(event) => setRestoreLocation(event.target.value)} /></Field><Button type="submit">Restore encrypted backup</Button></form>
      </div>}
    </Panel>}

    {book?.pending_owner_transfer && <Panel title="Ownership transfer pending" className="danger-zone">
      <p className="warning">This book is frozen. No accounting, workflow, backup, close, or administrative operation is allowed until the transfer is accepted or cancelled.</p>
      <p>Current owner: <strong>{book.owner_email}</strong><br />Nominated successor: <strong>{book.pending_owner_transfer.new_owner_email}</strong></p>
      {isBookOwner && <>
        {!book.is_open && <><p>The service restarted or the book is no longer decrypted. Reopen it with the current passphrase before the successor can accept.</p><form onSubmit={openBook}><Field label="Current book passphrase"><input type="password" required autoComplete="new-password" value={openPassphrase} onChange={(event) => setOpenPassphrase(event.target.value)} /></Field><Button type="submit">Resume pending transfer</Button></form></>}
        {book.is_open && <form onSubmit={async (event) => { event.preventDefault(); if (!window.confirm(`Cancel the transfer of ${book.name} to ${book.pending_owner_transfer.new_owner_email}?`)) return; const value = await request(`/api/books/${book.book_id}/ownership-transfer/cancel`, { method: "POST", body: JSON.stringify({ current_passphrase: owner.cancelPassphrase }) }, "Ownership transfer cancelled; the book is operational again."); if (value) { setOwner({ email: "", currentPassphrase: "", newPassphrase: "", confirm: "", cancelPassphrase: "" }); await onChanged("change"); } }}><Field label="Current book passphrase"><input type="password" required autoComplete="new-password" value={owner.cancelPassphrase} onChange={(event) => setOwner({ ...owner, cancelPassphrase: event.target.value })} /></Field><Button kind="danger" type="submit">Cancel pending transfer</Button></form>}
      </>}
      {me.user.email.toLowerCase() === book.pending_owner_transfer.new_owner_email.toLowerCase() && <>
        {!book.is_open && <p className="warning">Acceptance is waiting for the current owner to resume the decrypted book after the service restart.</p>}
        {book.is_open && <form onSubmit={async (event) => { event.preventDefault(); if (owner.newPassphrase !== owner.confirm) { setError("New passphrase confirmation does not match."); return; } if (!window.confirm(`Accept ownership of ${book.name}? A fresh encrypted book copy will replace the current live copy.`)) return; const value = await request(`/api/books/${book.book_id}/ownership-transfer/accept`, { method: "POST", body: JSON.stringify({ op_id: newId(), new_passphrase: owner.newPassphrase }) }, `Ownership of ${book.name} accepted.`); if (value) { setOwner({ email: "", currentPassphrase: "", newPassphrase: "", confirm: "", cancelPassphrase: "" }); await onChanged("change"); } }}><Field label="Choose new book passphrase"><input type="password" minLength="8" required autoComplete="new-password" value={owner.newPassphrase} onChange={(event) => setOwner({ ...owner, newPassphrase: event.target.value })} /></Field><Field label="Confirm new passphrase"><input type="password" minLength="8" required autoComplete="new-password" value={owner.confirm} onChange={(event) => setOwner({ ...owner, confirm: event.target.value })} /></Field><p className="muted">Your passphrase derives a key that wraps a fresh random book-encryption key. Neither passphrase nor plaintext key is stored.</p><Button kind="danger" type="submit">Accept ownership</Button></form>}
      </>}
      {!isBookOwner && me.user.email.toLowerCase() !== book.pending_owner_transfer.new_owner_email.toLowerCase() && <p>You cannot operate this book while its ownership transfer is pending.</p>}
    </Panel>}
    {book && !book.pending_owner_transfer && !isBookOwner && <Panel title="Book access"><p>You can launch assigned workflows, but only the current book owner can administer this book.</p></Panel>}
    {book && !book.pending_owner_transfer && isBookOwner && <>
      <nav className="tabs" aria-label="Book administration">
        {[['books', 'Book'], ['setup', 'Setup'], ['ledger', 'Ledger'], ['workflows', 'Workflows & roles'], ['audit', 'Audit'], ['ownership', 'Ownership']].map(([id, label]) => <button key={id} className={`tab ${tab === id ? "active" : ""}`} onClick={() => setTab(id)}>{label}</button>)}
      </nav>
      {loading && <p className="muted">Refreshing administration data…</p>}

      {tab === "books" && <Panel title={`${book.name} · ${book.is_open ? "Open" : "Closed"}`}>
        <p className="muted">Book ID <code>{book.book_id}</code><br />Entity ID <code>{book.entity_id}</code></p>
        {!book.is_open && <form onSubmit={openBook}><Field label="Book passphrase"><input type="password" required autoComplete="new-password" value={openPassphrase} onChange={(event) => setOpenPassphrase(event.target.value)} /></Field><Button type="submit">Open book</Button></form>}
        {book.is_open && <div className="actions"><Button kind="secondary" onClick={closeBook}>Close book</Button></div>}
        <hr />
        <form onSubmit={async (event) => { event.preventDefault(); await request(`/api/books/${book.book_id}/backup`, { method: "POST", body: JSON.stringify({ location: backupLocation }) }, (value) => `Encrypted backup written to ${value.location}.`); }}>
          <h3>Backup</h3><p className="muted">Copies the three portable encrypted book files. No passphrase is sent.</p><Field label="Server backup folder"><input required placeholder="/path/to/backup" value={backupLocation} onChange={(event) => setBackupLocation(event.target.value)} /></Field><Button type="submit">Create backup</Button>
        </form>
        {me.is_bootstrap_owner && <form onSubmit={async (event) => { event.preventDefault(); const value = await request("/api/books/restore", { method: "POST", body: JSON.stringify({ location: restoreLocation }) }, "Backup restored. Reopen it with its original passphrase."); if (value) { setRestoreLocation(""); await onChanged("change"); selectBook(value.book_id); } }}><h3>Restore</h3><p className="warning">The matching book must be closed. Restore replaces its encrypted files from the selected folder.</p><Field label="Server backup folder"><input required value={restoreLocation} onChange={(event) => setRestoreLocation(event.target.value)} /></Field><Button type="submit">Restore backup</Button></form>}
      </Panel>}

      {tab === "setup" && <>
        {!book.is_open && <Panel title="Setup"><p className="warning">Open the book before changing accounting setup.</p></Panel>}
        {book.is_open && <>
          <Panel title="Entity and resource types"><p>{data.entities.map((item) => item.name).join(", ") || "No entity loaded"}</p><div className="table-wrap"><table><thead><tr><th>Name</th><th>Kind</th><th>Code</th><th>Unit</th><th>Precision</th></tr></thead><tbody>{data.resourceTypes.map((item) => <tr key={item.resource_type_id}><td>{item.name}</td><td>{item.kind}</td><td>{item.code}</td><td>{item.unit_of_measure}</td><td>{item.precision}</td></tr>)}</tbody></table></div>
            <form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/resource-types`, { op_id: newId(), ...resource, precision: Number(resource.precision), metadata: {} }, "Resource type created."); if (value) setResource({ name: "", kind: "CURRENCY", code: "", unit_of_measure: "", precision: "2" }); }}><h3>Create resource type</h3><div className="grid"><Field label="Name"><input required value={resource.name} onChange={(event) => setResource({ ...resource, name: event.target.value })} /></Field><Field label="Kind"><select value={resource.kind} onChange={(event) => setResource({ ...resource, kind: event.target.value })}>{["CURRENCY", "INVENTORY", "COMMODITY", "DIGITAL_ASSET", "OTHER"].map((value) => <option key={value}>{value}</option>)}</select></Field><Field label="Code"><input required value={resource.code} onChange={(event) => setResource({ ...resource, code: event.target.value })} /></Field><Field label="Unit"><input required value={resource.unit_of_measure} onChange={(event) => setResource({ ...resource, unit_of_measure: event.target.value })} /></Field><Field label="Precision"><input type="number" min="0" max="18" required value={resource.precision} onChange={(event) => setResource({ ...resource, precision: event.target.value })} /></Field></div><Button type="submit">Create resource type</Button></form>
          </Panel>
          <Panel title="Charts"><div className="table-wrap"><table><thead><tr><th>Name</th><th>Description</th><th>Status</th></tr></thead><tbody>{data.charts.map((item) => <tr key={item.chart_id}><td>{item.name}</td><td>{item.description}</td><td>{item.is_active ? "Active" : "Inactive"}</td></tr>)}</tbody></table></div>
            <div className="grid"><form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/charts`, { op_id: newId(), entity_id: book.entity_id, name: chart.name, description: chart.description || null, activate: chart.activate, starter_template: chart.starter_template, resource_type_id: chart.starter_template === "CORPORATE" ? chart.resource_type_id : null }, chart.starter_template === "CORPORATE" ? "Corporate starter chart created." : "Empty chart created."); if (value) setChart({ ...chart, name: "", description: "", activate: false }); }}><h3>Create chart</h3><Field label="Name"><input required value={chart.name} onChange={(event) => setChart({ ...chart, name: event.target.value })} /></Field><Field label="Description"><input value={chart.description} onChange={(event) => setChart({ ...chart, description: event.target.value })} /></Field><Field label="Chart setup"><select value={chart.starter_template} onChange={(event) => setChart({ ...chart, starter_template: event.target.value })}><option value="CORPORATE">Corporate starter</option><option value="EMPTY">Empty chart</option></select></Field>{chart.starter_template === "CORPORATE" && <><Field label="Starter currency"><select required value={chart.resource_type_id} onChange={(event) => setChart({ ...chart, resource_type_id: event.target.value })}><option value="">Choose a currency…</option>{data.resourceTypes.filter((item) => item.kind === "CURRENCY").map((item) => <option key={item.resource_type_id} value={item.resource_type_id}>{item.code} — {item.name}</option>)}</select></Field><p className="muted">Creates Assets, Liabilities, Equity, Revenue, and Expenses with the approved starter hierarchy. Add specialized accounts when needed.</p></>}<label><input type="checkbox" checked={chart.activate} onChange={(event) => setChart({ ...chart, activate: event.target.checked })} /> Make active</label><br /><Button type="submit">Create chart</Button></form>
            <form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/charts/${copyChart.source}/copy`, { op_id: newId(), name: copyChart.name, description: copyChart.description || null, activate: copyChart.activate }, "Chart copied."); if (value) setCopyChart({ ...copyChart, name: "", description: "" }); }}><h3>Copy chart</h3><Field label="Source"><select required value={copyChart.source} onChange={(event) => setCopyChart({ ...copyChart, source: event.target.value })}>{data.charts.map((item) => <option key={item.chart_id} value={item.chart_id}>{item.name}</option>)}</select></Field><Field label="New name"><input required value={copyChart.name} onChange={(event) => setCopyChart({ ...copyChart, name: event.target.value })} /></Field><Field label="Description"><input value={copyChart.description} onChange={(event) => setCopyChart({ ...copyChart, description: event.target.value })} /></Field><label><input type="checkbox" checked={copyChart.activate} onChange={(event) => setCopyChart({ ...copyChart, activate: event.target.checked })} /> Make active</label><br /><Button type="submit">Copy chart</Button></form></div>
          </Panel>
          <Panel title="Accounts"><div className="table-wrap"><table><thead><tr><th>Code</th><th>Account hierarchy</th><th>Type</th><th>Resource</th><th>Balance</th><th>Status</th><th></th></tr></thead><tbody>{accountTree.map((item) => <tr key={item.account_id}><td>{item.code}</td><td><span style={{ paddingLeft: `${item.depth * 1.25}rem` }}>{item.depth > 0 ? "↳ " : ""}{item.name}</span></td><td>{item.account_type}</td><td>{data.resourceTypes.find((type) => type.resource_type_id === item.resource_type_id)?.code}</td><td>{data.balances[item.account_id]?.natural ?? "0"}</td><td>{item.is_active ? "Active" : "Inactive"}</td><td><Button kind="secondary" onClick={() => changed(`/api/books/${book.book_id}/accounts/${item.account_id}/active`, { op_id: newId(), is_active: !item.is_active }, `Account ${item.is_active ? "deactivated" : "reactivated"}.`, "PUT")}>{item.is_active ? "Deactivate" : "Reactivate"}</Button></td></tr>)}</tbody></table></div>
            <div className="grid"><form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/accounts`, { op_id: newId(), chart_id: account.chart_id, name: account.name, code: account.code || null, account_type: account.account_type, resource_type_id: account.resource_type_id, parent_account_id: account.parent_account_id || null, validation_rules: {}, metadata: {} }, "Account created."); if (value) setAccount({ ...account, name: "", code: "", parent_account_id: "" }); }}><h3>Create account</h3><Field label="Chart"><select required value={account.chart_id} onChange={(event) => setAccount({ ...account, chart_id: event.target.value })}>{data.charts.map((item) => <option key={item.chart_id} value={item.chart_id}>{item.name}</option>)}</select></Field><Field label="Name"><input required value={account.name} onChange={(event) => setAccount({ ...account, name: event.target.value })} /></Field><Field label="Code"><input value={account.code} onChange={(event) => setAccount({ ...account, code: event.target.value })} /></Field><Field label="Type"><select value={account.account_type} onChange={(event) => setAccount({ ...account, account_type: event.target.value })}>{["ASSET", "LIABILITY", "EQUITY", "REVENUE", "EXPENSE"].map((value) => <option key={value}>{value}</option>)}</select></Field><Field label="Resource"><select required value={account.resource_type_id} onChange={(event) => setAccount({ ...account, resource_type_id: event.target.value })}>{data.resourceTypes.map((item) => <option key={item.resource_type_id} value={item.resource_type_id}>{item.code} — {item.name}</option>)}</select></Field><Field label="Parent account (optional)"><select value={account.parent_account_id} onChange={(event) => setAccount({ ...account, parent_account_id: event.target.value })}><option value="">None</option>{data.accounts.filter((item) => item.chart_id === account.chart_id).map((item) => <option key={item.account_id} value={item.account_id}>{item.name}</option>)}</select></Field><Button type="submit">Create account</Button></form>
            <form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/accounts/${accountEdit.account_id}`, { op_id: newId(), name: accountEdit.name || null, code: accountEdit.code || null, metadata: null }, "Account metadata updated.", "PATCH"); if (value) setAccountEdit({ account_id: "", name: "", code: "" }); }}><h3>Update account</h3><Field label="Account"><select required value={accountEdit.account_id} onChange={(event) => { const selected = data.accounts.find((item) => item.account_id === event.target.value); setAccountEdit({ account_id: event.target.value, name: selected?.name || "", code: selected?.code || "" }); }}><option value="">Choose…</option>{data.accounts.map((item) => <option key={item.account_id} value={item.account_id}>{item.name}</option>)}</select></Field><Field label="Name"><input required value={accountEdit.name} onChange={(event) => setAccountEdit({ ...accountEdit, name: event.target.value })} /></Field><Field label="Code"><input value={accountEdit.code} onChange={(event) => setAccountEdit({ ...accountEdit, code: event.target.value })} /></Field><Button type="submit">Update account</Button></form></div>
          </Panel>
          <Panel title="Periods and prices"><div className="grid"><div><div className="table-wrap"><table><thead><tr><th>Period</th><th>Dates</th><th>Status</th><th></th></tr></thead><tbody>{data.periods.map((item) => <tr key={item.period_id}><td>{item.name}</td><td>{item.start_date}–{item.end_date}</td><td>{item.status}</td><td><Button kind="secondary" onClick={() => changed(`/api/books/${book.book_id}/periods/${item.period_id}/${item.status === "OPEN" ? "close" : "reopen"}`, { op_id: newId() }, `Period ${item.status === "OPEN" ? "closed" : "reopened"}.`)}>{item.status === "OPEN" ? "Close" : "Reopen"}</Button></td></tr>)}</tbody></table></div><form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/periods`, { op_id: newId(), entity_id: book.entity_id, ...period }, "Period created."); if (value) setPeriod({ name: "", start_date: today(), end_date: today() }); }}><h3>Create period</h3><Field label="Name"><input required value={period.name} onChange={(event) => setPeriod({ ...period, name: event.target.value })} /></Field><Field label="Start"><input type="date" required value={period.start_date} onChange={(event) => setPeriod({ ...period, start_date: event.target.value })} /></Field><Field label="End"><input type="date" required value={period.end_date} onChange={(event) => setPeriod({ ...period, end_date: event.target.value })} /></Field><Button type="submit">Create period</Button></form></div>
            <div><div className="table-wrap"><table><thead><tr><th>Base/quote</th><th>Rate</th><th>As of</th></tr></thead><tbody>{data.prices.map((item, index) => <tr key={`${item.base_resource_type_id}-${item.quote_resource_type_id}-${item.as_of}-${index}`}><td>{data.resourceTypes.find((type) => type.resource_type_id === item.base_resource_type_id)?.code}/{data.resourceTypes.find((type) => type.resource_type_id === item.quote_resource_type_id)?.code}</td><td>{item.rate}</td><td>{item.as_of}</td></tr>)}</tbody></table></div><form onSubmit={async (event) => { event.preventDefault(); await changed(`/api/books/${book.book_id}/prices`, { op_id: newId(), ...price }, "Price recorded."); }}><h3>Record price</h3><Field label="Base"><select required value={price.base_resource_type_id} onChange={(event) => setPrice({ ...price, base_resource_type_id: event.target.value })}>{data.resourceTypes.map((item) => <option key={item.resource_type_id} value={item.resource_type_id}>{item.code}</option>)}</select></Field><Field label="Quote"><select required value={price.quote_resource_type_id} onChange={(event) => setPrice({ ...price, quote_resource_type_id: event.target.value })}>{data.resourceTypes.map((item) => <option key={item.resource_type_id} value={item.resource_type_id}>{item.code}</option>)}</select></Field><Field label="Rate"><input required inputMode="decimal" value={price.rate} onChange={(event) => setPrice({ ...price, rate: event.target.value })} /></Field><Field label="As of"><input type="date" required value={price.as_of} onChange={(event) => setPrice({ ...price, as_of: event.target.value })} /></Field><Button type="submit">Record price</Button></form></div></div>
          </Panel>
        </>}
      </>}

      {tab === "ledger" && <Panel title="Journal entries and balances">
        {!book.is_open ? <p className="warning">Open the book before using the ledger.</p> : <>
          <form onSubmit={postEntry}><h3>Post balanced entry</h3><div className="grid"><Field label="Entry date"><input type="date" required value={entry.entry_date} onChange={(event) => setEntry({ ...entry, entry_date: event.target.value })} /></Field><Field label="Description"><input required value={entry.description} onChange={(event) => setEntry({ ...entry, description: event.target.value })} /></Field><Field label="Amount"><input required inputMode="decimal" value={entry.amount} onChange={(event) => setEntry({ ...entry, amount: event.target.value })} /></Field><Field label="Debit account"><select required value={entry.debit_account} onChange={(event) => setEntry({ ...entry, debit_account: event.target.value })}><option value="">Choose…</option>{data.accounts.filter((item) => item.is_active).map((item) => <option key={item.account_id} value={item.account_id}>{item.name}</option>)}</select></Field><Field label="Credit account"><select required value={entry.credit_account} onChange={(event) => setEntry({ ...entry, credit_account: event.target.value })}><option value="">Choose…</option>{data.accounts.filter((item) => item.is_active).map((item) => <option key={item.account_id} value={item.account_id}>{item.name}</option>)}</select></Field><Field label="Memo"><input value={entry.memo} onChange={(event) => setEntry({ ...entry, memo: event.target.value })} /></Field></div>{entryError && <p className="error" role="alert">{entryError}</p>}<Button type="submit" disabled={entryPosting}>{entryPosting ? "Posting…" : "Post entry"}</Button></form>
          <form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/entries/reverse`, { new_entry_id: newId(), original_entry_id: reverse.original_entry_id, entry_date: reverse.entry_date, description: reverse.description || null, metadata: {} }, "Reversal posted."); if (value) setReverse({ original_entry_id: "", entry_date: today(), description: "" }); }}><h3>Reverse entry</h3><div className="grid"><Field label="Original entry"><select required value={reverse.original_entry_id} onChange={(event) => setReverse({ ...reverse, original_entry_id: event.target.value })}><option value="">Choose…</option>{data.entries.map((item) => <option key={item.entry_id} value={item.entry_id}>{item.entry_date} — {item.description}</option>)}</select></Field><Field label="Reversal date"><input type="date" required value={reverse.entry_date} onChange={(event) => setReverse({ ...reverse, entry_date: event.target.value })} /></Field><Field label="Description"><input value={reverse.description} onChange={(event) => setReverse({ ...reverse, description: event.target.value })} /></Field></div><Button type="submit">Post reversal</Button></form>
          <div className="table-wrap"><table><thead><tr><th>Date</th><th>Description</th><th>Source</th><th>Lines</th><th>ID</th></tr></thead><tbody>{data.entries.map((item) => <tr key={item.entry_id}><td>{item.entry_date}</td><td>{item.description}</td><td>{item.source}</td><td>{item.lines.map((line) => `${accountName(line.account_id)}: ${line.debit_amount ? `Dr ${line.debit_amount}` : `Cr ${line.credit_amount}`}`).join("; ")}</td><td><code>{item.entry_id}</code></td></tr>)}</tbody></table></div>
          <h3>Account balances</h3><div className="table-wrap"><table><thead><tr><th>Account</th><th>Debits</th><th>Credits</th><th>Natural balance</th></tr></thead><tbody>{data.accounts.map((item) => <tr key={item.account_id}><td>{item.name}</td><td>{data.balances[item.account_id]?.debit_total ?? "0"}</td><td>{data.balances[item.account_id]?.credit_total ?? "0"}</td><td>{data.balances[item.account_id]?.natural ?? "0"}</td></tr>)}</tbody></table></div>
        </>}
      </Panel>}

      {tab === "workflows" && <Panel title="Workflow and role administration">
        {!book.is_open ? <p className="warning">Open the book before administering workflows.</p> : <>
          <p className="muted">Lifecycle: generated artifact → deployed workflow → assigned role → displayed in that user's My workflows.</p>
          <h3>Deployed workflows</h3><div className="table-wrap"><table><thead><tr><th>Name</th><th>Artifact</th><th>Availability</th><th>Workflow ID</th></tr></thead><tbody>{data.workflows.map((item) => <tr key={item.workflow_deployment_id}><td>{item.workflow_name}<br /><span className="muted">{item.description}</span></td><td><code>{item.workflow_deployment_id}</code></td><td>{item.artifact_available ? "Available" : "Missing or modified"}</td><td><code>{item.workflow_id}</code></td></tr>)}</tbody></table></div>
          <div className="grid"><form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/workflow-artifacts`, workflowArtifact, "Workflow artifact generated."); if (value) setWorkflowArtifact({ workflow_name: "", description: "", kind: "journal" }); }}><h3>Generate standalone workflow SPA</h3><Field label="Workflow name"><input required value={workflowArtifact.workflow_name} onChange={(event) => setWorkflowArtifact({ ...workflowArtifact, workflow_name: event.target.value })} /></Field><Field label="Description"><textarea value={workflowArtifact.description} onChange={(event) => setWorkflowArtifact({ ...workflowArtifact, description: event.target.value })} /></Field><Field label="Workflow kind"><select value={workflowArtifact.kind} onChange={(event) => setWorkflowArtifact({ ...workflowArtifact, kind: event.target.value })}><option value="journal">Two-account journal entry</option><option value="opening_balance_import">Opening-balance import</option></select></Field><p className="muted">Creates an inspectable workflow artifact. Deploy it and assign its role before use. Import users also need the account-list role.</p><Button type="submit">Generate artifact</Button></form>
          <div><h3>Generated artifacts</h3>{data.artifacts.map((item) => { const deployed = data.workflows.some((workflow) => workflow.workflow_deployment_id === item.workflow_deployment_id); return <article className="workflow-card" key={item.workflow_deployment_id}><strong>{item.workflow_name}</strong><p className="muted">{item.description || "No description"}</p><code>{item.workflow_deployment_id}</code><JsonDetails value={item} />{deployed ? <p className="status good">Deployed</p> : <Button onClick={() => changed(`/api/books/${book.book_id}/workflows/deploy`, { workflow_deployment_id: item.workflow_deployment_id, workflow_id: item.workflow_id, entity_id: book.entity_id, workflow_name: item.workflow_name, description: item.description, backend_api_calls: item.backend_api_calls, required_inputs: item.required_inputs, metadata: item.metadata || {} }, "Workflow deployed. Its auto-role is not assigned to anyone yet.")}>Deploy</Button>}</article>; })}</div></div>
          <h3>Roles</h3><div className="table-wrap"><table><thead><tr><th>Role</th><th>Workflows</th><th>API permissions</th><th>Assigned users</th></tr></thead><tbody>{data.roles.map((item) => <tr key={item.role_id}><td>{item.name}<br /><span className="muted">{item.description}</span></td><td>{item.workflow_ids.map((id) => data.workflows.find((workflow) => workflow.workflow_id === id)?.workflow_name || id).join(", ") || "None"}</td><td>{item.permissions?.join(", ") || "None"}</td><td>{item.assigned_user_ids.map(userName).join(", ") || "None"}</td></tr>)}</tbody></table></div>
          <div className="grid"><form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/roles`, { op_id: newId(), entity_id: book.entity_id, name: role.name, description: role.description || null, permissions: role.permissions }, "Role created."); if (value) setRole({ name: "", description: "", permissions: [] }); }}><h3>Create role</h3><Field label="Name"><input required value={role.name} onChange={(event) => setRole({ ...role, name: event.target.value })} /></Field><Field label="Description"><input value={role.description} onChange={(event) => setRole({ ...role, description: event.target.value })} /></Field><label className="field"><span><input type="checkbox" checked={role.permissions.includes("list_accounts")} onChange={(event) => setRole({ ...role, permissions: event.target.checked ? ["list_accounts"] : [] })} /> Allow listing accounts</span></label><Button type="submit">Create role</Button></form>
          <form onSubmit={async (event) => { event.preventDefault(); await changed(`/api/books/${book.book_id}/roles/${roleWorkflow.role_id}/workflows`, { op_id: newId(), workflow_id: roleWorkflow.workflow_id }, "Workflow attached to role."); }}><h3>Attach workflow to role</h3><Field label="Role"><select required value={roleWorkflow.role_id} onChange={(event) => setRoleWorkflow({ ...roleWorkflow, role_id: event.target.value })}><option value="">Choose…</option>{data.roles.map((item) => <option key={item.role_id} value={item.role_id}>{item.name}</option>)}</select></Field><Field label="Workflow"><select required value={roleWorkflow.workflow_id} onChange={(event) => setRoleWorkflow({ ...roleWorkflow, workflow_id: event.target.value })}><option value="">Choose…</option>{data.workflows.map((item) => <option key={item.workflow_id} value={item.workflow_id}>{item.workflow_name}</option>)}</select></Field><Button type="submit">Attach workflow</Button></form>
          <form onSubmit={async (event) => { event.preventDefault(); const value = await changed(`/api/books/${book.book_id}/roles/${roleUser.role_id}/users`, { op_id: newId(), user_email: roleUser.user_email }, "Role assigned. The user's My workflows will update on focus or refresh."); if (value) setRoleUser({ ...roleUser, user_email: "" }); }}><h3>Assign role to user</h3><Field label="Role"><select required value={roleUser.role_id} onChange={(event) => setRoleUser({ ...roleUser, role_id: event.target.value })}><option value="">Choose…</option>{data.roles.map((item) => <option key={item.role_id} value={item.role_id}>{item.name}</option>)}</select></Field><Field label="Verified user email"><input type="email" required value={roleUser.user_email} onChange={(event) => setRoleUser({ ...roleUser, user_email: event.target.value })} /></Field><div className="actions"><Button type="submit">Assign to user</Button><Button kind="secondary" type="button" disabled={!roleUser.role_id || data.roles.find((item) => item.role_id === roleUser.role_id)?.assigned_user_ids.includes(me?.user?.user_id)} onClick={() => changed(`/api/books/${book.book_id}/roles/${roleUser.role_id}/users`, { op_id: newId(), assign_to_self: true }, "Role assigned to you. My workflows will update now.")}>Assign to me</Button></div></form></div>
        </>}
      </Panel>}

      {tab === "audit" && <Panel title="Immutable audit sequence">{!book.is_open ? <p className="warning">Open the book to inspect its audit log.</p> : <div className="table-wrap"><table><thead><tr><th>#</th><th>Type</th><th>Time</th><th>Actor</th><th>Outcome</th></tr></thead><tbody>{data.audit.map((item, index) => <tr key={item.event_id}><td>{index + 1}</td><td>{item.event_type}</td><td>{new Date(item.occurred_at).toLocaleString()}</td><td><code>{item.actor_user_id}</code></td><td><JsonDetails label="Event" value={item} /></td></tr>)}</tbody></table></div>}</Panel>}

      {tab === "ownership" && <Panel title="Transfer ownership" className="danger-zone"><p className="warning">Step 1 nominates the successor and freezes the open book. The successor must sign in separately and choose their own new passphrase in step 2.</p><form onSubmit={async (event) => { event.preventDefault(); if (!window.confirm(`Freeze ${book.name} and nominate ${owner.email} as its successor?`)) return; const value = await request(`/api/books/${book.book_id}/ownership-transfer/initiate`, { method: "POST", body: JSON.stringify({ transfer_id: newId(), new_owner_email: owner.email, current_passphrase: owner.currentPassphrase }) }, `Transfer initiated for ${owner.email}. The book is now frozen.`); if (value) { setOwner({ email: "", currentPassphrase: "", newPassphrase: "", confirm: "", cancelPassphrase: "" }); await onChanged("change"); } }}><Field label="Successor email"><input type="email" required autoComplete="off" value={owner.email} onChange={(event) => setOwner({ ...owner, email: event.target.value })} /></Field><Field label="Current book passphrase"><input type="password" minLength="8" required autoComplete="new-password" value={owner.currentPassphrase} onChange={(event) => setOwner({ ...owner, currentPassphrase: event.target.value })} /></Field><p className="muted">The current passphrase is checked once and never retained or prefilled.</p><Button kind="danger" type="submit">Confirm and freeze book</Button></form></Panel>}
    </>}
  </>;
}
