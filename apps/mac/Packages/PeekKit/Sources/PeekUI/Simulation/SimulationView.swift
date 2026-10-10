import AppKit
import PeekCore
import SwiftUI

/// The Simulation window (BLUEPRINT §8.11): toggles on the left, what happened on the right.
struct SimulationView: View {
    @Bindable var engine: SimulationEngine

    var body: some View {
        HStack(spacing: 0) {
            SimulationControls(engine: engine)
                .frame(width: 400)
            Divider()
            SimulationOutput(engine: engine)
                .frame(minWidth: 420, maxWidth: .infinity)
        }
        .frame(minWidth: 860, minHeight: 620)
    }
}

// MARK: - Controls

private struct SimulationControls: View {
    @Bindable var engine: SimulationEngine

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    Menu("Load a preset") {
                        ForEach(SimulationPreset.allCases) { preset in
                            Button("\(preset.title)  (\(preset.rawValue))") {
                                var scenario = preset.scenario
                                scenario.position = engine.scenario.position
                                engine.scenario = scenario
                            }
                        }
                    }
                } footer: {
                    SettingsFootnote("Presets keep the position you picked. The same names work at launch: --simulate <name>.")
                }

                Section("Position") {
                    HStack(alignment: .center, spacing: 14) {
                        SimulationPositionPicker(selection: $engine.scenario.position)
                            .frame(width: 176, height: 112)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Position \(engine.scenario.position.rawValue)").font(.headline)
                            Text(engine.scenario.position.side.rawValue).foregroundStyle(.secondary)
                            Stepper(
                                "Position", value: positionNumber, in: 1...8
                            )
                            .labelsHidden()
                        }
                    }
                }

                Section("Speak") {
                    Toggle("Speak sample audio", isOn: $engine.scenario.speak)
                    if engine.scenario.speak {
                        TextField(
                            "What the Silicon says", text: $engine.scenario.speakText,
                            prompt: Text("What the Silicon says"), axis: .vertical
                        )
                        .labelsHidden()
                        .lineLimit(2...4)
                        HStack {
                            Text("\(engine.scenario.speakText.scalarCount)/\(SimulationScenario.maxSpeakScalars)")
                                .font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Button("Use sample text") { engine.scenario.useSampleSpeech() }
                                .buttonStyle(.link)
                        }
                        SettingsFootnote(
                            "Plays synthesized speech-like audio (not a real voice) through the real speech player, so "
                                + "speech.level, progress and done behave as with streamed ElevenLabs audio.")
                    }
                }

                Section("Content") {
                    Picker("Content", selection: $engine.scenario.content) {
                        Text("None").tag(SimulationScenario.Content.none)
                        Text("Show").tag(SimulationScenario.Content.show)
                        Text("Ask").tag(SimulationScenario.Content.ask)
                    }
                    .pickerStyle(.segmented)
                    switch engine.scenario.content {
                    case .none:
                        EmptyView()
                    case .show:
                        Picker("Show", selection: $engine.scenario.showVariant) {
                            ForEach(SimulationScenario.ShowVariant.allCases, id: \.self) { variant in
                                Text(variant.title).tag(variant)
                            }
                        }
                    case .ask:
                        Picker("Ask", selection: $engine.scenario.askType) {
                            Text("Text").tag(AskType.text)
                            Text("Single choice").tag(AskType.singleChoice)
                            Text("Multiple choice").tag(AskType.multipleChoice)
                            Text("Slider").tag(AskType.slider)
                            Text("Range").tag(AskType.range)
                        }
                        Toggle("Images in options", isOn: $engine.scenario.optionImages)
                            .disabled(!(engine.scenario.askType == .singleChoice || engine.scenario.askType == .multipleChoice))
                        Picker("Sample", selection: $engine.scenario.askSample) {
                            ForEach(SimulationScenario.AskSample.allCases, id: \.self) { sample in
                                Text(sample.title).tag(sample)
                            }
                        }
                        .help("Long: a long question and long option labels (cut short, revealed on hover). "
                            + "Stepped: a 0–10 slider with a dot at each step.")
                    }
                }

                Section {
                    Picker("Mode", selection: $engine.scenario.mode) {
                        Text("Normal").tag(DisplayMode.normal)
                        Text("Compact").tag(DisplayMode.compact)
                    }
                    Picker("Appearance", selection: $engine.scenario.appearance) {
                        Text("System").tag(SimulationScenario.AppearanceChoice.system)
                        Text("Light").tag(SimulationScenario.AppearanceChoice.light)
                        Text("Dark").tag(SimulationScenario.AppearanceChoice.dark)
                    }
                    Picker("Backdrop tone", selection: $engine.scenario.backdropTone) {
                        Text("Light").tag(SimulationScenario.BackdropChoice.light)
                        Text("Dark").tag(SimulationScenario.BackdropChoice.dark)
                        Text("Live sample").tag(SimulationScenario.BackdropChoice.live)
                    }
                    Picker("Context", selection: $engine.scenario.context) {
                        Text("Simulation").tag(InputContext.simulation)
                        Text("Production").tag(InputContext.production)
                    }
                } header: {
                    Text("Environment")
                } footer: {
                    SettingsFootnote(
                        "Context is what the drawing reads as input.context. The bubble is always a Simulation bubble.")
                }
                .pickerStyle(.segmented)
            }
            .formStyle(.grouped)

            Divider()
            SimulationActionBar(engine: engine)
                .padding(12)
        }
    }

    private var positionNumber: Binding<Int> {
        Binding(
            get: { engine.scenario.position.rawValue },
            set: { value in
                if let slot = SlotIndex(rawValue: value) { engine.scenario.position = slot }
            })
    }
}

