import SwiftUI

struct PermissionsView: View {
    @Bindable var model: PermissionsModel

    private func perform(_ action: PermissionAction) {
        guard let id = model.selectedID else { return }
        Task { await model.perform(action, expectedID: id) }
    }

    var body: some View {
        Form {
            Section("Account and organization") {
                if model.contexts.isEmpty {
                    SettingsFootnote("Sign in with the Peek CLI to add an account. Use `peek --profile NAME login` to keep another account or organization separate.")
                } else {
                    Picker("Saved login", selection: Binding(get: { model.selectedID }, set: { id in
                        model.select(id)
                        Task { await model.perform(.status, expectedID: id) }
                    })) {
                        ForEach(model.contexts) { context in
                            Text(context.label).tag(Optional(context.id))
                        }
                    }
                    if let context = model.selected {
                        VStack(alignment: .leading, spacing: 8) {
                            HStack(alignment: .top, spacing: 24) {
                                VStack(alignment: .leading, spacing: 3) {
                                    Text("Account").font(.caption).foregroundStyle(.secondary)
                                    Text(context.actor.publicID)
                                }
                                VStack(alignment: .leading, spacing: 3) {
                                    Text("Organization").font(.caption).foregroundStyle(.secondary)
                                    Text(context.orgID)
                                }
                                Spacer(minLength: 0)
                            }
                            Text(context.environmentLabel).font(.caption).foregroundStyle(.secondary)
                            Text(context.apiURL).font(.caption).foregroundStyle(.secondary)
                        }.textSelection(.enabled)
                    }
                }
                HStack {
                    Button("Refresh accounts", systemImage: "arrow.clockwise") {
                        Task { await model.reloadContexts() }
                    }.disabled(model.loadingContexts || model.busy != nil)
                    if model.loadingContexts { ProgressView().controlSize(.small) }
                }
            }

            if model.selected != nil {
                Section("Ting deliveries") {
                    SettingsFootnote("Review Peek’s Ting permissions in IAM for this account and organization. Saving approval does not enable deliveries or retry queued answers.")
                    if model.enrolled {
                        Label("Deliveries enabled for this saved login", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                    }
                    if let request = model.request {
                        LabeledContent("Permission review", value: request.completed ? "Approval saved" : request.authorization.status.capitalized)
                        if let expiry = request.authorization.expiry, !request.completed {
                            LabeledContent("Expires") { Text(expiry, style: .relative) }
                        }
                        if request.completed {
                            if !model.enrolled {
                                Button("Enable deliveries and retry queued answers") {
                                    perform(.enroll)
                                }.buttonStyle(.borderedProminent).disabled(model.busy != nil)
                            }
                        } else if model.needsFreshReview {
                            SettingsFootnote("This review ended. Clear it and start a fresh review to continue.")
                        } else {
                            if let url = request.authorization.reviewURL {
                                Link("Review permissions in IAM", destination: url)
                            }
                            SecureField("Approval code from IAM", text: $model.code)
                                .textContentType(.oneTimeCode).disabled(model.busy != nil)
                            HStack {
                                Button("Save approval") { perform(.complete) }
                                    .buttonStyle(.borderedProminent)
                                    .disabled(model.busy != nil || model.code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                                Button("Retry saved completion") { perform(.complete) }
                                    .disabled(model.busy != nil || !model.code.isEmpty)
                            }
                            SettingsFootnote("If a reply was interrupted, retry saved completion without re-entering the code. Peek’s background service recovers the original request.")
                        }
                        HStack {
                            Button("Check status") { perform(.status) }
                            Button("Clear local review") { perform(.cancel) }
                        }.disabled(model.busy != nil)
                        SettingsFootnote("Clearing a local review keeps queued answers and existing enrollment. Manage previously granted permissions in IAM.")
                    } else {
                        Button("Review Ting permissions") { perform(.start) }
                            .buttonStyle(.borderedProminent).disabled(model.busy != nil)
                    }
                    if model.busy != nil { ProgressView("Updating permissions…").controlSize(.small) }
                    if let notice = model.notice { SettingsFootnote(notice) }
                }
            }
            if let error = model.error {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.callout).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
        .task { await model.reloadContexts() }
        .onDisappear { model.suspend() }
    }
}
