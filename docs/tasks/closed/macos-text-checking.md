---
status: done
created: 2026-09-04
updated: 2026-09-22
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# Automatic correction and substitutions in the macOS view

**Status.** Done, in `feat(swift): the macOS view corrects spelling, expands
text replacements, and curls quotes and dashes as prose is typed`. The view
has `NSTextView`'s four per-view flags, seeded from `NSSpellChecker`'s
`isAutomatic…Enabled` and following the system's change notifications, and
its four `toggleAutomatic…:` actions, validated with a checkmark and
disabled in the source view and on a reader. The SwiftUI Edit menu carries
Correct Spelling Automatically and a Substitutions menu. When a keystroke
completes a substitution, the checker is asked about that keystroke's
paragraph, with code masked. A quote is curled as it is typed, a `--`
becomes a dash when the next character is not a hyphen, and a word is
corrected or expanded when the character that ends it is typed. Each is
made through core's new `Doc::substitute`, which leaves the caret where the
typing left it and is an undo step of its own. It replaces the checker's
characters only when they are exactly those bytes in the source, so a
substitution cannot reach into hidden markup. A corrected word is underlined
in blue, and the system's reversion indicator offers the original back when
the caret rests on it. Two differences from `NSTextView`: there is no
suggestion bubble before a word is committed, and text committed by an IME
or dictation is not substituted.

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
