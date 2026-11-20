import { useCallback, useEffect, useState } from "react";
import { api, ApiError } from "../lib/api";
import type {
  ExtensionDetail,
  ExtensionLoadResult,
  ExtensionLogEntry,
  ExtensionPermissions,
  ExtensionRun,
  ExtensionRuntimeList,
  LoadedExtension,
  PluginCompatibilityReport,
  PluginSummary,
} from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Skeleton, StatCard, Table, Toggle } from "../components/elements";

type Verdict = "compatible" | "partial" | "blocked" | "not-analysed";

function verdictTone(verdict: string): "teal" | "amber" | "red" | "grey" {
  switch (verdict as Verdict) {
    case "compatible":
      return "teal";
    case "partial":
      return "amber";
    case "blocked":
      return "red";
    default:
      return "grey";
  }
}

function verdictLabel(verdict: string): string {
  switch (verdict as Verdict) {
    case "compatible":
      return "Compatible";
    case "partial":
      return "Partial";
    case "blocked":
      return "Blocked";
    case "not-analysed":
      return "Not analysed";
    default:
      return verdict;
  }
}

/** Log level → badge tone. Denied and error are both red: a denial is
 *  the sandbox refusing a call, and must stand out the same way. */
function logLevelTone(level: string): "red" | "amber" | "grey" {
  switch (level) {
    case "denied":
    case "error":
      return "red";
    case "warn":
      return "amber";
    default:
      return "grey";
  }
}

function runStatusTone(status: string): "teal" | "amber" | "red" | "grey" {
  switch (status) {
    case "ok":
      return "teal";
    case "timeout":
      return "amber";
    case "error":
    case "denied":
      return "red";
    default:
      return "grey";
  }
}

