use super::*;

fn oid(n: u8) -> Oid {
    Oid::from_bytes([n; 20])
}

fn step(action: RebaseAction, n: u8, message: Option<&str>) -> RebaseStep {
    RebaseStep {
        action,
        commit: oid(n),
        message: message.map(str::to_owned),
    }
}

fn plan(steps: Vec<RebaseStep>) -> RebasePlan {
    RebasePlan {
        base: oid(0),
        steps,
    }
}

/// The range `base..HEAD`: commits 1..=4, oldest first.
fn commits() -> Vec<Oid> {
    (1..=4).map(oid).collect()
}

use RebaseAction::{Drop, Fixup, Pick, Reword, Squash};

fn check(steps: Vec<RebaseStep>) -> Result<(), PlanError> {
    plan(steps).validate(&commits())
}

fn fails(steps: Vec<RebaseStep>, step: Option<usize>, reason: PlanErrorReason) {
    assert_eq!(check(steps), Err(PlanError { step, reason }));
}

#[test]
fn validate_accepts_default_and_mixed_plans() {
    assert_eq!(
        RebasePlan::pick_all(oid(0), &commits()).validate(&commits()),
        Ok(())
    );
    check(vec![
        step(Reword, 2, Some("new")),
        step(Pick, 1, None),
        step(Drop, 3, None),
        step(Squash, 4, Some("combined")),
    ])
    .unwrap();
    // A drop at the start: the first kept step is the pick.
    check(vec![
        step(Drop, 1, None),
        step(Pick, 2, None),
        step(Fixup, 3, None),
        step(Squash, 4, None),
    ])
    .unwrap();
}

#[test]
fn validate_rejects_commits_outside_the_range_duplicates_and_missing() {
    fails(
        vec![step(Pick, 1, None), step(Pick, 9, None)],
        Some(1),
        PlanErrorReason::UnknownCommit(oid(9)),
    );
    fails(
        vec![
            step(Pick, 1, None),
            step(Pick, 2, None),
            step(Drop, 1, None),
        ],
        Some(2),
        PlanErrorReason::DuplicateCommit(oid(1)),
    );
    fails(
        vec![
            step(Pick, 1, None),
            step(Pick, 2, None),
            step(Pick, 4, None),
        ],
        None,
        PlanErrorReason::MissingCommit(oid(3)),
    );
}

#[test]
fn validate_rejects_all_dropped() {
    fails(
        (1..=4).map(|n| step(Drop, n, None)).collect(),
        None,
        PlanErrorReason::AllDropped,
    );
}

#[test]
fn validate_rejects_squash_or_fixup_with_nothing_before() {
    for action in [Squash, Fixup] {
        fails(
            vec![
                step(Drop, 1, None),
                step(action, 2, None),
                step(Pick, 3, None),
                step(Pick, 4, None),
            ],
            Some(1),
            PlanErrorReason::NothingToCombine,
        );
    }
}

#[test]
fn validate_message_rules() {
    let rest = || vec![step(Pick, 3, None), step(Pick, 4, None)];
    let with = |mut head: Vec<RebaseStep>| {
        head.extend(rest());
        head
    };
    // Reword without a message.
    fails(
        with(vec![step(Reword, 1, None), step(Pick, 2, None)]),
        Some(0),
        PlanErrorReason::MissingMessage,
    );
    // Message on a pick and on a drop.
    fails(
        with(vec![step(Pick, 1, Some("x")), step(Pick, 2, None)]),
        Some(0),
        PlanErrorReason::UnexpectedMessage,
    );
    fails(
        with(vec![step(Pick, 1, None), step(Drop, 2, Some("x"))]),
        Some(1),
        PlanErrorReason::UnexpectedMessage,
    );
    // Message on a squash that is not the last of its group (a drop in between does not end it).
    fails(
        vec![
            step(Pick, 1, None),
            step(Squash, 2, Some("x")),
            step(Drop, 3, None),
            step(Fixup, 4, None),
        ],
        Some(1),
        PlanErrorReason::UnexpectedMessage,
    );
    // Reword starting a group.
    fails(
        with(vec![step(Reword, 1, Some("x")), step(Fixup, 2, None)]),
        Some(0),
        PlanErrorReason::RewordInGroup,
    );
    // Blank and NUL.
    fails(
        with(vec![step(Reword, 1, Some(" \n\t")), step(Pick, 2, None)]),
        Some(0),
        PlanErrorReason::EmptyMessage,
    );
    fails(
        with(vec![step(Pick, 1, None), step(Fixup, 2, Some("a\0b"))]),
        Some(1),
        PlanErrorReason::NulInMessage,
    );
}

