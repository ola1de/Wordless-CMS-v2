#!/usr/bin/env node
// WORDLESS v2 P5 — Rollback rehearsal.
//
// Rehearses the cutover runbook locally, end to end, against ONE shared
// D1 file (the same file both workers read in turn):
//
//   Phase A (v1 -> v2): fresh v1 D1 -> start v1 -> login + create a post
//     -> stop v1 -> copy the D1 file into v2's state dir -> apply ONLY
//     migration 0011 (the v2-only webhooks migration, exactly as the
//     real upgrade would) -> start v2 -> the v1 session must still work,
//     the v1 post must still read, and a v2 post must create cleanly.
//   Phase B (v2 -> v1, rollback): stop v2 -> copy the D1 file back ->
//     start v1 -> the same session must still work, the v2-created post
//     must still read, and a new v1 post must create cleanly.
//
// This is the proof for the doc §1.3 claim that rollback needs no data
// work: schema and sessions are identical, so the binary swap is the
// whole cutover and the whole rollback.
//
// The TS repo is NEVER modified — v1 runs from a scratch copy under
// tests/parity/work-rollback/v1 (node_modules symlinked back), exactly
// like run-parity.sh. Nothing is deployed; no Cloudflare credentials
// are involved.
//
// Usage: node tests/parity/rollback.mjs
// Exit: 0 = every check passed; 1 = any check failed (see the table).
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const PARITY = path.join(ROOT, 'tests', 'parity');
// Location of the v1 TypeScript repo. Override with V1SRC when it lives
// elsewhere, e.g. V1SRC=$HOME/Wordless-CMS node tests/parity/rollback.mjs
const V1SRC = process.env.V1SRC || path.join(process.env.HOME, 'workspace', 'wordless', 'wordless-cms');
const WORK = path.join(PARITY, 'work-rollback');
const V1_DIR = path.join(WORK, 'v1');
const V2_DIR = ROOT; // v2 runs from its own repo root, like run-parity.sh
const WRANGLER = path.join(V1SRC, 'node_modules', '.bin', 'wrangler');
const V1_PORT = 8811;
const V2_PORT = 8812;
const V1 = `http://127.0.0.1:${V1_PORT}`;
const V2 = `http://127.0.0.1:${V2_PORT}`;

process.env.TMPDIR = path.join(process.env.HOME, '.tmp-cargo');
process.env.PATH = `${path.join(process.env.HOME, '.cargo', 'bin')}:${process.env.PATH}`;

