#!/usr/bin/env bash
# fmonero uninstaller: reverse scripts/install.sh.
#
#   scripts/uninstall.sh --dry-run      # print the plan, change nothing
#   scripts/uninstall.sh --yes          # do it without asking
#   scripts/uninstall.sh --yes --menu   # also strip the fmonero lines from the Omarchy menu
#   scripts/uninstall.sh --yes --purge  # remove disposable data, retain stable locks
#
# It removes only what the installer and `fmonero setup` created. Your wallets, the
# blockchain, Monero's own binaries and any system monerod.service are never touched,
# and it never uses sudo.

set -euo pipefail
umask 077

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${FMONERO_PREFIX:-$HOME/.local/bin}"
BIN="fmonero"
PLUGIN_ID="oliver.monero"
PLUGIN_LINK="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins/$PLUGIN_ID"
MENU_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/extensions/omarchy-menu.jsonc"
DESKTOP_FILE="${XDG_DATA_HOME:-$HOME/.local/share}/applications/fmonero.desktop"
SYSTEMD_DIR="${FMONERO_SYSTEMD_USER_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user}"
NODE_UNIT="fmonero-monerod.service"
WALLET_UNIT="fmonero-wallet-rpc.service"
CONFIG_DIR="${FMONERO_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/fmonero}"
STATE_DIR="${FMONERO_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/fmonero}"
CACHE_DIR="${FMONERO_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/fmonero}"
BACKUP_DIR="$STATE_DIR/backups"
# Never removed, by any flag: this is where monero-wallet-rpc keeps the wallet files.
WALLET_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/fmonero"

ASSUME_YES=0
DRY_RUN=0
DO_MENU=0
DO_PURGE=0
QUIET=0
USE_COLOR=1

E_FAIL=1
E_USAGE=2
E_SAFETY=3

usage() {
  cat <<'EOF'
fmonero uninstaller

Usage: scripts/uninstall.sh [OPTIONS]

Options:
  --yes            Do not ask for confirmation
  --dry-run        Print every step and change nothing
  --menu           Also remove fmonero's entries from ~/.config/omarchy/extensions/omarchy-menu.jsonc
                   (verbatim backup first; restored if the result no longer parses)
  --purge          Remove config/cache and disposable state; retain payment/session locks
  --prefix DIR     Where the binary was installed (default: ~/.local/bin)
  --quiet          Only warnings and errors
  --no-color       Never emit colour
  -h, --help       This text

Removes, in order:
  1. the daemons          guarded fmonero app stop --yes, then disable owned units
  2. the unit files       ~/.config/systemd/user/fmonero-{monerod,wallet-rpc}.service
  3. the bar widget       omarchy plugin disable oliver.monero
  4. the plugin symlink   ~/.config/omarchy/plugins/oliver.monero  (only when it is a symlink)
  5. the binary           PREFIX/fmonero
  6. --menu, --purge      only when asked

Never removed: your wallets (~/.local/share/fmonero/wallets), the blockchain
(~/.bitmonero), /usr/bin/monero*, /etc/monerod.conf, any system monerod.service.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes | -y) ASSUME_YES=1 ;;
    --dry-run | -n) DRY_RUN=1 ;;
    --menu) DO_MENU=1 ;;
    --purge) DO_PURGE=1 ;;
    --prefix)
      [ $# -ge 2 ] || {
        echo "uninstall.sh: --prefix needs a directory" >&2
        exit "$E_USAGE"
      }
      PREFIX="$2"
      shift
      ;;
    --prefix=*) PREFIX="${1#--prefix=}" ;;
    --quiet | -q) QUIET=1 ;;
    --no-color) USE_COLOR=0 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "uninstall.sh: unknown argument '$1'" >&2
      echo "hint: scripts/uninstall.sh --help" >&2
      exit "$E_USAGE"
      ;;
  esac
  shift
done

