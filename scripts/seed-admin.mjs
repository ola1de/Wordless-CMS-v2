#!/usr/bin/env node
/**
 * WORDLESS v2.0 first-administrator seed (Phase P5).
 *
 * Creates the first administrator so the staging admin works end to
 * end. The admin UI cannot create the first admin (creating users
 * requires an admin), so this script fills the gap. Mirrors the v1
 * script (wordless-cms/scripts/seed-admin.mjs) — same rows, same
 * PBKDF2 format `pbkdf2$100000$<base64url salt>$<base64url hash>`,
 * same refusal when an administrator already exists.
 *
 * Usage:
 *   npm run seed-admin                              Interactive prompts, local D1
 *   npm run seed-admin -- --login admin --email a@example.com
 *                                                     Non-interactive (password prompted)
 *   npm run seed-admin -- --remote --env staging ...  Remote D1 (requires a
 *                                                     Cloudflare login first).
 *
 * --remote REQUIRES --env staging|production: unlike v1, there is no
 * silent production default — the first v2 admin belongs on staging
 * (production already has its admin from v1; seeding production would
 * be refused anyway because an administrator exists there).
 *
 * The password is never printed, logged, or stored anywhere except
 * the database. Secrets are never involved: this script takes no
 * Cloudflare credential and never asks for one.
 */
import { spawnSync } from "node:child_process";
import { randomBytes, pbkdf2Sync } from "node:crypto";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const ARGS = process.argv.slice(2);
const REMOTE = ARGS.includes("--remote");
const KNOWN_ENVS = new Set(["staging", "production"]);

/** Read one --flag value (or null when absent). */
function flag(name) {
  const i = ARGS.indexOf(name);
  return i >= 0 && i + 1 < ARGS.length ? ARGS[i + 1] : null;
}

const ENV = flag("--env") ?? null;

const MIN_PASSWORD = 8;
const MAX_PASSWORD = 128;
const LOGIN_RE = /^[A-Za-z0-9_.\-]+$/;
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

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

