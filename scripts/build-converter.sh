#!/usr/bin/env bash
# WORDLESS v2 — build the converter Worker (WASM + JS entry) via worker-build.
#
# Mirror of scripts/build-worker.sh for crates/wordless-converter.
# MUST run from the crate directory: worker-build 0.9.0 resolves its
# staging/output paths relative to the crate root.
set -euo pipefail
# Resolve the script's own directory BEFORE the cd below (dirname is
# relative to the caller's cwd; after cd it no longer resolves).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/../crates/wordless-converter"

# Toolchain: worker 0.9.0 + worker-build 0.9.0 (see Cargo.toml note).
# VM-only workarounds (2026-11-19): this VM cannot download from GitHub
# releases or the npm registry (TLS "UnknownIssuer"), so worker-build's
# pinned wasm-bindgen CLI and esbuild are supplied from local installs
# when present. On a normal machine these overrides are unset and
# worker-build downloads its own tools.
if [ -z "${WASM_BINDGEN_BIN:-}" ] && [ -x "$HOME/.cargo-wb01/bin/wasm-bindgen" ]; then
  export WASM_BINDGEN_BIN="$HOME/.cargo-wb01/bin/wasm-bindgen"
fi
if [ -z "${ESBUILD_BIN:-}" ]; then
  if command -v esbuild >/dev/null 2>&1; then
    export ESBUILD_BIN="esbuild"
  elif [ -x "$HOME/workspace/wordless/wordless-cms/node_modules/.bin/esbuild" ]; then
    # The v1 repo's Vite dependency ships esbuild 0.28.2 — the version
    # worker-build 0.9.0 expects. Read-only reuse; nothing is installed.
    export ESBUILD_BIN="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/esbuild"
  fi
fi

# --no-opt: worker-build's own wasm-opt download is skipped; we run
# wasm-opt ourselves below (binaryen is a project devDependency, so it is
# present after `npm install` on any machine). Same feature flags as
# the main Worker: wasm-bindgen 0.2.130 emits exception-handling /
# bulk-memory features a bare wasm-opt rejects.
# The pass adds ~90 s to the build; skip it only with WORDLESS_NO_WASM_OPT=1.
worker-build --release --no-opt

# wasm-opt -Oz (size reduction, free-tier gate). Resolves in order:
#   $WASM_OPT_BIN, <workspace>/node_modules/.bin/wasm-opt, system wasm-opt.
WASM_OPT_BIN="${WASM_OPT_BIN:-}"
if [ -z "$WASM_OPT_BIN" ]; then
  for candidate in \
    "$SCRIPT_DIR/../node_modules/.bin/wasm-opt" \
    "$(command -v wasm-opt || true)"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      WASM_OPT_BIN="$candidate"
      break
    fi
  done
fi
WASM_OUT="build/index_bg.wasm"
if [ -n "${WORDLESS_NO_WASM_OPT:-}" ]; then
  echo "build-converter: WORDLESS_NO_WASM_OPT=1 — skipping wasm-opt (build will be over the free-tier limit)"
elif [ -z "$WASM_OPT_BIN" ]; then
  echo "build-converter: WARNING — wasm-opt not found; WASM is unoptimized and over the 3 MiB free-tier limit." >&2
  echo "build-converter: run 'npm install' (binaryen is a devDependency) or set WASM_OPT_BIN." >&2
else
  BEFORE_BYTES="$(stat -c %s "$WASM_OUT")"
  "$WASM_OPT_BIN" \
    --enable-exception-handling --enable-bulk-memory \
    --enable-nontrapping-float-to-int --enable-sign-ext --enable-multivalue \
    --enable-reference-types --enable-simd --enable-tail-call --enable-gc \
    --enable-extended-const \
    --enable-strings --enable-multimemory --enable-mutable-globals \
    --enable-bulk-memory-opt \
    -Oz "$WASM_OUT" -o "$WASM_OUT.opt"
  mv "$WASM_OUT.opt" "$WASM_OUT"
  AFTER_BYTES="$(stat -c %s "$WASM_OUT")"
  echo "build-converter: wasm-opt -Oz: $BEFORE_BYTES -> $AFTER_BYTES bytes (limit 3145728)"
  if [ "$AFTER_BYTES" -gt 3145728 ]; then
    echo "build-converter: ERROR — optimized WASM ($AFTER_BYTES bytes) still exceeds the free-tier 3 MiB limit (3145728)." >&2
    exit 1
  fi
fi
