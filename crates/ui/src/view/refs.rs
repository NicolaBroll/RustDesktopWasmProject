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
            (RefKind::LocalBranch, tr!("refs-local-branches")),
            (RefKind::RemoteBranch, tr!("refs-remote-branches")),
            (RefKind::Tag, tr!("refs-tags")),
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
            egui::CollapsingHeader::new(tr!("refs-section", title = &title, n = refs.len()))
                .id_salt(kind_salt(kind))
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

/// L'id dell'intestazione di una sezione: non cambia con la lingua né con il numero di ref.
fn kind_salt(kind: RefKind) -> &'static str {
    match kind {
        RefKind::LocalBranch => "ref-locali",
        RefKind::RemoteBranch => "ref-remoti",
        RefKind::Tag => "ref-tag",
        RefKind::Stash => "ref-stash",
    }
}

pub(super) fn ref_tooltip(info: &RefInfo, is_head: bool) -> String {
    let (name, target) = (&info.name, info.target.to_string());
    match info.kind {
        RefKind::LocalBranch if is_head => {
            tr!("refs-tooltip-head", name = name, target = &target)
        }
        RefKind::LocalBranch => tr!("refs-tooltip-local", name = name, target = &target),
        RefKind::RemoteBranch | RefKind::Tag => {
            tr!("refs-tooltip-other", name = name, target = &target)
        }
        RefKind::Stash => tr!("refs-tooltip-stash", name = name, target = &target),
    }
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
            .add_enabled(
                can_write && !is_head,
                egui::Button::new(tr!("refs-checkout")),
            )
            .on_hover_text(tr!("refs-checkout-hint", name = name))
            .on_disabled_hover_text(if is_head {
                tr!("refs-already-active")
            } else {
                wait()
            });
        if button.clicked() {
            actions.push(Action::Checkout(name.to_owned()));
            ui.close();
        }
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new(tr!("refs-new-branch")))
        .on_hover_text(tr!("refs-new-branch-hint", id = info.target.short()))
        .on_disabled_hover_text(wait());
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
        .add_enabled(can_write, egui::Button::new(tr!("refs-rename")))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::RenameBranch(name.to_owned()));
        ui.close();
    }
    ui.separator();
    let pull_hint = match (&info.upstream, is_head) {
        (None, _) => tr!("refs-pull-no-upstream", name = name),
        (Some(up), true) => tr!(
            "refs-pull-head-hint",
            upstream = up.name.strip_prefix("refs/remotes/").unwrap_or(&up.name)
        ),
        (Some(up), false) => tr!(
            "refs-pull-other-hint",
            upstream = up.name.strip_prefix("refs/remotes/").unwrap_or(&up.name),
            name = name
        ),
    };
    let label = if is_head {
        tr!("refs-pull")
    } else {
        tr!("refs-pull-ff")
    };
    let button = ui
        .add_enabled(
            can_write && info.upstream.is_some(),
            egui::Button::new(label),
        )
        .on_hover_text(&pull_hint)
        .on_disabled_hover_text(if info.upstream.is_some() {
            wait()
        } else {
            pull_hint.clone()
        });
    if button.clicked() {
        actions.push(Action::Pull(Some(name.to_owned()), None));
        ui.close();
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new(tr!("refs-push")))
        .on_hover_text(match &info.upstream {
            Some(up) => tr!(
                "refs-push-hint",
                name = name,
                upstream = up.name.strip_prefix("refs/remotes/").unwrap_or(&up.name)
            ),
            None => tr!("refs-push-first-hint", name = name),
        })
        .on_disabled_hover_text(wait());
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
            egui::Button::new(RichText::new(tr!("refs-delete")).color(color)),
        )
        .on_hover_text(tr!("refs-delete-hint"))
        .on_disabled_hover_text(if is_head {
            tr!("refs-delete-head")
        } else {
            wait()
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
    let active = tr!("refs-current-branch");
    let (into, blocked) = match gate {
        merge::Gate::Into(branch) => (branch.as_str(), None),
        merge::Gate::Blocked(reason) => (active.as_str(), Some(reason.as_str())),
    };
    let items = [
        (
            tr!("refs-merge-item", into = into),
            false,
            tr!("refs-merge-hint", what = what, into = into),
        ),
        (
            tr!("refs-merge-no-ff-item", into = into),
            true,
            tr!("refs-merge-no-ff-hint", what = what, into = into),
        ),
    ];
    for (label, no_ff, hint) in items {
        let button = ui
            .add_enabled(can_write && blocked.is_none(), egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(blocked.map_or_else(wait, str::to_owned));

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