if [ ! -t 1 ] || [ -n "${NO_COLOR:-}" ] || [ -n "${CI:-}" ]; then USE_COLOR=0; fi
if [ "$USE_COLOR" -eq 1 ]; then
  C_DIM=$'\033[2m' C_RED=$'\033[31m' C_GRN=$'\033[32m' C_YEL=$'\033[33m' C_BLU=$'\033[34m' C_OFF=$'\033[0m'
else
  C_DIM='' C_RED='' C_GRN='' C_YEL='' C_BLU='' C_OFF=''
fi

info() { [ "$QUIET" -eq 1 ] || printf '%s\n' "${C_BLU}->${C_OFF} $*"; }
ok() { [ "$QUIET" -eq 1 ] || printf '%s\n' "${C_GRN}ok${C_OFF}   $*"; }
skip() { [ "$QUIET" -eq 1 ] || printf '%s\n' "${C_DIM}--   $*${C_OFF}"; }
warn() { printf '%s\n' "${C_YEL}warn${C_OFF} $*" >&2; }
err() { printf '%s\n' "${C_RED}error${C_OFF} $*" >&2; }
plan() { [ "$QUIET" -eq 1 ] || printf '%s\n' "${C_DIM}would${C_OFF} $*"; }

run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    plan "$*"
    return 0
  fi
  info "$*"
  # Only this shell owns the teardown leases; desktop helpers must not inherit them.
  "$@" 9>&- 8>&-
}

DEST="$PREFIX/$BIN"

# ------------------------------------------------------- 0. the money record --

refuse_pending() {
  local record
  for record in pending.json native-pending.json; do
    # Presence is enough: never parse away a corrupt/unreadable record or follow a
    # dangling symlink. This applies to ordinary uninstall as well as purge.
    if [ -e "$STATE_DIR/$record" ] || [ -L "$STATE_DIR/$record" ]; then
      err "$record exists; inspect actual transfers before uninstalling. No automatic retry or clearing is safe."
      exit "$E_SAFETY"
    fi
  done
}

guard_file() {
  local path="$1"
  if [ -L "$path" ] || { [ -e "$path" ] && { [ ! -f "$path" ] || [ ! -O "$path" ] || [ ! -r "$path" ] || [ ! -w "$path" ]; }; }; then
    err "The teardown lock is not an owned regular file: $path"
    exit "$E_SAFETY"
  fi
}

take_money_lock() {
  guard_file "$STATE_DIR/send.lock"
  if [ "$DRY_RUN" -eq 1 ] && [ ! -e "$STATE_DIR/send.lock" ]; then return; fi
  exec 9>>"$STATE_DIR/send.lock"
  if ! flock -n 9; then
    err "A wallet/payment operation holds send.lock. Finish it before uninstalling."
    exit "$E_SAFETY"
  fi
  guard_file "$STATE_DIR/send.lock"
  if [ "$(stat -Lc '%d:%i' /proc/$$/fd/9)" != "$(stat -Lc '%d:%i' "$STATE_DIR/send.lock")" ]; then
    err "The payment lock changed during teardown. Nothing was removed."
    exit "$E_SAFETY"
  fi
}

# A test fixture must never cause a real systemctl invocation. These are the only
# systemd queries used by teardown; unknown manager state is a refusal, not down.
unit_value() {
  local unit="$1" field="$2"
  if [ -n "${FMONERO_SYSTEMCTL_FIXTURE:-}" ]; then
    [ -f "$FMONERO_SYSTEMCTL_FIXTURE" ] && [ -r "$FMONERO_SYSTEMCTL_FIXTURE" ] || return 1
    awk -v unit="$unit" -v field="$field" '
      $1 == unit { print (field == "ActiveState" ? $2 : $3); found=1; exit }
      END { if (!found) print (field == "ActiveState" ? "inactive" : "disabled") }
    ' "$FMONERO_SYSTEMCTL_FIXTURE"
  else
    timeout --kill-after=2s 5s systemctl --user show --property="$field" --value "$unit" 9>&- 8>&-
  fi
}

