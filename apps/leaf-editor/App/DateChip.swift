import CoreText
import Foundation
import LeafFFI
import LeafUI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// The demo's date chip: `:date[2026-10-12]`, an inline directive, drawn as a
/// pill and edited in place by a picture session leaf drives — the second
/// client of the public picture session, after leaf's own math editor. The web
/// demo's `date-chip.js`, in AppKit and UIKit.
///
/// Open, ← and → move between the year, the month and the day, and leave at
/// the ends; ↑ and ↓ change the field under the caret; digits overwrite it;
/// a calendar opens at the caret and follows the date, and picking a day
/// writes it and leaves. Everything writes back as splices on the label.
/// See docs/proposals/embedded-editors.md.
enum DateChip {
    static let kind = "directive:date"

    /// The closed chip: its date in a pill, or a label that is no date in a
    /// red one.
    static let renderer = RegionRenderer { region, context in
        let date = ChipDate(region.label)
        return pill(date?.text ?? region.label, size: context.size, ink: context.ink.cgColor,
                    fault: date == nil).picture
    }

    static let editor = RegionEditor { region, entry, host in
        DateSession(region, entry: entry, host: host)
    }

    /// The pill's inset around its text, in em.
    static let inset: CGFloat = 0.35

    /// A pill around `text` at `size` points per em, and where each of its
    /// characters starts, in points from the pill's left.
    static func pill(_ text: String, size: CGFloat, ink: CGColor, fault: Bool = false)
        -> (picture: RegionPicture, offsets: [CGFloat], width: CGFloat) {
        let font = CTFontCreateUIFontForLanguage(.system, size * 0.92, nil)
            ?? CTFontCreateWithName("Helvetica" as CFString, size * 0.92, nil)
        let colour = fault ? CGColor(red: 0.8, green: 0.2, blue: 0.2, alpha: 1) : ink
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): colour,
        ]))
        var ascent: CGFloat = 0, descent: CGFloat = 0
        let textWidth = CGFloat(CTLineGetTypographicBounds(line, &ascent, &descent, nil))
        let pad = size * inset
        let width = (textWidth + pad * 2).rounded(.up)
        let height = (ascent + descent + size * 0.3).rounded(.up)
        let baseline = (height - (ascent + descent)) / 2 + ascent
        let offsets = (0...text.utf16.count).map { pad + CTLineGetOffsetForStringIndex(line, $0, nil) }
        let picture = RegionPicture(width: width, height: height, baseline: baseline) { ctx, rect in
            let sx = rect.width / max(1, width), sy = rect.height / max(1, height)
            ctx.saveGState()
            ctx.translateBy(x: rect.minX, y: rect.minY)
            ctx.scaleBy(x: sx, y: sy)
            let box = CGRect(x: 0.5, y: 0.5, width: width - 1, height: height - 1)
            let path = CGPath(roundedRect: box, cornerWidth: height / 2, cornerHeight: height / 2, transform: nil)
            ctx.addPath(path)
            ctx.setFillColor(colour.copy(alpha: 0.1) ?? colour)
            ctx.fillPath()
            ctx.addPath(path)
            ctx.setStrokeColor(colour.copy(alpha: 0.45) ?? colour)
            ctx.setLineWidth(1)
            ctx.strokePath()
            // Core Text sets its line y-up.
            ctx.textMatrix = .identity
            ctx.translateBy(x: pad, y: baseline)
            ctx.scaleBy(x: 1, y: -1)
            ctx.textPosition = .zero
            CTLineDraw(line, ctx)
            ctx.restoreGState()
        }
        return (picture, offsets, width)
    }
}

/// A date as the chip reads and writes it: `YYYY-MM-DD`.
struct ChipDate: Equatable {
    var year: Int
    var month: Int
    var day: Int

    init?(_ label: String) {
        let parts = label.split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 3, parts[0].count == 4, parts[1].count == 2, parts[2].count == 2,
              let y = Int(parts[0]), let m = Int(parts[1]), let d = Int(parts[2]),
              (1...12).contains(m), (1...ChipDate.days(y, m)).contains(d)
        else { return nil }
        (year, month, day) = (y, m, d)
    }

    init(year: Int, month: Int, day: Int) {
        (self.year, self.month, self.day) = (year, month, day)
    }

    var text: String { String(format: "%04d-%02d-%02d", year, month, day) }

    static func days(_ year: Int, _ month: Int) -> Int {
        var c = DateComponents()
        c.year = year
        c.month = month
        let cal = Calendar(identifier: .gregorian)
        guard let date = cal.date(from: c), let range = cal.range(of: .day, in: .month, for: date) else { return 31 }
        return range.count
    }

    /// The day clamped into its month.
    mutating func settle() {
        month = min(max(month, 1), 12)
        day = min(max(day, 1), Self.days(year, month))
    }

    var spoken: String {
        let f = DateFormatter()
        f.dateStyle = .long
        return date.map(f.string(from:)) ?? text
    }

    var date: Date? {
        var c = DateComponents()
        (c.year, c.month, c.day) = (year, month, day)
        return Calendar(identifier: .gregorian).date(from: c)
    }

