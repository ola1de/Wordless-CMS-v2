#!/usr/bin/env node
/**
 * WORDLESS v2.0 deploy guard (Phase P5; converter split added 2026-11-20).
 *
 * Usage (from the wordless-rust directory):
 *   npm run deploy:staging      deploy to staging
 *   npm run deploy:production   deploy to production
 *   node scripts/deploy.mjs --env staging --confirm [--paid]
 *
 * Deliberately refuses to deploy without ALL of:
 *   1. --env staging|production (the top-level config carries local-dev
 *      placeholder ids — deploying it anywhere would be wrong), and
 *   2. --confirm (a plain `npm run deploy` never deploys), and
 *   3. a successful `wrangler whoami` (proves a Cloudflare login exists), and
 *   4. a real database_id in the chosen [env.*] block (not the placeholder).
 *
 * Before touching Cloudflare it runs the pre-deploy checks: the full
 * cargo test suite, ALL SIX Rust/WASM builds, and the WASM SIZE GATE
 * for all six Workers — the free tier allows a 3 MiB script per
 * Worker. The size decision was resolved by the converter split
 * (2026-11-20) and the social+Woo split the same day, and the school
 * split later the same day, and the SEO split later the same
 * day: the PHP → TypeScript engine + Elementor renderer run in
 * their own Worker (wordless-converter), social networking in
 * wordless-social, WooCommerce in wordless-woocommerce, the school
 * portal + CBT in wordless-school, and the SEO Engine in
 * wordless-seo, each bound to the main Worker as a service
 * binding, so all six binaries fit the free tier without Workers
 * Paid. --paid is still accepted ($5/mo, 10 MB script limit) if a
 * future phase ever outgrows the free tier. See
 * docs/cutover-runbook.md prerequisite 1.
 *
 * Then: remote D1 migrations for that environment (0011_p3.sql,
 * 0012 (P6 extensions), 0013 (P7 social), 0014 (P8 Woo) and 0015
 * (P9 school) and 0016 (P10 SEO audit runs) are new on top of
 * a v1 database — the upgrade path rehearsed locally in
 * tests/parity/rollback.mjs) → deploy SOCIAL, then WOO, then
 * SCHOOL, then SEO, then the CONVERTER (`wrangler deploy
 * --env <name>` from each sub-worker crate, which binds the service
 * name wordless-<name>-<env>) → deploy the MAIN Worker → verify
 * /api/health on the main URL and /__social/health, /__woo/health,
 * /__school/health, /__seo/health, /__convert/health on the
 * sub-worker URLs, and print them all.
 *
 * No credentials are read, stored or printed. Never run this from the
 * development VM — there are no Cloudflare credentials there by design;
 * deployment runs from the user's own machine.
 */
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const ARGS = process.argv.slice(2);
const KNOWN_ENVS = new Set(["staging", "production"]);
const FREE_LIMIT = 3 * 1024 * 1024; // Cloudflare Workers free tier: 3 MiB script
const PAID_LIMIT = 10 * 1024 * 1024; // Workers Paid ($5/mo): 10 MiB script
const MAIN_WASM = "crates/wordless-worker/build/index_bg.wasm";
const CONVERTER_WASM = "crates/wordless-converter/build/index_bg.wasm";
const SOCIAL_WASM = "crates/wordless-social-worker/build/index_bg.wasm";
const WOO_WASM = "crates/wordless-woocommerce-worker/build/index_bg.wasm";
const SCHOOL_WASM = "crates/wordless-school-worker/build/index_bg.wasm";
const SEO_WASM = "crates/wordless-seo-worker/build/index_bg.wasm";
const PLACEHOLDER_ID = "00000000-0000-0000-0000-000000000000";

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
  let out = '"';
  let backslashes = 0;
  for (const ch of arg) {
    if (ch === "\\") { backslashes++; continue; }
    if (ch === '"') { out += "\\".repeat(backslashes * 2 + 1) + '"'; backslashes = 0; continue; }
    out += "\\".repeat(backslashes) + ch;
    backslashes = 0;
  }
  out += "\\".repeat(backslashes * 2) + '"';
  return out;
}

