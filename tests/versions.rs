use everywhere::versions::{Clock, Decision, Mode, Relation, reconcile};
#[test]
fn vector_clock_tracks_causality_without_wall_time() {
    let a = Clock::default().advance("a").unwrap();
    let b = Clock::default().advance("b").unwrap();
    assert_eq!(a.relation(&b), Relation::Concurrent);
    let merged = a.merge(&b).advance("a").unwrap();
    assert_eq!(merged.relation(&a), Relation::After);
    assert_eq!(a.relation(&merged), Relation::Before);
    assert_eq!(a.relation(&a), Relation::Equal);
}
#[test]
fn modes_and_delete_modify_conflict_are_explicit() {
    let old = Clock::default().advance("a").unwrap();
    let left = old.advance("a").unwrap();
    let right = old.advance("b").unwrap();
    assert_eq!(
        reconcile(
            Mode::Bidirectional,
            Some((&left, Some("left"))),
            (&right, Some("right"))
        ),
        Decision::Conflict
    );
    assert_eq!(
        reconcile(
            Mode::Bidirectional,
            Some((&left, None)),
            (&right, Some("right"))
        ),
        Decision::Conflict
    );
    assert_eq!(
        reconcile(
            Mode::Bidirectional,
            Some((&old, Some("old"))),
            (&left, None)
        ),
        Decision::Apply
    );
    assert_eq!(
        reconcile(
            Mode::SendOnly,
            Some((&old, Some("old"))),
            (&left, Some("new"))
        ),
        Decision::Ignore
    );
    assert_eq!(
        reconcile(
            Mode::ReceiveOnly,
            Some((&left, Some("local edit"))),
            (&right, Some("remote"))
        ),
        Decision::Conflict
    );
    assert_eq!(
        reconcile(Mode::ReceiveOnly, None, (&right, Some("new"))),
        Decision::Apply
    );
    assert_eq!(
        reconcile(
            Mode::Bidirectional,
            Some((&left, Some("same"))),
            (&right, Some("same"))
        ),
        Decision::Merge
    );
}
#[test]
fn equal_clock_with_different_content_is_rejected() {
    let clock = Clock::default().advance("a").unwrap();
    assert_eq!(
        reconcile(
            Mode::Bidirectional,
            Some((&clock, Some("one"))),
            (&clock, Some("two"))
        ),
        Decision::Invalid
    );
}