    init?(date: Date) {
        let c = Calendar(identifier: .gregorian).dateComponents([.year, .month, .day], from: date)
        guard let y = c.year, let m = c.month, let d = c.day else { return nil }
        self.init(year: y, month: m, day: d)
    }
}

/// An open chip.
final class DateSession: RegionPictureSession {
    private enum Field: Int, CaseIterable {
        case year, month, day
        /// Its characters in `YYYY-MM-DD`.
        var range: Range<Int> { [0..<4, 5..<7, 8..<10][rawValue] }
    }

    private let host: RegionHost
    private var date: ChipDate
    private var field: Field
    /// The digits typed into the field so far, overwriting it.
    private var typed = ""
    /// Where the label sits in the directive's source, `:date[` … `]`.
    private var labelRange: Range<Int>
    private var popover: CalendarPopover?
    private var shownSize: CGFloat = 16

    init?(_ region: RegionView, entry: RegionEntry, host: RegionHost) {
        guard let date = ChipDate(region.label), let range = Self.label(in: region.content) else { return nil }
        self.host = host
        self.date = date
        labelRange = range
        switch entry {
        case .home: field = .year
        case .end: field = .day
        case let .point(p, size):
            let offsets = DateChip.pill(date.text, size: size, ink: CGColor(gray: 0, alpha: 1)).offsets
            let i = offsets.lastIndex { $0 <= p.x } ?? 0
            field = i < 5 ? .year : i < 8 ? .month : .day
        }
    }

    /// The UTF-8 range of the label in `content`, between `[` and `]`.
    private static func label(in content: String) -> Range<Int>? {
        let bytes = Array(content.utf8)
        guard let open = bytes.firstIndex(of: UInt8(ascii: "[")),
              let close = bytes[open...].firstIndex(of: UInt8(ascii: "]"))
        else { return nil }
        return (open + 1)..<close
    }

    func focus() {
        // The popover is opened once the chip has been laid out, so it has a
        // caret to stand at.
        DispatchQueue.main.async { [weak self] in self?.showCalendar() }
    }

    func picture(size: CGFloat, ink: LeafColor) -> RegionSessionPicture? {
        shownSize = size
        let (picture, offsets, _) = DateChip.pill(date.text, size: size, ink: ink.cgColor)
        let r = field.range
        let top = picture.height * 0.12, h = picture.height * 0.76
        let selected = CGRect(x: offsets[r.lowerBound], y: top, width: offsets[r.upperBound] - offsets[r.lowerBound],
                              height: h)
        return RegionSessionPicture(picture: picture, caret: CGRect(x: offsets[r.upperBound], y: top, width: 0, height: h),
                                    selection: [selected])
    }

    func input(_ input: RegionInput) -> RegionOutcome {
        typed = ""
        switch input {
        case .left, .wordLeft:
            guard let before = Field(rawValue: field.rawValue - 1) else { return .leave(atEnd: false) }
            field = before
            return moved()
        case .right, .wordRight:
            guard let after = Field(rawValue: field.rawValue + 1) else { return .leave(atEnd: true) }
            field = after
            return moved()
        case .lineStart, .home:
            field = .year
            return moved()
        case .lineEnd, .end:
            field = .day
            return moved()
        case .up: return step(1)
        case .down: return step(-1)
        case .cancel, .newline, .tab: return .leave(atEnd: true)
        default: return .pass
        }
    }

    /// Digits overwrite the field: the first replaces it, later ones fill it,
    /// and a full field moves on to the next. Anything else is not the chip's.
    func type(_ text: String) -> RegionOutcome {
        var splices: [RegionSplice] = []
        for c in text {
            guard let digit = c.wholeNumberValue, c.isASCII else { return splices.isEmpty ? .pass : .edited(splices) }
            typed.append(Character(String(digit)))
            let value = Int(typed) ?? 0
            switch field {
            case .year: date.year = value
            case .month: date.month = max(value, 1)
            case .day: date.day = max(value, 1)
            }
            let full = typed.count == field.range.count
                || (field == .month && value > 1) || (field == .day && value > 3)
            if full {
                typed = ""
                if let next = Field(rawValue: field.rawValue + 1) { field = next }
            }
            if field != .year || full { date.settle() }
            splices += write()
        }
        return .edited(splices)
    }

    func click(at point: CGPoint, size: CGFloat) -> RegionOutcome {
        typed = ""
        let offsets = DateChip.pill(date.text, size: size, ink: CGColor(gray: 0, alpha: 1)).offsets
        let i = offsets.lastIndex { $0 <= point.x } ?? 0
        field = i < 5 ? .year : i < 8 ? .month : .day
        return moved()
    }

    func reload(content: String) -> Bool {
        guard let range = Self.label(in: content) else { return false }
        let label = String(decoding: Array(content.utf8)[range], as: UTF8.self)
        guard let date = ChipDate(label) else { return false }
        labelRange = range
        self.date = date
        popover?.show(date)
        return true
    }

    func copy() -> String? { date.text }

    var accessibilityValue: String? { date.spoken }

