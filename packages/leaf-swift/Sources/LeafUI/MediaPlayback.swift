//  MediaPlayback.swift
//
//  Real playback for a block video or audio: an AVKit player installed over the
//  box `MediaLayout` reserved, in place of the drawn still and play badge.
//
//  ## Installed on activation, not on sight
//
//  A player is created when the reader activates the box, not when the document
//  is laid out. That is the whole reason this stays simple:
//
//    * An `AVPlayer` per media in a long document would be dozens of decoders and
//      file handles for things nobody has asked to watch. Lazily installing means
//      a document costs exactly what is being played.
//    * Nothing needs culling, and so nothing needs to observe scrolling. The
//      players are subviews of the scrolling document view, so their frames live
//      in *its* coordinate space and scrolling moves them for free — the frames
//      only change when the layout does.
//
//  Until then the box draws its poster (or a labelled chip) with a play badge,
//  which is what `BlockChrome.drawMedia` already does and what a reader expects
//  an unplayed video to look like anyway.
//
//  ## Keyed by src, not by row
//
//  An installed player survives edits: rows renumber on every keystroke, but a
//  video's `src` doesn't, so typing above a playing video repositions it rather
//  than tearing it down and restarting playback. A player whose `src` leaves the
//  frame entirely (the line was deleted, the view was switched to source) is
//  removed and its playback stopped.

import AVFoundation
import AVKit
import CoreGraphics
import Foundation
import LeafFFI

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// How the editor answers a reader activating a block video or audio.
public enum MediaPlaybackMode {
    /// Install an AVKit player over the box and play there. The default: a video
    /// in a document should play where it sits.
    case inline
    /// Draw the still and the play badge, and hand the `src` to the host's
    /// `onOpenMedia` instead. For a host that wants to present its own player —
    /// fullscreen, a separate pane, an external window — or that shows media the
    /// editor's own loader can't reach.
    case host
}

/// Owns the AVKit players currently installed over media boxes, keyed by `src`.
///
/// One per text view. The view calls `activate` when the reader taps a box, and
/// `reposition` after every layout so installed players follow their boxes and
/// any whose media has left the frame are removed.
final class MediaPlayerHost {
    /// One installed player: its AVKit view and the item it is playing.
    private struct Installed {
        let player: AVPlayer
        #if canImport(AppKit) && !targetEnvironment(macCatalyst)
        let view: AVPlayerView
        #elseif canImport(UIKit)
        let controller: AVPlayerViewController
        var view: UIView { controller.view }
        #endif
    }

    private var installed: [String: Installed] = [:]

    /// Whether `src` currently has a player installed — the view asks so it can
    /// skip drawing the still and badge underneath one.
    func isPlaying(_ src: String) -> Bool { installed[src] != nil }

    /// Whether anything is installed at all, so a view with no media can skip the
    /// whole reposition pass.
    var isEmpty: Bool { installed.isEmpty }

    // MARK: install / remove

    /// Install a player for `media` over `rect` in `container`, and start it.
    /// Returns false when a player couldn't be installed — on iOS that means no
    /// owning view controller was found to parent it to, and the caller should
    /// fall back to handing the media to the host.
    ///
    /// Re-activating an already-installed media just toggles play/pause, so a
    /// second tap on a playing video does the obvious thing.
    @discardableResult
    func activate(_ media: MediaView, at rect: CGRect, in container: LeafView, url: URL) -> Bool {
        if let existing = installed[media.src] {
            // Already installed: treat the tap as a transport control.
            if existing.player.timeControlStatus == .paused {
                existing.player.play()
            } else {
                existing.player.pause()
            }
            return true
        }

        let player = AVPlayer(url: url)

        #if canImport(AppKit) && !targetEnvironment(macCatalyst)
        let view = AVPlayerView()
        view.player = player
        view.controlsStyle = .inline
        // The box is already rounded where the still was drawn; match it so
        // installing a player doesn't change the block's silhouette.
        view.wantsLayer = true
        view.layer?.cornerRadius = MediaMetrics.corner
        view.layer?.masksToBounds = true
        view.frame = rect
        container.addSubview(view)
        installed[media.src] = Installed(player: player, view: view)
        #elseif canImport(UIKit)
        // AVPlayerViewController must be a child of a real view controller — it
        // installs gesture recognisers and manages its own presentation. A UIView
        // inside a package has no controller of its own, so find the one that
        // actually owns it by walking the responder chain. Any real host has one
        // (a UIHostingController, at minimum); if somehow none does, say so
        // rather than adding an orphaned controller's view and hoping.
        guard let parent = container.owningViewController else { return false }
        let controller = AVPlayerViewController()
        controller.player = player
        controller.showsPlaybackControls = true
        // The editor scrolls; a player that could go fullscreen from a gesture
        // would fight the scroll view for the same drags.
        controller.videoGravity = .resizeAspect
        controller.view.frame = rect
        controller.view.layer.cornerRadius = MediaMetrics.corner
        controller.view.layer.masksToBounds = true
        controller.view.backgroundColor = .clear
        parent.addChild(controller)
        container.addSubview(controller.view)
        controller.didMove(toParent: parent)
        installed[media.src] = Installed(player: player, controller: controller)
        #endif

        player.play()
        return true
    }

    /// Move every installed player onto its box's current rect, and remove any
    /// whose media is no longer in the frame.
    ///
    /// `rects` is the current frame's media boxes keyed by `src`; anything
    /// installed and absent from it has been edited away or scrolled out of the
    /// document entirely (a view switch to source), and its playback stops.
    func reposition(_ rects: [String: CGRect]) {
        for (src, entry) in installed {
            guard let rect = rects[src] else {
                remove(src)
                continue
            }
            entry.view.frame = rect
        }
    }

    /// Stop and remove the player for `src`.
    func remove(_ src: String) {
        guard let entry = installed.removeValue(forKey: src) else { return }
        entry.player.pause()
        #if canImport(AppKit) && !targetEnvironment(macCatalyst)
        entry.view.player = nil
        entry.view.removeFromSuperview()
        #elseif canImport(UIKit)
        entry.controller.willMove(toParent: nil)
        entry.controller.view.removeFromSuperview()
        entry.controller.removeFromParent()
        entry.controller.player = nil
        #endif
    }

    /// Stop and remove everything — a document swap, or the view going away.
    func removeAll() {
        for src in installed.keys { remove(src) }
    }

    deinit {
        // `removeAll` touches UIKit/AppKit views, which must happen on the main
        // thread; a text view is deallocated there, so this is already the main
        // thread in every real path.
        removeAll()
    }
}

#if canImport(UIKit) && !os(macOS)
extension UIResponder {
    /// The nearest view controller up the responder chain — the one that really
    /// owns this view, and so the right parent for a child `AVPlayerViewController`.
    var owningViewController: UIViewController? {
        var responder: UIResponder? = self
        while let current = responder {
            if let controller = current as? UIViewController { return controller }
            responder = current.next
        }
        return nil
    }
}
#endif
