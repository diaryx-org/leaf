//  UndoBridge.swift
//
//  The `UndoManager` the two text views hand the responder chain.
//
//  leaf keeps no history of its own — twig owns the bytes and carries the caret
//  through each step — so there is nothing here to register: no closures, no
//  targets, no grouping. What the system wants from an undo manager is four
//  answers — may I undo, may I redo, do it, do it back — and this class gives
//  each straight from core. With it in place the whole native undo surface
//  works without the view catching a key: the Edit menu's Undo and Redo enable
//  and fire through `NSWindow`'s standard `undo:`/`redo:` handling, iOS's
//  three-finger swipe and shake gestures find it through `UIResponder`, and the
//  edit menu's Undo/Redo appear when there is something to do.
//
//  Subclassing rather than registering is the only honest shape. A registered
//  action would have to *be* the inverse edit, and the inverse lives in twig;
//  registering "call `doc.undo()`" would leave two histories that have to agree
//  about depth, which is exactly the lockstep leaf-core got rid of.

import Foundation

final class LeafUndoManager: UndoManager {
    private let query: () -> (canUndo: Bool, canRedo: Bool)
    private let undoStep: () -> Void
    private let redoStep: () -> Void

    init(state: @escaping () -> (canUndo: Bool, canRedo: Bool),
         undo: @escaping () -> Void,
         redo: @escaping () -> Void) {
        query = state
        undoStep = undo
        redoStep = redo
        super.init()
        // Never let AppKit/UIKit open an implicit group per event loop pass:
        // nothing is registered, so a group would only ever be empty.
        groupsByEvent = false
    }

    override var canUndo: Bool { query().canUndo }
    override var canRedo: Bool { query().canRedo }

    override func undo() {
        guard canUndo else { return }
        NotificationCenter.default.post(name: .NSUndoManagerWillUndoChange, object: self)
        undoStep()
        NotificationCenter.default.post(name: .NSUndoManagerDidUndoChange, object: self)
    }

    override func redo() {
        guard canRedo else { return }
        NotificationCenter.default.post(name: .NSUndoManagerWillRedoChange, object: self)
        redoStep()
        NotificationCenter.default.post(name: .NSUndoManagerDidRedoChange, object: self)
    }

    // The history is twig's; nothing the system registers here can take part
    // in it, and the base class must not think it did.
    override func registerUndo(withTarget target: Any, selector: Selector, object anObject: Any?) {}
    override func removeAllActions() {}
    override func removeAllActions(withTarget target: Any) {}
}
