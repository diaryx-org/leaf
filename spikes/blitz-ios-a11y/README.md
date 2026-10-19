# Spike: VoiceOver on a Blitz page, on iOS (2026-10-19)

Does iOS's accessibility (VoiceOver, XCUITest, an agent) reach a page drawn by
Blitz? Yes, with AccessKit's iOS adapter and four small changes to Blitz's
tree. Verified on an iPhone 17 simulator, iOS 27.

```sh
scripts/run.sh                                  # build, bundle, run on agent-blitz-a11y
SIMCTL_CHILD_BLITZ_A11Y_ACTIVATE="Press me" scripts/run.sh   # and activate the button as VoiceOver would
cd uitest && xcodegen generate && xcodebuild test -project ReadTree.xcodeproj \
  -scheme ReadTree -destination 'platform=iOS Simulator,name=agent-blitz-a11y'
xcrun simctl delete agent-blitz-a11y             # when done
```

## What it is

A one-window Dioxus page (a heading, a paragraph, a button that counts its
presses, a status line) on blitz-shell, hosted as leaf's Mac app hosts its
windows (`src/main.rs`). `accesskit_ios` 0.2.1's `SubclassingAdapter` takes
over winit's `UIView`; after every frame the adapter is given blitz-dom's
tree; an action request comes back through the event loop and is carried out
as a click at the node's middle.

blitz-shell's own `accessibility` feature is not used: it goes through
`accesskit_xplat`, which has no iOS adapter and falls back to a null one.

## What it showed

`uitest/` reads the tree from outside, as XCUITest sees it, then taps the
button by its accessibility frame. The app's self-test
(`BLITZ_A11Y_ACTIVATE`) finds the element by its label through UIKit's own
accessibility API and calls `accessibilityActivate`, which is VoiceOver's
double tap. Both pressed the button ("Pressed 2 times").

| Element | Type | Label | Frame (pt) |
|---|---|---|---|
| heading | static text | Leaf on Blitz | 56, 209, 290×33 |
| paragraph | static text | VoiceOver should read this paragraph, … | 56, 261, 290×40 |
| button | button | Press me | 56, 330, 104×42 |
| status | static text | Pressed 1 times | 56, 401, 290×20 |

## What it needed

In blitz-dom, on diaryx-org's fork (`github.com/diaryx-org/blitz`, branch
`leaf`), all platform-neutral:

1. **Bounds** on every node (DioxusLabs/blitz#842), and the viewport's for
   the document node, which has no box.
2. **Text as static text**: a `Label` with its text as its *value* (AccessKit's
   convention, which the iOS adapter follows strictly; macOS's accepted a
   label). A `TextRun` is dropped by every adapter outside a text field.
   Whitespace-only text is skipped. An element with no semantics is a
   `GenericContainer`, passed through, not an unnamed `Unknown`.
3. **The click action** on buttons, links, menu items, checkboxes, radios,
   tabs, options and switches, so assistive tech offers a press.

In the host (`src/main.rs`), what blitz-shell would do on iOS:

4. **The safe area.** blitz-shell lays the page out inside the window's safe
   area, and the adapter places nodes in the view, which is the whole screen:
   the tree's root is translated by the safe area's insets, and a click asked
   for is placed with them. Without it every frame was 62 pt too high, and a
   tap at an element's frame missed it.
5. **Carrying out an action request** (blitz-shell's handler is a `TODO`).

And in winit-uikit (leaf's `patches/winit-uikit`): rust-windowing/winit#4723, the
UIScene lifecycle, without which an app built with the iOS 27 SDK does not
launch. The page draws with Vello on the CPU: Vello on the GPU needs indirect
execution, which the simulator's Metal lacks (a device may not).

## Not covered

VoiceOver itself does not run in the simulator; `accessibilityActivate` is
what it calls. A real device, the editor widget (whose rows need roles), text
input and editing, and the tree's cost per frame (rebuilt whole) are left.
