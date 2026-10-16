// leaf_dioxus_embed.h — the leaf-dioxus editor in a native view its host owns.
//
// Written by hand beside src/ffi.rs, which is the authority: every function
// and struct here is declared there with the same name and layout. Every
// function is called on the main thread; strings are UTF-8, borrowed for the
// call when they are the host's and freed with leaf_string_free when they are
// the editor's.

#ifndef LEAF_DIOXUS_EMBED_H
#define LEAF_DIOXUS_EMBED_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct LeafView LeafView;

// Document formats.
enum { LEAF_FORMAT_MARKDOWN = 0, LEAF_FORMAT_DJOT = 1, LEAF_FORMAT_HTML = 2 };

// Modifier bits.
enum {
    LEAF_MOD_SHIFT = 1,
    LEAF_MOD_CONTROL = 2,
    LEAF_MOD_ALT = 4,
    LEAF_MOD_COMMAND = 8,
};

// What `request` asks the host to do.
enum {
    LEAF_REQUEST_SAVE = 1,
    LEAF_REQUEST_SAVE_AS = 2,
    LEAF_REQUEST_ZOOM = 3,   // arg: 1 in, -1 out, 0 actual size
    LEAF_REQUEST_SETTINGS = 4,
    LEAF_REQUEST_CHARACTER_PALETTE = 5,
};

// What a context menu should offer, as bits.
enum {
    LEAF_CONTEXT_SELECTION = 1,
    LEAF_CONTEXT_LINK = 2,
    LEAF_CONTEXT_GO_TO_NOTE = 4,
    LEAF_CONTEXT_BACK_TO_REFERENCE = 8,
};

// Table menu items, for leaf_view_table.
enum {
    LEAF_TABLE_INSERT = 0,        // arg: columns
    LEAF_TABLE_INSERT_ROW = 1,    // arg: 1 below, 0 above
    LEAF_TABLE_DELETE_ROW = 2,
    LEAF_TABLE_INSERT_COLUMN = 3, // arg: 1 right, 0 left
    LEAF_TABLE_DELETE_COLUMN = 4,
    LEAF_TABLE_ALIGN = 5,         // arg: 0 default, 1 left, 2 center, 3 right
    LEAF_TABLE_MOVE_ROW = 6,      // arg: 1 down, 0 up
    LEAF_TABLE_MOVE_COLUMN = 7,   // arg: 1 right, 0 left
};

// Actions no key stands for, for leaf_view_action.
enum {
    LEAF_ACTION_OPEN_LINK = 1,
    LEAF_ACTION_COPY_LINK = 2,
    LEAF_ACTION_FOLLOW_FOOTNOTE = 3,
    LEAF_ACTION_INSERT_FOOTNOTE = 4,
    LEAF_ACTION_INSERT_RULE = 5,
    LEAF_ACTION_INSERT_PAGE_BREAK = 6,
    LEAF_ACTION_TOGGLE_VERSE = 7,
};

// The host's half: called on the main thread with `ctx`; any may be null.
typedef struct {
    void *ctx;
    void (*changed)(void *ctx);
    void (*request)(void *ctx, uint32_t kind, int32_t arg);
    void (*open)(void *ctx, const char *what);
    void (*context_menu)(void *ctx, uint32_t flags);
    void (*ime_area)(void *ctx, double x, double y, double w, double h);
    void (*set_cursor)(void *ctx, uint32_t cursor); // 0 arrow, 1 I-beam, 2 hand
} LeafHostCallbacks;

typedef struct {
    uint64_t revision;
    bool dirty;
    bool source_view;
    bool in_table;
    bool has_selection;
    bool can_undo;
    bool can_redo;
    uint32_t heading;   // 0 in a paragraph
    bool code_block;
    bool verse;
    bool blockquote;
    int32_t task;       // -1 none, 0 open, 1 done
    uint32_t marks;     // LEAF_MARK_* bits
    bool link;
    int32_t mark_color; // -1 none, else LEAF_COLOR_*
    bool preserve_breaks;
} LeafState;

// Inline marks, as LeafState.marks bits.
enum {
    LEAF_MARK_STRONG = 1,
    LEAF_MARK_EMPH = 2,
    LEAF_MARK_CODE = 4,
    LEAF_MARK_HIGHLIGHT = 8,
    LEAF_MARK_SUPERSCRIPT = 16,
    LEAF_MARK_SUBSCRIPT = 32,
    LEAF_MARK_UNDERLINE = 64,
    LEAF_MARK_STRIKE = 128,
};

