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

git checkout -q main
printf 'trunk\n' > trunk.txt
cmt 2020-02-01T00:00:00Z trunk-moves
at 2020-02-01T00:00:01Z git cherry-pick "$feat"
at 2020-02-01T00:00:02Z git cherry-pick "$rev"
at 2020-02-01T00:00:03Z git revert --no-edit HEAD
printf 'line1\nline2\n' > sq.txt
cmt 2020-02-02T00:00:00Z squash-sq
at 2020-02-02T00:00:01Z git cherry-pick "$part"
printf 'main\n' > conflict.txt
cmt 2020-02-03T00:00:00Z main-conflict
git checkout -q main
