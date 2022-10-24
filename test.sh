#!/bin/sh -e

TMPDIR=${TMPDIR:-/tmp}

rm -rf "$TMPDIR/working_copies" "$TMPDIR/repos"

SRCDIR=$PWD
mkdir "$TMPDIR/repos"
cd "$TMPDIR/repos"
REPODIR=$PWD
git init --bare composite
git init --bare service1
git init --bare service2
cd composite
mkdir "$TMPDIR/working_copies"
cd "$TMPDIR/working_copies"
WRKDIR=$PWD
git clone "$REPODIR/service1"
git -C service1 commit --allow-empty -m "Initial commit"
git -C service1 push origin main
git clone "$REPODIR/service2"
git -C service2 commit --allow-empty -m "Initial commit"
git -C service2 push origin main
git clone "$REPODIR/composite"
cd composite
git -c protocol.file.allow=always submodule add ../service1
git -c protocol.file.allow=always submodule add ../service2
git commit -m "Add submodules"
git push origin main
cd ..
cd service1
head -c 100 /dev/random | base64 > file
git add file
git commit -m "$(date)"
git checkout -b feature/a_feature
head -c 100 /dev/random | base64 > file
git add file
git commit -m "$(date)-feature"
git push origin feature/a_feature
cd "$SRCDIR"
RUST_BACKTRACE=1 RUST_LOG=trace cargo run -- pipeline -r "$WRKDIR/service1" -c "$REPODIR/composite"
