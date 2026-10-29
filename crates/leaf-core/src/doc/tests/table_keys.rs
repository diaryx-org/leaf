use super::*;

const TABLE: &str = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n| Fig | 12 |\n";

#[test]
fn wysiwyg_right_crosses_a_cell_border_without_stalling() {
    // The border and padding between two cells all share one source offset,
    // so a column-stepping caret would sit on `│` and then stall there
    // forever. Right must step: end of "Name" -> start of "Qty".
    let mut d = wysiwyg_doc("tbl_right", TABLE);
    d.caret = TABLE.find("Name").unwrap() + 4; // just after "Name"
    d.move_right(false);
    assert_eq!(
        d.caret,
        TABLE.find("Qty").unwrap(),
        "should land in the next cell"
    );
    let (r, c) = d.caret_pos();
    assert_eq!(d.vmap.rows[r].glyphs[c].ch, 'Q');
}

#[test]
fn wysiwyg_left_crosses_back_to_the_previous_cell() {
    let mut d = wysiwyg_doc("tbl_left", TABLE);
    d.caret = TABLE.find("Qty").unwrap();
    d.move_left(false);
    assert_eq!(
        d.caret,
        TABLE.find("Name").unwrap() + 4,
        "end of the previous cell"
    );
}

#[test]
fn wysiwyg_down_steps_over_a_table_rule() {
    // Between the header and the first body row sits a `├───┼───┤` rule.
    // It's drawn but holds no caret, so one Down must reach "Pear".
    let mut d = wysiwyg_doc("tbl_down", TABLE);
    d.caret = TABLE.find("Name").unwrap();
    d.move_down(false);
    assert_eq!(
        d.caret,
        TABLE.find("Pear").unwrap(),
        "one Down reaches the body row"
    );
    d.move_down(false);
    assert_eq!(d.caret, TABLE.find("Fig").unwrap());
}

#[test]
fn wysiwyg_tab_walks_the_cells_and_shift_tab_walks_back() {
    let mut d = wysiwyg_doc("tbl_tab", TABLE);
    d.caret = TABLE.find("Name").unwrap();
    // A hop lands with the destination cell's whole content selected, the
    // caret at its end — so typing replaces the cell like a form field.
    assert!(d.cell_hop(true));
    assert_eq!(
        d.selected_text(),
        Some("Qty"),
        "the target cell comes up selected"
    );
    assert_eq!(d.caret, TABLE.find("Qty").unwrap() + "Qty".len());
    assert!(d.cell_hop(true), "Tab wraps onto the next row's first cell");
    assert_eq!(d.selected_text(), Some("Pear"));
    assert!(d.cell_hop(false));
    assert_eq!(d.selected_text(), Some("Qty"));
}

#[test]
fn tab_outside_a_table_is_not_a_cell_hop() {
    // `cell_hop` reports false so the frontend can indent as usual.
    let mut d = wysiwyg_doc("tbl_none", "just a paragraph\n");
    d.caret = 4;
    assert!(!d.cell_hop(true));
    assert_eq!(d.caret, 4, "a refused hop leaves the caret alone");
}

#[test]
fn tab_at_the_last_cell_declines_rather_than_leaving_the_table() {
    let mut d = wysiwyg_doc("tbl_edge", TABLE);
    d.caret = TABLE.rfind("12").unwrap(); // the final cell
    assert!(!d.cell_hop(true), "no cell after the last one");
    d.caret = TABLE.find("Name").unwrap();
    assert!(!d.cell_hop(false), "no cell before the first one");
}

#[test]
fn wysiwyg_vertical_cell_motion_holds_the_column() {
    // Down/Up step to the cell above/below in the *same column*, not back to
    // the top-left the way a naive row/col motion over the picture would.
    let mut d = wysiwyg_doc("tbl_vert", TABLE);
    d.caret = TABLE.find("Qty").unwrap();
    // Each vertical hop selects the destination cell, holding the column.
    assert!(d.cell_move_vertical(true));
    assert_eq!(d.selected_text(), Some("3"), "Down holds column 1");
    assert!(d.cell_move_vertical(true));
    assert_eq!(d.selected_text(), Some("12"), "Down again, still column 1");
    assert!(!d.cell_move_vertical(true), "no row below the last");
    assert!(d.cell_move_vertical(false));
    assert_eq!(d.selected_text(), Some("3"), "Up holds column 1");
    assert!(d.cell_move_vertical(false));
    assert_eq!(d.selected_text(), Some("Qty"), "Up onto the header");
    assert!(!d.cell_move_vertical(false), "no row above the header");
}

