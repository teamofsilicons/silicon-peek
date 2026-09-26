import AppKit
import Combine
import PeekCore
import SwiftUI

/// The Settings scene content (BLUEPRINT §8.11): General, Voice, Testing, Startup, Diagnostics.
///
/// Every change goes through ``SettingsModel`` → ``PeekControlling/setSetting(_:_:)``, which writes
/// `~/Library/Application Support/Peek/settings.json` and sends peekd `settings.changed`.
public struct SettingsRootView: View {
    private let coordinator: PeekCoordinator
    @State private var model: SettingsModel
    @State private var diagnostics: DiagnosticsModel

    public init(coordinator: PeekCoordinator) {
        self.coordinator = coordinator
        _model = State(initialValue: SettingsModel(controls: coordinator))
        _diagnostics = State(initialValue: DiagnosticsModel(paths: coordinator.paths))
    }

    public var body: some View {
        TabView {
            Tab("General", systemImage: "gearshape") {
                GeneralSettingsView(model: model, openSimulation: openSimulation)
            }
            Tab("Voice", systemImage: "waveform") {
                VoiceSettingsView(model: model)
            }
            Tab("Testing", systemImage: "testtube.2") {
                TestingEnvironmentsView(model: model)
            }
            Tab("Startup", systemImage: "power") {
                StartupSettingsView(model: model)
            }
            Tab("Diagnostics", systemImage: "stethoscope") {
                DiagnosticsView(model: model, diagnostics: diagnostics)
            }
        }
        .frame(width: 560)
        .frame(minHeight: 460, idealHeight: 640)
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
            .font(.callout)
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
