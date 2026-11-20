/**
 * Activity (§6.3) — the audit log, newest first, with actor and
 * object. "system" is actor 0; every other row names the user id.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatDateTime } from "../lib/format";
import type { ActivityList } from "../lib/types";
import { Alert, Button, Card, EmptyState, Skeleton, Table } from "../components/elements";

export function ActivityPage({ timezone }: { timezone: string }): React.ReactElement {
  const [list, setList] = useState<ActivityList | null>(null);
  const [page, setPage] = useState(1);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setList(await api.get<ActivityList>(`/api/activity?page=${page}&per_page=20`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load activity.");
    }
  }, [page]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Activity" onDismiss={() => setError(null)}>{error}</Alert> : null}
      <Card title="Activity log" sub={list !== null ? `${list.total} entries` : undefined}>
        {list === null ? (
          <Skeleton height={180} label="Loading activity" />
        ) : list.entries.length === 0 ? (
          <EmptyState icon="activity" title="No activity recorded yet" description="Changes to posts, settings and orders will appear here." />
        ) : (
          <>
            <Table
              caption="Activity log"
              columns={[
                { key: "action", label: "Action" },
                { key: "actor", label: "Actor" },
                { key: "details", label: "Details" },
                { key: "when", label: "When" },
              ]}
              rows={list.entries.map((entry) => ({
                action: <span className="row-title">{entry.action}</span>,
                actor: entry.actorId === 0 ? "system" : `user #${entry.actorId}`,
                details: (
                  <span>
                    {entry.objectType ? `${entry.objectType}${entry.objectId ? ` #${entry.objectId}` : ""}` : "—"}
                    {entry.details ? <span className="row-sub">{entry.details}</span> : null}
                  </span>
                ),
                when: formatDateTime(entry.createdAt, timezone),
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
