// Regenerate the PBKDF2 test vector used by wordless-kernel's crypto
// tests (crates/wordless-kernel/src/crypto.rs) and the smoke-test seed
// (scripts/seed-admin.sql generation). The vector is derived from the
// v1 format with Node's WebCrypto-compatible pbkdf2Sync:
//   PBKDF2-HMAC-SHA256, 100,000 iterations, 16-byte salt, 32-byte hash.
// Usage:
//   node scripts/gen-pbkdf2-vector.mjs <password> [salt-hex]
// With no salt-hex, a fixed 0..15 salt is used so the output is stable.
// The output is the exact `pbkdf2$<iter>$<salt>$<hash>` stored string.
import { pbkdf2Sync, randomBytes } from "node:crypto";

const password = process.argv[2] ?? "correct-horse-battery-staple";
const salt = process.argv[3]
  ? Buffer.from(process.argv[3], "hex")
  : Buffer.from([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
const hash = pbkdf2Sync(password, salt, 100_000, 32, "sha256");
console.log(`password: ${password}`);
console.log(`salt_b64url: ${salt.toString("base64url")}`);
console.log(`hash_b64url: ${hash.toString("base64url")}`);
console.log(`stored: pbkdf2$100000$${salt.toString("base64url")}$${hash.toString("base64url")}`);
if (!process.argv[3]) {
  console.log(`random_salt_hex_example: ${randomBytes(16).toString("hex")}`);
}
