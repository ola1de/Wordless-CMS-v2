/**
 * Visual Builder (P11) — layout model and helpers.
 *
 * A layout IS Elementor-compatible JSON: an array of sections, each
 * holding columns, each holding widgets — the same `_elementor_data`
 * the P3 renderer displays. The builder edits that JSON directly, so
 * there is no second format to drift: what the builder saves is what
 * the public site renders. An Elementor layout imported from
 * WordPress opens here with its sections and widgets intact; widget
 * types this renderer does not know are shown as unsupported blocks
 * (kept in the layout, never silently dropped).
 */
import type { IconName } from "../components/icons";

export type BuilderNodeType = "section" | "column" | "widget";

export interface BuilderNode {
  id: string;
  elType: BuilderNodeType;
  /** Widget nodes only: the widgetType from the palette. */
  widgetType?: string;
  settings?: Record<string, unknown>;
  elements?: BuilderNode[];
}

export type BuilderLayout = BuilderNode[];

export interface PaletteField {
  key: string;
  label: string;
  kind: "text" | "textarea" | "url" | "number" | "checkbox" | "select" | "list-text" | "list-object" | "rows";
  options: string[];
  placeholder: string;
}

export interface PaletteItem {
  type: string;
  label: string;
  icon: IconName;
  description: string;
  sourceExport: string;
  fields: PaletteField[];
}

export interface PaletteIcon {
  name: IconName;
  group: string;
}

export interface PaletteDevice {
  id: string;
  label: string;
  width: number;
}

export interface ExtensionWidget {
  extensionSlug: string;
  type: string;
  label: string;
  icon: IconName;
  description: string;
  fields: unknown;
  rendersPublicly: boolean;
  extensionStatus?: string;
}

export interface BuilderPalette {
  widgets: PaletteItem[];
  elements: PaletteItem[];
  icons: PaletteIcon[];
  devices: PaletteDevice[];
  extensionWidgets: ExtensionWidget[];
}

export interface PreviewResult {
  html: string;
  unsupported: string[];
  device: string;
  deviceWidth: number;
}

let idCounter = 0;
export function newNodeId(): string {
  idCounter += 1;
  return `wl-${Date.now().toString(36)}-${idCounter}-${Math.floor(Math.random() * 1e6).toString(36)}`;
}

export function createSection(columns = 1): BuilderNode {
  const cols: BuilderNode[] = [];
  for (let i = 0; i < Math.max(1, Math.min(4, columns)); i++) {
    cols.push({ id: newNodeId(), elType: "column", elements: [] });
  }
  return { id: newNodeId(), elType: "section", elements: cols };
}

export function createWidgetNode(widgetType: string, settings: Record<string, unknown> = {}): BuilderNode {
  return { id: newNodeId(), elType: "widget", widgetType, settings };
}

export function createEmptyLayout(): BuilderLayout {
  return [createSection(1)];
}

/** Find a node anywhere in the layout (sections, columns, widgets). */
export function findNode(layout: BuilderLayout, id: string): BuilderNode | null {
  for (const node of layout) {
    if (node.id === id) return node;
    if (node.elements) {
      const found = findNode(node.elements, id);
      if (found) return found;
    }
  }
  return null;
}

/** Map over every node, returning a new layout (immutable update). */
export function mapNodes(layout: BuilderLayout, fn: (n: BuilderNode) => BuilderNode): BuilderLayout {
  return layout.map((node) => {
    const next = fn(node);
    if (next.elements) {
      return { ...next, elements: mapNodes(next.elements, fn) };
    }
    return next;
  });
}

/** Append a widget to a column (by column id). */
export function addWidgetToColumn(layout: BuilderLayout, columnId: string, widget: BuilderNode): BuilderLayout {
  return mapNodes(layout, (node) => {
    if (node.id === columnId && node.elType === "column") {
      return { ...node, elements: [...(node.elements ?? []), widget] };
    }
    return node;
  });
}

/** Remove a node (and its children) from the layout. */
export function removeNode(layout: BuilderLayout, id: string): BuilderLayout {
  return layout
    .filter((node) => node.id !== id)
    .map((node) => (node.elements ? { ...node, elements: removeNode(node.elements, id) } : node));
}

/** Move a widget one step within its parent, or to a neighbouring parent. */
export function moveWidget(layout: BuilderLayout, widgetId: string, direction: -1 | 1): BuilderLayout {
  const moveIn = (nodes: BuilderNode[]): BuilderNode[] => {
    const idx = nodes.findIndex((n) => n.id === widgetId);
    if (idx >= 0) {
      const next = idx + direction;
      if (next < 0 || next >= nodes.length) return nodes;
      const copy = [...nodes];
      const item = copy.splice(idx, 1)[0];
      if (item === undefined) return nodes;
      copy.splice(next, 0, item);
      return copy;
    }
    return nodes.map((n) =>
      n.elements ? { ...n, elements: moveIn(n.elements) } : n,
    );
  };
  return moveIn(layout);
}

