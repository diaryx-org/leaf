//  LeafToolbarItems.swift
//
//  The formatting controls as a window's toolbar: in the middle of a Mac
//  window's title bar, and on an iPad in the navigation bar's centre, where
//  the system folds what does not fit into its own overflow menu. A host's
//  own items (a source toggle, an attach button) are its own
//  `ToolbarItemGroup`s beside this one.

import SwiftUI

/// The formatting controls as one toolbar group.
///
///     .toolbar {
///         LeafToolbarItems(editor: model)
///         ToolbarItem(placement: .primaryAction) { sourceToggle }
///     }
///
/// On iOS the default placement is `.secondaryAction`, which the navigation
/// bar centres under `.toolbarRole(.editor)`. A phone's bar has no room for
/// it; there the keyboard's row is the toolbar.
public struct LeafToolbarItems<Editor: LeafToolbarEditor>: ToolbarContent {
    let editor: Editor
    let placement: ToolbarItemPlacement

    public init(editor: Editor, placement: ToolbarItemPlacement = .leafFormatting) {
        self.editor = editor
        self.placement = placement
    }

    public var body: some ToolbarContent {
        ToolbarItemGroup(placement: placement) {
            LeafToolbarControls(editor: editor)
        }
    }
}

public extension ToolbarItemPlacement {
    /// Where the formatting controls go by default: the centre of a Mac
    /// window's toolbar, and an iPad navigation bar's secondary actions.
    static var leafFormatting: ToolbarItemPlacement {
        #if os(macOS)
        .principal
        #else
        .secondaryAction
        #endif
    }
}
