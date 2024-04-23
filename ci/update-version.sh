#!/bin/bash

set -euxfo pipefail

cd $(dirname "$0")/..

version=$(grep -m 1 -oE '## \[.*?\]' CHANGELOG.md | sed -e 's/[# \[]//g' -e 's/\]//g')

rg '^version = \"[\d\.]+"' --iglob 'rust/Cargo.toml' --iglob "pyproject.toml" -m 1 -r "version = \"${version}\"" -q
rg '^version: [\d\.]+' --iglob dart/broadcast_wsrpc/pubspec.yaml --iglob "pyproject.toml" -m 1 -r "version: ${version}" -q

./ci/check_version.sh