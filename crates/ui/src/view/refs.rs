use super::*;

/// `writing`: c'è una scrittura in corso, le voci del menu che scrivono aspettano.
pub(crate) fn refs_panel(ui: &mut Ui, repo: &RepoView, writing: bool, actions: &mut Vec<Action>) {
    let current = match &repo.head {
        Head::Branch { name, .. } => Some(format!("refs/heads/{name}")),
        Head::Detached(_) => None,
    };
    let can_write = !writing && !repo.modal_open();
    let merge = merge::Gate::new(&repo.head, repo.status.as_ref());
    egui::ScrollArea::vertical().show(ui, |ui| {
        let sections = [
            (RefKind::LocalBranch, "Branch locali"),
            (RefKind::RemoteBranch, "Branch remoti"),
            (RefKind::Tag, "Tag"),
        ];
        for (kind, title) in sections {
            // I remoti configurati sotto i loro branch, prima dei tag.
            if kind == RefKind::Tag {
                remotes_section(ui, repo, can_write, actions);
            }
            let refs: Vec<&RefInfo> = repo.refs.iter().filter(|r| r.kind == kind).collect();
            if refs.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(format!("{title} ({})", refs.len()))
                .default_open(kind != RefKind::Tag)
                .show(ui, |ui| {
                    for info in refs {
                        let is_head = current.as_deref() == Some(&info.name);
                        ref_row(ui, info, is_head, can_write, &merge, actions);
                    }
                });
        }
        stash_section(ui, repo, can_write, actions);
    });
}

/// `can_write`: le voci che scrivono sono attive (nessuna scrittura in corso, nessun dialogo).
/// `merge`: in quale branch "Unisci" unirebbe, o perché non si può.
pub(super) fn ref_row(
    ui: &mut Ui,
    info: &RefInfo,
    is_head: bool,
    can_write: bool,
    merge: &merge::Gate,
    actions: &mut Vec<Action>,
) {
    // L'id della riga viene dal nome del ref, non dalla posizione: se i ref si rileggono fra
    // i due clic, un doppio clic non finisce su un altro branch.
    ui.push_id(&info.name, |ui| {
        ref_row_inner(ui, info, is_head, can_write, merge, actions);
    });
}

pub(super) fn ref_row_inner(
    ui: &mut Ui,
    info: &RefInfo,
    is_head: bool,
    can_write: bool,
    merge: &merge::Gate,
    actions: &mut Vec<Action>,
) {
    let local = info.kind == RefKind::LocalBranch;
    ui.horizontal(|ui| {
        // Il branch corrente è evidenziato e in grassetto.
        let name = RichText::new(info.short_name());
        let name = if is_head { name.strong() } else { name };
        let response = ui.selectable_label(is_head, name);
        if response.double_clicked() && local && !is_head && can_write {
            actions.push(Action::Checkout(info.short_name().to_owned()));
        } else if response.clicked() {
            actions.push(Action::SelectCommit(info.target));
        }
        if info.kind != RefKind::Stash {
            response.context_menu(|ui| ref_menu(ui, info, is_head, can_write, merge, actions));
        }
        if let Some(upstream) = &info.upstream {
            upstream_label(ui, upstream);
        }
    })
    .response
    .on_hover_text(ref_tooltip(info, is_head));
}

pub(super) fn ref_tooltip(info: &RefInfo, is_head: bool) -> String {
    let mut text = format!("{}\n{}\nClic: seleziona il commit", info.name, info.target);
    match info.kind {
        RefKind::LocalBranch if is_head => text.push_str("\nClic destro: altre azioni"),
        RefKind::LocalBranch => text.push_str("\nDoppio clic: checkout\nClic destro: altre azioni"),
        RefKind::RemoteBranch | RefKind::Tag => {
            text.push_str("\nClic destro: nuovo branch da qui, unisci nel branch attivo");
        }
        RefKind::Stash => {}
    }
    text
}

