/**
 * Undo/redo history for the Visual Builder — a pure, framework-free
 * stack (P11). Kept in its own module with only erasable TypeScript
 * syntax so the smoke test can exercise it directly with Node's
 * type stripping (`node --test`); see builderHistory.test.ts.
 */

export interface HistoryState<T> {
  past: T[];
  present: T;
  future: T[];
}

export function createHistory<T>(initial: T): HistoryState<T> {
  return { past: [], present: initial, future: [] };
}

/** Push a new present; drops the redo stack. Past is capped at 100. */
export function historyPush<T>(h: HistoryState<T>, next: T): HistoryState<T> {
  const past = [...h.past, h.present];
  if (past.length > 100) past.shift();
  return { past, present: next, future: [] };
}

export function historyUndo<T>(h: HistoryState<T>): HistoryState<T> {
  if (h.past.length === 0) return h;
  const present = h.past[h.past.length - 1];
  if (present === undefined) return h;
  return {
    past: h.past.slice(0, -1),
    present,
    future: [h.present, ...h.future],
  };
}

export function historyRedo<T>(h: HistoryState<T>): HistoryState<T> {
  if (h.future.length === 0) return h;
  const present = h.future[0];
  if (present === undefined) return h;
  return {
    past: [...h.past, h.present],
    present,
    future: h.future.slice(1),
  };
}

export function canUndo<T>(h: HistoryState<T>): boolean {
  return h.past.length > 0;
}

export function canRedo<T>(h: HistoryState<T>): boolean {
  return h.future.length > 0;
}
