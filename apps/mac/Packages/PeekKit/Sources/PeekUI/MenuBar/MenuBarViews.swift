import AppKit
import PeekCore
import SwiftUI

/// The menu bar icon: the connection and pause state at a glance.
///
/// It is created at launch, so it also carries the `--simulate <scenario>` launch hook
/// (``SimulationLaunchHook``), which presents a Simulation scenario for automated screenshots.
public struct MenuBarLabel: View {
    private let coordinator: PeekCoordinator

    public init(coordinator: PeekCoordinator) {
        self.coordinator = coordinator
        SimulationLaunchHook.scheduleIfRequested(for: coordinator)
    }

    public var body: some View {
        Image(systemName: symbol)
            .accessibilityLabel(accessibilityText)
            .task { SimulationLaunchHook.scheduleIfRequested(for: coordinator) }
    }

    private var symbol: String {
        if coordinator.paused { return "pause.circle" }
        return coordinator.linkState.isConnected ? "circle.circle.fill" : "circle.dashed"
    }

    private var accessibilityText: String {
        if coordinator.paused { return "Peek (paused)" }
        return coordinator.linkState.isConnected ? "Peek" : "Peek (not connected to peekd)"
    }
}

/// The `MenuBarExtra(.window)` content (BLUEPRINT §8.11): the slots with their occupants and
/// hotkeys, pause, Simulation, Settings and Quit.
public struct MenuBarContentView: View {
    private let coordinator: PeekCoordinator
    @Environment(\.openSettings) private var openSettings

    public init(coordinator: PeekCoordinator) { self.coordinator = coordinator }

    private var controls: any PeekControlling { coordinator }

    public var body: some View {
        let summary = MenuBarSummary(
            slots: controls.slots, modifier: controls.settings.hotkeyModifier, showTestPeeks: controls.settings.showTestPeeks)
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 18)
                .padding(.top, 12)
                .padding(.bottom, 8)
            Divider()
            slotList(summary)
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
            Divider()
            pauseSection
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
            Divider()
            VStack(spacing: 0) {
                MenuBarActionButton(title: "Simulation…", symbol: "play.rectangle") {
                    SimulationWindowController.show(for: coordinator)
                }
                MenuBarActionButton(title: "Settings…", symbol: "gearshape", shortcut: "⌘,") {
                    NSApp.activate()
                    openSettings()
                }
                .keyboardShortcut(",", modifiers: .command)
                MenuBarActionButton(title: "Quit Peek", symbol: "power", shortcut: "⌘Q") {
                    NSApp.terminate(nil)
                }
                .keyboardShortcut("q", modifiers: .command)
            }
            .padding(6)
        }
        .frame(width: 336)
        .buttonBorderShape(.capsule)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text("Peek").font(.system(size: 18, weight: .semibold))
                Spacer()
                Circle()
                    .fill(controls.linkState.isConnected ? Color.green : Color.orange)
                    .frame(width: 7, height: 7)
                Text(controls.linkState.shortStatus).font(.caption).foregroundStyle(.secondary)
            }
            if !controls.linkState.isConnected, case .waiting = controls.linkState {
                Text(controls.linkState.statusSentence)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }

    @ViewBuilder
    private func slotList(_ summary: MenuBarSummary) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            if summary.rows.isEmpty {
                Text("No Silicon holds a position yet. A Silicon claims one with `peek register side <1-8>`.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 6)
            } else {
                ForEach(summary.rows) { row in
                    MenuBarSlotRow(row: row)
                }
            }
            HStack {
                Text(summary.freePositionsText)
                Spacer()
                if let tests = summary.testEnvironmentsText { Text(tests) }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.top, 4)
        }
    }

    private var pauseSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            Toggle("Pause all peeks", isOn: Binding(get: { controls.paused }, set: { controls.paused = $0 }))
                .toggleStyle(.switch)
                .controlSize(.small)
            if controls.paused {
                Text("Silicons' peeks wait in peekd until you resume. Nothing is lost.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let problem = controls.lastProblem {
                Label(problem, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }
        }
    }
}

private struct MenuBarSlotRow: View {
    let row: MenuBarSummary.Row

    var body: some View {
        HStack(spacing: 8) {
            SlotPositionGlyph(index: row.index)
                .frame(width: 22, height: 15)
            Text(row.name)
                .lineLimit(1)
                .foregroundStyle(row.muted ? .secondary : .primary)
            if let pill = row.testPill {
                Text(pill)
                    .font(.caption2.weight(.semibold))
                    .lineLimit(1)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(.orange.opacity(0.22), in: Capsule())
                    .help(row.testTooltip ?? pill)
            }
            if !row.hasDrawing {
                Image(systemName: "circle.dotted")
                    .foregroundStyle(.secondary)
                    .help("No drawing registered yet: its bubble shows the fallback circle")
            }
            Spacer(minLength: 4)
            Text(row.hotkey ?? "—")
                .font(.system(.callout, design: .rounded))
                .foregroundStyle(.secondary)
                .help(row.hotkey.map { "Press \($0) to summon this bubble" } ?? "No hotkey for this position")
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 6)
        .opacity(row.muted ? 0.6 : 1)
        .help(row.muted ? "Show test peeks is off: this Silicon's bubbles wait in peekd" : row.actorID)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "Position \(row.index.rawValue), \(row.index.side.rawValue): \(row.name)"
                + (row.testPill.map { ", \($0)" } ?? "") + (row.hotkey.map { ", hotkey \($0)" } ?? ""))
    }
}

/// A tiny screen with a dot at the slot's position.
struct SlotPositionGlyph: View {
    let index: SlotIndex

    var body: some View {
        Canvas { context, size in
            let rect = CGRect(origin: .zero, size: size).insetBy(dx: 0.75, dy: 0.75)
            context.stroke(Path(roundedRect: rect, cornerRadius: 2.5), with: .style(.secondary), lineWidth: 1)
            let point = SlotDiagram.point(for: index, in: rect, inset: 3.2)
            let dot = CGRect(x: point.x - 2.2, y: point.y - 2.2, width: 4.4, height: 4.4)
            context.fill(Path(ellipseIn: dot), with: .style(.tint))
        }
        .accessibilityHidden(true)
    }
}

/// A full-width, menu-item-like button for the MenuBarExtra window.
struct MenuBarActionButton: View {
    let title: String
    let symbol: String
    var shortcut: String?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: symbol).frame(width: 16)
                Text(title)
                Spacer()
                if let shortcut { Text(shortcut).foregroundStyle(.secondary) }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .contentShape(Rectangle())
            .background(hovering ? Color.accentColor.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 5))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}
