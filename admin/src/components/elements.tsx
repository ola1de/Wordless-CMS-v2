/**
 * Wordless v2.0 shared element library (P4 admin foundation, doc §6
 * principle 9: "a real element library, not one-off styling").
 *
 * One look, one behaviour, one dark mode per element. Every style comes
 * from the token system in styles.css — no element hard-codes a colour
 * outside the token vars. Every element is keyboard-operable with visible
 * focus (:focus-visible in styles.css) and aria labels where icon-only.
 *
 * This is the library wave 2 migrates the 20 pages onto, and the P11
 * Visual Builder palette renders from (§6 / doc v1.4: the palette IS the
 * library, not a copy). Existing pages still use components/ui.tsx for
 * now; that file is untouched by P4 and wave 2 consolidates the two.
 * Do not add a one-off button, input, or card to a page — use this file.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes } from "react";
import { Icon } from "./icons";
import type { IconName } from "./icons";

/* ------------------------------------------------------------------ */
/* Button                                                              */
/* ------------------------------------------------------------------ */

type ButtonBase = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label"> & {
  variant?: "primary" | "secondary" | "ghost" | "danger" | "dark";
  size?: "small" | "medium" | "large";
  icon?: IconName;
};

/** Icon-only buttons MUST carry an aria-label (screen-reader name). */
export type ButtonProps =
  | (ButtonBase & { iconOnly?: false; "aria-label"?: string })
  | (ButtonBase & { iconOnly: true; "aria-label": string });

export function Button(props: ButtonProps): React.ReactElement {
  const { variant = "secondary", size = "medium", icon, iconOnly = false, children, className, ...rest } = props;
  const classes = [
    variant === "primary" ? "primary" : "",
    variant === "ghost" ? "ghost" : "",
    variant === "danger" ? "danger" : "",
    variant === "dark" ? "btn-dark" : "",
    size === "small" ? "small" : "",
    size === "large" ? "btn-large" : "",
    iconOnly ? "icon-btn" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button type="button" className={classes === "" ? undefined : classes} {...rest}>
      {icon !== undefined ? <Icon name={icon} size={size === "small" ? 15 : 17} /> : null}
      {iconOnly ? null : children}
    </button>
  );
}

/* ------------------------------------------------------------------ */
/* Input / Select / Toggle                                             */
/* ------------------------------------------------------------------ */

export interface FieldProps {
  label?: string;
  hint?: string;
  error?: string;
  id?: string;
}

export function Input({
  label,
  hint,
  error,
  id,
  ...rest
}: FieldProps & InputHTMLAttributes<HTMLInputElement>): React.ReactElement {
  const fallbackId = useIdSafe("wl-input");
  const inputId = id ?? fallbackId;
  return (
    <div className="field">
      {label !== undefined ? (
        <label htmlFor={inputId}>{label}</label>
      ) : null}
      <input id={inputId} aria-invalid={error !== undefined} aria-describedby={hint !== undefined ? `${inputId}-hint` : undefined} {...rest} />
      {error !== undefined ? (
        <div className="field-error" role="alert">{error}</div>
      ) : hint !== undefined ? (
        <div className="hint" id={`${inputId}-hint`}>{hint}</div>
      ) : null}
    </div>
  );
}

export function Select({
  label,
  hint,
  error,
  id,
  children,
  ...rest
}: FieldProps & SelectHTMLAttributes<HTMLSelectElement>): React.ReactElement {
  const fallbackId = useIdSafe("wl-select");
  const selectId = id ?? fallbackId;
  return (
    <div className="field">
      {label !== undefined ? (
        <label htmlFor={selectId}>{label}</label>
      ) : null}
      <select id={selectId} aria-invalid={error !== undefined} {...rest}>
        {children}
      </select>
      {error !== undefined ? (
        <div className="field-error" role="alert">{error}</div>
      ) : hint !== undefined ? (
        <div className="hint">{hint}</div>
      ) : null}
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}): React.ReactElement {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      title={label}
      className={`toggle${checked ? " on" : ""}`}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span className="toggle-knob" aria-hidden="true" />
      <span className="toggle-label">{label}</span>
    </button>
  );
}