const results = [];
function check(id, ok, detail = '') {
  results.push({ id, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${id}${detail ? ` — ${detail}` : ''}`);
}

function sh(cmd, cwd, { stdio = 'pipe' } = {}) {
  return execFileSync('bash', ['-c', cmd], { cwd, stdio, encoding: 'utf8' });
}

// --- D1 state-file discovery -------------------------------------------
// Local wrangler keeps each database in
// .wrangler/state/v3/d1/miniflare-D1DatabaseObject/<hash>.sqlite (+ -wal/-shm).
function d1Files(dir) {
  const base = path.join(dir, '.wrangler', 'state', 'v3', 'd1', 'miniflare-D1DatabaseObject');
  if (!fs.existsSync(base)) return [];
  return fs.readdirSync(base)
    // The database file is <64-hex>.sqlite; `metadata.sqlite` in the
    // same directory is wrangler metadata, NOT the database — never copy it.
    .filter((f) => /^[0-9a-f]{64}\.sqlite$/.test(f))
    .map((f) => path.join(base, f));
}

function copyD1(fromDir, toDir, label) {
  const from = d1Files(fromDir);
  const to = d1Files(toDir);
  if (from.length !== 1) throw new Error(`${label}: expected exactly 1 D1 file in ${fromDir}, found ${from.length}`);
  if (to.length !== 1) throw new Error(`${label}: expected exactly 1 D1 file in ${toDir}, found ${to.length}`);
  for (const suffix of ['', '-wal', '-shm']) {
    const src = from[0] + suffix;
    if (fs.existsSync(src)) fs.copyFileSync(src, to[0] + suffix);
    else if (fs.existsSync(to[0] + suffix)) fs.unlinkSync(to[0] + suffix);
  }
  const size = fs.statSync(from[0]).size;
  console.log(`  ${label}: copied ${path.basename(from[0])} (${size} bytes) -> ${path.basename(to[0])}`);
}

// --- worker lifecycle ----------------------------------------------------
let child = null;
function startWorker(dir, port, label) {
  console.log(`== starting ${label} on :${port} ==`);
  child = spawn(WRANGLER, ['dev', '--port', String(port), '--local'], {
    cwd: dir, stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout.on('data', () => {});
  child.stderr.on('data', () => {});
}
async function stopWorker() {
  if (!child) return;
  child.kill('SIGTERM');
  await new Promise((r) => setTimeout(r, 1500));
  child = null;
}
async function waitForHealth(base, label, maxTries = 150) {
  for (let i = 0; i < maxTries; i++) {
    try {
      const r = await fetch(`${base}/api/health`);
      if (r.ok) { console.log(`  ${label} up after ~${i * 2}s`); return true; }
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 2000));
  }
  console.log(`FAIL: ${label} did not answer ${base}/api/health`);
  return false;
}

// --- HTTP helpers ---------------------------------------------------------
let cookie = '';
async function req(base, method, p, { body = null, auth = false } = {}) {
  const headers = { 'Content-Type': 'application/json' };
  if (auth && cookie) headers.Cookie = cookie;
  const r = await fetch(`${base}${p}`, {
    method, headers, body: body ? JSON.stringify(body) : undefined,
  });
  const text = await r.text();
  let json = null;
  try { json = JSON.parse(text); } catch { /* HTML/text body */ }
  return { status: r.status, json, text, setCookie: r.headers.get('set-cookie') };
}

async function main() {
  console.log('== P5 rollback: clean room ==');
  fs.rmSync(WORK, { recursive: true, force: true });
  fs.rmSync(path.join(ROOT, '.wrangler'), { recursive: true, force: true });
  fs.mkdirSync(V1_DIR, { recursive: true });

  console.log('== P5 rollback: v1 scratch copy (TS repo untouched) ==');
  sh(`(cd "${V1SRC}" && tar --exclude=node_modules --exclude=.wrangler --exclude=.git -cf - .) | (cd "${V1_DIR}" && tar --no-same-owner -xf -)`, WORK);
  fs.symlinkSync(path.join(V1SRC, 'node_modules'), path.join(V1_DIR, 'node_modules'));

  console.log('== P5 rollback: v1 migrations + seed ==');
  sh(`"${WRANGLER}" d1 migrations apply DB --local >/dev/null`, V1_DIR);
  sh(`"${WRANGLER}" d1 execute DB --local --file "${path.join(PARITY, 'seed.sql')}" >/dev/null`, V1_DIR);

  console.log('== P5 rollback: v2 migrations on an empty DB (creates its state file) ==');
  sh(`"${WRANGLER}" d1 migrations apply DB --local >/dev/null`, V2_DIR);
  console.log(`  v1 D1: ${d1Files(V1_DIR).map((f) => path.basename(f))}`);
  console.log(`  v2 D1: ${d1Files(V2_DIR).map((f) => path.basename(f))}`);

  // ---- Phase A: v1 live -------------------------------------------------
  startWorker(V1_DIR, V1_PORT, 'v1');
  if (!await waitForHealth(V1, 'v1')) process.exit(1);

  let r = await req(V1, 'GET', '/api/health');
  check('A1 v1 health 200', r.status === 200, `status ${r.status}`);

  r = await req(V1, 'POST', '/api/auth/login', { body: { login: 'admin', password: 'parity-admin-2026' } });
  const loginOk = r.status === 200 && !!r.setCookie;
  if (r.setCookie) cookie = r.setCookie.split(';')[0];
  check('A2 v1 login 200 + session cookie', loginOk, `status ${r.status}`);

  r = await req(V1, 'GET', '/api/auth/me', { auth: true });
  check('A3 v1 /api/auth/me 200 (session works)', r.status === 200 && r.json?.data?.user?.login === 'admin', `status ${r.status}`);

  r = await req(V1, 'POST', '/api/posts', { auth: true, body: {
    title: 'Rollback rehearsal (v1)', slug: 'rollback-v1',
    content: '<!-- wp:paragraph --><p>Created on v1 before the swap.</p><!-- /wp:paragraph -->',
    status: 'publish',
  } });
  const v1PostId = r.json?.data?.id;
  check('A4 v1 creates post', r.status === 201 && !!v1PostId, `status ${r.status} id ${v1PostId}`);

  r = await req(V1, 'GET', '/rollback-v1');
  check('A5 v1 public page for the post 200', r.status === 200, `status ${r.status}`);

  // ---- Swap: v1 -> v2, same D1 file --------------------------------------
  await stopWorker();
  console.log('== P5 rollback: swapping the D1 file v1 -> v2 ==');
  copyD1(V1_DIR, V2_DIR, 'v1->v2');
  console.log('== P5 rollback: applying migration 0011 only (the real upgrade path) ==');
  const migOut = sh(`"${WRANGLER}" d1 migrations apply DB --local`, V2_DIR);
  const mig0011 = /0011/.test(migOut);
  check('A6 migration 0011 applied to the v1 file', mig0011, migOut.split('\n').filter((l) => /0011|migrations? applied|No migrations/.test(l)).join(' | ').slice(0, 160));

  startWorker(V2_DIR, V2_PORT, 'v2');
  if (!await waitForHealth(V2, 'v2', 300)) process.exit(1);

  // THE session-survival proof: the exact cookie issued by v1, unchanged.
  r = await req(V2, 'GET', '/api/auth/me', { auth: true });
  check('A7 v2 /api/auth/me 200 with the UNCHANGED v1 cookie', r.status === 200 && r.json?.data?.user?.login === 'admin', `status ${r.status}`);

  r = await req(V2, 'GET', `/api/posts/${v1PostId}`, { auth: true });
  check('A8 v2 reads the v1-created post', r.status === 200 && r.json?.data?.title === 'Rollback rehearsal (v1)', `status ${r.status}`);

  r = await req(V2, 'GET', '/rollback-v1');
  check('A9 v2 public page for the v1 post 200', r.status === 200, `status ${r.status}`);

  r = await req(V2, 'POST', '/api/posts', { auth: true, body: {
    title: 'Rollback rehearsal (v2)', slug: 'rollback-v2',
    content: '<!-- wp:paragraph --><p>Created on v2 after the swap.</p><!-- /wp:paragraph -->',
    status: 'publish',
  } });
  const v2PostId = r.json?.data?.id;
  check('A10 v2 creates post', r.status === 201 && !!v2PostId, `status ${r.status} id ${v2PostId}`);

  // ---- Phase B: rollback v2 -> v1, same D1 file --------------------------
  await stopWorker();
  console.log('== P5 rollback: swapping the D1 file v2 -> v1 ==');
  copyD1(V2_DIR, V1_DIR, 'v2->v1');

  startWorker(V1_DIR, V1_PORT, 'v1 (rolled back)');
  if (!await waitForHealth(V1, 'v1-rollback')) process.exit(1);

  r = await req(V1, 'GET', '/api/auth/me', { auth: true });
  check('B1 v1 /api/auth/me 200 after rollback (same cookie)', r.status === 200 && r.json?.data?.user?.login === 'admin', `status ${r.status}`);

  r = await req(V1, 'GET', `/api/posts/${v2PostId}`, { auth: true });
  check('B2 v1 reads the v2-created post after rollback', r.status === 200 && r.json?.data?.title === 'Rollback rehearsal (v2)', `status ${r.status}`);

  r = await req(V1, 'GET', '/rollback-v2');
  check('B3 v1 public page for the v2 post 200 after rollback', r.status === 200, `status ${r.status}`);

  r = await req(V1, 'POST', '/api/posts', { auth: true, body: {
    title: 'Rollback rehearsal (back on v1)', slug: 'rollback-v1-again',
    content: '<!-- wp:paragraph --><p>Created on v1 after the rollback.</p><!-- /wp:paragraph -->',
    status: 'publish',
  } });
  check('B4 v1 creates post after rollback', r.status === 201, `status ${r.status}`);

  r = await req(V1, 'GET', '/api/posts?per_page=100', { auth: true });
  const titles = (r.json?.data?.posts ?? []).map((p) => p.title);
  const allThree = ['Rollback rehearsal (v1)', 'Rollback rehearsal (v2)', 'Rollback rehearsal (back on v1)'].every((t) => titles.includes(t));
  check('B5 v1 post list contains all three rehearsal posts', r.status === 200 && allThree, `found ${titles.filter((t) => t.startsWith('Rollback')).length}/3`);

  await stopWorker();
  const failed = results.filter((x) => !x.ok);
  console.log(`\n== P5 rollback: ${results.length - failed.length}/${results.length} checks passed ==`);
  if (failed.length) { console.log('FAILED:', failed.map((f) => f.id).join(', ')); process.exitCode = 1; }
}

main().catch(async (err) => {
  console.error('rollback rehearsal failed:', err);
  await stopWorker();
  process.exit(1);
});
