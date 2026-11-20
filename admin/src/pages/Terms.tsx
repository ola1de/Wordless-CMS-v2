/**
 * Categories & Tags (§6.3).
 *
 * Create, rename inline, delete (blocked with the real count when
 * the term is in use). Tabs switch taxonomy without losing the page.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { Term } from "../lib/types";
import { Alert, Button, Card, EmptyState, Input, Skeleton, Table, Tabs } from "../components/elements";

interface TermList {
  terms: Term[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

export function TermsPage(): React.ReactElement {
  const [taxonomy, setTaxonomy] = useState<"category" | "tag">("category");
  const [list, setList] = useState<TermList | null>(null);
  const [page, setPage] = useState(1);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [description, setDescription] = useState("");
  const [editing, setEditing] = useState<Term | null>(null);
  const [editName, setEditName] = useState("");

  const load = useCallback(async () => {
    try {
      setList(await api.get<TermList>(`/api/terms?taxonomy=${taxonomy}&page=${page}&per_page=20`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load terms.");
    }
  }, [taxonomy, page]);

  useEffect(() => {
    void load();
  }, [load]);

  const create = async (): Promise<void> => {
    setError(null);
    try {
      await api.post<Term>("/api/terms", {
        name: name.trim(),
        slug: slug.trim() || undefined,
        taxonomy,
        description: description.trim() || undefined,
      });
      setNotice(`Created “${name.trim()}”.`);
      setName("");
      setSlug("");
      setDescription("");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Create failed.");
    }
  };

  const saveEdit = async (): Promise<void> => {
    if (!editing) return;
    try {
      await api.put<Term>(`/api/terms/${editing.termTaxonomyId}`, { name: editName.trim() });
      setNotice("Saved.");
      setEditing(null);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Save failed.");
    }
  };

  const remove = async (term: Term): Promise<void> => {
    if (term.count > 0) {
      setError(`“${term.name}” is used by ${term.count} post(s) and cannot be deleted yet.`);
      return;
    }
    if (!window.confirm(`Delete “${term.name}”?`)) return;
    try {
      await api.del(`/api/terms/${term.termTaxonomyId}`);
      setNotice("Deleted.");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed.");
    }
  };

  const label = taxonomy === "category" ? "Categories" : "Tags";
  const singular = taxonomy === "category" ? "category" : "tag";

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Categories & tags" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Tabs
        ariaLabel="Taxonomy"
        value={taxonomy}
        onChange={(id) => { setTaxonomy(id as "category" | "tag"); setPage(1); }}
        tabs={[
          { id: "category", label: "Categories", icon: "terms" },
          { id: "tag", label: "Tags", icon: "terms" },
        ]}
      />

      <Card title={`Add new ${singular}`}>
        <div className="form-grid">
          <Input label="Name" id="term-name" value={name} onChange={(e) => setName(e.target.value)} />
          <Input label="Slug (optional)" id="term-slug" value={slug} onChange={(e) => setSlug(e.target.value)} hint="Leave blank and the slug is made from the name." />
          <Input label="Description (optional)" id="term-desc" value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>
        <Button variant="primary" icon="plus" disabled={name.trim() === ""} onClick={() => void create()}>Add {singular}</Button>
      </Card>

      <Card title={label} sub={list !== null ? `${list.total} total` : undefined}>
        {list === null ? (
          <Skeleton height={160} label={`Loading ${label.toLowerCase()}`} />
        ) : list.terms.length === 0 ? (
          <EmptyState icon="terms" title={`No ${label.toLowerCase()} yet`} description={`Create the first ${singular} above.`} />
        ) : (
          <>
            <Table
              caption={label}
              columns={[
                { key: "name", label: "Name" },
                { key: "slug", label: "Slug" },
                { key: "count", label: "Posts" },
                { key: "actions", label: "Actions" },
              ]}
              rows={list.terms.map((term) =>
                editing?.termTaxonomyId === term.termTaxonomyId
                  ? {
                      name: <Input label="Term name" value={editName} onChange={(e) => setEditName(e.target.value)} />,
                      slug: `/${term.slug}`,
                      count: String(term.count),
                      actions: (
                        <span className="actions">
                          <Button size="small" variant="primary" onClick={() => void saveEdit()}>Save</Button>
                          <Button size="small" onClick={() => setEditing(null)}>Cancel</Button>
                        </span>
                      ),
                    }
                  : {
                      name: <span className="row-title">{term.name}</span>,
                      slug: `/${term.slug}`,
                      count: String(term.count),
                      actions: (
                        <span className="actions">
                          <Button size="small" icon="edit" onClick={() => { setEditing(term); setEditName(term.name); }}>Rename</Button>
                          <Button size="small" variant="danger" icon="trash" onClick={() => void remove(term)}>Delete</Button>
                        </span>
                      ),
                    },
              )}
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
