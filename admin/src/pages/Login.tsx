/**
 * Login — refined card (§6.3).
 *
 * Password visibility toggle, TOTP as a smooth in-place step (no page
 * reload — the code field appears and focus moves to it), lockout state
 * that quotes the server's own retry time ("Try again after
 * <locked_until>") verbatim, and dark-mode aware via the admin token
 * system. Branding (logo, colours, background, layout, heading,
 * tagline, toggles, custom CSS) renders from
 * GET /api/login-branding when that endpoint exists; until then it
 * falls back to the saved customisation or the default look — see
 * lib/loginBranding.ts. Social sign-in buttons are shown only when
 * the customisation enables them AND a provider is actually
 * configured — there is no social provider in v2 yet, so the toggle
 * alone never renders a working button; the preview in Settings says
 * so. No fake sign-in options are rendered.
 *
 * Forced password change (P4, entry-criteria): when the login (or
 * /me) response carries `mustChangePassword: true` — set by an admin
 * reset — the user is NOT let into the admin. Instead this page shows
 * a forced-change step (new password + confirm, at least 8
 * characters, different from the temporary password) calling
 * POST /api/auth/change-password with the just-used password as
 * `currentPassword`. The change revokes every session and clears the
 * cookie, so the page signs in again with the new password before
 * calling onLoggedIn. There is no skip: the only way out of the step
 * is "Back to sign-in", which logs nothing in — the flag stays set
 * server-side, so the next sign-in prompts again. Session restore is
 * gated too: App.tsx checks the flag on /me and, when set, revokes the
 * session and sends the user to sign-in with an explanation instead of
 * letting them into the admin (coordinator fix, 2026-11-19).
 */
import { useEffect, useRef, useState } from "react";
import { api, ApiError } from "../lib/api";
import type { User } from "../lib/types";
import { Alert, Button, Input } from "../components/elements";
import { Icon, LogoMark } from "../components/icons";
import { fetchLoginBranding } from "../lib/loginBranding";
import type { LoginBranding } from "../lib/loginBranding";

/** The server lockout message carries the exact retry time; surface it, don't paraphrase it. */
function lockoutText(message: string): string | null {
  const m = /Try again after\s+(.+?)\.?\s*$/.exec(message);
  if (m === null) return null;
  return `Too many failed attempts. You can try again after ${m[1]}.`;
}

