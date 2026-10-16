//  SettingsView.swift
//
//  Leaf ▸ Settings…: the reader's display choices as one form. Every row
//  edits a `DisplayChoice` key in `UserDefaults`, so every window and every
//  launch reads the same ones, and every open document redraws as one
//  changes.

import AppKit
import SwiftUI

struct SettingsView: View {
    @AppStorage(DisplayChoice.columnWidthKey) private var columnWidth: ColumnWidth = .medium
    @AppStorage(DisplayChoice.textSizeKey) private var textSize: TextSize = .medium
    @AppStorage(DisplayChoice.flowKey) private var flowPreserved = false
    @AppStorage(DisplayChoice.paperKey) private var paper: Paper = .usLetter
    private var typography = StoredTypography()

    var body: some View {
        Form {
            Section("Text") {
                FontPicker(title: "Font:", selection: typography.$bodyFont,
                           defaults: [("Iowan Old Style (default)", DisplayChoice.serifFontName),
                                      ("System", DisplayChoice.systemFontName)],
                           families: FontPicker.allFamilies)
                FontPicker(title: "Code font:", selection: typography.$codeFont,
                           defaults: [("Menlo (default)", DisplayChoice.monoFontName)],
                           families: FontPicker.monospacedFamilies)
                Picker("Size:", selection: $textSize) {
                    ForEach(TextSize.allCases) { Text($0.label).tag($0) }
                }
            }
            Section("Layout") {
                Picker("Line spacing:", selection: typography.$lineSpacing) {
                    ForEach(LineSpacing.allCases) { Text($0.label).tag($0) }
                }
                Picker("Column width:", selection: $columnWidth) {
                    ForEach(ColumnWidth.allCases) { Text($0.label).tag($0) }
                }
                Toggle("Preserve line breaks in new windows", isOn: $flowPreserved)
            }
            Section("Page") {
                Picker("Paper:", selection: $paper) {
                    ForEach(Paper.allCases) { Text($0.label).tag($0) }
                }
            }
            HStack {
                Spacer()
                Button("Restore Defaults", action: restoreDefaults)
            }
        }
        .formStyle(.grouped)
        // A grouped form is a scroll view, and its ideal height came out a
        // point short of its content, which put up a scroller beside a pane
        // with nothing to scroll to.
        .scrollDisabled(true)
        .frame(width: 440)
        .fixedSize(horizontal: false, vertical: true)
    }

    /// Every choice back to what a first launch has. Removing the keys rather
    /// than writing the defaults, so a later release that changes a default
    /// reaches a reader who never chose otherwise.
    private func restoreDefaults() {
        [DisplayChoice.columnWidthKey, DisplayChoice.textSizeKey, DisplayChoice.flowKey,
         DisplayChoice.bodyFontKey, DisplayChoice.codeFontKey, DisplayChoice.lineSpacingKey,
         DisplayChoice.paperKey]
            .forEach(UserDefaults.standard.removeObject(forKey:))
    }
}

/// A font family menu: the defaults first, then every family installed that
/// fits. A family stored by a font since uninstalled keeps its row, so the
/// menu still says what is chosen.
private struct FontPicker: View {
    let title: String
    @Binding var selection: String
    let defaults: [(label: String, name: String)]
    let families: [String]

    var body: some View {
        Picker(title, selection: $selection) {
            ForEach(defaults, id: \.name) { Text($0.label).tag($0.name) }
            Divider()
            if !families.contains(selection) && !defaults.contains(where: { $0.name == selection }) {
                Text(selection).tag(selection)
            }
            ForEach(families, id: \.self) { Text($0).tag($0) }
        }
    }

    /// Every family installed, hidden ones (a leading `.`) aside.
    static let allFamilies: [String] =
        NSFontManager.shared.availableFontFamilies.filter { !$0.hasPrefix(".") }

    /// The families with a fixed-pitch face, for code.
    static let monospacedFamilies: [String] = {
        let names = NSFontManager.shared.availableFontNames(with: .fixedPitchFontMask) ?? []
        let families = Set(names.compactMap { NSFont(name: $0, size: 12)?.familyName })
        return families.filter { !$0.hasPrefix(".") }.sorted()
    }()
}
