import { useCallback, useEffect, useMemo, useState } from "react";
import { api, errorText } from "./api.js";
import OwnerWorkspace from "./OwnerWorkspace.jsx";
import EntitySetup from "./EntitySetup.jsx";

function Button({ children, kind = "primary", ...props }) {
  return <button className={`button ${kind === "primary" ? "" : kind}`} {...props}>{children}</button>;
}

function Login({ authConfig, message, setMessage, onLogin }) {
  const [devEmail, setDevEmail] = useState("");
  async function devLogin(event) {
    event.preventDefault();
    const result = await api("/api/auth/dev-login", { method: "POST", body: JSON.stringify({ email: devEmail }) });
    if (result.ok) { setMessage(""); onLogin(result.body); }
    else setMessage(errorText(result));
  }
  return <main className="login">
    <h1>First Principles Accounting</h1>
    <p>Sign in to open your accounting workspace.</p>
    {(authConfig.providers || []).map((provider) => <Button key={provider.id} onClick={() => { window.location.href = `/api/auth/${provider.id}/login`; }}>Sign in with {provider.display_name}</Button>)}
    {authConfig.dev_login_enabled && <form onSubmit={devLogin}>
      <p className="warning">Development login is enabled. Do not use this mode on a shared deployment.</p>
      <label className="field">Email<input type="email" required value={devEmail} onChange={(event) => setDevEmail(event.target.value)} /></label>
      <Button type="submit">Development sign in</Button>
    </form>}
    {(authConfig.providers || []).length === 0 && !authConfig.dev_login_enabled && <p className="error">No login method is configured.</p>}
    {message && <p className="error">{message}</p>}
  </main>;
}

function MyWorkflows({ book, workflows, loading }) {
  const available = (workflows || []).filter((workflow) => workflow.artifact_available !== false);
  const unavailable = (workflows || []).filter((workflow) => workflow.artifact_available === false);
  return <section className="panel">
    <h2>My workflows</h2>
    <p className="muted">Only workflows granted to your assigned roles appear here. Each opens as an independent application.</p>
    {!book && <p>Select a book to see your workflows.</p>}
    {loading && <p className="muted">Refreshing workflows…</p>}
    {book && workflows && available.length === 0 && <p className="muted">No available workflows are assigned to you in this book.</p>}
    <div className="workflow-list">
      {available.map((workflow) => <article className="workflow-card" key={workflow.workflow_deployment_id}>
        <h3>{workflow.workflow_name}</h3>
        <p className="muted">{workflow.description || "No description"}</p>
        <a className="button" href={`${workflow.frontend_route}?book_id=${book.book_id}&entity_id=${book.entity_id}`}>Launch workflow</a>
      </article>)}
    </div>
    {unavailable.length > 0 && <div className="warning">{unavailable.length} assigned workflow artifact{unavailable.length === 1 ? " is" : "s are"} missing or modified and cannot be launched.</div>}
  </section>;
}

