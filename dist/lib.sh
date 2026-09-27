# Shared by install.sh, uninstall.sh and dev-plugin.sh. Sourced, not run.
#
# The one rule: Opal replaces or removes a path only if it is a regular file
# (never a symlink, never a folder) whose SHA-256 matches either the line for
# that path in Opal's install record, or a file some Opal version wrote
# (dist/known-hashes.tsv). Every replacement swaps the file out first and
# checks the bytes that came out, so an edit made after the check is kept,
# not lost. Everything else is yours: it stays, and the output says so.
#
# Record: ${XDG_STATE_HOME:-~/.local/state}/opal/installed.tsv, one
# "<sha256><TAB><absolute path>" per file Opal wrote, plus
# "backup<TAB><original><TAB><backup>" for files moved aside with your
# consent. Backups under state/opal/backup/ are never deleted by Opal.
#
# What stays imprecise, on purpose (documented in the README):
# parent folders are resolved the way the OS does (a symlinked ~/.local/bin
# is followed; only the final path component is never a link); a file you
# deliberately set back to an older Opal version's exact bytes counts as
# Opal's; when `mv --exchange` isn't available the swap degrades to two
# renames with a microsecond gap; hard links elsewhere keep the old bytes.

[[ -n ${BASH_VERSION:-} ]] || { echo "dist/lib.sh needs bash" >&2; return 1 2>/dev/null || exit 1; }

OPAL_URL="https://github.com/derekross/opal"
MARKER=".installed-by-opal"

CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CACHE_HOME="${XDG_CACHE_HOME:-$HOME/.cache}"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"

BINDIR="$HOME/.local/bin"
UNITDIR="$CONFIG_HOME/systemd/user"            # systemd honours XDG_CONFIG_HOME
UNIT="$UNITDIR/opal.service"
APPDIR="$DATA_HOME/applications"               # so does the desktop spec
LAUNCHER="$APPDIR/opal-nostrconnect.desktop"
MIMEAPPS="$CONFIG_HOME/mimeapps.list"
PLUGINDIR="$HOME/.config/omarchy/plugins"      # where Omarchy itself looks
STATEDIR="$STATE_HOME/opal"
MANIFEST="$STATEDIR/installed.tsv"
BACKUPDIR="$STATEDIR/backup"
DATADIR="$DATA_HOME/opal"                      # as crates/opal-core/src/paths.rs
CONFIGDIR="$CONFIG_HOME/opal"
CACHEDIR="$CACHE_HOME/opal"

PLUGIN_ID="$(jq -r .id manifest.json 2>/dev/null || true)"
[[ $PLUGIN_ID =~ ^[a-z0-9][a-z0-9.-]{0,63}$ ]] || { echo "manifest.json has no usable plugin id" >&2; exit 1; }
PLUGIN_PATH="$PLUGINDIR/$PLUGIN_ID"
OLD_PLUGIN_PATH="$PLUGINDIR/opal"              # the id before v0.2.0

say() { printf '%s\n' "$*"; }
note() { printf '  %s\n' "$*"; }
die() { printf '%s\n' "$*" >&2; exit 1; }

# ── Files ──────────────────────────────────────────────────────────────
path_kind() {
  if [[ -L $1 ]]; then echo symlink
  elif [[ ! -e $1 ]]; then echo missing
  elif [[ -f $1 ]]; then echo file
  elif [[ -d $1 ]]; then echo dir
  else echo other
  fi
}
# SHA-256 of a regular file, or nothing. Never fails the caller.
file_hash() {
  local out
  out="$(sha256sum -- "$1" 2>/dev/null)" || out=
  printf '%s\n' "${out%% *}"
}
describe_file() {
  local size when
  size="$(stat -c %s -- "$1" 2>/dev/null || echo '?')"
  when="$(date -r "$1" '+%Y-%m-%d %H:%M' 2>/dev/null || echo '?')"
  printf '%s bytes, modified %s\n' "$size" "$when"
}
# One-line, shell-safe spelling of a name for messages.
q() { printf '%q' "$1"; }

