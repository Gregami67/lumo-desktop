#!/usr/bin/env bash
# Build a package from this checkout instead of the pinned upstream commit.
# Extra arguments are passed to makepkg, e.g. ./makepkg-local.sh -si

set -euo pipefail

cd "$(dirname "$0")"

export LUMO_PKGBUILD_LOCAL=1
exec makepkg "$@"
