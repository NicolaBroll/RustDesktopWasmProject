#!/bin/sh
# Rigenera i file patch-*.bin (input dei test di patch_tests.rs) in un repo temporaneo.
# Uso: sh patch.sh <cartella-di-uscita>
#
# Ogni file è l'output di `git -c core.quotePath=false diff` con gli argomenti di DIFF_ARGS
# (ripetuti qui sotto in `d`), più `--cached` per l'indice:
# - patch-worktree.bin: `d`, il diff worktree → indice, input dello stage di righe;
# - patch-indice.bin: `d --cached`, il diff HEAD → indice, input dell'unstage di righe.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Autrice' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

d() {
    git -c core.quotePath=false diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3 "$@"
}

righe() {
    i=1
    while [ "$i" -le "$1" ]; do
        printf 'riga %s\n' "$i"
        i=$((i + 1))
    done
}

git init -q -b main repo
cd repo

# --- Base comune -----------------------------------------------------------------------
righe 20 >vicini.txt
printf 'uno\nsenza a capo' >aggiunge-a-capo.txt
printf 'uno\ncon a capo\n' >toglie-a-capo.txt
printf 'vecchia' >entrambi-senza.txt
printf 'a\nb' >coda.txt
printf 'uno\r\ndue\r\ntre\r\n' >crlf.txt
printf 'caff\350\nseconda\n' >latin1.txt
printf 'a\nb\nc\n' >"con	tab.txt"
printf 'a\nb\nc\n' >"$(printf 'nome\377.txt')"
printf 'via uno\nvia due\nvia tre\n' >eliminato.txt
printf 'resta\n' >script.sh
righe 8 >vecchio-nome.txt
git add -A
git commit -q -m base

# --- Indice: modifiche messe in stage (input dell'unstage) -------------------------------
# vicini.txt: due hunk separati da righe invariate, e due modifiche vicine nello stesso hunk.
sed -e 's/^riga 2$/riga 2 in stage/' -e 's/^riga 4$/riga 4 in stage/' \
    -e 's/^riga 15$/riga 15 in stage\nriga 15 bis/' vicini.txt >tmp && mv tmp vicini.txt
printf 'uno\nsenza a capo\npiù\n' >aggiunge-a-capo.txt
printf 'nuova' >entrambi-senza.txt
printf 'UNO\r\ndue\r\nTRE\r\n' >crlf.txt
printf 'nuovo uno\nnuovo due\nnuovo tre\n' >nuovo.txt
git rm -q eliminato.txt
git mv vecchio-nome.txt "nuovo nome.txt"
sed -e 's/^riga 7$/riga 7 rinominata/' "nuovo nome.txt" >tmp && mv tmp "nuovo nome.txt"
printf 'a\nB\nc\nd\n' >"con	tab.txt"
git add -A
d --cached >"$out/patch-indice.bin"
git commit -q -m indice

# --- Worktree: modifiche non in stage (input dello stage) --------------------------------
righe 20 >vicini.txt
sed -e 's/^riga 1$/riga 1 nel worktree/' -e 's/^riga 9$/riga 9 nel worktree/' \
    -e 's/^riga 17$/riga 17 nel worktree/' vicini.txt >tmp && mv tmp vicini.txt
printf 'uno\nsenza a capo\n' >aggiunge-a-capo.txt
printf 'uno\ncon a capo' >toglie-a-capo.txt
printf 'altra\nnuova' >entrambi-senza.txt
printf 'a\nb\nc' >coda.txt
printf 'UNO\r\nDUE\r\nTRE\r\nquattro\r\n' >crlf.txt
printf 'caff\350\nSECONDA\nterza\n' >latin1.txt
printf 'a\nb\nc\nd\ne\n' >"$(printf 'nome\377.txt')"
rm "nuovo nome.txt"
printf 'resta\naggiunta\n' >script.sh
chmod +x script.sh
printf 'pronto uno\npronto due\npronto tre\n' >intento.txt
git add -N intento.txt
d >"$out/patch-worktree.bin"
