use super::*;

/// Ultimo componente del percorso, per il titolo.
pub(super) fn repo_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

pub(super) fn head_text(head: &Head) -> String {
    match head {
        Head::Branch { name, target: None } => format!("{name} (senza commit)"),
        Head::Branch { name, .. } => name.clone(),
        Head::Detached(id) => format!("HEAD staccata su {}", id.short()),
    }
}

/// Avanti e indietro rispetto all'upstream (`↑2 ↓1`, `upstream sparito`), con la spiegazione
/// al passaggio del mouse; niente se il branch è allineato.
pub(super) fn upstream_label(ui: &mut Ui, upstream: &Upstream) {
    let (short, long) = remotes::ahead_behind(upstream);
    if short.is_empty() {
        return;
    }
    let text = if upstream.ahead.is_none() {
        RichText::new(short)
            .color(ui.visuals().warn_fg_color)
            .small()
    } else {
        RichText::new(short).weak()
    };
    ui.label(text).on_hover_text(long);
}

/// `writing`: c'è una scrittura in corso, Annulla, Ripeti, Aggiorna, Fetch, Pull e Push
/// aspettano. `network`: il fetch, pull o push in corso con il suo ultimo avanzamento.
pub(crate) fn top_bar(
    ui: &mut Ui,
    repo: Option<&RepoView>,
    can_pick: bool,
    writing: bool,
    network: Option<Network>,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        if can_pick && ui.button("Apri…").clicked() {
            actions.push(Action::PickFolder);
        }
        let Some(repo) = repo else {
            return;
        };
        let button = ui
            .add_enabled(!writing, egui::Button::new("Aggiorna"))
            .on_hover_text("Rilegge ref, stato e log")
            .on_disabled_hover_text("Dopo l'operazione in corso");
        if button.clicked() {
            actions.push(Action::Refresh);
        }
        ui.separator();
        match network {
            Some(network) => network_progress(ui, network, actions),
            None => {
                let enabled = !writing && !repo.modal_open();
                fetch_buttons(ui, repo, enabled, actions);
                sync_buttons(ui, repo, enabled, actions);
                stash_push_button(ui, repo, enabled, "Stash…", actions);
            }
        }
        ui.separator();
        // Con un'operazione in corso (merge, rebase...) il motore rifiuta undo e redo.
        let in_operation = repo.status.as_ref().is_some_and(|s| s.operation.is_some());
        for op in [UndoOp::Undo, UndoOp::Redo] {
            let available = op.available(&repo.history);
            let enabled = available && !in_operation && !writing && !repo.modal_open();
            let button = ui.add_enabled(enabled, egui::Button::new(op.label()));
            let hint = op.hint(&repo.history);
            let button = if available && in_operation {
                button.on_disabled_hover_text(format!(
                    "{hint} (non durante un'operazione in corso: concluderla o annullarla dal \
                     riquadro in alto)"
                ))
            } else if available && !enabled {
                button.on_disabled_hover_text(format!("{hint} (dopo l'operazione in corso)"))
            } else {
                button.on_hover_text(&hint).on_disabled_hover_text(&hint)
            };
            if button.clicked() {
                actions.push(Action::Undo(op));
            }
        }
        ui.separator();
        ui.strong(repo_name(&repo.path)).on_hover_text(&repo.path);
        ui.separator();
        ui.label(head_text(&repo.head));
        if let Some(status) = &repo.status {
            if let Some(upstream) = &status.upstream {
                upstream_label(ui, upstream);
            }
            let changes = status.entries.len();
            if changes > 0 {
                ui.separator();
                ui.label(format!("{changes} file modificati"));
            }
        }
    });
}

