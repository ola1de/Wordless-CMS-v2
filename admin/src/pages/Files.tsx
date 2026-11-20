/**
 * File manager (§6.3) — browse the R2 bucket by folder prefix,
 * upload, download, rename and delete files. Admin-only
 * (manage_options, enforced by the API routes). No shell access and
 * no in-place editing: files are downloaded to view or edit
 * elsewhere. Folders owned by other flows (extensions/, themes/,
 * backups/, imports/) are listed but read-only here — use the
 * Marketplace, Themes, Backup or Import page for those.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatBytes, formatDateTime } from "../lib/format";
import type { FileEntry, FileListResult } from "../lib/types";
import { Alert, Breadcrumbs, Button, Card, EmptyState, Input, Modal, Skeleton, Table } from "../components/elements";

function parentPrefix(prefix: string): string {
  const slash = prefix.lastIndexOf("/");
  return slash < 0 ? "" : prefix.slice(0, slash);
}

function formatModified(value: string | null): string {
  if (value === null) return "—";
  return formatDateTime(value);
}

const READ_ONLY_PREFIXES = ["extensions/", "themes/", "backups/", "imports/"];

export function FilesPage(): React.ReactElement {
  const [prefix, setPrefix] = useState("");
  const [list, setList] = useState<FileListResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [file, setFile] = useState<File | null>(null);
  const [renameTarget, setRenameTarget] = useState<FileEntry | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<FileEntry | null>(null);

  const load = useCallback(async (p: string) => {
    setLoading(true);
    setError(null);
    try {
      setList(await api.listFiles(p));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load files.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(prefix);
  }, [prefix, load]);

  const openFolder = (folder: string): void => {
    setPrefix(prefix === "" ? folder : `${prefix}/${folder}`);
  };

  const goUp = (): void => setPrefix(parentPrefix(prefix));

  const doUpload = async (): Promise<void> => {
    if (file === null) return;
    setUploading(true);
    setError(null);
    setNotice(null);
    try {
      const entry = await api.uploadFile(file, prefix);
      setNotice(`Uploaded ${entry.name} (${formatBytes(entry.size)}).`);
      setFile(null);
      await load(prefix);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Upload failed.");
    } finally {
      setUploading(false);
    }
  };

  const doRename = async (): Promise<void> => {
    if (renameTarget === null || renameValue.trim() === "") return;
    setError(null);
    try {
      const entry = await api.renameFile(renameTarget.key, renameValue.trim());
      setNotice(`Renamed to ${entry.name}.`);
      setRenameTarget(null);
      await load(prefix);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Rename failed.");
    }
  };

  const doDelete = async (): Promise<void> => {
    if (deleteTarget === null) return;
    setError(null);
    try {
      await api.deleteFile(deleteTarget.key);
      setNotice(`Deleted ${deleteTarget.name}.`);
      setDeleteTarget(null);
      await load(prefix);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed.");
    }
  };

  const crumbs = prefix === "" ? [] : prefix.split("/");
  const readOnlyHere = READ_ONLY_PREFIXES.some((p) => prefix.startsWith(p));

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Files" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <p className="hint">
        Every file stored in the site&apos;s storage: media uploads, installed extensions, themes, backups and
        imports. Uploads are capped at 50 MB (same file types as the Media library). Folders owned by other pages —{" "}
        <code>extensions/</code>, <code>themes/</code>, <code>backups/</code>, <code>imports/</code> — are read-only
        here; manage them from the Marketplace, Themes, Backup or Import page. Files are never executed.
      </p>

      <Card title="Upload to this folder" sub={readOnlyHere ? "This folder is owned by another page — uploads are disabled here." : undefined}>
        <div className="toolbar">
          <Input label="File to upload" type="file" onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
          <Button variant="primary" icon="upload" disabled={uploading || file === null || readOnlyHere} onClick={() => void doUpload()}>{uploading ? "Uploading…" : "Upload"}</Button>
        </div>
      </Card>

      <Card title="Files">
        <Breadcrumbs items={[{ label: "Storage root", href: "#/files" }, ...crumbs.map((c, i) => ({ label: c, href: i === crumbs.length - 1 ? undefined : "#/files" }))]} />
        {prefix !== "" ? <Button size="small" icon="chevronLeft" onClick={goUp}>Up one level</Button> : null}
        {loading || list === null ? (
          <Skeleton height={160} label="Loading files" />
        ) : list.folders.length === 0 && list.files.length === 0 ? (
          <EmptyState icon="folder" title="This folder is empty" />
        ) : (
          <>
            <Table
              caption={`Files in ${prefix === "" ? "storage root" : prefix}`}
              columns={[
                { key: "name", label: "Name" },
                { key: "type", label: "Type" },
                { key: "size", label: "Size" },
                { key: "modified", label: "Modified" },
                { key: "actions", label: "Actions" },
              ]}
              rows={[
                ...list.folders.map((f) => ({
                  name: <Button variant="ghost" size="small" icon="folder" onClick={() => openFolder(f)}>{f}/</Button>,
                  type: "folder",
                  size: "—",
                  modified: "—",
                  actions: <span className="muted">Open to browse</span>,
                })),
                ...list.files.map((f) => ({
                  name: <span><span className="row-title">{f.name}</span>{f.isMedia ? <span className="muted"> · media</span> : null}</span>,
                  type: f.type,
                  size: formatBytes(f.size),
                  modified: formatModified(f.modified),
                  actions: (
                    <span className="actions">
                      <Button size="small" icon="download" onClick={() => { window.location.href = `/api/files/download?key=${encodeURIComponent(f.key)}`; }}>Download</Button>
                      <Button size="small" icon="edit" disabled={readOnlyHere} onClick={() => { setRenameTarget(f); setRenameValue(f.name); }}>Rename</Button>
                      <Button size="small" variant="danger" icon="trash" disabled={readOnlyHere} onClick={() => setDeleteTarget(f)}>Delete</Button>
                    </span>
                  ),
                })),
              ]}
            />
            {list.truncated ? <p className="hint">Listing capped at 1,000 entries — this folder has more files than shown.</p> : null}
          </>
        )}
      </Card>

      {renameTarget !== null ? (
        <Modal
          title={`Rename ${renameTarget.name}`}
          onClose={() => setRenameTarget(null)}
          actions={
            <>
              <Button onClick={() => setRenameTarget(null)}>Cancel</Button>
              <Button variant="primary" onClick={() => void doRename()}>Rename</Button>
            </>
          }
        >
          <Input label="New file name" value={renameValue} onChange={(e) => setRenameValue(e.target.value)} />
        </Modal>
      ) : null}

      {deleteTarget !== null ? (
        <Modal
          title={`Delete ${deleteTarget.name}?`}
          onClose={() => setDeleteTarget(null)}
          actions={
            <>
              <Button onClick={() => setDeleteTarget(null)}>Cancel</Button>
              <Button variant="danger" onClick={() => void doDelete()}>Delete</Button>
            </>
          }
        >
          <p>This deletes the file from storage{deleteTarget.isMedia ? " and removes its Media library record" : ""}. This cannot be undone.</p>
        </Modal>
      ) : null}
    </div>
  );
}
