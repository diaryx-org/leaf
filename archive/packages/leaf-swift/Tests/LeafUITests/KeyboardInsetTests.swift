//  KeyboardInsetTests.swift
//
//  `LeafEditorController` keeps the keyboard off the prose by content inset,
//  and the inset is the keyboard against the frame as it stands — taken again
//  when the frame or its safe area moves after the keyboard has, which is
//  when a host's layout answers it. Reckoned once at the notification, a bar
//  leaving the host's safe area a pass later left blank paper above the keys.

#if canImport(UIKit)
import XCTest
import UIKit
@testable import LeafUI

final class KeyboardInsetTests: XCTestCase {
    private let screen = CGRect(x: 0, y: 0, width: 402, height: 874)
    /// A keyboard 336 tall under a 44pt accessory.
    private var keyboard: CGRect { CGRect(x: 0, y: screen.height - 380, width: 402, height: 380) }

    /// The editor filling a host controller, in a window, with `bottom` of
    /// safe area under it.
    private func editor(bottom: CGFloat) -> (UIWindow, UIViewController, LeafEditorController) {
        let window = UIWindow(frame: screen)
        let host = UIViewController()
        window.rootViewController = host
        window.isHidden = false
        host.additionalSafeAreaInsets.bottom = bottom
        let controller = LeafEditorController()
        // As `makeUIViewController` sets it: a scroll that bounds vertically
        // takes the safe area into its adjusted insets, as the editor's does.
        controller.scroll.alwaysBounceVertical = true
        host.addChild(controller)
        controller.view.frame = host.view.bounds
        controller.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        host.view.addSubview(controller.view)
        controller.didMove(toParent: host)
        window.layoutIfNeeded()
        return (window, host, controller)
    }

    private func keyboardWillChangeFrame(to end: CGRect) {
        NotificationCenter.default.post(name: UIResponder.keyboardWillChangeFrameNotification, object: nil,
                                        userInfo: [UIResponder.keyboardFrameEndUserInfoKey: NSValue(cgRect: end),
                                                   UIResponder.keyboardAnimationDurationUserInfoKey: 0.0])
    }

    /// What the keyboard leaves visible: the frame less every bottom inset.
    private func visibleBottom(_ controller: LeafEditorController) -> CGFloat {
        controller.scroll.bounds.height - controller.scroll.adjustedContentInset.bottom
    }

    func testTheInsetMeetsTheKeyboard() {
        let (window, _, controller) = editor(bottom: 34)
        defer { window.isHidden = true }
        keyboardWillChangeFrame(to: keyboard)
        XCTAssertEqual(visibleBottom(controller), keyboard.minY, accuracy: 0.5)
    }

    func testASafeAreaThatShrinksAfterTheKeyboardRoseLeavesNoGap() {
        // A host bar of 60 over the home indicator's 34, leaving as the
        // keyboard comes — a layout pass after the notification.
        let (window, host, controller) = editor(bottom: 94)
        defer { window.isHidden = true }
        keyboardWillChangeFrame(to: keyboard)
        host.additionalSafeAreaInsets.bottom = 34
        window.layoutIfNeeded()
        XCTAssertEqual(visibleBottom(controller), keyboard.minY, accuracy: 0.5)
    }

    func testASafeAreaThatGrowsUnderTheKeyboardLeavesNoGap() {
        let (window, host, controller) = editor(bottom: 34)
        defer { window.isHidden = true }
        keyboardWillChangeFrame(to: keyboard)
        host.additionalSafeAreaInsets.bottom = 94
        window.layoutIfNeeded()
        XCTAssertEqual(visibleBottom(controller), keyboard.minY, accuracy: 0.5)
    }

    func testAFrameThatShrinksToTheKeyboardDropsTheInset() {
        let (window, host, controller) = editor(bottom: 34)
        defer { window.isHidden = true }
        keyboardWillChangeFrame(to: keyboard)
        // The host's own keyboard avoidance, landing after the notification.
        controller.view.autoresizingMask = []
        controller.view.frame = CGRect(x: 0, y: 0, width: 402, height: keyboard.minY)
        host.view.layoutIfNeeded()
        controller.view.layoutIfNeeded()
        XCTAssertEqual(controller.scroll.contentInset.bottom, 0, accuracy: 0.5)
    }

    func testAHiddenKeyboardLeavesNoInset() {
        let (window, _, controller) = editor(bottom: 34)
        defer { window.isHidden = true }
        keyboardWillChangeFrame(to: keyboard)
        keyboardWillChangeFrame(to: keyboard.offsetBy(dx: 0, dy: keyboard.height))
        XCTAssertEqual(controller.scroll.contentInset.bottom, 0)
    }
}
#endif
