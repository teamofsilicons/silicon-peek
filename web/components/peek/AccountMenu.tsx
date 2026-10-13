"use client";

import { useCallback, useEffect, useState } from "react";
import { Accordion } from "@/components/silicon-ui/accordion/accordion";
import { Alert } from "@/components/silicon-ui/alert/alert";
import { Button } from "@/components/silicon-ui/button/button";
import { PasswordField } from "@/components/silicon-ui/password-field/password-field";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/silicon-ui/popover/popover";
import { UserMenu } from "@/components/silicon-ui/user-menu/user-menu";
import "@/styles/account.css";
import { restoreSession, signIn, signInSilicon, signOut, useSession, watchSession } from "@/lib/session";

/** Sign-in with Silicon Accounts: a popover while signed out, Silicon UI's user menu once signed in. */
export default function AccountMenu() {
  const session = useSession();
  const [restoring, setRestoring] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  const [slt, setSlt] = useState("");

  const run = useCallback(async (action: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }, []);

  // While signed out, a problem opens the sign-in popover, which shows it.
  useEffect(() => {
    if (error && !session) setOpen(true);
  }, [error, session]);

  const alert = (
    <Alert tone="danger" title="Silicon Accounts" open={!!error} onDismiss={() => setError("")}>
      {error}
    </Alert>
  );

  useEffect(() => {
    restoreSession()
      .catch((problem) => setError(problem instanceof Error ? problem.message : String(problem)))
      .finally(() => setRestoring(false));
    return watchSession(setError);
  }, []);

  return (
    <div className="account-menu">
      {session ? (
        <UserMenu
          user={{ name: session.actor.public_id, email: session.actor.type === "silicon" ? "Silicon account" : "Carbon account", plan: "Silicon Accounts" }}
          showTheme={false}
          onSignOut={() => run(signOut)}
        />
      ) : (
        <Popover open={open} onOpenChange={setOpen}>
          <PopoverTrigger asChild>
            <Button variant="secondary" size="sm" loading={restoring}>
              Sign in
            </Button>
          </PopoverTrigger>
          <PopoverContent align="end" className="account-panel" aria-label="Silicon Accounts">
            <strong className="account-title">Silicon Accounts</strong>
            <p>Use your own Carbon or Silicon account.</p>
            <Button loading={busy} onClick={() => void run(signIn)}>
              Continue with Silicon Accounts
            </Button>
            <Accordion
              defaultOpen={-1}
              items={[
                {
                  title: "Sign in as a Silicon",
                  content: (
                    <form
                      className="account-form"
                      onSubmit={(event) => {
                        event.preventDefault();
                        const token = slt.trim();
                        setSlt("");
                        if (token) void run(() => signInSilicon(token));
                      }}
                    >
                      <p>
                        Get a one-use token with <code>silicon-accounts login --app peek</code>.
                      </p>
                      <PasswordField label="Short-lived token" value={slt} onChange={(event) => setSlt(event.currentTarget.value)} autoComplete="off" required />
                      <Button type="submit" variant="secondary" loading={busy}>
                        Sign in
                      </Button>
                    </form>
                  ),
                },
              ]}
            />
            {alert}
          </PopoverContent>
        </Popover>
      )}
      {session ? <div className="account-alert">{alert}</div> : null}
    </div>
  );
}
