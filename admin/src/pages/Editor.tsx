import { useEffect, useRef, useState } from "react";
import { api, ApiError } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { postUrl } from "../lib/permalinks";
import type { MediaItem, MediaList, Post, PostSeo, Revision, RevisionDiff, SeoAnalysis, Term } from "../lib/types";
import { emptyParagraph, newBlockId, parseBlocks, readingTimeMinutes, serializeBlocks, wordCountOfBlocks } from "../editor/blocks";
import type { Block } from "../editor/blocks";
import { Alert, Badge, Button, Card, EmptyState, Input, Modal, Select, Skeleton, Table, Toggle } from "../components/elements";
import { Icon } from "../components/icons";

export interface EditorSettings {
  timezone: string;
  permalinkStructure: string;
}

const ADDABLE: Array<{ type: Block["type"]; label: string }> = [
  { type: "paragraph", label: "Paragraph" },
  { type: "heading", label: "Heading" },
  { type: "list", label: "List" },
  { type: "quote", label: "Quote" },
  { type: "image", label: "Image" },
  { type: "code", label: "Code" },
  { type: "separator", label: "Separator" },
];

function blankBlock(type: Block["type"]): Block {
  switch (type) {
    case "heading":
      return { id: newBlockId(), type, level: 2, html: "" };
    case "list":
      return { id: newBlockId(), type, ordered: false, items: [""] };
    case "quote":
      return { id: newBlockId(), type, html: "" };
    case "image":
      return { id: newBlockId(), type, attachmentId: 0, url: "", alt: "" };
    case "code":
      return { id: newBlockId(), type, code: "" };
    case "separator":
      return { id: newBlockId(), type };
    case "raw":
      return { id: newBlockId(), type, raw: "" };
    case "paragraph":
    default:
      return emptyParagraph();
  }
}