#[test]
fn error_display_names_the_step() {
    let e = PlanError {
        step: Some(1),
        reason: PlanErrorReason::MissingMessage,
    };
    assert_eq!(e.to_string(), "step 2: `reword` needs a message");
    let e = PlanError {
        step: None,
        reason: PlanErrorReason::AllDropped,
    };
    assert_eq!(e.to_string(), "every commit is dropped");
}

#[test]
fn is_noop_only_for_all_picks_in_order() {
    let c = commits();
    assert!(RebasePlan::pick_all(oid(0), &c).is_noop(&c));
    let mut reordered = RebasePlan::pick_all(oid(0), &c);
    reordered.steps.swap(0, 1);
    assert!(!reordered.is_noop(&c));
    let mut reworded = RebasePlan::pick_all(oid(0), &c);
    reworded.steps[2] = step(Reword, 3, Some("x"));
    assert!(!reworded.is_noop(&c));
    let mut dropped = RebasePlan::pick_all(oid(0), &c);
    dropped.steps[3].action = Drop;
    assert!(!dropped.is_noop(&c));
    assert!(!RebasePlan::pick_all(oid(0), &c[..3]).is_noop(&c));
}

fn rebased(sources: &[u8], message: Option<&str>) -> RebasedCommit {
    RebasedCommit {
        sources: sources.iter().map(|&n| oid(n)).collect(),
        message: message.map(str::to_owned),
    }
}

#[test]
fn result_of_pick_reword_and_drop() {
    let p = plan(vec![
        step(Pick, 1, None),
        step(Drop, 2, None),
        step(Reword, 3, Some("new")),
        step(Pick, 4, None),
    ]);
    assert_eq!(
        p.result(),
        vec![
            rebased(&[1], None),
            rebased(&[3], Some("new")),
            rebased(&[4], None)
        ]
    );
}

#[test]
fn result_of_groups() {
    // Squash group without a message (git's combination), fixup-only group (first message).
    let p = plan(vec![
        step(Pick, 1, None),
        step(Squash, 2, None),
        step(Pick, 3, None),
        step(Fixup, 4, None),
    ]);
    assert_eq!(
        p.result(),
        vec![rebased(&[1, 2], None), rebased(&[3, 4], None)]
    );
    // Group with a message on its last step, a drop inside the group.
    let p = plan(vec![
        step(Pick, 1, None),
        step(Fixup, 2, None),
        step(Drop, 3, None),
        step(Squash, 4, Some("combined")),
    ]);
    assert_eq!(p.result(), vec![rebased(&[1, 2, 4], Some("combined"))]);
}

#[test]
fn todo_text_of_a_mixed_plan() {
    let p = plan(vec![
        step(Reword, 2, Some("reworded")),
        step(Pick, 1, None),
        step(Fixup, 3, None),
        step(Drop, 5, None),
        step(Squash, 4, Some("combined")),
        step(Pick, 6, None),
        step(Fixup, 7, None),
    ]);
    let todo = p.todo();
    let h = |n: u8| oid(n).to_string();
    assert_eq!(
        todo.text,
        format!(
            "pick {}\nbreak\npick {}\nfixup {}\nsquash {}\nbreak\npick {}\nfixup {}\ndrop {}\n",
            h(2),
            h(1),
            h(3),
            h(4),
            h(6),
            h(7),
            h(5)
        )
    );
    assert_eq!(todo.messages, vec!["reworded", "combined"]);
    assert_eq!(h(2).len(), 40);
}

#[test]
fn serde_round_trip_and_shape() {
    let p = plan(vec![step(Pick, 1, None), step(Squash, 2, Some("m"))]);
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["steps"][0]["action"], "pick");
    assert!(json["steps"][0].get("message").is_none());
    assert_eq!(json["steps"][1]["action"], "squash");
    assert_eq!(json["steps"][1]["message"], "m");
    let back: RebasePlan = serde_json::from_value(json).unwrap();
    assert_eq!(back, p);

    let unknown = serde_json::json!({"action": "pick", "commit": oid(1).to_string(), "x": 1});
    assert!(serde_json::from_value::<RebaseStep>(unknown).is_err());
    let bad_action = serde_json::json!({"action": "edit", "commit": oid(1).to_string()});
    assert!(serde_json::from_value::<RebaseStep>(bad_action).is_err());
}
