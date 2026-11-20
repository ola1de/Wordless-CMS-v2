/**
 * Site Health (§6.3) — every check with its real status and message,
 * critical first. Stat cards count what the API returned; nothing
 * is summarised away.
 */
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { HealthCheck, SiteHealth } from "../lib/types";
import { Alert, Badge, Card, EmptyState, Skeleton, StatCard, Table } from "../components/elements";

function statusTone(s: string): "teal" | "amber" | "red" | "grey" {
  if (s === "good") return "teal";
  if (s === "recommended") return "amber";
  if (s === "critical") return "red";
  return "grey";
}

const ORDER: Record<string, number> = { critical: 0, recommended: 1, good: 2 };

export function HealthPage(): React.ReactElement {
  const [checks, setChecks] = useState<HealthCheck[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .get<SiteHealth>("/api/site-health")
      .then((h) => {
        if (!cancelled) setChecks(h.checks);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load site health.");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const critical = checks?.filter((c) => c.status === "critical").length ?? 0;
  const recommended = checks?.filter((c) => c.status === "recommended").length ?? 0;
  const good = checks?.filter((c) => c.status === "good").length ?? 0;
  const sorted = [...(checks ?? [])].sort((a, b) => (ORDER[a.status] ?? 3) - (ORDER[b.status] ?? 3));

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Site health" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {checks === null && error === null ? (
        <div className="grid" aria-label="Loading health checks">
          <Skeleton height={92} label="Loading critical checks" />
          <Skeleton height={92} label="Loading recommended checks" />
          <Skeleton height={92} label="Loading good checks" />
        </div>
      ) : (
        <div className="grid">
          <StatCard label="Critical issues" value={String(critical)} icon="alert" tone={critical > 0 ? "danger" : undefined} />
          <StatCard label="Recommended fixes" value={String(recommended)} icon="info" tone={recommended > 0 ? "warn" : undefined} />
          <StatCard label="Checks passing" value={String(good)} icon="checkCircle" />
        </div>
      )}
      <Card title="Health checks" sub={critical > 0 ? "Fix the critical items first — they can break the site." : "No critical issues."}>
        {checks === null ? (
          <Skeleton height={180} label="Loading health checks" />
        ) : checks.length === 0 ? (
          <EmptyState icon="health" title="No health checks returned" />
        ) : (
          <Table
            caption="Site health checks"
            columns={[
              { key: "check", label: "Check" },
              { key: "status", label: "Status" },
              { key: "details", label: "Details" },
            ]}
            rows={sorted.map((check) => ({
              check: <span className="row-title">{check.name}</span>,
              status: <Badge tone={statusTone(check.status)}>{check.status}</Badge>,
              details: check.message,
            }))}
          />
        )}
      </Card>
    </div>
  );
}
