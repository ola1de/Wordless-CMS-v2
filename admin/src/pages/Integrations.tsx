/**
 * Integrations admin (Milestone 8).
 *
 * Every integration's status is shown honestly (configured / not
 * configured, with the live detail). Non-secret settings can be edited
 * here — email API URL + sender, analytics provider + measurement id.
 * Secret fields are rejected by the server with a 400; secrets go in
 * `wrangler secret put` and are never shown or stored here.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { AnalyticsStatus, BindingStatus, IntegrationInfo, MailLogList } from "../lib/types";
import { EmptyState, ErrorBox, Pagination, Spinner } from "../components/ui";

export function IntegrationsPage(): React.ReactElement {
  const [integrations, setIntegrations] = useState<IntegrationInfo[]>([]);
  const [mail, setMail] = useState<MailLogList | null>(null);
  const [mailPage, setMailPage] = useState(1);
  const [analytics, setAnalytics] = useState<AnalyticsStatus | null>(null);
  const [bindings, setBindings] = useState<BindingStatus[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  // Email config form.
  const [emailApiUrl, setEmailApiUrl] = useState("");
  const [emailFromName, setEmailFromName] = useState("");
  const [emailFromEmail, setEmailFromEmail] = useState("");
  // Analytics config form.
  const [analyticsProvider, setAnalyticsProvider] = useState("none");
  const [analyticsMeasurementId, setAnalyticsMeasurementId] = useState("");
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    try {
      const list = await api.listIntegrations();
      setIntegrations(list);
      const email = list.find((i) => i.id === "email");
      if (email !== undefined) {
        setEmailApiUrl(typeof email.config.apiUrl === "string" ? email.config.apiUrl : "");
        setEmailFromName(typeof email.config.fromName === "string" ? email.config.fromName : "");
        setEmailFromEmail(typeof email.config.fromEmail === "string" ? email.config.fromEmail : "");
      }
      const an = list.find((i) => i.id === "analytics");
      if (an !== undefined) {
        setAnalyticsProvider(typeof an.config.provider === "string" ? an.config.provider : "none");
        setAnalyticsMeasurementId(
          typeof an.config.measurementId === "string" ? an.config.measurementId : "",
        );
      }
      setAnalytics(await api.getAnalyticsStatus());
      setBindings(await api.getCloudflareBindings());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load integrations.");
    } finally {
      setLoading(false);
    }
  }, []);

  const loadMail = useCallback(async (page: number) => {
    try {
      setMail(await api.listMailLog(page));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the mail log.");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    void loadMail(mailPage);
  }, [mailPage, loadMail]);

  const fail = (err: unknown): void => {
    setError(err instanceof Error ? err.message : "Something went wrong.");
  };

  const saveEmail = async (e: React.FormEvent): Promise<void> => {
    e.preventDefault();
    setError(null);
    setNotice(null);
    setSaving(true);
    try {
      await api.updateIntegrationConfig("email", {
        apiUrl: emailApiUrl.trim(),
        fromName: emailFromName.trim(),
        fromEmail: emailFromEmail.trim(),
      });
      setNotice("Email settings saved. The API key (if any) still needs `wrangler secret put EMAIL_API_KEY`.");
      await load();
    } catch (err) {
      fail(err);
    } finally {
      setSaving(false);
    }
  };

  const saveAnalytics = async (e: React.FormEvent): Promise<void> => {
    e.preventDefault();
    setError(null);
    setNotice(null);
    setSaving(true);
    try {
      await api.updateIntegrationConfig("analytics", {
        provider: analyticsProvider,
        measurementId: analyticsMeasurementId.trim(),
      });
      setNotice("Analytics settings saved.");
      await load();
    } catch (err) {
      fail(err);
    } finally {
      setSaving(false);
    }
  };

  if (loading) return <Spinner />;

  return (
    <div>
      {error !== null && <ErrorBox message={error} onDismiss={() => setError(null)} />}
      {notice !== null && (
        <p className="notice" role="status">
          {notice}
        </p>
      )}
      <p className="muted">
        Status is detected live from the environment and stored config. A “not configured” integration is shown as
        such — never as working.
      </p>

      <h3>Integrations</h3>
      {integrations.length === 0 ? (
        <EmptyState>No integrations registered.</EmptyState>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Kind</th>
              <th>Status</th>
              <th>Detail</th>
              <th>Setup</th>
            </tr>
          </thead>
          <tbody>
            {integrations.map((i) => (
              <tr key={i.id}>
                <td>{i.name}</td>
                <td>{i.kind}</td>
                <td>{i.configured ? "Configured" : "Not configured"}</td>
                <td>{i.configuredDetail}</td>
                <td>{i.setupHint}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h3>Email settings</h3>
      <p className="muted">
        When no email provider is configured, messages are recorded in the mail log below (test mode) and never
        delivered — the UI says so, and the log proves it.
      </p>
      <form onSubmit={(e) => void saveEmail(e)} className="form-grid">
        <label>
          API URL (HTTP provider endpoint; leave empty for test mode)
          <input
            value={emailApiUrl}
            onChange={(e) => setEmailApiUrl(e.target.value)}
            placeholder="https://api.resend.com/emails"
            maxLength={500}
          />
        </label>
        <label>
          Sender name
          <input value={emailFromName} onChange={(e) => setEmailFromName(e.target.value)} maxLength={200} />
        </label>
        <label>
          Sender email
          <input
            value={emailFromEmail}
            onChange={(e) => setEmailFromEmail(e.target.value)}
            type="email"
            maxLength={200}
          />
        </label>
        <button type="submit" className="btn-primary" disabled={saving}>
          Save email settings
        </button>
      </form>

      <h3>Analytics settings</h3>
      <p className="muted">
        {analytics === null
          ? ""
          : analytics.configured
            ? `Configured: ${analytics.provider} (${analytics.measurementId}). The documented snippet is injected into public pages.`
            : "Not configured — public pages carry no tracking snippet."}
      </p>
      <form onSubmit={(e) => void saveAnalytics(e)} className="form-grid">
        <label>
          Provider
          <select value={analyticsProvider} onChange={(e) => setAnalyticsProvider(e.target.value)}>
            <option value="none">None</option>
            <option value="ga4">Google Analytics 4</option>
            <option value="plausible">Plausible</option>
          </select>
        </label>
        <label>
          Measurement ID (GA4) or domain (Plausible)
          <input
            value={analyticsMeasurementId}
            onChange={(e) => setAnalyticsMeasurementId(e.target.value)}
            maxLength={100}
          />
        </label>
        <button type="submit" className="btn-primary" disabled={saving}>
          Save analytics settings
        </button>
      </form>

      <h3>Mail log</h3>
      <p className="muted">
        “logged” means recorded in test mode, not delivered. “sent” means the HTTP provider accepted the message.
      </p>
      {mail === null ? (
        <Spinner />
      ) : mail.entries.length === 0 ? (
        <EmptyState>No mail yet.</EmptyState>
      ) : (
        <>
          <table>
            <thead>
              <tr>
                <th>To</th>
                <th>Subject</th>
                <th>Status</th>
                <th>When</th>
              </tr>
            </thead>
            <tbody>
              {mail.entries.map((m) => (
                <tr key={m.id}>
                  <td>{m.toEmail}</td>
                  <td>{m.subject}</td>
                  <td>{m.status}</td>
                  <td>{m.createdAt}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <Pagination page={mail.page} totalPages={mail.totalPages} total={mail.total} onPage={setMailPage} />
        </>
      )}

      <h3>Cloudflare bindings</h3>
      <p className="muted">Read-only: what is actually bound in this environment. Nothing is invented.</p>
      <table>
        <thead>
          <tr>
            <th>Binding</th>
            <th>Present</th>
            <th>Detail</th>
          </tr>
        </thead>
        <tbody>
          {bindings.map((b) => (
            <tr key={b.binding}>
              <td>{b.binding}</td>
              <td>{b.present ? "Yes" : "No"}</td>
              <td>{b.detail}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
