/**
 * Conversion (§6.3).
 *
 * Upload → live progress → per-file verdicts in plain language →
 * generated code with syntax highlighting → "what this means"
 * summary. No jargon without a translation: every verdict says what
 * was converted, what was not, and why, in words a site owner can
 * act on. Converted code is stored, never executed.
 */
import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import type { ConversionJob, ExtensionRecord } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Progress, Skeleton, Stepper, Table } from "../components/elements";

const KEYWORDS = new Set(["const", "let", "var", "function", "return", "if", "else", "for", "while", "import", "export", "from", "default", "new", "class", "extends", "typeof", "await", "async", "try", "catch", "throw", "switch", "case", "break", "continue", "do", "of", "in", "this", "true", "false", "null", "undefined", "type", "interface", "string", "number", "boolean", "void"]);

/** Tiny TypeScript highlighter: keywords, strings, comments, numbers. No dependencies. */
function highlight(code: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  const re = /(\/\/[^\n]*|\/\*[\s\S]*?\*\/)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`)|\b(\d+(?:\.\d+)?)\b|\b([A-Za-z_$][\w$]*)\b/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let key = 0;
  while ((m = re.exec(code)) !== null) {
    if (m.index > last) nodes.push(<span key={key++}>{code.slice(last, m.index)}</span>);
    const [full, comment, str, num, word] = m;
    if (comment !== undefined) nodes.push(<span key={key++} className="tok-comment">{full}</span>);
    else if (str !== undefined) nodes.push(<span key={key++} className="tok-string">{full}</span>);
    else if (num !== undefined) nodes.push(<span key={key++} className="tok-number">{full}</span>);
    else if (word !== undefined && KEYWORDS.has(word)) nodes.push(<span key={key++} className="tok-keyword">{full}</span>);
    else nodes.push(<span key={key++}>{full}</span>);
    last = m.index + full.length;
  }
  if (last < code.length) nodes.push(<span key={key++}>{code.slice(last)}</span>);
  return nodes;
}

export function ConversionPage(): React.ReactElement {
  const [jobs, setJobs] = useState<ConversionJob[] | null>(null);
  const [extensions, setExtensions] = useState<ExtensionRecord[] | null>(null);
  const [selected, setSelected] = useState<ConversionJob | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [showCode, setShowCode] = useState(false);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const load = async (): Promise<void> => {
    try {
      const [list, exts] = await Promise.all([api.listConversionJobs(), api.listExtensions()]);
      setJobs(list.jobs);
      setExtensions(exts);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load conversion jobs.");
    }
  };

  useEffect(() => {
    let cancelled = false;
    void load().then(() => { if (!cancelled) { /* load() sets state itself */ } });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const anyActive = jobs?.some((j) => j.status === "pending" || j.status === "running") ?? false;
  useEffect(() => {
    if (!anyActive) return;
    const timer = window.setInterval(() => { void load(); }, 2000);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [anyActive]);

  useEffect(() => {
    if (selected === null || jobs === null) return;
    const fresh = jobs.find((j) => j.id === selected.id);
    if (fresh !== undefined && fresh.updatedAt !== selected.updatedAt) setSelected(fresh);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jobs]);

  const startJob = async (start: () => Promise<ConversionJob>): Promise<void> => {
    setError(null);
    setWorking(true);
    try {
      const job = await start();
      setSelected(job);
      setShowCode(false);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not start the conversion job.");
    } finally {
      setWorking(false);
    }
  };

  const onFile = async (file: File | undefined): Promise<void> => {
    if (!file) return;
    if (!file.name.toLowerCase().endsWith(".php")) {
      setError("Only .php files can be converted.");
      return;
    }
    await startJob(() => api.uploadPhpFile(file));
  };

  const report = selected?.report;
  const helloDolly = extensions?.find((e) => e.slug === "hello-dolly");
  const stepState = (idx: number): "done" | "current" | "todo" => {
    if (selected === undefined || selected === null) return idx === 0 ? "current" : "todo";
    if (selected.status === "done" || selected.status === "failed") return "done";
    if (idx === 0) return "done";
    if (idx === 1) return "current";
    return "todo";
  };

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Conversion" onDismiss={() => setError(null)}>{error}</Alert> : null}

      <Card title="Convert a WordPress plugin" sub="The converter translates PHP source to a Wordless extension module — statically, without ever running the PHP. Uploads are capped at 1 MB. Anything the sandbox blocks (eval, shell commands, dynamic includes, unserialize) fails the job with a reason instead of a partial success. Converted code runs only through the Wordless extension API.">
        <Stepper steps={[
          { label: "Upload", state: stepState(0) },
          { label: "Converting", state: stepState(1) },
          { label: "Verdicts", state: stepState(2) },
          { label: "Generated code", state: stepState(3) },
        ]} />
        <div className="toolbar">
          <input
            ref={fileRef}
            type="file"
            accept=".php"
            aria-label="PHP file to convert"
            onChange={(e) => { void onFile(e.target.files?.[0]); e.target.value = ""; }}
          />
          <Button variant="primary" icon="upload" disabled={working} onClick={() => fileRef.current?.click()}>{working ? "Converting…" : "Upload a .php file"}</Button>
          <Button icon="sparkles" disabled={working} onClick={() => void startJob(() => api.startFixtureJob("hello-dolly"))}>Convert the Hello Dolly fixture</Button>
        </div>
        <p className="hint">Hello Dolly is the bundled WordPress plugin — a known-good first conversion.</p>
        {selected !== null && (selected.status === "pending" || selected.status === "running") ? (
          <Progress value={selected.progress} max={100} label={`Converting ${selected.filename}: ${selected.progress}%`} />
        ) : null}
      </Card>

      <Card title="Installed extensions" sub="Extensions converted and stored here. Stored code runs once it is loaded through the sandboxed extension runtime (P6) — load it from the Plugins page; a plugin with blocked files cannot load, and the reason is shown there.">
        {extensions === null ? (
          <Skeleton height={100} label="Loading extensions" />
        ) : extensions.length === 0 ? (
          <EmptyState icon="conversion" title="No extensions installed yet" description="Convert the Hello Dolly fixture to install the first one." />
        ) : (
          <Table
            caption="Installed extensions"
            columns={[
              { key: "name", label: "Extension" },
              { key: "version", label: "Version" },
              { key: "status", label: "Status" },
              { key: "source", label: "Source" },
            ]}
            rows={extensions.map((ext) => ({
              name: <span><span className="row-title">{ext.name}</span> <span className="muted">({ext.slug})</span></span>,
              version: ext.version,
              status: <Badge tone={ext.status === "active" ? "teal" : "grey"}>{ext.status}</Badge>,
              source: ext.source ?? "—",
            }))}
          />
        )}
        {helloDolly ? (
          <p className="hint">Hello Dolly is installed and active: its admin_notices hook returns a lyric line and admin_head returns its stylesheet. See examples/hello-dolly/ in the repo for the generated module.</p>
        ) : null}
      </Card>

      <Card title="Conversion jobs" sub="Select a job to see its report.">
        {jobs === null ? (
          <Skeleton height={120} label="Loading conversion jobs" />
        ) : jobs.length === 0 ? (
          <EmptyState icon="conversion" title="No conversion jobs yet" description="Upload a .php file above to start the first one." />
        ) : (
          <Table
            caption="Conversion jobs"
            columns={[
              { key: "file", label: "File" },
              { key: "status", label: "Status" },
              { key: "progress", label: "Progress" },
              { key: "started", label: "Started" },
            ]}
            rows={jobs.map((job) => ({
              file: (
                <Button variant="ghost" size="small" onClick={() => { setSelected(job); setShowCode(false); }}>
                  {job.filename}{job.fixture ? ` (fixture: ${job.fixture})` : ""}
                </Button>
              ),
              status: <Badge tone={job.status === "done" ? "teal" : job.status === "failed" ? "red" : "amber"}>{job.status}</Badge>,
              progress: `${job.progress}%`,
              started: new Date(job.createdAt).toLocaleString(),
            }))}
          />
        )}
      </Card>

      {selected !== null ? (
        <Card title={`Job report — ${selected.filename}`} sub={`Status: ${selected.status}`}>
          {selected.error ? <Alert tone="err" title="Job failed">{selected.error}</Alert> : null}
          {report ? (
            <>
              <p className="muted">{report.source.lines} lines · {report.ast.nodes} AST nodes · {report.ast.functions} functions · {report.ast.classes} classes · {report.ast.hooks} hooks · {report.durationMs} ms</p>

              <Alert tone={report.unconverted.length === 0 ? "ok" : report.converted.functions.length > 0 ? "warn" : "err"} title="What this means">
                {report.unconverted.length === 0
                  ? `Everything in this file converted cleanly — ${report.converted.functions.length} function(s) and ${report.converted.hooks.length} hook(s) are ready as a Wordless extension. The code is stored, not running yet.`
                  : report.converted.functions.length > 0
                    ? `${report.converted.functions.length} function(s) converted and usable; ${report.unconverted.length} piece(s) could not be converted and are listed below with the reason. The extension will work without the unconverted pieces only if nothing depends on them — check the list before activating.`
                    : `Nothing in this file could be converted safely. The reasons below explain why — usually a blocked capability (eval, shell, dynamic include) or PHP the converter does not understand yet. The file is not installed as an extension.`}
              </Alert>

              <h3>Verdicts</h3>
              <Table
                caption="Conversion verdicts"
                columns={[
                  { key: "item", label: "What" },
                  { key: "verdict", label: "Verdict" },
                  { key: "reason", label: "Why, in plain language" },
                ]}
                rows={[
                  ...report.converted.functions.map((fn) => ({
                    item: <code>{fn}</code>,
                    verdict: <Badge tone="teal">Converted</Badge>,
                    reason: "Translated to TypeScript and stored as part of the extension.",
                  })),
                  ...report.converted.hooks.map((h) => ({
                    item: <code>{h.kind} “{h.tag}” → {h.functionName}</code>,
                    verdict: <Badge tone="teal">Converted</Badge>,
                    reason: "The WordPress hook is now a Wordless hook with the same name.",
                  })),
                  ...report.unconverted.map((u, i) => ({
                    item: <code key={i}>{u.kind}{u.name ? ` ${u.name}` : ""}</code>,
                    verdict: <Badge tone="red">Not converted</Badge>,
                    reason: `${u.reason} (line ${u.line}). This piece is left out — the rest of the file still converts.`,
                  })),
                ]}
                empty={<EmptyState icon="conversion" title="Nothing converted and nothing flagged" description="The file produced no functions, hooks, or unconverted pieces." />}
              />

              {report.transformed.length > 0 ? (
                <>
                  <h3>Transformed</h3>
                  <ul>
                    {report.transformed.map((t, i) => (
                      <li key={i}><strong>{t.what}:</strong> {t.note}</li>
                    ))}
                  </ul>
                </>
              ) : null}
              {report.warnings.length > 0 ? (
                <>
                  <h3>Warnings ({report.warnings.length})</h3>
                  <ul>
                    {report.warnings.map((w, i) => (
                      <li key={i}>{w}</li>
                    ))}
                  </ul>
                </>
              ) : null}
              <p className="muted">Extension: <code>{report.extension.name}</code> {report.extension.version} (slug <code>{report.extension.slug}</code>) · Output: <code>{report.output.filename}</code> ({report.output.bytes} bytes)</p>
              <Button size="small" icon={showCode ? "chevronDown" : "chevronRight"} onClick={() => setShowCode((v) => !v)}>{showCode ? "Hide generated code" : "Show generated code"}</Button>
              {showCode ? (
                <pre className="code-block" aria-label="Generated TypeScript code">{highlight(report.output.code)}</pre>
              ) : null}
            </>
          ) : selected.status === "done" ? (
            <p className="muted">The job finished but no report was stored.</p>
          ) : null}
        </Card>
      ) : null}
    </div>
  );
}