/// Il menu contestuale di un ref: sui branch locali checkout, nuovo branch, merge nel branch
/// attivo, rinomina, pull, push ed eliminazione; su branch remoti e tag nuovo branch e merge.
pub(super) fn ref_menu(
    ui: &mut Ui,
    info: &RefInfo,
    is_head: bool,
    can_write: bool,
    merge: &merge::Gate,
    actions: &mut Vec<Action>,
) {
    let name = info.short_name();
    if info.kind == RefKind::LocalBranch {
        let button = ui
            .add_enabled(can_write && !is_head, egui::Button::new("Checkout"))
            .on_hover_text(format!(
                "Passa a «{name}»: le modifiche locali restano, se non toccano file diversi nel \
                 branch (altrimenti niente cambia)"
            ))
            .on_disabled_hover_text(if is_head {
                "È già il branch attivo"
            } else {
                WAIT
            });
        if button.clicked() {
            actions.push(Action::Checkout(name.to_owned()));
            ui.close();
        }
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new("Nuovo branch da qui…"))
        .on_hover_text(format!(
            "Crea un branch su {} senza passarci",
            info.target.short()
        ))
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        let (start, proposed) = Start::from_ref(info);
        actions.push(Action::NewBranch(start, proposed));
        ui.close();
    }
    if !is_head {
        merge_items(ui, name, name, merge, can_write, actions);
    }
    if info.kind != RefKind::LocalBranch {
        return;
    }
    if !is_head {
        ui.separator();
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new("Rinomina…"))
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::RenameBranch(name.to_owned()));
        ui.close();
    }
    ui.separator();
    let pull_hint = match (&info.upstream, is_head) {
        (None, _) => {
            format!("«{name}» non ha un upstream: fare prima Push, che lo imposta al primo push")
        }
        (Some(up), true) => format!(
            "Scarica {} e lo integra: fast-forward, merge o rebase secondo la configurazione",
            up.name.strip_prefix("refs/remotes/").unwrap_or(&up.name)
        ),
        (Some(up), false) => format!(
            "Scarica {} e porta avanti «{name}» senza passarci (solo fast-forward: se è \
             divergente bisogna passarci)",
            up.name.strip_prefix("refs/remotes/").unwrap_or(&up.name)
        ),
    };
    let label = if is_head {
        "Pull"
    } else {
        "Pull (fast-forward)"
    };
    let button = ui
        .add_enabled(
            can_write && info.upstream.is_some(),
            egui::Button::new(label),
        )
        .on_hover_text(&pull_hint)
        .on_disabled_hover_text(if info.upstream.is_some() {
            WAIT
        } else {
            &pull_hint
        });
    if button.clicked() {
        actions.push(Action::Pull(Some(name.to_owned()), None));
        ui.close();
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new("Push"))
        .on_hover_text(match &info.upstream {
            Some(up) => format!(
                "Pubblica «{name}» su {}. Mai forzato",
                up.name.strip_prefix("refs/remotes/").unwrap_or(&up.name)
            ),
            None => format!("Primo push di «{name}»: lo pubblica e imposta l'upstream"),
        })
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::Push(Some(name.to_owned())));
        ui.close();
    }
    ui.separator();
    let enabled = can_write && !is_head;
    let color = if enabled {
        ui.visuals().error_fg_color
    } else {
        ui.visuals().weak_text_color()
    };
    let button = ui
        .add_enabled(
            enabled,
            egui::Button::new(RichText::new("Elimina").color(color)),
        )
        .on_hover_text(
            "Elimina il branch. Se è unito si elimina subito; se ha commit che non sono altrove \
             chiede conferma. In ogni caso il commit resta in un backup \
             (refs/project/backup/delete-…) e «Annulla» ricrea il branch",
        )
        .on_disabled_hover_text(if is_head {
            "È il branch attivo: passare prima a un altro branch per eliminarlo"
        } else {
            WAIT
        });
    if button.clicked() {
        actions.push(Action::DeleteBranch(name.to_owned(), info.target));
        ui.close();
    }
}

/// Le voci "Unisci in «branch»" (fast-forward se basta) e "… con commit di merge" (`no_ff`) per
/// `rev`, mostrato come `what`; spente con il motivo se il branch attivo non lo permette.
pub(super) fn merge_items(
    ui: &mut Ui,
    rev: &str,
    what: &str,
    gate: &merge::Gate,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.separator();
    let (into, blocked) = match gate {
        merge::Gate::Into(branch) => (branch.as_str(), None),
        merge::Gate::Blocked(reason) => ("branch attivo", Some(*reason)),
    };
    let items = [
        (
            format!("Unisci in «{into}»"),
            false,
            format!(
                "Unisce «{what}» in «{into}»: fast-forward se «{into}» non ha commit propri, \
                 altrimenti un commit di merge. Con conflitti il merge resta in corso (si \
                 risolve o si annulla dal riquadro in alto). «Annulla» annulla un merge concluso"
            ),
        ),
        (
            format!("Unisci in «{into}» con commit di merge"),
            true,
            format!(
                "Unisce «{what}» in «{into}» sempre con un commit di merge, anche dove \
                 basterebbe il fast-forward (--no-ff)"
            ),
        ),
    ];
    for (label, no_ff, hint) in items {
        let button = ui
            .add_enabled(can_write && blocked.is_none(), egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(blocked.unwrap_or(WAIT));
        if button.clicked() {
            actions.push(Action::Merge {
                rev: rev.to_owned(),
                what: what.to_owned(),
                no_ff,
            });
            ui.close();
        }
    }
}
