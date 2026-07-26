// Thin fetch wrapper. Same-origin cookies are sent automatically; on a 401 we
// broadcast so the app can drop back to the login screen (mid-session expiry).
export async function apiFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  const res = await fetch(input, init);
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent("mh-unauthorized"));
  }
  return res;
}

export async function checkAuth(): Promise<boolean> {
  try {
    const res = await fetch("/api/v1/auth/status");
    if (!res.ok) return false;
    const data = (await res.json()) as { authenticated: boolean };
    return data.authenticated;
  } catch {
    return false;
  }
}

export async function login(code: string): Promise<boolean> {
  const res = await fetch("/api/v1/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  return res.ok;
}
