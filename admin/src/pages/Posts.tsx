/**
 * Posts / Pages list (§6.3 — content lists).
 *
 * Shared-elements rebuild of the v1 list: status filter, search,
 * trash/restore, pagination. Rows are real API data only; the empty
 * state names the filter that produced it so an empty filtered list
 * is never mistaken for an empty site.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatDateTime } from "../lib/format";
import type { PostList } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Select, Skeleton, Table } from "../components/elements";

const STATUSES = ["publish", "draft", "trash"];

function statusTone(status: string): "teal" | "amber" | "red" | "blue" | "grey" {
  if (status === "publish") return "teal";
  if (status === "draft") return "amber";
  if (status === "trash") return "red";
  if (status === "private") return "blue";
  return "grey";
}

export function PostsPage({ type, timezone }: { type: "post" | "page"; timezone: string }): React.ReactElement {
  const [list, setList] = useState<PostList | null>(null);
  const [page, setPage] = useState(1);
  const [status, setStatus] = useState("");
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [workingId, setWorkingId] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      const params = new URLSearchParams({ type, page: String(page), per_page: "20" });
      if (status) params.set("status", status);
      if (query) params.set("search", query);
      setList(await api.get<PostList>(`/api/posts?${params.toString()}`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load content.");
    }
  }, [type, page, status, query]);

  useEffect(() => {
    void load();
  }, [load]);

  const trash = async (id: number): Promise<void> => {
    setError(null);
    setWorkingId(id);
    try {
      await api.post(`/api/posts/${id}/trash`);
      setNotice("Moved to trash.");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to trash.");
    } finally {
      setWorkingId(null);
    }
  };

  const restore = async (id: number): Promise<void> => {
    setError(null);
    setWorkingId(id);
    try {
      await api.post(`/api/posts/${id}/restore`);
      setNotice("Restored as draft.");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to restore.");
    } finally {
      setWorkingId(null);
    }
  };

  const label = type === "page" ? "Pages" : "Posts";
  const editBase = type === "page" ? "#/pages" : "#/posts";

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Could not load content" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <div className="toolbar">
        <Select label="Status filter" value={status} onChange={(e) => { setStatus(e.target.value); setPage(1); }} aria-label="Filter by status">
          <option value="">All statuses</option>
          {STATUSES.map((s) => (
            <option key={s} value={s}>{s}</option>
          ))}
        </Select>
        <Input
          label="Search"
          type="search"
          placeholder={`Search ${label.toLowerCase()}…`}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              setQuery(search.trim());
              setPage(1);
            }
          }}
        />
        <Button icon="search" onClick={() => { setQuery(search.trim()); setPage(1); }}>Search</Button>
        <span className="spacer" />
        <Button variant="primary" icon="plus" onClick={() => { window.location.hash = `${editBase}/new`; }}>
          Add new {type === "page" ? "page" : "post"}
        </Button>
      </div>

      <Card title={label} sub={list !== null ? `${list.total} total` : undefined}>
        {list === null ? (
          <Skeleton height={180} label={`Loading ${label.toLowerCase()}`} />
        ) : list.posts.length === 0 ? (
          <EmptyState
            icon="posts"
            title={status === "" && query === "" ? `No ${label.toLowerCase()} yet` : `No ${label.toLowerCase()} match`}
            description={status === "" && query === "" ? `Create the first ${type} to get started.` : "Try a different status or search term."}
            action={status === "" && query === "" ? <Button variant="primary" icon="plus" onClick={() => { window.location.hash = `${editBase}/new`; }}>Create the first one</Button> : <Button onClick={() => { setStatus(""); setQuery(""); setSearch(""); setPage(1); }}>Clear filters</Button>}
          />
        ) : (
          <>
            <Table
              caption={`${label} list`}
              columns={[
                { key: "title", label: "Title" },
                { key: "status", label: "Status" },
                { key: "modified", label: "Modified" },
                { key: "actions", label: "Actions" },
              ]}
              rows={list.posts.map((post) => ({
                title: (
                  <span>
                    <a href={`${editBase}/${post.id}/edit`} className="row-title">{post.title || "(untitled)"}</a>
                    <span className="row-sub">/{post.slug}</span>
                  </span>
                ),
                status: <Badge tone={statusTone(post.status)}>{post.status}</Badge>,
                modified: formatDateTime(post.modified, timezone),
                actions:
                  post.status === "trash" ? (
                    <Button size="small" disabled={workingId === post.id} onClick={() => void restore(post.id)}>Restore</Button>
                  ) : (
                    <Button size="small" variant="danger" disabled={workingId === post.id} onClick={() => void trash(post.id)}>Trash</Button>
                  ),
              }))}
            />
            <div className="pagination">
              <Button size="small" icon="chevronLeft" disabled={list.page <= 1} onClick={() => setPage(list.page - 1)}>Previous</Button>
              <span>Page {list.page} of {Math.max(list.totalPages, 1)} ({list.total} total)</span>
              <Button size="small" icon="chevronRight" disabled={list.page >= list.totalPages} onClick={() => setPage(list.page + 1)}>Next</Button>
            </div>
          </>
        )}
      </Card>
    </div>
  );
}
