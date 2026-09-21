# leaf, natively

A native **SwiftUI** front end driving *leaf-core* over the FFI — the same caret model and AST→glyph map the terminal and desktop apps use, on macOS and iOS.

## What's live

- WYSIWYG rendering with `inline code`
- **Bold**, *italic*, and ==highlight==
- Click (or tap) to place the caret, drag to select
- A [link](https://example.invalid/docs) you edit by clicking into it — ⌘-click or right-click to follow it

| Feature | Status |
| --- | :---: |
| Tables | editable |
| Lists | nesting |

> The document is a live, round-trippable AST the whole time you type.

## Math

A formula in a line, $E = mc^2$, is typeset where it sits, and one on lines of its own is set in display style:

$$
\int_0^1 x\,dx = \frac{1}{2}
$$

Put the caret on a formula's line and it shows its TeX, ready to edit.

This paragraph is written in semantic line breaks:
one clause per source line,
a soft break after each.
Toggle the ⏎ menu to fold them into flowing prose or preserve them as written.

```rust
fn main() {
    println!("rendered by leaf-core");
}
```

## Attachments

Images draw inline. Video and audio show a still and a play badge until you
tap one, and then play right where they sit.

![the leaf banner](banner.png)

<video src="clip.mp4" poster="banner.png" controls></video>

<audio src="take.mp3" controls></audio>

A `data:` picture carries its own bytes, so it needs neither a document
directory nor the app — the editor decodes it:

![a dot](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGAAAABgCAIAAABt+uBvAAAACXBIWXMAAAABAAAAAQBPJcTWAAAA2ElEQVR4nO3QQQ3AIADAQEgwO5+IQM4ULH2yx52CpvPsZ/Bt3Q74O4OCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUDAoGBYOCQcGgYFAwKBgUXjPZA4Om5tBAAAAAAElFTkSuQmCC)

And a source the editor can't read itself is handed to the app, which
fetches it into its cache and answers with the file — this one's host does
not exist, so its badge stays:

<video src="https://example.invalid/remote.mp4" controls></video>

Try the toolbar, or the keyboard's arrows and ⌘B / ⌘I.
