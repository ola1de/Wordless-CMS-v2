#!/usr/bin/env node
// WORDLESS v2 P11 — builder palette check (source-level, no server).
//
// The palette IS the library, not a copy: this script proves the
// Rust palette constants match the admin element/icon libraries and
// the Elementor renderer, so a drift fails loudly instead of shipping
// a palette that quietly drops an element or icon.
//
//   node scripts/check-builder-palette.mjs
//
// Checks:
//   1. every `export function` in admin/src/components/elements.tsx
//      appears exactly once as a `source_export` in ELEMENT_ITEMS
//      (crates/wordless-builder/src/palette.rs) — and vice versa.
//   2. every name in the `IconName` union in icons.tsx appears in
//      BUILDER_ICONS (crates/wordless-builder/src/palette_data.rs)
//      — and vice versa (104 expected).
//   3. every palette item's `icon` is a known icon name.
//   4. the 10 core widget types equal wordless-elementor's
//      SUPPORTED_ELEMENTOR_WIDGETS, and every element widget type is
//      in wordless-elementor's WORDLESS_ELEMENT_WIDGETS (so the
//      palette never offers a type the renderer drops).
//
// Exit 0 when everything matches; exit 1 listing every mismatch.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const read = (p) => fs.readFileSync(path.join(ROOT, p), "utf8");

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  ok  " : "  FAIL"} ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};

// --- TS sources -------------------------------------------------------
const iconsTs = read("admin/src/components/icons.tsx");
const elementsTs = read("admin/src/components/elements.tsx");

const union = iconsTs.match(/export type IconName =([\s\S]*?);/);
const tsIcons = [...union[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
const tsExports = [...elementsTs.matchAll(/^export function (\w+)/gm)].map((m) => m[1]);

// --- Rust sources ------------------------------------------------------
const paletteRs = read("crates/wordless-builder/src/palette.rs");
const paletteDataRs = read("crates/wordless-builder/src/palette_data.rs");
const elementorRs = read("crates/wordless-elementor/src/lib.rs");

const rustIcons = [...paletteDataRs.matchAll(/^\s*\("([^"]+)",/gm)].map((m) => m[1]);
const rustExports = [...paletteDataRs.matchAll(/^\s*"([A-Za-z]+)",$/gm)].map((m) => m[1]);
const elementsSection = paletteRs.slice(
  paletteRs.indexOf("pub const ELEMENT_ITEMS"),
  paletteRs.indexOf("/// The responsive preview devices"),
);
const itemExports = [...elementsSection.matchAll(/source_export: "([^"]*)"/g)]
  .map((m) => m[1])
  .filter((s) => s.length > 0);
const itemIcons = [...paletteRs.matchAll(/^\s*icon: "([^"]+)",/gm)].map((m) => m[1]);

const sorted = (a) => [...a].sort();
const sameSet = (a, b) => JSON.stringify(sorted(a)) === JSON.stringify(sorted(b));
const missing = (a, b) => a.filter((x) => !b.includes(x));
const extra = (a, b) => b.filter((x) => !a.includes(x));

// --- 1. element exports -------------------------------------------------
check(
  `elements.tsx exports (${tsExports.length}) == ELEMENT_ITEMS source_exports (${itemExports.length})`,
  sameSet(tsExports, itemExports),
  sameSet(tsExports, itemExports)
    ? ""
    : `missing: ${missing(tsExports, itemExports).join(", ") || "none"}; extra: ${extra(tsExports, itemExports).join(", ") || "none"}`,
);
check(
  `palette_data ELEMENT_EXPORTS (${rustExports.length}) == elements.tsx exports`,
  sameSet(tsExports, rustExports),
  sameSet(tsExports, rustExports)
    ? ""
    : `missing: ${missing(tsExports, rustExports).join(", ") || "none"}; extra: ${extra(tsExports, rustExports).join(", ") || "none"}`,
);

// --- 2. icons ------------------------------------------------------------
check(
  `IconName union (${tsIcons.length}) == BUILDER_ICONS (${rustIcons.length})`,
  sameSet(tsIcons, rustIcons) && tsIcons.length === 104,
  sameSet(tsIcons, rustIcons)
    ? ""
    : `missing: ${missing(tsIcons, rustIcons).join(", ") || "none"}; extra: ${extra(tsIcons, rustIcons).join(", ") || "none"}`,
);

// --- 3. palette item icons are known --------------------------------------
const unknownIcons = [...new Set(itemIcons)].filter((i) => !tsIcons.includes(i));
check(
  `every palette item icon is a known icon (${new Set(itemIcons).size} used)`,
  unknownIcons.length === 0,
  unknownIcons.length ? `unknown: ${unknownIcons.join(", ")}` : "",
);

// --- 4. widget types match the renderer -----------------------------------
const coreList = elementorRs.match(/SUPPORTED_ELEMENTOR_WIDGETS: &\[&str\] = &\[([\s\S]*?)\];/);
const coreTypes = [...coreList[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
const wlList = elementorRs.match(/WORDLESS_ELEMENT_WIDGETS: &\[&str\] = &\[([\s\S]*?)\];/);
const wlTypes = [...wlList[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
const paletteTypes = [...paletteRs.matchAll(/^\s*widget_type: "([^"]+)",/gm)].map((m) => m[1]);
const coreInPalette = paletteTypes.filter((t) => coreTypes.includes(t));
const elementInPalette = [...new Set(paletteTypes.filter((t) => t.startsWith("wordless-")))];

check(
  `core widget types in palette (${coreInPalette.length}) == renderer list (${coreTypes.length})`,
  sameSet(coreTypes, [...new Set(coreInPalette)]),
  sameSet(coreTypes, [...new Set(coreInPalette)])
    ? ""
    : `missing: ${missing(coreTypes, coreInPalette).join(", ") || "none"}; extra: ${extra(coreTypes, coreInPalette).join(", ") || "none"}`,
);
const unrendered = elementInPalette.filter((t) => !wlTypes.includes(t));
check(
  `element widget types (${elementInPalette.length}) all rendered by wordless-elementor (${wlTypes.length} types)`,
  unrendered.length === 0 && elementInPalette.length === wlTypes.length,
  unrendered.length
    ? `not rendered: ${unrendered.join(", ")}`
    : elementInPalette.length !== wlTypes.length
      ? `palette has ${elementInPalette.length}, renderer has ${wlTypes.length}`
      : "",
);

if (failures.length) {
  console.log(`\ncheck-builder-palette: FAIL (${failures.length} check(s) failed)`);
  process.exit(1);
}
console.log("\ncheck-builder-palette: PASS — palette matches the libraries and the renderer.");
