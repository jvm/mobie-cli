#!/usr/bin/env bash

set -euo pipefail

sed -n 's/^version = "\(.*\)"$/\1/p' apps/mobie/Cargo.toml | head -n 1
