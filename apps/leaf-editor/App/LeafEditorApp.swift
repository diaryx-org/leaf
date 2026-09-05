import SwiftUI
import LeafUI

@main
struct LeafEditorApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
                #if os(macOS)
                .frame(minWidth: 480, idealWidth: 720, minHeight: 320, idealHeight: 640)
                #endif
        }
        // Format and View menus aimed at whichever editor the window shows —
        // `LeafEditor` publishes itself as the scene's focused editor.
        .commands { LeafEditorCommands() }
    }
}
