/**
 * WORDLESS v2 P5 — Parity harness (differential tester).
 *
 * Runs the corpus from corpus.mjs against two live workers (v1 TypeScript
 * and v2 Rust) in lockstep, capturing per-worker variables, and diffs
 * status + content-type + location + body. Zero unexplained diffs is the
 * bar. Latency is recorded per step and aggregated (p50/p95) per group.
 *
 * Usage:
 *   node harness.mjs --v1 http://127.0.0.1:8801 --v2 http://127.0.0.1:8802 \
 *     --fixtures ./fixtures --out ./results/parity-report.json
 *
 * The harness does NOT start the workers — run-parity.sh does that (it also
 * applies migrations + seed first). It exits 0 only when there are no
 * unexplained diffs and no MATCH-BUT-UNEXPECTED steps; otherwise 1.
 *
 * Normalisation (what is folded before comparison, and why):
 * - ISO/SQL/RFC-822 datetimes under time-ish keys (except publishAt, which
 *   the corpus sets explicitly and must match exactly) -> <time>:
 *   wall-clock creation timestamps differ by seconds between workers.
 * - secret-ish keys (token, secret, password, hash, salt, totp, recovery,
 *   signature, api key, set-cookie) -> <secret>: random or credential values.
 * - UUID strings (order/cart/backup/conversion/AI ids are randomUUID())
 *   -> <uuid>; media-key hex segments -> <hex>.
 * - `version` -> <version> only on steps flagged normalizeVersion
 *   (health: 1.1.0 vs 2.0.0-dev is intentional; public pages carry the
 *   version in the footer).
 * Autoincrement integer ids are compared EXACTLY: the lockstep corpus from
 * an identical seed must assign identical ids; any divergence is a real
 * behavioural diff and must surface, not be folded away.
 */
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { STEPS } from './corpus.mjs';
import { KNOWN_DIFFS } from './known-diffs.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// ---------------------------------------------------------------- config
const argv = process.argv.slice(2);
function arg(name, fallback) {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
}
const V1 = arg('--v1', 'http://127.0.0.1:8801');
const V2 = arg('--v2', 'http://127.0.0.1:8802');
const FIXTURES = arg('--fixtures', path.join(__dirname, 'fixtures'));
const OUT = arg('--out', path.join(__dirname, 'results', 'parity-report.json'));
const LATENCY_OUT = arg('--latency-out', path.join(__dirname, 'results', 'latency.json'));

const SECRETS = {
  mock: 'parity-mock-secret',
  paystack: 'sk_test_parity_paystack',
  flutterwaveSecret: 'FLWSECK_TEST-parity',
  flutterwaveHash: 'flw-parity-hash',
  stripeSecretKey: 'sk_test_parity_stripe',
  stripeWebhookSecret: 'whsec_parity_test',
};

// ------------------------------------------------------------ normaliser
const UUID_RE = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;
const HEX_SEG_RE = /([0-9a-f]{16,})/gi;
const ISO_DATE_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?$/;
const SQL_DATE_RE = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/;
const RFC822_RE = /^[A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} (GMT|UTC)$/;
const TIME_SUFFIXES = ['createdat', 'updatedat', 'date', 'time', 'lastseen', 'firstseen', 'registered', 'appliedat', 'modified', 'dategmt', 'modifiedgmt', 'commentdate', 'expiresat', 'publishedat', 'deletedat', 'lockeduntil', 'attemptedat', 'lastactivity', 'postdate', 'postmodified', 'postdategmt', 'convertedat', 'at'];
const SECRET_KEY_END = /(token|secret|password|passwd|hash|salt|totp|otpauth|recovery|signature|api[_-]?key|set-cookie|cookie|checksum)$/i;
const SECRET_KEY_START = /^(recovery|totp)/i;
function isSecretKey(key) {
  return SECRET_KEY_END.test(key) || SECRET_KEY_START.test(key);
}
function isTimeKey(key) {
  // Squash snake_case/camelCase so dateGmt, date_gmt, createdAt and
  // created_at all match. publishAt is EXCLUDED: the corpus sets it
  // explicitly and the scheduled value must match exactly.
  const sq = key.toLowerCase().replace(/_/g, '');
  if (sq === 'publishat') return false;
  return TIME_SUFFIXES.some((s) => sq.endsWith(s));
}

