//  WindowFrame.swift
//
//  A document window that reopens where it was left. `DocumentGroup` gives its
//  windows no frame autosave name, so AppKit records nothing: a document
//  resized, closed and reopened comes back at the scene's default size. This
//  names each window after its file, which is what makes AppKit save the frame
//  to the user defaults on every move and resize, and put it back when a window
//  of the same name next appears.
//
//  Keyed by path, so a document moved or renamed in the Finder opens at the
//  default size once and is remembered under its new name from then on. An
//  untitled document has no name until it is saved.

#if os(macOS)
import AppKit
import SwiftUI

extension View {
    /// Remember this window's frame under `fileURL`'s path.
    func remembersWindowFrame(for fileURL: URL?) -> some View {
        background(WindowFrameName(name: fileURL.map { "Leaf document \($0.standardizedFileURL.path)" }))
    }
}

private struct WindowFrameName: NSViewRepresentable {
    let name: String?

    func makeNSView(context: Context) -> Probe { Probe() }

    func updateNSView(_ probe: Probe, context: Context) {
        probe.name = name
        probe.apply()
    }

    /// An empty view whose only job is to learn its window.
    final class Probe: NSView {
        var name: String?
        /// The name last given to the window, so a re-render does not restore
        /// the saved frame over one the reader is in the middle of changing.
        private var applied: String?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            apply()
        }

        func apply() {
            guard let window, let name, name != applied else { return }
            applied = name
            // SwiftUI sizes a new window after it is attached; restoring on the
            // next turn of the run loop lands after that, not under it.
            DispatchQueue.main.async {
                // Restore first: naming a window saves its current frame under
                // the name, which would overwrite the one being restored.
                window.setFrameUsingName(name)
                window.setFrameAutosaveName(name)
            }
        }
    }
}
#endif
