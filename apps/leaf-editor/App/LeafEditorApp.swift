import SwiftUI

@main
struct LeafEditorApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
                #if os(macOS)
                .frame(minWidth: 480, idealWidth: 720, minHeight: 320, idealHeight: 640)
                #endif
        }
    }
}
