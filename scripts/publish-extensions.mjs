#!/usr/bin/env node
/**
 * WORDLESS v2.0 — publish a loaded extension as a real Worker (Phase P6).
 *
 * Usage (from the wordless-rust directory, on the user's own machine):
 *   node scripts/publish-extensions.mjs <slug> --env staging|production --confirm
 *   node scripts/publish-extensions.mjs <slug> --env staging --confirm --dispatch-namespace wordless-extensions
 *   node scripts/publish-extensions.mjs <slug> --env production --confirm --skip-main-redeploy
 *
 * The extension must already be LOADED (POST /api/extensions/:slug/load)
 * — load generates the Worker project (worker-entry.ts + shim.ts +
 * wordpress.d.ts + wrangler.toml) and stores it in R2 under
 * extensions/<slug>/worker/ with the converted files under
 * extensions/<slug>/converted/. This script downloads that project
 * and deploys it. It never touches the extension's source or the
 * registry: publishing only makes a loaded extension reachable.
 *
 * Two publish paths (see docs/04-rust-v2-architecture.md §3 P6):
 *
 *   DEFAULT — service binding (FREE). The extension is deployed as a
 *   normal account-level Worker (free tier, its own 3 MB limit) and the
 *   main Worker reaches it through `[[services]] binding =
 *   "EXT_<SLUG>" service = "<slug>"`. The script adds that binding to
 *   wrangler.toml and redeploys the main Worker (via scripts/deploy.mjs,
 *   which runs the tests, the Rust build, and the WASM size gate — the
 *   redeploy is REFUSED while the main WASM is over the 3 MB free-tier
 *   limit, so resolve the size decision first or use
 *   --skip-main-redeploy and redeploy later). One binding per
 *   extension, declared at deploy time: fine for the user's own site,
 *   not for an arbitrary marketplace.
 *
 *   --dispatch-namespace <name> — Workers for Platforms. The extension
 *   is deployed into the namespace and the main Worker resolves it by
 *   slug at runtime (`EXT_DISPATCHER.get(slug)`); no main-Worker
 *   redeploy is needed. This is the design target and the mechanism
 *   the §9.1 marketplace economy needs — but it is NOT free: Workers
 *   for Platforms is $25/month with no free tier (re-check on
 *   Cloudflare's official pricing page at deploy time). If the deploy
 *   403s, the account does not have Workers for Platforms enabled.
 *
 * Requires: a Cloudflare login (`wrangler whoami` must show an
 * authenticated account — the same guard as scripts/deploy.mjs; note
 * `wrangler whoami` exits 0 even when logged out, so the output is
 * checked). No credentials are read, stored or printed. The Cloudflare
 * API token, if used, comes from wrangler's own login, never from an
 * argument or an environment variable this script reads.
 *
 * NEVER run this from the development VM — there are no Cloudflare
 * credentials there by design; publishing runs from the user's own
 * machine. This script has NOT been verified live (no account in the
 * VM): the first live run is a real test — read its output, do not
 * assume success.
 */
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  mkdtempSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { join, dirname } from "node:path";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const ARGS = process.argv.slice(2);
const KNOWN_ENVS = new Set(["staging", "production"]);

function fail(msg) {
  console.error(`\n  ✗ ${msg}`);
  process.exit(1);
}
function ok(msg) {
  console.log(`  ✓ ${msg}`);
}
function log(msg = "") {
  console.log(msg);
}

