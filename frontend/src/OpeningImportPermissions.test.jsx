import * as React from "react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import OwnerWorkspace from "./OwnerWorkspace.jsx";

const workflowSource = readFileSync(resolve(process.cwd(), "../backend/src/opening_import_template.js"), "utf8");

function response(body, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
}

function definition(key, type, property = null, parent = null) {
  return { external_account_key: key, proposed_account_name: key, proposed_account_type: type,
    resource_code: "USD", attribution_entity_key: property, parent_external_account_key: parent };
}

function packageFor(version = "1.2") {
  const building = definition("building", "ASSET", version === "1.0" ? null : "property");
  const equity = definition("equity", "EQUITY");
  const source = { source_id: "reviewed", label: "Synthetic reviewed source", sha256: "a".repeat(64) };
  const result = { schema_version: version, import_id: "synthetic-import", source_balance_date: "2024-12-31",
    opening_entry_date: "2025-01-01", declared_accounting_method: "ACCRUAL", declared_balance_basis: "BOOK",
    resource_codes: ["USD"], source_material: [source], control_totals: { USD: { debit: "100", credit: "100" } },
    balance_rows: [{ ...building, debit: "100", credit: "0", source_id: source.source_id, source_reference: "page 1" },
      { ...equity, debit: "0", credit: "100", source_id: source.source_id, source_reference: "page 1" }] };
  result.balance_rows.forEach((row) => { delete row.parent_external_account_key; });
  if (version !== "1.0") Object.assign(result, { entity_namespace: "synthetic", related_entities: [
    { external_entity_key: "property", name: "Example property", category: "PROPERTY", relationship: "OWNERSHIP", effective_from: "2020-01-01" },
  ] });
  if (version === "1.2") Object.assign(result, {
    account_definitions: [definition("depreciation", "EXPENSE", "property", "expenses"), building, equity,
      definition("expenses", "EXPENSE"), definition("accumulated", "ASSET", "property"), definition("rent", "REVENUE", "property")],
    property_facts: [{ external_entity_key: "property", address: "Synthetic address", property_type: "RESIDENTIAL",
      source_year: 2024, fair_rental_days: 365, personal_use_days: 0, source_id: source.source_id, source_reference: "page 2" }],
    fixed_assets: [{ external_asset_key: "asset", name: "Example building", attribution_entity_key: "property",
      cost_account_key: "building", accumulated_depreciation_account_key: "accumulated", depreciation_expense_account_key: "depreciation",
      land_account_key: null, in_service_date: "2024-01-01", cost: "100", land: "0", depreciable_basis: "100",
      business_use_percent: "100", useful_life_years: "27.5", method: "STRAIGHT_LINE", convention: "MID_MONTH",
      accumulated_depreciation_as_of: "2024-12-31", accumulated_depreciation: "0",
      schedules: [{ year: 2025, amount: "3.64", status: "PROJECTED", basis: "TAX_EQUALS_BOOK" }],
      source_id: source.source_id, source_reference: "page 3" }],
  });
  return result;
}

function renderWorkflow() {
  let component;
  // Exercise the actual generated standalone SPA without its deployed React bundle.
  const source = workflowSource.replace(/^import[^\n]+\n/, "").replace("__WORKFLOW_NAME_JSON__", JSON.stringify("Opening import"));
  new Function("React", "createRoot", source)(React, () => ({ render: (element) => { component = element.type; } }));
  render(React.createElement(component));
}

