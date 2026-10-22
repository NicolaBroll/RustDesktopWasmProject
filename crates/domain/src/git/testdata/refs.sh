#!/bin/sh
# Rigenera i file refs-*.bin (usati da refs_tests.rs) in repo temporanei.
# Uso: sh refs.sh <cartella-di-uscita>
#
# Ogni file è l'output di `git for-each-ref` con gli argomenti di REFS_ARGS (formato e pattern
# ripetuti qui sotto in `refs`: se cambiano in refs.rs vanno cambiati anche qui).
set -eu
out=$(cd "$1" && pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cd "$dir"

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C TZ=UTC
export GIT_AUTHOR_NAME='Anna Autrice' GIT_AUTHOR_EMAIL=anna@example.com
export GIT_COMMITTER_NAME='Carlo Committer' GIT_COMMITTER_EMAIL=carlo@example.com
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'

refs() {
    git for-each-ref \
        '--format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00' \
        refs/heads/ refs/remotes/ refs/tags/ refs/stash
}

commit() {
    git commit -q --allow-empty -m "$1"
}

# --- Repo appena creato: nessun ref, output vuoto. -----------------------------------------
git init -q -b main vuoto
(cd vuoto && refs >"$out/refs-vuoto.bin")

# --- Repo con un remoto vero (bare) e ogni tipo di ref. ------------------------------------
git init -q --bare -b main origin.git
git init -q -b main repo
(
    cd repo
    git remote add origin ../origin.git
    echo base >file.txt
    git add file.txt
    commit 'Base'
    # main: in pari con origin/main.
    git push -q -u origin main 2>/dev/null

    # avanti: due commit non ancora inviati (ahead 2).
    git checkout -q -b avanti
    git push -q -u origin avanti 2>/dev/null
    commit 'Avanti 1'
    commit 'Avanti 2'

    # indietro: origin/indietro ha un commit in più (behind 1).
    git checkout -q -b indietro main
    commit 'Indietro 1'
    git push -q -u origin indietro 2>/dev/null
    git reset -q --hard HEAD~1

    # entrambi: divergente (ahead 1, behind 2).
    git checkout -q -b entrambi main
    commit 'Entrambi remoto 1'
    commit 'Entrambi remoto 2'
    git push -q -u origin entrambi 2>/dev/null
    git reset -q --hard main
    commit 'Entrambi locale'

    # sparito: upstream configurato ma cancellato dal remoto (gone).
    git checkout -q -b sparito main
    git push -q -u origin sparito 2>/dev/null
    git push -q origin --delete sparito 2>/dev/null

    # Solo sul remoto: refs/remotes/origin/solo-remoto senza branch locale.
    git checkout -q -b solo-remoto main
    commit 'Solo remoto'
    git push -q origin solo-remoto 2>/dev/null
    git checkout -q main
    git branch -q -D solo-remoto

    # origin/HEAD simbolico verso origin/main.
    git remote set-head origin main >/dev/null

    # Branch senza upstream: con `/`, non ASCII, non UTF-8 (byte 0xff).
    git branch feature/nuova-interfaccia
    git branch "$(printf 'caff\303\250')"
    git branch "$(printf 'latin-\377')"
    # Upstream locale (remote = `.`): l'upstream è refs/heads/main.
    git branch -q --track su-main main
    # Ref simbolico locale: scartato dal parser come origin/HEAD.
    git symbolic-ref refs/heads/alias refs/heads/main

    # Tag: leggero, annotato, annotato di un tag annotato, leggero su un albero.
    git tag v0.1
    git tag -a -m 'Versione 1.0' v1.0 avanti
    git -c advice.nestedTag=false tag -a -m 'Tag del tag' v1.0-doppio v1.0
    git tag albero 'main^{tree}'

    # Note: refs/notes/commits non rientra nei pattern.
    git notes add -m 'una nota' main

    # Stash.
    echo modifica >>file.txt
    git stash -q

    refs >"$out/refs.bin"
)

# --- Upstream con un a capo nel nome (da `branch.<b>.merge` nella config). -----------------
git init -q -b main a-capo
(
    cd a-capo
    commit 'Base'
    git config remote.origin.url ../origin.git
    git config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
    git config branch.main.remote origin
    git config branch.main.merge "$(printf 'refs/heads/riga\nnuova')"
    refs >"$out/refs-upstream-a-capo.bin"
)
