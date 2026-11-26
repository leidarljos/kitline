#!/bin/sh
set -eu
cd "$(dirname "$0")"
rm -rf clones
mkdir -p clones
git init -q -b develop clones/ttech
git -C clones/ttech config user.email rgoswami@ieee.org
git -C clones/ttech config user.name "Rohit Goswami"
git -C clones/ttech config commit.gpgsign false
git -C clones/ttech commit --allow-empty -qm root
git -C clones/ttech checkout -q -b land
git -C clones/ttech commit --allow-empty -qm land
tip=$(git -C clones/ttech rev-parse HEAD)
tree=$(git -C clones/ttech rev-parse 'HEAD^{tree}')
extra=$(git -C clones/ttech commit-tree "$tree" -p "$tip" -m extra)
git -C clones/ttech update-ref refs/tmp/extra "$extra"
root=$(pwd)
git -C clones/ttech bundle create "$root/clones/ttech.bundle" refs/tmp/extra "^$tip"