/// Il pulsante "Fetch" e, con più remoti, il menu per sceglierne uno.
pub(super) fn fetch_buttons(
    ui: &mut Ui,
    repo: &RepoView,
    enabled: bool,
    actions: &mut Vec<Action>,
) {
    let button = ui
        .add_enabled(enabled, egui::Button::new("Fetch"))
        .on_hover_text(
            "Scarica commit, branch remoti e tag dal remoto dell'upstream del branch attivo \
             (altrimenti origin, o l'unico remoto). Non tocca branch locali, indice né file",
        )
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::Fetch(None));
    }
    let remotes = repo.remotes.names();
    if remotes.len() > 1 {
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("Da…", |ui| {
                for remote in remotes {
                    if ui.button(format!("Fetch da «{remote}»")).clicked() {
                        actions.push(Action::Fetch(Some(remote)));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Sceglie il remoto da cui fare il fetch");
        });
    }
}

/// I pulsanti "Pull" e "Push" del branch attivo. Mai un push forzato da qui: il force si
/// propone solo nella finestra di un push rifiutato.
pub(super) fn sync_buttons(ui: &mut Ui, repo: &RepoView, enabled: bool, actions: &mut Vec<Action>) {
    let branch = match &repo.head {
        Head::Branch {
            name,
            target: Some(_),
        } => Some(name.as_str()),
        _ => None,
    };
    let upstream = repo
        .status
        .as_ref()
        .and_then(|s| s.upstream.as_ref())
        .map(|u| u.name.as_str());
    let (pull_hint, push_hint) = match (branch, upstream) {
        (None, _) => (
            "HEAD è staccata (o il branch non ha commit): passare a un branch".to_owned(),
            "HEAD è staccata (o il branch non ha commit): passare a un branch".to_owned(),
        ),
        (Some(b), Some(up)) => (
            format!(
                "Scarica {up} e lo integra in «{b}»: fast-forward se può, altrimenti merge o \
                 rebase secondo pull.rebase (senza configurazione: merge). Con modifiche locali \
                 il merge non parte. «Annulla» riporta il branch dov'era"
            ),
            format!(
                "Pubblica «{b}» su {up}. Mai forzato: se il remoto ha commit che non hai il push \
                 è rifiutato e si propone il Pull. Un push non si annulla"
            ),
        ),
        (Some(b), None) => (
            format!(
                "«{b}» non ha un upstream: il pull spiega come impostarlo (oppure fare Push, che \
                 lo imposta)"
            ),
            format!(
                "Primo push di «{b}»: lo pubblica su origin (o sull'unico remoto) con lo stesso \
                 nome e imposta l'upstream. Un push non si annulla"
            ),
        ),
    };
    let enabled = enabled && branch.is_some();
    let pull = ui
        .add_enabled(enabled, egui::Button::new("Pull"))
        .on_hover_text(&pull_hint);
    let pull = if branch.is_some() {
        pull.on_disabled_hover_text(WAIT)
    } else {
        pull.on_disabled_hover_text(&pull_hint)
    };
    if pull.clicked() {
        actions.push(Action::Pull(None, None));
    }
    let push = ui
        .add_enabled(enabled, egui::Button::new("Push"))
        .on_hover_text(&push_hint);
    let push = if branch.is_some() {
        push.on_disabled_hover_text(WAIT)
    } else {
        push.on_disabled_hover_text(&push_hint)
    };
    if push.clicked() {
        actions.push(Action::Push(None));
    }
}

/// Il fetch, pull o push in corso: cosa, fase, barra (se git dà la percentuale) e
/// "Interrompi".
pub(super) fn network_progress(ui: &mut Ui, network: Network, actions: &mut Vec<Action>) {
    ui.spinner();
    ui.strong(&network.what);
    let progress = network.progress.as_ref();
    let (text, fraction) = match progress {
        Some(p) => fetch::describe(p),
        None => ("Connessione al remoto…".to_owned(), None),
    };
    let label = ui.label(text);
    if let Some(p) = progress {
        label.on_hover_text(&p.text);
    }
    if let Some(fraction) = fraction {
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_width(120.0)
                .show_percentage(),
        );
    }
    let stop = ui
        .add_enabled(!network.cancelling, egui::Button::new("Interrompi"))
        .on_hover_text(network.stop_hint)
        .on_disabled_hover_text("Interruzione in corso…");
    if stop.clicked() {
        actions.push(network.cancel);
    }
}

/// `write` è la scrittura in corso, descritta: ha la precedenza sugli altri comandi. `notice`
/// è un messaggio informativo, mostrato se non c'è niente in corso né un errore.
pub(crate) fn status_bar(
    ui: &mut Ui,
    running: &[Job],
    write: Option<&str>,
    error: &str,
    notice: &str,
) {
    ui.horizontal(|ui| {
        if let Some(write) = write {
            ui.spinner();
            ui.label(format!("{write} in corso…"));
        } else if let Some(job) = running.first() {
            ui.spinner();
            let text = match running.len() - 1 {
                0 => format!("{} in corso…", job.command),
                n => format!("{} in corso… (+{n} in coda)", job.command),
            };
            ui.label(text);
        } else if !error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else if !notice.is_empty() {
            ui.label(notice);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::warn_if_debug_build(ui);
        });
    });
}

pub(crate) fn welcome(ui: &mut Ui, can_pick: bool, busy: bool, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 3.0);
        if busy {
            ui.spinner();
            ui.label("Apertura del repository…");
        } else if can_pick {
            ui.heading("Nessun repository aperto");
            ui.add_space(8.0);
            if ui.button("Apri un repository…").clicked() {
                actions.push(Action::PickFolder);
            }
        } else {
            ui.heading("Nessun repository aperto");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_is_the_last_component() {
        assert_eq!(repo_name("/home/ada/progetto"), "progetto");
        assert_eq!(repo_name("/home/ada/progetto/"), "progetto");
        assert_eq!(repo_name(r"C:\repo\app"), "app");
        assert_eq!(repo_name("esempio"), "esempio");
        assert_eq!(repo_name("/"), "/");
    }
}
