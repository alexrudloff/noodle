#!/bin/zsh
set -euo pipefail

repo_root="${0:A:h:h}"
cd "${repo_root}"

cargo test --test e2e -- --nocapture --test-threads=1
