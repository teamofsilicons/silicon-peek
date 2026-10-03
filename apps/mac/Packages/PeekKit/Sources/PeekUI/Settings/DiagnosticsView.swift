import AppKit
import PeekCore
import PeekDrawing
import PeekIPC
import SwiftUI

/// Settings › Diagnostics: versions, the socket, settings.json and the end of peekd.log.
struct DiagnosticsView: View {
    let model: SettingsModel
    let diagnostics: DiagnosticsModel
    @State private var copied = false

    private var controls: any PeekControlling { model.controls }

    var body: some View {
        Form {
            Section("Versions") {
                LabeledContent("Peek.app", value: diagnostics.appInfo.versionText)
                if let bundleID = diagnostics.appInfo.bundleIdentifier {
                    LabeledContent("Bundle", value: bundleID + (diagnostics.appInfo.isDevelopmentBuild ? " (development)" : ""))
                }
                LabeledContent("peekd", value: peekdVersion)
                LabeledContent("macOS", value: ProcessInfo.processInfo.operatingSystemVersionString)
                LabeledContent("QuickJS", value: QuickJSInfo.version)
                LabeledContent("Glass", value: glassText)
            }

            Section("Connection") {
                SettingsFootnote(controls.linkState.statusSentence)
                PathRow(title: "Socket", path: diagnostics.socketPath, revealable: false)
                LabeledContent("Microphone") {
                    Text(controls.mic.permission.diagnosticsText)
                        .multilineTextAlignment(.trailing)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Section("Settings file") {
                PathRow(title: "settings.json", path: diagnostics.paths.settingsFile.path, revealable: true)
                ForEach(controls.settingsWarnings, id: \.self) { warning in
                    Label(warning, systemImage: "exclamationmark.triangle").font(.callout).foregroundStyle(.orange)
                }
                if let problem = controls.lastProblem {
                    Label(problem, systemImage: "exclamationmark.octagon").font(.callout).foregroundStyle(.red)
                        .textSelection(.enabled)
                }
            }

            Section {
                PathRow(title: "peekd.log", path: diagnostics.peekdLogURL.path, revealable: true)
                logBody
                HStack {
                    Button("Refresh") { Task { await diagnostics.refreshLog() } }
                    Button(copied ? "Copied" : "Copy Diagnostics") {
                        let pasteboard = NSPasteboard.general
                        pasteboard.clearContents()
                        pasteboard.setString(diagnostics.report(controls: controls), forType: .string)
                        copied = true
                    }
                    Spacer()
                    if let loadedAt = diagnostics.loadedAt {
                        Text("read \(loadedAt.formatted(date: .omitted, time: .standard))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            } header: {
                Text("peekd log")
            } footer: {
                SettingsFootnote("Paste the copied diagnostics into `peek report` when something goes wrong.")
            }
        }
        .formStyle(.grouped)
        .peekSettingsPane("Diagnostics", subtitle: "Connection, permissions, and recent activity in one place.")
        .task { await diagnostics.refreshLog() }
    }

    private var peekdVersion: String {
        if case .connected(let hello) = controls.linkState {
            return "\(hello.peekdVersion ?? "version not reported") (protocol \(hello.protocolVersion))"
        }
        return "not connected"
    }

    private var glassText: String {
        switch controls.drawing.glassMode {
        case .live: "Liquid Glass (live)"
        case .frosted: "frosted fallback: this macOS lacks the private active-appearance override"
        }
    }

    @ViewBuilder private var logBody: some View {
        switch diagnostics.log {
        case .notLoaded, .loading:
            ProgressView().controlSize(.small).frame(maxWidth: .infinity)
        case .failed(.missing):
            // The path is already shown above; say what a missing log means.
            SettingsFootnote(
                "No log yet. peekd creates peekd.log when it starts; if Peek shows \"not connected\", peekd has not "
                    + "started (see Settings › Startup).")
        case .failed(let failure):
            SettingsFootnote(failure.description)
        case .loaded(let tail):
            if tail.lines.isEmpty {
                SettingsFootnote("peekd.log is empty.")
            } else {
                ScrollViewReader { proxy in
                    ScrollView([.vertical, .horizontal]) {
                        Text(tail.text)
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(6)
                            .id("tail")
                    }
                    .frame(height: 220)
                    .background(.background.secondary, in: RoundedRectangle(cornerRadius: 6))
                    .onAppear { proxy.scrollTo("tail", anchor: .bottom) }
                }
                if tail.truncated {
                    SettingsFootnote(
                        "Showing the last \(tail.lines.count) lines of \(ByteCountFormatter.string(fromByteCount: Int64(tail.fileSize), countStyle: .file)).")
                }
            }
        }
    }
}

private struct PathRow: View {
    let title: String
    let path: String
    let revealable: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(title)
                Spacer()
                if revealable {
                    Button {
                        let url = URL(fileURLWithPath: path)
                        if FileManager.default.fileExists(atPath: path) {
                            NSWorkspace.shared.activateFileViewerSelecting([url])
                        } else {
                            NSWorkspace.shared.open(url.deletingLastPathComponent())
                        }
                    } label: {
                        Image(systemName: "magnifyingglass")
                    }
                    .buttonStyle(.borderless)
                    .help("Show in Finder")
                }
            }
            Text(path)
                .font(.system(.callout, design: .monospaced))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