/** Quote one argument for a Windows shell command line (MSVC rules). */
function quoteArg(arg) {
  if (arg === "") return '""';
  if (!/[\s"]/.test(arg)) return arg;
  return '"' + arg.replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/, "$1$1") + '"';
}

function run(cmd, args, opts = {}) {
  const isWin = process.platform === "win32";
  const res = spawnSync(cmd, args, {
    cwd: opts.cwd ?? ROOT,
    shell: isWin,
    encoding: "utf8",
    env: { ...process.env, ...(opts.env ?? {}) },
  });
  return {
    ok: res.status === 0,
    status: res.status,
    stdout: res.stdout ?? "",
    stderr: res.stderr ?? "",
  };
}

function flag(name) {
  const i = ARGS.indexOf(name);
  if (i === -1) return null;
  return ARGS[i + 1] ?? null;
}
function hasFlag(name) {
  return ARGS.includes(name);
}

// Positional args: skip flags and their values.
const positional = [];
for (let i = 0; i < ARGS.length; i++) {
  const a = ARGS[i];
  if (a === "--env" || a === "--dispatch-namespace" || a === "--source") { i++; continue; }
  if (a.startsWith("--")) continue;
  positional.push(a);
}
const slug = positional[0];
const env = flag("--env");
const dispatchNamespace = flag("--dispatch-namespace");
const source = flag("--source") ?? "extension";
const skipMainRedeploy = hasFlag("--skip-main-redeploy");
const confirm = hasFlag("--confirm");

if (!slug || !/^[a-z0-9][a-z0-9-]*$/.test(slug)) {
  fail("Usage: node scripts/publish-extensions.mjs <slug> --env staging|production --confirm [--dispatch-namespace <name>] [--source theme] [--skip-main-redeploy]\n  <slug> must be a lowercase extension slug (e.g. hello-dolly).");
}
if (source !== "extension" && source !== "theme") {
  fail('--source must be "extension" (default, a marketplace plugin) or "theme" (an imported theme loaded with source=theme).');
}
if (!env || !KNOWN_ENVS.has(env)) {
  fail("--env staging|production is required (the top-level config carries local-dev placeholder ids).");
}
if (!confirm) {
  fail("Refusing to publish without --confirm (a plain invocation never deploys).");
}

const binding = `EXT_${slug.toUpperCase().replace(/-/g, "_")}`;
const workdir = mkdtempSync(join(tmpdir(), `wordless-ext-${slug}-`));

log(`\n  WORDLESS v2.0 — publish extension "${slug}" to ${env}`);
log(`  Mode: ${dispatchNamespace ? `dispatch namespace "${dispatchNamespace}" (Workers for Platforms — $25/mo, no free tier)` : `service binding "${binding}" (FREE)`}`);
log(`  Work dir: ${workdir}\n`);

// ------------------------------------------------------- 1. whoami
const whoami = run("npx", ["wrangler", "whoami"]);
const loggedIn =
  whoami.ok &&
  /account id/i.test(whoami.stdout) &&
  !/not authenticated/i.test(whoami.stdout);
if (!loggedIn) {
  fail("Not logged in to Cloudflare (`wrangler whoami` shows no authenticated account). Run `npx wrangler login` first — official browser OAuth, never paste a token here.");
}
ok(`Logged in to Cloudflare:\n${whoami.stdout.trim().split("\n").map((l) => `     ${l}`).join("\n")}`);

// ------------------------------------------------- 2. R2 bucket name
function bucketForEnv(envName) {
  const toml = readFileSync(join(ROOT, "wrangler.toml"), "utf8");
  // Find the [env.<env>] section and its r2_buckets bucket_name.
  const section = toml.split(new RegExp(`\\[env\\.${envName}[\\s\\S]*?(?=\\[env\\.|$)`))[0];
  const m = /bucket_name\s*=\s*"([^"]+)"/.exec(section);
  return m ? m[1] : null;
}
const bucket = bucketForEnv(env);
if (!bucket) fail(`Could not find the R2 bucket for [env.${env}] in wrangler.toml.`);
ok(`R2 bucket for ${env}: ${bucket}`);

// ------------------------------------------- 3. download the project
const R2_PREFIX = `extensions/${slug}`;
function r2Get(key, dest) {
  const r = run("npx", [
    "wrangler", "r2", "object", "get",
    `${bucket}/${key}`, "--file", dest, "--env", env,
  ]);
  if (!r.ok) {
    fail(`Could not download R2 object ${key} (is the extension loaded? POST /api/extensions/${slug}/load first).\n${r.stderr.trim()}`);
  }
  return r;
}

mkdirSync(join(workdir, "converted"), { recursive: true });
for (const f of ["worker-entry.ts", "shim.ts", "wordpress.d.ts", "wrangler.toml"]) {
  r2Get(`${R2_PREFIX}/worker/${f}`, join(workdir, f));
}
ok("Downloaded the generated Worker project from R2");