require_stopped() {
  local unit value
  for unit in "$NODE_UNIT" "$WALLET_UNIT"; do
    value="$(unit_value "$unit" ActiveState)" || {
      err "Cannot prove $unit is stopped. Keep the integration and try again when the user service manager is available."
      exit "$E_SAFETY"
    }
    case "$value" in
      inactive|failed) ;;
      *) err "$unit is not confirmed stopped; teardown was refused."; exit "$E_SAFETY" ;;
    esac
  done
}

command -v flock >/dev/null && command -v timeout >/dev/null || {
  err "Safe teardown requires flock and timeout."; exit "$E_FAIL";
}
if [ -L "$STATE_DIR" ] || { [ -e "$STATE_DIR" ] && { [ ! -d "$STATE_DIR" ] || [ ! -O "$STATE_DIR" ] || [ ! -r "$STATE_DIR" ] || [ ! -w "$STATE_DIR" ] || [ ! -x "$STATE_DIR" ]; }; }; then
  err "The state directory is not an accessible owned directory."; exit "$E_SAFETY"
fi
refuse_pending

if [ "$DRY_RUN" -eq 0 ] && [ "$ASSUME_YES" -eq 0 ]; then
  if [ -t 0 ] && [ -t 1 ]; then
    printf '%s\n' "This will stop and remove fmonero's two systemd user units, disable the $PLUGIN_ID widget, remove its symlink, and delete $DEST."
    if [ "$DO_MENU" -eq 1 ]; then printf '%s\n' "It will also strip fmonero's lines from $MENU_FILE (backed up first)."; fi
    if [ "$DO_PURGE" -eq 1 ]; then printf '%s\n' "It will also purge disposable data, retaining the payment/session state skeleton."; fi
    printf '%s\n' "Your wallets in $WALLET_HOME/wallets and the blockchain in ~/.bitmonero are not touched."
    printf '%s' "Proceed? [y/N] "
    read -r reply || reply=""
    case "$reply" in
      y | Y | yes | YES) : ;;
      *)
        err "cancelled"
        exit "$E_SAFETY"
        ;;
    esac
  else
    err "uninstall.sh needs --yes when stdin is not a terminal"
    echo "hint: scripts/uninstall.sh --yes   # or --dry-run to see the plan" >&2
    exit "$E_SAFETY"
  fi
fi

# --------------------------------------------------------------- 1. daemons --

# Creating only the stable guard files is necessary when an installation has no
# state yet. No service or integration mutation occurs until exclusion succeeds.
if [ "$DRY_RUN" -eq 0 ]; then mkdir -p -- "$STATE_DIR"; fi
take_money_lock
refuse_pending

# Preserve arbitrary configured wallet/chain locations even if they are nested
# beneath a normally disposable directory. Without the binary, retain all purge
# data: an idempotent integration uninstall cannot guess the old configuration.
check_purge_paths() {
  local wallet_path chain_path
  PURGE_VERIFIED=0
  if [ "$DO_PURGE" -eq 1 ] && [ -x "$DEST" ]; then
    command -v python3 >/dev/null || { err "Safe purge needs python3 to check configured data paths."; exit "$E_FAIL"; }
    wallet_path="$(timeout --kill-after=2s 10s "$DEST" settings get wallet.dir --json 9>&- 8>&-)" || exit "$E_SAFETY"
    chain_path="$(timeout --kill-after=2s 10s "$DEST" settings get node.data_dir --json 9>&- 8>&-)" || exit "$E_SAFETY"
    if ! python3 - "$wallet_path" "$chain_path" "$WALLET_HOME" "$HOME/.bitmonero" "$STATE_DIR" "$CONFIG_DIR" "$CACHE_DIR" <<'PY'
import json, pathlib, sys
try:
    protected = [pathlib.Path(json.loads(raw)["value"]).resolve() for raw in sys.argv[1:3]]
    protected += [pathlib.Path(raw).resolve() for raw in sys.argv[3:5]]
    state, config, cache = [pathlib.Path(raw).resolve() for raw in sys.argv[5:8]]
    for disposable in [state, config, cache]:
        if any(disposable == p or disposable in p.parents or p in disposable.parents for p in protected):
            raise ValueError("purge overlaps wallet or blockchain storage")
    if any(p == state or p in state.parents or state in p.parents for p in [config, cache]):
        raise ValueError("purge directories overlap the retained state skeleton")
except Exception:
    print("Purge refused: configured data paths could not be separated safely.", file=sys.stderr)
    sys.exit(3)
PY
    then exit "$E_SAFETY"; fi
    PURGE_VERIFIED=1
  elif [ "$DO_PURGE" -eq 1 ]; then
    warn "The binary is absent; retaining config/state/cache because configured wallet and chain paths cannot be verified."
  fi
}
check_purge_paths

