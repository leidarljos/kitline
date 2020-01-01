#!/bin/sh
# Deterministic fixture. Each head is named grade/<grade>/<bucket>.
# No network. Dates are fixed so the repo bytes do not depend on the clock.
set -eu
if [ "$#" -ne 1 ]; then
  echo "usage: init.sh <repo>" >&2
  exit 2
fi
repo=$1
rm -rf "$repo"
mkdir -p "$repo"
cd "$repo"

export GIT_AUTHOR_NAME="Rohit Goswami"
export GIT_AUTHOR_EMAIL="rgoswami@ieee.org"
export GIT_COMMITTER_NAME="Rohit Goswami"
export GIT_COMMITTER_EMAIL="rgoswami@ieee.org"

git init -q -b main
git config user.email rgoswami@ieee.org
git config user.name "Rohit Goswami"
git config commit.gpgsign false
git config core.hooksPath /dev/null

cmt() {
  GIT_AUTHOR_DATE=$1
  GIT_COMMITTER_DATE=$1
  export GIT_AUTHOR_DATE GIT_COMMITTER_DATE
  shift
  git add -A
  git commit -qm "$1"
}

at() {
  GIT_AUTHOR_DATE=$1
  GIT_COMMITTER_DATE=$1
  export GIT_AUTHOR_DATE GIT_COMMITTER_DATE
  shift
  "$@"
}

printf 'base\n' > base.txt
cmt 2020-01-01T00:00:00Z root
printf 'base\n' > conflict.txt
printf 'shared\n' > shared.txt
cmt 2020-01-02T00:00:00Z fork
fork=$(git rev-parse HEAD)
git branch grade/ancestor/drop "$fork"

git checkout -q -b grade/patch-equal/drop "$fork"
printf 'feat\n' > feat.txt
cmt 2020-01-03T00:00:00Z feat
feat=$(git rev-parse HEAD)

git checkout -q -b grade/patch-equal/rest "$fork"
printf 'rev\n' > rev.txt
cmt 2020-01-04T00:00:00Z rev
rev=$(git rev-parse HEAD)

git checkout -q -b grade/squash-equal/drop "$fork"
printf 'line1\n' > sq.txt
cmt 2020-01-05T00:00:00Z sq-one
printf 'line1\nline2\n' > sq.txt
cmt 2020-01-06T00:00:00Z sq-two

# Squash-equal, and the merge is not a no-op. Label: squash-equal / rest.
git checkout -q -b grade/squash-equal/rest "$fork"
printf 'alpha  beta\n' > sqrest.txt
cmt 2020-01-11T00:00:00Z sqrest-one
printf 'alpha  beta\ngamma\n' > sqrest.txt
cmt 2020-01-12T00:00:00Z sqrest-two

git checkout -q -b grade/no-op/drop "$fork"
printf 'x\n' > noop.txt
cmt 2020-01-07T00:00:00Z noop-add
git rm -q noop.txt
cmt 2020-01-08T00:00:00Z noop-drop

git checkout -q -b grade/partial/rebase "$fork"
printf 'p\n' > part.txt
cmt 2020-01-09T00:00:00Z part-one
part=$(git rev-parse HEAD)
printf 'q\n' > part2.txt
cmt 2020-01-10T00:00:00Z part-two

git checkout -q -b grade/open/rest "$fork"
printf 'apply\n' > apply.txt
cmt 2024-06-01T00:00:00Z apply

git checkout -q -b grade/open/rebase "$fork"
printf 'branch\n' > conflict.txt
cmt 2021-03-01T00:00:00Z conflict

git checkout -q -b grade/keep/keep "$fork"
printf 'rel\n' > release.txt
cmt 2022-02-02T00:00:00Z release

# v1: own commit, patch already on trunk. Tag v1 is a different ancestor.
git checkout -q -b v1 "$fork"
printf 'v1\n' > v1.txt
cmt 2020-03-01T00:00:00Z v1-unique
v1feat=$(git rev-parse HEAD)