function normalizeString(s, step) {
  let out = s.replace(UUID_RE, '<uuid>');
  // ISO datetimes embedded in message strings (e.g. lockout "Try again
  // after 2026-11-19T21:21:54.671Z") and the mock AI's "Request #123" are
  // wall-clock / random values, not behaviour.
  out = out.replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z/g, '<time>');
  out = out.replace(/Request #\d+/g, 'Request #<n>');
  // Secrets embedded in URLs (otpauth://...?secret=...).
  out = out.replace(/([?&]secret=)[A-Z2-7]+/gi, '$1<secret>');
  // Random hex ids that are not UUIDs: 32-hex backup ids, 40-hex
  // checksums, media-key prefixes.
  out = out.replace(/\b[0-9a-f]{40}\b/gi, '<hex>');
  out = out.replace(/\b[0-9a-f]{32}\b/gi, '<hex>');
  out = out.replace(/(\/media\/)[0-9a-f]{8,}-/gi, '$1<hex>-');
  if (out.includes('media/') || out.includes('uploads/')) {
    out = out.replace(HEX_SEG_RE, '<hex>');
  }
  if (step?.normalizeVersion) out = out.replace(/1\.1\.0|2\.0\.0-dev/g, '<version>');
  return out;
}

function normalizeValue(v, step, key = '') {
  if (typeof v === 'string') {
    if (isSecretKey(key)) return '<secret>';
    if (isTimeKey(key) && (ISO_DATE_RE.test(v) || SQL_DATE_RE.test(v) || RFC822_RE.test(v))) return '<time>';
    if (key === 'objectKey') return normalizeString(v, step).replace(/^[0-9a-f]{8,}-/, '<hex>-');
    return normalizeString(v, step);
  }
  if (typeof v === 'number') {
    // Measured wall time in a report (conversion durationMs), not a
    // configured value — both workers measure their own speed.
    if (key.toLowerCase().replace(/_/g, '') === 'durationms') return '<duration>';
    return v;
  }
  if (Array.isArray(v)) {
    if (isSecretKey(key)) return '<secret>';
    return v.map((item) => normalizeValue(item, step, key));
  }
  if (v !== null && typeof v === 'object') {
    const out = {};
    for (const [k, val] of Object.entries(v)) {
      if (k === 'version' && step?.normalizeVersion) { out[k] = '<version>'; continue; }
      out[k] = normalizeValue(val, step, k);
    }
    return out;
  }
  return v;
}

function normalizeText(text, step) {
  let out = text
    .replace(UUID_RE, '<uuid>')
    .replace(/[?&]v=[0-9a-f]{8,}/gi, '')
    .replace(/\b(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun), \d{2} (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) \d{4} \d{2}:\d{2}:\d{2} (?:GMT|UTC)\b/g, '<time>')
    .replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z/g, '<time>')
    .replace(HEX_SEG_RE, (m, g) => (g.length >= 32 ? '<hex>' : m));
  if (step?.normalizeVersion) out = out.replace(/1\.1\.0|2\.0\.0-dev/g, '<version>');
  return out;
}

