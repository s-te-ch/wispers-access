import SwiftUI

/// Join a share by pasting its `wax_` invite code. As the code is typed the
/// footer previews which backend it points at — so a self-hosted invite visibly
/// announces itself — and parse/join errors surface with their real reason.
struct AddShareScreen: View {
    @Environment(ShareManager.self) private var manager
    @Environment(\.dismiss) private var dismiss

    @State private var inviteText = ""
    @State private var isJoining = false
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("wax_…", text: $inviteText, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.body.monospaced())
                        .lineLimit(2...4)
                } header: {
                    Text("Invite code")
                } footer: {
                    backendPreview
                }

                if let errorMessage {
                    Section {
                        Label(errorMessage, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Add share")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Join") { Task { await join() } }
                        .disabled(trimmed.isEmpty || isJoining)
                }
            }
            .overlay {
                if isJoining {
                    ProgressView("Joining…")
                        .controlSize(.large)
                        .padding(24)
                        .background(.regularMaterial, in: .rect(cornerRadius: 12))
                }
            }
            .interactiveDismissDisabled(isJoining)
        }
    }

    /// Live, non-blocking preview of the pasted code's backend. Parse errors are
    /// intentionally swallowed here — they belong on the Join action, not on
    /// every keystroke of a half-typed code.
    @ViewBuilder
    private var backendPreview: some View {
        if let invite = try? InviteCode.parse(inviteText) {
            if let backend = invite.backend {
                Text("Self-hosted backend: \(backend)")
            } else {
                Text("Managed backend")
            }
        } else {
            Text("Paste the wax_ code you were given.")
        }
    }

    private var trimmed: String {
        inviteText.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func join() async {
        errorMessage = nil
        isJoining = true
        defer { isJoining = false }
        do {
            try await manager.join(inviteCode: inviteText)
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
