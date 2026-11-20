/**
 * Security Centre (§6.3) — a status board, not a pile of forms.
 *
 * Headers, lockout, 2FA coverage across users, sessions, and recent
 * failed logins — each with its real state and a plain-language
 * reading. 2FA setup shows the secret and recovery codes once, as
 * the API returns them.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { SecurityOverview, SessionView, TotpSetup } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Skeleton, StatCard, Table } from "../components/elements";

export function SecurityPage(): React.ReactElement {
  const [overview, setOverview] = useState<SecurityOverview | null>(null);
  const [sessions, setSessions] = useState<SessionView[] | null>(null);
  const [twoFaEnabled, setTwoFaEnabled] = useState<boolean | null>(null);
  const [setup, setSetup] = useState<TotpSetup | null>(null);
  const [setupCode, setSetupCode] = useState("");
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [disablePassword, setDisablePassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [o, s, t] = await Promise.all([
        api.get<SecurityOverview>("/api/security/overview"),
        api.get<SessionView[]>("/api/auth/sessions"),
        api.get<{ enabled: boolean }>("/api/auth/2fa/status"),
      ]);
      setOverview(o);
      setSessions(s);
      setTwoFaEnabled(t.enabled);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load security overview.");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const startSetup = async (): Promise<void> => {
    setError(null);
    setBusy(true);
    try {
      setSetup(await api.post<TotpSetup>("/api/auth/2fa/setup", {}));
      setRecoveryCodes(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not start two-factor setup.");
    } finally {
      setBusy(false);
    }
  };

  const confirmSetup = async (): Promise<void> => {
    if (!setup) return;
    setError(null);
    setBusy(true);
    try {
      const result = await api.post<{ recoveryCodes: string[] }>("/api/auth/2fa/confirm", { code: setupCode.trim() });
      setRecoveryCodes(result.recoveryCodes);
      setSetup(null);
      setSetupCode("");
      setNotice("Two-factor authentication is now enabled. Store the recovery codes somewhere safe — they are shown once.");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "That code was not accepted.");
    } finally {
      setBusy(false);
    }
  };

  const disable2fa = async (): Promise<void> => {
    setError(null);
    setBusy(true);
    try {
      await api.post("/api/auth/2fa/disable", { password: disablePassword });
      setDisablePassword("");
      setNotice("Two-factor authentication is disabled. You have been signed out everywhere — sign in again.");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not disable two-factor authentication.");
    } finally {
      setBusy(false);
    }
  };

  const revokeAll = async (): Promise<void> => {
    if (!window.confirm("Sign out every session, including this one?")) return;
    setError(null);
    try {
      await api.post<{ revoked: number }>("/api/auth/sessions/revoke-all", {});
      setNotice("All sessions revoked. You will need to sign in again.");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not revoke sessions.");
    }
  };

  const lockedCount = overview?.lockout.lockedKeys.length ?? 0;
  const twoFaCount = overview?.twoFactor.filter((u) => u.enabled).length ?? 0;
  const twoFaTotal = overview?.twoFactor.length ?? 0;

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Security" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? (
        <Alert tone="ok" onDismiss={() => setNotice(null)}>
          {notice}
          {recoveryCodes !== null ? (
            <span>
              <strong> Recovery codes (one-time use, shown once):</strong>
              <ul>
                {recoveryCodes.map((code) => (
                  <li key={code}><code>{code}</code></li>
                ))}
              </ul>
            </span>
          ) : null}
        </Alert>
      ) : null}

      {overview === null ? (
        <div className="grid" aria-label="Loading security status">
          <Skeleton height={92} label="Loading headers status" />
          <Skeleton height={92} label="Loading lockout status" />
          <Skeleton height={92} label="Loading two-factor coverage" />
          <Skeleton height={92} label="Loading sessions" />
        </div>
      ) : (
        <div className="grid">
          <StatCard label="Security headers" value={overview.headers.applied ? "Applied" : "Not applied"} icon="shield" tone={overview.headers.applied ? undefined : "danger"} />
          <StatCard label="Keys currently locked out" value={String(lockedCount)} icon="lock" tone={lockedCount > 0 ? "warn" : undefined} />
          <StatCard label="Two-factor coverage" value={`${twoFaCount} of ${twoFaTotal}`} icon="key" tone={twoFaTotal > 0 && twoFaCount < twoFaTotal ? "warn" : undefined} />
          <StatCard label="Active sessions (site-wide)" value={String(overview.sessions.active)} icon="users" />
        </div>
      )}

      <Card title="Security headers" sub="Applied to every response, including error pages.">
        {overview === null ? (
          <Skeleton height={60} label="Loading headers" />
        ) : (
          <p>
            {overview.headers.applied ? "Headers are applied" : "Headers are NOT applied"}: Content-Security-Policy, X-Content-Type-Options: nosniff, X-Frame-Options: DENY, Referrer-Policy, Permissions-Policy. The CSP allows the inline styles and scripts the admin and analytics need — see docs/security/security.md for the rationale.
          </p>
        )}
      </Card>

      <Card title="Login lockout" sub="Five failed attempts in 15 minutes locks a login name / IP; a successful login clears the counter.">
        {overview === null ? (
          <Skeleton height={80} label="Loading lockout" />
        ) : (
          <>
            <p>
              {overview.lockout.config.maxAttempts} failed attempts in {overview.lockout.config.windowMinutes} minutes locks a login name / IP for {overview.lockout.config.lockoutMinutes} minutes (doubling on consecutive lockouts, capped at 60).
            </p>
            {overview.lockout.lockedKeys.length === 0 ? (
              <EmptyState icon="lock" title="No keys are currently locked" />
            ) : (
              <Table
                caption="Locked keys"
                columns={[
                  { key: "key", label: "Key" },
                  { key: "attempts", label: "Attempts" },
                  { key: "until", label: "Locked until" },
                ]}
                rows={overview.lockout.lockedKeys.map((k) => ({ key: <code>{k.key}</code>, attempts: String(k.attempts), until: k.lockedUntil }))}
              />
            )}
            {overview.defaultAdminLoginExists ? (
              <Alert tone="err" title="Default login name in use">A user with the default login name “admin” exists — rename it.</Alert>
            ) : (
              <p className="muted">No user with the default login name “admin” exists.</p>
            )}
          </>
        )}
      </Card>

      <Card title="Two-factor authentication" sub="Your account, plus coverage across the users the API reports.">
        {twoFaEnabled === null ? (
          <Skeleton height={80} label="Loading two-factor status" />
        ) : twoFaEnabled ? (
          <>
            <p>Two-factor authentication is enabled for your account.</p>
            <Input label="Account password (to disable 2FA)" id="disable-2fa-password" type="password" autoComplete="current-password" value={disablePassword} onChange={(e) => setDisablePassword(e.target.value)} hint="Disabling signs you out everywhere." />
            <Button variant="danger" disabled={busy} onClick={() => void disable2fa()}>Disable two-factor authentication</Button>
          </>
        ) : (
          <>
            <p>Two-factor authentication is not enabled for your account.</p>
            {setup === null ? (
              <Button variant="primary" icon="key" disabled={busy} onClick={() => void startSetup()}>Set up two-factor authentication</Button>
            ) : (
              <>
                <p>Scan this with your authenticator app, or enter the secret manually: <code>{setup.secret}</code></p>
                <p className="muted">Setup URL (QR-friendly): <code>{setup.url}</code></p>
                <Input label="6-digit code from your authenticator app" id="totp-setup-code" inputMode="numeric" autoComplete="one-time-code" value={setupCode} onChange={(e) => setSetupCode(e.target.value)} />
                <Button variant="primary" disabled={busy} onClick={() => void confirmSetup()}>Confirm and enable</Button>
              </>
            )}
          </>
        )}
        {overview !== null && overview.twoFactor.length > 0 ? (
          <>
            <h3>2FA coverage</h3>
            <Table
              caption="Two-factor coverage by user"
              columns={[
                { key: "login", label: "Login" },
                { key: "role", label: "Role" },
                { key: "enabled", label: "2FA" },
              ]}
              rows={overview.twoFactor.map((a) => ({
                login: a.login,
                role: a.role,
                enabled: a.enabled ? <Badge tone="teal">enabled</Badge> : <Badge tone="red">not enabled</Badge>,
              }))}
            />
          </>
        ) : null}
      </Card>

      <Card title={`Your sessions (${overview ? overview.sessions.active : "…"} active site-wide)`} sub="Sessions are identified by browser and last-seen time; only a prefix of the IP hash is ever shown.">
        {sessions === null ? (
          <Skeleton height={120} label="Loading sessions" />
        ) : sessions.length === 0 ? (
          <EmptyState icon="users" title="No sessions" />
        ) : (
          <>
            <Table
              caption="Sessions"
              columns={[
                { key: "created", label: "Created" },
                { key: "lastSeen", label: "Last seen" },
                { key: "expires", label: "Expires" },
                { key: "browser", label: "Browser" },
                { key: "status", label: "Status" },
              ]}
              rows={sessions.map((s) => ({
                created: s.createdAt,
                lastSeen: s.lastSeenAt ?? "—",
                expires: s.expiresAt,
                browser: s.userAgent ?? "—",
                status: (
                  <span>
                    {s.current ? <Badge tone="teal">this session</Badge> : null}
                    {s.revoked ? <Badge tone="red">revoked</Badge> : null}
                  </span>
                ),
              }))}
            />
            <Button variant="danger" onClick={() => void revokeAll()}>Revoke all my sessions</Button>
          </>
        )}
      </Card>

      <Card title="Recent failed logins">
        {overview === null ? (
          <Skeleton height={80} label="Loading failed logins" />
        ) : overview.failedLogins.length === 0 ? (
          <EmptyState icon="shield" title="No failed logins recorded" />
        ) : (
          <Table
            caption="Recent failed logins"
            columns={[
              { key: "when", label: "When" },
              { key: "details", label: "Details" },
            ]}
            rows={overview.failedLogins.map((f) => ({ when: f.createdAt, details: f.details ?? "—" }))}
          />
        )}
      </Card>
    </div>
  );
}