# app stop owns the money lock itself. Release our preliminary probe before it;
# after it exits, reacquire and recheck to close the stop-to-cleanup race.
exec 9>&-
if [ -x "$DEST" ]; then
  if [ "$DRY_RUN" -eq 1 ]; then
    plan "$DEST app stop --yes --json"
  else
    if timeout --kill-after=2s 60s "$DEST" app stop --yes --json; then :; else
      stop_rc=$?
      err "Guarded app stop failed; leaving all integration/data in place."
      exit "$stop_rc"
    fi
  fi
fi
take_money_lock
refuse_pending
if [ "$DRY_RUN" -eq 0 ]; then
  require_stopped
  guard_file "$STATE_DIR/app-session.lock"
  exec 8>>"$STATE_DIR/app-session.lock"
  if ! flock -w 3 8; then
    err "A frontend is still open. Close it before uninstalling; no integration was removed."
    exit "$E_SAFETY"
  fi
  guard_file "$STATE_DIR/app-session.lock"
  if [ "$(stat -Lc '%d:%i' /proc/$$/fd/8)" != "$(stat -Lc '%d:%i' "$STATE_DIR/app-session.lock")" ]; then
    err "The application lock changed during teardown."; exit "$E_SAFETY"
  fi
  # A wallet may have completed a guarded operation in the released-lock window.
  # Re-read its configured data paths under final exclusion before any removal.
  check_purge_paths
fi

for unit in "$NODE_UNIT" "$WALLET_UNIT"; do
  enabled="$(unit_value "$unit" UnitFileState)" || { err "Cannot inspect $unit before removal."; exit "$E_SAFETY"; }
  if [ "$enabled" = enabled ]; then
    if [ -n "${FMONERO_SYSTEMCTL_FIXTURE:-}" ]; then
      skip "isolated fixture: no host unit disable for $unit"
    else
      run timeout --kill-after=2s 10s systemctl --user disable "$unit"
    fi
  fi
done

# ------------------------------------------------------------ 2. unit files --

UNITS_REMOVED=0
for unit in "$NODE_UNIT" "$WALLET_UNIT"; do
  if [ -f "$SYSTEMD_DIR/$unit" ]; then
    run rm -f -- "$SYSTEMD_DIR/$unit"
    UNITS_REMOVED=1
  else
    skip "no $SYSTEMD_DIR/$unit"
  fi
done
if [ "$UNITS_REMOVED" -eq 1 ] && [ -z "${FMONERO_SYSTEMCTL_FIXTURE:-}" ]; then
  run timeout --kill-after=2s 10s systemctl --user daemon-reload
fi

# ------------------------------------------------------------ 3. bar widget --

if command -v omarchy >/dev/null 2>&1; then
  plugin_state="$(timeout --kill-after=2s 10s omarchy plugin list 9>&- 8>&- 2>/dev/null | awk -v id="$PLUGIN_ID" '$1 == id { print $2 }' || true)"
  if [ "$plugin_state" = "enabled" ]; then
    run timeout --kill-after=2s 10s omarchy plugin disable "$PLUGIN_ID"
  elif [ -n "$plugin_state" ]; then
    skip "$PLUGIN_ID is already $plugin_state"
  else
    skip "$PLUGIN_ID is not registered with omarchy"
  fi
