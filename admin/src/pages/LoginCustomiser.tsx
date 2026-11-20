/**
 * Login customiser (§6.3 Login + preview screen 12).
 *
 * Logo (media library upload), brand colours, background mode,
 * layout, welcome heading, tagline/social/back-to-site toggles,
 * custom CSS — with a live preview that updates as you type.
 *
 * Contrast: WCAG ratios are computed from the custom colours
 * (accent on white, white on accent, accent on deep) and the UI
 * WARNS when a pair fails AA — it never silently applies a failing
 * combination. The warning does not block saving: the admin may
 * still choose it deliberately, but the failure is visible.
 *
 * Persistence — LANDED (API worker, P4): choices save to the server
 * via PUT /api/settings (flat camelCase branding fields, stored
 * under the wl_login_* option keys) and the login page reads them
 * from public GET /api/login-branding. See lib/loginBranding.ts.
 * A local copy is kept as a read cache only. Bad hex colours are
 * rejected by the server (400) and surfaced here.
 */
import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { Alert, Badge, Button, Card, Input, Select, Toggle } from "../components/elements";
import { contrastRatio, isHexColour } from "../lib/contrast";
import {
  DEFAULT_LOGIN_BRANDING,
  LOGIN_OPTION_KEYS,
  brandingToOptionMap,
  clearLocalBranding,
  fetchLoginBranding,
  readLocalBranding,
  saveLoginBranding,
} from "../lib/loginBranding";
import type { LoginBgMode, LoginBranding, LoginLayout } from "../lib/loginBranding";
import { LogoMark } from "../components/icons";

