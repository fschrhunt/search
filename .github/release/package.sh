#!/bin/sh
# Package an already-built native binary; CI and releases use the same archive contract.
set -eu

[ "$#" -eq 5 ] || { echo 'usage: .github/release/package.sh TAG PLATFORM ARCH BINARY OUTPUT_DIRECTORY' >&2; exit 2; }
tag=$1
platform=$2
arch=$3
binary=$4
out=$5
mkdir -p "$out"
work=$(mktemp -d "${TMPDIR:-/tmp}/search-package.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir "$work/package" "$work/verify"
cp "$binary" "$work/package/search"
cp LICENSE "$work/package/"
archive="$out/search_${tag}_${platform}_${arch}.tar.gz"
tar -C "$work/package" -czf "$archive" search LICENSE

printf '%s\n' search LICENSE > "$work/expected"
tar -tzf "$archive" > "$work/members"
diff -u "$work/expected" "$work/members"
tar -C "$work/verify" -xzf "$archive"
test -x "$work/verify/search"
cmp "$binary" "$work/verify/search"
cmp LICENSE "$work/verify/LICENSE"
