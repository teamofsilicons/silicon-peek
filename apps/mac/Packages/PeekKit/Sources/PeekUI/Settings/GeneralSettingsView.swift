import PeekCore
import SwiftUI

/// Settings › General: mode, display, backdrop, hotkeys, telemetry, CLI updates, Simulation.
struct GeneralSettingsView: View {
    let model: SettingsModel
    let openSimulation: () -> Void

    var body: some View {
        Form {
            Section("Bubbles") {
                Picker("Mode", selection: Binding(get: { model.settings.mode }, set: { model.setMode($0) })) {
                    Text("Normal").tag(DisplayMode.normal)
                    Text("Compact").tag(DisplayMode.compact)
                }
                .pickerStyle(.segmented)
                SettingsFootnote(
                    model.settings.mode == .normal
                        ? "Normal: the Silicon's drawing in a circle, with what it shows or asks laid out along an information arc."
                        : "Compact: a small drawing at the left of a straight line, with the show or ask on the right. Takes less space.")

                Picker("Display", selection: Binding(get: { model.settings.display }, set: { model.setDisplay($0) })) {
                    Text("Menu-bar screen").tag(DisplayTarget.main)
                    Text("Screen under the pointer").tag(DisplayTarget.pointer)
                }
                SettingsFootnote(
                    model.settings.display == .main
                        ? "Bubbles appear on the screen that has the menu bar."
                        : "Each bubble appears on the screen the pointer is on when the Silicon sends it.")

                Picker("Backdrop", selection: Binding(get: { model.settings.backdrop }, set: { model.setBackdrop($0) })) {
                    Text("Desktop picture").tag(BackdropSourceSetting.wallpaper)
                    Text("Screen contents (needs Screen Recording)").tag(BackdropSourceSetting.screen)
                }
                if let hint = model.backdropHint { SettingsFootnote(hint) }
                if model.settings.backdrop == .screen, model.screenCaptureGranted == false {
                    HStack {
                        Button("Allow Screen Recording…") { model.requestScreenCapture() }
                        Button("Open Privacy Settings") { model.screenCapture.openSystemSettings() }
                    }
                }
            }

            Section("Keyboard") {
                Picker(
                    "Hotkeys", selection: Binding(get: { model.settings.hotkeyModifier }, set: { model.setHotkeyModifier($0) })
                ) {
                    ForEach(HotkeyModifier.allCases, id: \.self) { modifier in
                        Text(modifier == PeekSettings.defaults.hotkeyModifier
                            ? "\(modifier.symbols)1 … \(modifier.symbols)8 (default)"
                            : "\(modifier.symbols)1 … \(modifier.symbols)8").tag(modifier)
                    }
                }
                SettingsFootnote(
                    "Position 1 is top centre, then clockwise to 8 at top left. A hotkey exists only while a Silicon holds "
                        + "that position. After \(model.settings.hotkeyModifier.symbols)N press \\ to answer by voice, start "
                        + "typing to answer by text, or Esc to slide the bubble back.")
                ForEach(model.controls.hotkeyProblems, id: \.self) { problem in
                    Label(problem, systemImage: "exclamationmark.triangle.fill")
                        .font(.callout)
                        .foregroundStyle(.orange)
                        .accessibilityLabel("Hotkey problem: \(problem)")
                }
            }

            Section("Privacy") {
                Toggle(
                    "Share usage and diagnostics",
                    isOn: Binding(get: { model.settings.telemetry }, set: { model.setTelemetry($0) }))
                SettingsFootnote(
                    "Sends usage events and errors to Team of Silicons through peek's server so problems can be found and fixed. "
                        + "Silicon ids are hashed. Transcripts, typed answers, audio and drawings are never included. "
                        + "Turning this off also clears events peekd has not sent yet.")
            }

            Section("Updates") {
                Toggle(
                    "Keep Silicons' peek CLI up to date",
                    isOn: Binding(get: { model.settings.cliWatchdog }, set: { model.setCLIWatchdog($0) }))
                SettingsFootnote(
                    "Honeycomb updates the CLI every minute. As a fallback, peekd checks hourly and runs "
                        + "`honeycomb update 'peek'` for a Silicon whose CLI has been behind for more than two hours. "
                        + "Peek.app itself always updates through peekd.")
            }

            Section("Simulation") {
                HStack(alignment: .firstTextBaseline) {
                    SettingsFootnote(
                        "Preview bubbles at any position with sample speech, shows and asks, using the real drawing runtime. "
                            + "Nothing is sent to peekd, IAM, Ting, Deepgram or OpenAI.")
                    Spacer()
                    Button("Open Simulation…", action: openSimulation)
                }
            }

            SettingsErrorBanner(model: model)
        }
        .formStyle(.grouped)
    }
}