else
  skip "omarchy is not on PATH; cannot disable the widget"
fi

# -------------------------------------------------------- 4. plugin symlink --

if [ -L "$PLUGIN_LINK" ]; then
  target="$(readlink -f -- "$PLUGIN_LINK" || true)"
  run rm -f -- "$PLUGIN_LINK"
  if [ -n "$target" ]; then skip "that link pointed at $target (left alone)"; fi
elif [ -d "$PLUGIN_LINK" ]; then
  warn "$PLUGIN_LINK is a real directory, not the installer's symlink; leaving it alone"
  echo "hint: rm -rf '$PLUGIN_LINK'   # only if you are sure it is yours" >&2
else
  skip "no plugin link at $PLUGIN_LINK"
fi

# ---------------------------------------------------------------- 5. binary --

if [ -f "$DEST" ]; then
  run rm -f -- "$DEST"
else
  skip "no binary at $DEST"
fi

if [ -f "$DESKTOP_FILE" ]; then
  run rm -f -- "$DESKTOP_FILE"
else
  skip "no application launcher entry at $DESKTOP_FILE"
fi

# ------------------------------------------------------------ 6. menu lines --

# The menu file is the owner's: full of comments and their own entries. Remove only
# whole lines whose key is "monero…" plus a lone "// fmonero" marker, take a verbatim
# backup first, and put the backup back if the result stops parsing (a stripped entry
# can leave a trailing comma behind).
menu_strip() {
  local src="$1" out="$2"
  awk '
    /^[[:space:]]*\/\/[[:space:]]*fmonero[[:space:]]*$/ { next }
    /^[[:space:]]*"monero(\.[a-z-]+)?"[[:space:]]*:/ { next }
    { print }
  ' "$src" >"$out"
}

# Dropping the last entries of the object leaves the entry above them with a trailing
# comma, which JSONC does not allow either. Remove exactly one comma, on the last real
# line before the file's closing brace.
menu_fix_trailing_comma() {
  local src="$1" out="$2"
  awk '
    { line[NR] = $0 }
    END {
      close_at = 0
      for (i = NR; i >= 1; i--) if (line[i] ~ /^[[:space:]]*}[[:space:]]*$/) { close_at = i; break }
      if (close_at > 1) {
        for (i = close_at - 1; i >= 1; i--) {
          if (line[i] ~ /^[[:space:]]*$/) continue
          if (line[i] ~ /^[[:space:]]*\/\//) continue
          sub(/,[[:space:]]*$/, "", line[i])
          break
        }
      }
      for (i = 1; i <= NR; i++) print line[i]
    }
  ' "$src" >"$out"
}

menu_parses() {
  python3 - "$1" <<'PY'
import json, sys
src = open(sys.argv[1], encoding="utf-8").read()
# strip // line comments outside strings, then parse
out, i, n, in_str, esc = [], 0, len(src), False, False
while i < n:
    c = src[i]
    if in_str:
        out.append(c)
        if esc: esc = False
        elif c == "\\": esc = True
        elif c == '"': in_str = False
        i += 1
        continue
    if c == '"':
        in_str = True; out.append(c); i += 1; continue
    if c == "/" and i + 1 < n and src[i+1] == "/":
        while i < n and src[i] != "\n": i += 1
        continue
    out.append(c); i += 1
try:
    json.loads("".join(out))
except Exception as e:
    print(e, file=sys.stderr); sys.exit(1)
PY
}

