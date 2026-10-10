import { Show, createSignal, onCleanup, onMount } from "solid-js";
import Button from "../components/silicon-ui/Button.tsx";
import { restoreSession, session, signIn, signInSilicon, signOut, watchSession } from "./session.ts";
import "../styles/account.css";

export default function AccountMenu() {
  const [open, setOpen] = createSignal(false);
  const [busy, setBusy] = createSignal(true);
  const [error, setError] = createSignal("");
  const [slt, setSlt] = createSignal("");
  const run = async (action: () => Promise<void>) => {
    setBusy(true); setError("");
    try { await action(); }
    catch (problem) { setError(problem instanceof Error ? problem.message : String(problem)); }
    finally { setBusy(false); }
  };
  onMount(() => {
    void run(restoreSession);
    const stop = watchSession(setError);
    onCleanup(stop);
  });
  return <div class="account-menu">
    <Button variant="secondary" aria-expanded={open()} aria-controls="account-panel" onClick={() => setOpen(!open())}>
      {session()?.actor.public_id ?? "Sign in"}
    </Button>
    <Show when={open() || error()}>
      <section class="account-panel" id="account-panel" aria-label="Silicon Accounts">
        <div class="account-heading"><strong>Silicon Accounts</strong><button type="button" aria-label="Close account panel" onClick={() => { setOpen(false); setError(""); }}>×</button></div>
        <Show when={session()} fallback={<>
          <p>Use your own Carbon or Silicon account.</p>
          <Button loading={busy()} onClick={() => void run(signIn)}>Continue with Silicon Accounts</Button>
          <details><summary>Sign in as a Silicon</summary>
            <p>Get a one-use token with <code>silicon-accounts login --app peek</code>.</p>
            <form onSubmit={event => { event.preventDefault(); const token = slt().trim(); setSlt(""); if (token) void run(() => signInSilicon(token)); }}>
              <label for="peek-slt">Short-lived token</label>
              <input id="peek-slt" type="password" value={slt()} onInput={event => setSlt(event.currentTarget.value)} autocomplete="off" required />
              <Button loading={busy()} type="submit">Sign in</Button>
            </form>
          </details>
        </>}>
          <p>Signed in as <strong>{session()?.actor.public_id}</strong>.</p>
          <p class="account-note">Your sign-in stays available in this browser until it expires or you sign out.</p>
          <Button variant="secondary" loading={busy()} onClick={() => void run(signOut)}>Sign out</Button>
        </Show>
        <Show when={error()}><p class="account-error" role="alert">{error()}</p></Show>
      </section>
    </Show>
  </div>;
}
