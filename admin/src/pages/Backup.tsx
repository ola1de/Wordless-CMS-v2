/**
 * Backup (§6.3) — a timeline with verify badges.
 *
 * Backups are real ZIPs in R2 (backups/<id>.zip): one JSON file per
 * database table, a media manifest, and a SHA-256 checksum per file.
 * A backup is only "verified" after its checksums are re-read and
 * recomputed. Restore replaces the database — it needs the typed
 * confirmation RESTORE and refuses a backup that failed verification.
 * The 10 most recent backups are kept.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { BackupJobResult, BackupRow, BackupVerifyReport } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Modal, Skeleton, Table } from "../components/elements";

export function BackupPage(): React.ReactElement {
  const [backups, setBackups] = useState<BackupRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [verifyReport, setVerifyReport] = useState<BackupVerifyReport | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<BackupRow | null>(null);
  const [restoreConfirmation, setRestoreConfirmation] = useState("");

  const load = useCallback(async () => {
    try {
      setBackups(await api.get<BackupRow[]>("/api/backup"));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load backups.");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const create = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const job = await api.post<BackupJobResult>("/api/backup/create", {});
      if (job.status === "done" && job.result !== null && !("error" in job.result)) {
        setNotice(`Backup created: ${job.result.id} (${formatBytes(job.result.size)}). Verify it before relying on it.`);
      } else {
        setError(job.result !== null && "error" in job.result ? job.result.error : "Backup job did not complete.");
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Backup failed.");
    } finally {
      setBusy(false);
    }
  };

  const verify = async (backup: BackupRow): Promise<void> => {
    setError(null);
    setNotice(null);
    try {
      const report = await api.post<BackupVerifyReport>(`/api/backup/${backup.id}/verify`, {});
      setVerifyReport(report);
      if (report.ok) {
        setNotice(`Backup ${backup.id} verified: every file checksum matches.`);
      } else {
        setError(`Backup ${backup.id} FAILED verification — do not restore it.`);
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Verification failed.");
    }
  };

  const download = (backup: BackupRow): void => {
    window.location.href = `/api/backup/${backup.id}/download`;
  };

  const restore = async (): Promise<void> => {
    if (!restoreTarget) return;
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const report = await api.post<{
        backupId: string;
        restoredTables: string[];
        rowsRestored: number;
        missingMedia: string[];
        mediaChecked: number;
      }>(`/api/backup/${restoreTarget.id}/restore`, { confirmation: restoreConfirmation });
      setNotice(
        `Restored ${report.rowsRestored} rows across ${report.restoredTables.length} tables.` +
          (report.missingMedia.length > 0
            ? ` Missing media keys (not restored): ${report.missingMedia.join(", ")}.`
            : " No media keys were missing."),
      );
      setRestoreTarget(null);
      setRestoreConfirmation("");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Restore failed.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Backup" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Card title="Backups" sub="Real ZIPs in R2 (backups/<id>.zip): one JSON file per database table, a media manifest, and a SHA-256 checksum per file. The 10 most recent backups are kept." actions={<Button variant="primary" icon="backup" disabled={busy} onClick={() => void create()}>{busy ? "Creating backup…" : "Create backup now"}</Button>}>
        {backups === null ? (
          <Skeleton height={160} label="Loading backups" />
        ) : backups.length === 0 ? (
          <EmptyState icon="backup" title="No backups yet" description="Create one above — a site without a backup has no way back." action={<Button variant="primary" icon="backup" disabled={busy} onClick={() => void create()}>Create backup now</Button>} />
        ) : (
          <ol className="backup-timeline">
            {backups.map((b) => (
              <li key={b.id} className="backup-row">
                <span className={`backup-dot${b.verified ? " verified" : ""}`} aria-hidden="true" />
                <span className="action-body">
                  <strong>{b.filename}</strong> <Badge tone={b.verified ? "teal" : "amber"}>{b.verified ? `Verified${b.verifiedAt ? ` · ${b.verifiedAt}` : ""}` : "Not verified"}</Badge>
                  <span className="muted">{b.createdAt} · {formatBytes(b.size)} · {Object.keys(b.tables).length} tables · {b.mediaCount} media files{b.mediaMissing > 0 ? ` · ${b.mediaMissing} media file(s) missing from storage` : ""}</span>
                </span>
                <span className="actions">
                  <Button size="small" icon="checkCircle" onClick={() => void verify(b)}>Verify</Button>
                  <Button size="small" icon="download" onClick={() => download(b)}>Download</Button>
                  <Button size="small" variant="danger" disabled={!b.verified} onClick={() => setRestoreTarget(b)}>Restore</Button>
                </span>
              </li>
            ))}
          </ol>
        )}
        <p className="hint">Restore is only available for backups that have passed verification. A backup is only “verified” after its checksums are re-read and recomputed — never on trust.</p>
      </Card>

      {verifyReport !== null ? (
        <Card title={`Verification: ${verifyReport.backupId}`} sub={`Verified ${verifyReport.verifiedAt}`} actions={<Button size="small" onClick={() => setVerifyReport(null)}>Close</Button>}>
          {verifyReport.ok ? (
            <Alert tone="ok" title="All files match their checksums">This backup is safe to restore.</Alert>
          ) : (
            <Alert tone="err" title="Checksum mismatch — this backup must not be restored">At least one file does not match the checksum recorded when the backup was made.</Alert>
          )}
          <Table
            caption="Verification files"
            columns={[
              { key: "file", label: "File" },
              { key: "status", label: "Status" },
            ]}
            rows={verifyReport.files.map((f) => ({
              file: <code>{f.file}</code>,
              status: f.ok ? <Badge tone="teal">ok</Badge> : <Badge tone="red">mismatch</Badge>,
            }))}
          />
        </Card>
      ) : null}

      {restoreTarget !== null ? (
        <Modal
          title={`Restore backup ${restoreTarget.id}`}
          onClose={() => { setRestoreTarget(null); setRestoreConfirmation(""); }}
          actions={
            <>
              <Button onClick={() => { setRestoreTarget(null); setRestoreConfirmation(""); }}>Cancel</Button>
              <Button variant="danger" disabled={busy || restoreConfirmation !== "RESTORE"} onClick={() => void restore()}>Restore this backup</Button>
            </>
          }
        >
          <p>This replaces the current database with the backup from {restoreTarget.createdAt}. Media files are checked against storage and reported if missing — they are not copied. This cannot be undone.</p>
          <Input label="Type RESTORE to confirm" id="restore-confirmation" value={restoreConfirmation} onChange={(e) => setRestoreConfirmation(e.target.value)} placeholder="RESTORE" />
        </Modal>
      ) : null}
    </div>
  );
}
