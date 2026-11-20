/**
 * Sites (§12 Multisite) — one install, many sites. Lists every site,
 * creates a new one, archives/unarchives it, and deletes it (which
 * wipes ALL of that site's content — the confirm step says so).
 * The public title of each site is its own `blogname` option, shown
 * here and editable inline: changing one site's title never touches
 * another site. "Switch" makes the site the admin acts on (the
 * header switcher follows) and reloads so every page re-fetches
 * under the new site.
 */
import { useEffect, useState } from "react";
import { api, getCurrentSiteId, setCurrentSite } from "../lib/api";
import type { Site } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Skeleton, Table } from "../components/elements";

function slugHint(slug: string): string {
  if (slug === "") return "";
  if (!/^[a-z0-9][a-z0-9-]{0,62}$/.test(slug)) {
    return "Lowercase letters, digits and hyphens, 1–63 characters.";
  }
  return "";
}

export function SitesPage(props: { onSitesChanged: () => void }): React.ReactElement {
  const [sites, setSites] = useState<Site[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [slug, setSlug] = useState("");
  const [title, setTitle] = useState("");
  const [creating, setCreating] = useState(false);
  const [busyId, setBusyId] = useState<number | null>(null);
  // Per-site public title (`blogname` option), keyed by site id.
  const [blogname, setBlogname] = useState<Record<number, string>>({});
  const [blognameBusy, setBlognameBusy] = useState<number | null>(null);
  const currentId = getCurrentSiteId() ?? 1;

  const load = (): void => {
    api
      .listSites()
      .then((list) => {
        setSites(list);
        // Pull each site's public title in parallel; a missing option
        // just falls back to the site's own title.
        void Promise.all(
          list.map((s) =>
            api
              .getSiteOption<string>(s.id, "blogname")
              .then((r) => {
                if (typeof r.value === "string" && r.value !== "") {
                  setBlogname((prev) => ({ ...prev, [s.id]: r.value }));
                }
              })
              .catch(() => undefined),
          ),
        );
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Failed to load sites.");
      });
  };

  useEffect(load, []);

  const create = (): void => {
    const hint = slugHint(slug.trim().toLowerCase());
    if (hint !== "") {
      setError(hint);
      return;
    }
    if (title.trim() === "") {
      setError("Give the new site a title.");
      return;
    }
    setCreating(true);
    setError(null);
    setNotice(null);
    api
      .createSite(slug.trim().toLowerCase(), title.trim())
      .then((site) => {
        setNotice(`Site "${site.title}" created — its address is /${site.slug}/ and it starts with the network defaults.`);
        setSlug("");
        setTitle("");
        setSites((prev) => (prev === null ? [site] : [...prev, site]));
        props.onSitesChanged();
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Failed to create the site.");
      })
      .finally(() => setCreating(false));
  };

  const setStatus = (site: Site, status: "public" | "archived"): void => {
    setBusyId(site.id);
    setError(null);
    api
      .updateSite(site.id, { status })
      .then((updated) => {
        setSites((prev) => prev?.map((s) => (s.id === updated.id ? updated : s)) ?? prev);
        props.onSitesChanged();
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Failed to update the site.");
      })
      .finally(() => setBusyId(null));
  };

  const remove = (site: Site): void => {
    if (site.id === 1) return; // The main site can never be deleted.
    const confirmed = window.confirm(
      `Delete "${site.title}" (${site.slug})?\n\nThis permanently wipes ALL of the site's content — posts, pages, terms, comments, media records and redirects. Users and installed themes are shared and stay. This cannot be undone.`,
    );
    if (!confirmed) return;
    setBusyId(site.id);
    setError(null);
    api
      .deleteSite(site.id)
      .then((r) => {
        setNotice(`Site "${site.title}" deleted — ${r.deletedPosts} post row(s) removed.`);
        setSites((prev) => prev?.filter((s) => s.id !== site.id) ?? prev);
        if (currentId === site.id) {
          setCurrentSite(null);
          window.location.reload();
          return;
        }
        props.onSitesChanged();
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Failed to delete the site.");
      })
      .finally(() => setBusyId(null));
  };

  const switchSite = (site: Site): void => {
    setCurrentSite(site.id === 1 ? null : site.id);
    // Reload so settings, posts, media and every other page re-fetch
    // under the new site instead of showing the old site's data.
    window.location.reload();
  };

  const saveBlogname = (site: Site): void => {
    const value = (blogname[site.id] ?? "").trim();
    if (value === "") {
      setError("The public title cannot be empty.");
      return;
    }
    setBlognameBusy(site.id);
    setError(null);
    api
      .setSiteOption(site.id, "blogname", value)
      .then(() => {
        setNotice(`Public title for "${site.title}" set to "${value}". Other sites are untouched.`);
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Failed to save the public title.");
      })
      .finally(() => setBlognameBusy(null));
  };

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Sites" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" title="Sites" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}
      <Card
        title="Create a site"
        sub="A new site starts with the network defaults and no content. Its address is example.com/<slug>/ — path-based routing needs no setup; subdomain routing is a Workers-routes option."
      >
        <div style={{ display: "flex", gap: 12, flexWrap: "wrap", alignItems: "end" }}>
          <div style={{ minWidth: 180 }}>
            <Input label="Slug" value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="e.g. blog" />
          </div>
          <div style={{ minWidth: 220, flex: 1 }}>
            <Input label="Title" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="e.g. Company Blog" />
          </div>
          <Button onClick={create} disabled={creating || slug.trim() === "" || title.trim() === ""}>
            {creating ? "Creating…" : "Create site"}
          </Button>
        </div>
        {slug !== "" && slugHint(slug.trim().toLowerCase()) !== "" ? (
          <p style={{ margin: "8px 0 0" }}>{slugHint(slug.trim().toLowerCase())}</p>
        ) : null}
      </Card>
      {sites === null ? (
        <Skeleton height={180} label="Loading sites" />
      ) : sites.length === 0 ? (
        <EmptyState title="No sites" description="The network should always have at least the main site — if this is empty, the install was not seeded." />
      ) : (
        <Card title={`Sites (${sites.length})`} sub="Users, installed themes and code are shared across the network. Content, settings and active theme are per-site.">
          <Table
            caption="Sites"
            columns={[
              { key: "site", label: "Site" },
              { key: "address", label: "Address" },
              { key: "status", label: "Status" },
              { key: "title", label: "Public title" },
              { key: "actions", label: "Actions" },
            ]}
            rows={sites.map((site) => ({
              site: (
                <span>
                  <strong>{site.title}</strong>
                  {site.id === currentId ? <Badge tone="teal"> current</Badge> : null}
                  <br />
                  <span style={{ opacity: 0.7 }}>#{site.id} · created {site.createdAt.slice(0, 10)}</span>
                </span>
              ),
              address: <code>{site.slug === "" ? "/" : `/${site.slug}/`}</code>,
              status: (
                <Badge tone={site.status === "public" ? "teal" : "amber"}>{site.status}</Badge>
              ),
              title: (
                <span style={{ display: "flex", gap: 6, alignItems: "center" }}>
                  <input
                    aria-label={`Public title for ${site.title}`}
                    value={blogname[site.id] ?? site.title}
                    onChange={(e) => setBlogname((prev) => ({ ...prev, [site.id]: e.target.value }))}
                    style={{ minWidth: 140 }}
                  />
                  <Button
                    variant="ghost"
                    onClick={() => saveBlogname(site)}
                    disabled={blognameBusy === site.id || (blogname[site.id] ?? site.title).trim() === ""}
                  >
                    Save
                  </Button>
                </span>
              ),
              actions: (
                <span style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                  {site.id !== currentId ? (
                    <Button variant="ghost" onClick={() => switchSite(site)} disabled={busyId === site.id}>
                      Switch
                    </Button>
                  ) : null}
                  {site.status === "public" ? (
                    <Button variant="ghost" onClick={() => setStatus(site, "archived")} disabled={busyId === site.id}>
                      Archive
                    </Button>
                  ) : (
                    <Button variant="ghost" onClick={() => setStatus(site, "public")} disabled={busyId === site.id}>
                      Unarchive
                    </Button>
                  )}
                  {site.id !== 1 ? (
                    <Button variant="danger" onClick={() => remove(site)} disabled={busyId === site.id}>
                      Delete
                    </Button>
                  ) : null}
                </span>
              ),
            }))}
          />
          <p style={{ margin: "12px 0 0", opacity: 0.75 }}>
            Archived sites still resolve for administrators but their public pages return 404. Deleting the main site
            is refused.
          </p>
        </Card>
      )}
    </div>
  );
}