function mockWorkflow({ existing = [], prepareStatus = 200, importStatus = 200, reviewStatus = 200, reviewPermission = "list_fixed_assets", updateStatus = 200, failCreate = false } = {}) {
  const accounts = [...existing];
  const creates = [];
  const imports = [];
  const updates = [];
  let assetRecords;
  let postedMappings = {};
  let readDenied = reviewStatus !== 200;
  let createDenied = failCreate;
  const fetchMock = vi.fn((path, options = {}) => {
    const value = String(path);
    if (value.includes("/opening-import/context?")) return response({ chart_id: "chart", chart_name: "Primary" });
    if (value.includes("/accounts?")) return response(accounts);
    if (value.endsWith("/resource-types")) return response([{ resource_type_id: "usd", code: "USD" }]);
    if (value.endsWith("/prepare-identities")) return prepareStatus === 200
      ? response({ resolved_entities: [{ external_entity_key: "property", name: "Example property", entity_id: "property-id" }] })
      : response({ error_code: "PERMISSION_DENIED", message: "create_entity permission required" }, prepareStatus);
    if (value.endsWith("/accounts") && options.method === "POST") {
      const body = JSON.parse(options.body);
      creates.push(body);
      if (createDenied) return response({ error_code: "PERMISSION_DENIED", message: "create_account permission required" }, 403);
      const id = `account-${accounts.length + 1}`;
      accounts.push({ ...body, account_id: id, is_active: true });
      return response({ id });
    }
    if (value.endsWith("/opening-import") && options.method === "POST") {
      const body = JSON.parse(options.body);
      imports.push(body);
      postedMappings = body.account_mappings;
      return importStatus === 200 ? response({ id: "entry-id" })
        : response({ error_code: "PERMISSION_DENIED", message: "create_fixed_asset permission required" }, importStatus);
    }
    if (value.endsWith("/fixed-assets/asset-id") && options.method === "PATCH") {
      const body = JSON.parse(options.body); updates.push(body);
      if (updateStatus !== 200) return response({ error_code: updateStatus === 403 ? "PERMISSION_DENIED" : "INVALID_INPUT", message: updateStatus === 403 ? "update_fixed_asset permission required" : "stale revision" }, updateStatus);
      assetRecords = [{ ...body.asset, revision: body.asset.revision + 1 }];
      return response({ id: "update-id" });
    }
    if (value.endsWith("/opening-import/review")) {
      if (readDenied) return response({ error_code: "PERMISSION_DENIED", message: `${reviewPermission} permission required` }, 403);
      const pkg = imports.length ? JSON.parse(imports.at(-1).file_content) : packageFor();
      assetRecords ||= (pkg.fixed_assets || []).map((asset) => {
        const { external_asset_key, attribution_entity_key, cost_account_key, accumulated_depreciation_account_key,
          depreciation_expense_account_key, land_account_key, source_id, source_reference, ...facts } = asset;
        return { ...facts, asset_id: "asset-id", entity_id: "entity-id", external_key: external_asset_key,
          external_namespace: pkg.entity_namespace, property_entity_id: "property-id", cost_account_id: postedMappings[cost_account_key] || "cost-control",
          accumulated_depreciation_account_id: postedMappings[accumulated_depreciation_account_key] || "accumulated-control",
          depreciation_expense_account_id: postedMappings[depreciation_expense_account_key] || "depreciation-control",
          land_account_id: land_account_key ? postedMappings[land_account_key] : null, revision: 0, status: "ACTIVE",
          linked_entry_id: imports.length ? "entry-id" : "prior-entry-id", source_metadata: { source_id, source_reference } };
      });
      return response({ assets: assetRecords,
      property_profiles: (pkg.property_facts || []).map((profile) => ({ ...profile, property_entity_id: "property-id" })),
      balances: Object.entries(imports.length ? postedMappings : Object.fromEntries(accounts.map((account) => [account.name, account.account_id]))).map(([key, account_id]) => ({ account_id, account_name: key, property_entity_id: null,
        balance: { debit_total: key === "building" ? "100" : "0", credit_total: key === "equity" ? "100" : "0", natural: ["building", "equity"].includes(key) ? "100" : "0" } })) });
    }
    throw new Error(`Unexpected fetch: ${value}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  return { accounts, creates, imports, updates, fetchMock, assets: () => assetRecords,
    allowReview: () => { readDenied = false; }, allowCreate: () => { createDenied = false; } };
}

async function upload(user, pkg) {
  const file = new File([JSON.stringify(pkg)], "synthetic.json", { type: "application/json" });
  file.text = async () => JSON.stringify(pkg);
  await user.upload(screen.getByLabelText("Reviewed JSON file"), file);
  await screen.findByRole("heading", { name: "Package preview" });
}

async function prepare(user) {
  await user.click(screen.getByRole("button", { name: "Prepare identities" }));
  await screen.findByText("Prepared 1 properties.");
}

async function finish(user) {
  await user.click(screen.getByRole("button", { name: "Finish mapping" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Post opening balances" }).disabled).toBe(false));
}

describe("Opening import permission-aware workflow", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/?book_id=book");
    let id = 0;
    vi.stubGlobal("crypto", { randomUUID: vi.fn(() => `operation-${++id}`) });
  });
  afterEach(() => { cleanup(); vi.unstubAllGlobals(); window.history.replaceState({}, "", "/"); });

  it("maps every 1.2 definition, creates parents first, permits overrides, and reviews without owner-only reads", async () => {
    const mock = mockWorkflow();
    const user = userEvent.setup();
    renderWorkflow();
    await screen.findByText("Active chart: Primary");
    await upload(user, packageFor());
    expect(screen.getByRole("button", { name: "Finish mapping" }).disabled).toBe(true);
    expect(screen.getAllByText(/Zero opening balance:/)).toHaveLength(4);
    expect(within(screen.getByRole("table", { name: "Fixed assets preview" })).getByText("2025: 3.64 (PROJECTED; TAX_EQUALS_BOOK)")).toBeTruthy();
    expect(within(screen.getByRole("table", { name: "Property facts preview" })).getByText("Synthetic address")).toBeTruthy();
    expect(screen.getByText(/Combined register, property-profile, and balance review requires all three permissions: list_accounts, list_account_balances, and list_fixed_assets/)).toBeTruthy();
    await prepare(user);
    await user.clear(screen.getByLabelText("New account name for depreciation"));
    await user.type(screen.getByLabelText("New account name for depreciation"), "Reviewed depreciation");
    await finish(user);
    expect(mock.creates).toHaveLength(6);
    expect(mock.creates[0].name).toBe("expenses");
    expect(mock.creates[1]).toMatchObject({ name: "Reviewed depreciation", associated_entity_id: "property-id", parent_account_id: mock.accounts[0].account_id });
    await finish(user);
    expect(mock.creates).toHaveLength(6);
    await user.click(screen.getByRole("button", { name: "Post opening balances" }));
    await screen.findByRole("heading", { name: "Post-import verification" });
    expect(Object.keys(mock.imports[0].account_mappings).sort()).toEqual(packageFor().account_definitions.map((item) => item.external_account_key).sort());
    expect(mock.imports[0].file_content).toBe(JSON.stringify(packageFor()));
    await screen.findByRole("table", { name: "Imported asset register" });
    const balances = within(screen.getByRole("table", { name: "Imported account balances" }));
    expect(balances.getAllByRole("row")).toHaveLength(7);
    expect(balances.getByText("Reviewed depreciation")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Post opening balances" }).disabled).toBe(true);
    expect(mock.fetchMock.mock.calls.some(([path]) => String(path).endsWith("/opening-import/review"))).toBe(true);
    expect(mock.fetchMock.mock.calls.some(([path]) => String(path).endsWith("/balance"))).toBe(false);
    expect(mock.fetchMock.mock.calls.some(([path]) => String(path).endsWith("/entries"))).toBe(false);
  });

  it("allows selecting an existing parent and overriding the proposed hierarchy", async () => {
    const mock = mockWorkflow({ existing: [{ account_id: "parent", chart_id: "chart", name: "Existing expenses", account_type: "EXPENSE", resource_type_id: "usd", is_active: true }] });
    const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor()); await prepare(user);
    await user.selectOptions(screen.getByLabelText("Parent account for depreciation"), "parent");
    await finish(user);
    expect(mock.creates.find((item) => item.name === "depreciation").parent_account_id).toBe("parent");
  });

  it("preserves generated deployment placeholders and uses prepared property attribution for 1.2 choices", async () => {
    expect(workflowSource).toContain('const WORKFLOW_ID = "__WORKFLOW_ID__"');
    expect(workflowSource).toContain('const WORKFLOW_DEPLOYMENT_ID = "__WORKFLOW_DEPLOYMENT_ID__"');
    expect(workflowSource).toContain("const WORKFLOW_NAME = __WORKFLOW_NAME_JSON__");
    const candidates = [
      { account_id: "correct", chart_id: "chart", name: "Correct property building", associated_entity_id: "property-id" },
      { account_id: "wrong", chart_id: "chart", name: "Wrong property building", associated_entity_id: "other-property" },
      { account_id: "book-wide", chart_id: "chart", name: "Book-wide asset", associated_entity_id: null },
      { account_id: "other-chart", chart_id: "other-chart", name: "Other chart building", associated_entity_id: "property-id" },
    ].map((account) => ({ ...account, account_type: "ASSET", resource_type_id: "usd", is_active: true }));
    mockWorkflow({ existing: candidates }); const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor()); await prepare(user);
    const mapping = within(screen.getByLabelText("Account mapping for building"));
    expect(mapping.getByRole("option", { name: "Correct property building" })).toBeTruthy();
    for (const name of ["Wrong property building", "Book-wide asset", "Other chart building"]) expect(mapping.queryByRole("option", { name })).toBeNull();
  });

  it("reviews and refreshes the register on a reopened workflow without loading a file or posting", async () => {
    const mock = mockWorkflow({ existing: [{ account_id: "existing", chart_id: "chart", name: "Previously imported building", account_type: "ASSET", resource_type_id: "usd", is_active: true }] });
    const user = userEvent.setup(); renderWorkflow();
    await user.click(await screen.findByRole("button", { name: "Review current register and balances" }));
    await screen.findByRole("heading", { name: "Book register and balances" });
    await screen.findByRole("table", { name: "Imported asset register" });
    expect(within(screen.getByRole("table", { name: "Imported account balances" })).getByText("Previously imported building")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Refresh verification" }));
    await waitFor(() => expect(mock.fetchMock.mock.calls.filter(([path]) => String(path).endsWith("/opening-import/review"))).toHaveLength(2));
    expect(mock.imports).toHaveLength(0);
    expect(mock.creates).toHaveLength(0);
    expect(mock.fetchMock.mock.calls.some(([path]) => String(path).endsWith("/balance"))).toBe(false);
  });

  it("updates only names and schedule amounts using the current register revision without posting activity", async () => {
    const mock = mockWorkflow(); const user = userEvent.setup(); renderWorkflow();
    await user.click(await screen.findByRole("button", { name: "Review current register and balances" }));
    await user.click(await screen.findByRole("button", { name: "Review Example building" }));
    const original = structuredClone(mock.assets()[0]);
    const form = within(screen.getByRole("form", { name: "Asset name and schedule review" }));
    await user.clear(form.getByLabelText("Asset name")); await user.type(form.getByLabelText("Asset name"), "Reviewed building");
    await user.clear(form.getByLabelText("2025 schedule amount")); await user.type(form.getByLabelText("2025 schedule amount"), "4.00");
    await user.click(form.getByRole("button", { name: "Save review changes" }));
    await screen.findByText("Asset name and starting schedules updated. No journal entry or depreciation expense was posted.");
    expect(mock.updates).toHaveLength(1);
    expect(mock.updates[0].asset).toEqual({ ...original, name: "Reviewed building", schedules: [{ ...original.schedules[0], amount: "4.00" }] });
    expect(mock.assets()[0].revision).toBe(1);
    expect(mock.imports).toHaveLength(0);
    expect(mock.fetchMock.mock.calls.some(([path]) => String(path).endsWith("/fixed-assets/transactions") || String(path).endsWith("/entries"))).toBe(false);
    expect(screen.queryByRole("form", { name: "Asset name and schedule review" })).toBeNull();
  });

  it.each([403, 409])("preserves review edits on update rejection (%s) and does not alter register facts", async (status) => {
    const mock = mockWorkflow({ updateStatus: status }); const user = userEvent.setup(); renderWorkflow();
    await user.click(await screen.findByRole("button", { name: "Review current register and balances" }));
    await user.click(await screen.findByRole("button", { name: "Review Example building" }));
    await user.type(screen.getByLabelText("Asset name"), " revised");
    await user.click(screen.getByRole("button", { name: "Save review changes" }));
    await screen.findByText(status === 403 ? /PERMISSION_DENIED: update_fixed_asset permission required/ : /INVALID_INPUT: stale revision/);
    expect(screen.getByLabelText("Asset name").value).toBe("Example building revised");
    expect(mock.assets()[0].name).toBe("Example building");
    expect(mock.assets()[0].revision).toBe(0);
    expect(mock.imports).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Save review changes" }).disabled).toBe(false);
  });

  it("rejects cyclic parent choices before any accounts are created", async () => {
    const mock = mockWorkflow(); const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor()); await prepare(user);
    await user.selectOptions(screen.getByLabelText("Parent account for expenses"), "definition:depreciation");
    await user.click(screen.getByRole("button", { name: "Finish mapping" }));
    await screen.findByText(/Parent hierarchy contains a cycle/);
    expect(mock.creates).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Post opening balances" }).disabled).toBe(true);
  });

  it("requires explicit selection of existing accounts rather than silently reusing same-name accounts", async () => {
    const mock = mockWorkflow({ existing: [{ account_id: "building-id", chart_id: "chart", name: "building", account_type: "ASSET", resource_type_id: "usd", associated_entity_id: "property-id", is_active: true }] });
    const user = userEvent.setup(); renderWorkflow(); await upload(user, packageFor()); await prepare(user);
    await user.click(screen.getByRole("button", { name: "Finish mapping" }));
    await screen.findByText(/Account name already exists: building/);
    expect(mock.creates).toHaveLength(0);
    await user.selectOptions(screen.getByLabelText("Account mapping for building"), "building-id");
    await finish(user);
    expect(mock.creates).toHaveLength(5);
  });

  it("shows create_fixed_asset denial without pretending the import succeeded", async () => {
    const mock = mockWorkflow({ importStatus: 403 }); const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor()); await prepare(user); await finish(user);
    await user.click(screen.getByRole("button", { name: "Post opening balances" }));
    await screen.findByText("PERMISSION_DENIED: create_fixed_asset permission required");
    expect(screen.queryByRole("heading", { name: "Post-import verification" })).toBeNull();
    expect(mock.fetchMock.mock.calls.some(([path]) => String(path).endsWith("/opening-import/review"))).toBe(false);
    expect(screen.getByRole("button", { name: "Post opening balances" }).disabled).toBe(false);
  });

  it.each(["list_accounts", "list_account_balances", "list_fixed_assets"])("keeps a successful import posted when %s is missing and refreshes after permission is granted", async (permission) => {
    const mock = mockWorkflow({ reviewStatus: 403, reviewPermission: permission }); const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor()); await prepare(user); await finish(user);
    await user.click(screen.getByRole("button", { name: "Post opening balances" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(`${permission} permission required`);
    expect(alert.textContent).toContain("Review requires all three permissions: list_accounts, list_account_balances, and list_fixed_assets.");
    expect(screen.getByRole("button", { name: "Post opening balances" }).disabled).toBe(true);
    expect(screen.queryByRole("table", { name: "Imported asset register" })).toBeNull();
    expect(within(screen.getByRole("table", { name: "Imported account balances" })).getAllByText("Unavailable").length).toBeGreaterThan(0);
    mock.allowReview();
    await user.click(screen.getByRole("button", { name: "Refresh verification" }));
    await screen.findByRole("table", { name: "Imported asset register" });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(mock.imports).toHaveLength(1);
  });

  it("blocks account creation when identity preparation is denied", async () => {
    const mock = mockWorkflow({ prepareStatus: 403 }); const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor());
    await user.click(screen.getByRole("button", { name: "Prepare identities" }));
    await screen.findByText("PERMISSION_DENIED: create_entity permission required");
    expect(screen.getByRole("button", { name: "Finish mapping" }).disabled).toBe(true);
    expect(mock.creates).toHaveLength(0);
  });

  it("retries an unchanged account request with the same operation ID after permission denial", async () => {
    const mock = mockWorkflow({ failCreate: true }); const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor()); await prepare(user);
    await user.click(screen.getByRole("button", { name: "Finish mapping" }));
    await screen.findByText(/create_account permission required/);
    mock.allowCreate(); await finish(user);
    expect(mock.creates[1].op_id).toBe(mock.creates[0].op_id);
    expect(mock.accounts).toHaveLength(6);
  });

  it.each(["1.0", "1.1"])("retains the %s balance-row mapping and posting flow", async (version) => {
    const mock = mockWorkflow(); const user = userEvent.setup(); renderWorkflow();
    await upload(user, packageFor(version));
    if (version === "1.1") await prepare(user);
    else expect(screen.queryByRole("button", { name: "Prepare identities" })).toBeNull();
    await finish(user);
    expect(mock.creates).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Post opening balances" }));
    await screen.findByRole("heading", { name: "Post-import verification" });
    expect(Object.keys(mock.imports[0].account_mappings).sort()).toEqual(["building", "equity"]);
    expect(mock.creates.find((item) => item.name === "building").associated_entity_id).toBe(version === "1.1" ? "property-id" : null);
  });
});

describe("Generic asset and account-balance role permission choices", () => {
  afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
  it("offers explicit review and asset management grants in role creation and permission addition", async () => {
    const roleBodies = [];
    const permissionBodies = [];
    const fetchMock = vi.fn((path, options = {}) => {
      if (options.method === "POST" && String(path).endsWith("/roles")) { roleBodies.push(JSON.parse(options.body)); return response({ id: "role" }); }
      if (options.method === "POST" && String(path).endsWith("/permissions")) { permissionBodies.push(JSON.parse(options.body)); return response({ id: "role" }); }
      if (String(path).includes("/roles?")) return response([{ role_id: "role", name: "Asset reviewer", permissions: [], workflow_ids: [], assigned_user_ids: [] }]);
      return response([]);
    });
    vi.stubGlobal("fetch", fetchMock);
    const user = userEvent.setup();
    render(<OwnerWorkspace me={{ user: { user_id: "owner", email: "owner@example.com" } }}
      book={{ book_id: "book", entity_id: "entity", is_open: true, name: "Example book" }} isBookOwner={true}
      onChanged={vi.fn()} setMessage={vi.fn()} setError={vi.fn()} selectBook={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Workflows & roles" }));
    const createForm = within(screen.getByRole("heading", { name: "Create role" }).closest("form"));
    await user.type(createForm.getByLabelText("Name"), "Asset operator");
    for (const label of ["List accounts", "List account balances", "List fixed assets and property profiles", "Create fixed assets", "Update fixed assets"]) await user.click(createForm.getByLabelText(label));
    await user.click(createForm.getByRole("button", { name: "Create role" }));
    await waitFor(() => expect(roleBodies).toHaveLength(1));
    expect(roleBodies[0].permissions).toEqual(["list_accounts", "list_account_balances", "list_fixed_assets", "create_fixed_asset", "update_fixed_asset"]);
    const addForm = within(screen.getByRole("heading", { name: "Add permission to role" }).closest("form"));
    for (const permission of ["list_accounts", "list_account_balances", "list_fixed_assets", "create_fixed_asset", "update_fixed_asset"]) {
      await user.selectOptions(addForm.getByLabelText("Role"), "role");
      await user.selectOptions(addForm.getByLabelText("Permission"), permission);
      await user.click(addForm.getByRole("button", { name: "Add permission" }));
      await waitFor(() => expect(permissionBodies.some((item) => item.permission === permission)).toBe(true));
    }
  });
});
