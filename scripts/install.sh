#!/usr/bin/env bash
# fmonero installer: build from this checkout, install the binary, wire up Omarchy.
#
#   scripts/install.sh --dry-run        # print the plan, change nothing
#   scripts/install.sh --yes            # do it without asking
#
# Every step is idempotent: running it twice leaves the same machine. It never uses
# sudo, never writes outside $HOME, never reads or writes a wallet file, and never
# starts a daemon — `fmonero setup` writes the two systemd user units disabled, and
# `fmonero node up` is the only thing that starts monerod (DESIGN.md §2i, §4.2).

set -euo pipefail
# 077, not 022: everything fmonero writes under $HOME is owner-only (DESIGN.md §2b.7).
# The binary itself is installed with an explicit `install -m 0755`, and PREFIX is
# created 0755 when it is missing, so this mask only tightens files nothing else reads.
umask 077

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${FMONERO_PREFIX:-$HOME/.local/bin}"
BIN="fmonero"

ASSUME_YES=0
DRY_RUN=0
DO_SETUP=1
QUIET=0
USE_COLOR=1
LOCK_DIR=""
TMP_BIN=""

# Exit codes mirror fmonero's own (DESIGN.md §4a): 0 ok, 1 failure, 2 usage,
# 3 safety (refused without --yes), 4 backend (a required tool is missing).
E_FAIL=1
E_USAGE=2
E_SAFETY=3
E_BACKEND=4

usage() {
  cat <<'EOF'
fmonero installer

Usage: scripts/install.sh [OPTIONS]

Options:
  --yes            Do not ask for confirmation
  --dry-run        Print every step and change nothing (no build, no lock, no write)
  --no-setup       Install the binary only; skip `fmonero setup --all`
  --prefix DIR     Where to install the binary (default: ~/.local/bin)
  --quiet          Only warnings and errors
  --no-color       Never emit colour (NO_COLOR and a non-TTY stdout are honoured too)
  -h, --help       This text

Steps:
  1. check for cargo, a writable prefix, and monerod
  2. cargo build --release -p fmonero      (the mock server is never built or installed)
  3. install the binary to PREFIX/fmonero  (atomic rename; safe while fmonero runs)
  4. fmonero setup --all --yes             (units, plugin symlink + enable + bar placement,
                                            menu entries) — the units are written DISABLED

Reverse it with scripts/uninstall.sh.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes | -y) ASSUME_YES=1 ;;
    --dry-run | -n) DRY_RUN=1 ;;
    --no-setup) DO_SETUP=0 ;;
    --prefix)
      [ $# -ge 2 ] || {
        echo "install.sh: --prefix needs a directory" >&2
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
      echo "install.sh: unknown argument '$1'" >&2
      echo "hint: scripts/install.sh --help" >&2
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

cleanup() {
  if [ -n "$TMP_BIN" ] && [ -e "$TMP_BIN" ]; then rm -f -- "$TMP_BIN"; fi
  if [ -n "$LOCK_DIR" ] && [ -d "$LOCK_DIR" ]; then rmdir -- "$LOCK_DIR" 2>/dev/null || true; fi
  return 0
}
trap cleanup EXIT

# ---------------------------------------------------------------- preflight --

[ -f "$REPO/Cargo.toml" ] && [ -d "$REPO/crates/fmonero" ] || {
  err "$REPO does not look like the fmonero checkout (no Cargo.toml + crates/fmonero)"
  exit "$E_FAIL"
}

if ! command -v cargo >/dev/null 2>&1; then
  err "cargo is not on PATH; fmonero is built from source"
  echo "hint: omarchy pkg add rustup && rustup default stable" >&2
  exit "$E_BACKEND"
fi

# monerod is not required to install fmonero, and its absence is not an error here:
# `fmonero doctor` reports it, and every command that needs a daemon exits 4 with the
# same hint. Saying so once now beats discovering it from the first command.
if ! command -v monerod >/dev/null 2>&1; then
  warn "monerod is not on PATH; fmonero installs, but every command that needs the node or the wallet will exit 4"
  echo "hint: omarchy pkg add monero   # then: fmonero setup --units --yes" >&2
fi

if [ "$DO_SETUP" -eq 1 ] && ! command -v omarchy >/dev/null 2>&1; then
  warn "omarchy is not on PATH; the desktop integration step will report what it cannot do"
fi

DEST="$PREFIX/$BIN"
if [ -e "$DEST" ] && [ ! -f "$DEST" ]; then
  err "$DEST exists and is not a regular file; refusing to replace it"
  exit "$E_FAIL"
fi
if [ "$DRY_RUN" -eq 0 ]; then
  if [ ! -d "$PREFIX" ]; then
    # A bin directory other tools may also resolve through; 0755, not the 0700 the
    # script's own umask would give it.
    (umask 022 && mkdir -p -- "$PREFIX") || {
      err "cannot create $PREFIX"
      exit "$E_FAIL"
    }
  fi
  [ -w "$PREFIX" ] || {
    err "$PREFIX is not writable"
    exit "$E_FAIL"
  }
fi

case ":$PATH:" in
  *":$PREFIX:"*) : ;;
  *) warn "$PREFIX is not on your PATH. The Omarchy shell's PATH includes ~/.local/bin but not ~/.cargo/bin, so install there." ;;
esac

OLD_VERSION=""
if [ -x "$DEST" ]; then OLD_VERSION="$("$DEST" --version 2>/dev/null || true)"; fi

# ------------------------------------------------------------------- consent --

