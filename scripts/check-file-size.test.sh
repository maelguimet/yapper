#!/bin/sh
# Exercises scripts/check-file-size and .githooks/pre-commit in a scratch repository.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
script=$here/check-file-size
hook=$here/../.githooks/pre-commit
workflow=$here/../.github/workflows/file-size.yml
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q
git config user.email test@example.com
git config user.name test
git config core.quotePath true
mkdir scripts .githooks
cp "$script" scripts/check-file-size
cp "$hook" .githooks/pre-commit
git add scripts .githooks
git commit -q --no-verify -m tooling

bytes() { head -c "$1" /dev/zero | tr '\0' 'a'; }
big() { bytes "$1" >"$2"; }
pass() {
  if ! scripts/check-file-size "$@" 2>err; then
    cat err
    echo "FAIL: expected pass: $case" >&2
    exit 1
  fi
}
fail() {
  if scripts/check-file-size "$@" 2>err; then
    echo "FAIL: expected block: $case" >&2
    exit 1
  fi
}
expect() {
  grep -qF -- "$1" err || {
    cat err
    echo "FAIL: $case: missing '$1'" >&2
    exit 1
  }
}
commit() { git commit -q --no-verify -m "$1"; }

case="normal file"
big 1000 small.rs
git add small.rs
pass

case="file at the cap"
big 20000 edge.rs
git add edge.rs
pass

case="oversized file"
big 20001 huge.rs
git add huge.rs
fail
expect "huge.rs is 20001 bytes (~5000 tokens), over the 20000-byte (~5000-token) cap"
expect "move tests into a separate file"
git rm -q --cached huge.rs

case="lockfiles by exact name"
for f in Cargo.lock package-lock.json pnpm-lock.yaml yarn.lock poetry.lock uv.lock; do big 50000 "$f"; done
git add Cargo.lock package-lock.json pnpm-lock.yaml yarn.lock poetry.lock uv.lock
pass

case="a text file merely ending in .lock"
big 50000 notes.lock
git add notes.lock
fail
expect "notes.lock is 50000 bytes"
git rm -q --cached notes.lock

case="hand-written svg"
big 50000 icon.svg
git add icon.svg
fail
expect "icon.svg is 50000 bytes"
git rm -q --cached icon.svg

case="binary content"
{ printf 'GIF89a\000'; bytes 50000; } >demo.gif
{ printf '\000'; bytes 50000; } >blob.dat
git add demo.gif blob.dat
pass

case="quoted names: accents, tab, newline, quote"
for name in "café.rs" "tab	name.rs" "new
line.rs" 'quote".rs' "sp ace.rs"; do
  big 30000 "$name"
  git add "$name"
  fail
  git rm -q --cached "$name"
done
big 30000 "café.rs"
git add "café.rs"
fail --all
expect "café.rs is 30000 bytes"
git rm -q --cached "café.rs"

case="staged content counts, not the working tree"
big 30000 staged.rs
git add staged.rs
big 10 staged.rs
fail
expect "staged.rs is 30000 bytes"
big 10 staged.rs
git add staged.rs
big 30000 staged.rs
pass
git rm -q -f --cached staged.rs
rm staged.rs

commit base

case="a new over-cap file under an existing exemption"
mkdir gen vendor
printf 'gen/** linguist-generated\nvendor/** linguist-vendored\n' >.gitattributes
git add .gitattributes
commit attrs
big 50000 gen/out.rs
big 50000 vendor/lib.rs
git add gen vendor
fail
expect "gen/out.rs is 50000 bytes (~12500 tokens) and exempt through .gitattributes, but was not an exempt file at HEAD"
expect "vendor/lib.rs is 50000 bytes"
pass --all
commit generated

case="generated and vendored paths committed earlier may grow"
big 60000 gen/out.rs
big 60000 vendor/lib.rs
git add gen vendor
pass
git reset -q --hard

case="uncommitted .gitattributes does not exempt"
printf 'gen/** linguist-generated\nvendor/** linguist-vendored\nbig/** linguist-generated\n' >.gitattributes
mkdir big
big 50000 big/file.rs
git add big/file.rs
fail
expect "big/file.rs is 50000 bytes"

case="staged .gitattributes exemption is rejected"
git add .gitattributes
fail
expect "big/file.rs is 50000 bytes (~12500 tokens) and exempt through .gitattributes, but was not an exempt file at HEAD"
git reset -q
git checkout -q -- .gitattributes
rm -rf big

case="baselined offender"
big 30000 legacy.rs
printf '# bytes path\n30000 legacy.rs\n' >.file-size-baseline
git add legacy.rs .file-size-baseline
commit legacy
pass --all

case="baselined offender that shrinks, with its entry lowered"
big 25000 legacy.rs
printf '# bytes path\n25000 legacy.rs\n' >.file-size-baseline
git add legacy.rs .file-size-baseline
pass

case="baselined offender that grows"
big 30001 legacy.rs
git add legacy.rs
fail
expect "legacy.rs grew to 30001 bytes (~7500 tokens); its baseline is 25000 bytes"
git reset -q --hard

case="a new offender with a new baseline entry is rejected"
big 40000 sneaky.rs
printf '# bytes path\n30000 legacy.rs\n40000 sneaky.rs\n' >.file-size-baseline
git add sneaky.rs .file-size-baseline
fail
expect ".file-size-baseline adds sneaky.rs (40000 bytes)"

case="raising a baseline entry is rejected"
git reset -q --hard
big 35000 legacy.rs
printf '# bytes path\n35000 legacy.rs\n' >.file-size-baseline
git add legacy.rs .file-size-baseline
fail
expect ".file-size-baseline raises legacy.rs from 30000 to 35000 bytes"
git reset -q --hard

