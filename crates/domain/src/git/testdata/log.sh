#!/bin/sh
# Rigenera log.bin e log-vuoto.bin (usati da tests.rs) in un repo temporaneo.
# Uso: sh log.sh <cartella-di-uscita>
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Autrice' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com

# at <secondi> <fuso autore> <fuso committer>
at() {
    export GIT_AUTHOR_DATE="$1 $2" GIT_COMMITTER_DATE="$1 $3"
}

git init -q -b main .

# Commit radice, corpo vuoto, fuso positivo.
at 1700000000 +0200 +0200
git commit -q --allow-empty -m 'Commit radice'

# Corpo su più righe; oggetto su due righe (lo stesso paragrafo); fusi diversi.
at 1700000100 +0530 -0800
git commit -q --allow-empty -F - <<'MSG'
Oggetto su
due righe

Prima riga del corpo.
Seconda riga del corpo.

Secondo paragrafo.
MSG

# Branch laterale con autore non UTF-8 (Latin-1, senza intestazione encoding). `git commit`
# convertirebbe il nome in UTF-8, quindi l'oggetto si scrive a mano con `hash-object`.
git checkout -q -b laterale
{
    printf 'tree %s\n' "$(git write-tree)"
    printf 'parent %s\n' "$(git rev-parse HEAD)"
    printf 'author Jos\351 Latin1 <jose@example.com> 1700000200 -0330\n'
    printf 'committer Carlo Committer <carlo@example.com> 1700000200 +0000\n'
    printf '\nAutore non UTF-8\n'
} | git hash-object -t commit -w --stdin >oid
git update-ref refs/heads/laterale "$(cat oid)"

# Messaggio Latin-1 dichiarato con i18n.commitEncoding: git log lo converte in UTF-8.
at 1700000300 +0100 +0100
printf 'Caff\350 dichiarato Latin-1\n\nCorpo in Latin-1: perch\351.\n' |
    GIT_AUTHOR_NAME="$(printf 'Ren\351e Encoding')" \
    git -c i18n.commitEncoding=ISO-8859-1 commit -q --allow-empty -F -

# Merge con due genitori, corpo vuoto.
git checkout -q main
at 1700000400 -0700 +1245
git merge -q --no-ff --no-edit -m 'Merge laterale' laterale

git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color \
    --format='%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b' \
    >"$out/log.bin"

# Range vuoto: nessun commit.
git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color \
    --format='%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b' \
    HEAD..HEAD >"$out/log-vuoto.bin"
