#!/usr/bin/env node
// WORDLESS v2 — WASM size report (free-tier guard).
//
// Prints all six Workers' WASM sizes against the Cloudflare free-tier
// 3 MiB script limit (3,145,728 bytes). Run after every release build:
//   npm run size          (or: node scripts/size-report.mjs)
// Exits non-zero when any build is over the limit, so CI / deploy
// scripts can gate on it. The deploy script (scripts/deploy.mjs)
// enforces the same limit independently before any deploy.
//
// Added 2026-11-19 (size fix): the P5 build was 3,328,519 bytes
// (105.8%) until wasm-opt -Oz brought it to 2,906,063. This report
// exists so P7–P12 catch regressions the moment a build lands, not
// at deploy time. Extended 2026-11-20 (converter split): the
// conversion engine now runs in its own Worker, so both binaries
// are reported.
import { statSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const LIMIT = 3 * 1024 * 1024; // 3,145,728 — Cloudflare Workers free tier
const TARGET = 3_000_000; // soft target: leave headroom for later phases
const WORKERS = [
  { name: "main", path: "crates/wordless-worker/build/index_bg.wasm" },
  { name: "converter", path: "crates/wordless-converter/build/index_bg.wasm" },
  { name: "social", path: "crates/wordless-social-worker/build/index_bg.wasm" },
  { name: "woo", path: "crates/wordless-woocommerce-worker/build/index_bg.wasm" },
  { name: "school", path: "crates/wordless-school-worker/build/index_bg.wasm" },
  { name: "seo", path: "crates/wordless-seo-worker/build/index_bg.wasm" },
];

let failed = false;
for (const w of WORKERS) {
  const full = join(ROOT, w.path);
  if (!existsSync(full)) {
    console.error(`size-report: no build found at ${w.path} — run 'npm run build:all' first.`);
    failed = true;
    continue;
  }
  const bytes = statSync(full).size;
  const pct = ((bytes / LIMIT) * 100).toFixed(1);
  const headroom = LIMIT - bytes;
  console.log(`size-report: ${w.name} (${w.path})`);
  console.log(`  size:     ${bytes.toLocaleString("en-US")} bytes (${pct}% of the 3 MiB free-tier limit)`);
  console.log(`  limit:    ${LIMIT.toLocaleString("en-US")} bytes`);
  console.log(`  headroom: ${headroom.toLocaleString("en-US")} bytes ${headroom < 0 ? "(OVER LIMIT)" : ""}`);
  console.log(`  target:   <= ${TARGET.toLocaleString("en-US")} bytes (soft target, leaves growth room)`);
  if (bytes > LIMIT) {
    console.error("size-report: FAIL — over the free-tier limit. See SIZE-REPORT.md for the levers (wasm-opt -Oz, profile tuning).");
    failed = true;
  } else if (bytes > TARGET) {
    console.log("size-report: PASS (under limit) but over the soft target — watch growth.");
  } else {
    console.log("size-report: PASS — under the limit with room to grow.");
  }
}
if (failed) process.exit(1);