// Highlight colours, in menu order.
enum {
    LEAF_COLOR_RED = 0,
    LEAF_COLOR_ORANGE = 1,
    LEAF_COLOR_YELLOW = 2,
    LEAF_COLOR_GREEN = 3,
    LEAF_COLOR_BLUE = 4,
    LEAF_COLOR_PURPLE = 5,
    LEAF_COLOR_BROWN = 6,
};

LeafView *leaf_view_new(void *ns_view, double width, double height, double scale, bool dark,
                        const uint8_t *source, size_t len, uint32_t format, const char *path,
                        LeafHostCallbacks callbacks);
void leaf_view_free(LeafView *view);

void leaf_view_resize(LeafView *view, double width, double height, double scale);
void leaf_view_set_dark(LeafView *view, bool dark);
void leaf_view_set_focused(LeafView *view, bool focused);
void leaf_view_modifiers(LeafView *view, uint32_t mods);
// kind: 0 move, 1 press, 2 release, 3 left; button: 0 main, 1 secondary.
void leaf_view_pointer(LeafView *view, uint32_t kind, double x, double y, uint32_t button,
                       uint32_t mods);
void leaf_view_scroll(LeafView *view, double dx, double dy, bool precise);

void leaf_view_key(LeafView *view, const char *key, const char *code, uint32_t mods);
void leaf_view_insert_text(LeafView *view, const char *text);
void leaf_view_set_marked_text(LeafView *view, const char *text, size_t sel_start, size_t sel_end);
void leaf_view_delete_surrounding(LeafView *view, size_t before, size_t after);

char *leaf_view_source(LeafView *view);
void leaf_string_free(char *s);
void leaf_view_set_path(LeafView *view, const char *path);
void leaf_view_mark_saved(LeafView *view);
void leaf_view_state(LeafView *view, LeafState *out);
void leaf_view_set_look(LeafView *view, const char *body, const char *mono, float size,
                        float leading, float measure, uint32_t appearance);
void leaf_view_table(LeafView *view, uint32_t cmd, int32_t arg);
void leaf_view_action(LeafView *view, uint32_t action);
char *leaf_view_link_at_caret(LeafView *view);
char *leaf_view_selected_text(LeafView *view);

// The text the system indexes, in UTF-16 ranges: the visible text, or with
// prose the same with all but prose blanked, for the spelling checker.
char *leaf_view_text(LeafView *view, bool prose);
void leaf_view_selected_range(LeafView *view, size_t *loc, size_t *len);
void leaf_view_select_range(LeafView *view, size_t loc, size_t len);
// Replace loc..loc+len, if its source bytes spell original; behind: as an
// automatic substitution, the caret left where it was.
bool leaf_view_replace_range(LeafView *view, size_t loc, size_t len, const char *original,
                             const char *replacement, bool behind);
void leaf_view_undo_group(LeafView *view, bool begin);
// The UTF-16 index of the character at a point of the view, -1 off the text;
// and where a range is drawn (its first line), as x, y, width, height.
int64_t leaf_view_index_at_point(LeafView *view, double x, double y);
bool leaf_view_range_rect(LeafView *view, size_t loc, size_t len, double out[4]);
// ranges: count (location, length) pairs, misspelled in the text at revision.
void leaf_view_set_misspellings(LeafView *view, uint64_t revision, const size_t *ranges,
                                size_t count);
void leaf_view_insert_link(LeafView *view, const char *destination);
void leaf_view_highlight(LeafView *view, int32_t color); // -1 the plain highlighter
void leaf_view_set_preserve_breaks(LeafView *view, bool preserve);

// The document as a PDF on paper paper_w x paper_h points: the bytes,
// *out_len long, freed with leaf_bytes_free; null if it could not be made.
uint8_t *leaf_pdf(const uint8_t *source, size_t len, uint32_t format, const char *path,
                  const char *body, const char *mono, float size, float leading,
                  float paper_w, float paper_h, float margin, size_t *out_len);
void leaf_bytes_free(uint8_t *bytes, size_t len);

#ifdef __cplusplus
}
#endif

#endif