// ------------------------------------------------------------ deep compare
function firstDiff(a, b, pathStr = '$') {
  if (a === b) return null;
  if (typeof a !== typeof b) return { path: pathStr, v1: a, v2: b, why: `type ${typeof a} vs ${typeof b}` };
  if (a === null || b === null) return { path: pathStr, v1: a, v2: b, why: 'null mismatch' };
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b)) return { path: pathStr, v1: a, v2: b, why: 'array vs non-array' };
    if (a.length !== b.length) return { path: pathStr, v1: `length ${a.length}`, v2: `length ${b.length}`, why: 'array length' };
    for (let i = 0; i < a.length; i++) {
      const d = firstDiff(a[i], b[i], `${pathStr}[${i}]`);
      if (d) return d;
    }
    return null;
  }
  if (typeof a === 'object') {
    const ka = Object.keys(a).sort();
    const kb = Object.keys(b).sort();
    for (const k of ka) {
      if (!(k in b)) return { path: `${pathStr}.${k}`, v1: a[k], v2: undefined, why: 'key missing in v2' };
    }
    for (const k of kb) {
      if (!(k in a)) return { path: `${pathStr}.${k}`, v1: undefined, v2: b[k], why: 'key extra in v2' };
    }
    for (const k of ka) {
      const d = firstDiff(a[k], b[k], `${pathStr}.${k}`);
      if (d) return d;
    }
    return null;
  }
  return { path: pathStr, v1: a, v2: b, why: 'value mismatch' };
}

// ------------------------------------------------------------ TOTP (RFC 6238)
function base32Decode(s) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  let bits = 0;
  let value = 0;
  const bytes = [];
  for (const ch of s.toUpperCase().replace(/=+$/, '')) {
    const idx = alphabet.indexOf(ch);
    if (idx < 0) throw new Error(`bad base32 char ${ch}`);
    value = (value << 5) | idx;
    bits += 5;
    if (bits >= 8) { bytes.push((value >>> (bits - 8)) & 0xff); bits -= 8; }
  }
  return Buffer.from(bytes);
}
function totp(secret, stepSeconds = 30, digits = 6) {
  const counter = Math.floor(Date.now() / 1000 / stepSeconds);
  const key = base32Decode(secret);
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(counter));
  const hmac = crypto.createHmac('sha1', key).update(msg).digest();
  const offset = hmac[hmac.length - 1] & 0x0f;
  const code = ((hmac[offset] & 0x7f) << 24) | ((hmac[offset + 1] & 0xff) << 16) | ((hmac[offset + 2] & 0xff) << 8) | (hmac[offset + 3] & 0xff);
  return String(code % 10 ** digits).padStart(digits, '0');
}

// ------------------------------------------------------------ variable resolution
function resolve(value, vars) {
  if (typeof value === 'string') {
    const whole = value.match(/^\{\{([a-zA-Z0-9_]+)\}\}$/);
    if (whole) return vars[whole[1]];
    return value.replace(/\{\{([a-zA-Z0-9_]+)\}\}/g, (_, name) => String(vars[name] ?? ''));
  }
  if (Array.isArray(value)) return value.map((v) => resolve(v, vars));
  if (value !== null && typeof value === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = resolve(v, vars);
    return out;
  }
  return value;
}

function getPath(obj, dotted) {
  return dotted.split('.').reduce((cur, seg) => (cur === undefined || cur === null ? undefined : cur[seg]), obj);
}

// ------------------------------------------------------------ webhook bodies
function buildWebhook(step, vars) {
  const ref = vars.orderRef;
  switch (step.webhook) {
    case 'mock': {
      const body = JSON.stringify({ event: 'payment.paid', reference: ref });
      return { body, headers: { 'x-mock-signature': crypto.createHmac('sha256', SECRETS.mock).update(body).digest('hex') } };
    }
    case 'mock-bad': {
      const body = JSON.stringify({ event: 'payment.paid', reference: ref });
      return { body, headers: { 'x-mock-signature': 'deadbeef' } };
    }
    case 'paystack': {
      const body = JSON.stringify({ event: 'charge.success', data: { reference: ref, status: 'success' } });
      return { body, headers: { 'x-paystack-signature': crypto.createHmac('sha512', SECRETS.paystack).update(body).digest('hex') } };
    }
    case 'flutterwave': {
      const body = JSON.stringify({ event: 'charge.completed', data: { tx_ref: ref, status: 'successful' } });
      return { body, headers: { 'verif-hash': SECRETS.flutterwaveHash } };
    }
    case 'stripe': {
      const body = JSON.stringify({ type: 'checkout.session.completed', data: { object: { id: ref, payment_status: 'paid' } } });
      const t = Math.floor(Date.now() / 1000);
      const v1 = crypto.createHmac('sha256', SECRETS.stripeWebhookSecret).update(`${t}.${body}`).digest('hex');
      return { body, headers: { 'stripe-signature': `t=${t},v1=${v1}` } };
    }
    default:
      throw new Error(`unknown webhook kind ${step.webhook}`);
  }
}

