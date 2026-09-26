import PeekUI
import SwiftUI

/// Peek.app: an LSUIElement menu-bar app (no Dock icon). Everything else lives in PeekKit;
/// this target only wires the scenes, the app delegate and the service registration.
@main
struct PeekApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        MenuBarExtra {
            MenuBarContentView(coordinator: appDelegate.coordinator)
        } label: {
            MenuBarLabel(coordinator: appDelegate.coordinator)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsRootView(coordinator: appDelegate.coordinator)
        }
    }
}
