#!/bin/zsh
set -euo pipefail

function _noodle_uninstall_detect_root() {
  local script_path="${0:A}"
  local candidate="${script_path:h:h}"
  if [[ -d "${candidate}/bin" && -d "${candidate}/plugin" && -d "${candidate}/config" ]]; then
    print -r -- "${candidate:A}"
    return 0
  fi
  print -r -- "${HOME}/.noodle"
}

function _noodle_uninstall_confirm() {
  local install_root="$1"
  if [[ "${NOODLE_UNINSTALL_YES:-0}" == "1" ]]; then
    return 0
  fi
  if [[ ! -r /dev/tty || ! -w /dev/tty ]]; then
    print -u2 -- "noodle uninstall error: rerun with NOODLE_UNINSTALL_YES=1 when no terminal is available."
    return 1
  fi
  local reply=""
  print -n -- "Remove noodle from ${install_root}? [Y/n]: " >/dev/tty
  read -r reply </dev/tty
  case "${reply:l}" in
    ""|y|yes) return 0 ;;
  esac
  print -u2 -- "Cancelled."
  return 1
}

function _noodle_uninstall_cleanup_zshrc() {
  local install_root="$1"
  local zshrc_path="${ZDOTDIR:-${HOME}}/.zshrc"
  python3 - "${zshrc_path}" "${install_root}" <<'PY'
from pathlib import Path
import sys

zshrc_path = Path(sys.argv[1]).expanduser()
install_root = Path(sys.argv[2]).expanduser()
start_marker = "# >>> noodle shell integration >>>"
end_marker = "# <<< noodle shell integration <<<"
legacy_target = str(install_root / "plugin" / "noodle.plugin.zsh")

if not zshrc_path.exists():
    sys.exit(0)

lines = zshrc_path.read_text().splitlines()
filtered = []
inside_block = False
for line in lines:
    stripped = line.strip()
    if stripped == start_marker:
        inside_block = True
        continue
    if stripped == end_marker:
        inside_block = False
        continue
    if inside_block:
        continue
    if "noodle.plugin.zsh" in stripped and stripped.startswith("source "):
        if legacy_target in stripped or "$HOME/.noodle/plugin/noodle.plugin.zsh" in stripped:
            continue
    filtered.append(line)

while filtered and not filtered[-1].strip():
    filtered.pop()

if filtered:
    zshrc_path.write_text("\n".join(filtered) + "\n")
else:
    zshrc_path.write_text("")
PY
}

install_root="${NOODLE_INSTALL_ROOT:-$(_noodle_uninstall_detect_root)}"
install_root="${install_root:A}"
launch_agent_label="${NOODLE_LAUNCH_AGENT_LABEL:-com.noodle.daemon}"
launch_agent_dir="${HOME}/Library/LaunchAgents"
launch_agent_plist="${launch_agent_dir}/${launch_agent_label}.plist"
launchctl_domain="gui/$(id -u)"
launchctl_bin="${NOODLE_LAUNCHCTL_BIN:-launchctl}"

_noodle_uninstall_confirm "${install_root}" || exit $?

"${launchctl_bin}" bootout "${launchctl_domain}/${launch_agent_label}" >/dev/null 2>&1 || true
"${launchctl_bin}" bootout "${launchctl_domain}" "${launch_agent_plist}" >/dev/null 2>&1 || true
"${launchctl_bin}" remove "${launch_agent_label}" >/dev/null 2>&1 || true

_noodle_uninstall_cleanup_zshrc "${install_root}"
rm -f "${launch_agent_plist}"
rm -rf "${install_root}"

print "Removed noodle from ${install_root}."
print "Removed noodle shell integration from ${ZDOTDIR:-${HOME}}/.zshrc."
