#!/bin/bash

set -euxfo pipefail

cd $(dirname "$0")/..

rust_version=$(grep -E -m 1 "^version" rust/Cargo.toml | cut -d "=" -f 2 | sed 's/[" ]//g')
changelog_version=$(grep -m 1 -oE '## \[.*?\]' CHANGELOG.md | sed -e 's/[# \[]//g' -e 's/\]//g')
python_version=$(grep -E -m 1 "version" pyproject.toml | cut -d "=" -f 2 | sed 's/[" ,]//g')
dart_version=$(grep -E -m 1 "version" dart/broadcast_wsrpc/pubspec.yaml | cut -d ":" -f 2 | sed 's/[ ]//g')

if [[ $rust_version != $changelog_version ]]; then
    echo "Invalid version in changelog"
    exit 1
fi

if [[ $rust_version != $python_version ]]; then
    echo "Invalid version in python package"
    exit 1
fi

if [[ $rust_version != $dart_version ]]; then
    echo "Invalid version in dart package"
    exit 1
fi
