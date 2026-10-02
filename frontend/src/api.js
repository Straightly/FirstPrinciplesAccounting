export async function api(path, options = {}) {
  try {
    const response = await fetch(path, {
      credentials: "same-origin",
      ...options,
      headers: options.body
        ? { "Content-Type": "application/json", ...(options.headers || {}) }
        : options.headers,
    });
    const body = await response.json().catch(() => ({}));
    return { ok: response.ok, status: response.status, body };
  } catch (error) {
    return {
      ok: false,
      status: 0,
      body: { error_code: "NETWORK_ERROR", message: error.message },
    };
  }
}

export function errorText(result) {
  return `${result.body?.error_code || "ERROR"}: ${result.body?.message || "Request failed"}`;
}

export function newId() {
  return crypto.randomUUID();
}

export function today() {
  return new Date().toISOString().slice(0, 10);
}
