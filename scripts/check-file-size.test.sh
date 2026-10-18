#!/bin/sh
# Exercises scripts/check-file-size in a scratch repository.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
script=$here/check-file-size
hook=$here/../.githooks/pre-commit
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q
git config user.email test@example.com
git config user.name test
mkdir scripts
cp "$script" scripts/check-file-size

bytes() { head -c "$1" /dev/zero | tr '\0' 'a'; }
big() { bytes "$1" > "$2"; }
pass() { if scripts/check-file-size "$@" 2>err; then :; else cat err; echo "FAIL: expected pass: $case" >&2; exit 1; fi; }
fail() { if scripts/check-file-size "$@" 2>err; then echo "FAIL: expected block: $case" >&2; exit 1; fi; }
expect() { grep -q -- "$1" err || { cat err; echo "FAIL: $case: missing '$1'" >&2; exit 1; }; }

case="normal file"
big 1000 small.rs; git add small.rs; pass

case="file at the cap"
big 20000 edge.rs; git add edge.rs; pass

case="oversized file"
big 20001 huge.rs; git add huge.rs; fail
expect "huge.rs is 20001 bytes (~5000 tokens), over the 20000-byte (~5000-token) cap"
expect "move tests into a separate file"
git rm -q --cached huge.rs

case="lockfiles"
for f in Cargo.lock package-lock.json pnpm-lock.yaml yarn.lock poetry.lock uv.lock; do big 50000 "$f"; done
git add Cargo.lock package-lock.json pnpm-lock.yaml yarn.lock poetry.lock uv.lock; pass

case="binary asset"
{ printf 'GIF89a\000'; bytes 50000; } > demo.gif; git add demo.gif; pass

case="binary content without an asset extension"
{ printf '\000'; bytes 50000; } > blob.dat; git add blob.dat; pass

case="generated and vendored files"
mkdir gen vendor
big 50000 gen/out.rs; big 50000 vendor/lib.rs
printf 'gen/** linguist-generated\nvendor/** linguist-vendored\n' > .gitattributes
git add .gitattributes gen vendor; pass

case="baselined offender"
git commit -qm base
big 30000 legacy.rs; git add legacy.rs
printf '# bytes path\n30000 legacy.rs\n' > .file-size-baseline; git add .file-size-baseline; pass

case="baselined offender that shrinks"
big 25000 legacy.rs; git add legacy.rs; pass

case="baselined offender that grows"
big 30001 legacy.rs; git add legacy.rs; fail
expect "legacy.rs grew to 30001 bytes (~7500 tokens); its baseline is 30000 bytes"
big 30000 legacy.rs; git add legacy.rs

case="file with spaces in its name"
big 20001 "my file.rs"; git add "my file.rs"; fail; expect "my file.rs is 20001 bytes"
git rm -q --cached "my file.rs"

case="--all checks tracked files"
git commit -qm more
pass --all
big 20001 huge.rs; git add huge.rs; fail --all; expect "huge.rs is 20001 bytes"

case="--list prints offenders in baseline format"
scripts/check-file-size --list > list
if ! grep -qx "20001 huge.rs" list || ! grep -qx "30000 legacy.rs" list; then
  cat list; echo "FAIL: $case" >&2; exit 1
fi

case="pre-commit blocks the commit"
mkdir .githooks
cp "$hook" .githooks/pre-commit
git config core.hooksPath .githooks
if git commit -qm huge 2>err; then echo "FAIL: $case" >&2; exit 1; fi
expect "huge.rs is 20001 bytes"

echo "check-file-size: all cases passed"