/**
 * Move a widget into a target column, before a given sibling (or at
 * the end when `beforeId` is null). Used by drag-and-drop; a no-op
 * when the widget or the column is not found.
 */
export function moveWidgetTo(
  layout: BuilderLayout,
  widgetId: string,
  targetColumnId: string,
  beforeId: string | null,
): BuilderLayout {
  const widget = findNode(layout, widgetId);
  if (!widget || widget.elType !== "widget") return layout;
  const without = removeNode(layout, widgetId);
  return mapNodes(without, (node) => {
    if (node.id === targetColumnId && node.elType === "column") {
      const children = [...(node.elements ?? [])];
      const at = beforeId ? children.findIndex((c) => c.id === beforeId) : -1;
      if (at >= 0) children.splice(at, 0, widget);
      else children.push(widget);
      return { ...node, elements: children };
    }
    return node;
  });
}

/** Replace a node's settings wholesale (the settings panel commits). */
export function updateNodeSettings(
  layout: BuilderLayout,
  id: string,
  settings: Record<string, unknown>,
): BuilderLayout {
  return mapNodes(layout, (node) => (node.id === id ? { ...node, settings } : node));
}

/** Read a dotted setting (`image.url`, `space.size`). */
export function getSetting(node: BuilderNode, key: string): unknown {
  const parts = key.split(".");
  let cur: unknown = node.settings ?? {};
  for (const part of parts) {
    if (typeof cur !== "object" || cur === null) return undefined;
    cur = (cur as Record<string, unknown>)[part];
  }
  return cur;
}

/** Set a dotted setting, returning a new settings object. */
export function setSetting(
  settings: Record<string, unknown>,
  key: string,
  value: unknown,
): Record<string, unknown> {
  const parts = key.split(".");
  const next: Record<string, unknown> = { ...settings };
  let cur: Record<string, unknown> = next;
  for (let i = 0; i < parts.length - 1; i++) {
    const part = parts[i];
    if (part === undefined) return next;
    const child =
      typeof cur[part] === "object" && cur[part] !== null
        ? { ...(cur[part] as Record<string, unknown>) }
        : {};
    cur[part] = child;
    cur = child;
  }
  const last = parts[parts.length - 1];
  if (last === undefined) return next;
  cur[last] = value;
  return next;
}

/** Parse a stored `_elementor_data` meta value into a layout. */
export function parseLayout(raw: string | null | undefined): { layout: BuilderLayout; warnings: string[] } {
  if (!raw || !raw.trim()) return { layout: createEmptyLayout(), warnings: [] };
  try {
    const data: unknown = JSON.parse(raw);
    if (!Array.isArray(data) || data.length === 0) {
      return { layout: createEmptyLayout(), warnings: ["The stored layout was empty, so a blank section was started."] };
    }
    return { layout: data as BuilderLayout, warnings: [] };
  } catch {
    return { layout: createEmptyLayout(), warnings: ["The stored layout could not be read, so a blank section was started."] };
  }
}

/** The widget types this renderer knows, from a palette payload. */
export function knownWidgetTypes(palette: BuilderPalette | null): Set<string> {
  const out = new Set<string>();
  if (!palette) return out;
  for (const item of palette.widgets) out.add(item.type);
  for (const item of palette.elements) out.add(item.type);
  for (const item of palette.extensionWidgets) out.add(item.type);
  return out;
}

/** Widget types in a layout that no palette entry covers. */
export function unsupportedInLayout(layout: BuilderLayout, known: Set<string>): string[] {
  const found: string[] = [];
  const walk = (nodes: BuilderNode[]): void => {
    for (const node of nodes) {
      if (node.elType === "widget" && node.widgetType && !known.has(node.widgetType)) {
        if (!found.includes(node.widgetType)) found.push(node.widgetType);
      }
      if (node.elements) walk(node.elements);
    }
  };
  walk(layout);
  return found;
}

/** Default settings for a palette item (empty strings / first option / false). */
export function defaultSettings(item: PaletteItem): Record<string, unknown> {
  const settings: Record<string, unknown> = {};
  for (const field of item.fields) {
    const value: unknown =
      field.kind === "checkbox"
        ? false
        : field.kind === "number"
          ? field.placeholder === ""
            ? 0
            : Number(field.placeholder) || 0
          : field.kind === "select"
            ? field.options[0] ?? ""
            : field.kind === "list-text" || field.kind === "list-object" || field.kind === "rows"
              ? []
              : field.placeholder;
    settings[field.key] = value;
  }
  // Un-flatten dotted keys (image.url → { image: { url } }).
  let out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(settings)) {
    out = setSetting(out, key, value);
  }
  return out;
}
