#!/bin/sh
# Rigenera i file diff-*.bin (usati da diff_tests.rs) in un repo temporaneo.
# Uso: sh diff.sh <cartella-di-uscita>
#
# Ogni file è l'output di `git -c core.quotePath=false diff` con gli argomenti di DIFF_ARGS
# (ripetuti qui sotto in `d`: se cambiano in diff.rs vanno cambiati anche qui), più le
# revisioni o `--cached`. Eccezioni, indicate caso per caso: `diff-nomi-quotepath.bin` lascia
# `core.quotePath` al valore predefinito (vero, nomi non ASCII in ottale) e `diff-config.bin`
# usa una configurazione ostile che gli argomenti devono neutralizzare.
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

git init -q -b main repo
cd repo

# --- Testo: più hunk con intestazione di contesto, conteggi omessi, fine file senza a capo,
# CRLF, contenuto non UTF-8, righe che sembrano intestazioni. -----------------------------
printf 'int main() {\n' >codice.c
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    printf '    riga(%s);\n' "$i" >>codice.c
done
printf '}\n' >>codice.c
printf 'solo\n' >una-riga.txt
printf 'resta\nsenza a capo' >aggiunge-a-capo.txt
printf 'resta\ncon a capo\n' >toglie-a-capo.txt
printf 'vecchia senza a capo' >entrambi-senza.txt
printf 'uno\r\ndue\r\n' >crlf.txt
printf 'caff\350\nseconda\n' >latin1.txt
printf 'testa\n-- trattini\n++ pi\303\271\ncoda\n' >finte-intestazioni.txt
printf 'prima\n\nterza\n' >riga-vuota.txt
git add -A
git commit -q -m 'Testo'

sed -e 's/riga(2);/riga(2); \/\/ cambiata/' -e 's/riga(18);/riga(18);\n    nuova();/' codice.c >tmp
mv tmp codice.c
printf 'sola\n' >una-riga.txt
printf 'resta\nsenza a capo\n' >aggiunge-a-capo.txt
printf 'resta\ncon a capo' >toglie-a-capo.txt
printf 'nuova senza a capo' >entrambi-senza.txt
printf 'uno\r\nDUE\r\n' >crlf.txt
printf 'caff\350\nSECONDA\n' >latin1.txt
printf 'testa\n--- a/finta\n+++ b/finta\ndiff --git a/x b/x\ncoda\n' >finte-intestazioni.txt
printf 'prima\n\nTERZA\n' >riga-vuota.txt
d >"$out/diff-testo.bin"

# Stesse modifiche con una configurazione ostile: gli argomenti di DIFF_ARGS la neutralizzano
# (tranne `diff.suppressBlankEmpty`, che scrive le righe di contesto vuote senza lo spazio).
cat >"$dir/ostile.gitconfig" <<'CFG'
[diff]
	noprefix = true
	mnemonicPrefix = true
	external = false
	renames = false
	relative = true
	submodule = log
	suppressBlankEmpty = true
	context = 0
[color]
	ui = always
	diff = always
[core]
	quotePath = true
CFG
GIT_CONFIG_GLOBAL="$dir/ostile.gitconfig" d >"$out/diff-config.bin"
git add -A
git commit -q -m 'Testo cambiato'

# --- File: nuovi, eliminati, vuoti, cambi di modo, rinomine, cambio di tipo, binari. -----
printf 'contenuto abbastanza lungo da riconoscere la rinomina\n' >rinomina-esatta.txt
printf 'contenuto modificato poco, abbastanza lungo\nriga due\nriga tre\nriga quattro\n' >rinomina-modificata.txt
printf 'diventa eseguibile e cambia nome\n' >rinomina-modo.sh
printf 'da eliminare\nseconda\n' >eliminato.txt
: >vuoto-eliminato.txt
printf '#!/bin/sh\n' >eseguibile.sh
printf 'diventa un link\n' >tipo.txt
printf 'GIF89a\000\001\002\003' >immagine.gif
printf 'BIN\000\001' >binario-eliminato.bin
git add -A
git commit -q -m 'File'