function run(cmd, args, { stdio = "pipe" } = {}) {
  // On Windows, npm/npx are .cmd shims and Node cannot spawn them without
  // a shell (ENOENT for `npm`, EINVAL for `npm.cmd` — confirmed on a real
  // Windows install for the v1 installer). Run through cmd.exe on Windows,
  // quoting arguments first: with shell:true Node concatenates args
  // unescaped, so values with spaces (e.g. Windows paths) need quoting.
  const home = process.env.HOME ?? process.env.USERPROFILE ?? "";
  const env = {
    ...process.env,
    PATH: `${home}/.cargo/bin:${process.env.PATH ?? ""}`,
    TMPDIR: `${home}/.tmp-cargo`,
  };
  const useShell = process.platform === "win32";
  const finalArgs = useShell ? args.map(quoteArg) : args;
  const result = spawnSync(cmd, finalArgs, { cwd: ROOT, stdio, encoding: "utf8", shell: useShell, env });
  return {
    ok: result.status === 0,
    code: result.status ?? 1,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function ask(question) {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  return new Promise((resolve) => {
    rl.question(question, (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });
}

/** Prompt for a secret without echoing it (prints * per character). Falls back to a plain prompt when stdin is not a TTY. */
function askHidden(question) {
  if (!process.stdin.isTTY) return ask(question);
  return new Promise((resolve) => {
    process.stdout.write(question);
    const stdin = process.stdin;
    stdin.setRawMode(true);
    stdin.resume();
    let value = "";
    const onData = (chunk) => {
      const ch = chunk.toString("utf8");
      if (ch === "\n" || ch === "\r" || ch === "\u0004") {
        stdin.setRawMode(false);
        stdin.pause();
        stdin.removeListener("data", onData);
        process.stdout.write("\n");
        resolve(value);
      } else if (ch === "\u0003") {
        // Ctrl+C — restore the terminal before exiting.
        stdin.setRawMode(false);
        process.stdout.write("\n");
        process.exit(130);
      } else if (ch === "\u007f" || ch === "\b") {
        if (value.length > 0) {
          value = value.slice(0, -1);
          process.stdout.write("\b \b");
        }
      } else if (ch >= " " && ch !== "\u007f") {
        value += ch;
        process.stdout.write("*");
      }
    };
    stdin.on("data", onData);
  });
}

async function confirm(question) {
  const answer = await ask(`${question} [y/N] `);
  return answer.toLowerCase() === "y" || answer.toLowerCase() === "yes";
}

function validateInput({ login, email, password, confirm }) {
  if (login.length < 1 || login.length > 60) fail("Login must be 1–60 characters.");
  if (!LOGIN_RE.test(login)) fail("Login may only contain letters, digits, underscore, dot and hyphen.");
  if (email.length === 0 || email.length > 100 || !EMAIL_RE.test(email)) {
    fail("Email must be a valid email address.");
  }
  if (password.length < MIN_PASSWORD) fail(`Password must be at least ${MIN_PASSWORD} characters.`);
  if (password.length > MAX_PASSWORD) fail(`Password must be at most ${MAX_PASSWORD} characters.`);
  if (password !== confirm) fail("Password and confirmation do not match.");
}

/** Base64url without padding — the same encoding as the Rust auth crate. */
function base64url(bytes) {
  return Buffer.from(bytes).toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** PBKDF2 hash in the repo format: pbkdf2$100000$<salt>$<hash>. */
function hashPassword(password) {
  const salt = randomBytes(16);
  const hash = pbkdf2Sync(password, salt, 100000, 32, "sha256");
  return `pbkdf2$100000$${base64url(salt)}$${base64url(hash)}`;
}

/** SQL-escape a string literal (single quotes doubled). */
function sqlStr(s) {
  return `'${String(s).replace(/'/g, "''")}'`;
}

function d1Args() {
  return REMOTE ? ["--remote", "--env", ENV] : ["--local"];
}

/** Run one D1 statement and return the parsed --json rows (or fail). */
function d1Query(sql, what) {
  const r = run("npx", ["wrangler", "d1", "execute", "DB", ...d1Args(), "--json", "--command", sql]);
  if (!r.ok) {
    fail(`Could not query the ${REMOTE ? "remote" : "local"} database (${what}).\n${r.stderr || r.stdout}`);
  }
  let parsed;
  try {
    parsed = JSON.parse(r.stdout);
  } catch {
    fail(`Could not parse the database response (${what}).`);
  }
  const first = Array.isArray(parsed) ? parsed[0] : parsed;
  return first?.results ?? [];
}

/** Run a multi-statement SQL file against D1. */
function d1ExecFile(path, what) {
  const r = run("npx", ["wrangler", "d1", "execute", "DB", ...d1Args(), "--file", path]);
  if (!r.ok) {
    fail(`Could not write to the ${REMOTE ? "remote" : "local"} database (${what}).\n${r.stderr || r.stdout}`);
  }
  return r.stdout;
}

async function main() {
  if (ARGS.includes("--help") || ARGS.includes("-h")) {
    log("Usage: npm run seed-admin [-- --login <login> --email <email> --password <pw> --confirm <pw>] [--remote --env staging|production]");
    log("  Creates the first administrator. Refuses when one already exists.");
    log("  Local D1 by default; --remote requires --env and a Cloudflare login (`wrangler whoami`).");
    return;
  }

  requireWrangler();
  if (REMOTE && (ENV === null || !KNOWN_ENVS.has(ENV))) {
    fail('Remote seeds require --env staging|production (no silent default).\n  npm run seed-admin -- --remote --env staging');
  }
  if (ENV !== null && !KNOWN_ENVS.has(ENV)) {
    fail(`Unknown environment "${ENV}". Use --env staging or --env production.`);
  }

  log("\n  WORDLESS v2 — seed the first administrator\n");

  if (REMOTE) {
    const whoami = run("npx", ["wrangler", "whoami"]);
    // `wrangler whoami` exits 0 even when NOT authenticated — trust the
    // output, not the code (same guard as scripts/deploy.mjs).
    const authed =
      whoami.ok &&
      /account id/i.test(whoami.stdout) &&
      !/not authenticated/i.test(whoami.stdout);
    if (!authed) {
      fail("Not logged in to Cloudflare (`wrangler whoami` shows no authenticated account). Run `npx wrangler login` first, or seed locally (without --remote).");
    }
    ok("Logged in to Cloudflare (remote seed).");
    if (!(await confirm(`Create the administrator on the REMOTE database (${ENV})?`))) {
      log("  Cancelled. Nothing was changed.");
      return;
    }
  }

  let login = flag("--login") ?? "";
  let email = flag("--email") ?? "";
  let password = flag("--password") ?? null;
  let confirmPw = flag("--confirm") ?? null;

  if (login === "") login = await ask("  Admin login: ");
  if (email === "") email = await ask("  Admin email: ");
  if (password === null) password = await askHidden("  Password (min 8 characters): ");
  if (confirmPw === null) confirmPw = await askHidden("  Confirm password: ");

  login = login.trim();
  email = email.trim().toLowerCase();
  validateInput({ login, email, password, confirm: confirmPw });

  // Refusal path: an administrator already exists.
  const admins = d1Query(
    `SELECT COUNT(*) AS n FROM wp_usermeta WHERE meta_key = 'wp_capabilities' AND meta_value LIKE '%"administrator":true%'`,
    "checking for an existing administrator",
  );
  if (Number(admins[0]?.n ?? 0) > 0) {
    fail(
      "An administrator already exists — refusing to seed another one.\n" +
        "  To regain access, ask an existing administrator to reset your password\n" +
        "  (Users page → Reset password), or use the password-reset flow.",
    );
  }

  const dupLogin = d1Query(`SELECT ID FROM wp_users WHERE user_login = ${sqlStr(login)} LIMIT 1`, "checking the login");
  if (dupLogin.length > 0) fail(`A user with login "${login}" already exists. Choose a different login.`);
  const dupEmail = d1Query(`SELECT ID FROM wp_users WHERE user_email = ${sqlStr(email)} LIMIT 1`, "checking the email");
  if (dupEmail.length > 0) fail(`A user with email "${email}" already exists. Choose a different email.`);

  const now = new Date().toISOString().replace("T", " ").replace(/\.\d+Z$/, "");
  const nicename = login.toLowerCase().replace(/[^a-z0-9]+/g, "-");
  const hash = hashPassword(password);
  // Clear the plaintext from memory as soon as the hash exists.
  password = null;
  confirmPw = null;

  const dir = mkdtempSync(join(tmpdir(), "wordless-seed-"));
  const file = join(dir, "seed.sql");
  try {
    writeFileSync(
      file,
      `INSERT INTO wp_users (user_login, user_pass, user_nicename, user_email, user_url, user_registered, display_name)\n` +
        `VALUES (${sqlStr(login)}, ${sqlStr(hash)}, ${sqlStr(nicename)}, ${sqlStr(email)}, '', ${sqlStr(now)}, ${sqlStr(login)});\n` +
        `INSERT INTO wp_usermeta (user_id, meta_key, meta_value)\n` +
        `VALUES ((SELECT ID FROM wp_users WHERE user_login = ${sqlStr(login)}), 'wp_capabilities', '{"administrator":true}');\n`,
    );
    d1ExecFile(file, "creating the administrator");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }

  const created = d1Query(
    `SELECT ID, user_login, user_email FROM wp_users WHERE user_login = ${sqlStr(login)} LIMIT 1`,
    "reading back the created administrator",
  );
  if (created.length === 0) {
    fail("The insert reported success but the user could not be read back. Check the database manually.");
  }
  ok(`Administrator created (id ${created[0].ID}, login "${created[0].user_login}", ${created[0].user_email})`);
  log(`  Sign in at /admin with login "${login}".`);
}

main().catch((err) => {
  console.error(`\n  ✗ Seed failed: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