#[test]
fn tab_off_the_last_cell_grows_a_row_and_enters_it() {
    let mut d = wysiwyg_doc("tbl_grow", TABLE);
    d.caret = TABLE.rfind("12").unwrap();
    let rows_before = d.source.matches('\n').count();
    assert!(d.cell_tab(true), "acts as a table key");
    assert_eq!(
        d.source.matches('\n').count(),
        rows_before + 1,
        "a fresh row was appended"
    );
    assert!(d.caret_in_table(), "the caret entered the new row");
    // The caret sits in the new row's first cell — past the old last cell.
    assert!(d.caret > TABLE.rfind("12").unwrap());
}

#[test]
fn return_in_a_table_drops_a_cell_and_grows_a_row_at_the_bottom() {
    let mut d = wysiwyg_doc("tbl_ret", TABLE);
    d.caret = TABLE.find("Name").unwrap();
    assert!(d.cell_return(), "acts as a table key");
    assert_eq!(
        d.selected_text(),
        Some("Pear"),
        "Return drops one cell, selecting it"
    );
    // From the last row, Return appends a row and enters it.
    d.caret = TABLE.rfind("Fig").unwrap();
    let rows_before = d.source.matches('\n').count();
    assert!(d.cell_return());
    assert_eq!(d.source.matches('\n').count(), rows_before + 1);
    assert!(d.caret_in_table());
}

#[test]
fn return_and_tab_outside_a_table_decline() {
    let mut d = wysiwyg_doc("tbl_decline", "just a paragraph\n");
    d.caret = 4;
    assert!(!d.cell_return(), "no table: the frontend inserts a newline");
    assert!(!d.cell_tab(true), "no table: the frontend indents");
    assert!(
        !d.cell_line_break(),
        "no table: the frontend breaks the line"
    );
}

#[test]
fn a_click_under_a_trailing_table_lands_past_it_and_enter_opens_a_line() {
    // A document that ends in a table used to end *inside* it: nothing
    // past the last cell was a caret stop, so a click in the blank space
    // under the grid snapped back into the table and there was no way to
    // write a line after it. The bottom border's end is that stop now.
    let mut d = wysiwyg_doc("tbl_trail", TABLE);
    let rows = d.vmap.num_rows();
    d.click(rows + 3, 0, false);
    let end = TABLE.trim_end_matches('\n').len();
    assert_eq!(d.caret, end, "the caret stands just past the table");
    assert!(!d.caret_in_table(), "past the table is outside it");
    assert!(!d.cell_return(), "Return there is the frontend's newline");
    d.newline();
    d.insert("after");
    assert_eq!(
        d.source,
        format!("{TABLE}\nafter\n"),
        "Enter opens a paragraph under the table"
    );
}

#[test]
fn typing_at_a_table_s_trailing_stop_opens_a_paragraph_first() {
    // The stop sits at the end of the table's last source line, and a
    // line glued under a table is a row of it — `| Fig | 12 |x` would be a
    // three-cell row. So the text gets a paragraph of its own, as it does
    // beside a block picture.
    let mut d = wysiwyg_doc("tbl_type", TABLE);
    d.caret = TABLE.trim_end_matches('\n').len();
    d.insert("x");
    assert_eq!(d.source, format!("{TABLE}\nx\n"));
    assert_eq!(d.caret, TABLE.len() + 2, "the caret follows the text");
    // And a paste, which joins the block exactly as typing would.
    let mut d = wysiwyg_doc("tbl_paste", TABLE);
    d.caret = TABLE.trim_end_matches('\n').len();
    d.paste("pasted");
    assert_eq!(d.source, format!("{TABLE}\npasted\n"));
}

#[test]
fn right_leaves_a_table_by_its_trailing_stop_and_backspace_steps_back_in() {
    let mut d = wysiwyg_doc("tbl_edge", TABLE);
    let last_cell_end = TABLE.rfind("12").unwrap() + 2;
    let end = TABLE.trim_end_matches('\n').len();
    d.caret = last_cell_end;
    d.move_right(false);
    assert_eq!(d.caret, end, "Right from the last cell leaves the table");
    // Backspace there takes no byte: the one behind the caret is the row's
    // closing `|`, which the rich view never drew. It steps back instead.
    d.backspace();
    assert_eq!(d.source, TABLE, "nothing deleted");
    assert_eq!(d.caret, last_cell_end, "back into the last cell");
    // Down from the last row lands on the same stop, and Up returns.
    d.move_down(false);
    assert_eq!(d.caret, end, "Down from the last row leaves the table");
    d.move_up(false);
    assert_eq!(d.caret, last_cell_end);
}