export default function App() {
  const [authConfig, setAuthConfig] = useState(null);
  const [me, setMe] = useState(null);
  const [books, setBooks] = useState(null);
  const [selectedBookId, setSelectedBookId] = useState("");
  const [workflows, setWorkflows] = useState(null);
  const [loadingWorkflows, setLoadingWorkflows] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [refreshEpoch, setRefreshEpoch] = useState(0);
  const selectedBook = useMemo(() => (books || []).find((book) => book.book_id === selectedBookId) || null, [books, selectedBookId]);
  const isBookOwner = Boolean(selectedBook && me && selectedBook.owner_email.toLowerCase() === me.user.email.toLowerCase());

  const loadBooks = useCallback(async () => {
    if (!me) return;
    const result = await api("/api/books/mine");
    if (result.ok) {
      setBooks(result.body);
      setSelectedBookId((current) => result.body.some((book) => book.book_id === current) ? current : "");
      setError("");
    } else setError(errorText(result));
  }, [me]);

  const loadWorkflows = useCallback(async () => {
    if (!selectedBook) { setWorkflows(null); return; }
    if (!selectedBook.is_open || selectedBook.pending_owner_transfer) { setWorkflows([]); return; }
    setLoadingWorkflows(true);
    const result = await api(`/api/books/${selectedBook.book_id}/workflows/mine?entity_id=${selectedBook.entity_id}`);
    setLoadingWorkflows(false);
    if (result.ok) { setWorkflows(result.body); setError(""); }
    else setError(errorText(result));
  }, [selectedBook]);

  const refreshAll = useCallback(async (reason = "manual") => {
    await loadBooks();
    setRefreshEpoch((value) => value + 1);
    if (reason === "manual") setMessage("Workspace refreshed.");
  }, [loadBooks]);

  useEffect(() => {
    Promise.all([api("/api/auth/config"), api("/api/auth/me")]).then(([config, identity]) => {
      setAuthConfig(config.ok ? config.body : {});
      setMe(identity.ok ? identity.body : null);
    });
    if (new URLSearchParams(window.location.search).get("login_error")) setMessage("Google login was cancelled or denied.");
  }, []);
  useEffect(() => { loadBooks(); }, [loadBooks]);
  useEffect(() => { loadWorkflows(); }, [loadWorkflows, refreshEpoch]);
  useEffect(() => {
    function onFocus() { if (me) refreshAll("focus"); }
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [me, refreshAll]);

  async function rotateSession() {
    const result = await api("/api/auth/refresh", { method: "POST" });
    if (result.ok) setMessage("Session token rotated."); else setError(errorText(result));
  }
  async function logout() {
    await api("/api/auth/logout", { method: "POST" });
    setMe(null); setBooks(null); setSelectedBookId(""); setWorkflows(null); setMessage(""); setError("");
  }

  if (authConfig === null) return <main className="login"><p>Loading FPA…</p></main>;
  if (!me) return <Login authConfig={authConfig} message={message} setMessage={setMessage} onLogin={setMe} />;

  return <div className="shell">
    <header className="topbar">
      <div className="brand"><h1>First Principles Accounting</h1><p>{me.user.display_name} · {me.user.email}</p></div>
      <div className="top-actions">
        <Button kind="secondary" onClick={() => refreshAll("manual")}>Refresh</Button>
        <Button kind="secondary" onClick={rotateSession}>Rotate session</Button>
        <Button kind="secondary" onClick={logout}>Sign out</Button>
      </div>
    </header>
    <div className="layout">
      <aside className="sidebar">
        <h2>Books</h2>
        {books === null && <p className="muted">Loading books…</p>}
        {books && books.length === 0 && <p className="muted">No books are available yet.</p>}
        <div className="book-list">{(books || []).map((book) => <button className={`book ${book.book_id === selectedBookId ? "active" : ""}`} key={book.book_id} onClick={() => setSelectedBookId(book.book_id)}>
          <strong>{book.name}</strong><span className={`status ${book.is_open && !book.pending_owner_transfer ? "good" : ""}`}>{book.pending_owner_transfer ? "transferring" : book.is_open ? "open" : "closed"}</span><div className="muted">{book.owner_email}</div>
        </button>)}</div>
        <p className="muted">User ID<br /><code>{me.user.user_id}</code></p>
        <p className="muted">Bootstrap owner: {me.is_bootstrap_owner ? "yes" : "no"}</p>
      </aside>
      <main className="content">
        {message && <div className="message">{message}</div>}
        {error && <div className="error">{error}</div>}
        <MyWorkflows book={selectedBook} workflows={workflows} loading={loadingWorkflows} />
        <EntitySetup book={selectedBook} refreshEpoch={refreshEpoch} setMessage={setMessage} setError={setError} />
        <OwnerWorkspace me={me} books={books || []} book={selectedBook} isBookOwner={isBookOwner} refreshEpoch={refreshEpoch} onChanged={refreshAll} setMessage={setMessage} setError={setError} selectBook={setSelectedBookId} />
      </main>
    </div>
  </div>;
}
