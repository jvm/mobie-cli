#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <target-triple> <version>" >&2
  exit 1
fi

target="$1"
version="$2"
binary="target/${target}/release/mobie"
archive="mobie-v${version}-${target}.tar.gz"

cargo build --locked --release -p mobie --target "${target}"

staging_dir="$(mktemp -d)"
trap 'rm -rf "${staging_dir}"' EXIT

cp "${binary}" "${staging_dir}/mobie"
tar -C "${staging_dir}" -czf "${archive}" mobie
