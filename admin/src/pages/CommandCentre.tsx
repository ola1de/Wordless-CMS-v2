/**
 * AI Command Centre (§6.3 — the flagship screen).
 *
 * Conversation-style task composer, live task progress (polls while
 * any task is running), approvals as cards with approve/reject +
 * reason, agent directory in plain language, memory as an editable
 * fact list. Honest rules kept from v1: AI output is labelled as
 * such; nothing publishes without an approval; facts are re-ingested
 * from the site (rebuild) and cannot be hand-edited, while notes are
 * yours to add and delete. The email/social composers only draft —
 * they never send.
 */
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { AgentInfo, AiApproval, AiMode, AiProvidersResponse, AiTask, MemoryEntry, MemoryList, ProviderInfo } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Modal, Progress, Select, Skeleton, Tabs } from "../components/elements";
import { Icon } from "../components/icons";

const MODE_ORDER: AiMode[] = ["manual", "supervised", "autonomous"];
const MODE_LABELS: Record<AiMode, string> = { manual: "Manual", supervised: "Supervised", autonomous: "Autonomous" };
const MODE_TEXT: Record<AiMode, string> = {
  manual: "Agents only draft. Nothing is applied.",
  supervised: "Every proposal waits for your approval. Nothing applies on its own.",
  autonomous: "Low-risk proposals (e.g. draft posts) apply automatically. Anything destructive or publishing still waits for approval.",
};

function riskTone(risk: string): "teal" | "amber" | "red" | "grey" {
  if (risk === "low") return "teal";
  if (risk === "medium") return "amber";
  if (risk === "high") return "red";
  return "grey";
}