git mv rinomina-esatta.txt rinominata-esatta.txt
git mv rinomina-modificata.txt rinominata-modificata.txt
printf 'riga cinque\n' >>rinominata-modificata.txt
git mv rinomina-modo.sh rinominato-modo.sh
chmod +x rinominato-modo.sh
git rm -q eliminato.txt vuoto-eliminato.txt binario-eliminato.bin
: >vuoto-nuovo.txt
printf 'nuovo\n' >nuovo.txt
chmod +x eseguibile.sh
rm tipo.txt
ln -s nuovo.txt tipo.txt
printf 'GIF89a\000\001\002\004' >immagine.gif
printf 'NEW\000\002' >binario-nuovo.bin
git add -A
d --cached >"$out/diff-file.bin"
git commit -q -m 'File cambiati'

# --- Copie: riconosciute solo con -C. ---------------------------------------------------
cp nuovo.txt copia.txt
git add copia.txt
d --cached -C --find-copies-harder >"$out/diff-copie.bin"
git commit -q -m 'Copia'

# --- Nomi: spazi, non UTF-8, a capo, tab, virgolette, backslash. ------------------------
printf 'spazio\n' >'con spazio.txt'
printf 'latin\n' >"$(printf 'caff\377.txt')"
printf 'utf8\n' >"$(printf 'perch\303\251.txt')"
printf 'a capo\n' >"$(printf 'a\ncapo.txt')"
printf 'tab\n' >"$(printf 'con\ttab.txt')"
printf 'virgolette\n' >'con "virgolette".txt'
printf 'backslash\n' >'con\back.txt'
printf 'da rinominare con un nome strano\n' >semplice.txt
git add -A
git commit -q -m 'Nomi'
for f in 'con spazio.txt' "$(printf 'caff\377.txt')" "$(printf 'perch\303\251.txt')" \
    "$(printf 'a\ncapo.txt')" "$(printf 'con\ttab.txt')" 'con "virgolette".txt' 'con\back.txt'; do
    printf 'modificato\n' >>"$f"
done
git mv semplice.txt "$(printf 'strano "nome"\tcon spazio.txt')"
git add -A
d --cached >"$out/diff-nomi.bin"
# core.quotePath predefinito (vero): i byte non ASCII diventano escape ottali.
git diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3 --cached >"$out/diff-nomi-quotepath.bin"
git commit -q -m 'Nomi cambiati'

# --- Sottomodulo: aggiunto, poi spostato su un altro commit (con `diff.submodule=log`, che
# `--submodule=short` neutralizza). ---------------------------------------------------
git init -q -b main "$dir/sub"
git -C "$dir/sub" commit -q --allow-empty -m 'Uno'
git -c protocol.file.allow=always submodule --quiet add "$dir/sub" sub
# URL relativo al posto della cartella temporanea, per un output riproducibile.
git config -f .gitmodules submodule.sub.url ../sub
git add .gitmodules
d --cached >"$out/diff-sottomodulo-nuovo.bin"
git commit -q -m 'Sottomodulo'
git -C "$dir/sub" commit -q --allow-empty -m 'Due'
git -C sub -c protocol.file.allow=always pull -q
git -c diff.submodule=log diff --no-color --no-ext-diff --no-textconv --find-renames --full-index --src-prefix=a/ --dst-prefix=b/ --submodule=short --no-relative --unified=3 >"$out/diff-sottomodulo.bin"
git add sub
git commit -q -m 'Sottomodulo aggiornato'

# --- Nessuna modifica: output vuoto. -----------------------------------------------------
d >"$out/diff-vuoto.bin"

# --- Conflitto: nel worktree un diff combinato (`diff --cc`), nell'indice `* Unmerged path`.
git checkout -q -b laterale
printf 'dal branch\n' >conflitto.txt
printf 'laterale\n' >"$(printf 'conflitto\tstrano.txt')"
git add -A
git commit -q -m 'Laterale'
git checkout -q main
printf 'da main\n' >conflitto.txt
printf 'main\n' >"$(printf 'conflitto\tstrano.txt')"
printf 'pulito\n' >>nuovo.txt
git add -A
git commit -q -m 'Principale'
git merge -q laterale >/dev/null 2>&1 || true
printf 'nel worktree\n' >>nuovo.txt
d >"$out/diff-conflitto.bin"
d --cached >"$out/diff-conflitto-indice.bin"