if [ "$DRY_RUN" -eq 0 ] && [ "$ASSUME_YES" -eq 0 ]; then
  if [ -t 0 ] && [ -t 1 ]; then
    printf '%s\n' "This will build fmonero and install it to $DEST${OLD_VERSION:+ (replacing $OLD_VERSION)}."
    if [ "$DO_SETUP" -eq 1 ]; then
      printf '%s\n' "Then: $BIN setup --all --yes  (systemd user units, written disabled; plugin link; bar widget; menu entries)"
    fi
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
    err "install.sh needs --yes when stdin is not a terminal"
    echo "hint: scripts/install.sh --yes   # or --dry-run to see the plan" >&2
    exit "$E_SAFETY"
  fi
fi

# ---------------------------------------------------------------------- lock --

if [ "$DRY_RUN" -eq 0 ]; then
  LOCK_DIR="${XDG_RUNTIME_DIR:-/tmp}/fmonero-install.lock"
  if ! mkdir -- "$LOCK_DIR" 2>/dev/null; then
    err "another install.sh holds $LOCK_DIR"
    echo "hint: rmdir '$LOCK_DIR'   # if no installer is running" >&2
    LOCK_DIR=""
    exit "$E_FAIL"
  fi
fi

# --------------------------------------------------------------------- build --

# `-p fmonero` and nothing else: crates/fmonero-mock is a test-only workspace member
# (DESIGN.md §8 Phase 0) and must never reach ~/.local/bin.
if [ "$DRY_RUN" -eq 1 ]; then
  plan "cargo build --release -p fmonero   (in $REPO)"
else
  info "cargo build --release -p fmonero"
  (cd -- "$REPO" && cargo build --release -p fmonero) || {
    err "the build failed; nothing was installed"
    exit "$E_FAIL"
  }
fi

BUILT="$REPO/target/release/$BIN"
if [ "$DRY_RUN" -eq 0 ] && [ ! -x "$BUILT" ]; then
  err "$BUILT is missing after a successful build"
  exit "$E_FAIL"
fi

# ------------------------------------------------------------------- install --

if [ "$DRY_RUN" -eq 1 ]; then
  plan "install -m 0755 $BUILT $DEST"
elif [ -f "$DEST" ] && cmp -s -- "$BUILT" "$DEST"; then
  skip "$DEST is already this exact binary"
else
  # Write beside the target and rename: replacing a running binary in place gives
  # ETXTBSY, and a half-copied fmonero on PATH is worse than none.
  TMP_BIN="$PREFIX/.$BIN.install.$$"
  install -m 0755 -- "$BUILT" "$TMP_BIN"
  mv -f -- "$TMP_BIN" "$DEST"
  TMP_BIN=""
  ok "installed $DEST"
fi

NEW_VERSION=""
if [ "$DRY_RUN" -eq 0 ]; then
  NEW_VERSION="$("$DEST" --version 2>/dev/null || true)"
  [ -n "$NEW_VERSION" ] || {
    err "$DEST does not run"
    exit "$E_FAIL"
  }
fi

# --------------------------------------------------------------------- setup --

SETUP_RC=0
if [ "$DO_SETUP" -eq 0 ]; then
  skip "desktop integration (--no-setup)"
elif [ "$DRY_RUN" -eq 1 ]; then
  # Ask an fmonero that already exists (this checkout's build, else the installed
  # one) for the real plan; fall back to naming the command when there is none.
  # `setup --all --dry-run` writes nothing by contract (DESIGN.md §4.2) and its
  # diff redacts the node's rpc-login value.
  PROBE=""
  if [ -x "$BUILT" ]; then
    PROBE="$BUILT"
  elif [ -x "$DEST" ]; then
    PROBE="$DEST"
  fi
  if [ -n "$PROBE" ]; then
    info "$PROBE setup --all --dry-run"
    "$PROBE" setup --all --dry-run || SETUP_RC=$?
    if [ "$SETUP_RC" -ne 0 ]; then
      warn "that fmonero exited $SETUP_RC for the setup plan (see above); the real run may differ"
      SETUP_RC=0
    fi
  else
    plan "$BIN setup --all --yes   (units written disabled, plugin link, enable, bar placement, menu entries)"
  fi
else
  info "$BIN setup --all --yes"
  "$DEST" setup --all --yes || SETUP_RC=$?
fi

# ------------------------------------------------------------------- summary --

if [ "$QUIET" -eq 0 ]; then
  echo
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '%s\n' "${C_DIM}dry run: nothing above was executed.${C_OFF}"
  else
    printf '%s\n' "${C_GRN}fmonero installed${C_OFF}  ${NEW_VERSION:-}"
    printf '%s\n' "  binary   $DEST"
    printf '%s\n' "  next     fmonero doctor            # what is healthy on this machine"
    printf '%s\n' "           fmonero node up --wait    # start monerod and wait for it to catch up"
    printf '%s\n' "           fmonero tui               # Node · Wallet · Send · Log"
    printf '%s\n' "  kept     your wallets, ~/.bitmonero, Monero's own binaries, any system monerod.service"
    printf '%s\n' "  undo     $REPO/scripts/uninstall.sh"
  fi
fi

if [ "$SETUP_RC" -ne 0 ]; then
  err "fmonero setup --all exited $SETUP_RC; the binary is installed, the desktop integration is not complete"
  echo "hint: fmonero setup --all --dry-run   # what it wanted to do" >&2
  exit "$SETUP_RC"
fi
