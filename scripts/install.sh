#!/bin/sh
# Install an extracted, checksum-bearing package without touching device state.
set -eu
umask 077
fail() { printf '%s\n' "$*" >&2; exit 1; }
valid_id() { case "$1" in ''|*[!a-zA-Z0-9._-]*|.*) return 1;; esac; [ "${#1}" -le 160 ]; }
hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}
[ "$#" -ge 2 ] || fail 'Usage: install.sh install PACKAGE PREFIX | install.sh rollback PREFIX'
action=$1
case "$action" in
  install) [ "$#" -eq 3 ] || fail 'Expected PACKAGE PREFIX'; package=$2; prefix=$3;;
  rollback) [ "$#" -eq 2 ] || fail 'Expected PREFIX'; prefix=$2;;
  *) fail 'Unknown action';;
esac
mkdir -p "$prefix"
prefix=$(CDPATH= cd -- "$prefix" && pwd)
if [ ! -f "$prefix/.everywhere-install" ]; then
  [ -z "$(ls -A "$prefix")" ] || fail 'Prefix must be empty or an existing Everywhere installation'
  printf 'everywhere-install-v1\n' > "$prefix/.everywhere-install"
fi
[ "$(cat "$prefix/.everywhere-install")" = everywhere-install-v1 ] || fail 'Unknown installation format'
mkdir "$prefix/.install-lock" 2>/dev/null || fail 'Installation is busy; inspect .install-lock after an interrupted installer'
trap 'rm -f "$prefix/.current-new" "$prefix/.previous-new"; rmdir "$prefix/.install-lock"' EXIT HUP INT TERM
switch_to() {
  next=$1
  valid_id "$next" || fail 'Invalid version pointer'
  [ -f "$prefix/versions/$next/everywhere" ] || fail 'Version binary is missing'
  if [ -f "$prefix/current" ]; then
    previous=$(cat "$prefix/current")
    valid_id "$previous" || fail 'Invalid current pointer'
    [ "$previous" != "$next" ] || return 0
    printf '%s\n' "$previous" > "$prefix/.previous-new"
    mv -f "$prefix/.previous-new" "$prefix/previous"
  fi
  printf '%s\n' "$next" > "$prefix/.current-new"
  mv -f "$prefix/.current-new" "$prefix/current"
}
if [ "$action" = rollback ]; then
  [ -f "$prefix/previous" ] || fail 'No previous version available'
  next=$(cat "$prefix/previous")
  switch_to "$next"
  printf 'Rolled back to %s\n' "$next"
  exit 0
fi
[ -f "$package/everywhere" ] && [ ! -L "$package/everywhere" ] || fail 'Package binary must be a regular file'
version=$(cat "$package/VERSION")
valid_id "$version" || fail 'Invalid package version'
[ "${#version}" -le 64 ] || fail 'Package version too long'
expected=$(awk 'NR==1 {print $1}' "$package/SHA256SUMS")
case "$expected" in *[!a-f0-9]*) fail 'Invalid SHA-256 checksum';; esac
[ "${#expected}" -eq 64 ] || fail 'Invalid SHA-256 checksum'
[ "$(hash_file "$package/everywhere")" = "$expected" ] || fail 'Package checksum mismatch'
next="$version-$expected"
mkdir -p "$prefix/versions"
stage="$prefix/versions/.stage-$$"
[ ! -e "$stage" ] || fail 'Staging directory already exists'
mkdir "$stage"
cp "$package/everywhere" "$stage/everywhere"
chmod 700 "$stage/everywhere"
[ "$(hash_file "$stage/everywhere")" = "$expected" ] || fail 'Copied binary checksum mismatch'
"$stage/everywhere" --version >/dev/null || fail 'Candidate cannot run on this device'
if [ -d "$prefix/versions/$next" ]; then
  [ "$(hash_file "$prefix/versions/$next/everywhere")" = "$expected" ] || fail 'Installed version is corrupt'
  rm "$stage/everywhere"; rmdir "$stage"
else
  mv "$stage" "$prefix/versions/$next"
fi
cat > "$prefix/.launcher-new" <<'LAUNCHER'
#!/bin/sh
set -eu
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
version=$(cat "$base/current")
case "$version" in ''|*[!a-zA-Z0-9._-]*|.*) printf 'Invalid installation pointer\n' >&2; exit 1;; esac
exec "$base/versions/$version/everywhere" "$@"
LAUNCHER
chmod 700 "$prefix/.launcher-new"
mv -f "$prefix/.launcher-new" "$prefix/everywhere"
switch_to "$next"
printf 'Installed %s\nRun: %s/everywhere\n' "$version" "$prefix"
