//  LinkDestination.swift
//
//  Where a followed link goes when the host declines it: the one URL the
//  system is handed. A destination that is already a URL — `https:`,
//  `mailto:`, `file:` — goes as it is. One that is a path, which is most links
//  in a folder of notes (`notes.txt`, `../2026/july.md`, `my%20draft.dj#v2`),
//  is a file beside the document, and has to be made into one: handed over
//  bare, `URL(string: "notes.txt")` is a URL with no scheme and no base, and
//  Launch Services answers it with error -50.
//
//  The rules are the media loader's (`MediaStore.resolve`), so that a link and
//  an image written the same way reach the same file: `/` is the root of the
//  file system, anything else is relative to `documentDirectory`.

import Foundation

public enum LinkDestination {
    /// The URL following `destination` opens, or nil when there is none: a
    /// relative path with no directory to resolve it against, or an empty one.
    ///
    /// A path's `#fragment` and `?query` are dropped — they name a place in the
    /// file, not a file — and its percent-escapes are decoded, since Markdown
    /// writes `my%20draft.md` for a name with a space in it.
    public static func url(for destination: String, relativeTo directory: URL?) -> URL? {
        let trimmed = destination.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if let url = URL(string: trimmed), let scheme = url.scheme, !scheme.isEmpty {
            return url
        }
        let path = trimmed.prefix { $0 != "#" && $0 != "?" }
        guard !path.isEmpty else { return nil }
        let decoded = String(path).removingPercentEncoding ?? String(path)
        if decoded.hasPrefix("/") { return URL(fileURLWithPath: decoded).standardizedFileURL }
        guard let directory else { return nil }
        return URL(fileURLWithPath: decoded, relativeTo: directory).standardizedFileURL
    }

    /// Whether `url` is worth handing to the system: anything that isn't a
    /// file, or a file that is there. A link to a file that has gone is a
    /// beep, not a system dialog.
    static func isOpenable(_ url: URL) -> Bool {
        !url.isFileURL || FileManager.default.fileExists(atPath: url.path)
    }
}
