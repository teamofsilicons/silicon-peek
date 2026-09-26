import PeekCore
import SwiftUI

/// Settings › Testing: testing environments in use and "Show test peeks" (gap-testing §9.3, BLUEPRINT §2.9).
struct TestingEnvironmentsView: View {
    let model: SettingsModel

    var body: some View {
        Form {
            Section {
                Toggle(
                    "Show test peeks",
                    isOn: Binding(get: { model.settings.showTestPeeks }, set: { model.setShowTestPeeks($0) }))
                SettingsFootnote(
                    model.settings.showTestPeeks
                        ? "Bubbles from testing environments appear with a TEST pill and a dashed ring. A production peek "
                            + "always takes the position first; the test bubble waits with its ask still pending."
                        : "Bubbles from testing environments wait in peekd, silently, until you turn this back on. "
                            + "Production peeks are not affected.")
            }

            Section("Environments in use") {
                let environments = model.testingEnvironments
                if environments.isEmpty {
                    SettingsFootnote(
                        "No Silicon holds a position in a testing environment. A Silicon joins one with "
                            + "`peek --test <ENV_UUID> register side <1-8>`; it then appears here with the environment's name.")
                } else {
                    ForEach(environments) { environment in
                        TestingEnvironmentRow(environment: environment)
                    }
                }
            }

            Section {
                SettingsFootnote(
                    "Testing environments use isolated identities with connected services: answers typed into a test "
                        + "bubble reach the test Silicon through Ting. Simulation is different: isolated local data, "
                        + "no live services, labelled SIMULATION.")
            }

            SettingsErrorBanner(model: model)
        }
        .formStyle(.grouped)
    }
}

private struct TestingEnvironmentRow: View {
    let environment: TestingEnvironmentSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(environment.pillText)
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(.orange.opacity(0.22), in: Capsule())
                    .overlay(Capsule().strokeBorder(.orange.opacity(0.6), style: StrokeStyle(lineWidth: 1, dash: [3, 2])))
                Spacer()
                if let generation = environment.generation {
                    Text("generation \(generation)").font(.caption).foregroundStyle(.secondary)
                }
            }
            Text(environment.id)
                .font(.system(.caption, design: .monospaced))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
            ForEach(environment.occupants) { occupant in
                HStack(spacing: 6) {
                    Text(occupant.displayName)
                    Text("\(occupant.actorID)[\(occupant.orgID)]").foregroundStyle(.secondary)
                    Spacer()
                    Text("position \(occupant.slot.rawValue) · \(occupant.slot.side.rawValue)")
                        .foregroundStyle(.secondary)
                }
                .font(.callout)
            }
        }
        .padding(.vertical, 2)
        .help("Testing environment \(environment.id)" + (environment.generation.map { ", generation \($0)" } ?? ""))
    }
}