private struct SimulationActionBar: View {
    let engine: SimulationEngine

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let problem = engine.validationError {
                Label(problem, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Button {
                    Task { await engine.simulate() }
                } label: {
                    Label("Simulate", systemImage: "play.fill").frame(minWidth: 110)
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(engine.validationError != nil || engine.status == .preparing)

                Button("Stop") { engine.stop() }
                    .keyboardShortcut(".", modifiers: .command)
                    .disabled(!engine.status.isActive)
                Spacer()
                if engine.status == .preparing { ProgressView().controlSize(.small) }
            }
        }
    }
}

/// A small screen with the 8 positions; click one to choose it.
struct SimulationPositionPicker: View {
    @Binding var selection: SlotIndex

    var body: some View {
        GeometryReader { proxy in
            let rect = CGRect(origin: .zero, size: proxy.size)
            ZStack(alignment: .topLeading) {
                RoundedRectangle(cornerRadius: 8)
                    .fill(.background.secondary)
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator))
                Rectangle()
                    .fill(.quaternary)
                    .frame(width: rect.width - 2, height: 7)
                    .offset(x: 1, y: 1)
                    .clipShape(UnevenRoundedRectangle(topLeadingRadius: 7, topTrailingRadius: 7))
                ForEach(SlotIndex.allCases, id: \.self) { index in
                    let point = SlotDiagram.point(for: index, in: rect.insetBy(dx: 0, dy: 4).offsetBy(dx: 0, dy: 4), inset: 13)
                    Button {
                        selection = index
                    } label: {
                        Text("\(index.rawValue)")
                            .font(.system(size: 10, weight: .semibold, design: .rounded))
                            .frame(width: 20, height: 20)
                            .foregroundStyle(index == selection ? Color.white : Color.primary)
                            .background(Circle().fill(index == selection ? Color.accentColor : Color.secondary.opacity(0.18)))
                    }
                    .buttonStyle(.plain)
                    .position(point)
                    .help("Position \(index.rawValue): \(index.side.rawValue)")
                    .accessibilityLabel("Position \(index.rawValue), \(index.side.rawValue)")
                    .accessibilityAddTraits(index == selection ? .isSelected : [])
                }
            }
        }
    }
}

// MARK: - Output

