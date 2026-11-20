/**
 * Import (§6.3 — grouped under Settings/System in the nav, kept as its own screen).
 *
 * WXR or SQL dump upload with a per-item report. The importer parses
 * data only — it never executes imported code. WordPress password
 * hashes are not portable, so every imported user gets a forced
 * password reset.
 */
import { useState } from "react";
import type { ImportJobResult, ImportReport } from "../lib/types";
import { Alert, Badge, Button, Card, Input, Select, Table } from "../components/elements";

const FIXTURE_BASE = "https://raw.githubusercontent.com/ola1de/Wordless-CMS/main/test/fixtures/wordpress";

export function ImportPage(): React.ReactElement {
  const [file, setFile] = useState<File | null>(null);
  const [kind, setKind] = useState<"wxr" | "sql">("wxr");
  const [job, setJob] = useState<ImportJobResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const start = async (): Promise<void> => {
    if (!file) return;
    setError(null);
    setJob(null);
    setBusy(true);
    try {
      const form = new FormData();
      form.append("file", file);
      const res = await fetch(kind === "wxr" ? "/api/import/wxr" : "/api/import/sql", {
        method: "POST",
        credentials: "same-origin",
        body: form,
      });
      const body = (await res.json()) as { ok: boolean; data?: ImportJobResult; error?: { message?: string } };
      if (!res.ok || !body.ok || !body.data) {
        throw new Error(body.error?.message ?? `Import failed (${res.status}).`);
      }
      setJob(body.data);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Import failed.");
    } finally {
      setBusy(false);
    }
  };

  const report = (job?.result !== null && job?.result !== undefined && !("error" in job.result) ? job.result : null) as ImportReport | null;
  const jobError = job?.result !== null && job?.result !== undefined && "error" in job.result ? job.result.error : null;

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Import failed" onDismiss={() => setError(null)}>{error}</Alert> : null}

      <Card title="Import a WordPress site" sub="WXR (WordPress eXtended RSS) or a mysqldump of the 12 wp_* tables, up to 50 MB. The importer parses data only — it never executes imported code. WordPress password hashes are not portable, so every imported user gets a forced password reset and cannot sign in until an administrator resets their password. PHP-serialized values become JSON; serialized objects are refused and recorded as failed.">
        <p>
          Test fixtures (synthetic, for trying the importer):{" "}
          <a href={`${FIXTURE_BASE}/wordpress-export.xml`} target="_blank" rel="noreferrer">wordpress-export.xml</a> ·{" "}
          <a href={`${FIXTURE_BASE}/wordpress-dump.sql`} target="_blank" rel="noreferrer">wordpress-dump.sql</a>
        </p>
        <div className="form-grid">
          <Select label="Format" value={kind} onChange={(e) => setKind(e.target.value as "wxr" | "sql")}>
            <option value="wxr">WXR (WordPress export XML)</option>
            <option value="sql">SQL dump (mysqldump)</option>
          </Select>
          <Input label="File (max 50 MB)" type="file" accept={kind === "wxr" ? ".xml" : ".sql"} onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
        </div>
        <Button variant="primary" icon="import" disabled={busy || !file} onClick={() => void start()}>{busy ? "Importing…" : "Start import"}</Button>
      </Card>

      {jobError !== null ? (
        <Alert tone="err" title="Import job failed">{jobError}</Alert>
      ) : null}

      {job !== null ? (
        <Card title={`Import job ${job.id}`} sub={`Status: ${job.status}`}>
          {report !== null ? (
            <>
              <p>{report.counts.imported} imported · {report.counts.skipped} skipped · {report.counts.failed} failed{report.ignoredTables !== undefined && report.ignoredTables.length > 0 ? ` · ignored tables: ${report.ignoredTables.join(", ")}` : ""}</p>
              <Table
                caption="Import counts by type"
                columns={[
                  { key: "type", label: "Type" },
                  { key: "imported", label: "Imported" },
                  { key: "skipped", label: "Skipped" },
                  { key: "failed", label: "Failed" },
                ]}
                rows={Object.entries(report.counts.byType).map(([type, counts]) => ({ type, imported: String(counts.imported), skipped: String(counts.skipped), failed: String(counts.failed) }))}
              />
              <h3>Per-item report</h3>
              <Table
                caption="Per-item import report"
                columns={[
                  { key: "type", label: "Type" },
                  { key: "source", label: "Source" },
                  { key: "status", label: "Status" },
                  { key: "reason", label: "Reason" },
                ]}
                rows={report.items.map((item) => ({
                  type: item.type,
                  source: <code>{item.sourceId}</code>,
                  status: <Badge tone={item.status === "imported" ? "teal" : item.status === "failed" ? "red" : "amber"}>{item.status}</Badge>,
                  reason: item.reason ?? "—",
                }))}
                empty={<p className="muted">No items in this import.</p>}
              />
            </>
          ) : null}
        </Card>
      ) : null}

      <Card title="What is not imported" sub="Multisite tables, plugin tables, uploads binary blobs, and serialized PHP objects. Post revisions are skipped. See docs/import/wordpress-import.md for the full list.">
        <p className="muted">If an import skips something you expected, check the per-item report above for the reason.</p>
      </Card>
    </div>
  );
}
