#!/bin/zsh
set -euo pipefail

repo_root="${0:A:h:h}"
cd "${repo_root}"

echo "==> Checking tool registry exposure"
cargo test --test e2e daemon_exposes_tool_registry_and_builtin_tool_calls -- --nocapture --test-threads=1

echo
echo "==> Running every builtin noodle tool"
cargo test --test e2e all_builtin_primitives_are_covered_and_work -- --nocapture --test-threads=1

echo
echo "==> Running model-driven chat tool harness"
cargo test --test e2e chat_tool_harness_prints_model_selected_tools_and_results -- --nocapture --test-threads=1
