# leaf-android

The Android editor for [leaf](../../README.md): `LeafEditor`, a Jetpack Compose
editor over `leaf-core`'s document model, reached through the same UniFFI
binding the Apple package uses ([`leaf-ffi`](../../crates/leaf-ffi)), generated
as Kotlin. The Android peer of [`leaf-swift`](../leaf-swift) and
[`leaf-web`](../leaf-web); [`apps/leaf-android`](../../apps/leaf-android) is the
app over it.

It is its own Gradle build, group `org.diaryx.leaf`, with two modules:

| module | what it is |
|--------|------------|
| `leaf-ffi` | `libleaf_ffi.so` for each ABI and the Kotlin generated from it (package `uniffi.leaf_ffi`), both written under `leaf-ffi/build/rust/` by `cargo xtask android` and never committed. |
| `leaf-compose` | `LeafEditor`, `LeafEditorState`, `LeafFormattingBar` and `LeafTheme`. Compiled *against* `uniffi.leaf_ffi` without carrying it. |

The split is for a host that links leaf-ffi into a larger Rust library of its
own — several UniFFI components in one `.so` — and so already has a
`uniffi.leaf_ffi` package: it depends on `leaf-compose` alone. A host with
nothing of its own depends on both, as the app does.

## Using it

```kotlin
val state = remember { LeafEditorState(LeafDoc(text, "markdown")) }
Column(Modifier.imePadding()) {
    LeafEditor(state, Modifier.weight(1f))
    LeafFormattingBar(state)
}
// …and to save: write state.source() wherever it came from, then state.markSaved().
```

`LeafEditorState.chrome` is what a toolbar lights itself from, `onEdit` hears
every change to the text, and `command { doc -> doc.toggleBold() }` runs any
core command and repaints. `requestFocus()` takes the keyboard without a tap,
for a document opened to be written in, and `goTo(locator)` opens the document
at the place a link's `#v2` names. Only the editing surface is here —
saving, the document's name, and the window are the host's.

A link is followed from a chip that stands under the caret while it rests in
one — a tap places the caret, so it cannot also be the gesture that leaves, and
the platform's text menu has no room for an item of the editor's own. Open
moves to a `#fragment` in the same document itself, then asks the host's
`onOpenLink`, which returns true to claim a destination only it understands (a
note app's `./sibling.md` or `id:6tzwsxg`), and otherwise hands the destination
to the system.

## How it works

The contract every leaf frontend keeps: core owns the text, the caret and the
selection, and a frontend paints what core answers and sends gestures back.
Every call returns a `DocView` frame, and core answers with *changes*
(`set_incremental_frames`), so a keystroke carries the row it touched rather
than the document across JNA.

- **Rows are wrapped here.** Core hands over unwrapped rows (`set_unwrapped`,
  one per block), and each is shaped by Compose's `TextMeasurer` at the
  column's width, with continuation lines hung under the row's prefix through
  `TextIndent`. The caret (`getCursorRect`), the hit test
  (`getOffsetForPosition`), the selection (`getPathForRange`) and the glyphs
  all come from that one layout, so they cannot disagree. A row's string is its
  runs' text exactly, so its UTF-16 indices are core's `caret_ch`/`click_ch`;
  a quote's gutter is a placeholder over its `│ ` characters, so the bar's width
  is the theme's without an offset moving. Up and Down move between *these*
  lines, keeping a goal column.
- **The keyboard is an `InputConnection`** (`LeafInputConnection`), opened
  through Compose's `PlatformTextInputModifierNode`. The IME's flat UTF-16
  indices are converted through core (`utf16_index_for_offset`,
  `text_in_range`), which spells exactly the text it draws. Composing text is
  written into the document as it is composed, through core's `insert` over
  the composing range rather than a raw splice — so an armed Bold takes it, and
  hidden-markup mode keeps a typed `*` literal — and the range is found again
  afterwards in caret stops. A one-character delete behind a bare caret is
  core's Backspace, which is structural at a block's start.
- **Clipboard** copies the selection's HTML flavour beside its text and pastes
  HTML through `paste_rich` where the clipboard has it.

## Not yet

Tables draw as core's box-drawn picture in monospace rather than a grid, block
media and display math as their placeholder rows, and a directive's panel is
not outlined — see [Android block views](../../docs/tasks/android-block-views.md).
The Apple views' footnote and link peeks, wikilink following, find, the
presentation-vocabulary menus and page layout have no counterpart yet.

## Building

`cargo xtask android` builds the library, generates the binding, builds the app
and installs it on the attached device; `--ffi-only` stops after the binding,
which is what Android Studio needs before its first sync of either build. It
needs an Android SDK with an NDK (found through `ANDROID_HOME`, or Homebrew's
`android-commandlinetools`), `cargo-ndk`, and the `aarch64-linux-android`
target (`x86_64-linux-android` too for `--all-abis`).

The generated Kotlin is used as UniFFI writes it. An error variant's `message`
field would clash with `Throwable.message`, so in Kotlin alone it is called
`reason` (`[bindings.kotlin.rename]` in `crates/leaf-ffi/uniffi.toml`):
`LeafException.Parse.reason` is what Swift calls `LeafError.Parse(message:)`.

twig-sys builds for Android from its Zig source (bionic's thread-locals need
API 29, which is `minSdk`); twig 3.11.1 is the first release that knows the two
Android targets.