# Temp files this run made; removed on exit. Never anything of yours.
TEMPS=()
cleanup_temps() { local t; for t in "${TEMPS[@]}"; do [[ -e $t || -L $t ]] && rm -rf -- "$t"; done; return 0; }
trap cleanup_temps EXIT

MV_EXCHANGE=0
mv --help 2>/dev/null | grep -q -- '--exchange' && MV_EXCHANGE=1

# ── The install record ─────────────────────────────────────────────────
declare -A MANIFEST_HASH=()   # absolute path -> sha256
BACKUP_LINES=()               # "original<TAB>backup"
load_manifest() {
  MANIFEST_HASH=(); BACKUP_LINES=()
  [[ -f $MANIFEST && ! -L $MANIFEST ]] || return 0
  local h p extra
  while IFS=$'\t' read -r h p extra || [[ -n $h ]]; do
    if [[ $h == backup ]]; then
      [[ $p == /* && $extra == /* ]] && BACKUP_LINES+=("$p"$'\t'"$extra")
    elif [[ $h =~ ^[0-9a-f]{64}$ && $p == /* && -z $extra ]]; then
      MANIFEST_HASH["$p"]=$h
    else
      note "ignoring an odd line in $MANIFEST: ${h:0:20}…"
    fi
  done <"$MANIFEST"
}
write_manifest() {
  local tmp p b
  if (( ${#MANIFEST_HASH[@]} == 0 && ${#BACKUP_LINES[@]} == 0 )); then
    rm -f -- "$MANIFEST"
    return 0
  fi
  mkdir -p -m 700 -- "$STATEDIR"
  tmp="$(mktemp -- "$STATEDIR/.installed.XXXXXX")"
  {
    for p in "${!MANIFEST_HASH[@]}"; do printf '%s\t%s\n' "${MANIFEST_HASH[$p]}" "$p"; done | LC_ALL=C sort -t $'\t' -k2
    for b in "${BACKUP_LINES[@]}"; do printf 'backup\t%s\n' "$b"; done
  } >"$tmp"
  chmod 600 -- "$tmp"
  mv -T -- "$tmp" "$MANIFEST"
}
record() { MANIFEST_HASH["$1"]="$(file_hash "$1")"; write_manifest; }
unrecord() { [[ -n ${MANIFEST_HASH[$1]:-} ]] || return 0; unset 'MANIFEST_HASH[$1]'; write_manifest; }

# The state folder must be a real, writable folder before anything is written.
# Also takes the lock, so two installs (two checkouts, say) can't interleave.
prepare_state() {
  if [[ -L $STATEDIR && ! -d $STATEDIR ]]; then die "$STATEDIR is a dangling link; fix it first."; fi
  [[ -e $STATEDIR && ! -d $STATEDIR ]] && die "$STATEDIR exists and isn't a folder; move it aside first."
  mkdir -p -m 700 -- "$STATEDIR" || die "can't create $STATEDIR"
  [[ -w $STATEDIR ]] || die "$STATEDIR isn't writable."
  exec 9>>"$STATEDIR/.lock"
  flock -n 9 || die "another Opal install or uninstall is running (lock: $STATEDIR/.lock)."
}

# ── What Opal versions wrote ───────────────────────────────────────────
declare -A KNOWN=()   # "sha256<TAB>logical path" -> label
render_plugin_manifest() { jq '.entryPoints |= with_entries(.value |= ltrimstr("shell-plugin/"))' manifest.json; }
render_launcher() {
  local line
  while IFS= read -r line || [[ -n $line ]]; do printf '%s\n' "${line//@BINDIR@/$BINDIR}"; done <dist/opal-nostrconnect.desktop
}
# What every release before the X-Opal-Source line wrote.
render_launcher_legacy() { render_launcher | awk '!/^X-Opal-Source=/'; }
hash_stdin() { sha256sum | cut -d' ' -f1; }
load_known() {
  local h l logical f
  KNOWN=()
  [[ -f dist/known-hashes.tsv ]] || die "dist/known-hashes.tsv is missing; run this from an Opal checkout."
  while IFS=$'\t' read -r h l logical || [[ -n $h ]]; do
    [[ $h == \#* || -z $logical ]] && continue
    KNOWN["$h"$'\t'"$logical"]=$l
  done <dist/known-hashes.tsv
  # This checkout counts too (an install made from it without a record).
  KNOWN["$(file_hash dist/opal.service)"$'\t'unit]=checkout
  while IFS= read -r -d '' f; do
    [[ ${f#shell-plugin/} == manifest.json ]] && continue
    KNOWN["$(file_hash "$f")"$'\t'"plugin/${f#shell-plugin/}"]=checkout
  done < <(find shell-plugin -type f -print0)
  KNOWN["$(render_plugin_manifest | hash_stdin)"$'\t'plugin/manifest.json]=checkout
  KNOWN["$(render_launcher | hash_stdin)"$'\t'launcher]=checkout
  KNOWN["$(render_launcher_legacy | hash_stdin)"$'\t'launcher]=legacy
}

# The marker earlier installs left in the plugin copy: exactly the v0.3.x
# content (known by hash), or the URL followed by "<sha256>  <file>" lines
# (an unreleased build), whose lines then count as a record for that folder.
# Anything else is your file.
declare -A MARKER_HASH=()
MARKER_STATE=none   # none | known | hashes | foreign
load_marker() {
  local dir=$1 m="$1/$MARKER" line first=1
  MARKER_HASH=(); MARKER_STATE=none
  [[ -e $m || -L $m ]] || return 0
  [[ -f $m && ! -L $m ]] || { MARKER_STATE=foreign; return 0; }
  if [[ -n ${KNOWN["$(file_hash "$m")"$'\t'"plugin/$MARKER"]:-} ]]; then MARKER_STATE=known; return 0; fi
  while IFS= read -r line || [[ -n $line ]]; do
    if (( first )); then
      first=0
      [[ $line == "$OPAL_URL" ]] || { MARKER_STATE=foreign; return 0; }
      continue
    fi
    if [[ $line =~ ^([0-9a-f]{64})\ \ ([^[:space:]/].*)$ ]]; then
      MARKER_HASH["$dir/${BASH_REMATCH[2]}"]=${BASH_REMATCH[1]}
    else
      MARKER_STATE=foreign; MARKER_HASH=(); return 0
    fi
  done <"$m"
  MARKER_STATE=hashes
}

# The only ownership test: owned_file <absolute path> <logical path>.
owned_file() {
  local p=$1 logical=$2 h
  [[ -f $p && ! -L $p ]] || return 1
  h="$(file_hash "$p")"
  [[ -n $h ]] || return 1
  [[ ${MANIFEST_HASH[$p]:-} == "$h" ]] && return 0
  [[ -n ${KNOWN["$h"$'\t'"$logical"]:-} ]] && return 0
  [[ ${MARKER_HASH[$p]:-} == "$h" ]] && return 0
  return 1
}

# ── Backups, replacing, removing ───────────────────────────────────────
# backup_file <path> <name>: moves the file into the backup folder under a
# name nobody else has, records it, and leaves the new path in BACKUP_DEST
# (not printed: a subshell couldn't update the record).
BACKUP_DEST=
backup_file() {
  local src=$1 name=$2 ts n=0 dest
  ts="$(date +%Y%m%dT%H%M%S)"
  mkdir -p -m 700 -- "$BACKUPDIR"
  while :; do
    dest="$BACKUPDIR/$name.$ts"; (( n )) && dest="$dest.$n"
    ( set -o noclobber; : >"$dest" ) 2>/dev/null && break
    (( n++ )); (( n > 999 )) && die "couldn't find a free backup name in $BACKUPDIR"
  done
  mv -T -- "$src" "$dest"
  BACKUP_LINES+=("$src"$'\t'"$dest")
  write_manifest
  BACKUP_DEST=$dest
}

# replace_owned <source> <dest> <mode> <expected hash of what's at dest>
# With an empty expected hash, nothing must be at dest (link(2) refuses
# otherwise). With one, the new file is swapped in and the file that came
# out is checked: if it isn't the expected bytes any more (changed after
# the check, or a link that appeared), it is kept as a backup and said so.
replace_owned() {
  local src=$1 dest=$2 mode=$3 expect=$4 dir tmp out b
  dir="$(dirname -- "$dest")"
  mkdir -p -- "$dir"
  tmp="$(mktemp -- "$dir/.opal.XXXXXX")"; TEMPS+=("$tmp")
  cp -- "$src" "$tmp"
  chmod "$mode" -- "$tmp"
  if [[ -z $expect ]]; then
    if ! ln -- "$tmp" "$dest" 2>/dev/null; then
      rm -f -- "$tmp"
      note "$dest appeared while installing; not written."
      return 1
    fi
    rm -f -- "$tmp"
  else
    if (( MV_EXCHANGE )) && mv --exchange -T -- "$tmp" "$dest" 2>/dev/null; then
      out=$tmp
    else
      # No atomic exchange: two renames, a moment with nothing at dest.
      out="$(mktemp -u -- "$dir/.opal.XXXXXX")"
      mv -T -- "$dest" "$out" || { rm -f -- "$tmp"; note "couldn't move $dest aside; not replaced."; return 1; }
      mv -T -- "$tmp" "$dest"
    fi
    if [[ -L $out || "$(file_hash "$out")" != "$expect" ]]; then
      backup_file "$out" "$(basename -- "$dest")"
      note "$dest changed after it was checked; what was there is kept in $BACKUP_DEST"
    else
      rm -f -- "$out"
    fi
  fi
  MANIFEST_HASH["$dest"]="$(file_hash "$dest")"
  write_manifest
}

# remove_owned <path> <expected hash>: moves the file aside, checks it, removes it.
remove_owned() {
  local p=$1 expect=$2 out b
  out="$(mktemp -u -- "$(dirname -- "$p")/.opal.XXXXXX")"
  mv -T -- "$p" "$out" || { note "couldn't move $p aside; left as is."; return 1; }
  if [[ -L $out || "$(file_hash "$out")" != "$expect" ]]; then
    backup_file "$out" "$(basename -- "$p")"
    note "$p changed after it was checked; kept in $BACKUP_DEST"
  else
    rm -f -- "$out"
  fi
  unset 'MANIFEST_HASH[$p]'
  write_manifest
}

# Remove the folders that held <rel> inside <root>, while they're empty.
prune_dirs() {
  local root=$1 rel=$2
  rel="$(dirname -- "$rel")"
  while [[ $rel != . && $rel != / ]]; do
    rmdir -- "$root/$rel" 2>/dev/null || break
    rel="$(dirname -- "$rel")"
  done
}

# walk <dir>: every entry below it, relative, into WALK. Fails (and says
# why) if the folder can't be read fully; then nothing in it is touched.
WALK=()
walk() {
  local dir=$1 list err
  WALK=()
  list="$(mktemp)"; err="$(mktemp)"; TEMPS+=("$list" "$err")
  if ! find "$dir" -mindepth 1 -printf '%P\0' >"$list" 2>"$err"; then
    note "couldn't read all of $dir: $(head -c 200 -- "$err")"
    return 1
  fi
  mapfile -d '' -t WALK <"$list"
  rm -f -- "$list" "$err"
}

# ── The plugin copy ────────────────────────────────────────────────────
plugin_dir_state() {
  local k; k="$(path_kind "$1")"
  case $k in
    missing | symlink | other) echo "$k" ;;
    file) echo other ;;
    dir) if [[ -e $1/.git || -L $1/.git ]]; then echo checkout; else echo dir; fi ;;
  esac
}

# The files Opal ships into a plugin copy: SHIPPED[rel] = source path.
declare -A SHIPPED=()
load_shipped() {
  local f rendered
  SHIPPED=()
  while IFS= read -r -d '' f; do
    [[ ${f#shell-plugin/} == manifest.json ]] && continue
    SHIPPED["${f#shell-plugin/}"]=$f
  done < <(find shell-plugin -type f -print0)
  rendered="$(mktemp)"; TEMPS+=("$rendered")
  render_plugin_manifest >"$rendered"
  SHIPPED[manifest.json]=$rendered
}

# Say "keeping …" once per top-level folder of yours, not once per file in it.
declare -A ANNOUNCED=()
keep_note() {
  local rel=$1 why=$2 top=${1%%/*}
  if [[ $rel == */* && -n ${ANNOUNCED[$top]:-} ]]; then return 0; fi
  if [[ $rel != */* && -d ${3:-}/$rel && ! -L ${3:-}/$rel ]]; then ANNOUNCED[$top]=1; note "keeping $(q "$rel")/: $why"; return 0; fi
  note "keeping $(q "$rel"): $why"
}

# update_plugin_copy <dir>: Opal's files replaced or added in place, its
# files that are no longer shipped removed; everything of yours kept.
update_plugin_copy() {
  local dir=$1 rel kind h
  local -A done=() skip=()
  local -a pruned=()
  ANNOUNCED=()
  load_marker "$dir"
  walk "$dir" || { note "leaving $dir as it is."; return 1; }
  for rel in "${WALK[@]}"; do
    kind="$(path_kind "$dir/$rel")"
    case $kind in
      dir)
        if [[ -n ${SHIPPED[$rel]:-} ]]; then note "keeping $(q "$rel")/: a folder where Opal ships a file (Opal's isn't installed)"; skip[$rel]=1
        else keep_note "$rel" "not Opal's" "$dir"; fi
        unrecord "$dir/$rel"
        continue ;;
      symlink | other)
        keep_note "$rel" "not Opal's (a link)" "$dir"
        [[ -n ${SHIPPED[$rel]:-} ]] && { note "Opal's $(q "$rel") isn't installed."; skip[$rel]=1; }
        unrecord "$dir/$rel"
        continue ;;
    esac
    if [[ $rel == "$MARKER" && ( $MARKER_STATE == known || $MARKER_STATE == hashes ) ]]; then
      remove_owned "$dir/$rel" "$(file_hash "$dir/$rel")"
      continue
    fi
    if owned_file "$dir/$rel" "plugin/$rel"; then
      h="$(file_hash "$dir/$rel")"
      if [[ -n ${SHIPPED[$rel]:-} ]]; then
        replace_owned "${SHIPPED[$rel]}" "$dir/$rel" 644 "$h" && done[$rel]=1
      else
        note "removing $(q "$rel"): Opal no longer ships it"
        remove_owned "$dir/$rel" "$h" && pruned+=("$rel")
      fi
    elif [[ -n ${SHIPPED[$rel]:-} ]]; then
      note "keeping $(q "$rel"): you changed it (Opal's version isn't installed)"
      unset 'MANIFEST_HASH[$dir/$rel]'; write_manifest
      skip[$rel]=1
    else
      keep_note "$rel" "not a file Opal wrote, or you changed it" "$dir"
    fi
  done
  for rel in "${!SHIPPED[@]}"; do
    [[ -n ${done[$rel]:-} || -n ${skip[$rel]:-} ]] && continue
    replace_owned "${SHIPPED[$rel]}" "$dir/$rel" 644 "" || true
  done
  for rel in "${pruned[@]}"; do prune_dirs "$dir" "$rel"; done
}

# create_plugin_copy <dir>: a fresh copy where nothing was.
create_plugin_copy() {
  local dir=$1 rel
  mkdir -p -m 755 -- "$dir"
  for rel in "${!SHIPPED[@]}"; do replace_owned "${SHIPPED[$rel]}" "$dir/$rel" 644 "" || true; done
}

# remove_plugin_copy <dir>: Opal's files go, yours stay. Returns 0 if the
# folder is gone afterwards.
remove_plugin_copy() {
  local dir=$1 rel kind h
  local -a pruned=()
  ANNOUNCED=()
  load_marker "$dir"
  walk "$dir" || { note "leaving $dir as it is."; return 1; }
  for rel in "${WALK[@]}"; do
    kind="$(path_kind "$dir/$rel")"
    case $kind in
      dir) keep_note "$rel" "not Opal's" "$dir"; continue ;;
      symlink | other) keep_note "$rel" "not Opal's (a link)" "$dir"; continue ;;
    esac
    if [[ $rel == "$MARKER" && ( $MARKER_STATE == known || $MARKER_STATE == hashes ) ]]; then
      remove_owned "$dir/$rel" "$(file_hash "$dir/$rel")"; continue
    fi
    if owned_file "$dir/$rel" "plugin/$rel"; then
      h="$(file_hash "$dir/$rel")"
      remove_owned "$dir/$rel" "$h" && pruned+=("$rel")
    else
      keep_note "$rel" "not a file Opal wrote, or you changed it" "$dir"
    fi
  done
  for rel in "${pruned[@]}"; do prune_dirs "$dir" "$rel"; done
  if rmdir -- "$dir" 2>/dev/null; then return 0; fi
  note "kept $dir: it still holds entries that aren't Opal's."
  return 1
}

# ── Everything else Opal touches ───────────────────────────────────────
systemctl_user() {
  local out
  if out="$(systemctl --user "$@" 2>&1)"; then return 0; fi
  note "systemctl --user $*: ${out:-failed}"
  return 0
}
# ask <question> → 0 for yes. Only a terminal can say yes.
ask() {
  local reply
  [[ -t 0 ]] || return 1
  read -r -p "$1 [y/N] " reply </dev/tty || return 1
  [[ $reply =~ ^[Yy] ]]
}

# The unit, as systemd sees it and as it is on disk.
UNIT_KIND=missing UNIT_STATE=missing UNIT_FRAGMENT= UNIT_EXECSTART= UNIT_DROPIN=0
inspect_unit() {
  local show
  UNIT_KIND="$(path_kind "$UNIT")"
  UNIT_DROPIN=0; [[ -d $UNIT.d ]] && UNIT_DROPIN=1
  show="$(systemctl --user show -p FragmentPath -p ExecStart opal.service 2>/dev/null || true)"
  UNIT_FRAGMENT="$(printf '%s\n' "$show" | awk -F= '$1 == "FragmentPath" { print substr($0, 14) }')"
  UNIT_EXECSTART="$(printf '%s\n' "$show" | awk -F= '$1 == "ExecStart" { print substr($0, 11) }')"
  case $UNIT_KIND in
    missing)
      if [[ -n $UNIT_FRAGMENT && $UNIT_FRAGMENT != /dev/null ]]; then UNIT_STATE=elsewhere; else UNIT_STATE=missing; fi ;;
    symlink) UNIT_STATE=symlink ;;
    dir | other) UNIT_STATE=other ;;
    file)
      if [[ -n $UNIT_FRAGMENT && $UNIT_FRAGMENT != "$UNIT" && $UNIT_FRAGMENT != /dev/null ]]; then UNIT_STATE=elsewhere
      elif owned_file "$UNIT" unit; then UNIT_STATE=owned
      elif grep -qF -- "$OPAL_URL" "$UNIT"; then UNIT_STATE=edited
      else UNIT_STATE=foreign; fi ;;
  esac
}
unit_runs_our_binary() { [[ $UNIT_EXECSTART == *"$BINDIR/opald"* ]]; }
unit_is_active() { systemctl --user is-active --quiet opal.service 2>/dev/null; }

LAUNCHER_STATE=missing
inspect_launcher() {
  case "$(path_kind "$LAUNCHER")" in
    missing) LAUNCHER_STATE=missing ;;
    symlink | dir | other) LAUNCHER_STATE=other ;;
    file) if owned_file "$LAUNCHER" launcher; then LAUNCHER_STATE=owned; else LAUNCHER_STATE=foreign; fi ;;
  esac
}

# binary_state <path>: missing | symlink | other | owned | unrecorded
binary_state() {
  case "$(path_kind "$1")" in
    missing) echo missing ;;
    symlink) echo symlink ;;
    dir | other) echo other ;;
    file) if owned_file "$1" "bin/$(basename -- "$1")"; then echo owned; else echo unrecorded; fi ;;
  esac
}

# Take Opal's handler out of mimeapps.list, leaving every other entry.
mimeapps_remove_opal() {
  local tmp rc
  if [[ ! -e $MIMEAPPS && ! -L $MIMEAPPS ]]; then return 0; fi
  if [[ ! -f $MIMEAPPS || -L $MIMEAPPS ]]; then note "$MIMEAPPS isn't a regular file; the nostrconnect:// entry is left as it is."; return 0; fi
  tmp="$(mktemp -- "$(dirname -- "$MIMEAPPS")/.opal.XXXXXX")"; TEMPS+=("$tmp")
  awk '
    BEGIN { key = "x-scheme-handler/nostrconnect="; ours = "opal-nostrconnect.desktop" }
    /^\[/ { insec = ($0 == "[Default Applications]" || $0 == "[Added Associations]") }
    insec && index($0, key) == 1 {
      n = split(substr($0, length(key) + 1), t, ";"); out = ""
      for (i = 1; i <= n; i++) if (t[i] != "" && t[i] != ours) out = out t[i] ";"
      if (out == "") { changed = 1; next }
      if (out != substr($0, length(key) + 1)) { changed = 1; $0 = key out }
    }
    { print }
    END { exit changed ? 0 : 3 }
  ' "$MIMEAPPS" >"$tmp"; rc=$?
  case $rc in
    0) chmod --reference="$MIMEAPPS" -- "$tmp" 2>/dev/null || true
       mv -T -- "$tmp" "$MIMEAPPS"
       note "removed opal-nostrconnect.desktop from the nostrconnect:// entries in $MIMEAPPS" ;;
    3) rm -f -- "$tmp" ;;
    *) rm -f -- "$tmp"; note "couldn't edit $MIMEAPPS (awk exit $rc); left as it is." ;;
  esac
}

# ── Release binaries ───────────────────────────────────────────────────
# The tarball a no-Rust install downloads is checked against a hash pinned
# in this checkout (dist/release-checksums.tsv), never against a checksum
# file fetched from the same release: a release can be edited, a reviewed
# commit can't. Each pin also names the source commit the build attestation
# vouched for when the pin was made.
RELEASE_CHECKSUMS="${OPAL_RELEASE_CHECKSUMS:-dist/release-checksums.tsv}"
# pinned_release <asset>: prints "<sha256> <source commit>" or nothing.
pinned_release() {
  local h a c
  [[ -f $RELEASE_CHECKSUMS ]] || return 0
  while IFS=$'\t' read -r h a c || [[ -n $h ]]; do
    [[ $h == \#* || -z $a ]] && continue
    [[ $a == "$1" && $h =~ ^[0-9a-f]{64}$ ]] && { printf '%s %s\n' "$h" "$c"; return 0; }
  done <"$RELEASE_CHECKSUMS"
}
# verify_pinned <file> <asset>: 0 if the file is exactly the pinned bytes.
verify_pinned() {
  local pin
  pin="$(pinned_release "$2")"
  if [[ -z $pin ]]; then
    note "no pinned checksum for $2 in $RELEASE_CHECKSUMS."
    return 1
  fi
  if [[ "$(file_hash "$1")" != "${pin%% *}" ]]; then
    note "$2 doesn't match the checksum pinned in this checkout."
    return 1
  fi
  PINNED_COMMIT="${pin#* }"
}
PINNED_COMMIT=

print_paths() {
  say "Paths:"
  note "binaries  $BINDIR/opald, $BINDIR/opal"
  note "service   $UNIT"
  note "launcher  $LAUNCHER"
  note "plugin    $PLUGIN_PATH"
  note "record    $MANIFEST$( [[ -f $MANIFEST ]] || printf ' (none yet)')"
  note "data      $DATADIR, $CONFIGDIR, $CACHEDIR"
}
