//  SettingsView.swift
//
//  The reader's display choices, as one form for both platforms: Leaf ▸
//  Settings… on the Mac, and on iOS a sheet from the gear in the document's
//  navigation bar. Every row edits a `DisplayChoice` key in `UserDefaults`,
//  so every window and every launch reads the same ones, and every open
//  document redraws as one changes.

import LeafUI
import SwiftUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

struct SettingsView: View {
    @AppStorage(DisplayChoice.columnWidthKey) private var columnWidth: ColumnWidth = .medium
    @AppStorage(DisplayChoice.textSizeKey) private var textSize: TextSize = .medium
    @AppStorage(DisplayChoice.paperKey) private var paper: Paper = .usLetter
    @AppStorage(DisplayChoice.flowKey) private var flowPreserved = false
    private var typography = StoredTypography()

    var body: some View {
        #if os(macOS)
        form
            // A grouped form is a scroll view, and its ideal height came out a
            // point short of its content, which put up a scroller beside a pane
            // with nothing to scroll to.
            .scrollDisabled(true)
            .frame(width: 440)
            .fixedSize(horizontal: false, vertical: true)
        #else
        form
        #endif
    }

    private var form: some View {
        Form {
            Section("Text") {
                FontPicker(title: "Font", selection: typography.$bodyFont,
                           defaults: [("Iowan Old Style (default)", EditorTheme.serifFontName),
                                      ("System", EditorTheme.systemFontName)],
                           families: FontPicker.allFamilies)
                FontPicker(title: "Code font", selection: typography.$codeFont,
                           defaults: [("System monospaced (default)", EditorTheme.systemMonospacedFontName)],
                           families: FontPicker.monospacedFamilies)
                Picker(label("Size"), selection: $textSize) {
                    ForEach(TextSize.allCases) { Text($0.label).tag($0) }
                }
            }
            Section("Layout") {
                Picker(label("Line spacing"), selection: typography.$lineSpacing) {
                    ForEach(LineSpacing.allCases) { Text($0.label).tag($0) }
                }
                Picker(label("Paragraph spacing"), selection: typography.$paragraphSpacing) {
                    ForEach(ParagraphSpacing.allCases) { Text($0.label).tag($0) }
                }
                Picker(label("Column width"), selection: $columnWidth) {
                    ForEach(ColumnWidth.allCases) { Text($0.label).tag($0) }
                }
                Toggle("Preserve line breaks in new windows", isOn: $flowPreserved)
            }
            Section("Page") {
                Picker(label("Paper"), selection: $paper) {
                    ForEach(Paper.allCases) { Text($0.label).tag($0) }
                }
            }
            #if os(macOS)
            HStack {
                Spacer()
                Button("Restore Defaults", action: restoreDefaults)
            }
            #else
            Section {
                Button("Restore Defaults", role: .destructive, action: restoreDefaults)
            }
            #endif
        }
        .formStyle(.grouped)
    }

    /// Every choice back to what a first launch has. Removing the keys rather
    /// than writing the defaults, so a later release that changes a default
    /// reaches a reader who never chose otherwise.
    private func restoreDefaults() {
        let keys = [
            DisplayChoice.columnWidthKey, DisplayChoice.textSizeKey, DisplayChoice.paperKey,
            DisplayChoice.flowKey, DisplayChoice.bodyFontKey, DisplayChoice.codeFontKey,
            DisplayChoice.lineSpacingKey, DisplayChoice.paragraphSpacingKey,
        ]
        keys.forEach(UserDefaults.standard.removeObject(forKey:))
    }
}

/// A row's label in each platform's spelling: a Mac form's labels end in a
/// colon, an iOS list's do not.
private func label(_ text: String) -> String {
    #if os(macOS)
    text + ":"
    #else
    text
    #endif
}

/// A font family menu: the theme's own names first, then every family
/// installed that fits. A family stored by a font since uninstalled keeps
/// its row, so the menu still says what is chosen; the editor sets such a
/// font in the system face.
private struct FontPicker: View {
    let title: String
    @Binding var selection: String
    let defaults: [(label: String, name: String)]
    let families: [String]

    var body: some View {
        Picker(label(title), selection: $selection) {
            ForEach(defaults, id: \.name) { Text($0.label).tag($0.name) }
            Divider()
            if !families.contains(selection) && !defaults.contains(where: { $0.name == selection }) {
                Text(selection).tag(selection)
            }
            ForEach(families, id: \.self) { Text($0).tag($0) }
        }
        #if os(iOS)
        // Some hundred families: a menu that long is a scroll the length of
        // the screen, where a pushed list can be searched by eye.
        .pickerStyle(.navigationLink)
        #endif
    }

    /// Every family installed, hidden ones (a leading `.`) aside.
    static let allFamilies: [String] = {
        #if canImport(AppKit)
        NSFontManager.shared.availableFontFamilies.filter { !$0.hasPrefix(".") }
        #else
        UIFont.familyNames.filter { !$0.hasPrefix(".") }.sorted()
        #endif
    }()

    /// The families with a fixed-pitch face, for code.
    static let monospacedFamilies: [String] = {
        #if canImport(AppKit)
        let names = NSFontManager.shared.availableFontNames(with: .fixedPitchFontMask) ?? []
        let families = Set(names.compactMap { NSFont(name: $0, size: 12)?.familyName })
        #else
        let families = Set(UIFont.familyNames.filter { family in
            UIFont.fontNames(forFamilyName: family).contains { name in
                UIFont(name: name, size: 12)?.fontDescriptor.symbolicTraits.contains(.traitMonoSpace) == true
            }
        })
        #endif
        return families.filter { !$0.hasPrefix(".") }.sorted()
    }()
}
