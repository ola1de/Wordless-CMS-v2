/**
 * WCAG 2.1 contrast-ratio helpers (P4 login customiser, §6.4).
 *
 * Pure functions, no dependencies — the login customiser computes the
 * ratio for every custom colour pair it applies and WARNS (never silently
 * applies) when a pair fails WCAG AA (4.5:1 for normal text, 3:1 for
 * large text / UI components). Ratios are computed from the hex values
 * the user typed, so the warning is measured, not guessed.
 */

export interface ContrastResult {
  ratio: number;
  /** Rounded to two decimals for display, e.g. 4.52. */
  display: string;
  /** WCAG AA for normal-sized text. */
  aaNormal: boolean;
  /** WCAG AA for large text (≥18pt / 14pt bold) and UI components. */
  aaLarge: boolean;
  /** WCAG AAA for normal text. */
  aaaNormal: boolean;
}

function parseHex(hex: string): [number, number, number] | null {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
  if (m === null) return null;
  const v = m[1]!;
  return [parseInt(v.slice(0, 2), 16), parseInt(v.slice(2, 4), 16), parseInt(v.slice(4, 6), 16)];
}

function luminance(rgb: [number, number, number]): number {
  const channel = (c: number): number => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
}

/** WCAG contrast ratio between two hex colours, or null if either is not a 6-digit hex. */
export function contrastRatio(foreground: string, background: string): ContrastResult | null {
  const fg = parseHex(foreground);
  const bg = parseHex(background);
  if (fg === null || bg === null) return null;
  const l1 = luminance(fg);
  const l2 = luminance(bg);
  const ratio = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
  return {
    ratio,
    display: ratio.toFixed(2),
    aaNormal: ratio >= 4.5,
    aaLarge: ratio >= 3,
    aaaNormal: ratio >= 7,
  };
}

/** True when a hex string is a valid 6-digit colour. */
export function isHexColour(value: string): boolean {
  return parseHex(value) !== null;
}
