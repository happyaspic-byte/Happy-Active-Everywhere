use everywhere::{
    model::{Content, Versions, validate_path},
    versions::Relation,
};

fn file(letter: char) -> Content {
    Content::File(letter.to_string().repeat(64))
}

#[test]
fn three_offline_edits_converge_in_every_delivery_order() {
    let initial = Versions::default().edit("a", file('a')).unwrap();
    let a = initial.edit("a", file('b')).unwrap();
    let b = initial.edit("b", file('c')).unwrap();
    let c = initial.edit("c", file('d')).unwrap();
    let expected = a.join(&b).unwrap().join(&c).unwrap();
    assert_eq!(expected.heads.len(), 3);
    for inputs in [
        [&a, &b, &c],
        [&a, &c, &b],
        [&b, &a, &c],
        [&b, &c, &a],
        [&c, &a, &b],
        [&c, &b, &a],
    ] {
        let got = inputs
            .into_iter()
            .try_fold(Versions::default(), |v, next| v.join(next))
            .unwrap();
        assert_eq!(got, expected);
        assert_eq!(got.join(&got).unwrap(), got);
    }
}

#[test]
fn delete_and_edit_keep_the_edit_and_converge_after_resolution() {
    let initial = Versions::default().edit("a", file('a')).unwrap();
    let deleted = initial.edit("a", Content::Deleted).unwrap();
    let edited = initial.edit("b", file('b')).unwrap();
    let conflicted = deleted.join(&edited).unwrap();
    assert_eq!(conflicted.heads.len(), 2);
    assert_eq!(conflicted.selected().unwrap().content, file('b'));
    let resolved = conflicted.edit("b", file('b')).unwrap();
    assert_eq!(resolved.heads.len(), 1);
    for previous in &conflicted.heads {
        assert_eq!(
            resolved.heads[0].clock.relation(&previous.clock),
            Relation::After
        );
    }
    assert_eq!(resolved.join(&initial).unwrap(), resolved);
    assert_eq!(resolved.join(&conflicted).unwrap(), resolved);
}

#[test]
fn equal_clock_with_different_payload_is_rejected() {
    let original = Versions::default().edit("a", file('a')).unwrap();
    let mut invalid = original.clone();
    invalid.heads[0].content = file('b');
    assert!(original.join(&invalid).is_err());
}

#[test]
fn tombstone_prevents_an_offline_device_resurrecting_stale_content() {
    let original = Versions::default().edit("a", file('a')).unwrap();
    let deleted = original.edit("b", Content::Deleted).unwrap();
    assert_eq!(deleted.join(&original).unwrap(), deleted);
    assert_eq!(original.join(&deleted).unwrap(), deleted);
}

#[test]
fn portable_paths_accept_unicode_and_reject_escape_and_alias_names() {
    for path in ["한글 문서/😀.txt", "empty", "folder/nested"] {
        validate_path(path).unwrap();
    }
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "a//b",
        "C:/file",
        "a\\b",
        "a/NUL.txt",
        "a/COM1",
        "a.",
        "a ",
        ".everywhere-folder",
        "a/.everywhere-objects/x",
    ] {
        assert!(validate_path(path).is_err(), "accepted {path:?}");
    }
}
