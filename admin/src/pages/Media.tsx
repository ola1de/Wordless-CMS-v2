/**
 * Media library (§6.3).
 *
 * Grid with drag-and-drop upload and real per-file progress (XHR
 * upload progress — the fetch-based api.uploadMedia reports no
 * progress, so uploads here use XMLHttpRequest against the same
 * /api/media endpoint with the same session cookie), search + filter
 * by type, alt-text editing, delete. The derivatives note is honest:
 * when the Cloudflare Images binding is unbound the library says the
 * original file is served and why.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { formatBytes, formatDateTime } from "../lib/format";
import type { DerivativeStatus, MediaItem, MediaList } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Modal, Select, Skeleton } from "../components/elements";
import { Icon } from "../components/icons";

/**
 * The /api/media list endpoint accepts one exact `mime` value (e.g.
 * image/jpeg) — no prefix match and no server-side search. So the
 * type filter sends exact values and search/date filter the loaded
 * page client-side; the UI says so instead of pretending the search
 * covers the whole library. A server-side search is recorded as a
 * missing-endpoint note for the API worker (see README).
 */
const MIME_OPTIONS: Array<{ value: string; label: string }> = [
  { value: "", label: "All types" },
  { value: "image/jpeg", label: "JPEG" },
  { value: "image/png", label: "PNG" },
  { value: "image/gif", label: "GIF" },
  { value: "image/webp", label: "WebP" },
  { value: "image/avif", label: "AVIF" },
  { value: "image/svg+xml", label: "SVG" },
  { value: "application/pdf", label: "PDF" },
];

interface UploadState {
  name: string;
  progress: number;
  done: boolean;
  error?: string;
}

/** Same endpoint + same-origin cookie as api.uploadMedia, but with XHR progress events. */
function uploadWithProgress(file: File, onProgress: (pct: number) => void): Promise<MediaItem> {
  return new Promise((resolve, reject) => {
    const form = new FormData();
    form.append("file", file);
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/media", true);
    xhr.withCredentials = true;
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) onProgress(Math.round((e.loaded / e.total) * 100));
    };
    xhr.onload = () => {
      try {
        const body = JSON.parse(xhr.responseText) as { ok: boolean; data?: MediaItem; error?: { message?: string } };
        if (xhr.status >= 200 && xhr.status < 300 && body.ok && body.data !== undefined) resolve(body.data);
        else reject(new Error(body.error?.message ?? `Upload failed (${xhr.status}).`));
      } catch {
        reject(new Error(`Upload failed (${xhr.status}).`));
      }
    };
    xhr.onerror = () => reject(new Error("Upload failed — the connection was interrupted."));
    xhr.send(form);
  });
}

