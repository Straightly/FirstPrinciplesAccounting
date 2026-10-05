import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App.jsx";

const book = {
  book_id: "11111111-1111-4111-8111-111111111111",
  entity_id: "22222222-2222-4222-8222-222222222222",
  name: "Test Book",
  owner_email: "owner@example.com",
  is_open: true,
};
const workflow = {
  workflow_deployment_id: "33333333-3333-4333-8333-333333333333",
  workflow_id: "44444444-4444-4444-8444-444444444444",
  workflow_name: "Record receipt",
  frontend_route: "/workflows/333/code/index.html",
  artifact_available: true,
};

function response(body, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  }));
}

describe("FPA application shell", () => {
  beforeEach(() => {
    vi.stubGlobal("crypto", { randomUUID: vi.fn(() => "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa") });
    vi.stubGlobal("confirm", vi.fn(() => true));
  });
  afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

  it("discovers an assigned standalone workflow and refreshes on window focus", async () => {
    let workflowReads = 0;
    const fetchMock = vi.fn((path) => {
      if (path === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (path === "/api/auth/me") return response({ user: { user_id: "u1", email: "worker@example.com", display_name: "Worker" }, is_bootstrap_owner: false, allowed_actions: [] });
      if (path === "/api/books/mine") return response([book]);
      if (String(path).includes("/workflows/mine")) { workflowReads += 1; return response([workflow]); }
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    const link = await screen.findByRole("link", { name: "Launch workflow" });
    expect(link.getAttribute("href")).toContain("book_id=11111111");
    expect(link.getAttribute("href")).toContain("entity_id=22222222");
    const beforeFocus = workflowReads;
    fireEvent.focus(window);
    await waitFor(() => expect(workflowReads).toBeGreaterThan(beforeFocus));
  });

  it("keeps owner administration separate from role-scoped My workflows", async () => {
    const fetchMock = vi.fn((path) => {
      const value = String(path);
      if (value === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (value === "/api/auth/me") return response({ user: { user_id: "owner-id", email: "owner@example.com", display_name: "Owner" }, is_bootstrap_owner: true, allowed_actions: ["create_accounting_book"] });
      if (value === "/api/books/mine") return response([book]);
      if (value.includes("/workflows/mine")) return response([]);
      if (value.endsWith("/entities")) return response([{ entity_id: book.entity_id, name: "Test Book" }]);
      if (value.endsWith("/resource-types") || value.includes("/charts?") || value.includes("/periods?") || value.endsWith("/prices") || value.includes("/entries?") || value.endsWith("/audit-log") || value.includes("/roles?") || value.endsWith("/workflow-artifacts") || value.endsWith("/users")) return response([]);
      if (value.includes("/workflows?")) return response([workflow]);
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    expect(await screen.findByText("No available workflows are assigned to you in this book.")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Workflows & roles" }));
    expect((await screen.findAllByText("Record receipt")).length).toBeGreaterThan(0);
    expect(screen.queryByRole("link", { name: "Launch workflow" })).toBeNull();
  });

  it("lets the book owner assign a selected role to themselves", async () => {
    const roleId = "55555555-5555-4555-8555-555555555555";
    let assigned = false;
    const fetchMock = vi.fn((path, options = {}) => {
      const value = String(path);
      if (value === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (value === "/api/auth/me") return response({ user: { user_id: "owner-id", email: "owner@example.com", display_name: "Owner" }, is_bootstrap_owner: true, allowed_actions: [] });
      if (value === "/api/books/mine") return response([book]);
      if (value.includes("/workflows/mine")) return response(assigned ? [workflow] : []);
      if (value.endsWith(`/roles/${roleId}/users`) && options.method === "POST") {
        expect(JSON.parse(options.body).assign_to_self).toBe(true);
        assigned = true;
        return response({ id: roleId });
      }
      if (value.endsWith("/entities")) return response([{ entity_id: book.entity_id, name: "Test Book" }]);
      if (value.endsWith("/resource-types") || value.includes("/charts?") || value.includes("/periods?") || value.endsWith("/prices") || value.includes("/entries?") || value.endsWith("/audit-log") || value.endsWith("/workflow-artifacts") || value.endsWith("/users")) return response([]);
      if (value.includes("/roles?")) return response([{ role_id: roleId, name: "Importer", description: null, workflow_ids: [workflow.workflow_id], assigned_user_ids: assigned ? ["owner-id"] : [] }]);
      if (value.includes("/workflows?")) return response([workflow]);
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    await user.click(await screen.findByRole("button", { name: "Workflows & roles" }));
    const selfButton = await screen.findByRole("button", { name: "Assign to me" });
    await user.selectOptions(within(selfButton.closest("form")).getByRole("combobox", { name: "Role" }), roleId);
    await user.click(selfButton);
    await waitFor(() => expect(screen.getByRole("link", { name: "Launch workflow" })).toBeTruthy());
    expect(selfButton.disabled).toBe(true);
  });

  it("finishes a posted entry with cleared required fields and fresh client identifiers", async () => {
    let uuidSequence = 0;
    crypto.randomUUID.mockImplementation(() => `00000000-0000-4000-8000-${String(++uuidSequence).padStart(12, "0")}`);
    const chartId = "55555555-5555-4555-8555-555555555555";
    const cashId = "66666666-6666-4666-8666-666666666666";
    const revenueId = "77777777-7777-4777-8777-777777777777";
    const postedBodies = [];
    const accounts = [
      { account_id: cashId, chart_id: chartId, name: "Cash", code: "1000", account_type: "ASSET", resource_type_id: "usd", is_active: true },
      { account_id: revenueId, chart_id: chartId, name: "Revenue", code: "4000", account_type: "REVENUE", resource_type_id: "usd", is_active: true },
    ];
    const fetchMock = vi.fn((path, options = {}) => {
      const value = String(path);
      if (value === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (value === "/api/auth/me") return response({ user: { user_id: "owner-id", email: "owner@example.com", display_name: "Owner" }, is_bootstrap_owner: true, allowed_actions: [] });
      if (value === "/api/books/mine") return response([book]);
      if (value.includes("/workflows/mine")) return response([]);
      if (value.endsWith("/entities")) return response([{ entity_id: book.entity_id, name: "Test Book" }]);
      if (value.endsWith("/resource-types")) return response([{ resource_type_id: "usd", name: "US Dollar", code: "USD", kind: "CURRENCY", unit_of_measure: "USD", precision: 2 }]);
      if (value.includes("/charts?")) return response([{ chart_id: chartId, name: "Primary", is_active: true }]);
      if (value.includes("/accounts?")) return response(accounts);
      if (value.includes("/accounts/") && value.endsWith("/balance")) return response({ debit_total: "0", credit_total: "0", natural: "0" });
      if (value.endsWith("/entries") && options.method === "POST") {
        const body = JSON.parse(options.body);
        postedBodies.push(body);
        return response({ id: body.entry_id });
      }
      if (value.includes("/entries?") || value.includes("/periods?") || value.endsWith("/prices") || value.endsWith("/audit-log") || value.includes("/workflows?") || value.includes("/roles?") || value.endsWith("/workflow-artifacts") || value.endsWith("/users")) return response([]);
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    await user.click(await screen.findByRole("button", { name: "Ledger" }));
    const entryForm = within(screen.getByRole("heading", { name: "Post balanced entry" }).closest("form"));

    await user.type(entryForm.getByLabelText("Description"), "Initial funding");
    await user.type(entryForm.getByLabelText("Amount"), "125.00");
    await user.selectOptions(entryForm.getByLabelText("Debit account"), cashId);
    await user.selectOptions(entryForm.getByLabelText("Credit account"), revenueId);
    expect(screen.queryByText(/Idempotency key/)).toBeNull();
    await user.click(entryForm.getByRole("button", { name: "Post entry" }));

    await waitFor(() => expect(postedBodies).toHaveLength(1));
    await waitFor(() => expect(entryForm.getByLabelText("Description").value).toBe(""));
    expect(entryForm.getByLabelText("Amount").value).toBe("");
    expect(entryForm.getByLabelText("Debit account").value).toBe(cashId);
    expect(entryForm.getByLabelText("Credit account").value).toBe(revenueId);

    await user.type(entryForm.getByLabelText("Description"), "Second funding");
    await user.type(entryForm.getByLabelText("Amount"), "50.00");
    await user.click(entryForm.getByRole("button", { name: "Post entry" }));
    await waitFor(() => expect(postedBodies).toHaveLength(2));
    expect(postedBodies[1].entry_id).not.toBe(postedBodies[0].entry_id);
    expect(postedBodies[1].lines[0].line_id).not.toBe(postedBodies[0].lines[0].line_id);
    expect(postedBodies[1].lines[1].line_id).not.toBe(postedBodies[0].lines[1].line_id);
  });

  it("shows a rejected journal entry beside the form and restores its submit button", async () => {
    const chartId = "55555555-5555-4555-8555-555555555555";
    const cashId = "66666666-6666-4666-8666-666666666666";
    const expenseId = "77777777-7777-4777-8777-777777777777";
    const accounts = [
      { account_id: cashId, chart_id: chartId, name: "Cash", account_type: "ASSET", resource_type_id: "usd", is_active: true },
      { account_id: expenseId, chart_id: chartId, name: "Expense", account_type: "EXPENSE", resource_type_id: "usd", is_active: true },
    ];
    const fetchMock = vi.fn((path, options = {}) => {
      const value = String(path);
      if (value === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (value === "/api/auth/me") return response({ user: { user_id: "owner-id", email: "owner@example.com", display_name: "Owner" }, is_bootstrap_owner: true, allowed_actions: [] });
      if (value === "/api/books/mine") return response([book]);
      if (value.includes("/workflows/mine")) return response([]);
      if (value.endsWith("/entities")) return response([{ entity_id: book.entity_id, name: "Test Book" }]);
      if (value.endsWith("/resource-types")) return response([{ resource_type_id: "usd", name: "US Dollar", code: "USD", kind: "CURRENCY", unit_of_measure: "USD", precision: 2 }]);
      if (value.includes("/charts?")) return response([{ chart_id: chartId, name: "Primary", is_active: true }]);
      if (value.includes("/accounts?")) return response(accounts);
      if (value.includes("/accounts/") && value.endsWith("/balance")) return response({ debit_total: "0", credit_total: "0", natural: "0" });
      if (value.endsWith("/entries") && options.method === "POST") return Promise.resolve(new Response("Failed to deserialize journal entry field", { status: 422 }));
      if (value.includes("/entries?") || value.includes("/periods?") || value.endsWith("/prices") || value.endsWith("/audit-log") || value.includes("/workflows?") || value.includes("/roles?") || value.endsWith("/workflow-artifacts") || value.endsWith("/users")) return response([]);
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    await user.click(await screen.findByRole("button", { name: "Ledger" }));
    const entryForm = within(screen.getByRole("heading", { name: "Post balanced entry" }).closest("form"));
    await user.type(entryForm.getByLabelText("Description"), "Filing fee");
    await user.type(entryForm.getByLabelText("Amount"), "75.00");
    await user.selectOptions(entryForm.getByLabelText("Debit account"), expenseId);
    await user.selectOptions(entryForm.getByLabelText("Credit account"), cashId);
    await user.click(entryForm.getByRole("button", { name: "Post entry" }));

    expect((await entryForm.findByRole("alert")).textContent).toContain("Failed to deserialize journal entry field");
    expect(entryForm.getByRole("button", { name: "Post entry" }).disabled).toBe(false);
  });

  it("creates a corporate starter chart with an explicitly selected currency", async () => {
    const postedBodies = [];
    let created = false;
    const usdId = "88888888-8888-4888-8888-888888888888";
    const inventoryId = "99999999-9999-4999-8999-999999999999";
    const chartId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const assetsId = "10000000-0000-4000-8000-000000000001";
    const currentAssetsId = "10000000-0000-4000-8000-000000000002";
    const cashId = "10000000-0000-4000-8000-000000000003";
    const checkingId = "10000000-0000-4000-8000-000000000004";
    const starterAccounts = [
      { account_id: checkingId, chart_id: chartId, name: "Checking Account", code: null, account_type: "ASSET", resource_type_id: usdId, parent_account_id: cashId, is_active: true },
      { account_id: assetsId, chart_id: chartId, name: "Assets", code: null, account_type: "ASSET", resource_type_id: usdId, parent_account_id: null, is_active: true },
      { account_id: cashId, chart_id: chartId, name: "Cash", code: null, account_type: "ASSET", resource_type_id: usdId, parent_account_id: currentAssetsId, is_active: true },
      { account_id: currentAssetsId, chart_id: chartId, name: "Current Assets", code: null, account_type: "ASSET", resource_type_id: usdId, parent_account_id: assetsId, is_active: true },
    ];
    const fetchMock = vi.fn((path, options = {}) => {
      const value = String(path);
      if (value === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (value === "/api/auth/me") return response({ user: { user_id: "owner-id", email: "owner@example.com", display_name: "Owner" }, is_bootstrap_owner: true, allowed_actions: [] });
      if (value === "/api/books/mine") return response([book]);
      if (value.includes("/workflows/mine")) return response([]);
      if (value.endsWith("/entities")) return response([{ entity_id: book.entity_id, name: "Test Book" }]);
      if (value.endsWith("/resource-types")) return response([
        { resource_type_id: usdId, name: "US Dollar", code: "USD", kind: "CURRENCY", unit_of_measure: "USD", precision: 2 },
        { resource_type_id: inventoryId, name: "Widget", code: "WIDGET", kind: "INVENTORY", unit_of_measure: "each", precision: 0 },
      ]);
      if (value.endsWith("/charts") && options.method === "POST") {
        postedBodies.push(JSON.parse(options.body));
        created = true;
        return response({ id: chartId });
      }
      if (value.includes("/charts?")) return response(created ? [{ chart_id: chartId, name: "Primary chart", description: null, is_active: true }] : []);
      if (value.includes("/accounts?")) return response(created ? starterAccounts : []);
      if (value.includes("/accounts/") && value.endsWith("/balance")) return response({ debit_total: "0", credit_total: "0", natural: "0" });
      if (value.includes("/periods?") || value.endsWith("/prices") || value.includes("/entries?") || value.endsWith("/audit-log") || value.includes("/workflows?") || value.includes("/roles?") || value.endsWith("/workflow-artifacts") || value.endsWith("/users")) return response([]);
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    await user.click(await screen.findByRole("button", { name: "Setup" }));

    expect((await screen.findByLabelText("Chart setup")).value).toBe("CORPORATE");
    const currency = screen.getByLabelText("Starter currency");
    await waitFor(() => expect(currency.value).toBe(usdId));
    expect(within(currency).queryByRole("option", { name: /Widget/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Create chart" }));

    await waitFor(() => expect(postedBodies).toHaveLength(1));
    expect(postedBodies[0]).toMatchObject({
      entity_id: book.entity_id,
      name: "Primary chart",
      starter_template: "CORPORATE",
      resource_type_id: usdId,
    });
    const accountTable = (await screen.findByRole("columnheader", { name: "Account hierarchy" })).closest("table");
    const checking = within(accountTable).getByText(/Checking Account/, { selector: "span" });
    expect(checking.style.paddingLeft).toBe("3.75rem");
  });

  it("requires a blank current-book passphrase when the owner initiates transfer", async () => {
    const fetchMock = vi.fn((path) => {
      const value = String(path);
      if (value === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (value === "/api/auth/me") return response({ user: { user_id: "owner-id", email: "owner@example.com", display_name: "Owner" }, is_bootstrap_owner: true, allowed_actions: [] });
      if (value === "/api/books/mine") return response([book]);
      if (value.includes("/workflows/mine")) return response([]);
      if (value.endsWith("/entities")) return response([{ entity_id: book.entity_id, name: "Test Book" }]);
      if (value.endsWith("/resource-types") || value.includes("/charts?") || value.includes("/periods?") || value.endsWith("/prices") || value.includes("/entries?") || value.endsWith("/audit-log") || value.includes("/workflows?") || value.includes("/roles?") || value.endsWith("/workflow-artifacts") || value.endsWith("/users")) return response([]);
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    await user.click(screen.getByRole("button", { name: "Ownership" }));
    const passphrase = screen.getByLabelText("Current book passphrase");
    expect(passphrase.value).toBe("");
    expect(passphrase.getAttribute("autocomplete")).toBe("new-password");
    expect(screen.queryByLabelText("New passphrase")).toBeNull();
  });

  it("shows the nominated successor an acceptance form with their own blank passphrase", async () => {
    const pendingBook = {
      ...book,
      pending_owner_transfer: {
        transfer_id: "55555555-5555-4555-8555-555555555555",
        new_owner_email: "successor@example.com",
        initiated_at_ms: 1,
      },
    };
    const fetchMock = vi.fn((path) => {
      const value = String(path);
      if (value === "/api/auth/config") return response({ providers: [], dev_login_enabled: true });
      if (value === "/api/auth/me") return response({ user: { user_id: "successor-id", email: "successor@example.com", display_name: "Successor" }, is_bootstrap_owner: false, allowed_actions: [] });
      if (value === "/api/books/mine") return response([pendingBook]);
      if (value.endsWith("/ownership-transfer/accept")) return response({ ...pendingBook, owner_email: "successor@example.com", pending_owner_transfer: null });
      throw new Error(`unexpected fetch ${path}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /Test Book/ }));
    const newPassphrase = await screen.findByLabelText("Choose new book passphrase");
    const confirmation = screen.getByLabelText("Confirm new passphrase");
    expect(newPassphrase.value).toBe("");
    await user.type(newPassphrase, "successor secret");
    await user.type(confirmation, "successor secret");
    await user.click(screen.getByRole("button", { name: "Accept ownership" }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([path, options]) =>
      String(path).endsWith("/ownership-transfer/accept")
      && JSON.parse(options.body).new_passphrase === "successor secret"
    )).toBe(true));
  });
});
