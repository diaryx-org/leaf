//  Localized.swift
//
//  The package's one string lookup. It was a private copy on each platform view
//  until the footnote peek needed the same wording from neither — three copies of
//  a bundle lookup being three chances for one of them to point somewhere else.

import Foundation

/// A localized UI string with an English fallback, looked up in the bundle this
/// package ships in — the host app's, for a statically linked package. So a host
/// can translate the menus (drop a `Localizable.strings` with these keys) without
/// the library owning a resource bundle, and the English `value` shows otherwise.
func loc(_ key: String, _ value: String) -> String {
    NSLocalizedString(key, tableName: nil, bundle: Bundle(for: LeafTextView.self),
                      value: value, comment: "")
}