#[test]
fn a_table_s_trailing_stop_sits_between_it_and_the_text_below() {
    // With prose under the table, the stop is one hop between the last
    // cell and the paragraph — the shape a block picture's second stop has.
    let src = format!("{TABLE}\nafter\n");
    let mut d = wysiwyg_doc("tbl_mid", &src);
    d.caret = TABLE.rfind("12").unwrap() + 2;
    d.move_right(false);
    assert_eq!(d.caret, TABLE.trim_end_matches('\n').len());
    d.move_right(false);
    assert_eq!(d.caret, src.find("after").unwrap());
    // Typing at the stop still opens a paragraph, and the text below keeps
    // its own.
    d.move_left(false);
    d.insert("x");
    assert_eq!(d.source, format!("{TABLE}\nx\n\nafter\n"));
}

#[test]
fn shift_return_inserts_an_in_cell_break_the_renderer_reads_as_a_line() {
    let mut d = wysiwyg_doc("tbl_break", TABLE);
    d.caret = TABLE.find("Pear").unwrap() + 4; // just after "Pear"
    assert!(d.cell_line_break(), "acts as a table key");
    assert!(
        d.source.contains("Pear<br>"),
        "spelled as an inline <br>: {}",
        d.source
    );
    assert!(d.caret_in_table(), "still in the cell, past the break");
    // The break renders as a real line: the "Pear" cell now draws two lines,
    // so the table's picture is one row taller than a single-line table.
    d.build_visual(80);
    let table = &d.vmap.tables[0];
    let cell = &table.grid[1].cells[0]; // first body row, first column
    assert!(
        cell.glyphs.iter().any(|g| g.ch == '\n'),
        "the cell carries the break as a newline glyph for the frontend to split"
    );
}

#[test]
fn shift_return_in_a_markdown_cell_leaves_a_semantic_hard_break_not_raw_html() {
    // twig promotes the in-cell `<br>` to a `hard_break`, so the break reads
    // back as structure — the whole point of routing through insert_line_break
    // instead of splicing raw `<br>` bytes.
    let mut d = wysiwyg_doc("tbl_break_semantic", TABLE);
    d.caret = TABLE.find("Pear").unwrap() + 4;
    assert!(d.cell_line_break());
    let kinds: Vec<Kind> = d
        .editor
        .nodes()
        .unwrap()
        .iter()
        .map(|n| n.kind.clone())
        .collect();
    assert!(kinds.contains(&Kind::HardBreak), "got {kinds:?}");
    assert!(
        !kinds.contains(&Kind::RawInline),
        "still raw HTML: {kinds:?}"
    );
}

#[test]
fn backspace_over_an_in_cell_break_deletes_the_whole_br_not_a_byte() {
    // The `<br>` draws as one newline glyph, so Backspace over it must take
    // all four bytes — a one-byte delete would strand a visible `<br` in the
    // cell (the reported bug).
    let mut d = wysiwyg_doc("tbl_break_bs", TABLE);
    d.caret = TABLE.find("Pear").unwrap() + 4;
    assert!(d.cell_line_break());
    assert!(d.source.contains("Pear<br>"), "precondition: {}", d.source);
    d.backspace(); // caret sits just past the break
    assert!(
        !d.source.contains("<br"),
        "no half-deleted <br left: {}",
        d.source
    );
    assert!(
        d.source.contains("| Pear |"),
        "the cell is back to one line: {}",
        d.source
    );
}

#[test]
fn backspace_at_a_cell_start_is_a_wall() {
    // The task's repro: two presses used to take the padding and then the
    // `|`, merging `d` into `c`'s cell and leaving the row a column short.
    let src = "| a | b |\n| - | - |\n| c | d |\n";
    let mut d = wysiwyg_doc("tbl_wall_bs", src);
    d.place_caret(src.find("d |").unwrap(), false);
    for _ in 0..3 {
        d.backspace();
    }
    assert_eq!(d.source, src);
    // Inside the padding too — right after the `|`, before the space. Set
    // directly: `place_caret` would snap it onto the stop past the space.
    d.caret = src.find("| d").unwrap() + 1;
    d.backspace();
    assert_eq!(d.source, src);
    // The row's first cell, and an empty one, are walls the same.
    d.place_caret(src.find("c |").unwrap(), false);
    d.backspace();
    assert_eq!(d.source, src);
    // A cell that opens on hidden markup: the wall is the first letter
    // drawn, not the `**` in front of it.
    let bold = "| a | b |\n| - | - |\n| c | **d** |\n";
    let mut d = wysiwyg_doc("tbl_wall_bs_bold", bold);
    d.place_caret(bold.find("d**").unwrap(), false);
    d.backspace();
    assert_eq!(d.source, bold);
    let empty = "| a | b |\n| - | - |\n| c |   |\n";
    let mut d = wysiwyg_doc("tbl_wall_bs_empty", empty);
    d.place_caret(empty.find("|   |").unwrap() + 2, false);
    d.backspace();
    assert_eq!(d.source, empty);
}