/** Fail fast when Wrangler is not installed locally: without it `npx`
 * tries to download wrangler, which fails confusingly (EPIPE) in
 * restricted environments. Run `npm install` in this directory first. */
function requireWrangler() {
  const bin = join(ROOT, "node_modules", ".bin", process.platform === "win32" ? "wrangler.cmd" : "wrangler");
  if (!existsSync(bin)) {
    fail("Wrangler is not installed in this directory. Run `npm install` here first (it installs wrangler 4.165.0 as a project dependency).");
  }
}

function run(cmd, args, { stdio = "pipe", cwd = ROOT } = {}) {
  // On Windows, npm/npx are .cmd shims and Node cannot spawn them without
  // a shell (ENOENT for `npm`, EINVAL for `npm.cmd` — confirmed on a real
  // Windows install for the v1 installer). Run through cmd.exe on Windows,
  // quoting arguments first: with shell:true Node concatenates args
  // unescaped, so values with spaces (e.g. Windows paths) need quoting.
  // Cargo lives in ~/.cargo/bin on Unix (rustup) — prepend it so the
  // script works from a plain shell — and point TMPDIR at a real
  // directory: /tmp is a 512 MB tmpfs on the development VM and cargo
  // builds spill over it. Both are harmless on Windows (rustup puts
  // cargo on PATH there; TMPDIR is ignored).
  const home = process.env.HOME ?? process.env.USERPROFILE ?? "";
  const env = {
    ...process.env,
    PATH: `${home}/.cargo/bin:${process.env.PATH ?? ""}`,
    TMPDIR: `${home}/.tmp-cargo`,
  };
  const useShell = process.platform === "win32";
  const finalArgs = useShell ? args.map(quoteArg) : args;
  const result = spawnSync(cmd, finalArgs, { cwd, stdio, encoding: "utf8", shell: useShell, env });
  return { ok: result.status === 0, code: result.status ?? 1, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

/** Read a --flag value (or null when absent). */
function flag(name) {
  const i = ARGS.indexOf(name);
  return i >= 0 && i + 1 < ARGS.length ? ARGS[i + 1] : null;
}

async function main() {
  if (ARGS.includes("--help") || ARGS.includes("-h")) {
    log("Usage: npm run deploy:staging | npm run deploy:production");
    log("  node scripts/deploy.mjs --env staging|production --confirm [--paid]");
    log("  --paid declares Workers Paid ($5/mo, 10 MB script limit). Without it");
    log("  the free-tier 3 MiB limit applies to EACH Worker and an oversize");
    log("  build is refused. Deploys the social, Woo, school, and converter Workers");
    log("  first, then the main Worker. Pre-deploy checks (cargo tests + all five WASM builds +");
    log("  size gates) run first. Requires a Cloudflare login (`npx wrangler login`) first.");
    return;
  }
  requireWrangler();
  const env = flag("--env");
  if (env === null || !KNOWN_ENVS.has(env)) {
    fail(
      "Refusing to deploy: --env staging|production is required\n" +
        "  npm run deploy:staging\n" +
        "  npm run deploy:production\n" +
        "  (The top-level wrangler.toml carries local-dev placeholder ids.)",
    );
  }
  if (!ARGS.includes("--confirm")) {
    fail(
      "Refusing to deploy: re-run with --confirm\n" +
        `  npm run deploy:${env}\n` +
        "  (A plain `npm run deploy` never deploys.)",
    );
  }
  const paid = ARGS.includes("--paid");
  const limit = paid ? PAID_LIMIT : FREE_LIMIT;
  const envArgs = ["--env", env];

  log(`  Deploying v2.0.0 to environment: ${env}${paid ? " (Workers Paid, 10 MB limit)" : " (free tier, 3 MB limit per Worker)"}`);

  log("\n  0. Pre-deploy checks (cargo tests + all five WASM builds + size gates)");
  let r = run("cargo", ["test", "--workspace", "--lib"], { stdio: "inherit" });
  if (!r.ok) fail("`cargo test --workspace --lib` failed. Fix the failing tests before deploying.");
  ok("Cargo tests pass");
  r = run("bash", ["scripts/build-worker.sh"], { stdio: "inherit" });
  if (!r.ok) fail("`bash scripts/build-worker.sh` failed (on Windows: run from Git Bash, or add it to PATH). Fix the build errors and retry.");
  ok("Main WASM build passed");
  r = run("bash", ["scripts/build-converter.sh"], { stdio: "inherit" });
  if (!r.ok) fail("`bash scripts/build-converter.sh` failed. Fix the build errors and retry.");
  ok("Converter WASM build passed");
  r = run("bash", ["scripts/build-social.sh"], { stdio: "inherit" });
  if (!r.ok) fail("`bash scripts/build-social.sh` failed. Fix the build errors and retry.");
  ok("Social WASM build passed");
  r = run("bash", ["scripts/build-woo.sh"], { stdio: "inherit" });
  if (!r.ok) fail("`bash scripts/build-woo.sh` failed. Fix the build errors and retry.");
  ok("Woo WASM build passed");
  r = run("bash", ["scripts/build-school.sh"], { stdio: "inherit" });
  if (!r.ok) fail("`bash scripts/build-school.sh` failed. Fix the build errors and retry.");
  ok("School WASM build passed");
  r = run("bash", ["scripts/build-seo.sh"], { stdio: "inherit" });
  if (!r.ok) fail("`bash scripts/build-seo.sh` failed. Fix the build errors and retry.");
  ok("SEO WASM build passed");
  // Size gate: EACH Worker must fit the limit on its own.
  for (const [label, wasmPath] of [["main", MAIN_WASM], ["converter", CONVERTER_WASM], ["social", SOCIAL_WASM], ["woo", WOO_WASM], ["school", SCHOOL_WASM], ["seo", SEO_WASM]]) {
    let wasmBytes;
    try {
      wasmBytes = statSync(`${ROOT}/${wasmPath}`).size;
    } catch {
      fail(`Build produced no ${wasmPath}. The build script or wrangler.toml [build] command is broken.`);
    }
    if (wasmBytes > limit) {
      fail(
        `WASM size gate (${label}): ${wasmPath} is ${wasmBytes.toLocaleString("en-US")} bytes, over the ${paid ? "Workers Paid 10 MiB" : "free-tier 3 MiB"} limit (${limit.toLocaleString("en-US")} bytes).\n` +
          "  Resolve the size problem first (docs/cutover-runbook.md prerequisite 1):\n" +
          "    - wasm-opt -Oz (binaryen) — run 'npm install' (binaryen is a devDependency) then rebuild; or\n" +
          "    - move more code out of the oversize Worker (the conversion engine was already split out 2026-11-20); or\n" +
          "    - Workers Paid ($5/mo) — re-run with --paid.",
      );
    }
    ok(`${label} WASM size ${wasmBytes.toLocaleString("en-US")} bytes is within the ${paid ? "10 MiB" : "3 MiB"} limit`);
  }

  log("\n  1. Environment configuration check");
  const toml = readFileSync(`${ROOT}/wrangler.toml`, "utf8");
  // The env's configuration spans its [env.X] header plus its [[env.X.*]]
  // subsections, up to the next [env.Y] header (or end of file).
  const envBlock = toml.split(`[env.${env}]`)[1]?.split("\n[env.")[0] ?? "";
  // P5 guard-test catch: env sections use REPLACE-ME, not the zero UUID
  // the top-level placeholder uses — reject both (and a missing id).
  if (
    !envBlock.includes("database_id") ||
    envBlock.includes("REPLACE-ME") ||
    envBlock.includes(PLACEHOLDER_ID)
  ) {
    fail(
      `wrangler.toml [env.${env}] still carries a placeholder database_id.\n` +
        "  Fill in the real ids first (docs/cutover-runbook.md step 2):\n" +
        `  staging: create wordless-staging D1/R2/KV and paste their ids;\n` +
        "  production: the SAME D1 id, R2 bucket and KV namespace as the live v1 worker.",
    );
  }
  ok(`[env.${env}] has a real database_id`);
  // The converter Worker is stateless (no D1/R2), but its [env.*] must
  // still name the service the main Worker's binding points at.
  const converterToml = readFileSync(`${ROOT}/crates/wordless-converter/wrangler.toml`, "utf8");
  if (!converterToml.includes(`[env.${env}]`) || converterToml.includes("REPLACE-ME")) {
    fail(`crates/wordless-converter/wrangler.toml has no clean [env.${env}] section. Fill in the converter's service name for ${env} first.`);
  }
  ok(`converter wrangler.toml has a clean [env.${env}] section`);
  for (const [label, tomlPath] of [["social", `${ROOT}/crates/wordless-social-worker/wrangler.toml`], ["woo", `${ROOT}/crates/wordless-woocommerce-worker/wrangler.toml`], ["school", `${ROOT}/crates/wordless-school-worker/wrangler.toml`], ["seo", `${ROOT}/crates/wordless-seo-worker/wrangler.toml`]]) {
    const subToml = readFileSync(tomlPath, "utf8");
    if (!subToml.includes(`[env.${env}]`) || subToml.includes("REPLACE-ME")) {
      fail(`${tomlPath} has no clean [env.${env}] section. Fill in the ${label} Worker's service name for ${env} first.`);
    }
    ok(`${label} wrangler.toml has a clean [env.${env}] section`);
  }

  log("\n  2. Cloudflare login check");
  const whoami = run("npx", ["wrangler", "whoami"]);
  // P5 guard-test catch: `wrangler whoami` exits 0 even when NOT
  // authenticated — it just prints "You are not authenticated." Trust
  // the output: a real login prints an Account ID line.
  const authed =
    whoami.ok &&
    /account id/i.test(whoami.stdout) &&
    !/not authenticated/i.test(whoami.stdout);
  if (!authed) {
    fail("Not logged in to Cloudflare (`wrangler whoami` shows no authenticated account). Run `npx wrangler login` first — official browser OAuth, never paste a token here.");
  }
  ok(`Logged in to Cloudflare:\n${whoami.stdout.trim().split("\n").map((l) => `     ${l}`).join("\n")}`);

  log(`\n  3. Remote D1 migrations (${env})`);
  r = run("npx", ["wrangler", "d1", "migrations", "apply", "DB", "--remote", ...envArgs], { stdio: "inherit" });
  if (!r.ok) fail("Remote D1 migrations failed. Check the output above.");
  ok("Remote D1 migrations applied (on a v1 database 0011_p3.sql, 0012 (P6), 0013 (P7), 0014 (P8), 0015 (P9 school) and 0016 (P10 SEO audit runs) are new)");

  // Social, Woo, school, SEO, then converter FIRST: the main
  // Worker's SOCIAL / WOO / SCHOOL / SEO / CONVERTER service
  // bindings resolve to wordless-<name>-<env>, so each target must
  // exist before the main Worker deploys. Social, Woo, school and
  // SEO share the main Worker's D1 — the main Worker still runs all
  // migrations (their wrangler.tomls declare no migrations_dir), so
  // deploy order among the sub-workers does not matter, only that
  // all five precede main.
  let socialUrl = null;
  let wooUrl = null;
  let schoolUrl = null;
  let seoUrl = null;
  for (const [label, dir] of [["social", "wordless-social-worker"], ["woo", "wordless-woocommerce-worker"], ["school", "wordless-school-worker"], ["seo", "wordless-seo-worker"]]) {
    log(`\n  4. Deploy ${label} (${env})`);
    r = run("npx", ["wrangler", "deploy", ...envArgs], { cwd: `${ROOT}/crates/${dir}` });
    if (!r.ok) fail(`"wrangler deploy" (${label}) failed. Check the output above.`);
    const subUrlMatch = (r.stdout + r.stderr).match(/https:\/\/[A-Za-z0-9-]+\.[A-Za-z0-9-]+\.workers\.dev/);
    if (!subUrlMatch) {
      fail(`${label} deploy reported success but no workers.dev URL was found in the output. Run \`npx wrangler deploy\` in crates/${dir} and check the printed URL.`);
    }
    ok(`${label} deployed to ${subUrlMatch[0]}`);
    if (label === "social") socialUrl = subUrlMatch[0];
    else if (label === "woo") wooUrl = subUrlMatch[0];
    else if (label === "school") schoolUrl = subUrlMatch[0];
    else seoUrl = subUrlMatch[0];
  }
  log(`\n  5. Deploy converter (${env})`);
  r = run("npx", ["wrangler", "deploy", ...envArgs], { cwd: `${ROOT}/crates/wordless-converter` });
  if (!r.ok) fail("`wrangler deploy` (converter) failed. Check the output above.");
  const converterUrlMatch = (r.stdout + r.stderr).match(/https:\/\/[A-Za-z0-9-]+\.[A-Za-z0-9-]+\.workers\.dev/);
  const converterUrl = converterUrlMatch ? converterUrlMatch[0] : null;
  if (!converterUrl) {
    fail("Converter deploy reported success but no workers.dev URL was found in the output. Run `npx wrangler deploy` in crates/wordless-converter and check the printed URL.");
  }
  ok(`Converter deployed to ${converterUrl}`);

  log(`\n  6. Deploy main (${env})`);
  r = run("npx", ["wrangler", "deploy", ...envArgs]);
  if (!r.ok) fail("`wrangler deploy` (main) failed. Check the output above.");
  const urlMatch = (r.stdout + r.stderr).match(/https:\/\/[A-Za-z0-9-]+\.[A-Za-z0-9-]+\.workers\.dev/);
  const url = urlMatch ? urlMatch[0] : null;
  if (!url) {
    fail("Main deploy reported success but no workers.dev URL was found in the output. Run `npx wrangler deploy` and check the printed URL.");
  }
  ok(`Main deployed to ${url}`);

  log("\n  7. Verify");
  try {
    const res = await fetch(`${url}/api/health`);
    if (!res.ok) fail(`Health check failed: GET ${url}/api/health returned ${res.status}. The deploy may be broken — check the dashboard and roll back if needed.`);
    ok(`Health check passed: GET ${url}/api/health returned 200`);
  } catch (e) {
    fail(`Health check failed: could not reach ${url}/api/health (${e.message}).`);
  }
  try {
    const res = await fetch(`${converterUrl}/__convert/health`);
    if (!res.ok) fail(`Converter health check failed: GET ${converterUrl}/__convert/health returned ${res.status}. The main Worker will fail conversion jobs — check the dashboard and roll back if needed.`);
    ok(`Converter health check passed: GET ${converterUrl}/__convert/health returned 200`);
  } catch (e) {
    fail(`Converter health check failed: could not reach ${converterUrl}/__convert/health (${e.message}).`);
  }
  for (const [label, url2, path] of [["social", socialUrl, "/__social/health"], ["woo", wooUrl, "/__woo/health"], ["school", schoolUrl, "/__school/health"], ["seo", seoUrl, "/__seo/health"]]) {
    try {
      const res = await fetch(`${url2}${path}`);
      if (!res.ok) fail(`${label} health check failed: GET ${url2}${path} returned ${res.status}. The main Worker will fail ${label} routes — check the dashboard and roll back if needed.`);
      ok(`${label} health check passed: GET ${url2}${path} returned 200`);
    } catch (e) {
      fail(`${label} health check failed: could not reach ${url2}${path} (${e.message}).`);
    }
  }
  try {
    const res = await fetch(`${converterUrl}/__convert/health`);
    if (!res.ok) fail(`Converter health check failed: GET ${converterUrl}/__convert/health returned ${res.status}. The main Worker will fail conversion jobs — check the dashboard and roll back if needed.`);
    ok(`Converter health check passed: GET ${converterUrl}/__convert/health returned 200`);
  } catch (e) {
    fail(`Converter health check failed: could not reach ${converterUrl}/__convert/health (${e.message}).`);
  }

  log(`\n  Done. Next: the verification + soak checklist in docs/cutover-runbook.md (step ${env === "staging" ? "5" : "7"}).`);
  log(`  Rollback, if anything is wrong: npx wrangler rollback --env ${env} (main) and the same in crates/wordless-social-worker (social), crates/wordless-woocommerce-worker (woo), crates/wordless-school-worker (school), crates/wordless-seo-worker (seo), and crates/wordless-converter (converter).`);
}

main();
