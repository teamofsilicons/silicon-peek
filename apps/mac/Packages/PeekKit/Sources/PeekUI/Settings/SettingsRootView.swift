import AppKit
import Combine
import PeekCore
import SwiftUI

/// The Settings scene content, including account-scoped feature permissions through peekd.
///
/// Preferences go through ``SettingsModel`` → ``PeekControlling/setSetting(_:_:)``.
/// Account-scoped feature approvals use local IPC through ``PermissionsModel``; peekd owns credentials.
public struct SettingsRootView: View {
    private let coordinator: PeekCoordinator
    @State private var model: SettingsModel
    @State private var diagnostics: DiagnosticsModel
    @State private var permissions: PermissionsModel

    public init(coordinator: PeekCoordinator) {
        self.coordinator = coordinator
        _model = State(initialValue: SettingsModel(controls: coordinator))
        _diagnostics = State(initialValue: DiagnosticsModel(paths: coordinator.paths))
        _permissions = State(initialValue: PermissionsModel(link: coordinator.link))
    }

    public var body: some View {
        TabView {
            Tab("General", systemImage: "gearshape") {
                GeneralSettingsView(model: model, openSimulation: openSimulation)
            }
            Tab("Voice", systemImage: "waveform") {
                VoiceSettingsView(model: model)
            }
            Tab("Permissions", systemImage: "person.badge.key") {
                PermissionsView(model: permissions)
            }
            Tab("Startup", systemImage: "power") {
                StartupSettingsView(model: model)
            }
            Tab("Diagnostics", systemImage: "stethoscope") {
                DiagnosticsView(model: model, diagnostics: diagnostics)
            }
        }
        .frame(width: 620)
        .controlSize(.large)
        .buttonBorderShape(.capsule)
        .frame(minHeight: 520, idealHeight: 680)
        .onAppear { model.refreshSystemStatus() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            // Permissions and login items change in System Settings while Peek is in the background.
            model.refreshSystemStatus()
        }
    }

    private func openSimulation() {
        SimulationWindowController.show(for: coordinator)
    }
}

/// A footnote under a control: secondary, wrapping, selectable. `code` spans are rendered as code;
/// text without a backtick is shown verbatim (paths and reasons from peekd are never parsed).
struct SettingsFootnote: View {
    let text: String

    init(_ text: String) { self.text = text }

    private var attributed: AttributedString {
        guard text.contains("`") else { return AttributedString(text) }
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }

    var body: some View {
        Text(attributed)
            .font(.system(size: 12))
            .lineSpacing(3)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
    }
}

/// The last rejected change, shown at the bottom of a settings pane.
struct SettingsErrorBanner: View {
    let model: SettingsModel

    var body: some View {
        if let error = model.lastError ?? model.controls.lastProblem {
            Label {
                Text(error).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            } icon: {
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            }
            .font(.callout)
        }
    }
}


/// Native counterpart to the web control rhythm: clear headings, quiet surfaces, and
/// platform controls that keep macOS focus, contrast, keyboard, and reduced-motion behavior.
private struct SettingsPaneStyle: ViewModifier {
    let title: String
    let subtitle: String

    func body(content: Content) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text(title)
                    .font(.system(size: 24, weight: .semibold))
                    .accessibilityAddTraits(.isHeader)
                Text(subtitle)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 28)
            .padding(.top, 24)
            .padding(.bottom, 8)
            content
        }
    }
}

extension View {
    func peekSettingsPane(_ title: String, subtitle: String) -> some View {
        modifier(SettingsPaneStyle(title: title, subtitle: subtitle))
    }
}
