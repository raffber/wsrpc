#!/bin/bash

set -euxfo pipefail

cd $(dirname "$0")/..

changelog_version=$(grep -m 1 -oE '## \[.*?\]' CHANGELOG.md | sed -e 's/[# \[]//g' -e 's/\]//g')

sed -i "s/^version = \".*\"/version = \"$changelog_version\"/" rust/Cargo.toml
sed -i "s/^version = \".*\"/version = \"$changelog_version\"/" pyproject.toml
sed -i "s/^version: .*/version: $changelog_version/" dart/broadcast_wsrpc/pubspec.yaml

./ci/check_version.sh