/** "YYYY-MM-DDTHH:mm" in the browser's local time, for datetime-local inputs. */
function toLocalInputValue(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const pad = (n: number): string => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** A datetime-local value (local time) → ISO-8601 UTC, or null when invalid. */
function fromLocalInputValue(value: string): string | null {
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

/** Media picker modal: pick an existing image or upload a new one. */
function MediaPicker({
  title,
  onSelect,
  onClose,
}: {
  title: string;
  onSelect: (item: MediaItem) => void;
  onClose: () => void;
}): React.ReactElement {
  const [list, setList] = useState<MediaList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);

  const load = async (): Promise<void> => {
    try {
      setList(await api.get<MediaList>("/api/media?per_page=24"));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load media.");
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const upload = async (file: File): Promise<void> => {
    setUploading(true);
    setError(null);
    try {
      const item = await api.uploadMedia(file);
      await load();
      onSelect(item);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Upload failed.");
    } finally {
      setUploading(false);
    }
  };

  return (
    <Modal title={title} onClose={onClose}>
      {error ? <Alert tone="err" onDismiss={() => setError(null)}>{error}</Alert> : null}
      <Input
        label="Upload a new image"
        type="file"
        accept="image/*,.pdf"
        disabled={uploading}
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (file) void upload(file);
        }}
      />
      {uploading ? <p className="muted">Uploading…</p> : null}
      {!list ? (
        <Skeleton label="Loading media" />
      ) : list.media.length === 0 ? (
        <EmptyState icon="image" title="No media yet" description="Upload an image above." />
      ) : (
        <div className="media-grid">
          {list.media
            .filter((m) => m.mimeType.startsWith("image/"))
            .map((item) => (
              <button key={item.id} type="button" className="media-card" onClick={() => onSelect(item)} style={{ padding: 0, textAlign: "left" }}>
                <span className="thumb">
                  <img src={item.url} alt={item.alt || item.filename} />
                </span>
                <span className="meta">
                  <span className="name">{item.filename}</span>
                </span>
              </button>
            ))}
        </div>
      )}
      <div className="actions">
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
      </div>
    </Modal>
  );
}

type AutosaveState =
  | { kind: "idle" }
  | { kind: "dirty" }
  | { kind: "saving" }
  | { kind: "saved"; at: string }
  | { kind: "failed"; message: string };

export function EditorPage({
  type,
  postId,
  settings,
  onDone,
}: {
  type: "post" | "page";
  postId?: number;
  settings: EditorSettings;
  onDone: () => void;
}): React.ReactElement {
  const [post, setPost] = useState<Post | null>(null);
  const [title, setTitle] = useState("");
  const [slug, setSlug] = useState("");
  const [status, setStatus] = useState("draft");
  const [excerpt, setExcerpt] = useState("");
  const [blocks, setBlocks] = useState<Block[]>([emptyParagraph()]);
  const [loading, setLoading] = useState(postId !== undefined);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [pickerFor, setPickerFor] = useState<string | null>(null);
  const [revisions, setRevisions] = useState<Revision[] | null>(null);
  const [autosave, setAutosave] = useState<AutosaveState>({ kind: "idle" });
  const [addType, setAddType] = useState<Block["type"]>("paragraph");
  const [seo, setSeo] = useState<PostSeo | null>(null);
  const [seoDraft, setSeoDraft] = useState<PostSeo | null>(null);
  const [analysis, setAnalysis] = useState<SeoAnalysis | null>(null);
  const [seoSaving, setSeoSaving] = useState(false);
  const [distractionFree, setDistractionFree] = useState(false);
  // Categories / tags (P4 §0.1 A).
  const [allCategories, setAllCategories] = useState<Term[]>([]);
  const [allTags, setAllTags] = useState<Term[]>([]);
  const [termsError, setTermsError] = useState<string | null>(null);
  const [selectedCategoryIds, setSelectedCategoryIds] = useState<number[]>([]);
  const [selectedTagIds, setSelectedTagIds] = useState<number[]>([]);
  const [newCategoryName, setNewCategoryName] = useState("");
  const [tagInput, setTagInput] = useState("");
  const [tagSaving, setTagSaving] = useState(false);
  // Featured image (P4 §0.1 A). There is NO API endpoint to read or write
  // `_thumbnail_id` (see README blocker): the pick lives in state only,
  // and saving warns honestly instead of pretending it persisted.
  const [featuredImage, setFeaturedImage] = useState<MediaItem | null>(null);
  // Scheduling (P4): `publishAt` is an ISO-8601 UTC string from the API,
  // or null when nothing is scheduled. The API stores it as
  // `_wl_publish_at` postmeta on a draft; a due post flips to publish
  // on the next single-post read and on the 5-minute cron sweep.
  const [publishAt, setPublishAt] = useState<string | null>(null);
  const [scheduleInput, setScheduleInput] = useState("");
  // Slash-command insertion + drag reorder + revision diff.
  const [slash, setSlash] = useState<{ blockId: string; query: string } | null>(null);
  const [draggedId, setDraggedId] = useState<string | null>(null);
  const [dropTargetId, setDropTargetId] = useState<string | null>(null);
  const [diff, setDiff] = useState<{ fromId: number; toId: number; diff: RevisionDiff } | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const dirtyRef = useRef(false);
  const stateRef = useRef({ title, slug, status, excerpt, blocks, post });
  stateRef.current = { title, slug, status, excerpt, blocks, post };

  const markDirty = (): void => {
    dirtyRef.current = true;
    setAutosave((prev) => (prev.kind === "saved" || prev.kind === "idle" ? { kind: "dirty" } : prev));
  };

  useEffect(() => {
    let cancelled = false;
    // Term lists are needed for new posts too (categories/tags panels).
    void (async () => {
      try {
        const [cats, tags] = await Promise.all([
          api.listTerms("category"),
          api.listTerms("post_tag"),
        ]);
        if (cancelled) return;
        setAllCategories(cats.terms);
        setAllTags(tags.terms);
      } catch (err) {
        if (!cancelled) setTermsError(err instanceof Error ? err.message : "Failed to load categories and tags.");
      }
    })();
    if (postId === undefined) return;
    (async () => {
      try {
        const loaded = await api.get<Post>(`/api/posts/${postId}`);
        if (cancelled) return;
        setPost(loaded);
        setTitle(loaded.title);
        setSlug(loaded.slug);
        setStatus(loaded.status);
        setExcerpt(loaded.excerpt ?? "");
        setPublishAt(loaded.publishAt ?? null);
        setScheduleInput(loaded.publishAt ? toLocalInputValue(loaded.publishAt) : "");
        const parsed = parseBlocks(loaded.content || "");
        setBlocks(parsed.length > 0 ? parsed : [emptyParagraph()]);
        const [cats, tags] = await Promise.all([
          api.getPostTerms(postId, "category"),
          api.getPostTerms(postId, "post_tag"),
        ]);
        if (cancelled) return;
        setSelectedCategoryIds(cats.map((t) => t.termTaxonomyId));
        setSelectedTagIds(tags.map((t) => t.termTaxonomyId));
        setRevisions(await api.get<Revision[]>(`/api/posts/${postId}/revisions`));
        const seoFields = await api.getPostSeo(postId);
        if (cancelled) return;
        setSeo(seoFields);
        setSeoDraft(seoFields);
        setAnalysis(await api.getPostSeoAnalysis(postId));
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load the content.");
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [postId]);

  // Autosave: 20 s after the last edit, for existing posts. The status is
  // always truthful: dirty → saving → saved-with-time, or failed with a
  // message (never a silent failure).
  useEffect(() => {
    if (postId === undefined) return;
    const timer = window.setTimeout(() => {
      if (!dirtyRef.current) return;
      const s = stateRef.current;
      if (!s.post) return;
      setAutosave({ kind: "saving" });
      void api
        .post<Revision>(`/api/posts/${s.post.id}/autosave`, {
          title: s.title,
          content: serializeBlocks(s.blocks),
        })
        .then(async () => {
          dirtyRef.current = false;
          setAutosave({ kind: "saved", at: new Date().toLocaleTimeString() });
          setRevisions(await api.get<Revision[]>(`/api/posts/${s.post?.id}/revisions`));
        })
        .catch(() => {
          setAutosave({ kind: "failed", message: "Autosave failed — your latest edits are not saved yet. Save manually." });
        });
    }, 20000);
    return () => window.clearTimeout(timer);
  }, [blocks, title, excerpt, postId]);

  const updateBlock = (id: string, patch: Partial<Block>): void => {
    markDirty();
    setBlocks((prev) => prev.map((b) => (b.id === id ? { ...b, ...patch } : b)));
  };

  const replaceBlock = (id: string, type: Block["type"]): void => {
    markDirty();
    setSlash(null);
    setBlocks((prev) => prev.map((b) => (b.id === id ? { ...blankBlock(type), id } : b)));
  };

  const moveBlock = (id: string, dir: -1 | 1): void => {
    markDirty();
    setBlocks((prev) => {
      const idx = prev.findIndex((b) => b.id === id);
      const to = idx + dir;
      if (idx < 0 || to < 0 || to >= prev.length) return prev;
      const next = [...prev];
      const [moved] = next.splice(idx, 1);
      if (moved) next.splice(to, 0, moved);
      return next;
    });
  };

  const moveBlockTo = (fromId: string, toId: string): void => {
    if (fromId === toId) return;
    markDirty();
    setBlocks((prev) => {
      const from = prev.findIndex((b) => b.id === fromId);
      const to = prev.findIndex((b) => b.id === toId);
      if (from < 0 || to < 0) return prev;
      const next = [...prev];
      const [moved] = next.splice(from, 1);
      if (!moved) return prev;
      next.splice(to, 0, moved);
      return next;
    });
  };

  const deleteBlock = (id: string): void => {
    markDirty();
    setBlocks((prev) => (prev.length <= 1 ? prev : prev.filter((b) => b.id !== id)));
  };

  const addBlock = (): void => {
    markDirty();
    setBlocks((prev) => [...prev, blankBlock(addType)]);
  };

  const pickImage = (item: MediaItem): void => {
    if (pickerFor === "__og__") {
      setSeoDraft((prev) => (prev ? { ...prev, ogImageId: item.id } : prev));
    } else if (pickerFor === "__new__") {
      markDirty();
      setBlocks((prev) => [
        ...prev,
        { id: newBlockId(), type: "image", attachmentId: item.id, url: item.url, alt: item.alt },
      ]);
    } else if (pickerFor === "__featured__") {
      setFeaturedImage(item);
    } else if (pickerFor) {
      updateBlock(pickerFor, { attachmentId: item.id, url: item.url, alt: item.alt });
    }
    setPickerFor(null);
  };

  const updateSeo = (patch: Partial<PostSeo>): void => {
    setSeoDraft((prev) => (prev ? { ...prev, ...patch } : prev));
  };

  const saveSeo = async (): Promise<void> => {
    if (postId === undefined || !seoDraft) return;
    setError(null);
    setNotice(null);
    setSeoSaving(true);
    try {
      const saved = await api.updatePostSeo(postId, seoDraft);
      setSeo(saved);
      setSeoDraft(saved);
      setAnalysis(await api.getPostSeoAnalysis(postId));
      setNotice("SEO fields saved.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save SEO fields.");
    } finally {
      setSeoSaving(false);
    }
  };

  const toggleCategory = (termTaxonomyId: number): void => {
    markDirty();
    setSelectedCategoryIds((prev) =>
      prev.includes(termTaxonomyId) ? prev.filter((id) => id !== termTaxonomyId) : [...prev, termTaxonomyId],
    );
  };

  const addNewCategory = async (): Promise<void> => {
    const name = newCategoryName.trim();
    if (name === "") return;
    setError(null);
    try {
      const created = await api.createTerm(name, "category");
      setAllCategories((prev) => [...prev, created].sort((a, b) => a.name.localeCompare(b.name)));
      setSelectedCategoryIds((prev) => [...prev, created.termTaxonomyId]);
      setNewCategoryName("");
      markDirty();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to add the category.");
    }
  };

  const removeTag = (termTaxonomyId: number): void => {
    markDirty();
    setSelectedTagIds((prev) => prev.filter((id) => id !== termTaxonomyId));
  };

  const addTag = async (name: string): Promise<void> => {
    const clean = name.trim().toLowerCase();
    if (clean === "") return;
    setError(null);
    setTagSaving(true);
    try {
      const existing = allTags.find((t) => t.name.toLowerCase() === clean || t.slug === clean);
      const term = existing ?? (await api.createTerm(clean, "post_tag"));
      if (!existing) setAllTags((prev) => [...prev, term].sort((a, b) => a.name.localeCompare(b.name)));
      setSelectedTagIds((prev) => (prev.includes(term.termTaxonomyId) ? prev : [...prev, term.termTaxonomyId]));
      setTagInput("");
      markDirty();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to add the tag.");
    } finally {
      setTagSaving(false);
    }
  };

  const tagSuggestions = tagInput.trim() === ""
    ? []
    : allTags
        .filter(
          (t) =>
            !selectedTagIds.includes(t.termTaxonomyId) &&
            (t.name.toLowerCase().includes(tagInput.trim().toLowerCase()) ||
              t.slug.includes(tagInput.trim().toLowerCase())),
        )
        .slice(0, 6);

  /** Write the selected categories + tags through the API. Returns an error string or null. */
  const saveTerms = async (id: number): Promise<string | null> => {
    try {
      await api.setPostTerms(id, "category", selectedCategoryIds);
      await api.setPostTerms(id, "post_tag", selectedTagIds);
      return null;
    } catch (err) {
      return err instanceof Error ? err.message : "Failed to save categories and tags.";
    }
  };

  const save = async (mode: "draft" | "publish" | "schedule"): Promise<void> => {
    setError(null);
    setNotice(null);
    let publishAtValue: string | null | undefined;
    if (mode === "schedule") {
      const iso = fromLocalInputValue(scheduleInput);
      if (iso === null || new Date(iso).getTime() <= Date.now()) {
        setError("Choose a future date and time to schedule this post.");
        return;
      }
      publishAtValue = iso;
    } else if (mode === "publish") {
      // Publishing now clears any pending schedule (deliberate).
      publishAtValue = null;
    }
    setSaving(true);
    const content = serializeBlocks(blocks);
    const nextStatus = mode === "publish" ? "publish" : mode === "schedule" ? "draft" : status === "publish" ? "publish" : "draft";
    try {
      let saved: Post;
      const body: Record<string, unknown> = {
        title,
        content,
        slug: slug.trim() || undefined,
        status: nextStatus,
        excerpt,
      };
      if (publishAtValue !== undefined) body.publishAt = publishAtValue;
      if (post) {
        saved = await api.put<Post>(`/api/posts/${post.id}`, body);
      } else {
        saved = await api.post<Post>("/api/posts", { ...body, type });
        window.location.hash = `#/${type === "page" ? "pages" : "posts"}/${saved.id}/edit`;
      }
      const termsErrorMsg = await saveTerms(saved.id);
      setPost(saved);
      setStatus(saved.status);
      setPublishAt(saved.publishAt ?? null);
      setScheduleInput(saved.publishAt ? toLocalInputValue(saved.publishAt) : "");
      setRevisions(await api.get<Revision[]>(`/api/posts/${saved.id}/revisions`));
      dirtyRef.current = false;
      setAutosave({ kind: "saved", at: new Date().toLocaleTimeString() });
      if (termsErrorMsg !== null) {
        setNotice(
          mode === "schedule"
            ? `Scheduled, but categories/tags could not be saved: ${termsErrorMsg}`
            : `Saved, but categories/tags could not be saved: ${termsErrorMsg}`,
        );
      } else if (mode === "schedule") {
        setNotice(`Scheduled for ${saved.publishAt ? new Date(saved.publishAt).toLocaleString() : scheduleInput}. It will publish automatically at that time.`);
      } else {
        setNotice(mode === "publish" ? "Published." : "Draft saved.");
      }
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Save failed.");
    } finally {
      setSaving(false);
    }
  };

  const clearSchedule = async (): Promise<void> => {
    if (!post) return;
    setError(null);
    try {
      const saved = await api.put<Post>(`/api/posts/${post.id}`, { publishAt: null });
      setPost(saved);
      setPublishAt(saved.publishAt ?? null);
      setScheduleInput("");
      setNotice("Schedule cleared.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to clear the schedule.");
    }
  };

  const restoreRevision = async (revId: number): Promise<void> => {
    setError(null);
    try {
      const restored = await api.post<Post>(`/api/revisions/${revId}/restore`);
      setPost(restored);
      setTitle(restored.title);
      setSlug(restored.slug);
      setStatus(restored.status);
      setExcerpt(restored.excerpt ?? "");
      const parsed = parseBlocks(restored.content || "");
      setBlocks(parsed.length > 0 ? parsed : [emptyParagraph()]);
      setNotice("Revision restored.");
      setRevisions(await api.get<Revision[]>(`/api/posts/${restored.id}/revisions`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Restore failed.");
    }
  };

  const showDiff = async (revId: number): Promise<void> => {
    if (!revisions || revisions.length === 0) return;
    const latestId = revisions[0]?.id;
    if (latestId === undefined || latestId === revId) return;
    setDiff(null);
    setDiffLoading(true);
    try {
      const d = await api.getRevisionDiff(revId, latestId);
      setDiff({ fromId: revId, toId: latestId, diff: d });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the diff.");
    } finally {
      setDiffLoading(false);
    }
  };

  if (loading) return <Skeleton label="Loading editor" height={220} />;

  const previewUrl = post && post.status === "publish" ? postUrl(post, settings.permalinkStructure) : null;
  const wordCount = wordCountOfBlocks(blocks) + (title.trim() === "" ? 0 : title.trim().split(/\s+/).length);
  const readingMinutes = readingTimeMinutes(blocks);
  const selectedCategories = allCategories.filter((t) => selectedCategoryIds.includes(t.termTaxonomyId));
  const selectedTags = allTags.filter((t) => selectedTagIds.includes(t.termTaxonomyId));
  const autosaveText =
    autosave.kind === "saved"
      ? `Autosaved at ${autosave.at}`
      : autosave.kind === "saving"
        ? "Autosaving…"
        : autosave.kind === "failed"
          ? autosave.message
          : autosave.kind === "dirty"
            ? "Unsaved changes"
            : "No unsaved changes";

  return (
    <div>
      {error ? <Alert tone="err" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}
      {post?.hasElementorData === true && (
        <Card title="Elementor content">
          <p style={{ margin: 0 }}>
            This {type === "page" ? "page" : "post"} was built with Elementor. Wordless renders the Elementor layout
            on the public site; the Elementor drag-and-drop editor itself is not included. Edits below change the
            stored block content — the public site keeps showing the Elementor layout while its data is present.
          </p>
        </Card>
      )}

      <Card title={type === "page" ? "Page" : "Post"}>
        <Input
          label="Title"
          className="editor-title"
          value={title}
          onChange={(e) => {
            setTitle(e.target.value);
            markDirty();
          }}
          placeholder={type === "page" ? "Page title" : "Post title"}
        />
        <Input
          label="Slug (leave blank to generate from the title)"
          value={slug}
          onChange={(e) => {
            setSlug(e.target.value);
            markDirty();
          }}
          placeholder="my-post-slug"
          hint={post ? "Changing the slug keeps the old URL working — a redirect is created automatically." : undefined}
        />
      </Card>

      {!distractionFree ? (
        <Card title="Status & scheduling">
          <p style={{ marginTop: 0 }}>
            Status: <Badge tone={status === "publish" ? "teal" : status === "draft" ? "amber" : "blue"}>{status}</Badge>{" "}
            {publishAt ? (
              <span className="muted">Scheduled for {new Date(publishAt).toLocaleString()} — it publishes automatically at that time.</span>
            ) : (
              <span className="muted">Not scheduled.</span>
            )}
          </p>
          <Input
            label="Schedule for (your local time)"
            type="datetime-local"
            value={scheduleInput}
            onChange={(e) => setScheduleInput(e.target.value)}
            hint="A future time saves the post as a draft with a scheduled publish time. Publishing now clears any schedule."
          />
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <Button variant="secondary" disabled={saving || scheduleInput.trim() === ""} onClick={() => void save("schedule")}>
              Schedule
            </Button>
            {publishAt ? (
              <Button variant="ghost" disabled={saving} onClick={() => void clearSchedule()}>
                Clear schedule
              </Button>
            ) : null}
          </div>
        </Card>
      ) : null}

      {!distractionFree ? (
        <Card title="Categories">
          {termsError ? <Alert tone="err" onDismiss={() => setTermsError(null)}>{termsError}</Alert> : null}
          {allCategories.length === 0 ? (
            <p className="muted">No categories yet — add one below.</p>
          ) : (
            <div role="group" aria-label="Categories">
              {allCategories.map((term) => (
                <label key={term.termTaxonomyId} style={{ display: "flex", gap: 8, alignItems: "center", padding: "4px 0" }}>
                  <input
                    type="checkbox"
                    checked={selectedCategoryIds.includes(term.termTaxonomyId)}
                    onChange={() => toggleCategory(term.termTaxonomyId)}
                    style={{ width: "auto" }}
                  />
                  {term.name} <span className="muted">({term.count})</span>
                </label>
              ))}
            </div>
          )}
          <div style={{ display: "flex", gap: 8, alignItems: "flex-end", flexWrap: "wrap", marginTop: 8 }}>
            <div style={{ flex: "1 1 200px" }}>
              <Input
                label="Add new category"
                value={newCategoryName}
                onChange={(e) => setNewCategoryName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void addNewCategory();
                  }
                }}
                placeholder="Category name"
              />
            </div>
            <Button variant="secondary" disabled={newCategoryName.trim() === ""} onClick={() => void addNewCategory()}>
              Add
            </Button>
          </div>
        </Card>
      ) : null}

      {!distractionFree ? (
        <Card title="Tags">
          {termsError ? <Alert tone="err" onDismiss={() => setTermsError(null)}>{termsError}</Alert> : null}
          {selectedTags.length > 0 ? (
            <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 8 }}>
              {selectedTags.map((term) => (
                <span key={term.termTaxonomyId} style={{ display: "inline-flex", gap: 4, alignItems: "center" }}>
                  <Badge tone="teal">{term.name}</Badge>
                  <Button iconOnly icon="x" aria-label={`Remove tag ${term.name}`} variant="ghost" size="small" onClick={() => removeTag(term.termTaxonomyId)} />
                </span>
              ))}
            </div>
          ) : (
            <p className="muted">No tags yet.</p>
          )}
          <Input
            label="Add a tag"
            value={tagInput}
            onChange={(e) => setTagInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void addTag(tagInput);
              }
            }}
            placeholder="Type a tag, press Enter"
            hint="Suggestions appear as you type; Enter creates the tag if it does not exist yet."
          />
          {tagSuggestions.length > 0 ? (
            <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }} role="listbox" aria-label="Tag suggestions">
              {tagSuggestions.map((term) => (
                <Button key={term.termTaxonomyId} variant="ghost" size="small" disabled={tagSaving} onClick={() => void addTag(term.name)}>
                  {term.name}
                </Button>
              ))}
            </div>
          ) : null}
        </Card>
      ) : null}

      {!distractionFree ? (
        <Card title="Featured image">
          <p className="muted" style={{ marginTop: 0 }}>
            Stored as <code>_thumbnail_id</code> in postmeta (the WordPress convention). The API does not expose
            postmeta yet, so a pick cannot be saved — it is kept for this session only. See the README blocker.
          </p>
          {featuredImage ? (
            <div>
              <img src={featuredImage.url} alt={featuredImage.alt || featuredImage.filename} style={{ maxWidth: "100%", borderRadius: 8 }} />
              <p className="muted">{featuredImage.filename}</p>
              <Alert tone="warn">
                Picked, but NOT saved — there is no API endpoint to store the featured image yet. This choice will be
                lost on reload.
              </Alert>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                <Button variant="secondary" size="small" onClick={() => setPickerFor("__featured__")}>Replace</Button>
                <Button variant="ghost" size="small" onClick={() => setFeaturedImage(null)}>Remove</Button>
              </div>
            </div>
          ) : (
            <Button variant="secondary" onClick={() => setPickerFor("__featured__")}>Choose from media library</Button>
          )}
        </Card>
      ) : null}

      {!distractionFree ? (
        <Card title="Excerpt">
          <div className="field">
            <label htmlFor="editor-excerpt">Excerpt ({excerpt.length}/10,000)</label>
            <textarea
              id="editor-excerpt"
              value={excerpt}
              onChange={(e) => {
                setExcerpt(e.target.value);
                markDirty();
              }}
              rows={3}
              placeholder="A short summary, shown on archive pages and used as the fallback meta description."
            />
          </div>
        </Card>
      ) : null}

      <div className="block-list">
        {blocks.map((block, idx) => (
          <BlockView
            key={block.id}
            block={block}
            isFirst={idx === 0}
            isLast={idx === blocks.length - 1}
            isDropTarget={dropTargetId === block.id}
            onChange={(patch) => updateBlock(block.id, patch)}
            onMove={(dir) => moveBlock(block.id, dir)}
            onDelete={() => deleteBlock(block.id)}
            onPickImage={() => setPickerFor(block.id)}
            onDragStart={() => setDraggedId(block.id)}
            onDragOver={() => setDropTargetId(block.id)}
            onDragLeave={() => setDropTargetId((prev) => (prev === block.id ? null : prev))}
            onDrop={() => {
              if (draggedId) moveBlockTo(draggedId, block.id);
              setDraggedId(null);
              setDropTargetId(null);
            }}
            onDragEnd={() => {
              setDraggedId(null);
              setDropTargetId(null);
            }}
            slashQuery={slash?.blockId === block.id ? slash.query : null}
            onSlashOpen={(query) => setSlash({ blockId: block.id, query })}
            onSlashClose={() => setSlash((prev) => (prev?.blockId === block.id ? null : prev))}
            onSlashSelect={(t) => replaceBlock(block.id, t)}
          />
        ))}
      </div>

      <div className="add-block">
        <Select value={addType} onChange={(e) => setAddType(e.target.value as Block["type"])} aria-label="Block type">
          {ADDABLE.map((a) => (
            <option key={a.type} value={a.type}>
              {a.label}
            </option>
          ))}
        </Select>
        <Button variant="secondary" onClick={addBlock}>
          + Add block
        </Button>
        <Button variant="secondary" onClick={() => setPickerFor("__new__")}>
          Insert image from library
        </Button>
      </div>

      <div className="toolbar" style={{ marginTop: 24 }}>
        <Button variant="primary" disabled={saving} onClick={() => void save("draft")}>
          {saving ? "Saving…" : "Save draft"}
        </Button>
        <Button variant="secondary" disabled={saving} onClick={() => void save("publish")}>
          {saving ? "Saving…" : status === "publish" ? "Update" : "Publish"}
        </Button>
        {previewUrl ? (
          <a href={previewUrl} target="_blank" rel="noreferrer">
            <Button variant="ghost">Preview ↗</Button>
          </a>
        ) : (
          <span className="muted">Preview is available once published.</span>
        )}
        <span className="spacer" />
        <span className="muted">
          {wordCount} words · {readingMinutes === 0 ? "less than a minute" : `${readingMinutes} min read`}
        </span>
        <span className="muted" aria-live="polite">{autosaveText}</span>
        <Toggle checked={distractionFree} onChange={setDistractionFree} label="Distraction-free" />
        <Button variant="ghost" onClick={onDone}>
          Back to list
        </Button>
      </div>
      <p className="hint">
        Type <kbd>/</kbd> in an empty paragraph to insert a block, or drag a block by its handle to reorder it.
        Autosave runs 20 seconds after your last edit and always says whether it worked.
      </p>

      {!distractionFree ? (
        postId === undefined ? (
          <Card title="SEO">
            <p className="muted">Save this post first — SEO fields are set on the saved post.</p>
          </Card>
        ) : seoDraft ? (
          <Card title={`SEO${analysis ? ` — score ${analysis.score}/100` : ""}`}>
            <Input
              label={`SEO title (${seoDraft.title.length}/70)`}
              value={seoDraft.title}
              onChange={(e) => updateSeo({ title: e.target.value })}
              placeholder="Defaults to the post title"
            />
            <div className="field">
              <label htmlFor="seo-description">Meta description ({seoDraft.description.length}/160)</label>
              <textarea
                id="seo-description"
                value={seoDraft.description}
                onChange={(e) => updateSeo({ description: e.target.value })}
                rows={2}
                placeholder="Defaults to the excerpt"
              />
            </div>
            <Input
              label={`Focus keyword (${seoDraft.focusKeyword.length}/100)`}
              value={seoDraft.focusKeyword}
              onChange={(e) => updateSeo({ focusKeyword: e.target.value })}
              placeholder="e.g. winter training"
            />
            <Input
              label="Canonical URL (optional)"
              value={seoDraft.canonical}
              onChange={(e) => updateSeo({ canonical: e.target.value })}
              placeholder="https://example.com/this-post"
            />
            <div className="field">
              <label>
                <input
                  type="checkbox"
                  checked={seoDraft.noindex}
                  onChange={(e) => updateSeo({ noindex: e.target.checked })}
                  style={{ width: "auto" }}
                />{" "}
                Noindex — ask search engines not to index this post
              </label>
            </div>
            <Input
              label="Open Graph title"
              value={seoDraft.ogTitle}
              onChange={(e) => updateSeo({ ogTitle: e.target.value })}
              placeholder="Defaults to the SEO title"
            />
            <div className="field">
              <label htmlFor="seo-og-description">Open Graph description</label>
              <textarea
                id="seo-og-description"
                value={seoDraft.ogDescription}
                onChange={(e) => updateSeo({ ogDescription: e.target.value })}
                rows={2}
                placeholder="Defaults to the meta description"
              />
            </div>
            <div className="field">
              <label>Open Graph image</label>
              <p>
                {seoDraft.ogImageId > 0 ? `Attachment #${seoDraft.ogImageId}` : "No image set."}{" "}
                <Button variant="secondary" size="small" onClick={() => setPickerFor("__og__")}>Choose image</Button>{" "}
                {seoDraft.ogImageId > 0 ? (
                  <Button variant="ghost" size="small" onClick={() => updateSeo({ ogImageId: 0 })}>Remove</Button>
                ) : null}
              </p>
            </div>
            <Select
              label="Twitter card"
              value={seoDraft.twitterCard}
              onChange={(e) => updateSeo({ twitterCard: e.target.value as PostSeo["twitterCard"] })}
            >
              <option value="summary">Summary</option>
              <option value="summary_large_image">Summary with large image</option>
            </Select>
            <Button variant="primary" disabled={seoSaving} onClick={() => void saveSeo()}>
              {seoSaving ? "Saving…" : "Save SEO fields"}
            </Button>

            <div style={{ marginTop: 16, border: "1px solid var(--border)", padding: 12, borderRadius: 8 }}>
              <p className="muted" style={{ marginTop: 0 }}>
                Search snippet preview — live, from the fields above. An approximation of how a search result might
                look; it is not a real ranking.
              </p>
              <p style={{ color: "#1a0dab", fontSize: 18, margin: "4px 0" }}>{seoDraft.title || title || "(untitled)"}</p>
              <p className="muted" style={{ margin: "2px 0", fontSize: 13 }}>{previewUrl ?? `/${slug || "post"}`}</p>
              <p style={{ margin: "2px 0", fontSize: 13 }}>{seoDraft.description || excerpt || "No description yet."}</p>
            </div>

            {analysis ? (
              <div style={{ marginTop: 16 }}>
                <h4>SEO analysis — {analysis.score}/100</h4>
                <p className="muted">
                  {analysis.wordCount} words · {analysis.internalLinks} internal link(s) · {analysis.externalLinks}{" "}
                  external link(s) · {analysis.images} image(s), {analysis.imagesMissingAlt} without alt text
                  {analysis.readingEase !== null
                    ? ` · readability (Flesch estimate) ${analysis.readingEase}`
                    : " · readability estimate unavailable (under 10 words)"}
                  .
                </p>
                <ul>
                  {analysis.checks.map((check) => (
                    <li key={check.id}>
                      <Badge tone={check.status === "pass" ? "teal" : check.status === "warn" ? "amber" : "red"}>{check.status}</Badge>{" "}
                      <strong>{check.label}</strong> — {check.detail}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
          </Card>
        ) : null
      ) : null}

      {!distractionFree && revisions && revisions.length > 0 ? (
        <Card title={`Revisions (${revisions.length})`}>
          <Table
            caption="Post revisions"
            columns={[
              { key: "date", label: "Date" },
              { key: "title", label: "Title" },
              { key: "actions", label: "Actions" },
            ]}
            rows={revisions.slice(0, 10).map((rev, i) => ({
              date: formatDateTime(rev.date, settings.timezone),
              title: rev.title || "(untitled)",
              actions: (
                <span style={{ display: "inline-flex", gap: 6 }}>
                  <Button variant="secondary" size="small" onClick={() => void restoreRevision(rev.id)}>
                    Restore
                  </Button>
                  <Button
                    variant="ghost"
                    size="small"
                    disabled={i === 0 || diffLoading}
                    onClick={() => void showDiff(rev.id)}
                    title={i === 0 ? "This is the latest revision" : "Compare with the latest revision"}
                  >
                    Changes
                  </Button>
                </span>
              ),
            }))}
          />
          {diff ? (
            <div style={{ marginTop: 12 }}>
              <h4>
                Changes in revision #{diff.fromId} vs latest (#{diff.toId}) — {diff.diff.added} added,{" "}
                {diff.diff.removed} removed{diff.diff.exact ? "" : " (approximate diff)"}
              </h4>
              {diff.diff.addedLines.length > 0 ? (
                <div>
                  <strong>Added</strong>
                  <pre style={{ whiteSpace: "pre-wrap", background: "var(--good-bg)", padding: 8, borderRadius: 8 }}>
                    {diff.diff.addedLines.map((l) => `+ ${l}`).join("\n")}
                  </pre>
                </div>
              ) : null}
              {diff.diff.removedLines.length > 0 ? (
                <div>
                  <strong>Removed</strong>
                  <pre style={{ whiteSpace: "pre-wrap", background: "var(--danger-bg)", padding: 8, borderRadius: 8 }}>
                    {diff.diff.removedLines.map((l) => `− ${l}`).join("\n")}
                  </pre>
                </div>
              ) : (
                <p className="muted">No differing lines in the first 10.</p>
              )}
              <Button variant="ghost" size="small" onClick={() => setDiff(null)}>
                Hide diff
              </Button>
            </div>
          ) : null}
        </Card>
      ) : null}

      {pickerFor ? (
        <MediaPicker
          title={pickerFor === "__featured__" ? "Choose featured image" : pickerFor === "__og__" ? "Choose Open Graph image" : "Insert image"}
          onSelect={pickImage}
          onClose={() => setPickerFor(null)}
        />
      ) : null}
    </div>
  );
}

/** One editable block. contentEditable edits commit on blur to keep the caret stable. */
function BlockView({
  block,
  isFirst,
  isLast,
  isDropTarget,
  onChange,
  onMove,
  onDelete,
  onPickImage,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop,
  onDragEnd,
  slashQuery,
  onSlashOpen,
  onSlashClose,
  onSlashSelect,
}: {
  block: Block;
  isFirst: boolean;
  isLast: boolean;
  isDropTarget: boolean;
  onChange: (patch: Partial<Block>) => void;
  onMove: (dir: -1 | 1) => void;
  onDelete: () => void;
  onPickImage: () => void;
  onDragStart: () => void;
  onDragOver: () => void;
  onDragLeave: () => void;
  onDrop: () => void;
  onDragEnd: () => void;
  slashQuery: string | null;
  onSlashOpen: (query: string) => void;
  onSlashClose: () => void;
  onSlashSelect: (type: Block["type"]) => void;
}): React.ReactElement {
  const slashMatches =
    slashQuery === null
      ? []
      : ADDABLE.filter((a) => a.label.toLowerCase().includes(slashQuery.toLowerCase()) || a.type.startsWith(slashQuery.toLowerCase()));

  /** Slash-command detection: typing "/" at the start of an empty text block opens the menu. */
  const onTextInput = (e: React.FormEvent<HTMLElement>): void => {
    if (block.type !== "paragraph" && block.type !== "heading" && block.type !== "quote") return;
    const text = e.currentTarget.innerText ?? "";
    if (text.startsWith("/")) {
      onSlashOpen(text.slice(1));
    } else if (slashQuery !== null) {
      onSlashClose();
    }
  };

  const tools = (
    <div className="block-tools">
      <span
        className="kind"
        draggable
        onDragStart={(e) => {
          e.dataTransfer.effectAllowed = "move";
          onDragStart();
        }}
        onDragEnd={onDragEnd}
        title="Drag to reorder"
        aria-label={`Drag ${block.type} block to reorder`}
        style={{ cursor: "grab", display: "inline-flex", alignItems: "center", gap: 4 }}
      >
        <Icon name="menu" size={14} />
        {block.type}
      </span>
      {block.type === "heading" ? (
        <select
          value={block.level ?? 2}
          onChange={(e) => onChange({ level: Number(e.target.value) === 3 ? 3 : 2 })}
          aria-label="Heading level"
          style={{ width: "auto" }}
        >
          <option value={2}>H2</option>
          <option value={3}>H3</option>
        </select>
      ) : null}
      {block.type === "list" ? (
        <label style={{ display: "inline-flex", alignItems: "center", gap: 4, fontWeight: 400 }}>
          <input
            type="checkbox"
            checked={block.ordered ?? false}
            onChange={(e) => onChange({ ordered: e.target.checked })}
            style={{ width: "auto" }}
          />
          Numbered
        </label>
      ) : null}
      <span className="spacer" />
      <Button variant="ghost" size="small" disabled={isFirst} onClick={() => onMove(-1)} aria-label="Move up">
        ↑
      </Button>
      <Button variant="ghost" size="small" disabled={isLast} onClick={() => onMove(1)} aria-label="Move down">
        ↓
      </Button>
      <Button variant="danger" size="small" onClick={onDelete} aria-label="Delete block">
        Delete
      </Button>
    </div>
  );

  const slashMenu =
    slashQuery !== null ? (
      <div role="listbox" aria-label="Insert block" style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 8 }}>
        {slashMatches.length === 0 ? (
          <span className="muted">No block matches “/{slashQuery}”.</span>
        ) : (
          slashMatches.map((a) => (
            <Button key={a.type} variant="secondary" size="small" onClick={() => onSlashSelect(a.type)}>
              {a.label}
            </Button>
          ))
        )}
        <Button variant="ghost" size="small" onClick={onSlashClose} aria-label="Close block menu">
          Cancel
        </Button>
      </div>
    ) : null;

  const wrapProps = {
    className: "block",
    onDragOver: (e: React.DragEvent) => {
      e.preventDefault();
      onDragOver();
    },
    onDragLeave,
    onDrop: (e: React.DragEvent) => {
      e.preventDefault();
      onDrop();
    },
    style: isDropTarget ? { outline: "2px dashed var(--accent)", outlineOffset: 4 } : undefined,
  };

  switch (block.type) {
    case "paragraph":
    case "heading":
    case "quote": {
      const Tag = block.type === "heading" ? (block.level === 3 ? "h3" : "h2") : block.type === "quote" ? "blockquote" : "p";
      return (
        <div {...wrapProps}>
          {tools}
          <Tag
            contentEditable
            suppressContentEditableWarning
            dangerouslySetInnerHTML={{ __html: block.html ?? "" }}
            onInput={onTextInput}
            onBlur={(e) => onChange({ html: e.currentTarget.innerHTML })}
            data-placeholder={block.type === "paragraph" ? "Write… (type / for blocks)" : ""}
          />
          {slashMenu}
          {block.type === "quote" ? (
            <div className="field" style={{ marginTop: 8, marginBottom: 0 }}>
              <label htmlFor={`cite-${block.id}`}>Citation (optional)</label>
              <input
                id={`cite-${block.id}`}
                value={block.cite ?? ""}
                onChange={(e) => onChange({ cite: e.target.value })}
                placeholder="Author or source"
              />
            </div>
          ) : null}
        </div>
      );
    }
    case "list":
      return (
        <div {...wrapProps}>
          {tools}
          {(block.items ?? []).map((item, i) => (
            <div key={i} style={{ display: "flex", gap: 8, alignItems: "flex-start" }}>
              <span className="muted">{block.ordered ? `${i + 1}.` : "•"}</span>
              <div
                style={{ flex: 1 }}
                contentEditable
                suppressContentEditableWarning
                dangerouslySetInnerHTML={{ __html: item }}
                onBlur={(e) => {
                  const items = [...(block.items ?? [])];
                  items[i] = e.currentTarget.innerHTML;
                  onChange({ items });
                }}
              />
              <Button
                iconOnly
                icon="x"
                aria-label="Remove list item"
                variant="ghost"
                size="small"
                onClick={() => {
                  const items = (block.items ?? []).filter((_, j) => j !== i);
                  onChange({ items: items.length > 0 ? items : [""] });
                }}
              />
            </div>
          ))}
          <Button
            variant="secondary"
            size="small"
            onClick={() => onChange({ items: [...(block.items ?? []), ""] })}
          >
            + Add item
          </Button>
        </div>
      );
    case "image":
      return (
        <div {...wrapProps}>
          {tools}
          {block.url ? (
            <img src={block.url} alt={block.alt ?? ""} />
          ) : (
            <p className="muted">No image selected.</p>
          )}
          <div className="field">
            <label htmlFor={`alt-${block.id}`}>Alt text</label>
            <input id={`alt-${block.id}`} value={block.alt ?? ""} onChange={(e) => onChange({ alt: e.target.value })} />
          </div>
          <div className="field">
            <label htmlFor={`cap-${block.id}`}>Caption (optional)</label>
            <input
              id={`cap-${block.id}`}
              value={block.caption ?? ""}
              onChange={(e) => onChange({ caption: e.target.value })}
            />
          </div>
          <Button variant="secondary" size="small" onClick={onPickImage}>
            {block.url ? "Replace image" : "Choose image"}
          </Button>
        </div>
      );
    case "code":
      return (
        <div {...wrapProps}>
          {tools}
          <textarea
            value={block.code ?? ""}
            onChange={(e) => onChange({ code: e.target.value })}
            aria-label="Code"
            spellCheck={false}
          />
        </div>
      );
    case "separator":
      return (
        <div {...wrapProps}>
          {tools}
          <hr />
        </div>
      );
    case "raw":
    default:
      return (
        <div {...wrapProps}>
          {tools}
          <p className="hint">Unsupported block — preserved as raw HTML. Edit the source below; it is never dropped.</p>
          <textarea
            value={block.raw ?? ""}
            onChange={(e) => onChange({ raw: e.target.value })}
            aria-label="Raw HTML"
            spellCheck={false}
          />
        </div>
      );
  }
}