#[test]
fn backspace_inside_a_cell_still_deletes_a_character() {
    let src = "| a | b |\n| - | - |\n| c | de |\n";
    let mut d = wysiwyg_doc("tbl_wall_bs_mid", src);
    d.place_caret(src.find("e |").unwrap(), false);
    d.backspace();
    assert_eq!(d.source, "| a | b |\n| - | - |\n| c | e |\n");
}

#[test]
fn delete_at_a_cell_end_is_a_wall() {
    // The mirror: Delete at `c`'s end would take the padding, then the `|`.
    let src = "| a | b |\n| - | - |\n| c | d |\n";
    let mut d = wysiwyg_doc("tbl_wall_del", src);
    d.place_caret(src.find("c |").unwrap() + 1, false);
    for _ in 0..3 {
        d.delete_forward();
    }
    assert_eq!(d.source, src);
    // And inside the trailing padding, set directly past the snap.
    d.caret = src.find("c |").unwrap() + 2;
    d.delete_forward();
    assert_eq!(d.source, src);
    // The row's last cell is a wall on its closing `|` as well.
    d.place_caret(src.find("d |").unwrap() + 1, false);
    d.delete_forward();
    assert_eq!(d.source, src);
    // Mid-cell Delete is untouched.
    d.place_caret(src.find("c |").unwrap(), false);
    d.delete_forward();
    assert_eq!(d.source, "| a | b |\n| - | - |\n|  | d |\n");
}

#[test]
fn delete_forward_over_an_in_cell_break_deletes_the_whole_br() {
    let mut d = wysiwyg_doc("tbl_break_del", TABLE);
    d.caret = TABLE.find("Pear").unwrap() + 4;
    assert!(d.cell_line_break());
    d.caret = TABLE.find("Pear").unwrap() + 4; // back onto the break's start
    d.delete_forward();
    assert!(
        !d.source.contains("<br"),
        "no half-deleted <br: {}",
        d.source
    );
    assert!(
        d.source.contains("| Pear |"),
        "cell back to one line: {}",
        d.source
    );
}

#[test]
fn shift_return_in_a_djot_cell_is_swallowed_and_leaves_the_row_intact() {
    // Djot has no idiomatic in-cell break, so twig refuses it. The gesture is
    // still consumed (a real newline would split the one-line row), but the
    // cell must be left exactly as it was — no non-idiomatic `<br>` spliced in.
    let src = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n";
    let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
    d.caret = src.find("Pear").unwrap() + 4;
    assert!(d.caret_in_table(), "caret should be inside the djot table");
    assert!(
        d.cell_line_break(),
        "the key is consumed, not passed to the frontend"
    );
    assert_eq!(d.source, src, "the djot cell is left untouched");
    assert!(
        !d.source.contains("<br>"),
        "no non-idiomatic <br> spliced into djot"
    );
    assert!(
        d.status.is_some(),
        "the refusal is surfaced on the status line"
    );
}

#[test]
fn typing_in_a_cell_edits_that_cell() {
    // Editing comes free once offsets map correctly: the caret is a source
    // offset, so a normal splice lands inside the pipe table.
    let mut d = wysiwyg_doc("tbl_type", TABLE);
    d.caret = TABLE.find("Pear").unwrap() + 4;
    d.insert("s");
    assert!(d.source.contains("| Pears | 3 |"), "got {:?}", d.source);
}

#[test]
fn table_cells_are_the_grids_cells_read_off_the_parse() {
    // The cells a host reads without a visual map are the map's own: an
    // empty cell's home included, in source order.
    let src = "Intro\n\n| a | b |\n|---|---|\n|   | dd |\n| e |  |\n";
    let mut d = wysiwyg_doc("tbl_cells", src);
    let from_map: Vec<_> = d.vmap.tables[0]
        .grid
        .iter()
        .flat_map(|r| r.cells.iter().map(|c| c.start..c.end))
        .collect();
    let cells = d.table_cells();
    let text: Vec<_> = cells.iter().map(|c| c.text.clone()).collect();
    assert_eq!(text, from_map);
    let empty: Vec<_> = from_map.iter().filter(|r| r.is_empty()).collect();
    assert_eq!(empty.len(), 2, "{from_map:?}");
    // Each in its place in the grid, of the one table.
    let at: Vec<_> = cells.iter().map(|c| (c.row, c.col)).collect();
    assert_eq!(at, [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)]);
    assert!(cells.iter().all(|c| c.table == src.find('|').unwrap()));
    // Without a table, nothing.
    let mut d = wysiwyg_doc("tbl_cells_none", "Just text.\n");
    assert!(d.table_cells().is_empty());
}