/* ------------------------------------------------------------------ */
/* Tabs                                                                */
/* ------------------------------------------------------------------ */

export interface TabItem {
  id: string;
  label: string;
  icon?: IconName;
}

export function Tabs({
  tabs,
  value,
  onChange,
  ariaLabel,
}: {
  tabs: TabItem[];
  value: string;
  onChange: (id: string) => void;
  ariaLabel: string;
}): React.ReactElement {
  const listRef = useRef<HTMLDivElement>(null);
  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    e.preventDefault();
    const idx = tabs.findIndex((t) => t.id === value);
    if (idx === -1) return;
    const next = e.key === "ArrowRight" ? (idx + 1) % tabs.length : (idx - 1 + tabs.length) % tabs.length;
    const tab = tabs[next];
    if (tab !== undefined) {
      onChange(tab.id);
      const btn = listRef.current?.querySelector<HTMLElement>(`[data-tab-id="${tab.id}"]`);
      btn?.focus();
    }
  };
  return (
    <div className="tabs" role="tablist" aria-label={ariaLabel} onKeyDown={onKeyDown} ref={listRef}>
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          data-tab-id={t.id}
          aria-selected={t.id === value}
          className={t.id === value ? "tab active" : "tab"}
          onClick={() => onChange(t.id)}
        >
          {t.icon !== undefined ? <Icon name={t.icon} size={15} /> : null}
          {t.label}
        </button>
      ))}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Modal / Drawer                                                      */
/* ------------------------------------------------------------------ */

function useEscape(onClose: () => void): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
}

export function Modal({
  title,
  onClose,
  children,
  actions,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  actions?: ReactNode;
}): React.ReactElement {
  useEscape(onClose);
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" role="dialog" aria-modal="true" aria-label={title} onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h3>{title}</h3>
          <Button iconOnly icon="x" aria-label="Close dialog" variant="ghost" size="small" onClick={onClose} />
        </div>
        {children}
        {actions !== undefined ? <div className="actions">{actions}</div> : null}
      </div>
    </div>
  );
}