export function CommandCentrePage(): React.ReactElement {
  const [providers, setProviders] = useState<AiProvidersResponse | null>(null);
  const [mode, setMode] = useState<AiMode | null>(null);
  const [modeDescription, setModeDescription] = useState("");
  const [agents, setAgents] = useState<AgentInfo[] | null>(null);
  const [tasks, setTasks] = useState<AiTask[] | null>(null);
  const [selectedTask, setSelectedTask] = useState<AiTask | null>(null);
  const [approvals, setApprovals] = useState<AiApproval[] | null>(null);
  const [memory, setMemory] = useState<MemoryList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [probe, setProbe] = useState<Record<string, string>>({});
  const [agentId, setAgentId] = useState("");
  const [taskInput, setTaskInput] = useState("");
  const [taskMode, setTaskMode] = useState<"" | AiMode>("");
  const [noteText, setNoteText] = useState("");
  const [rejectReason, setRejectReason] = useState<Record<string, string>>({});
  const [tab, setTab] = useState("composer");
  const [editingNote, setEditingNote] = useState<MemoryEntry | null>(null);
  const [noteDraft, setNoteDraft] = useState("");

  const load = async (): Promise<void> => {
    try {
      const [prov, modeRes, agentList, taskList, apprList, mem] = await Promise.all([
        api.getAiProviders(),
        api.getAiMode(),
        api.listAiAgents(),
        api.listAiTasks(),
        api.listAiApprovals("pending"),
        api.getAiMemory(),
      ]);
      setProviders(prov);
      setMode(modeRes.mode);
      setModeDescription(modeRes.description);
      setAgents(agentList);
      setTasks(taskList.tasks);
      setApprovals(apprList.approvals);
      setMemory(mem);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the AI command centre.");
    }
  };

  useEffect(() => {
    let cancelled = false;
    void load().then(() => {
      if (!cancelled) { /* load() sets state itself */ }
    });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const refreshTasks = async (): Promise<void> => {
    try {
      const [taskList, apprList] = await Promise.all([api.listAiTasks(), api.listAiApprovals("pending")]);
      setTasks(taskList.tasks);
      setApprovals(apprList.approvals);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to refresh tasks.");
    }
  };

  /** Live progress: poll while any task is still running. */
  const anyRunning = (tasks ?? []).some((t) => t.status === "running" || t.status === "pending");
  useEffect(() => {
    if (!anyRunning) return;
    const id = window.setInterval(() => { void refreshTasks(); }, 2000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [anyRunning]);

  const selectProvider = async (id: string): Promise<void> => {
    setError(null);
    try {
      setProviders(await api.selectAiProvider(id));
      setNotice(`Provider set to ${id}.`);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not change the provider.");
    }
  };

  const testProvider = async (provider: ProviderInfo): Promise<void> => {
    setError(null);
    try {
      const result = await api.testAiProvider(provider.id);
      setProbe((prev) => ({ ...prev, [provider.id]: result.ok ? `OK — ${result.message}` : `Not OK — ${result.message}` }));
    } catch (err) {
      setProbe((prev) => ({ ...prev, [provider.id]: `Probe failed: ${err instanceof Error ? err.message : String(err)}` }));
    }
  };

  const setOperatingMode = async (next: AiMode): Promise<void> => {
    setError(null);
    try {
      const res = await api.setAiMode(next);
      setMode(res.mode);
      setModeDescription(res.description);
      setNotice(`Operating mode set to ${res.mode}.`);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not change the operating mode.");
    }
  };

  const runTask = async (): Promise<void> => {
    if (agentId === "" || taskInput.trim() === "") {
      setError("Pick an agent and write what you want it to do.");
      return;
    }
    setError(null);
    setNotice(null);
    setWorking(true);
    try {
      const task = await api.createAiTask({ agent: agentId, input: taskInput.trim(), mode: taskMode === "" ? undefined : taskMode });
      setSelectedTask(task);
      setTaskInput("");
      await refreshTasks();
      if (task.status === "failed") setError(task.error ?? "The AI task failed.");
      else if (task.result !== undefined && task.result.approvals.length > 0) setNotice(`Task finished. ${task.result.approvals.length} proposal(s) waiting for approval below.`);
      else if (task.result !== undefined && task.result.applied.length > 0) setNotice(`Task finished. ${task.result.applied.length} low-risk action(s) applied automatically.`);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not run the AI task.");
    } finally {
      setWorking(false);
    }
  };

  const decide = async (approval: AiApproval, approve: boolean): Promise<void> => {
    setError(null);
    try {
      if (approve) {
        await api.approveAiApproval(approval.id);
        setNotice(`Approved: ${approval.action.summary}`);
      } else {
        await api.rejectAiApproval(approval.id, rejectReason[approval.id] ?? "");
        setNotice(`Rejected: ${approval.action.summary}`);
      }
      await refreshTasks();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not decide the approval.");
    }
  };

  const addNote = async (): Promise<void> => {
    if (noteText.trim() === "") return;
    setError(null);
    try {
      await api.addAiMemoryNote(noteText.trim());
      setNoteText("");
      setMemory(await api.getAiMemory());
      setNotice("Note added to website memory.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not add the note.");
    }
  };

  const removeNote = async (note: MemoryEntry): Promise<void> => {
    setError(null);
    try {
      await api.deleteAiMemoryNote(note.id);
      setMemory(await api.getAiMemory());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not delete the note.");
    }
  };

  /** Notes have no update endpoint — editing replaces the note (delete + add) and says so. */
  const saveNoteEdit = async (): Promise<void> => {
    if (editingNote === null || noteDraft.trim() === "") return;
    setError(null);
    try {
      await api.deleteAiMemoryNote(editingNote.id);
      await api.addAiMemoryNote(noteDraft.trim());
      setEditingNote(null);
      setMemory(await api.getAiMemory());
      setNotice("Note updated.");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not update the note.");
    }
  };

  const rebuild = async (): Promise<void> => {
    setError(null);
    try {
      const res = await api.rebuildAiMemory();
      setMemory(await api.getAiMemory());
      setNotice(`Website memory rebuilt: ${res.facts} facts, ${res.notes} notes.`);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not rebuild website memory.");
    }
  };

  const availableAgents = (agents ?? []).filter((a) => a.available);
  const selectedProvider = providers?.providers.find((p) => p.id === providers.selected);
  const pendingCount = approvals?.length ?? 0;

  return (
    <div>
      {error !== null ? <Alert tone="err" title="AI command centre" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Tabs
        ariaLabel="AI command centre sections"
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "composer", label: "Ask the AI", icon: "ai" },
          { id: "approvals", label: `Approvals${pendingCount > 0 ? ` (${pendingCount})` : ""}`, icon: "clipboardCheck" },
          { id: "agents", label: "Agents", icon: "users" },
          { id: "memory", label: "Website memory", icon: "database" },
          { id: "providers", label: "Providers & mode", icon: "settingsGear" },
        ]}
      />

      {tab === "composer" ? (
        <>
          <Card title="Ask the AI" sub="Write what you want in plain language. The reply is labelled as AI output, and nothing is published without your approval.">
            <div className="chat-composer">
              <Select label="Which agent should do it?" value={agentId} onChange={(e) => setAgentId(e.target.value)}>
                <option value="">Choose an agent…</option>
                {availableAgents.map((a) => (
                  <option key={a.id} value={a.id}>{a.name} — {a.description}</option>
                ))}
              </Select>
              {(agents ?? []).filter((a) => !a.available).length > 0 ? (
                <p className="hint">{(agents ?? []).filter((a) => !a.available).length} agent(s) are unavailable right now ({(agents ?? []).filter((a) => !a.available).map((a) => `${a.name}: ${a.unavailableReason ?? "no reason given"}`).join("; ")}).</p>
              ) : null}
              <div className="field">
                <label htmlFor="ai-input">What should it do?</label>
                <textarea id="ai-input" value={taskInput} onChange={(e) => setTaskInput(e.target.value)} rows={3} placeholder="e.g. Draft a post about our new opening hours" onKeyDown={(e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void runTask(); }} />
                <p className="hint">Press Ctrl+Enter (or Cmd+Enter) to send.</p>
              </div>
              <Select label="Mode for this task" value={taskMode} onChange={(e) => setTaskMode(e.target.value as "" | AiMode)}>
                <option value="">Site mode ({mode ?? "…"})</option>
                {MODE_ORDER.map((m) => (
                  <option key={m} value={m}>{MODE_LABELS[m]}</option>
                ))}
              </Select>
              <Button variant="primary" icon="send" disabled={working} onClick={() => void runTask()}>{working ? "Running…" : "Run task"}</Button>
            </div>
            {selectedTask?.result !== undefined ? (
              <div className="ai-result">
                <p><Badge tone="blue">AI output</Badge> <strong>{selectedTask.agent}</strong></p>
                <p>{selectedTask.result.summary}</p>
                {selectedTask.result.output ? <pre className="code-block">{selectedTask.result.output}</pre> : null}
                {selectedTask.result.proposals.length > 0 ? <p className="muted">Proposals waiting: {selectedTask.result.proposals.map((p) => p.summary).join("; ")}</p> : null}
                {selectedTask.result.applied.length > 0 ? <p className="muted">Applied automatically: {selectedTask.result.applied.map((a) => a.summary).join("; ")}</p> : null}
              </div>
            ) : null}
          </Card>

          <Card title="Tasks" sub={anyRunning ? "A task is running — this list refreshes every 2 seconds until it finishes." : "Latest tasks, newest first."}>
            {tasks === null ? (
              <Skeleton height={120} label="Loading tasks" />
            ) : tasks.length === 0 ? (
              <EmptyState icon="ai" title="No AI tasks yet" description="Ask the AI to do something above and it will appear here." />
            ) : (
              <ul className="row-list">
                {tasks.slice(0, 10).map((t) => (
                  <li key={t.id} className="task-row">
                    <button type="button" className="task-row-btn" onClick={() => setSelectedTask(t)} aria-label={`View task ${t.agent}, ${t.status}`}>
                      <span><strong>{t.agent}</strong> — {t.status} ({t.mode})</span>
                      <span className="muted">{t.result?.summary ?? t.input.slice(0, 100)}</span>
                      {(t.status === "running" || t.status === "pending") ? <Progress value={t.progress} max={100} label={`Task progress ${t.progress}%`} /> : null}
                      {t.status === "failed" && t.error ? <span className="field-error">{t.error}</span> : null}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </Card>
        </>
      ) : null}

      {tab === "approvals" ? (
        <Card title="Approvals" sub="Nothing applies until you approve it. Rejecting asks for a reason so the record is honest.">
          {approvals === null ? (
            <Skeleton height={120} label="Loading approvals" />
          ) : approvals.length === 0 ? (
            <EmptyState icon="clipboardCheck" title="Nothing waiting for approval" description="When an agent proposes something that needs a decision, it will appear here as a card." />
          ) : (
            <div className="approval-grid">
              {approvals.map((a) => (
                <article key={a.id} className="approval-card">
                  <div className="approval-head">
                    <strong>{a.agent}</strong>
                    <Badge tone={riskTone(a.action.risk)}>Risk: {a.action.risk}</Badge>
                  </div>
                  <p>{a.action.summary}</p>
                  <p className="muted">Type: {a.action.type} · Created {a.createdAt}</p>
                  <Input
                    label="Reason for rejecting (optional)"
                    value={rejectReason[a.id] ?? ""}
                    onChange={(e) => setRejectReason((prev) => ({ ...prev, [a.id]: e.target.value }))}
                    placeholder="e.g. Wrong tone for this post"
                  />
                  <div className="actions">
                    <Button variant="primary" icon="check" onClick={() => void decide(a, true)}>Approve &amp; apply</Button>
                    <Button variant="danger" icon="x" onClick={() => void decide(a, false)}>Reject</Button>
                  </div>
                </article>
              ))}
            </div>
          )}
        </Card>
      ) : null}

      {tab === "agents" ? (
        <Card title="Agents" sub="What each agent can do, in plain language. The email and social composers only draft — they never send anything.">
          {agents === null ? (
            <Skeleton height={160} label="Loading agents" />
          ) : agents.length === 0 ? (
            <EmptyState icon="users" title="No agents are registered" />
          ) : (
            <ul className="row-list">
              {agents.map((a) => (
                <li key={a.id} className="agent-row">
                  <span className="agent-icon" aria-hidden="true"><Icon name="ai" size={18} /></span>
                  <span className="action-body">
                    <strong>{a.name}</strong> <Badge tone={a.available ? "teal" : "grey"}>{a.available ? "Available" : "Unavailable"}</Badge> <Badge tone={riskTone(a.risk)}>Risk: {a.risk}</Badge>
                    <span className="muted">{a.description}</span>
                    <span className="muted">Can do: {a.capabilities.join("; ")}{a.requiresProvider ? " — needs an AI provider." : " — works without a provider."}</span>
                    {!a.available && a.unavailableReason ? <span className="field-error">Unavailable: {a.unavailableReason}</span> : null}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </Card>
      ) : null}

      {tab === "memory" ? (
        <Card title="Website memory" sub="Facts the agents use as context. Facts come from the site itself — rebuild to refresh them. Notes are yours: add, edit, delete." actions={<Button icon="refresh" onClick={() => void rebuild()}>Rebuild memory from the site</Button>}>
          {memory === null ? (
            <Skeleton height={160} label="Loading memory" />
          ) : (
            <>
              <h3>Facts ({memory.facts.length})</h3>
              {memory.facts.length === 0 ? (
                <EmptyState icon="database" title="No facts yet" description="Rebuild memory from the site to re-ingest title, tagline, posts, theme and extensions." />
              ) : (
                <ul className="row-list">
                  {memory.facts.map((f) => (
                    <li key={f.id}><code>{f.key}</code>: {f.value.length > 200 ? `${f.value.slice(0, 200)}…` : f.value}</li>
                  ))}
                </ul>
              )}
              <h3>Notes ({memory.notes.length})</h3>
              {memory.notes.length === 0 ? <p className="muted">No notes yet.</p> : (
                <ul className="row-list">
                  {memory.notes.map((n) => (
                    <li key={n.id}>
                      <span>{n.value}</span>
                      <span className="actions">
                        <Button size="small" icon="edit" onClick={() => { setEditingNote(n); setNoteDraft(n.value); }}>Edit</Button>
                        <Button size="small" variant="danger" icon="trash" onClick={() => void removeNote(n)}>Delete</Button>
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <div className="toolbar" style={{ marginTop: 14 }}>
                <Input label="Add a note for the agents" value={noteText} onChange={(e) => setNoteText(e.target.value)} placeholder="e.g. Our tone is plain and friendly." />
                <Button icon="plus" onClick={() => void addNote()}>Add note</Button>
              </div>
              <p className="hint">Memory never contains secrets.</p>
            </>
          )}
        </Card>
      ) : null}

      {tab === "providers" ? (
        <>
          <Card title="AI providers" sub="Workers AI is the default and only required provider — no API key, just the [ai] binding in wrangler.toml. OpenAI-compatible is an optional fallback that needs a secret key. The mock provider is for local development only (canned responses, no real AI).">
            {providers === null ? (
              <Skeleton height={140} label="Loading providers" />
            ) : providers.providers.length === 0 ? (
              <EmptyState icon="settingsGear" title="No AI providers are registered" />
            ) : (
              <ul className="row-list">
                {providers.providers.map((p) => (
                  <li key={p.id} className="provider-row">
                    <span className="action-body">
                      <strong>{p.name}</strong> {p.recommended ? <Badge tone="blue">Recommended</Badge> : null} {providers.selected === p.id ? <Badge tone="teal">Selected</Badge> : null} <Badge tone={p.configured ? "teal" : "amber"}>{p.configured ? "Configured" : "Not configured"}</Badge>
                      <span className="muted">{p.description}</span>
                      <span className="muted">{p.configuredDetail}</span>
                      {probe[p.id] ? <span>{probe[p.id]}</span> : null}
                    </span>
                    <span className="actions">
                      <Button size="small" disabled={providers.selected === p.id} onClick={() => void selectProvider(p.id)}>{providers.selected === p.id ? "Selected" : "Select"}</Button>
                      <Button size="small" icon="zap" onClick={() => void testProvider(p)}>Test</Button>
                    </span>
                  </li>
                ))}
              </ul>
            )}
            {selectedProvider !== undefined && !selectedProvider.configured ? (
              <Alert tone="warn" title="Selected provider is not configured">Provider-based agents will fail honestly with “not configured” until it is. Analysis agents (SEO, security, performance, …) still work.</Alert>
            ) : null}
          </Card>
          <Card title="Operating mode" sub={modeDescription}>
            <div className="actions" role="group" aria-label="Operating mode">
              {MODE_ORDER.map((m) => (
                <Button key={m} variant={mode === m ? "primary" : "secondary"} aria-pressed={mode === m} onClick={() => void setOperatingMode(m)}>{MODE_LABELS[m]}</Button>
              ))}
            </div>
            {MODE_ORDER.map((m) => (
              <p key={m} className="muted" style={{ margin: "0.4rem 0 0" }}><strong>{MODE_LABELS[m]}:</strong> {MODE_TEXT[m]}</p>
            ))}
          </Card>
        </>
      ) : null}

      {editingNote !== null ? (
        <Modal
          title="Edit memory note"
          onClose={() => setEditingNote(null)}
          actions={
            <>
              <Button onClick={() => setEditingNote(null)}>Cancel</Button>
              <Button variant="primary" onClick={() => void saveNoteEdit()}>Save note</Button>
            </>
          }
        >
          <Input label="Note" value={noteDraft} onChange={(e) => setNoteDraft(e.target.value)} hint="Notes have no edit endpoint — saving replaces the note (delete + add)." />
        </Modal>
      ) : null}
    </div>
  );
}