// ------------------------------------------------------------ one request
async function doRequest(base, step, vars, cookies, label) {
  const url = base + resolve(step.path, vars);
  const headers = {};
  const cookie = step.auth && step.auth !== 'none' && step.auth !== 'anon' ? cookies[step.auth] : null;
  if (cookie) headers.cookie = cookie;
  // P12: per-step extra headers (e.g. X-Wordless-Site), with
  // {{var}} resolution like path/body. Applied after the auth
  // cookie so a step can deliberately override it.
  if (step.headers !== undefined) {
    Object.assign(headers, resolve(step.headers, vars));
  }
  let body;
  if (step.webhook) {
    const wh = buildWebhook(step, vars);
    body = wh.body;
    Object.assign(headers, wh.headers);
    headers['content-type'] = 'application/json';
  } else if (step.body !== undefined) {
    body = JSON.stringify(resolve(step.body, vars));
    headers['content-type'] = 'application/json';
  } else if (step.form !== undefined) {
    const params = new URLSearchParams();
    for (const [k, v] of Object.entries(resolve(step.form, vars))) params.set(k, String(v));
    body = params.toString();
    headers['content-type'] = 'application/x-www-form-urlencoded';
  } else if (step.multipart !== undefined) {
    const form = new FormData();
    for (const part of step.multipart) {
      if (part.value !== undefined) {
        form.append(part.name, String(resolve(part.value, vars)));
        continue;
      }
      const filePath = path.join(FIXTURES, part.file);
      const buf = fs.readFileSync(filePath);
      form.append(part.name, new Blob([buf], { type: part.type }), part.filename);
    }
    body = form;
  }
  const t0 = performance.now();
  const res = await fetch(url, { method: step.method, headers, body, redirect: 'manual' });
  const ms = performance.now() - t0;
  const rawText = await res.text();
  const setCookie = res.headers.get('set-cookie');
  return { status: res.status, contentType: res.headers.get('content-type') ?? '', location: res.headers.get('location') ?? '', rawText, setCookie, ms, label };
}

function parseBodyIfJson(rawText, contentType) {
  if (contentType.includes('application/json')) {
    try { return JSON.parse(rawText); } catch { return { __unparsed: rawText }; }
  }
  return rawText;
}

// ------------------------------------------------------------ run one step on both workers
async function runStep(step, state) {
  const results = {};
  for (const which of ['v1', 'v2']) {
    const base = which === 'v1' ? V1 : V2;
    const vars = state[which].vars;
    const cookies = state[which].cookies;
    if (step.compute === 'totp') {
      const secret = vars[step.from];
      const code = totp(secret);
      vars[step.as] = code;
      results[which] = { computed: true, ms: 0 };
      continue;
    }
    let attempts = 0;
    let last;
    const deadline = Date.now() + (step.poll?.timeoutMs ?? 0);
    do {
      attempts++;
      last = await doRequest(base, step, vars, cookies, which);
      if (!step.poll) break;
      const parsed = parseBodyIfJson(last.rawText, last.contentType);
      const cur = getPath(parsed, step.poll.path);
      if (cur === step.poll.equals) break;
      if (Date.now() >= deadline) break;
      await new Promise((r) => setTimeout(r, 500));
    } while (Date.now() < deadline);
    last.attempts = attempts;
    // captures (per worker, from raw values)
    if (step.capture) {
      const parsed = parseBodyIfJson(last.rawText, last.contentType);
      for (const [name, dotted] of Object.entries(step.capture)) {
        const val = getPath(parsed, dotted);
        if (val === undefined) last.captureFailed = last.captureFailed ?? [];
        if (val === undefined) last.captureFailed.push(`${name} <- ${dotted}`);
        else vars[name] = val;
      }
    }
    if (step.captureCookie && last.setCookie) {
      // first cookie in the header (the worker sets one session/cart cookie)
      cookies[step.auth === 'anon' ? 'cart' : step.captureCookie] = last.setCookie.split(';')[0];
    }
    results[which] = last;
  }
  return results;
}

