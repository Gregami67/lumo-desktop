#!/usr/bin/env bash
#
# Build the E2E testing stub into the bundled frontend.
#
# Bundles the driver from the clients repo (which imports the real desktop-tools.ts)
# and copies it next to the committed stub page. Both land in src-tauri/dist/, where
# Tauri embeds them at build time, and both are gitignored: the artifact is rebuilt
# every run so it can never drift from the source it was built from.
#
# Env:
#   CLIENTS_DIR  path to the clients checkout (default: ../clients next to this repo)

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
clients_dir=${CLIENTS_DIR:-$repo_root/../clients}
package_dir=$clients_dir/packages/lumo-api-client
harness_entry=$package_dir/e2e/desktop-bridge-harness.ts
desktop_tools=$package_dir/core/desktop-tools.ts
esbuild=$repo_root/node_modules/.bin/esbuild
out_dir=$repo_root/src-tauri/dist

fail() {
    echo "build-stub: $1" >&2
    exit 1
}

[ -d "$clients_dir" ] || fail "clients checkout not found at $clients_dir (set CLIENTS_DIR)"
[ -f "$harness_entry" ] || fail "harness entry not found at $harness_entry"
[ -f "$desktop_tools" ] || fail "desktop-tools.ts not found at $desktop_tools"
[ -x "$esbuild" ] || fail "esbuild not found at $esbuild (run npm install)"

clients_ref=$(git -C "$clients_dir" rev-parse --short HEAD 2>/dev/null || echo unknown)
if ! git -C "$clients_dir" diff --quiet HEAD 2>/dev/null; then
    clients_ref="$clients_ref-dirty"
fi
echo "build-stub: bundling against clients $clients_ref ($clients_dir)"

mkdir -p "$out_dir"

# The clients checkout can be sparse (CI) or read-only (local Docker), without
# node_modules. Resolve its React peer dependency from our build tooling instead.
"$esbuild" "$harness_entry" \
    --bundle \
    --alias:react="$repo_root/node_modules/react" \
    --format=iife \
    --target=es2020 \
    --outfile="$out_dir/e2e_testing_stub.js"

cp "$repo_root/e2e/e2e_testing_stub.html" "$out_dir/e2e_testing_stub.html"

echo "build-stub: wrote $out_dir/e2e_testing_stub.{html,js}"
