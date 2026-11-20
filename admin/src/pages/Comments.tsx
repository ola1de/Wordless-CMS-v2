/**
 * Comments (§6.3) — moderation queue.
 *
 * Approve / spam / trash per comment, with the action that is
 * already true hidden (an approved comment offers spam and trash,
 * not approve again). Status filter + pagination.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatDateTime } from "../lib/format";
import type { CommentList } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Select, Skeleton, Table } from "../components/elements";

const STATUSES = ["pending", "approved", "spam", "trash"];

function statusTone(s: string): "teal" | "amber" | "red" | "grey" {
  if (s === "approved") return "teal";
  if (s === "pending") return "amber";
  if (s === "spam" || s === "trash") return "red";
  return "grey";
}

export function CommentsPage({ timezone }: { timezone: string }): React.ReactElement {
  const [list, setList] = useState<CommentList | null>(null);
  const [page, setPage] = useState(1);
  const [status, setStatus] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [workingId, setWorkingId] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      const params = new URLSearchParams({ page: String(page), per_page: "20" });
      if (status) params.set("status", status);
      setList(await api.get<CommentList>(`/api/comments?${params.toString()}`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load comments.");
    }
  }, [page, status]);

  useEffect(() => {
    void load();
  }, [load]);

  const moderate = async (id: number, action: "approve" | "spam" | "trash"): Promise<void> => {
    setError(null);
    setWorkingId(id);
    try {
      await api.post(`/api/comments/${id}/${action}`);
      setNotice(`Comment ${action === "approve" ? "approved" : action === "spam" ? "marked as spam" : "trashed"}.`);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Moderation failed.");
    } finally {
      setWorkingId(null);
    }
  };

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Comments" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <div className="toolbar">
        <Select label="Status filter" value={status} onChange={(e) => { setStatus(e.target.value); setPage(1); }} aria-label="Filter by status">
          <option value="">All statuses</option>
          {STATUSES.map((s) => (
            <option key={s} value={s}>{s}</option>
          ))}
        </Select>
      </div>

      <Card title="Comments" sub={list !== null ? `${list.total} total${status !== "" ? ` · ${status}` : ""}` : undefined}>
        {list === null ? (
          <Skeleton height={180} label="Loading comments" />
        ) : list.comments.length === 0 ? (
          <EmptyState icon="comments" title={`No comments${status ? ` with status “${status}”` : ""}`} description={status === "pending" ? "Nothing is waiting for moderation." : "Try a different status filter."} />
        ) : (
          <>
            <Table
              caption="Comments"
              columns={[
                { key: "author", label: "Author" },
                { key: "comment", label: "Comment" },
                { key: "status", label: "Status" },
                { key: "date", label: "Date" },
                { key: "actions", label: "Actions" },
              ]}
              rows={list.comments.map((comment) => ({
                author: (
                  <span>
                    <span className="row-title">{comment.author}</span>
                    <span className="row-sub">{comment.authorEmail}</span>
                    <span className="row-sub">Post #{comment.postId}</span>
                  </span>
                ),
                comment: comment.content,
                status: <Badge tone={statusTone(comment.status)}>{comment.status}</Badge>,
                date: formatDateTime(comment.date, timezone),
                actions: (
                  <span className="actions">
                    {comment.status !== "approved" ? (
                      <Button size="small" icon="check" disabled={workingId === comment.id} onClick={() => void moderate(comment.id, "approve")}>Approve</Button>
                    ) : null}
                    {comment.status !== "spam" ? (
                      <Button size="small" disabled={workingId === comment.id} onClick={() => void moderate(comment.id, "spam")}>Spam</Button>
                    ) : null}
                    {comment.status !== "trash" ? (
                      <Button size="small" variant="danger" icon="trash" disabled={workingId === comment.id} onClick={() => void moderate(comment.id, "trash")}>Trash</Button>
                    ) : null}
                  </span>
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
