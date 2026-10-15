use super::*;

#[test]
fn word_motion_walks_word_by_word() {
    let g = |m, f: fn(&mut Doc)| golden("word_motion", m, f);
    assert_eq!(
        g("hello wor|ld", |d| d.move_word_left(false)),
        "hello |world"
    );
    assert_eq!(
        g("hello| world", |d| d.move_word_left(false)),
        "|hello world"
    );
    assert_eq!(
        g("hel|lo world", |d| d.move_word_right(false)),
        "hello| world"
    );
    assert_eq!(
        g("hello| world", |d| d.move_word_right(false)),
        "hello world|"
    );
    // Punctuation is its own class, so motion stops at the boundary.
    assert_eq!(g("|foo.bar", |d| d.move_word_right(false)), "foo|.bar");
}

#[test]
fn word_motion_extends_the_selection_when_asked() {
    assert_eq!(
        golden("word_sel", "hello |world", |d| d.move_word_right(true)),
        "hello [world|]"
    );
}

#[test]
fn delete_word_removes_a_whole_word() {
    let g = |m, f: fn(&mut Doc)| golden("del_word", m, f);
    assert_eq!(g("hello world|", |d| d.delete_word_back()), "hello |");
    assert_eq!(g("hello |world", |d| d.delete_word_forward()), "hello |");
    assert_eq!(g("foo |bar baz", |d| d.delete_word_back()), "|bar baz");
}
