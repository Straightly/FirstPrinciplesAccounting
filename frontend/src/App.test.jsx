import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
