#!/bin/sh
# Rigenera i file conflict-*.bin (input dei test di conflict_tests.rs) in un repo temporaneo.
# Uso: sh conflict.sh <cartella-di-uscita>
#
# Un merge di `altro` in `main` con conflitti in più file; ogni file .bin è il file del worktree
# dopo `git merge altro` (stile merge), o dopo `git checkout --conflict=diff3` / `zdiff3`:
# - conflict-merge.bin: due blocchi, stile merge;
# - conflict-diff3.bin, conflict-zdiff3.bin: lo stesso file con la base;
# - conflict-crlf.bin: righe con CRLF;
# - conflict-coda.bin: ultima riga senza a capo da entrambi i lati;
# - conflict-vuoto.bin: `main` toglie le righe che `altro` cambia (lato nostro vuoto);
# - conflict-size.bin: `conflict-marker-size=10` da `.gitattributes`;
# - conflict-latin1.bin: contenuto non UTF-8.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Autrice' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

git init -q -b main repo
cd repo
printf 'size.txt conflict-marker-size=10\n' >.gitattributes
printf 'uno\ndue\ntre\nquattro\ncinque\nsei\nsette\notto\nnove\n' >merge.txt
printf 'uno\r\ndue\r\ntre\r\n' >crlf.txt
printf 'uno\ndue' >coda.txt
printf 'prima\nx\ny\ndopo\n' >vuoto.txt
printf 'uno\ndue\ntre\n' >size.txt
printf 'caff\350\ndue\n' >latin1.txt
git add -A && git commit -q -m base

git checkout -q -b altro
printf 'uno\nDUE altro\ntre\nquattro\ncinque\nsei\nsette\nOTTO altro\nnove\n' >merge.txt
printf 'uno\r\nDUE altro\r\ntre\r\n' >crlf.txt
printf 'uno\ndue altro' >coda.txt
printf 'prima\nX\nY\ndopo\n' >vuoto.txt
printf 'uno\nDUE altro\ntre\n' >size.txt
printf 'caff\350 altro\ndue\n' >latin1.txt
git commit -q -am altro

git checkout -q main
printf 'uno\nDUE main\ntre\nquattro\ncinque\nsei\nsette\nOTTO main\nOTTO bis\nnove\n' >merge.txt
printf 'uno\r\nDUE main\r\ntre\r\n' >crlf.txt
printf 'uno\ndue main' >coda.txt
printf 'prima\ndopo\n' >vuoto.txt
printf 'uno\nDUE main\ntre\n' >size.txt
printf 'caff\350 main\ndue\n' >latin1.txt
git commit -q -am main

git merge -q altro >/dev/null 2>&1 || true
for f in merge crlf coda vuoto size latin1; do cp "$f.txt" "$out/conflict-$f.bin"; done
git checkout -q --conflict=diff3 -- merge.txt
cp merge.txt "$out/conflict-diff3.bin"
git checkout -q --conflict=zdiff3 -- merge.txt
cp merge.txt "$out/conflict-zdiff3.bin"