// ------------------------------------------------------------ compare
function stripPaths(obj, paths) {
  for (const p of paths) {
    const parts = p.split('.');
    let cur = obj;
    for (let i = 0; i < parts.length - 1; i++) {
      if (cur === undefined || cur === null) break;
      cur = cur[parts[i]];
    }
    if (cur && typeof cur === 'object') delete cur[parts[parts.length - 1]];
  }
}

function compareStep(step, r1, r2) {
  const issues = [];
  if (step.v2only) {
    // P12 fix (was the P11 harness gap): v2-only steps previously
    // returned 'v2-only' without ANY check, so a wrong v2 status was
    // silently counted as a pass. Validate v2's status (and body,
    // when the step declares one) against the step's expectation.
    // Every v2-only step in the corpus carries `expect` (verified
    // 2026-11-20: 176/176); a step without one is a corpus bug and
    // is flagged rather than passed.
    if (step.expect === undefined) {
      return { outcome: 'unexpected', issues: [{ kind: 'expect-v2', v1: r1.status, v2: r2.status, why: 'v2-only step has no expect status — corpus bug' }] };
    }
    if (r2.status !== step.expect) {
      issues.push({ kind: 'expect-v2', v1: r1.status, v2: r2.status, why: `v2 returned ${r2.status}, expected ${step.expect}` });
    }
    if (step.expectBody !== undefined && !r2.rawText.includes(step.expectBody)) {
      issues.push({ kind: 'expectBody-v2', why: `v2 body does not contain ${JSON.stringify(step.expectBody)}` });
    }
    return { outcome: issues.length ? 'unexpected' : 'v2-only', issues };
  }
  if (r1.status !== r2.status) issues.push({ kind: 'status', v1: r1.status, v2: r2.status });
  const ct1 = (r1.contentType || '').split(';')[0].trim();
  const ct2 = (r2.contentType || '').split(';')[0].trim();
  if (ct1 !== ct2) issues.push({ kind: 'content-type', v1: ct1, v2: ct2 });
  // Location headers carry fresh ids (order UUIDs in shop redirects);
  // fold UUIDs/hex the same way bodies are folded before comparing.
  const loc1 = r1.location ? normalizeString(r1.location, step) : r1.location;
  const loc2 = r2.location ? normalizeString(r2.location, step) : r2.location;
  if (loc1 !== loc2) issues.push({ kind: 'location', v1: r1.location, v2: r2.location });
  const mode = step.compare ?? 'json';
  if (mode === 'status-only') {
    // nothing more to compare
  } else if (mode === 'json') {
    const p1 = parseBodyIfJson(r1.rawText, r1.contentType);
    const p2 = parseBodyIfJson(r2.rawText, r2.contentType);
    const n1 = normalizeValue(p1, step);
    const n2 = normalizeValue(p2, step);
    // Documented additive fields only (see known-diffs.mjs): strip exactly
    // those paths so any OTHER diff on the step still surfaces.
    if (step.ignorePaths) { stripPaths(n1, step.ignorePaths); stripPaths(n2, step.ignorePaths); }
    const d = firstDiff(n1, n2);
    if (d) issues.push({ kind: 'body', ...d });
  } else if (mode === 'text') {
    const n1 = normalizeText(r1.rawText, step);
    const n2 = normalizeText(r2.rawText, step);
    if (n1 !== n2) {
      const lines1 = n1.split('\n');
      const lines2 = n2.split('\n');
      let firstLine = -1;
      for (let i = 0; i < Math.max(lines1.length, lines2.length); i++) {
        if (lines1[i] !== lines2[i]) { firstLine = i; break; }
      }
      // Character-level first difference on that line, with context, so a
      // one-line <script> or <link> diff is pinpointed instead of dumped.
      const l1 = lines1[firstLine] ?? '';
      const l2 = lines2[firstLine] ?? '';
      let firstChar = -1;
      for (let i = 0; i < Math.max(l1.length, l2.length); i++) {
        if (l1[i] !== l2[i]) { firstChar = i; break; }
      }
      const ctx = (s, i) => s.slice(Math.max(0, i - 60), i + 80);
      issues.push({
        kind: 'body-text',
        path: `line ${firstLine + 1} char ${firstChar + 1}`,
        v1: ctx(l1, firstChar),
        v2: ctx(l2, firstChar),
        why: `text differs at line ${firstLine + 1} char ${firstChar + 1} of ${lines1.length} vs ${lines2.length} lines`,
      });
    }
  } else if (mode === 'bytes-hash') {
    const h1 = crypto.createHash('sha256').update(r1.rawText).digest('hex');
    const h2 = crypto.createHash('sha256').update(r2.rawText).digest('hex');
    if (h1 !== h2) issues.push({ kind: 'body-bytes', v1: h1.slice(0, 16), v2: h2.slice(0, 16), why: 'byte hash differs' });
  }
  return { outcome: issues.length ? 'DIFF' : 'match', issues };
}

