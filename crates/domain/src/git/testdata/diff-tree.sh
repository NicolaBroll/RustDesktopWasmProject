#!/bin/sh
# Rigenera i file diff-tree-*.bin (usati da diff_tree_tests.rs) in un repo temporaneo.
# Uso: sh diff-tree.sh <cartella-di-uscita>
#
# Ogni file è l'output di `git diff-tree` con gli argomenti di DIFF_TREE_ARGS (ripetuti qui
# sotto in `dt`: se cambiano in diff_tree.rs vanno cambiati anche qui), più le revisioni:
# `--root <commit>` per il commit radice, `<primo genitore> <commit>` per gli altri.
# `diff-tree-copie.bin` aggiunge `-C --find-copies-harder`.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Autrice' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

dt() {
    git diff-tree -z -r --raw --no-abbrev -M --no-commit-id "$@"
}

git init -q -b main repo
cd repo

# --- Radice: file in cartelle, con spazi, a capo e byte non UTF-8 nel nome. ----------------
mkdir -p src/annidata
printf 'uno\ndue\ntre\nquattro\ncinque\nsei\n' >modificato.txt
printf 'contenuto abbastanza lungo da riconoscere la rinomina\n' >vecchio.txt
printf 'contenuto modificato poco, abbastanza lungo\nriga due\nriga tre\nriga quattro\n' >sposta.txt
printf 'da eliminare\n' >eliminato.txt
printf '#!/bin/sh\n' >script.sh
printf 'diventa un link\n' >tipo.txt
printf 'spazio\n' >'con spazio.txt'
printf 'a capo\n' >"$(printf 'a\ncapo.txt')"
printf 'latin\n' >"$(printf 'caff\377.txt')"
printf 'profondo\n' >src/annidata/profondo.rs
git add -A
git commit -q -m 'Radice'
dt --root HEAD >"$out/diff-tree-radice.bin"

# --- Ogni tipo di modifica rispetto al genitore. -----------------------------------------
printf 'uno\ndue\nTRE\nquattro\ncinque\nsei\n' >modificato.txt
git mv vecchio.txt nuovo.txt
git mv sposta.txt spostato.txt
printf 'riga cinque\n' >>spostato.txt
git rm -q eliminato.txt
chmod +x script.sh
rm tipo.txt
ln -s modificato.txt tipo.txt
printf 'aggiunto\n' >src/aggiunto.rs
git add -A
git commit -q -m 'Ogni tipo di modifica'
dt HEAD~1 HEAD >"$out/diff-tree-misto.bin"

# --- Copia: riconosciuta solo con -C. ---------------------------------------------------
cp modificato.txt copia.txt
git add copia.txt
git commit -q -m 'Copia'
dt -C --find-copies-harder HEAD~1 HEAD >"$out/diff-tree-copie.bin"

# --- Commit vuoto: nessun file. ---------------------------------------------------------
git commit -q --allow-empty -m 'Vuoto'
dt HEAD~1 HEAD >"$out/diff-tree-vuoto.bin"

# --- Merge: confronto con il primo genitore (i file portati dal branch unito). -----------
git checkout -q -b laterale
printf 'dal branch\n' >laterale.txt
git add laterale.txt
git commit -q -m 'Laterale'
git checkout -q main
printf 'da main\n' >principale.txt
git add principale.txt
git commit -q -m 'Principale'
git merge -q --no-ff --no-edit laterale
dt HEAD^1 HEAD >"$out/diff-tree-merge.bin"