export function LoginPage({
  onLoggedIn,
  notice,
}: {
  onLoggedIn: (user: User) => void;
  /** Shown above the sign-in form — used when session restore finds a forced password change pending. */
  notice?: string;
}): React.ReactElement {
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [totp, setTotp] = useState("");
  const [needsTotp, setNeedsTotp] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lockout, setLockout] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [branding, setBranding] = useState<LoginBranding | null>(null);
  // Forced-change step (see header): set when the login response
  // carries mustChangePassword. `password` (the temporary password
  // just used to sign in) is the change call's currentPassword.
  const [needsPasswordChange, setNeedsPasswordChange] = useState(false);
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [showNewPassword, setShowNewPassword] = useState(false);
  const [changeError, setChangeError] = useState<string | null>(null);
  const totpRef = useRef<HTMLInputElement | null>(null);
  const changeRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    void fetchLoginBranding().then(({ branding: b }) => {
      if (!cancelled) setBranding(b);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (needsTotp) totpRef.current?.focus();
  }, [needsTotp]);

  useEffect(() => {
    if (needsPasswordChange) changeRef.current?.focus();
  }, [needsPasswordChange]);

  const submit = async (e?: React.FormEvent): Promise<void> => {
    e?.preventDefault();
    setError(null);
    setLockout(null);
    setBusy(true);
    try {
      const { user, mustChangePassword } = await api.login(login.trim(), password, needsTotp ? totp.trim() : undefined);
      if (mustChangePassword === true) {
        // Temp-password sign-in: do NOT enter the admin. The
        // forced-change step below is the only way forward.
        setNeedsTotp(false);
        setNeedsPasswordChange(true);
        return;
      }
      onLoggedIn(user);
    } catch (err) {
      if (err instanceof ApiError) {
        if (err.status === 429) {
          setLockout(lockoutText(err.message) ?? "Too many failed attempts. Wait 15 minutes before trying again.");
        } else if (err.status === 401 && err.message.includes("Two-factor")) {
          setNeedsTotp(true);
          setError("Two-factor authentication is enabled for this account — enter the 6-digit code from your authenticator app (or a recovery code).");
        } else {
          setError(err.message);
        }
      } else {
        setError("Login failed. Please try again.");
      }
    } finally {
      setBusy(false);
    }
  };

  /** Leave the forced-change step without entering the admin. The
   *  flag stays set server-side, so the next sign-in prompts again.
   *  This is not a skip — nothing here calls onLoggedIn. */
  const backToSignIn = (): void => {
    setNeedsPasswordChange(false);
    setNewPassword("");
    setConfirmPassword("");
    setChangeError(null);
    setPassword("");
    setTotp("");
    setNeedsTotp(false);
  };

  const submitChange = async (e?: React.FormEvent): Promise<void> => {
    e?.preventDefault();
    setChangeError(null);
    if (newPassword.length < 8) {
      setChangeError("Choose a new password of at least 8 characters.");
      return;
    }
    if (newPassword !== confirmPassword) {
      setChangeError("The two new passwords do not match.");
      return;
    }
    if (newPassword === password) {
      setChangeError("Choose a password different from the temporary one.");
      return;
    }
    setBusy(true);
    try {
      await api.changePassword(password, newPassword);
      // The change revoked every session and cleared the cookie, so
      // sign in again with the new password before entering.
      try {
        const { user, mustChangePassword } = await api.login(login.trim(), newPassword);
        if (mustChangePassword === true) {
          // Should not happen — the change clears the flag — but if
          // the server still demands a change, do not enter the admin.
          setChangeError("Your password was changed, but the server still asks for a new one. Sign in again to retry.");
          backToSignIn();
          setError("Your password was changed. Sign in with the new password.");
          return;
        }
        onLoggedIn(user);
      } catch {
        // Password changed; only the re-sign-in failed (e.g. network).
        // Send the user back to the sign-in form — the new password
        // is the valid one now.
        backToSignIn();
        setError("Your password was changed. Sign in with your new password.");
      }
    } catch (err) {
      if (err instanceof ApiError) setChangeError(err.message);
      else setChangeError("Could not change your password. Please try again.");
    } finally {
      setBusy(false);
    }
  };

  const b = branding;
  const wrapStyle: React.CSSProperties =
    b === null
      ? {}
      : b.bgMode === "dark"
        ? { background: `linear-gradient(160deg, ${b.deep} 0%, #041114 100%)` }
        : b.bgMode === "soft"
          ? { background: "#f2f6f4" }
          : b.bgMode === "photo" && b.bgImage !== ""
            ? { backgroundImage: `linear-gradient(rgba(6,26,30,0.55), rgba(6,26,30,0.55)), url(${b.bgImage})`, backgroundSize: "cover", backgroundPosition: "center" }
            : { background: `linear-gradient(160deg, ${b.deep} 0%, #041114 100%)` };

  return (
    <div className={`login-wrap${b?.layout === "split" ? " login-split" : ""}`} style={wrapStyle}>
      {b?.customCss !== undefined && b.customCss !== "" ? <style>{b.customCss}</style> : null}
      <div className="login-inner">
        <div className="login-card">
          <div className="login-brand">
            {b?.logo !== undefined && b.logo !== "" ? (
              <img src={b.logo} alt="Site logo" className="login-logo" />
            ) : (
              <LogoMark size={52} />
            )}
            <h1>Wordless</h1>
            <p className="login-heading">{b?.heading ?? "Welcome back"}</p>
            {b === null || b.tagline !== "" ? <p>{b?.tagline ?? "The autonomous website operating system"}</p> : null}
          </div>
          {lockout !== null ? (
            <Alert tone="warn" title="Account locked for now">{lockout} Your password is not wrong — the lockout clears itself at that time.</Alert>
          ) : null}
          {notice !== undefined && notice !== "" ? (
            <Alert tone="warn" title="Password change required">{notice}</Alert>
          ) : null}
          {error !== null ? <Alert tone="err" title="Could not sign you in">{error}</Alert> : null}
          {needsPasswordChange ? (
            <form onSubmit={(e) => void submitChange(e)}>
              <Alert tone="warn" title="Choose a new password">
                An administrator reset your password. Choose a new one before you can use the admin — this step can&rsquo;t be skipped, and the temporary password stops working once you change it.
              </Alert>
              {changeError !== null ? <Alert tone="err" title="Could not change your password">{changeError}</Alert> : null}
              <div className="field">
                <label htmlFor="login-new-password">New password</label>
                <div className="input-icon password-field">
                  <Icon name="lock" size={17} />
                  <input
                    ref={changeRef}
                    id="login-new-password"
                    type={showNewPassword ? "text" : "password"}
                    autoComplete="new-password"
                    placeholder="At least 8 characters"
                    value={newPassword}
                    onChange={(e) => setNewPassword(e.target.value)}
                    required
                  />
                  <Button
                    iconOnly
                    icon="eye"
                    aria-label={showNewPassword ? "Hide new password" : "Show new password"}
                    variant="ghost"
                    size="small"
                    onClick={() => setShowNewPassword((v) => !v)}
                  />
                </div>
                <p className="hint">At least 8 characters, and different from the temporary password.</p>
              </div>
              <div className="field">
                <label htmlFor="login-confirm-password">Confirm new password</label>
                <div className="input-icon password-field">
                  <Icon name="lock" size={17} />
                  <input
                    id="login-confirm-password"
                    type={showNewPassword ? "text" : "password"}
                    autoComplete="new-password"
                    placeholder="Repeat the new password"
                    value={confirmPassword}
                    onChange={(e) => setConfirmPassword(e.target.value)}
                    required
                  />
                </div>
              </div>
              <Button variant="primary" disabled={busy} size="large" onClick={() => void submitChange()}>
                {busy ? "Setting new password…" : "Set new password and sign in"}
              </Button>
              <p style={{ marginTop: 12, textAlign: "center" }}>
                <Button variant="ghost" size="small" onClick={backToSignIn}>
                  Back to sign-in
                </Button>
              </p>
            </form>
          ) : (
          <form onSubmit={(e) => void submit(e)}>
            <Input
              label="Username or email"
              id="login-username"
              autoComplete="username"
              placeholder="you@example.com"
              value={login}
              onChange={(e) => setLogin(e.target.value)}
              required
            />
            <div className="field">
              <label htmlFor="login-password">Password</label>
              <div className="input-icon password-field">
                <Icon name="lock" size={17} />
                <input
                  id="login-password"
                  type={showPassword ? "text" : "password"}
                  autoComplete="current-password"
                  placeholder="Your password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  required
                />
                <Button
                  iconOnly
                  icon="eye"
                  aria-label={showPassword ? "Hide password" : "Show password"}
                  variant="ghost"
                  size="small"
                  onClick={() => setShowPassword((v) => !v)}
                />
              </div>
            </div>
            <div className={`totp-step${needsTotp ? " open" : ""}`} aria-hidden={!needsTotp}>
              {needsTotp ? (
                <div className="field">
                  <label htmlFor="login-totp">Authenticator code (or recovery code)</label>
                  <div className="input-icon">
                    <Icon name="security" size={17} />
                    <input
                      ref={totpRef}
                      id="login-totp"
                      inputMode="numeric"
                      autoComplete="one-time-code"
                      placeholder="6-digit code"
                      value={totp}
                      onChange={(e) => setTotp(e.target.value)}
                      required
                    />
                  </div>
                  <p className="hint">Two-factor is on for this account. The code changes every 30 seconds.</p>
                </div>
              ) : null}
            </div>
            <Button variant="primary" disabled={busy} size="large" onClick={() => void submit()}>
              {busy ? "Signing in…" : needsTotp ? "Verify and sign in" : "Log in"}
            </Button>
          </form>
          )}
          {b?.showSocial === true ? (
            <p className="hint" style={{ marginTop: 14, textAlign: "center" }}>
              Social sign-in is switched on in the login customiser, but no social provider is configured in v2 yet — so no social button is shown here. Configure a provider first; the buttons will appear then.
            </p>
          ) : null}
        </div>
        {b?.showBackLink !== false ? (
          <p className="login-foot">
            <a href="/">← Back to the site</a>
          </p>
        ) : null}
        <p className="login-foot" style={{ marginTop: 8 }}>
          Five failed attempts in 15 minutes locks sign-in for 15 minutes.
        </p>
      </div>
    </div>
  );
}