case="renaming a baselined offender needs a new entry, which is rejected"
git mv legacy.rs moved.rs
fail
expect "moved.rs is 30000 bytes"
printf '# bytes path\n30000 moved.rs\n' >.file-size-baseline
git add .file-size-baseline
fail
expect ".file-size-baseline adds moved.rs"
git reset -q --hard

case="--against a base ref, as CI runs it"
git checkout -q -b feature
big 40000 sneaky.rs
printf '# bytes path\n30000 legacy.rs\n40000 sneaky.rs\n' >.file-size-baseline
git add sneaky.rs .file-size-baseline
commit sneaky
fail --against master
expect "adds sneaky.rs"
pass --all
git checkout -q master
pass --against HEAD
fail --against no-such-ref
expect "unknown ref no-such-ref"

case="--against a base without a baseline file"
git checkout -q -b nobase
git rm -q .file-size-baseline legacy.rs
commit "drop baseline"
git checkout -q -b adds-baseline
big 40000 sneaky.rs
printf '40000 sneaky.rs\n' >.file-size-baseline
git add sneaky.rs .file-size-baseline
commit sneaky
fail --against nobase
expect "adds sneaky.rs"
git checkout -q master

case="--against a base that predates the check"
git checkout -q --orphan pre-check
git rm -rq --cached .
commit_empty() { git commit -q --no-verify --allow-empty -m "$1"; }
commit_empty pre
git checkout -q -f master
pass --against pre-check
expect "pre-check predates this check"

case="--against: renaming the script does not skip the base check"
git checkout -q -b renamed
git mv scripts/check-file-size scripts/size-check
printf '# bytes path\n80000 legacy.rs\n' >.file-size-baseline
git add .file-size-baseline
commit renamed
if scripts/size-check --against master 2>err; then
  echo "FAIL: expected block: $case" >&2
  exit 1
fi
expect "raises legacy.rs from 30000 to 80000 bytes"
git checkout -q master

ci() {
  git update-ref refs/remotes/origin/master master
  runner=$(mktemp -d)
  awk '
    /with the base branch.s checker/ { step = 1 }
    step && /run: \|/ { body = 1; next }
    body && /^ *[^ ]/ { if (!indent) indent = match($0, /[^ ]/); if (match($0, /[^ ]/) < indent) exit }
    body { print substr($0, indent) }
  ' "$workflow" >ci.sh
  [ -s ci.sh ] || { echo "FAIL: no PR step in $workflow" >&2; exit 1; }
  BASE_REF=master RUNNER_TEMP=$runner sh -eu ci.sh 2>err
  ci_status=$?
  rm -rf "$runner" ci.sh
  return "$ci_status"
}

case="CI runs the base branch's checker, not the PR's"
git checkout -q -b weakened
printf '#!/bin/sh\nexit 0\n' >scripts/check-file-size
printf '# bytes path\n80000 legacy.rs\n' >.file-size-baseline
git add scripts/check-file-size .file-size-baseline
commit weakened
if ci; then
  echo "FAIL: expected block: $case" >&2
  exit 1
fi
expect "raises legacy.rs from 30000 to 80000 bytes"
git checkout -q master

case="CI on a clean PR passes"
git checkout -q -b clean
big 100 clean.rs
git add clean.rs
commit clean
ci || { cat err; echo "FAIL: expected pass: $case" >&2; exit 1; }
git checkout -q master

case="--against: narrowing an exemption passes"
git checkout -q -b narrow-base
mkdir -p training/a training/x
printf 'gen/** linguist-generated\nvendor/** linguist-vendored\ntraining/**/*.jsonl linguist-generated\n' >.gitattributes
big 60000 training/a/train1.jsonl
git add .gitattributes training
commit "training data"
git checkout -q -b narrowed
printf 'gen/** linguist-generated\nvendor/** linguist-vendored\ntraining/**/train*.jsonl linguist-generated\n' >.gitattributes
git add .gitattributes
pass
commit narrowed
pass --against narrow-base
pass --all

case="--against: a new over-cap file under an existing broad exemption"
git checkout -q narrow-base
git checkout -q -b notes
big 60000 training/x/notes.jsonl
git add training/x/notes.jsonl
commit notes
fail --against narrow-base
expect "training/x/notes.jsonl is 60000 bytes (~15000 tokens) and exempt through .gitattributes, but was not an exempt file at narrow-base"
pass --all

case="--against: a new exemption for a file committed earlier"
git checkout -q narrow-base
git checkout -q -b widened
big 60000 training/a/notes.md
git add training/a/notes.md
commit "notes over the cap"
printf 'training/** linguist-generated\n' >>.gitattributes
git add .gitattributes
fail
expect "training/a/notes.md is 60000 bytes"
commit widened
fail --against HEAD~1
expect "training/a/notes.md is 60000 bytes (~15000 tokens) and exempt through .gitattributes, but was not an exempt file at HEAD~1"
git checkout -q master

case="nothing staged"
git checkout -q clean
pass
git commit -q --allow-empty -m empty
pass --against master
git checkout -q master

case="--all checks tracked files"
big 20001 huge.rs
git add huge.rs
fail --all
expect "huge.rs is 20001 bytes"

case="--list prints offenders in baseline format"
scripts/check-file-size --list >list
if ! grep -qx "20001 huge.rs" list || ! grep -qx "30000 legacy.rs" list; then
  cat list
  echo "FAIL: $case" >&2
  exit 1
fi

case="the pre-commit hook blocks the commit"
git config core.hooksPath .githooks
if git commit -qm huge 2>err; then
  echo "FAIL: $case" >&2
  exit 1
fi
expect "huge.rs is 20001 bytes"

echo "check-file-size: all cases passed"
