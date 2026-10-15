# A document the editor has to draw

Ordinary prose, with **bold**, *italic*, `inline code`, ~~struck~~ text and a
[link](https://diaryx.org) in it — the inline shapes a row is made of, so a
row is several runs rather than one.

## Lists and quotes

- A list item, with **emphasis** inside it.
- A second item, long enough to soft-wrap at eighty columns and so become two
  visual rows out of one source line.
    - A nested item.

> A blockquote, with its gutter glyph on every row it spans, which is how a
> synthetic glyph gets into the measurement.

## A fenced block

```rust
fn main() {
    // Syntax highlighting puts a Token on every glyph here, which makes this
    // the most run-dense kind of row in the corpus.
    let greeting = "hello";
    println!("{greeting}, {}", 1 + 2);
}
```

Closing prose, so the block is not the last thing in the section.
