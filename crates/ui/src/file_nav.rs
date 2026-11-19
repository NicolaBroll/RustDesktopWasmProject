//! Walking a file list with the keyboard (U64): while a file of the stage area, of a commit or
//! of a comparison is open, the arrows move to the file before or after it and Home/End to the
//! first or last one; the diff follows. Data only, no egui: which file comes next.
//!
//! - The list is the one the right panel shows ([`RepoView::shown_files`]): in the stage area
//!   "Staged" then "Unstaged", as drawn. The files in conflict are skipped: they open the
//!   conflict editor, not a diff.
//! - At the ends the selection stays (no wrap around).

use crate::RepoView;
use crate::details::Shown;
use crate::diff::{DiffSource, DiffTarget};

/// Where a key takes the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Previous,
    Next,
    First,
    Last,
}

/// The file `step` leads to from the open one; `None` when the selection stays where it is (an
/// end of the list, or no files). From a file no longer in the list (just staged, say) the next
/// one is the first and the previous one the last.
pub(crate) fn step(files: &[DiffTarget], open: &DiffTarget, step: Step) -> Option<DiffTarget> {
    let last = files.len().checked_sub(1)?;
    let current = files.iter().position(|file| file == open);
    let index = match (step, current) {
        (Step::First, _) | (Step::Next, None) => 0,
        (Step::Last, _) | (Step::Previous, None) => last,
        (Step::Next, Some(i)) => (i + 1).min(last),
        (Step::Previous, Some(i)) => i.saturating_sub(1),
    };
    (current != Some(index)).then(|| files[index].clone())
}

impl RepoView {
    /// The files of the list the right panel shows now, each as the diff a click on it opens:
    /// the stage area with the changes row selected, else the open comparison, else the
    /// selected commit or stash once its details are loaded.
    pub(crate) fn shown_files(&self) -> Vec<DiffTarget> {
        if self.worktree_selected {
            let side = |source, items: &[crate::stage::StageItem]| {
                items
                    .iter()
                    .filter(|item| !item.is_conflict())
                    .map(|item| DiffTarget {
                        source,
                        path: item.path.clone(),
                    })
                    .collect::<Vec<_>>()
            };
            let mut files = side(DiffSource::Index, &self.stage.staged);
            files.extend(side(DiffSource::Worktree, &self.stage.unstaged));
            return files;
        }
        let (source, files) = if self.stash.selected().is_none()
            && let Some(comparison) = self.compare.view()
        {
            match &comparison.answer {
                Some(Ok(compared)) => (compared.source(comparison.merge_base), &compared.files),
                _ => return Vec::new(),
            }
        } else if let Shown::Loaded(details) = self.details.shown() {
            (DiffSource::Commit(details.commit.id), &details.files)
        } else {
            return Vec::new();
        };
        files
            .iter()
            .map(|file| DiffTarget {
                source,
                path: file.path.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::RepoPath;

    fn file(source: DiffSource, path: &str) -> DiffTarget {
        DiffTarget {
            source,
            path: RepoPath::from(path),
        }
    }

    /// Two staged files, then two unstaged ones ("b" is in both lists).
    fn lists() -> Vec<DiffTarget> {
        vec![
            file(DiffSource::Index, "a"),
            file(DiffSource::Index, "b"),
            file(DiffSource::Worktree, "b"),
            file(DiffSource::Worktree, "c"),
        ]
    }

    #[test]
    fn next_and_previous_cross_the_two_lists() {
        let files = lists();
        assert_eq!(step(&files, &files[0], Step::Next), Some(files[1].clone()));
        // From the last staged file to the first unstaged one, the same path in the other list.
        assert_eq!(step(&files, &files[1], Step::Next), Some(files[2].clone()));
        assert_eq!(
            step(&files, &files[2], Step::Previous),
            Some(files[1].clone())
        );
        assert_eq!(
            step(&files, &files[3], Step::Previous),
            Some(files[2].clone())
        );
    }

    #[test]
    fn the_selection_stays_at_the_ends() {
        let files = lists();
        assert_eq!(step(&files, &files[0], Step::Previous), None);
        assert_eq!(step(&files, &files[3], Step::Next), None);
        assert_eq!(step(&files, &files[0], Step::First), None);
        assert_eq!(step(&files, &files[3], Step::Last), None);
        assert_eq!(step(&[], &files[0], Step::Next), None);
    }

    #[test]
    fn home_and_end_go_to_the_first_and_last_file() {
        let files = lists();
        assert_eq!(step(&files, &files[2], Step::First), Some(files[0].clone()));
        assert_eq!(step(&files, &files[1], Step::Last), Some(files[3].clone()));
    }

    #[test]
    fn from_a_file_gone_from_the_list_the_walk_starts_at_an_end() {
        let files = lists();
        let gone = file(DiffSource::Worktree, "gone");
        assert_eq!(step(&files, &gone, Step::Next), Some(files[0].clone()));
        assert_eq!(step(&files, &gone, Step::Previous), Some(files[3].clone()));
    }
}
