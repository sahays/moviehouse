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

// A short-lived capability for one media entry, appended to the stream and
// subtitle URLs. AirPlay/Chromecast receivers fetch those URLs from their own
// device, without the session cookie, so the token is what lets them play.
// Returns null on failure — the caller falls back to plain cookie-auth URLs,
// which still work for playback in this browser.
export async function fetchPlaybackToken(
  mediaId: string,
): Promise<string | null> {
  try {
    const res = await apiFetch(`/api/v1/media/${mediaId}/playback-token`, {
      method: "POST",
    });
    if (!res.ok) return null;
    const data = (await res.json()) as { token?: string };
    return data.token ?? null;
  } catch {
    return null;
  }
}

export async function logout(): Promise<void> {
  try {
    await fetch("/api/v1/auth/logout", { method: "POST" });
  } finally {
    // Clears the session cookie server-side; broadcast so the app returns to
    // the login screen (the App gate listens for this event).
    window.dispatchEvent(new CustomEvent("mh-unauthorized"));
  }
}
