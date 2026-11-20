/**
 * Content Studio (§6.3) — editorial calendar, content templates
 * that create real drafts, and AI idea generation through the real
 * task API. Drafts are proposed, never published by the agent.
 * A draft with a future date is the schedule — Wordless has no
 * "future" post status yet.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { AiProvidersResponse, AiTask, AiTaskList, ContentCalendar, ContentTemplate } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Skeleton, Table } from "../components/elements";

function appliedPostId(task: AiTask): number | null {
  for (const applied of task.result?.applied ?? []) {
    const id = applied.result?.postId;
    if (typeof id === "number" && id > 0) return id;
  }
  return null;
}

export function StudioPage(): React.ReactElement {
  const [calendar, setCalendar] = useState<ContentCalendar | null>(null);
  const [templates, setTemplates] = useState<ContentTemplate[] | null>(null);
  const [providers, setProviders] = useState<AiProvidersResponse | null>(null);
  const [tasks, setTasks] = useState<AiTaskList | null>(null);
  const [idea, setIdea] = useState("");
  const [ideaTask, setIdeaTask] = useState<AiTask | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [cal, tpl, prov, recent] = await Promise.all([
        api.getCalendar(30),
        api.listTemplates(),
        api.getAiProviders(),
        api.listAiTasks(),
      ]);
      setCalendar(cal);
      setTemplates(tpl);
      setProviders(prov);
      setTasks(recent);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the content studio.");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const fail = (err: unknown): void => {
    setError(err instanceof Error ? err.message : "Something went wrong.");
  };

  const selectedProvider = providers?.providers.find((p) => p.id === providers.selected);
  const providerReady = selectedProvider?.configured === true;

  const useMockProvider = async (): Promise<void> => {
    setError(null);
    try {
      setProviders(await api.selectAiProvider("mock"));
      setNotice("Mock provider selected for local development. Switch back to Workers AI before going live.");
    } catch (err) {
      fail(err);
    }
  };

  const createDraft = async (slug: string): Promise<void> => {
    setError(null);
    setNotice(null);
    try {
      const { id } = await api.createFromTemplate(slug);
      setNotice("Draft created from the template.");
      window.location.hash = `#/posts/${id}/edit`;
    } catch (err) {
      fail(err);
    }
  };

  const generateIdea = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    setIdeaTask(null);
    if (idea.trim() === "") {
      setError("Write a topic first.");
      return;
    }
    setBusy(true);
    try {
      const task = await api.createAiTask({ agent: "content-writer", input: idea.trim() });
      setIdeaTask(task);
      setTasks(await api.listAiTasks());
      if (task.status === "failed") {
        setError(task.error ?? "The AI task failed.");
      }
    } catch (err) {
      fail(err);
    } finally {
      setBusy(false);
    }
  };

  const daysWithPosts = calendar?.days.filter((d) => d.posts.length > 0) ?? [];

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Content studio" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Card title="Editorial calendar — next 30 days" sub={calendar !== null ? `${calendar.total} post(s) in the window ${calendar.from} → ${calendar.to}.` : undefined}>
        {calendar === null ? (
          <Skeleton height={120} label="Loading calendar" />
        ) : daysWithPosts.length === 0 ? (
          <EmptyState icon="calendar" title="Nothing scheduled in the next 30 days" description="Publish a post, or save a draft with a future date — a draft with a future date is the schedule (Wordless has no “future” status yet)." />
        ) : (
          <Table
            caption="Editorial calendar"
            columns={[
              { key: "date", label: "Date" },
              { key: "posts", label: "Posts" },
            ]}
            rows={daysWithPosts.map((day) => ({
              date: day.date,
              posts: (
                <span>
                  {day.posts.map((p) => (
                    <span key={p.id} style={{ display: "block" }}>
                      <a href={`#/${p.type === "page" ? "pages" : "posts"}/${p.id}/edit`}>{p.title || "(untitled)"}</a>{" "}
                      <Badge tone={p.status === "publish" ? "teal" : "amber"}>{p.status}</Badge>
                    </span>
                  ))}
                </span>
              ),
            }))}
          />
        )}
      </Card>

      <Card title="Content templates" sub="Create a real draft post, prefilled with blocks. Nothing here is a demo — the draft is a post you can edit and publish.">
        {templates === null ? (
          <Skeleton height={120} label="Loading templates" />
        ) : templates.length === 0 ? (
          <EmptyState icon="studio" title="No templates available" />
        ) : (
          <Table
            caption="Content templates"
            columns={[
              { key: "name", label: "Template" },
              { key: "desc", label: "What it is" },
              { key: "actions", label: "Actions" },
            ]}
            rows={templates.map((t) => ({
              name: <span className="row-title">{t.name}</span>,
              desc: t.description,
              actions: <Button size="small" variant="primary" icon="plus" onClick={() => void createDraft(t.slug)}>Create draft</Button>,
            }))}
          />
        )}
      </Card>

      <Card title="Content ideas (AI)" sub={providerReady ? `Using ${selectedProvider?.name}. Drafts are proposed, never published by the agent.` : "Ideas are generated by the content-writer agent through the real AI task API — nothing is invented here."}>
        {providers === null ? (
          <Skeleton height={100} label="Loading providers" />
        ) : !providerReady ? (
          <Alert tone="warn" title="No AI provider is configured">
            {providers.selected} is the selected provider.{" "}
            <Button size="small" onClick={() => void useMockProvider()}>Use the mock provider (local development)</Button>
          </Alert>
        ) : null}
        <Input label="Topic for a draft post" id="idea-topic" value={idea} onChange={(e) => setIdea(e.target.value)} placeholder="e.g. how the pack prepares for winter" />
        <Button variant="primary" icon="sparkles" disabled={busy || !providerReady} onClick={() => void generateIdea()}>{busy ? "Drafting…" : "Generate draft"}</Button>
        {ideaTask?.result !== undefined ? (
          <div className="ai-result">
            <p><Badge tone="blue">AI output</Badge> <strong>{ideaTask.result.summary}</strong></p>
            {ideaTask.result.output ? <pre className="code-block">{ideaTask.result.output}</pre> : null}
            {appliedPostId(ideaTask) !== null ? (
              <p><a href={`#/posts/${appliedPostId(ideaTask) as number}/edit`}>Open the draft in the editor</a></p>
            ) : ideaTask.result.approvals.length > 0 ? (
              <p>The draft is waiting for approval — review it in the <a href="#/ai">AI command centre</a>.</p>
            ) : null}
          </div>
        ) : null}
      </Card>

      <Card title="Recent AI drafts">
        {tasks === null ? (
          <Skeleton height={120} label="Loading recent drafts" />
        ) : tasks.tasks.length === 0 ? (
          <EmptyState icon="ai" title="No AI tasks yet" description="Generate an idea above, or run an agent from the AI command centre." />
        ) : (
          <Table
            caption="Recent AI tasks"
            columns={[
              { key: "agent", label: "Agent" },
              { key: "request", label: "Request" },
              { key: "status", label: "Status" },
              { key: "draft", label: "Draft" },
            ]}
            rows={tasks.tasks.slice(0, 10).map((t) => {
              const postId = appliedPostId(t);
              return {
                agent: t.agent,
                request: t.input.slice(0, 80),
                status: <Badge tone={t.status === "done" ? "teal" : t.status === "failed" ? "red" : "amber"}>{t.status}</Badge>,
                draft: postId !== null ? <a href={`#/posts/${postId}/edit`}>Open draft</a> : <span className="muted">—</span>,
              };
            })}
          />
        )}
      </Card>
    </div>
  );
}