// Converted files: this wrangler has no `r2 object list`, so the
// converted paths come from the entry file itself — it imports every
// converted module (`from "./converted/<path>.js"`). The R2 key for a
// project path `converted/<path>.ts` is
// `<converted-prefix>/converted/<path>.ts` (the `converted/` segment
// is part of the generated path, not the prefix). The generated
// project itself always lives under `extensions/<slug>/worker/`
// (even for a theme source); only the converted code lives under the
// source prefix.
const convertedPrefix = source === "theme" ? `themes/${slug}` : `extensions/${slug}`;
const entry = readFileSync(join(workdir, "worker-entry.ts"), "utf8");
const convertedPaths = [...entry.matchAll(/from "\.\/(converted\/[^"]+?)\.js"/g)].map((m) => m[1] + ".ts");
if (convertedPaths.length === 0) {
  fail(`The generated worker-entry.ts imports no converted files — the extension has no converted code to run. Check the compat report (GET /api/extensions/${slug}) and re-load.`);
}
for (const p of convertedPaths) {
  const rel = p.replace(/^converted\//, "");
  const key = `${convertedPrefix}/converted/${rel}`;
  const dest = join(workdir, p);
  mkdirSync(dirname(dest), { recursive: true });
  r2Get(key, dest);
}
ok(`Downloaded ${convertedPaths.length} converted file(s) from R2`);

// The generated wrangler.toml names the Worker after the slug and
// points at worker-entry.ts; verify it says what we expect.
const extToml = readFileSync(join(workdir, "wrangler.toml"), "utf8");
if (!extToml.includes(`name = "${slug}"`)) {
  fail(`The generated wrangler.toml does not name the Worker "${slug}" — refusing to deploy a mismatched project.`);
}

// ------------------------------------------------------- 4. deploy
if (dispatchNamespace) {
  log(`\n  Deploying into dispatch namespace "${dispatchNamespace}"…`);
  const dep = run("npx", ["wrangler", "deploy", "--dispatch-namespace", dispatchNamespace], { cwd: workdir });
  if (!dep.ok) {
    const hint = /403|forbidden|not.*enabled/i.test(dep.stderr)
      ? "\n  A 403 here usually means the account does not have Workers for Platforms enabled — it is a $25/mo plan with no free tier. Use the default service-binding path instead (no --dispatch-namespace)."
      : "";
    fail(`wrangler deploy --dispatch-namespace failed.${hint}\n${dep.stderr.trim()}`);
  }
  ok(`Extension Worker deployed into dispatch namespace "${dispatchNamespace}"`);
  log(`\n  No main-Worker change needed: the resolver calls EXT_DISPATCHER.get("${slug}") at runtime.`);
  log(`  Verify: load the extension (if not already loaded) and invoke a hook, e.g.`);
  log(`    POST /api/extensions/${slug}/invoke  {"hook": "admin_notices", "kind": "action"}`);
} else {
  log(`\n  Deploying as a normal account-level Worker named "${slug}"…`);
  const dep = run("npx", ["wrangler", "deploy"], { cwd: workdir });
  if (!dep.ok) {
    fail(`wrangler deploy failed.\n${dep.stderr.trim()}`);
  }
  ok(`Extension Worker "${slug}" deployed (free tier, own 3 MB limit)`);

  // Add the service binding to the main wrangler.toml (per env).
  const mainTomlPath = join(ROOT, "wrangler.toml");
  let mainToml = readFileSync(mainTomlPath, "utf8");
  const bindingLine = `binding = "${binding}"`;
  const serviceLine = `service = "${slug}"`;
  if (mainToml.includes(bindingLine) && mainToml.includes(serviceLine)) {
    ok(`Service binding ${binding} already present in wrangler.toml`);
  } else {
    const block = `\n# P6: extension "${slug}" — added by scripts/publish-extensions.mjs.\n# Remove this block when the extension is unpublished.\n[[env.${env}.services]]\nbinding = "${binding}"\nservice = "${slug}"\n`;
    // Insert after the env's dispatch_namespaces block if present,
    // otherwise append at the end of the file.
    const anchor = new RegExp(
      `(\\[\\[env\\.${env}\\.dispatch_namespaces\\]\\][\\s\\S]*?namespace = "[^"]*"\\n)`,
    );
    const m = anchor.exec(mainToml);
    if (m) {
      mainToml = mainToml.slice(0, m.index + m[0].length) + block + mainToml.slice(m.index + m[0].length);
    } else {
      mainToml += block;
    }
    writeFileSync(mainTomlPath, mainToml);
    ok(`Added [[env.${env}.services]] ${binding} -> "${slug}" to wrangler.toml`);
  }

  if (skipMainRedeploy) {
    log(`\n  --skip-main-redeploy: the binding is recorded but the main Worker`);
    log(`  has NOT been redeployed, so it cannot reach the extension yet.`);
    log(`  Redeploy with:  npm run deploy:${env}`);
  } else {
    log(`\n  Redeploying the main Worker so the binding takes effect…`);
    const redeploy = run("node", ["scripts/deploy.mjs", "--env", env, "--confirm"]);
    if (!redeploy.ok) {
      log(`\n  ✗ Main-Worker redeploy failed — the extension Worker IS deployed,`);
      log(`    but the main Worker cannot reach it until it redeploys.`);
      log(`    Common cause: the WASM size gate (the main binary is over the`);
      log(`    3 MB free-tier limit — resolve the size decision first, then`);
      log(`    run:  npm run deploy:${env}`);
      log(`\n${redeploy.stdout.trim()}\n${redeploy.stderr.trim()}`);
      process.exit(1);
    }
    ok("Main Worker redeployed with the new service binding");
  }
  log(`\n  Verify: load the extension (if not already loaded) and invoke a hook, e.g.`);
  log(`    POST /api/extensions/${slug}/invoke  {"hook": "admin_notices", "kind": "action"}`);
}

rmSync(workdir, { recursive: true, force: true });
log(`\n  Done. Extension "${slug}" is published for ${env}.`);
