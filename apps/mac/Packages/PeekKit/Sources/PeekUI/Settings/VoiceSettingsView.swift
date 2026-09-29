import PeekCore
import SwiftUI

/// Settings › Voice: the default Google voice per language, and the speech-to-text language.
struct VoiceSettingsView: View {
    let model: SettingsModel

    var body: some View {
        Form {
            Section {
                ForEach(VoiceCatalog.languages, id: \.self) { language in
                    VoiceRow(model: model, language: language)
                }
            } header: {
                HStack {
                    Text("Speaking voice")
                    Spacer()
                    if model.hasCustomVoices {
                        Button("Reset All") { model.resetAllVoices() }
                            .buttonStyle(.link)
                            .font(.callout)
                    }
                }
            } footer: {
                SettingsFootnote(
                    "Peek streams speech with Google Gemini. These voices work across languages. A Silicon can customize "
                        + "the voice and delivery through Peek CLI; its voice choice overrides these defaults.")
            }

            Section {
                Picker("Language", selection: Binding(get: { model.sttSelection }, set: { model.selectSTT($0) })) {
                    Text("Automatic (\(STTLanguageChoices.automaticDescription()))").tag(SettingsModel.STTSelection.automatic)
                    Divider()
                    ForEach(STTLanguageChoices.common) { choice in
                        Text("\(choice.name) — \(choice.tag)").tag(SettingsModel.STTSelection.tag(choice.tag))
                    }
                    Divider()
                    Text("Other…").tag(SettingsModel.STTSelection.custom)
                }
                if model.sttSelection == .custom {
                    HStack {
                        TextField(
                            "BCP 47 tag", text: Binding(get: { model.customSTTLanguage }, set: { model.customSTTLanguage = $0 }),
                            prompt: Text("e.g. pt-BR"))
                            .textFieldStyle(.roundedBorder)
                            .onSubmit { model.commitCustomSTTLanguage() }
                        Button("Use") { model.commitCustomSTTLanguage() }
                    }
                    if let problem = model.customSTTProblem {
                        Text(problem).font(.callout).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
                    }
                }
            } header: {
                Text("Listening")
            } footer: {
                SettingsFootnote(
                    "After you stop recording, Peek sends the completed audio through its backend to OpenAI gpt-transcribe. "
                        + "Automatic uses your system languages as hints; OpenAI detects the spoken language. A selected language supplies a hint.")
            }

            SettingsErrorBanner(model: model)
        }
        .formStyle(.grouped)
    }
}

private struct VoiceRow: View {
    let model: SettingsModel
    let language: String

    var body: some View {
        HStack {
            Picker(
                VoiceCatalog.languageName(language),
                selection: Binding(get: { model.voice(for: language) }, set: { model.setVoice($0, for: language) })
            ) {
                ForEach(VoiceCatalog.voices(for: language)) { voice in
                    Text(voice.id == VoiceCatalog.defaultVoice(for: language) ? "\(voice.label) (default)" : voice.label)
                        .tag(voice.id)
                }
                // A valid voice written by hand into settings.json that this build does not list.
                if VoiceCatalog.voice(id: model.voice(for: language)) == nil {
                    Text(model.voice(for: language)).tag(model.voice(for: language))
                }
            }
            if model.isVoiceCustomized(for: language) {
                Button {
                    model.resetVoice(for: language)
                } label: {
                    Image(systemName: "arrow.uturn.backward")
                }
                .buttonStyle(.borderless)
                .help("Use the default voice for \(VoiceCatalog.languageName(language))")
            }
        }
    }
}
