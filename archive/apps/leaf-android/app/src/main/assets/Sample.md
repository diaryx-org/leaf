# leaf, on Android

A **Compose** front end driving *leaf-core* over the FFI — the same caret model and AST→glyph map the terminal, Apple and web editors use.

## What's live

- Tap to place the caret, double-tap or long-press to select a word, and drag the handles
- **Bold**, *italic*, ~~struck~~, `inline code`, and ==highlight==
- A [link](https://diaryx.org) — put the caret in it and the 🔗 key repoints it
- Nested lists
  - like this one
  - [ ] and a task you can tick
  - [x] with its box

> The document is a live, round-trippable AST the whole time you type.
>
> > Even a quote inside a quote.

---

```rust
fn main() {
    println!("rendered by leaf-core");
}
```

| Feature | Status |
| --- | :---: |
| Tables | drawn as text for now |
| Lists | nesting |

Try the toolbar above the keyboard, or a hardware keyboard's arrows and Ctrl+B / Ctrl+I.
