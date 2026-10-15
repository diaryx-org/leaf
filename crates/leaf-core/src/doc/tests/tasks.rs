use super::*;

#[test]
fn a_task_box_toggles_at_the_caret_and_reads_back() {
    let mut d = doc_with("task_toggle", "- [ ] todo\n- [x] done\n");
    d.caret = 8; // inside "todo"
    assert_eq!(d.task_checked_at_caret(), Some(false));
    d.toggle_task_checked();
    assert_eq!(d.source, "- [x] todo\n- [x] done\n");
    assert_eq!(d.task_checked_at_caret(), Some(true));
    d.toggle_task_checked();
    assert_eq!(d.source, "- [ ] todo\n- [x] done\n");
}

#[test]
fn a_click_toggles_a_box_without_taking_the_caret_with_it() {
    // The whole reason `toggle_task_at` exists apart from the caret form:
    // ticking a box elsewhere must not move the cursor out of what's being
    // typed.
    let mut d = doc_with("task_click", "- [ ] first\n- [ ] second\n");
    d.caret = 8; // inside "first"
    let second = d.source.find("second").unwrap();
    d.toggle_task_at(second);
    assert_eq!(d.source, "- [ ] first\n- [x] second\n");
    assert_eq!(d.caret, 8, "the caret stayed in the first item");
}

#[test]
fn a_paragraph_becomes_a_task_item_in_one_undo_step() {
    let mut d = doc_with("task_para", "first\n\nplain para\n");
    d.caret = d.source.find("para").unwrap();
    d.toggle_task_item();
    assert_eq!(d.source, "first\n\n- [ ] plain para\n");
    assert_eq!(d.task_checked_at_caret(), Some(false));
    assert_eq!(d.status, None);
    d.undo();
    assert_eq!(
        d.source, "first\n\nplain para\n",
        "one step takes both back"
    );
}

#[test]
fn a_blank_line_becomes_an_empty_task_item() {
    let mut d = doc_with("task_blank", "first\n\n");
    d.caret = d.source.len();
    d.toggle_task_item();
    assert_eq!(d.task_checked_at_caret(), Some(false), "{:?}", d.source);
    assert_eq!(d.status, None);
}

#[test]
fn a_plain_item_gains_and_loses_a_box() {
    let mut d = doc_with("task_mint", "- plain\n");
    d.caret = 4;
    assert_eq!(d.task_checked_at_caret(), None);
    d.toggle_task_item();
    assert_eq!(d.source, "- [ ] plain\n");
    assert_eq!(
        d.task_checked_at_caret(),
        Some(false),
        "a new box arrives unticked"
    );
    d.toggle_task_item();
    assert_eq!(d.source, "- plain\n");
}

#[test]
fn ticking_a_box_that_isnt_there_reports_rather_than_minting_one() {
    // `set checked` must not silently convert a bullet into a task — that is
    // `toggle_task_item`'s job, and twig refuses it here.
    let mut d = doc_with("task_none", "- plain\n");
    d.caret = 4;
    d.toggle_task_checked();
    assert_eq!(d.source, "- plain\n", "nothing written");
    assert!(
        d.status.is_some(),
        "the refusal should reach the status line"
    );
}

#[test]
fn a_task_item_in_a_quote_is_found_past_the_quote_marker() {
    let mut d = doc_with("task_quote", "> - [ ] nested\n");
    d.caret = d.source.find("nested").unwrap();
    assert_eq!(d.task_checked_at_caret(), Some(false));
    d.toggle_task_checked();
    assert_eq!(d.source, "> - [x] nested\n");
}
