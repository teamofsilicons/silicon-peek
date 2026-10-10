import SwiftUI

struct PermissionsView: View {
    @Bindable var model: PermissionsModel

    var body: some View {
        Form {
            Section("Account") {
                if model.contexts.isEmpty {
                    SettingsFootnote("Sign in with the Peek CLI to add your Carbon or Silicon account. Use `peek --profile NAME login` to keep another account separate.")
                } else {
                    Picker("Saved login", selection: Binding(get: { model.selectedID }, set: { id in
                        model.select(id)
                        Task { await model.perform(.status) }
                    })) {
                        ForEach(model.contexts) { context in Text(context.label).tag(Optional(context.id)) }
                    }
                    if let context = model.selected {
                        LabeledContent("Signed in as", value: context.actor.publicID)
                        Text(context.apiURL).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                }
                Button("Refresh accounts", systemImage: "arrow.clockwise") {
                    Task { await model.reloadContexts() }
                }.disabled(model.loadingContexts || model.busy)
                if model.loadingContexts { ProgressView().controlSize(.small) }
            }
            if model.selected != nil {
                Section("Ting deliveries") {
                    SettingsFootnote("Allow Peek to deliver your answers and messages to Ting using your account. You can revoke this access in Silicon Accounts.")
                    if model.enrolled {
                        Label("Deliveries enabled", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                    } else {
                        Button("Enable deliveries and retry queued answers") {
                            Task { await model.perform(.enroll) }
                        }.buttonStyle(.borderedProminent).disabled(model.busy)
                    }
                    if model.busy { ProgressView("Updating deliveries…").controlSize(.small) }
                    if let notice = model.notice { SettingsFootnote(notice) }
                }
            }
            if let error = model.error {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.callout).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
        .peekSettingsPane("Accounts & permissions", subtitle: "Choose a Carbon or Silicon account and manage deliveries.")
        .task { await model.reloadContexts() }
        .onDisappear { model.suspend() }
    }
}