export function LoginCustomiserScreen(): React.ReactElement {
  const [branding, setBranding] = useState<LoginBranding>(() => readLocalBranding() ?? DEFAULT_LOGIN_BRANDING);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);

  // Edit the live server values, not a stale local copy.
  useEffect(() => {
    let cancelled = false;
    void fetchLoginBranding().then(({ branding: b }) => {
      if (!cancelled) setBranding(b);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const update = <K extends keyof LoginBranding>(key: K, value: LoginBranding[K]): void => {
    setBranding((prev) => ({ ...prev, [key]: value }));
    setSaved(false);
  };

  const save = async (): Promise<void> => {
    setSaving(true);
    setError(null);
    try {
      await saveLoginBranding(branding);
      setSaved(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save the login customisation.");
    } finally {
      setSaving(false);
    }
  };

  const reset = async (): Promise<void> => {
    setSaving(true);
    setError(null);
    try {
      await saveLoginBranding(DEFAULT_LOGIN_BRANDING);
      clearLocalBranding();
      setBranding(DEFAULT_LOGIN_BRANDING);
      setSaved(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not reset the login customisation.");
    } finally {
      setSaving(false);
    }
  };

  const uploadLogo = async (file: File): Promise<void> => {
    setUploading(true);
    setError(null);
    try {
      const item = await api.uploadMedia(file);
      update("logo", item.url);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Logo upload failed.");
    } finally {
      setUploading(false);
    }
  };

  const accentOnWhite = contrastRatio(branding.accent, "#ffffff");
  const whiteOnAccent = contrastRatio("#ffffff", branding.accent);
  const accentOnDeep = contrastRatio(branding.accent, branding.deep);
  const warnings: string[] = [];
  if (accentOnWhite !== null && !accentOnWhite.aaNormal) {
    warnings.push(`Accent ${branding.accent} on white is ${accentOnWhite.display}:1 — below WCAG AA 4.5:1 for normal text. Buttons and links in this colour will be hard to read.`);
  }
  if (whiteOnAccent !== null && !whiteOnAccent.aaNormal) {
    warnings.push(`White text on accent ${branding.accent} is ${whiteOnAccent.display}:1 — below WCAG AA 4.5:1. Button labels in white on this colour will be hard to read.`);
  }
  if (accentOnDeep !== null && !accentOnDeep.aaLarge) {
    warnings.push(`Accent ${branding.accent} on deep ${branding.deep} is ${accentOnDeep.display}:1 — below WCAG AA 3:1 even for large text. The split-panel layout will look washed out.`);
  }
  if (!isHexColour(branding.accent)) warnings.push("Accent is not a valid 6-digit hex colour (e.g. #0d9488).");
  if (!isHexColour(branding.deep)) warnings.push("Deep is not a valid 6-digit hex colour (e.g. #07332e).");

  const previewBg: React.CSSProperties =
    branding.bgMode === "dark"
      ? { background: `linear-gradient(160deg, ${branding.deep} 0%, #041114 100%)` }
      : branding.bgMode === "soft"
        ? { background: "#f2f6f4" }
        : branding.bgMode === "photo" && branding.bgImage !== ""
          ? { backgroundImage: `linear-gradient(rgba(6,26,30,0.55), rgba(6,26,30,0.55)), url(${branding.bgImage})`, backgroundSize: "cover", backgroundPosition: "center" }
          : { background: `linear-gradient(160deg, ${branding.deep} 0%, #041114 100%)` };

  return (
    <div>
      <Alert tone="info" title="Saved on the server">
        Saving writes the eleven login options (PUT /api/settings, stored under the wl_login_* keys) and the login
        page reads them from public GET /api/login-branding — the same choices apply on every device. Option keys: {LOGIN_OPTION_KEYS.join(", ")}.
      </Alert>
      {saved ? <Alert tone="ok" title="Login customisation saved">The login page now uses these settings.</Alert> : null}
      {error !== null ? <Alert tone="err" title="Login customiser" onDismiss={() => setError(null)}>{error}</Alert> : null}

      <div className="customizer-layout">
        <div className="customizer-controls">
          <Card title="Logo" sub="Upload to the media library, or paste an image URL. Empty = the Wordless mark.">
            <div className="toolbar">
              <input
                ref={fileRef}
                type="file"
                accept="image/*"
                aria-label="Logo image"
                onChange={(e) => {
                  const f = e.target.files?.[0];
                  if (f !== undefined) void uploadLogo(f);
                  e.target.value = "";
                }}
              />
              <Button icon="upload" disabled={uploading} onClick={() => fileRef.current?.click()}>{uploading ? "Uploading…" : "Upload logo"}</Button>
              {branding.logo !== "" ? <Button size="small" variant="danger" onClick={() => update("logo", "")}>Remove logo</Button> : null}
            </div>
            <Input label="Logo URL" value={branding.logo} onChange={(e) => update("logo", e.target.value)} placeholder="https://…/logo.png" />
          </Card>

          <Card title="Brand colours" sub="Contrast is checked live below — a failing pair warns, it is never silently applied.">
            <div className="form-grid">
              <Input label="Accent colour" value={branding.accent} onChange={(e) => update("accent", e.target.value)} hint="Buttons, links, focus rings. e.g. #0d9488" />
              <Input label="Deep colour" value={branding.deep} onChange={(e) => update("deep", e.target.value)} hint="Page background base and split panel. e.g. #07332e" />
            </div>
            <ul className="contrast-list">
              <li>Accent on white: <strong>{accentOnWhite?.display ?? "—"}:1</strong> {accentOnWhite !== null ? (accentOnWhite.aaNormal ? <Badge tone="teal">AA pass</Badge> : <Badge tone="red">AA fail</Badge>) : null}</li>
              <li>White text on accent: <strong>{whiteOnAccent?.display ?? "—"}:1</strong> {whiteOnAccent !== null ? (whiteOnAccent.aaNormal ? <Badge tone="teal">AA pass</Badge> : <Badge tone="red">AA fail</Badge>) : null}</li>
              <li>Accent on deep: <strong>{accentOnDeep?.display ?? "—"}:1</strong> {accentOnDeep !== null ? (accentOnDeep.aaLarge ? <Badge tone="teal">AA pass (large text)</Badge> : <Badge tone="red">AA fail</Badge>) : null}</li>
            </ul>
            {warnings.length > 0 ? (
              <Alert tone="warn" title="Contrast warnings — these colours fail WCAG AA">
                <ul>
                  {warnings.map((w) => (
                    <li key={w}>{w}</li>
                  ))}
                </ul>
              </Alert>
            ) : (
              <Alert tone="ok" title="Contrast passes WCAG AA">Accent on white, white on accent, and accent on deep all meet AA.</Alert>
            )}
          </Card>

          <Card title="Background & layout">
            <Select label="Background" value={branding.bgMode} onChange={(e) => update("bgMode", e.target.value as LoginBgMode)}>
              <option value="soft">Soft (light)</option>
              <option value="gradient">Brand gradient</option>
              <option value="dark">Dark</option>
              <option value="photo">Photo wash</option>
            </Select>
            {branding.bgMode === "photo" ? (
              <Input label="Background photo URL" value={branding.bgImage} onChange={(e) => update("bgImage", e.target.value)} placeholder="https://…/background.jpg" hint="Shown darkened under the card so the form stays readable." />
            ) : null}
            <Select label="Layout" value={branding.layout} onChange={(e) => update("layout", e.target.value as LoginLayout)}>
              <option value="card">Centred card</option>
              <option value="split">Split brand panel</option>
            </Select>
          </Card>

          <Card title="Text & extras">
            <Input label="Welcome heading" value={branding.heading} onChange={(e) => update("heading", e.target.value)} />
            <Input label="Tagline" value={branding.tagline} onChange={(e) => update("tagline", e.target.value)} hint="Leave blank to hide the tagline under the heading." />
            <Toggle label="Show social sign-in buttons" checked={branding.showSocial} onChange={(v) => update("showSocial", v)} />
            <p className="hint">No social provider is configured in v2 yet, so enabling this shows no working button on the login page — it only records the preference for when a provider lands.</p>
            <Toggle label="Show “Back to site” link" checked={branding.showBackLink} onChange={(v) => update("showBackLink", v)} />
          </Card>

          <Card title="Custom CSS" sub="Extra CSS applied to the login page only. Two-factor, lockout and password rules stay enforced underneath — branding never weakens security.">
            <div className="field">
              <label htmlFor="login-custom-css">Custom CSS</label>
              <textarea id="login-custom-css" value={branding.customCss} onChange={(e) => update("customCss", e.target.value)} rows={6} spellCheck={false} placeholder=".login-card { border-radius: 24px; }" />
            </div>
            <details>
              <summary>Option keys this screen writes</summary>
              <ul>
                {Object.entries(brandingToOptionMap(branding)).map(([k, v]) => (
                  <li key={k}><code>{k}</code> = <code>{String(v).slice(0, 60) || "(empty)"}</code></li>
                ))}
              </ul>
            </details>
          </Card>

          <div className="actions">
            <Button variant="primary" icon="check" disabled={saving} onClick={() => void save()}>{saving ? "Saving…" : "Save login customisation"}</Button>
            <Button variant="danger" icon="refresh" disabled={saving} onClick={() => void reset()}>Reset to defaults</Button>
          </div>
        </div>

        <div className="customizer-preview">
          <Card title="Live preview — your-site.com/login" sub="Every control on the left changes this preview instantly. What you see is what visitors get.">
            <div className={`login-preview${branding.layout === "split" ? " split" : ""}`} style={previewBg}>
              {branding.customCss !== "" ? <style>{branding.customCss}</style> : null}
              {branding.layout === "split" ? (
                <div className="login-preview-panel" style={{ background: branding.deep }}>
                  {branding.logo !== "" ? <img src={branding.logo} alt="Site logo" /> : <LogoMark size={40} />}
                  <strong>Wordless</strong>
                  <span>{branding.heading}</span>
                </div>
              ) : null}
              <div className="login-preview-card">
                {branding.logo !== "" ? <img src={branding.logo} alt="Site logo" className="login-preview-logo" /> : <LogoMark size={40} />}
                <strong>Wordless</strong>
                <span className="login-preview-heading">{branding.heading}</span>
                {branding.tagline !== "" ? <span className="muted">{branding.tagline}</span> : null}
                <span className="login-preview-field">Email address</span>
                <span className="login-preview-field">Password ········</span>
                <span className="login-preview-btn" style={{ background: branding.accent }}>Sign in</span>
                {branding.showSocial ? <span className="muted">Social sign-in (no provider configured yet)</span> : null}
                {branding.showBackLink ? <span style={{ color: branding.accent }}>← Back to site</span> : null}
              </div>
            </div>
            <p className="hint">Preview only — the form here does not sign anyone in.</p>
          </Card>
        </div>
      </div>
    </div>
  );
}