export function PluginsPage(): React.ReactElement {
  const [plugins, setPlugins] = useState<PluginSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busySlug, setBusySlug] = useState<string | null>(null);
  const [detailSlug, setDetailSlug] = useState<string | null>(null);
  const [report, setReport] = useState<PluginCompatibilityReport | null | undefined>(undefined);
  const [reportError, setReportError] = useState<string | null>(null);
  const [reportLoading, setReportLoading] = useState(false);

  /* P6 sandbox runtime state. `runtime` is the GET /api/extensions/runtime
   * list (loaded instances + installed-but-not-loaded extensions);
   * `detailInstance` is the GET /api/extensions/:slug record for the
   * open detail card (null = not loaded, undefined = still loading). */
  const [runtime, setRuntime] = useState<ExtensionRuntimeList | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [runtimeBusy, setRuntimeBusy] = useState<string | null>(null);
  const [detailInstance, setDetailInstance] = useState<ExtensionDetail | null | undefined>(undefined);
  const [detailRuntimeError, setDetailRuntimeError] = useState<string | null>(null);
  const [perms, setPerms] = useState<ExtensionPermissions | null | undefined>(undefined);
  const [permChecked, setPermChecked] = useState<string[]>([]);
  const [permSaving, setPermSaving] = useState(false);
  const [permError, setPermError] = useState<string | null>(null);
  const [logs, setLogs] = useState<ExtensionLogEntry[] | null>(null);
  const [runs, setRuns] = useState<ExtensionRun[] | null>(null);
  /** The outcome of the most recent load attempt for the open detail
   *  card — a success (warnings + dispatch hint) or a refusal (the
   *  exact reasons and blocked files). */
  const [lastLoad, setLastLoad] = useState<{ slug: string; result: ExtensionLoadResult } | null>(null);

  const loadRuntime = useCallback(async () => {
    try {
      setRuntime(await api.listExtensionRuntime());
      setRuntimeError(null);
    } catch (err) {
      setRuntimeError(err instanceof Error ? err.message : "Failed to load the sandbox runtime state.");
    }
  }, []);

  const load = useCallback(async () => {
    try {
      setPlugins(await api.listPlugins());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load plugins.");
    }
    await loadRuntime();
  }, [loadRuntime]);

  useEffect(() => {
    void load();
  }, [load]);

  const openDetail = async (slug: string): Promise<void> => {
    setDetailSlug(slug);
    setReport(undefined);
    setReportError(null);
    setReportLoading(true);
    setDetailInstance(undefined);
    setDetailRuntimeError(null);
    setPerms(undefined);
    setPermError(null);
    setLogs(null);
    setRuns(null);
    try {
      const { report: r } = await api.getMarketplaceReport(slug);
      setReport(r);
    } catch (err) {
      setReportError(err instanceof Error ? err.message : "Failed to load the compatibility report.");
    } finally {
      setReportLoading(false);
    }
    // Runtime detail: 404 means "not loaded yet", which is a normal
    // state, not an error — only other failures are shown.
    try {
      const inst = await api.getExtension(slug);
      setDetailInstance(inst);
      const [p, l, r] = await Promise.all([
        api.getExtensionPermissions(slug),
        api.listExtensionLogs(slug, 50),
        api.listExtensionRuns(slug, 50),
      ]);
      setPerms(p);
      setPermChecked(p.available.filter((c) => c.granted).map((c) => c.id));
      setLogs(l.logs);
      setRuns(r.runs);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        setDetailInstance(null);
        setPerms(null);
      } else {
        // Unknown state: stop the skeleton, show the error, and leave
        // the "not loaded" block (with its Load button) as the retry.
        setDetailInstance(null);
        setPerms(null);
        setDetailRuntimeError(err instanceof Error ? err.message : "Failed to load the sandbox runtime state.");
      }
    }
  };

  const setStatus = async (plugin: PluginSummary, activate: boolean): Promise<void> => {
    setError(null);
    setNotice(null);
    setBusySlug(plugin.slug);
    try {
      await api.setPluginStatus(plugin.slug, activate ? "activate" : "deactivate");
      setNotice(`“${plugin.name}” ${activate ? "activated" : "deactivated"}.`);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to change the plugin status.");
    } finally {
      setBusySlug(null);
    }
  };

  /** P6: load a plugin into the sandbox (POST /api/extensions/:slug/load,
   *  no body). A refusal is a normal 200 response — the exact reasons
   *  and blocked files are shown in the detail card, not swallowed. */
  const loadExtension = async (plugin: PluginSummary): Promise<void> => {
    setError(null);
    setNotice(null);
    setRuntimeBusy(plugin.slug);
    try {
      const result = await api.loadExtension(plugin.slug);
      setLastLoad({ slug: plugin.slug, result });
      if (result.loaded) {
        setNotice(
          `“${plugin.name}” is loaded in the sandbox${result.dispatchConfigured ? "" : " — but its Worker has not been published yet, so its hooks cannot run"}.`,
        );
      }
      await loadRuntime();
      await openDetail(plugin.slug);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the plugin into the sandbox.");
    } finally {
      setRuntimeBusy(null);
    }
  };

  /** P6: unload a plugin from the sandbox (POST .../unload). Its code
   *  stays in R2 and it stays installed; loading again re-runs the
   *  load gate. */
  const unloadExtension = async (plugin: PluginSummary): Promise<void> => {
    setError(null);
    setNotice(null);
    setRuntimeBusy(plugin.slug);
    try {
      await api.unloadExtension(plugin.slug);
      setLastLoad(null);
      setNotice(`“${plugin.name}” unloaded from the sandbox. It stays installed.`);
      await loadRuntime();
      await openDetail(plugin.slug);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to unload the plugin from the sandbox.");
    } finally {
      setRuntimeBusy(null);
    }
  };

  /** P6: the kill switch (POST .../disable or .../enable). Disabling
   *  stops the plugin's hooks within one request. It is NOT the same
   *  as Deactivate, which only changes the marketplace status. */
  const setRuntimeStatus = async (plugin: PluginSummary, disable: boolean): Promise<void> => {
    setError(null);
    setNotice(null);
    setRuntimeBusy(plugin.slug);
    try {
      await api.setExtensionStatus(plugin.slug, disable ? "disable" : "enable");
      setNotice(
        disable
          ? `“${plugin.name}” disabled in the sandbox — its hooks stop within one request.`
          : `“${plugin.name}” enabled in the sandbox — its hooks fire again.`,
      );
      await loadRuntime();
      await openDetail(plugin.slug);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to change the sandbox status.");
    } finally {
      setRuntimeBusy(null);
    }
  };

  /** P6: save the granted capabilities (POST .../permissions). Only
   *  ids from the server-provided `available` list are ever sent —
   *  the server rejects unknown capability ids. */
  const savePermissions = async (): Promise<void> => {
    if (detailSlug === null || perms === null || perms === undefined) return;
    setPermError(null);
    setPermSaving(true);
    try {
      const availableIds = new Set(perms.available.map((c) => c.id));
      const ids = permChecked.filter((id) => availableIds.has(id));
      const result = await api.setExtensionPermissions(detailSlug, ids);
      setPermChecked(result.permissions);
      const fresh = await api.getExtensionPermissions(detailSlug);
      setPerms(fresh);
      setPermChecked(fresh.available.filter((c) => c.granted).map((c) => c.id));
      setNotice(`Permissions saved for “${detailSlug}”.`);
      await loadRuntime();
    } catch (err) {
      setPermError(err instanceof Error ? err.message : "Failed to save permissions.");
    } finally {
      setPermSaving(false);
    }
  };

  if (plugins === null) {
    return (
      <div>
        {error ? <Alert tone="err" onDismiss={() => setError(null)}>{error}</Alert> : null}
        <Skeleton label="Loading plugins" height={180} />
      </div>
    );
  }

  const activeCount = plugins.filter((p) => p.active).length;
  const blockedCount = plugins.filter((p) => p.verdict === "blocked").length;
  const loadedCount = runtime !== null ? runtime.loaded.length : 0;
  const loadedBySlug = new Map<string, LoadedExtension>(runtime !== null ? runtime.loaded.map((i) => [i.slug, i]) : []);
  const detail = detailSlug !== null ? plugins.find((p) => p.slug === detailSlug) : undefined;
  const blockedFiles = report ? report.files.filter((f) => f.verdict === "blocked") : [];
  const detailLoad = lastLoad !== null && lastLoad.slug === detailSlug ? lastLoad.result : null;

  return (
    <div>
      {error ? <Alert tone="err" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}
      {runtimeError ? <Alert tone="warn" onDismiss={() => setRuntimeError(null)}>{runtimeError}</Alert> : null}
      <p className="muted" style={{ marginTop: 0 }}>
        {plugins.length} installed · {activeCount} active · {loadedCount} loaded in the sandbox · {blockedCount} blocked.
        Installed, Active and Loaded are three different things. <strong>Installed</strong> means the plugin&apos;s
        converted code is on the site. <strong>Active</strong> is the marketplace status — it records that the plugin
        is switched on. <strong>Loaded in the sandbox</strong> means the converted code is actually running as a
        separate Worker: its hooks fire, and every call it makes to WordPress is checked against the capabilities you
        grant it. A plugin with blocked files cannot be activated or loaded — the report below says exactly why.
      </p>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(140px, 1fr))", gap: 12, marginBottom: 16 }}>
        <StatCard label="Installed" value={String(plugins.length)} icon="package" />
        <StatCard label="Active" value={String(activeCount)} icon="checkCircle" />
        <StatCard label="Loaded in sandbox" value={runtime === null ? "—" : String(loadedCount)} icon="play" />
        <StatCard label="Blocked" value={String(blockedCount)} icon="alert" tone={blockedCount > 0 ? "danger" : undefined} />
      </div>

      {plugins.length === 0 ? (
        <EmptyState
          icon="package"
          title="No plugins installed"
          description="Install a plugin from the Marketplace first — it will appear here with its compatibility verdict."
          action={
            <a href="#/commerce">
              <Button variant="primary">Go to Commerce / Marketplace</Button>
            </a>
          }
        />
      ) : (
        <Card title="Installed plugins">
          <Table
            caption="Installed plugins"
            columns={[
              { key: "plugin", label: "Plugin" },
              { key: "version", label: "Version" },
              { key: "status", label: "Status" },
              { key: "verdict", label: "Compatibility" },
              { key: "sandbox", label: "Sandbox" },
              { key: "files", label: "Files" },
              { key: "actions", label: "Actions" },
            ]}
            rows={plugins.map((p) => {
              const inst = loadedBySlug.get(p.slug);
              return {
                plugin: (
                  <span>
                    <strong>{p.name}</strong>
                    <br />
                    <span className="muted">{p.slug} · {p.kind}</span>
                  </span>
                ),
                version: p.version,
                status: <Badge tone={p.active ? "teal" : "grey"}>{p.active ? "Active" : "Inactive"}</Badge>,
                verdict: <Badge tone={verdictTone(p.verdict)}>{verdictLabel(p.verdict)}</Badge>,
                sandbox:
                  inst === undefined ? (
                    <Badge tone="grey">Not loaded</Badge>
                  ) : (
                    <span>
                      <Badge tone={inst.status === "disabled" ? "amber" : "teal"}>
                        {inst.status === "disabled" ? "Disabled" : "Loaded"}
                      </Badge>
                      {inst.permissions.length > 0 ? (
                        <span>
                          <br />
                          <span className="muted">{inst.permissions.join(", ")}</span>
                        </span>
                      ) : (
                        <span>
                          <br />
                          <span className="muted">no permissions granted</span>
                        </span>
                      )}
                    </span>
                  ),
                files:
                  p.phpFiles > 0
                    ? `${p.converted} converted · ${p.partial} partial · ${p.blocked} blocked of ${p.phpFiles}`
                    : "Not analysed",
                actions: (
                  <span style={{ display: "inline-flex", gap: 6, flexWrap: "wrap" }}>
                    <Button variant="secondary" size="small" onClick={() => void openDetail(p.slug)}>
                      Details
                    </Button>
                    {p.active ? (
                      <Button variant="ghost" size="small" disabled={busySlug === p.slug} onClick={() => void setStatus(p, false)}>
                        Deactivate
                      </Button>
                    ) : (
                      <Button variant="secondary" size="small" disabled={busySlug === p.slug || p.verdict === "blocked"} onClick={() => void setStatus(p, true)}>
                        Activate
                      </Button>
                    )}
                    {inst === undefined ? (
                      <Button
                        variant="primary"
                        size="small"
                        disabled={runtimeBusy === p.slug || p.verdict === "blocked"}
                        onClick={() => void loadExtension(p)}
                      >
                        Load in sandbox
                      </Button>
                    ) : (
                      <>
                        {inst.status === "disabled" ? (
                          <Button variant="secondary" size="small" disabled={runtimeBusy === p.slug} onClick={() => void setRuntimeStatus(p, false)}>
                            Enable
                          </Button>
                        ) : (
                          <Button variant="ghost" size="small" disabled={runtimeBusy === p.slug} onClick={() => void setRuntimeStatus(p, true)}>
                            Disable
                          </Button>
                        )}
                        <Button variant="ghost" size="small" disabled={runtimeBusy === p.slug} onClick={() => void unloadExtension(p)}>
                          Unload
                        </Button>
                      </>
                    )}
                  </span>
                ),
              };
            })}
          />
          <p className="hint">
            A blocked plugin cannot be activated or loaded from here — the report below says why. Disabling a loaded
            plugin stops its hooks within one request; it is not the same as Deactivate, which only changes the
            marketplace status. Installing new plugins happens in Commerce → Marketplace.
          </p>
        </Card>
      )}

      {detail ? (
        <Card
          title={`${detail.name} — compatibility report`}
          sub={`${detail.slug} · version ${detail.version} · ${verdictLabel(detail.verdict)}`}
          actions={
            <Button variant="ghost" size="small" onClick={() => setDetailSlug(null)}>
              Close
            </Button>
          }
        >
          <h4>Sandbox runtime</h4>
          {detailRuntimeError ? (
            <Alert tone="err" onDismiss={() => setDetailRuntimeError(null)}>{detailRuntimeError}</Alert>
          ) : detailInstance === undefined ? (
            <Skeleton label="Loading sandbox runtime state" height={80} />
          ) : detailInstance === null ? (
            <div>
              <p className="muted">
                Not loaded in the sandbox yet — its converted code is installed but its hooks do not run.
              </p>
              <Button
                variant="primary"
                size="small"
                disabled={runtimeBusy === detail.slug || detail.verdict === "blocked"}
                onClick={() => void loadExtension(detail)}
              >
                Load in sandbox
              </Button>
              {detail.verdict === "blocked" ? (
                <p className="hint">Blocked plugins cannot load — the compatibility report below says why.</p>
              ) : null}
            </div>
          ) : (
            <div>
              <p>
                <Badge tone={detailInstance.status === "disabled" ? "amber" : "teal"}>
                  {detailInstance.status === "disabled" ? "Disabled in sandbox" : "Loaded in sandbox"}
                </Badge>{" "}
                <span className="muted">
                  token epoch {detailInstance.tokenEpoch} · script <code>{detailInstance.scriptKey}</code>
                </span>
              </p>
              <p>
                Granted permissions:{" "}
                {detailInstance.permissions.length === 0 ? (
                  <span className="muted">none</span>
                ) : (
                  detailInstance.permissions.map((perm) => <code key={perm}>{perm} </code>)
                )}
              </p>
              <span style={{ display: "inline-flex", gap: 6, flexWrap: "wrap" }}>
                {detailInstance.status === "disabled" ? (
                  <Button variant="secondary" size="small" disabled={runtimeBusy === detail.slug} onClick={() => void setRuntimeStatus(detail, false)}>
                    Enable
                  </Button>
                ) : (
                  <Button variant="ghost" size="small" disabled={runtimeBusy === detail.slug} onClick={() => void setRuntimeStatus(detail, true)}>
                    Disable
                  </Button>
                )}
                <Button variant="ghost" size="small" disabled={runtimeBusy === detail.slug} onClick={() => void unloadExtension(detail)}>
                  Unload
                </Button>
              </span>
              <p className="hint">
                Disable is the kill switch: a disabled plugin&apos;s hooks stop within one request. Unload removes it
                from the sandbox entirely — it stays installed, and loading it again re-runs the load gate.
              </p>
            </div>
          )}

          {detailLoad !== null ? (
            detailLoad.loaded ? (
              <div>
                <Alert tone="ok">Loaded in the sandbox.</Alert>
                {detailLoad.warnings.length > 0 ? (
                  <div>
                    <h4>Load warnings</h4>
                    <ul>
                      {detailLoad.warnings.map((w) => (
                        <li key={w}>{w}</li>
                      ))}
                    </ul>
                  </div>
                ) : null}
                {detailLoad.dispatchConfigured ? null : (
                  <Alert tone="warn">
                    The extension Worker has not been published yet, so this plugin&apos;s hooks cannot run. From your
                    own machine, run <code>scripts/publish-extensions.mjs</code> to publish it — publishing is not
                    done from the admin.
                  </Alert>
                )}
              </div>
            ) : (
              <Alert tone="err" title="The load was refused">
                <ul>
                  {detailLoad.reasons.map((reason) => (
                    <li key={reason}>{reason}</li>
                  ))}
                </ul>
                {detailLoad.blockedFiles.length > 0 ? (
                  <div>
                    <h4>Blocked files</h4>
                    <ul>
                      {detailLoad.blockedFiles.map((f) => (
                        <li key={f.path}>
                          <code>{f.path}</code>
                          {f.reasons.length > 0 ? ` — ${f.reasons.join("; ")}` : ""}
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}
              </Alert>
            )
          ) : null}

          {detailInstance !== null && detailInstance !== undefined ? (
            <div>
              <h4>Permissions</h4>
              {permError ? <Alert tone="err" onDismiss={() => setPermError(null)}>{permError}</Alert> : null}
              {perms === undefined ? (
                <Skeleton label="Loading permissions" height={80} />
              ) : perms === null ? (
                <p className="muted">Permissions could not be loaded.</p>
              ) : (
                <div>
                  <p className="muted" style={{ marginTop: 0 }}>
                    What this plugin is allowed to do. The list comes from the server — only the capabilities shown
                    here exist, and only checked ones are sent when saving.
                  </p>
                  <ul style={{ listStyle: "none", padding: 0, margin: 0, display: "grid", gap: 12 }}>
                    {perms.available.map((cap) => (
                      <li key={cap.id}>
                        <Toggle
                          label={cap.id}
                          checked={permChecked.includes(cap.id)}
                          disabled={permSaving}
                          onChange={(checked) =>
                            setPermChecked((prev) =>
                              checked ? [...prev, cap.id] : prev.filter((id) => id !== cap.id),
                            )
                          }
                        />
                        <div className="muted" style={{ marginLeft: 44 }}>
                          {cap.description}
                          {cap.grantedByDefault ? " (granted on every load)" : ""}
                        </div>
                      </li>
                    ))}
                  </ul>
                  <div style={{ marginTop: 12 }}>
                    <Button variant="primary" size="small" disabled={permSaving} onClick={() => void savePermissions()}>
                      Save permissions
                    </Button>
                  </div>
                </div>
              )}

              <h4>Recent logs</h4>
              {logs === null ? (
                <Skeleton label="Loading logs" height={80} />
              ) : logs.length === 0 ? (
                <p className="muted">No log lines yet.</p>
              ) : (
                <Table
                  caption="Recent extension logs, most recent first"
                  columns={[
                    { key: "level", label: "Level" },
                    { key: "message", label: "Message" },
                    { key: "time", label: "Time" },
                  ]}
                  rows={logs.map((entry) => ({
                    level: <Badge tone={logLevelTone(entry.level)}>{entry.level}</Badge>,
                    message: entry.message,
                    time: new Date(entry.created_at).toLocaleString(),
                  }))}
                />
              )}

              <h4>Recent runs</h4>
              {runs === null ? (
                <Skeleton label="Loading runs" height={80} />
              ) : runs.length === 0 ? (
                <p className="muted">No hook runs recorded yet.</p>
              ) : (
                <Table
                  caption="Recent hook runs, most recent first"
                  columns={[
                    { key: "hook", label: "Hook" },
                    { key: "status", label: "Status" },
                    { key: "duration", label: "Duration" },
                    { key: "result", label: "Result" },
                    { key: "time", label: "Time" },
                  ]}
                  rows={runs.map((run) => ({
                    hook: <code>{run.hook}</code>,
                    status: <Badge tone={runStatusTone(run.status)}>{run.status}</Badge>,
                    duration: `${run.duration_ms} ms`,
                    result: run.error ?? run.output_summary ?? "—",
                    time: new Date(run.created_at).toLocaleString(),
                  }))}
                />
              )}
            </div>
          ) : null}

          <h4>Compatibility report</h4>
          {reportLoading ? (
            <Skeleton label="Loading compatibility report" height={120} />
          ) : reportError ? (
            <Alert tone="err" onDismiss={() => setReportError(null)}>{reportError}</Alert>
          ) : report === null ? (
            <p className="muted">This plugin has never been analysed — no compatibility report exists yet.</p>
          ) : report === undefined ? (
            <Skeleton label="Loading compatibility report" height={120} />
          ) : (
            <div>
              <p>
                <Badge tone={verdictTone(report.verdict)}>{verdictLabel(report.verdict)}</Badge>{" "}
                <span className="muted">
                  {report.convertible} convertible · {report.partial} partial · {report.blocked} blocked ·{" "}
                  {report.unparsed} unparsed · {report.skipped} skipped, of {report.phpFiles} PHP files
                  ({report.totalFiles} files total). Analysed{" "}
                  {report.analysedAt ? new Date(report.analysedAt).toLocaleString() : "at an unknown time"}.
                </span>
              </p>

              {report.blocked > 0 ? (
                <div>
                  <h4>Why this plugin cannot run</h4>
                  {report.topReasons.length === 0 ? (
                    <p className="muted">No blocking reasons were recorded.</p>
                  ) : (
                    <ul>
                      {report.topReasons.map((reason) => (
                        <li key={reason.kind}>
                          <strong>{reason.kind}</strong> — {reason.count} file{reason.count === 1 ? "" : "s"}
                          {reason.files.length > 0 ? (
                            <ul>
                              {reason.files.map((f) => (
                                <li key={f}>
                                  <code>{f}</code>
                                </li>
                              ))}
                            </ul>
                          ) : null}
                        </li>
                      ))}
                    </ul>
                  )}
                  {blockedFiles.length > 0 ? (
                    <div>
                      <h4>Blocked files</h4>
                      <ul>
                        {blockedFiles.map((f) => (
                          <li key={f.file}>
                            <code>{f.file}</code>
                            {f.reasons.length > 0 ? ` — ${f.reasons.join("; ")}` : ""}
                          </li>
                        ))}
                      </ul>
                      {report.filesTruncated ? (
                        <p className="muted">The per-file list is truncated — more files were analysed than shown.</p>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              ) : (
                <p className="muted">No blocked files — nothing stops this plugin from converting.</p>
              )}

              {report.conversion ? (
                <div>
                  <h4>Settings carried over from the original plugin</h4>
                  {report.conversion.settings.carriedOver.length === 0 ? (
                    <p className="muted">No settings were carried over.</p>
                  ) : (
                    <ul>
                      {report.conversion.settings.carriedOver.map((s) => (
                        <li key={s.key}>
                          <code>{s.key}</code> = <code>{String(s.value)}</code>{" "}
                          <span className="muted">(from {s.source})</span>
                        </li>
                      ))}
                    </ul>
                  )}
                  {report.conversion.settings.skipped.length > 0 ? (
                    <div>
                      <h4>Settings not carried over</h4>
                      <ul>
                        {report.conversion.settings.skipped.map((s) => (
                          <li key={s.key}>
                            <code>{s.key}</code>
                            {s.reason ? ` — ${s.reason}` : ""}
                          </li>
                        ))}
                      </ul>
                    </div>
                  ) : null}
                  <Alert tone="info">
                    Per-plugin settings cannot be edited here yet: the API has no settings endpoint
                    (<code>GET/PUT /api/plugins/:slug/settings</code> does not exist — no settings store exists
                    server-side). The values above are the read-only defaults the conversion carried over.
                  </Alert>
                </div>
              ) : null}
            </div>
          )}
        </Card>
      ) : null}
    </div>
  );
}