# HEAD: unique work, not the symbolic HEAD. The ref is created at the end
# so later commands still resolve HEAD to the symbolic ref.
git checkout -q -b tmp-head "$fork"
printf 'head\n' > head.txt
cmt 2020-03-02T00:00:00Z head-unique
headsha=$(git rev-parse HEAD)
git checkout -q main
git branch -D tmp-head

# Leading-dash name. Must not be parsed as an option.
git checkout -q -b tmp-out "$fork"
printf 'out\n' > out.txt
cmt 2020-03-03T00:00:00Z out-unique
git update-ref refs/heads/--output=x HEAD
git checkout -q main
git branch -D tmp-out

# U+00A0 in the name (UTF-8 c2 a0). Not ASCII space.
git checkout -q -b tmp-nbsp "$fork"
printf 'nbsp\n' > nbsp.txt
cmt 2020-03-04T00:00:00Z nbsp-unique
nbsp=$(printf 'foo\302\240bar')
git update-ref "refs/heads/${nbsp}" HEAD
git checkout -q main
git branch -D tmp-nbsp

# Trailing U+00A0. Ancestor of trunk, so the row is DROP.
git update-ref "$(printf 'refs/heads/trail\302\240')" "$fork"

# A byte that is not UTF-8. Ancestor of trunk, so the row is DROP.
git update-ref "$(printf 'refs/heads/bad\377name')" "$fork"

# A bar in the name. Unique work, so the row is open / rest.
git checkout -q -b tmp-bar "$fork"
printf 'bar\n' > bar.txt
cmt 2020-03-05T00:00:00Z bar-unique
git update-ref 'refs/heads/foo|bar' HEAD
git checkout -q main
git branch -D tmp-bar

# Author instants whose %aI text order is not epoch order.
# Label: open / rest. Named dates so it does not sit under grade/open/rest.
git checkout -q -b grade/open/dates "$fork"
printf 'early\n' > span.txt
cmt 2020-01-02T00:00:00Z span-early
printf 'late\n' > span.txt
cmt 2020-01-01T23:00:00-05:00 span-late

# Conflict path contains a space.
# Label: open / rebase.
git checkout -q -b grade/open/space "$fork"
printf 'branch\n' > "my file.txt"
cmt 2021-04-01T00:00:00Z space-conflict

# Conflict path has a leading ASCII space. Label: open / rebase.
git checkout -q -b grade/open/lead "$fork"
printf 'branch\n' > " foo.txt"
cmt 2021-04-02T00:00:00Z lead-conflict

git checkout -q main
printf 'trunk\n' > trunk.txt
cmt 2020-02-01T00:00:00Z trunk-moves
at 2020-02-01T00:00:01Z git cherry-pick "$feat"
at 2020-02-01T00:00:02Z git cherry-pick "$rev"
at 2020-02-01T00:00:03Z git revert --no-edit HEAD
printf 'line1\nline2\n' > sq.txt
cmt 2020-02-02T00:00:00Z squash-sq
printf 'alpha beta\ngamma\n' > sqrest.txt
cmt 2020-02-05T00:00:00Z squash-rest
at 2020-02-02T00:00:01Z git cherry-pick "$part"
printf 'main\n' > conflict.txt
cmt 2020-02-03T00:00:00Z main-conflict
at 2020-02-03T00:00:01Z git cherry-pick "$v1feat"
printf 'main\n' > "my file.txt"
cmt 2020-02-04T00:00:00Z main-space
printf 'main\n' > " foo.txt"
cmt 2020-02-06T00:00:00Z main-lead
git tag v1 "$fork"

# Orphan: no merge base with main.
blob=$(printf 'orphan\n' | git hash-object -w --stdin)
tree=$(printf '100644 blob %s\torphan.txt\n' "$blob" | git mktree)
orph=$(
  GIT_AUTHOR_DATE=2020-05-01T00:00:00Z
  GIT_COMMITTER_DATE=2020-05-01T00:00:00Z
  export GIT_AUTHOR_DATE GIT_COMMITTER_DATE
  git commit-tree "$tree" -m orphan
)
git update-ref refs/heads/grade/unrelated/unsorted "$orph"
git update-ref refs/heads/HEAD "$headsha"
git checkout -q main