private struct SimulationOutput: View {
    let engine: SimulationEngine

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SimulationStatusCard(engine: engine)
                .padding(14)
            Divider()
            HStack {
                Text("Log").font(.headline)
                Text("peek.log output, lifecycle and what the bubble sent").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(engine.logText, forType: .string)
                }
                .disabled(engine.log.isEmpty)
                Button("Clear") { engine.clearLog() }
                    .disabled(engine.log.isEmpty)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            SimulationLogList(entries: engine.log)
            Divider()
            Text("Isolated local data · no live services. Nothing is sent to peekd, Silicon Accounts, Ting, Deepgram or OpenAI.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
        }
    }
}

private struct SimulationStatusCard: View {
    let engine: SimulationEngine

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("SIMULATION")
                    .font(.caption.weight(.bold))
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(Color.purple.opacity(0.2), in: Capsule())
                Text(statusText).font(.callout).lineLimit(2).textSelection(.enabled)
            }
            HStack(spacing: 16) {
                LabeledContent("Phase", value: engine.phase?.rawValue ?? "—")
                LabeledContent("Drawing", value: drawingText)
            }
            .font(.callout)
            if let playback = engine.playback {
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(playback.done ? "speech done" : playback.started ? "speaking" : "buffering speech")
                        Spacer()
                        Text("\(Int((playback.progress * 100).rounded()))%").monospacedDigit()
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    ProgressView(value: min(max(playback.progress, 0), 1))
                    HStack(spacing: 6) {
                        Text("level").font(.caption).foregroundStyle(.secondary)
                        GeometryReader { proxy in
                            Capsule().fill(.quaternary)
                                .overlay(alignment: .leading) {
                                    Capsule().fill(Color.accentColor)
                                        .frame(width: proxy.size.width * min(max(playback.level, 0), 1))
                                }
                        }
                        .frame(height: 5)
                    }
                }
            }
        }
    }

    private var statusText: String {
        switch engine.status {
        case .idle: engine.isRunning ? "Ready. Press Simulate (⌘↩)." : "Press Simulate (⌘↩) to present a bubble."
        case .preparing: "Preparing the bubble…"
        case .presenting(let sendID, let slot): "On screen at position \(slot.rawValue) (\(slot.side.rawValue)) · \(sendID)"
        case .closed(let reason): "Bubble closed: \(reason)"
        case .failed(let message): message
        }
    }

    private var drawingText: String {
        switch engine.drawingStatus {
        case nil: "—"
        case .empty?: "not loaded"
        case .loading?: "loading"
        case .ready(let sha)?: "cassette.js (\(sha.prefix(8)))"
        case .fallback(let failure)?: "fallback: \(failure.reason.rawValue)"
        }
    }
}

private struct SimulationLogList: View {
    let entries: [SimulationLogEntry]

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 3) {
                    if entries.isEmpty {
                        Text("Nothing yet. Simulate a bubble, then click it, answer it or dismiss it.")
                            .foregroundStyle(.secondary)
                            .padding(.top, 8)
                    }
                    ForEach(entries) { entry in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(entry.date.formatted(.dateTime.hour().minute().second()))
                                .foregroundStyle(.tertiary)
                            Text(tag(entry.kind))
                                .foregroundStyle(color(entry.kind))
                                .frame(width: 58, alignment: .leading)
                            Text(entry.text)
                                .foregroundStyle(entry.kind == .error ? Color.red : Color.primary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .id(entry.id)
                    }
                }
                .font(.system(size: 11, design: .monospaced))
                .textSelection(.enabled)
                .padding(.horizontal, 14)
                .padding(.bottom, 8)
            }
            .onChange(of: entries.last?.id) { _, last in
                guard let last else { return }
                withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(last, anchor: .bottom) }
            }
        }
    }

    private func tag(_ kind: SimulationLogEntry.Kind) -> String {
        switch kind {
        case .info: "sim"
        case .drawing: "peek.log"
        case .request: "→ peekd"
        case .error: "error"
        }
    }

    private func color(_ kind: SimulationLogEntry.Kind) -> Color {
        switch kind {
        case .info: .secondary
        case .drawing: .purple
        case .request: .blue
        case .error: .red
        }
    }
}
