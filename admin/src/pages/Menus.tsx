/**
 * Menus (§6.3).
 *
 * Create menus, add custom-link items, remove items, assign a menu
 * to a theme location. Locations are theme slots (for example
 * “primary” for the main navigation); the public site renders the
 * menu assigned to each location.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { Menu, MenuItem } from "../lib/types";
import { Alert, Button, Card, EmptyState, Input, Select, Skeleton, Table } from "../components/elements";

export function MenusPage(): React.ReactElement {
  const [menus, setMenus] = useState<Menu[] | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [items, setItems] = useState<MenuItem[] | null>(null);
  const [locations, setLocations] = useState<Record<string, number>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [newMenuName, setNewMenuName] = useState("");
  const [itemTitle, setItemTitle] = useState("");
  const [itemUrl, setItemUrl] = useState("");
  const [locationName, setLocationName] = useState("primary");

  const loadMenus = useCallback(async () => {
    try {
      const [m, locs] = await Promise.all([
        api.get<Menu[]>("/api/menus"),
        api.get<Record<string, number>>("/api/menus/locations"),
      ]);
      setMenus(m);
      setLocations(locs);
      if (m.length > 0 && selectedId === null) setSelectedId(m[0]?.id ?? null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load menus.");
    }
  }, [selectedId]);

  useEffect(() => {
    void loadMenus();
  }, [loadMenus]);

  useEffect(() => {
    if (selectedId === null) {
      setItems(null);
      return;
    }
    let cancelled = false;
    api
      .get<MenuItem[]>(`/api/menus/${selectedId}/items`)
      .then((list) => {
        if (!cancelled) setItems(list);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load menu items.");
      });
    return () => {
      cancelled = true;
    };
  }, [selectedId]);

  const createMenu = async (): Promise<void> => {
    setError(null);
    try {
      const menu = await api.post<Menu>("/api/menus", { name: newMenuName.trim() });
      setNotice(`Menu “${menu.name}” created.`);
      setNewMenuName("");
      await loadMenus();
      setSelectedId(menu.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Create failed.");
    }
  };

  const addItem = async (): Promise<void> => {
    if (selectedId === null) return;
    setError(null);
    try {
      await api.post<MenuItem>(`/api/menus/${selectedId}/items`, {
        title: itemTitle.trim(),
        type: "custom",
        url: itemUrl.trim(),
      });
      setNotice("Menu item added.");
      setItemTitle("");
      setItemUrl("");
      setItems(await api.get<MenuItem[]>(`/api/menus/${selectedId}/items`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Add item failed.");
    }
  };

  const removeItem = async (itemId: number): Promise<void> => {
    if (selectedId === null) return;
    try {
      await api.del(`/api/menus/${selectedId}/items/${itemId}`);
      setItems(await api.get<MenuItem[]>(`/api/menus/${selectedId}/items`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Remove failed.");
    }
  };

  const setLocation = async (): Promise<void> => {
    if (selectedId === null) return;
    setError(null);
    try {
      const updated = await api.post<Record<string, number>>("/api/menus/locations", {
        location: locationName.trim().toLowerCase(),
        menuId: selectedId,
      });
      setLocations(updated);
      setNotice(`Menu assigned to location “${locationName.trim().toLowerCase()}”.`);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Assign failed.");
    }
  };

  const selected = menus?.find((m) => m.id === selectedId) ?? null;

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Menus" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Card title="Create a menu">
        <div className="form-grid">
          <Input label="Menu name" id="menu-name" value={newMenuName} onChange={(e) => setNewMenuName(e.target.value)} />
        </div>
        <Button variant="primary" icon="plus" disabled={newMenuName.trim() === ""} onClick={() => void createMenu()}>Create menu</Button>
      </Card>

      {menus === null ? (
        <Skeleton height={160} label="Loading menus" />
      ) : menus.length === 0 ? (
        <EmptyState icon="menus" title="No menus yet" description="Create one above." />
      ) : (
        <>
          <div className="toolbar">
            <Select label="Menu" value={String(selectedId ?? "")} onChange={(e) => setSelectedId(Number(e.target.value))} aria-label="Select menu">
              {menus.map((m) => (
                <option key={m.id} value={m.id}>{m.name}</option>
              ))}
            </Select>
            <span className="muted">
              Locations:{" "}
              {Object.keys(locations).length === 0
                ? "none assigned"
                : Object.entries(locations)
                    .map(([loc, menuId]) => `${loc} → ${menus.find((m) => m.id === menuId)?.name ?? menuId}`)
                    .join(", ")}
            </span>
          </div>

          <Card title={`Items — ${selected?.name ?? ""}`}>
            {items === null ? (
              <Skeleton height={120} label="Loading menu items" />
            ) : items.length === 0 ? (
              <EmptyState icon="menus" title="No items in this menu yet" description="Add a custom link below." />
            ) : (
              <Table
                caption={`Items in ${selected?.name ?? "menu"}`}
                columns={[
                  { key: "title", label: "Title" },
                  { key: "url", label: "URL" },
                  { key: "type", label: "Type" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={items.map((item) => ({
                  title: <span className="row-title">{item.title}</span>,
                  url: item.url,
                  type: item.type,
                  actions: <Button size="small" variant="danger" icon="trash" onClick={() => void removeItem(item.id)}>Remove</Button>,
                }))}
              />
            )}
            <h3>Add a custom link</h3>
            <div className="form-grid">
              <Input label="Title" id="item-title" value={itemTitle} onChange={(e) => setItemTitle(e.target.value)} />
              <Input label="URL" id="item-url" value={itemUrl} onChange={(e) => setItemUrl(e.target.value)} placeholder="https://example.com/page or /about" />
            </div>
            <Button variant="primary" icon="plus" disabled={itemTitle.trim() === "" || itemUrl.trim() === ""} onClick={() => void addItem()}>Add item</Button>
          </Card>

          <Card title="Assign to a location" sub="Locations are theme slots (for example “primary” for the main navigation). The public site renders the menu assigned to each location.">
            <div className="form-grid">
              <Input label="Location name" id="location-name" value={locationName} onChange={(e) => setLocationName(e.target.value)} />
            </div>
            <Button onClick={() => void setLocation()}>Assign “{selected?.name}” to this location</Button>
          </Card>
        </>
      )}
    </div>
  );
}
