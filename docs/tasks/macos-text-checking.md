---
status: open
created: 2026-09-04
updated: 2026-09-22
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Automatic correction and substitutions in the macOS view

**Where.** `packages/leaf-swift`, `LeafTextView` (macOS).

**What.** Spelling is done: since `9322b88` the view checks prose through
`NSSpellChecker` and draws the red underline. A right-click offers the
guesses, Ignore Spelling and Learn Spelling. Check Spelling While Typing
turns it on and off, and the source view is not checked. What the view
still lacks is everything the system does *to* text as it is typed. It asks
`requestChecking` for `.spelling` alone, so a Mac user's Correct Spelling
Automatically, their text replacements (System Settings ▸ Keyboard ▸ Text
Replacements), and smart quotes and dashes do nothing here. The iOS view has
all of these through `UITextInputTraits`, and turns them off in the source
view.

**How.** Ask for the correcting types as well (`.correction`,
`.replacement`, `.quote`, `.dash`) when the matching user default or
per-view toggle is on. Take them from `NSSpellChecker`'s
`isAutomaticSpellingCorrectionEnabled`,
`isAutomaticTextReplacementEnabled`, `isAutomaticQuoteSubstitutionEnabled`
and `isAutomaticDashSubstitutionEnabled`, the way `NSTextView` does. Apply
each result through `replaceRange` as the word or punctuation that triggered
it is committed, not over the whole document. A correction needs the
system's revert affordance too: an undo that puts back what was typed, and
the correction indicator (`showCorrectionIndicator(of:primaryString:alternativeStrings:forStringIn:view:completionHandler:)`).
Edit ▸ Substitutions and Edit ▸ Spelling and Grammar ▸ Correct Spelling
Automatically should toggle them per view, as `NSTextView`'s items do.
As with spelling, and as the iOS traits do, nothing is substituted in the
source view, where a straight quote or a `--` is markup.

**Done when.** With the system settings on, typing a misspelling and a space
in the rendered view corrects it, with the underline that offers the
original back. A text replacement expands. `"` and `--` become curly quotes
and an em dash. Undo reverts each one. None of these happen in the source
view, and the Edit menu's toggles turn each off.