// ------------------------------------------------------------ main
async function main() {
  const state = {
    v1: { vars: {}, cookies: {} },
    v2: { vars: {}, cookies: {} },
  };
  const startedAt = new Date().toISOString();
  const report = { startedAt, v1: V1, v2: V2, steps: [], diffs: [], latency: {} };
  const latSamples = {}; // group -> { v1: [], v2: [] }
  let pass = 0;
  let diffCount = 0;
  let unexpectedCount = 0;
  let v2onlyCount = 0;
  let knownCount = 0;

  for (const step of STEPS) {
    const results = await runStep(step, state);
    const r1 = results.v1;
    const r2 = results.v2;
    let outcome;
    let issues = [];
    let notes = [];
    if (step.compute === 'totp') {
      outcome = 'computed';
      pass++;
    } else {
      const cmp = compareStep(step, r1, r2);
      outcome = cmp.outcome;
      issues = cmp.issues;
      // expected-status check (both match each other but miss the expectation).
      // v2-only steps are validated inside compareStep (P12 fix); skip here
      // so a v2-only step is never double-reported.
      if (!step.v2only && r1.status === r2.status && step.expect !== undefined && r1.status !== step.expect) {
        outcome = 'unexpected';
        issues.push({ kind: 'expect', v1: r1.status, v2: r2.status, why: `both returned ${r1.status}, expected ${step.expect}` });
      }
      if (!step.v2only && step.expectBody !== undefined) {
        const hit = r1.rawText.includes(step.expectBody) && r2.rawText.includes(step.expectBody);
        if (!hit) {
          outcome = outcome === 'match' ? 'unexpected' : outcome;
          issues.push({ kind: 'expectBody', why: `body does not contain ${JSON.stringify(step.expectBody)}` });
        }
      }
      if (r1.captureFailed || r2.captureFailed) {
        notes.push(`capture failed: v1=${(r1.captureFailed ?? []).join(',')} v2=${(r2.captureFailed ?? []).join(',')}`);
        if (outcome === 'match') outcome = 'unexpected';
        issues.push({ kind: 'capture', v1: r1.captureFailed ?? [], v2: r2.captureFailed ?? [] });
      }
      if (outcome === 'match') pass++;
      else if (outcome === 'v2-only') v2onlyCount++;
      else if (outcome === 'unexpected') unexpectedCount++;
      else diffCount++;
    }
    // known-diff classification
    let knownId = null;
    if (outcome === 'DIFF' && step.knownDiff) {
      const kd = KNOWN_DIFFS[step.knownDiff];
      if (kd) { knownId = step.knownDiff; knownCount++; outcome = 'known-diff'; }
    }
    // latency
    if (r1.ms !== undefined) {
      const g = latSamples[step.group] ?? (latSamples[step.group] = { v1: [], v2: [] });
      g.v1.push(r1.ms);
      g.v2.push(r2.ms);
    }
    const rec = {
      id: step.id, group: step.group, method: step.method ?? step.compute, path: step.path ?? '',
      outcome, issues, notes, knownDiff: knownId,
      v1: r1.status !== undefined ? { status: r1.status, ms: Math.round(r1.ms * 10) / 10, attempts: r1.attempts } : { computed: true },
      v2: r2.status !== undefined ? { status: r2.status, ms: Math.round(r2.ms * 10) / 10, attempts: r2.attempts } : { computed: true },
    };
    report.steps.push(rec);
    if (outcome === 'DIFF' || outcome === 'unexpected') {
      report.diffs.push({ id: step.id, group: step.group, path: step.path, outcome, issues });
    }
    const flag = outcome === 'match' ? 'ok' : outcome === 'v2-only' ? 'v2-only' : outcome === 'known-diff' ? 'KNOWN' : outcome === 'computed' ? 'calc' : '!!!';
    console.log(`${flag.padEnd(9)} ${step.id.padEnd(28)} v1=${r1.status ?? '—'} v2=${r2.status ?? '—'} ${r1.ms !== undefined ? `${Math.round(r1.ms)}ms/${Math.round(r2.ms)}ms` : ''}${issues.length && outcome !== 'known-diff' ? `  ${JSON.stringify(issues[0]).slice(0, 240)}` : ''}`);
  }

  // latency aggregation
  function pct(sorted, p) {
    if (!sorted.length) return null;
    const idx = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
    return Math.round(sorted[idx] * 10) / 10;
  }
  const allV1 = [];
  const allV2 = [];
  for (const [group, samples] of Object.entries(latSamples)) {
    const s1 = [...samples.v1].sort((a, b) => a - b);
    const s2 = [...samples.v2].sort((a, b) => a - b);
    allV1.push(...samples.v1);
    allV2.push(...samples.v2);
    report.latency[group] = {
      n: samples.v1.length,
      v1: { p50: pct(s1, 50), p95: pct(s1, 95) },
      v2: { p50: pct(s2, 50), p95: pct(s2, 95) },
    };
  }
  const a1 = [...allV1].sort((x, y) => x - y);
  const a2 = [...allV2].sort((x, y) => x - y);
  report.latency.overall = {
    n: allV1.length,
    v1: { p50: pct(a1, 50), p95: pct(a1, 95) },
    v2: { p50: pct(a2, 50), p95: pct(a2, 95) },
  };

  report.finishedAt = new Date().toISOString();
  report.totals = {
    steps: STEPS.length,
    match: pass,
    diff: diffCount,
    unexpected: unexpectedCount,
    knownDiff: knownCount,
    v2only: v2onlyCount,
    unexplained: diffCount - knownCount + unexpectedCount,
  };
  fs.mkdirSync(path.dirname(OUT), { recursive: true });
  fs.writeFileSync(OUT, JSON.stringify(report, null, 2));
  fs.mkdirSync(path.dirname(LATENCY_OUT), { recursive: true });
  fs.writeFileSync(LATENCY_OUT, JSON.stringify(report.latency, null, 2));
  console.log('\n=== PARITY SUMMARY ===');
  console.log(`steps: ${STEPS.length}  match: ${pass}  DIFF: ${diffCount}  unexpected: ${unexpectedCount}  known-diff: ${knownCount}  v2-only: ${v2onlyCount}`);
  console.log(`overall latency (local workerd, ms): v1 p50=${report.latency.overall.v1.p50} p95=${report.latency.overall.v1.p95} | v2 p50=${report.latency.overall.v2.p50} p95=${report.latency.overall.v2.p95}`);
  console.log(`report: ${OUT}`);
  console.log(`latency: ${LATENCY_OUT}`);
  if (report.totals.unexplained > 0) {
    console.log('UNEXPLAINED DIFFS — see report.diffs');
    process.exitCode = 1;
  }
}

main().catch((err) => {
  console.error('harness failed:', err);
  process.exit(1);
});