    func close() {
        popover?.close()
        popover = nil
    }

    private func moved() -> RegionOutcome {
        popover?.reposition()
        return .moved
    }

    private func step(_ by: Int) -> RegionOutcome {
        switch field {
        case .year: date.year += by
        case .month: date.month = (date.month - 1 + by + 12) % 12 + 1
        case .day:
            let n = ChipDate.days(date.year, date.month)
            date.day = (date.day - 1 + by + n) % n + 1
        }
        date.settle()
        return .edited(write())
    }

    /// The label rewritten as the date now stands.
    private func write() -> [RegionSplice] {
        let text = date.text
        let splice = RegionSplice(from: UInt32(labelRange.lowerBound), to: UInt32(labelRange.upperBound), text: text)
        labelRange = labelRange.lowerBound..<(labelRange.lowerBound + text.utf8.count)
        popover?.show(date)
        return [splice]
    }

    private func showCalendar() {
        guard popover == nil, let view = host.view, host.caretRect() != nil else { return }
        let popover = CalendarPopover(date: date, host: host, in: view) { [weak self] picked in
            guard let self else { return }
            self.date = picked
            self.host.edit(self.write())
            self.host.leave(atEnd: true)
        }
        self.popover = popover
        popover.reposition()
    }
}

#if canImport(AppKit)
/// A month to pick a day from, in a popover at the chip's caret. It never
/// takes the keyboard: the keys stay the chip's.
final class CalendarPopover: NSObject {
    private let popover = NSPopover()
    private let picker = NSDatePicker()
    private let host: RegionHost
    private weak var view: NSView?
    private let picked: (ChipDate) -> Void
    private var showing: ChipDate

    init(date: ChipDate, host: RegionHost, in view: NSView, picked: @escaping (ChipDate) -> Void) {
        self.host = host
        self.view = view
        self.picked = picked
        showing = date
        super.init()
        picker.datePickerStyle = .clockAndCalendar
        picker.datePickerElements = .yearMonthDay
        picker.dateValue = date.date ?? Date()
        picker.refusesFirstResponder = true
        picker.target = self
        picker.action = #selector(choose(_:))
        picker.sizeToFit()
        let controller = NSViewController()
        controller.view = picker
        popover.contentViewController = controller
        popover.behavior = .semitransient
        popover.animates = false
        picker.setAccessibilityIdentifier("date-chip-calendar")
    }

    /// Show the popover at the caret, or move it there.
    func reposition() {
        guard let view, let caret = host.caretRect() else { return }
        if popover.isShown {
            popover.positioningRect = caret
        } else {
            popover.show(relativeTo: caret, of: view, preferredEdge: .maxY)
            // The text view keeps the keys.
            view.window?.makeFirstResponder(view)
        }
    }

    func show(_ date: ChipDate) {
        showing = date
        if let d = date.date { picker.dateValue = d }
        reposition()
    }

    func close() { popover.close() }

    @objc private func choose(_ sender: NSDatePicker) {
        guard let date = ChipDate(date: sender.dateValue), date != showing else { return }
        picked(date)
    }
}
#elseif canImport(UIKit)
/// A month to pick a day from, in a popover at the chip's caret.
final class CalendarPopover: NSObject, UIPopoverPresentationControllerDelegate {
    private let controller = UIViewController()
    private let picker = UIDatePicker()
    private let host: RegionHost
    private weak var view: UIView?
    private let picked: (ChipDate) -> Void
    private var showing: ChipDate

    init(date: ChipDate, host: RegionHost, in view: UIView, picked: @escaping (ChipDate) -> Void) {
        self.host = host
        self.view = view
        self.picked = picked
        showing = date
        super.init()
        picker.datePickerMode = .date
        picker.preferredDatePickerStyle = .inline
        picker.date = date.date ?? Date()
        picker.addTarget(self, action: #selector(choose(_:)), for: .valueChanged)
        controller.view = picker
        controller.preferredContentSize = picker.sizeThatFits(CGSize(width: 320, height: 400))
        controller.modalPresentationStyle = .popover
    }

    func reposition() {
        guard let view, let caret = host.caretRect() else { return }
        if let presentation = controller.popoverPresentationController, controller.presentingViewController != nil {
            presentation.sourceRect = caret
            return
        }
        guard let root = view.window?.rootViewController else { return }
        let top = sequence(first: root) { $0.presentedViewController }.reversed().first ?? root
        controller.popoverPresentationController?.sourceView = view
        controller.popoverPresentationController?.sourceRect = caret
        controller.popoverPresentationController?.delegate = self
        top.present(controller, animated: false)
    }

    func adaptivePresentationStyle(for controller: UIPresentationController,
                                   traitCollection: UITraitCollection) -> UIModalPresentationStyle { .none }

    func show(_ date: ChipDate) {
        showing = date
        if let d = date.date { picker.date = d }
    }

    func close() { controller.dismiss(animated: false) }

    @objc private func choose(_ sender: UIDatePicker) {
        guard let date = ChipDate(date: sender.date), date != showing else { return }
        picked(date)
    }
}
#endif