export function Drawer({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}): React.ReactElement {
  useEscape(onClose);
  return (
    <div className="drawer-backdrop" onClick={onClose}>
      <aside className="drawer" role="dialog" aria-modal="true" aria-label={title} onClick={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <h3>{title}</h3>
          <Button iconOnly icon="x" aria-label="Close panel" variant="ghost" size="small" onClick={onClose} />
        </div>
        {children}
      </aside>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Toast (with a minimal toast host)                                   */
/* ------------------------------------------------------------------ */

export interface ToastItem {
  id: number;
  message: string;
  tone?: "ok" | "warn" | "err" | "info";
}

interface ToastContextValue {
  push: (message: string, tone?: ToastItem["tone"]) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

let toastSeq = 1;

/** Wrap the admin (or a subtree) to enable `useToast()`. Renders the host. */
export function ToastProvider({ children }: { children: ReactNode }): React.ReactElement {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const push = useCallback((message: string, tone: ToastItem["tone"] = "ok") => {
    const id = toastSeq++;
    setToasts((prev) => [...prev.slice(-2), { id, message, tone }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 5000);
  }, []);
  const value = useMemo(() => ({ push }), [push]);
  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-host" aria-live="polite" aria-atomic="false">
        {toasts.map((t) => (
          <Toast key={t.id} message={t.message} tone={t.tone} onDismiss={() => setToasts((prev) => prev.filter((x) => x.id !== t.id))} />
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (ctx === null) throw new Error("useToast must be used inside <ToastProvider>.");
  return ctx;
}

export function Toast({
  message,
  tone = "ok",
  onDismiss,
}: {
  message: string;
  tone?: ToastItem["tone"];
  onDismiss?: () => void;
}): React.ReactElement {
  const icon: IconName = tone === "err" ? "alert" : tone === "warn" ? "alert" : tone === "info" ? "info" : "checkCircle";
  return (
    <div className={`toast tone-${tone}`} role="status">
      <Icon name={icon} size={17} />
      <span>{message}</span>
      {onDismiss !== undefined ? (
        <Button iconOnly icon="x" aria-label="Dismiss notification" variant="ghost" size="small" onClick={onDismiss} />
      ) : null}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Table                                                               */
/* ------------------------------------------------------------------ */

export interface TableColumn {
  key: string;
  label: string;
}

export function Table({
  columns,
  rows,
  empty,
  caption,
}: {
  columns: TableColumn[];
  rows: Array<Record<string, ReactNode>>;
  empty?: ReactNode;
  caption?: string;
}): React.ReactElement {
  if (rows.length === 0 && empty !== undefined) {
    return <>{empty}</>;
  }
  return (
    <div className="table-wrap">
      <table>
        {caption !== undefined ? <caption className="sr-only">{caption}</caption> : null}
        <thead>
          <tr>
            {columns.map((c) => (
              <th key={c.key} scope="col">{c.label}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr key={i}>
              {columns.map((c) => (
                <td key={c.key}>{row[c.key] ?? null}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Badge / Tag, Avatar, Breadcrumbs, Stepper                           */
/* ------------------------------------------------------------------ */

export type BadgeTone = "teal" | "amber" | "red" | "blue" | "grey";

export function Badge({ tone = "teal", children }: { tone?: BadgeTone; children: ReactNode }): React.ReactElement {
  return <span className={`badge tone-${tone}`}>{children}</span>;
}

/** Tag is the same visual as Badge; kept as its own name for call sites. */
export function Tag({ tone = "teal", children }: { tone?: BadgeTone; children: ReactNode }): React.ReactElement {
  return <Badge tone={tone}>{children}</Badge>;
}

export type AvatarTone = "teal" | "blue" | "amber" | "berry" | "slate";

export function Avatar({
  name,
  tone = "teal",
  size = "medium",
}: {
  name: string;
  tone?: AvatarTone;
  size?: "small" | "medium" | "large";
}): React.ReactElement {
  return (
    <span className={`avatar avatar-${size} avatar-tone-${tone}`} aria-hidden="true">
      {initialsOf(name)}
    </span>
  );
}

export function AvatarStack({ names, max = 4 }: { names: string[]; max?: number }): React.ReactElement {
  const shown = names.slice(0, max);
  const rest = names.length - shown.length;
  const tones: AvatarTone[] = ["teal", "blue", "amber", "berry", "slate"];
  return (
    <span className="avatar-stack" role="group" aria-label={`${names.length} people`}>
      {shown.map((n, i) => (
        <Avatar key={n} name={n} tone={tones[i % tones.length]} size="small" />
      ))}
      {rest > 0 ? (
        <span className="avatar avatar-small avatar-tone-slate" aria-hidden="true">+{rest}</span>
      ) : null}
    </span>
  );
}

export function Breadcrumbs({ items }: { items: Array<{ label: string; href?: string }> }): React.ReactElement {
  return (
    <nav className="breadcrumbs" aria-label="Breadcrumb">
      <ol>
        {items.map((item, i) => {
          const last = i === items.length - 1;
          return (
            <li key={item.label} aria-current={last ? "page" : undefined}>
              {item.href !== undefined && !last ? <a href={item.href}>{item.label}</a> : <span>{item.label}</span>}
              {!last ? <span aria-hidden="true"> / </span> : null}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

export type StepState = "done" | "current" | "todo";

export function Stepper({ steps }: { steps: Array<{ label: string; state: StepState }> }): React.ReactElement {
  return (
    <ol className="stepper" aria-label="Progress">
      {steps.map((s, i) => (
        <li key={s.label} className={`step-${s.state}`} aria-current={s.state === "current" ? "step" : undefined}>
          <span className="step-dot" aria-hidden="true">
            {s.state === "done" ? <Icon name="check" size={13} /> : i + 1}
          </span>
          <span className="step-label">{s.label}</span>
        </li>
      ))}
    </ol>
  );
}

/* ------------------------------------------------------------------ */
/* EmptyState, Skeleton, Progress                                      */
/* ------------------------------------------------------------------ */

export function EmptyState({
  icon = "folder",
  title,
  description,
  action,
}: {
  icon?: IconName;
  title: string;
  description?: string;
  action?: ReactNode;
}): React.ReactElement {
  return (
    <div className="empty">
      <span className="empty-icon" aria-hidden="true">
        <Icon name={icon} size={24} />
      </span>
      <p className="empty-title">{title}</p>
      {description !== undefined ? <p>{description}</p> : null}
      {action !== undefined ? <div className="empty-action">{action}</div> : null}
    </div>
  );
}

export function Skeleton({ width, height = 12, label }: { width?: string | number; height?: number; label?: string }): React.ReactElement {
  return (
    <div
      className="skeleton"
      role="status"
      aria-label={label ?? "Loading"}
      style={{ width: width ?? "100%", height }}
    />
  );
}

export function Progress({
  value,
  max = 100,
  label,
}: {
  value: number;
  max?: number;
  label?: string;
}): React.ReactElement {
  const pct = max > 0 ? Math.min(100, Math.max(0, (value / max) * 100)) : 0;
  return (
    <div className="progress" role="progressbar" aria-valuenow={value} aria-valuemin={0} aria-valuemax={max} aria-label={label}>
      <span className="progress-fill" style={{ width: `${pct}%` }} />
    </div>
  );
}

export function ProgressRange({
  value,
  onChange,
  label,
  min = 0,
  max = 100,
}: {
  value: number;
  onChange: (value: number) => void;
  label: string;
  min?: number;
  max?: number;
}): React.ReactElement {
  return (
    <label className="progress-range">
      <span className="sr-only">{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        value={value}
        aria-label={label}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <span aria-hidden="true">{value}%</span>
    </label>
  );
}

/* ------------------------------------------------------------------ */
/* Command palette (Cmd/Ctrl+K, keyboard navigable)                    */
/* ------------------------------------------------------------------ */

export interface PaletteEntry {
  id: string;
  label: string;
  group?: string;
  icon?: IconName;
  hint?: string;
  onSelect: () => void;
}

/** Registers the global Cmd/Ctrl+K hotkey; call with the palette opener. */
export function useCommandPaletteHotkey(onOpen: () => void): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        onOpen();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onOpen]);
}

export function CommandPalette({
  open,
  onClose,
  entries,
  placeholder = "Search pages, posts, actions…",
}: {
  open: boolean;
  onClose: () => void;
  entries: PaletteEntry[];
  placeholder?: string;
}): React.ReactElement | null {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
      inputRef.current?.focus();
    }
  }, [open ]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q === "") return entries;
    return entries.filter(
      (e) => e.label.toLowerCase().includes(q) || (e.group ?? "").toLowerCase().includes(q),
    );
  }, [entries, query]);

  useEffect(() => {
    setActive(0);
  }, [query]);

  if (!open) return null;
  const activeEntry = filtered[active];

  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(filtered.length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (activeEntry !== undefined) {
        onClose();
        activeEntry.onSelect();
      }
    }
  };

  return (
    <div className="palette-backdrop" onClick={onClose}>
      <div className="palette-panel" role="dialog" aria-modal="true" aria-label="Command palette" onClick={(e) => e.stopPropagation()}>
        <div className="palette-input-row">
          <Icon name="search" size={17} />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={placeholder}
            aria-label="Search commands"
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-activedescendant={activeEntry !== undefined ? `palette-${activeEntry.id}` : undefined}
          />
          <kbd>esc</kbd>
        </div>
        <ul id="palette-list" role="listbox" aria-label="Commands" className="palette-list">
          {filtered.length === 0 ? (
            <li className="palette-empty">No matches for “{query}”.</li>
          ) : (
            filtered.map((entry, i) => (
              <li
                key={entry.id}
                id={`palette-${entry.id}`}
                role="option"
                aria-selected={i === active}
                className={i === active ? "palette-item active" : "palette-item"}
                onMouseEnter={() => setActive(i)}
                onClick={() => {
                  onClose();
                  entry.onSelect();
                }}
              >
                {entry.icon !== undefined ? <Icon name={entry.icon} size={16} /> : null}
                <span className="palette-label">{entry.label}</span>
                {entry.group !== undefined ? <span className="palette-group">{entry.group}</span> : null}
                {entry.hint !== undefined ? <kbd>{entry.hint}</kbd> : null}
              </li>
            ))
          )}
        </ul>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* Card, Alert, StatCard                                               */
/* ------------------------------------------------------------------ */

export function Card({
  title,
  sub,
  actions,
  children,
}: {
  title?: string;
  sub?: string;
  actions?: ReactNode;
  children: ReactNode;
}): React.ReactElement {
  return (
    <section className="card">
      {title !== undefined || actions !== undefined ? (
        <div className="card-head">
          <div>
            {title !== undefined ? <h3>{title}</h3> : null}
            {sub !== undefined ? <div className="card-sub">{sub}</div> : null}
          </div>
          {actions !== undefined ? <div className="card-actions">{actions}</div> : null}
        </div>
      ) : null}
      {children}
    </section>
  );
}

export type AlertTone = "ok" | "warn" | "err" | "info";

export function Alert({
  tone = "info",
  title,
  children,
  onDismiss,
}: {
  tone?: AlertTone;
  title?: string;
  children: ReactNode;
  onDismiss?: () => void;
}): React.ReactElement {
  const icon: IconName = tone === "ok" ? "checkCircle" : tone === "err" ? "alert" : tone === "warn" ? "alert" : "info";
  return (
    <div className={`alert alert-${tone}`} role={tone === "err" ? "alert" : "status"}>
      <Icon name={icon} size={18} />
      <div className="alert-body">
        {title !== undefined ? <strong>{title}</strong> : null}
        <div>{children}</div>
      </div>
      {onDismiss !== undefined ? (
        <Button iconOnly icon="x" aria-label="Dismiss" variant="ghost" size="small" onClick={onDismiss} />
      ) : null}
    </div>
  );
}

export function StatCard({
  label,
  value,
  delta,
  icon,
  tone,
}: {
  label: string;
  value: string;
  delta?: string;
  icon?: IconName;
  tone?: "warn" | "danger";
}): React.ReactElement {
  return (
    <div className="stat">
      {icon !== undefined ? (
        <span className={`stat-icon${tone !== undefined ? ` ${tone}` : ""}`} aria-hidden="true">
          <Icon name={icon} size={20} />
        </span>
      ) : null}
      <div>
        <div className="num">{value}</div>
        <div className="lbl">{label}</div>
        {delta !== undefined ? <div className="stat-delta">{delta}</div> : null}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* helpers                                                             */
/* ------------------------------------------------------------------ */

let idSeq = 1;

/** Stable-enough ids for label/input pairing (no crypto needed). */
function useIdSafe(prefix: string): string {
  const ref = useRef<string | null>(null);
  if (ref.current === null) {
    ref.current = `${prefix}-${idSeq++}`;
  }
  return ref.current;
}

function initialsOf(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "W";
  const first = parts[0]?.[0] ?? "W";
  const last = parts.length > 1 ? (parts[parts.length - 1]?.[0] ?? "") : "";
  return (first + last).toUpperCase();
}
