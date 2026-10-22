#!/bin/sh
# Rigenera i file status-*.bin (usati da status_tests.rs) in repo temporanei.
# Uso: sh status.sh <cartella-di-uscita>
#
# Ogni file è l'output di `git status --porcelain=v2 -z --branch --untracked-files=all`
# (gli argomenti di STATUS_ARGS); `status-ignorati.bin` aggiunge `--ignored` e
# `status-copie.bin` usa `-c status.renames=copies`.
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Autrice' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

status() {
    git status --porcelain=v2 -z --branch --untracked-files=all "$@"
}

# --- Repo vuoto: nessun commit, poi un file in stage e uno non tracciato. ---------------
git init -q -b main vuoto
(
    cd vuoto
    status >"$out/status-vuoto.bin"
    echo nuovo >nuovo.txt
    git add nuovo.txt
    echo libero >libero.txt
    status >"$out/status-vuoto-file.bin"
)

# --- Sottomodulo usato dal repo misto. -----------------------------------------------------
git init -q -b main sub
(
    cd sub
    echo sub >sub.txt
    git add sub.txt
    git commit -q -m 'Sottomodulo'
)

# --- Repo misto: upstream con ahead/behind e ogni tipo di modifica. ------------------------
git init -q -b main misto
(
    cd misto
    for f in modificato.txt in-stage.txt entrambi.txt cancellato.txt cancellato-wt.txt \
        vecchio-nome.txt tipo.txt eseguibile.sh 'con spazio.txt'; do
        printf 'riga 1 di %s\nriga 2\nriga 3\nriga 4\n' "$f" >"$f"
    done
    printf 'contenuto latin1\n' >"$(printf 'vecchio-\351.txt')"
    git add .
    git -c protocol.file.allow=always submodule add -q ../sub modulo
    git commit -q -m 'Base'
    base=$(git rev-parse HEAD)

    # Upstream: origin/main ha un commit che main non ha (behind 1); main ne ha due (ahead 2).
    git config remote.origin.url ../origin.git
    git config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
    git config branch.main.remote origin
    git config branch.main.merge refs/heads/main
    git checkout -q -b remoto
    git commit -q --allow-empty -m 'Solo sul remoto'
    git update-ref refs/remotes/origin/main "$(git rev-parse HEAD)"
    git checkout -q main
    git branch -q -D remoto
    git commit -q --allow-empty -m 'Locale 1'
    git commit -q --allow-empty -m 'Locale 2'

    echo modifica >>modificato.txt
    echo modifica >>in-stage.txt
    git add in-stage.txt
    echo modifica >>entrambi.txt
    git add entrambi.txt
    echo ancora >>entrambi.txt
    git rm -q cancellato.txt
    rm cancellato-wt.txt
    git mv vecchio-nome.txt nuovo-nome.txt
    git mv "$(printf 'vecchio-\351.txt')" "$(printf 'nuovo-\351.txt')"
    echo modifica >>'con spazio.txt'
    rm tipo.txt
    ln -s modificato.txt tipo.txt
    chmod +x eseguibile.sh
    echo aggiunto >aggiunto.txt
    git add aggiunto.txt
    echo intento >intento.txt
    git add -N intento.txt
    mkdir -p cartella/interna
    echo a >cartella/interna/profondo.txt
    echo b >"$(printf 'non-utf8-\377.txt')"
    printf 'nome\ncon a capo' >"$(printf 'a\ncapo.txt')"
    # Sottomodulo con un file modificato e uno non tracciato.
    echo modifica >>modulo/sub.txt
    echo x >modulo/libero.txt

    status >"$out/status-misto.bin"

)

# --- Upstream sparito: configurato ma senza il ref remoto, quindi niente `branch.ab`. ------
git init -q -b main sparito
(
    cd sparito
    git commit -q --allow-empty -m 'Base'
    git config remote.origin.url ../origin.git
    git config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
    git config branch.main.remote origin
    git config branch.main.merge refs/heads/main
    status >"$out/status-upstream-sparito.bin"
)

# --- Ignorati e copie. ---------------------------------------------------------------------
git init -q -b main ignorati
(
    cd ignorati
    printf '*.log\nbuild/\n' >.gitignore
    printf 'una riga abbastanza lunga da essere riconosciuta come copia\n' >originale.txt
    git add .
    git commit -q -m 'Base'
    echo log >debug.log
    mkdir build
    echo o >build/out.o
    echo libero >libero.txt
    status --ignored >"$out/status-ignorati.bin"
    cp originale.txt copia.txt
    echo modifica >>originale.txt
    git add .
    git -c status.renames=copies status --porcelain=v2 -z --branch --untracked-files=all \
        >"$out/status-copie.bin"
)

# --- Conflitti di merge su main senza upstream. --------------------------------------------
git init -q -b main conflitto
(
    cd conflitto
    for f in entrambi-modificati.txt cancellato-da-noi.txt cancellato-da-loro.txt; do
        printf 'base di %s\n' "$f" >"$f"
    done
    git add .
    git commit -q -m 'Base'
    git checkout -q -b laterale
    echo laterale >entrambi-modificati.txt
    echo laterale >cancellato-da-noi.txt
    git rm -q cancellato-da-loro.txt
    echo laterale >entrambi-aggiunti.txt
    git add .
    git commit -q -m 'Laterale'
    git checkout -q main
    echo main >entrambi-modificati.txt
    git rm -q cancellato-da-noi.txt
    echo main >cancellato-da-loro.txt
    echo main >entrambi-aggiunti.txt
    git add .
    git commit -q -m 'Main'
    git merge -q laterale >/dev/null 2>&1 || true
    echo pulito >pulito.txt
    status >"$out/status-conflitto.bin"
)

# --- HEAD staccata con un file modificato. -------------------------------------------------
git init -q -b main staccata
(
    cd staccata
    echo uno >file.txt
    git add file.txt
    git commit -q -m 'Uno'
    echo due >file.txt
    git commit -q -am 'Due'
    git checkout -q --detach HEAD~1
    echo modifica >file.txt
    status >"$out/status-staccata.bin"
)
