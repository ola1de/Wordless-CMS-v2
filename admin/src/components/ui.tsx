import type { ReactNode } from "react";
import { Icon } from "./icons";

export function Spinner(): React.ReactElement {
  return (
    <div className="spinner-row">
      <span className="spinner" aria-hidden="true" />
      <span>Loading…</span>
    </div>
  );
}

export function EmptyState({ children }: { children: ReactNode }): React.ReactElement {
  return <div className="empty">{children}</div>;
}

export function ErrorBox({ message, onDismiss }: { message: string; onDismiss?: () => void }): React.ReactElement {
  return (
    <div className="error" role="alert">
      <Icon name="alert" size={18} />
      <span style={{ flex: 1 }}>{message}</span>
      {onDismiss ? (
        <button type="button" className="ghost small icon-btn" aria-label="Dismiss" onClick={onDismiss}>
          <Icon name="x" size={16} />
        </button>
      ) : null}
    </div>
  );
}

export function Pagination({
  page,
  totalPages,
  total,
  onPage,
}: {
  page: number;
  totalPages: number;
  total: number;
  onPage: (page: number) => void;
}): React.ReactElement {
  return (
    <div className="pagination">
      <button type="button" className="small" disabled={page <= 1} onClick={() => onPage(page - 1)}>
        <Icon name="chevronLeft" size={15} />
        Previous
      </button>
      <span>
        Page {page} of {Math.max(totalPages, 1)} ({total} total)
      </span>
      <button type="button" className="small" disabled={page >= totalPages} onClick={() => onPage(page + 1)}>
        Next
        <Icon name="chevronRight" size={15} />
      </button>
    </div>
  );
}

export function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}): React.ReactElement {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-label={title}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h3>{title}</h3>
          <button type="button" className="ghost small icon-btn" aria-label="Close" onClick={onClose}>
            <Icon name="x" size={18} />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

export function Badge({ status }: { status: string }): React.ReactElement {
  return <span className={`badge ${status}`}>{status}</span>;
}