if [ "$DO_MENU" -eq 1 ]; then
  if [ ! -f "$MENU_FILE" ]; then
    skip "no menu file at $MENU_FILE"
  elif ! grep -qE '^[[:space:]]*"monero(\.[a-z-]+)?"[[:space:]]*:' "$MENU_FILE"; then
    skip "no fmonero entries in $MENU_FILE"
  elif ! command -v python3 >/dev/null 2>&1; then
    warn "python3 is missing; not editing $MENU_FILE without being able to check the result"
  else
    scratch="$(mktemp)"
    menu_strip "$MENU_FILE" "$scratch"
    if ! menu_parses "$scratch" 2>/dev/null; then
      fixed="$(mktemp)"
      menu_fix_trailing_comma "$scratch" "$fixed"
      if menu_parses "$fixed" 2>/dev/null; then
        mv -f -- "$fixed" "$scratch"
      else
        rm -f -- "$fixed"
      fi
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
      plan "rewrite $MENU_FILE:"
      diff -u --label "$MENU_FILE" --label "$MENU_FILE (after)" -- "$MENU_FILE" "$scratch" || true
      rm -f -- "$scratch"
    else
      stamp="$(date -u +%Y%m%dT%H%M%SZ)"
      mkdir -p -- "$BACKUP_DIR"
      cp -p -- "$MENU_FILE" "$BACKUP_DIR/omarchy-menu.jsonc.$stamp"
      cat -- "$scratch" >"$MENU_FILE"
      rm -f -- "$scratch"
      if menu_parses "$MENU_FILE"; then
        ok "removed fmonero's entries from $MENU_FILE (backup: $BACKUP_DIR/omarchy-menu.jsonc.$stamp)"
      else
        cp -p -- "$BACKUP_DIR/omarchy-menu.jsonc.$stamp" "$MENU_FILE"
        err "removing those lines left invalid JSON; restored $MENU_FILE from the backup"
        echo "hint: edit $MENU_FILE by hand and delete the \"monero…\" lines" >&2
        exit "$E_FAIL"
      fi
    fi
  fi
else
  if [ -f "$MENU_FILE" ] && grep -qE '^[[:space:]]*"monero(\.[a-z-]+)?"[[:space:]]*:' "$MENU_FILE"; then
    skip "menu entries kept in $MENU_FILE (pass --menu to remove them)"
  fi
fi

# -------------------------------------------------------------- 7. own data --

# $WALLET_HOME is deliberately absent from this list, and from every list in this
# script: ~/.local/share/fmonero/wallets is where monero-wallet-rpc keeps the wallet
# files, and a wallet file plus its password is the only copy of money that is not the
# seed. ~/.bitmonero (the chain) is likewise never touched; it is re-syncable but it is
# also 100+ GB, and it is not ours.
if [ "$DO_PURGE" -eq 1 ] && [ "$PURGE_VERIFIED" -eq 1 ]; then
  refuse_pending
  for d in "$CONFIG_DIR" "$CACHE_DIR"; do
    if [ -d "$d" ]; then
      run rm -rf -- "$d"
    else
      skip "no $d"
    fi
  done
  # Never unlink the lock inodes while this or an old frontend can still hold
  # them, nor reset the generation that tells old frontends to stop.
  shopt -s nullglob dotglob
  for entry in "$STATE_DIR"/*; do
    case "${entry##*/}" in
      send.lock|app-session.lock|app-stop-generation.json|.lock) continue ;;
    esac
    run rm -rf -- "$entry"
  done
  shopt -u nullglob dotglob
  skip "retained payment/session lock and generation state in $STATE_DIR"
else
  skip "config, state and cache kept in $CONFIG_DIR, $STATE_DIR, $CACHE_DIR (pass --purge to delete them)"
fi

if [ "$QUIET" -eq 0 ]; then
  echo
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '%s\n' "${C_DIM}dry run: nothing above was executed.${C_OFF}"
  else
    printf '%s\n' "${C_GRN}fmonero removed${C_OFF}"
    printf '%s\n' "  kept      your wallets in $WALLET_HOME/wallets, the chain in ~/.bitmonero,"
    printf '%s\n' "            /usr/bin/monero*, /etc/monerod.conf, any system monerod.service"
    printf '%s\n' "  reinstall $REPO/scripts/install.sh"
  fi
fi
