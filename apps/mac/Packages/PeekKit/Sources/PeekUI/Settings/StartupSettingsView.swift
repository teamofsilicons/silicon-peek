import SwiftUI

/// Settings › Startup: launch-at-login status for Peek.app and peekd, with the approval hint (§1.8).
struct StartupSettingsView: View {
    let model: SettingsModel

    var body: some View {
        Form {
            if let status = model.loginStatus {
                Section("Peek.app") {
                    StatusRow(title: "Open at login", state: status.app, summary: status.appSummary)
                }
                Section("Background helper (peekd)") {
                    StatusRow(title: "Run in background", state: status.helper, summary: status.helperSummary)
                }
                Section {
                    if status.needsApproval {
                        SettingsFootnote(
                            "macOS asks you once to allow Peek's background items. Open Login Items & Extensions and turn "
                                + "Peek on; Peek notices the change the next time it becomes active.")
                    } else {
                        SettingsFootnote(
                            "To stop Peek from opening at login, turn it off in System Settings › General › Login Items & "
                                + "Extensions. Silicons' peeks then wait until you open Peek.")
                    }
                    HStack {
                        Button("Open Login Items…") { model.loginItems.openLoginItemsSettings() }
                        Button("Refresh") { model.refreshSystemStatus() }
                    }
                }
            } else {
                ProgressView().frame(maxWidth: .infinity)
            }
        }
        .formStyle(.grouped)
        .peekSettingsPane("Ready when you are", subtitle: "Manage Peek at login and its background service.")
        .onAppear { model.refreshSystemStatus() }
    }
}

private struct StatusRow: View {
    let title: String
    let state: ServiceRegistrationState
    let summary: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            LabeledContent(title) {
                Label(label, systemImage: symbol)
                    .foregroundStyle(color)
            }
            SettingsFootnote(summary)
        }
    }

    private var label: String {
        switch state {
        case .enabled: "On"
        case .requiresApproval: "Needs approval"
        case .notRegistered: "Off"
        case .notFound: "Unavailable"
        case .disabledForThisRun: "Isolated run"
        case .unknown: "Unknown"
        }
    }

    private var symbol: String {
        switch state {
        case .enabled: "checkmark.circle.fill"
        case .requiresApproval: "exclamationmark.circle.fill"
        case .notRegistered, .notFound, .unknown: "minus.circle"
        case .disabledForThisRun: "flask"
        }
    }

    private var color: Color {
        switch state {
        case .enabled: .green
        case .requiresApproval: .orange
        case .notRegistered, .notFound, .unknown, .disabledForThisRun: .secondary
        }
    }
}
