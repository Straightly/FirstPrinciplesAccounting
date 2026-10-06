import { useEffect, useState } from "react";
import { api, errorText, newId, today } from "./api.js";

const empty = { entities: [], relationships: [], charts: [], resources: [], accounts: [] };

export default function EntitySetup({ book, refreshEpoch, setMessage, setError }) {
  const [data, setData] = useState(empty);
  const [access, setAccess] = useState({});
  const [entity, setEntity] = useState({ name: "", category: "PROPERTY" });
  const [relationship, setRelationship] = useState({ from_entity_id: "", to_entity_id: "", kind: "OWNS", effective_from: today(), effective_to: "" });
  const [account, setAccount] = useState({ chart_id: "", name: "", code: "", account_type: "ASSET", resource_type_id: "", parent_account_id: "", associated_entity_id: "" });

  async function load() {
    const base = `/api/books/${book.book_id}`;
    const paths = {
      entities: `${base}/entities`, relationships: `${base}/entity-relationships`,
      charts: `${base}/charts?entity_id=${book.entity_id}`, resources: `${base}/resource-types`,
    };
    const results = await Promise.all(Object.entries(paths).map(async ([key, path]) => [key, await api(path)]));
    const next = { ...empty };
    const rights = {};
    for (const [key, result] of results) {
      rights[key] = result.ok;
      if (result.ok) next[key] = result.body;
    }
    const accountResults = await Promise.all(next.charts.map((chart) => api(`${base}/accounts?chart_id=${chart.chart_id}`)));
    rights.accounts = accountResults.length > 0 && accountResults.every((result) => result.ok);
    if (rights.accounts) next.accounts = accountResults.flatMap((result) => result.body);
    setAccess(rights);
    setData(next);
  }

  useEffect(() => {
    if (book?.is_open && !book.pending_owner_transfer) load();
    else { setData(empty); setAccess({}); }
  }, [book?.book_id, book?.is_open, book?.pending_owner_transfer, refreshEpoch]);

  async function create(path, body, success) {
    const result = await api(`/api/books/${book.book_id}/${path}`, { method: "POST", body: JSON.stringify({ op_id: newId(), ...body }) });
    if (!result.ok) { setError(errorText(result)); return false; }
    setError("");
    setMessage(success);
    await load();
    return true;
  }

  if (!book?.is_open || book.pending_owner_transfer) return null;
  const subject = data.entities.find((item) => item.is_subject);
  const entityName = (id) => data.entities.find((item) => item.entity_id === id)?.name || id;
  const chartId = account.chart_id || data.charts.find((item) => item.is_active)?.chart_id || data.charts[0]?.chart_id || "";
  const resourceId = account.resource_type_id || data.resources.find((item) => item.kind === "CURRENCY")?.resource_type_id || data.resources[0]?.resource_type_id || "";
  const fromId = relationship.from_entity_id || (relationship.kind === "OWNS" ? subject?.entity_id : "") || "";
  return <section className="panel">
    <h2>Entities and linked accounts</h2>
    <p className="muted">This book has one accounting subject. Properties and units have their own IDs and relationships, but share its chart and periods. Each action requires its own assigned permission.</p>
    {!access.entities ? <p className="warning">Listing entities requires the list_entities permission.</p> : <>
      <div className="table-wrap"><table><thead><tr><th>Entity</th><th>Category</th><th>Role</th><th>ID</th></tr></thead><tbody>{data.entities.map((item) => <tr key={item.entity_id}><td>{item.name}</td><td>{item.category}</td><td>{item.is_subject ? "Book subject" : "Referenced"}</td><td><code>{item.entity_id}</code></td></tr>)}</tbody></table></div>
      <h3>Relationships</h3>
      <div className="table-wrap"><table><thead><tr><th>Relationship</th><th>From</th><th>To</th><th>Effective</th></tr></thead><tbody>{data.relationships.map((item) => <tr key={item.relationship_id}><td>{item.kind}</td><td>{entityName(item.from_entity_id)}</td><td>{entityName(item.to_entity_id)}</td><td>{item.effective_from}{item.effective_to ? ` to ${item.effective_to}` : " onward"}</td></tr>)}</tbody></table></div>
    </>}
    <div className="grid">
      <form onSubmit={async (event) => { event.preventDefault(); if (await create("entities", entity, "Entity created.")) setEntity({ ...entity, name: "" }); }}>
        <h3>Create referenced entity</h3><label className="field">Name<input required value={entity.name} onChange={(event) => setEntity({ ...entity, name: event.target.value })} /></label>
        <label className="field">Category<select value={entity.category} onChange={(event) => setEntity({ ...entity, category: event.target.value })}>{["PROPERTY", "UNIT", "ORGANIZATION", "PERSON", "OTHER"].map((value) => <option key={value}>{value}</option>)}</select></label>
        <button className="button" type="submit">Create entity</button>
      </form>
      <form onSubmit={async (event) => { event.preventDefault(); await create("entity-relationships", { ...relationship, from_entity_id: fromId, effective_to: relationship.effective_to || null }, "Relationship created."); }}>
        <h3>Link entities</h3><label className="field">Relationship<select value={relationship.kind} onChange={(event) => setRelationship({ ...relationship, kind: event.target.value, from_entity_id: "", to_entity_id: "" })}><option value="OWNS">Owns property</option><option value="CONTAINS">Contains unit</option></select></label>
        <label className="field">From<select required value={fromId} onChange={(event) => setRelationship({ ...relationship, from_entity_id: event.target.value })}><option value="">Choose…</option>{data.entities.filter((item) => relationship.kind === "OWNS" ? item.is_subject : item.category === "PROPERTY").map((item) => <option key={item.entity_id} value={item.entity_id}>{item.name}</option>)}</select></label>
        <label className="field">To<select required value={relationship.to_entity_id} onChange={(event) => setRelationship({ ...relationship, to_entity_id: event.target.value })}><option value="">Choose…</option>{data.entities.filter((item) => item.category === (relationship.kind === "OWNS" ? "PROPERTY" : "UNIT")).map((item) => <option key={item.entity_id} value={item.entity_id}>{item.name}</option>)}</select></label>
        <label className="field">Effective from<input type="date" required value={relationship.effective_from} onChange={(event) => setRelationship({ ...relationship, effective_from: event.target.value })} /></label>
        <label className="field">Effective through (optional)<input type="date" value={relationship.effective_to} onChange={(event) => setRelationship({ ...relationship, effective_to: event.target.value })} /></label>
        <button className="button" type="submit" disabled={!access.entities}>Create relationship</button>
      </form>
    </div>
    {(!access.charts || !access.resources || !access.accounts) && <p className="warning">To create a linked account in this screen, assign list_charts, list_resource_types, and list_accounts permissions. Creating it also requires create_account.</p>}
    {access.charts && access.resources && access.accounts && <form onSubmit={async (event) => { event.preventDefault(); const body = { ...account, chart_id: chartId, resource_type_id: resourceId, parent_account_id: account.parent_account_id || null, associated_entity_id: account.associated_entity_id || null, code: account.code || null, validation_rules: {}, metadata: {} }; if (await create("accounts", body, "Account created.")) setAccount({ ...account, name: "", code: "", parent_account_id: "" }); }}>
      <h3>Create account</h3><div className="grid">
        <label className="field">Chart<select required value={chartId} onChange={(event) => setAccount({ ...account, chart_id: event.target.value, parent_account_id: "" })}>{data.charts.map((item) => <option key={item.chart_id} value={item.chart_id}>{item.name}</option>)}</select></label>
        <label className="field">Name<input required value={account.name} onChange={(event) => setAccount({ ...account, name: event.target.value })} /></label>
        <label className="field">Code<input value={account.code} onChange={(event) => setAccount({ ...account, code: event.target.value })} /></label>
        <label className="field">Type<select value={account.account_type} onChange={(event) => setAccount({ ...account, account_type: event.target.value })}>{["ASSET", "LIABILITY", "EQUITY", "REVENUE", "EXPENSE"].map((value) => <option key={value}>{value}</option>)}</select></label>
        <label className="field">Resource<select required value={resourceId} onChange={(event) => setAccount({ ...account, resource_type_id: event.target.value })}>{data.resources.map((item) => <option key={item.resource_type_id} value={item.resource_type_id}>{item.code} · {item.name}</option>)}</select></label>
        <label className="field">Parent account<select value={account.parent_account_id} onChange={(event) => setAccount({ ...account, parent_account_id: event.target.value })}><option value="">None</option>{data.accounts.filter((item) => item.chart_id === chartId).map((item) => <option key={item.account_id} value={item.account_id}>{item.name}</option>)}</select></label>
        <label className="field">Associated entity<select value={account.associated_entity_id} onChange={(event) => setAccount({ ...account, associated_entity_id: event.target.value })}><option value="">Book-wide</option>{data.entities.filter((item) => !item.is_subject).map((item) => <option key={item.entity_id} value={item.entity_id}>{item.name} ({item.category})</option>)}</select></label>
      </div><button className="button" type="submit">Create account</button>
    </form>}
  </section>;
}