export function MediaPage(): React.ReactElement {
  const [list, setList] = useState<MediaList | null>(null);
  const [page, setPage] = useState(1);
  const [mime, setMime] = useState("");
  const [search, setSearch] = useState("");
  const [dateFilter, setDateFilter] = useState<"all" | "today" | "week" | "month">("all");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editing, setEditing] = useState<MediaItem | null>(null);
  const [altDraft, setAltDraft] = useState("");
  const [derivatives, setDerivatives] = useState<DerivativeStatus | null>(null);
  const [uploads, setUploads] = useState<UploadState[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const load = useCallback(async () => {
    try {
      const params = new URLSearchParams({ page: String(page), per_page: "24" });
      if (mime) params.set("mime", mime);
      setList(await api.get<MediaList>(`/api/media?${params.toString()}`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load media.");
    }
  }, [page, mime]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    api
      .get<DerivativeStatus>("/api/media/derivatives")
      .then((d) => {
        if (!cancelled) setDerivatives(d);
      })
      .catch(() => {
        // Informational only — a failure never blocks the library.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const uploadFiles = async (files: File[]): Promise<void> => {
    for (const file of files) {
      setUploads((prev) => [...prev, { name: file.name, progress: 0, done: false }]);
      try {
        await uploadWithProgress(file, (pct) => {
          setUploads((prev) => prev.map((u) => (u.name === file.name && !u.done ? { ...u, progress: pct } : u)));
        });
        setUploads((prev) => prev.map((u) => (u.name === file.name ? { ...u, progress: 100, done: true } : u)));
        setNotice(`Uploaded ${file.name}.`);
      } catch (err) {
        const msg = err instanceof Error ? err.message : "Upload failed.";
        setUploads((prev) => prev.map((u) => (u.name === file.name ? { ...u, done: true, error: msg } : u)));
        setError(msg);
      }
    }
    await load();
  };

  const saveAlt = async (): Promise<void> => {
    if (!editing) return;
    try {
      await api.put<MediaItem>(`/api/media/${editing.id}`, { alt: altDraft });
      setNotice("Alt text saved.");
      setEditing(null);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save alt text.");
    }
  };

  const remove = async (item: MediaItem): Promise<void> => {
    if (!window.confirm(`Delete ${item.filename}? This removes the file and cannot be undone.`)) return;
    try {
      await api.del(`/api/media/${item.id}`);
      setNotice(`Deleted ${item.filename}.`);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed.");
    }
  };

  const visibleUploads = uploads.filter((u) => !u.done || u.error !== undefined);

  /** Client-side search + date filter over the loaded page (see header note). */
  const shownMedia = (list?.media ?? []).filter((item) => {
    if (search.trim() !== "" && !item.filename.toLowerCase().includes(search.trim().toLowerCase())) return false;
    if (dateFilter !== "all") {
      const ageMs = Date.now() - new Date(item.date).getTime();
      const limit = dateFilter === "today" ? 86400000 : dateFilter === "week" ? 7 * 86400000 : 30 * 86400000;
      if (ageMs > limit) return false;
    }
    return true;
  });

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Media error" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      {derivatives !== null ? (
        <Alert tone={derivatives.bound ? "ok" : "info"} title={derivatives.bound ? "Image derivatives available" : "Original files only"}>
          {derivatives.bound
            ? `Cloudflare Images is bound; ${derivatives.stored} variant(s) stored; widths ${derivatives.widths.join(", ")}. Use ?width=&format= on a media URL.`
            : "Image derivatives are unavailable until the Cloudflare Images binding is configured (see wrangler.toml) — media URLs serve the original file. Nothing is resized or optimised in the meantime."}
        </Alert>
      ) : null}

      <div
        className={`dropzone${dragOver ? " over" : ""}`}
        onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragOver(false);
          const files = Array.from(e.dataTransfer.files);
          if (files.length > 0) void uploadFiles(files);
        }}
      >
        <Icon name="upload" size={22} />
        <p><strong>Drag files here to upload</strong></p>
        <p className="muted">Max 50 MB each; jpeg, png, gif, webp, avif, svg, pdf. Or choose files below.</p>
        <input
          ref={fileRef}
          type="file"
          multiple
          aria-label="Choose files to upload"
          onChange={(e) => {
            const files = Array.from(e.target.files ?? []);
            if (files.length > 0) void uploadFiles(files);
            e.target.value = "";
          }}
        />
      </div>

      {visibleUploads.length > 0 ? (
        <Card title="Uploads" sub="Real upload progress — a file is only listed as done when the server confirms it.">
          {visibleUploads.map((u) => (
            <div key={u.name} className="upload-row">
              <span className="upload-name">{u.name}</span>
              {u.error !== undefined ? (
                <Badge tone="red">Failed: {u.error}</Badge>
              ) : u.done ? (
                <Badge tone="teal">Uploaded</Badge>
              ) : (
                <span className="upload-progress" role="progressbar" aria-valuenow={u.progress} aria-valuemin={0} aria-valuemax={100} aria-label={`Uploading ${u.name}`}>
                  <span style={{ width: `${u.progress}%` }} />
                  <span className="upload-pct">{u.progress}%</span>
                </span>
              )}
            </div>
          ))}
        </Card>
      ) : null}

      <div className="toolbar">
        <Select label="Type filter" value={mime} onChange={(e) => { setMime(e.target.value); setPage(1); }} aria-label="Filter by type">
          {MIME_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>{o.label}</option>
          ))}
        </Select>
        <Input
          label="Search this page"
          type="search"
          placeholder="Search by filename…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          hint="Search filters the 24 files on this page; the API has no library-wide search yet."
        />
        <Select label="Date" value={dateFilter} onChange={(e) => setDateFilter(e.target.value as "all" | "today" | "week" | "month")} aria-label="Filter by date">
          <option value="all">Any date</option>
          <option value="today">Today</option>
          <option value="week">Last 7 days</option>
          <option value="month">Last 30 days</option>
        </Select>
        <span className="spacer" />
        <Button icon="upload" onClick={() => fileRef.current?.click()}>Choose files</Button>
      </div>

      {list === null ? (
        <div className="media-grid" aria-label="Loading media">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="media-card"><div className="thumb"><span className="muted">Loading…</span></div><div className="meta"><span className="muted">Loading…</span></div></div>
          ))}
        </div>
      ) : shownMedia.length === 0 ? (
        <EmptyState icon="media" title="No media found" description={search !== "" || mime !== "" || dateFilter !== "all" ? "Try a different type, date or search term." : "Upload your first file to get started."} action={<Button variant="primary" icon="upload" onClick={() => fileRef.current?.click()}>Upload a file</Button>} />
      ) : (
        <>
          <div className="media-grid">
            {shownMedia.map((item) => (
              <article key={item.id} className="media-card">
                <div className="thumb">
                  {item.mimeType.startsWith("image/") ? (
                    <img src={item.url} alt={item.alt || item.filename} loading="lazy" />
                  ) : (
                    <span className="file-icon"><Icon name="file" size={20} />{item.mimeType}<br />{item.filename}</span>
                  )}
                </div>
                <div className="meta">
                  <div className="name">{item.filename}</div>
                  <div className="sub">{formatBytes(item.size)} · {formatDateTime(item.date)}</div>
                  {item.alt ? <div className="sub">Alt: {item.alt}</div> : <div className="sub">No alt text yet.</div>}
                </div>
                <div className="actions">
                  <Button size="small" icon="edit" onClick={() => { setEditing(item); setAltDraft(item.alt); }}>Edit alt</Button>
                  <Button size="small" variant="danger" icon="trash" onClick={() => void remove(item)}>Delete</Button>
                </div>
              </article>
            ))}
          </div>
          <div className="pagination">
            <Button size="small" icon="chevronLeft" disabled={list.page <= 1} onClick={() => setPage(list.page - 1)}>Previous</Button>
            <span>Page {list.page} of {Math.max(list.totalPages, 1)} ({list.total} total)</span>
            <Button size="small" icon="chevronRight" disabled={list.page >= list.totalPages} onClick={() => setPage(list.page + 1)}>Next</Button>
          </div>
        </>
      )}

      {editing !== null ? (
        <Modal
          title={`Alt text — ${editing.filename}`}
          onClose={() => setEditing(null)}
          actions={
            <>
              <Button onClick={() => setEditing(null)}>Cancel</Button>
              <Button variant="primary" onClick={() => void saveAlt()}>Save alt text</Button>
            </>
          }
        >
          <Input label="Alt text" id="media-alt" value={altDraft} onChange={(e) => setAltDraft(e.target.value)} maxLength={500} hint="Describe the image for screen readers and search engines." />
        </Modal>
      ) : null}
    </div>
  );
}
