import { createSignal } from "solid-js";

const ACCOUNTS_URL = "https://accounts.teamofsilicons.com";
const SESSION_KEY = "peek.accounts.session.v1";
const SIGNIN_KEY = "peek.accounts.signin.v1";
type Session = {
  access_token: string;
  refresh_token: string;
  expires_in: number;
  expires_at: number;
  refresh_token_expires_at: string;
  account_id: string;
  actor: { type: "carbon" | "silicon"; public_id: string };
};
type PendingSignIn = { state: string; verifier: string; redirect: string; created: number; code?: string };

const [session, setSession] = createSignal<Session | null>(null);
export { session };

class AccountsError extends Error {
  constructor(message: string, readonly status: number, readonly code: string) { super(message); }
}

async function request<T>(path: string, body?: unknown, token?: string): Promise<T> {
  const response = await fetch(`/api/v1/auth/${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { ...(body === undefined ? {} : { "Content-Type": "application/json" }), ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
    cache: "no-store",
  });
  const result = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new AccountsError(result.error?.message ?? result.error_description ?? "Silicon Accounts could not complete the request. Please try again.", response.status, result.error?.code ?? result.error ?? "");
  }
  return result as T;
}

function readSession(): Session | null {
  try {
    const raw = localStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    const saved = JSON.parse(raw) as Session;
    if (!saved.access_token || !saved.refresh_token || !saved.actor?.public_id || !Number.isFinite(saved.expires_at)
      || !Number.isFinite(Date.parse(saved.refresh_token_expires_at)) || Date.parse(saved.refresh_token_expires_at) <= Date.now()) {
      localStorage.removeItem(SESSION_KEY);
      return null;
    }
    return saved;
  } catch { return null; }
}

function saveSession(tokens: Omit<Session, "expires_at">): Session {
  const saved = { ...tokens, expires_at: Date.now() + tokens.expires_in * 1000 };
  localStorage.setItem(SESSION_KEY, JSON.stringify(saved));
  setSession(saved);
  return saved;
}

function clearSession() {
  localStorage.removeItem(SESSION_KEY);
  setSession(null);
}

/** Refresh tokens rotate once; the browser lock also serializes refreshes between tabs. */
async function currentSession(): Promise<Session | null> {
  return navigator.locks.request(SESSION_KEY, async () => {
    const saved = readSession();
    if (!saved) { setSession(null); return null; }
    if (saved.expires_at > Date.now() + 60_000) { setSession(saved); return saved; }
    try {
      const tokens = await request<Omit<Session, "expires_at">>("refresh", { refresh_token: saved.refresh_token });
      return saveSession(tokens);
    } catch (error) {
      // Connectivity and server failures must not erase a valid long-lived sign-in.
      if (error instanceof AccountsError && (error.status === 401 || error.code === "invalid_grant" || error.code === "session_rejected")) clearSession();
      else setSession(saved);
      throw error;
    }
  });
}

function base64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function signIn(): Promise<void> {
  const state = base64url(crypto.getRandomValues(new Uint8Array(32)));
  const verifier = base64url(crypto.getRandomValues(new Uint8Array(32)));
  const redirect = `${window.location.origin}/`;
  const challenge = base64url(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))));
  sessionStorage.setItem(SIGNIN_KEY, JSON.stringify({ state, verifier, redirect, created: Date.now() }));
  const url = new URL("/authorize", ACCOUNTS_URL);
  url.search = new URLSearchParams({ app_id: "peek", redirect_uri: redirect, response_type: "code", scope: "profile", state, code_challenge: challenge, code_challenge_method: "S256" }).toString();
  window.location.assign(url);
}

export async function signInSilicon(slt: string): Promise<void> {
  await navigator.locks.request(SESSION_KEY, async () => saveSession(await request<Omit<Session, "expires_at">>("login", { slt })));
}

export async function restoreSession(): Promise<void> {
  const params = new URLSearchParams(window.location.search);
  const code = params.get("code");
  const pending = sessionStorage.getItem(SIGNIN_KEY);
  let login: PendingSignIn | null = null;
  if (pending) {
    try { login = JSON.parse(pending) as PendingSignIn; }
    catch { sessionStorage.removeItem(SIGNIN_KEY); }
  }
  if (code || params.has("error")) {
    if (!login) throw new Error("This sign-in has no matching browser request. Please sign in again.");
    if (params.get("state") !== login.state || Date.now() - login.created > 10 * 60_000) throw new Error("The sign-in request expired or did not match. Please sign in again.");
    if (params.has("error")) {
      sessionStorage.removeItem(SIGNIN_KEY);
      history.replaceState(null, "", `${window.location.pathname}${window.location.hash}`);
      throw new Error(params.get("error_description") ?? "Sign-in was canceled.");
    }
    // Keep the code and PKCE verifier until the server's replayable exchange succeeds.
    login.code = code!;
    sessionStorage.setItem(SIGNIN_KEY, JSON.stringify(login));
    history.replaceState(null, "", `${window.location.pathname}${window.location.hash}`);
  }
  if (login?.code) {
    if (Date.now() - login.created > 10 * 60_000) {
      sessionStorage.removeItem(SIGNIN_KEY);
      throw new Error("The sign-in request expired. Please sign in again.");
    }
    const exchange = { code: login.code, redirect_uri: login.redirect, code_verifier: login.verifier };
    await navigator.locks.request(SESSION_KEY, async () => {
      saveSession(await request<Omit<Session, "expires_at">>("exchange", exchange));
      sessionStorage.removeItem(SIGNIN_KEY);
    });
  }
  const saved = await currentSession();
  if (!saved) return;
  try { await request("me", undefined, saved.access_token); }
  catch (error) {
    if (error instanceof AccountsError && (error.status === 401 || error.status === 403)) clearSession();
    throw error;
  }
}

export async function signOut(): Promise<void> {
  await navigator.locks.request(SESSION_KEY, async () => {
    const saved = readSession();
    if (saved) await request("logout", { token: saved.refresh_token });
    clearSession();
  });
}

export function watchSession(onError: (message: string) => void): () => void {
  const refresh = () => { if (!document.hidden) void currentSession().catch(error => onError(String(error.message ?? error))); };
  const changed = (event: StorageEvent) => { if (event.key === SESSION_KEY) setSession(readSession()); };
  const interval = window.setInterval(refresh, 60_000);
  window.addEventListener("storage", changed);
  window.addEventListener("focus", refresh);
  return () => { clearInterval(interval); window.removeEventListener("storage", changed); window.removeEventListener("focus", refresh); };
}
