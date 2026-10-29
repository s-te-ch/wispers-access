import SwiftUI
import WispersAccessSdk

/// The add-a-share sheet: the join form, with Cancel while nothing is under
/// way. On success it offers Open / Back, or, given `onJoined`, hands the share
/// over and closes, for layouts that show the new share beside it (iPad).
struct AddShareScreen: View {
    var onJoined: ((Share) -> Void)?

    @Environment(BrowseRouter.self) private var router
    @Environment(\.dismiss) private var dismiss
    @State private var phase: JoinPhase = .idle

    var body: some View {
        NavigationStack {
            ZStack {
                AccessColor.background.ignoresSafeArea()
                form.padding(16)
            }
            .navigationTitle("Add a share")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if phase.isIdle {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { dismiss() }
                    }
                }
            }
            .interactiveDismissDisabled(!phase.isIdle)
        }
    }

    private var form: JoinShareForm {
        JoinShareForm(
            phase: $phase,
            onJoined: onJoined.map { _ in handOver },
            onOpen: open,
            onBackToList: { dismiss() }
        )
    }

    private func handOver(_ share: Share) {
        onJoined?(share)
        dismiss()
    }

    private func open(_ id: ShareId) {
        // Defer the push to the roster's onDismiss — see BrowseRouter.
        router.openAfterDismiss = id
        dismiss()
    }
}

/// Join a share by scanning its QR or pasting its invite code. The idle state
/// shows the camera with the code field below it. A scanned invite joins at
/// once, a pasted one on Join, both through `join()`. Once joining starts it
/// shows step progress (Validating → Joining), and on success a "Joined"
/// summary with Open / Back — unless `onJoined` takes the share instead. The
/// join itself is the SDK's, through `ShareManager`; this form only drives the
/// UI from the steps it reports.
struct JoinShareForm: View {
    @Binding var phase: JoinPhase
    /// Waits for a tap before turning the camera on, where a permission prompt
    /// on sight would be out of place (the iPad's first screen).
    var cameraOnTap = false
    var onJoined: ((Share) -> Void)?
    var onOpen: (ShareId) -> Void = { _ in }
    var onBackToList: () -> Void = {}

    @Environment(ShareManager.self) private var manager

    @State private var code = DemoMode.presentAddSheet ? DemoMode.sampleInvite : ""
    @State private var errorMessage: String?
    @State private var cameraTapped = false
    /// The last QR payload seen. The camera keeps decoding a code for as long
    /// as it is in frame, so a repeat is ignored, also after its join failed.
    /// Clearing the field forgets it.
    @State private var lastScan: String?

    var body: some View {
        switch phase {
        case .idle:
            idleContent
        case .joining(let current):
            JoinStepList(steps: JoinStep.allCases.map { ($0.label, stepStatus($0, current: current)) })
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        case .joined(let share):
            JoinSuccess(
                nickname: share.name,
                appCount: share.apps.count,
                onOpen: { onOpen(share.id) },
                onBackToList: onBackToList
            )
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
    }

    private var idleContent: some View {
        VStack(spacing: 0) {
            if QRScanPanel.isAvailable {
                labelledDivider("SCAN THE QR CODE")
                    .padding(.bottom, 18)
                QRScanPanel(
                    isStarted: !cameraOnTap || cameraTapped,
                    onStart: { cameraTapped = true },
                    onScan: scanned
                )
                    // Takes what the rest leaves, so it shrinks on short
                    // screens and above the keyboard.
                    .layoutPriority(1)
                labelledDivider("OR PASTE THE INVITE CODE")
                    .padding(.top, 26)
                    .padding(.bottom, 18)
            }
            codeEntry
            Spacer(minLength: 0)
        }
        .onChange(of: code) {
            errorMessage = nil
            if code.isEmpty { lastScan = nil }
        }
    }

    private func labelledDivider(_ label: String) -> some View {
        HStack(spacing: 14) {
            Rectangle().fill(AccessColor.outline).frame(height: 1)
            Text(label)
                .font(.caption.weight(.bold)).tracking(1.6)
                .foregroundStyle(AccessColor.onSurfaceVariant)
                .fixedSize()
            Rectangle().fill(AccessColor.outline).frame(height: 1)
        }
    }

    private var codeEntry: some View {
        VStack(alignment: .leading, spacing: 12) {
            codeField

            if let errorMessage {
                Text(errorMessage).font(.footnote).foregroundStyle(AccessColor.destructive)
                    // The camera has layout priority, and would squeeze this to a line.
                    .fixedSize(horizontal: false, vertical: true)
            }

            Button { Task { await join() } } label: {
                Text("Join").accessFilledButton()
            }
            .disabled(trimmedCode.isEmpty)
            .opacity(trimmedCode.isEmpty ? 0.5 : 1)
        }
    }

    /// One line, since the codes are too long to read whole anyway.
    private var codeField: some View {
        HStack(spacing: 4) {
            TextField("wax1_…", text: $code)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.asciiCapable)
                .submitLabel(.go)
                .onSubmit { Task { await join() } }
                .accessibilityLabel("Invitation code")
            if code.isEmpty {
                PasteButton(payloadType: String.self) { strings in
                    if let pasted = strings.first {
                        code = pasted.trimmingCharacters(in: .whitespacesAndNewlines)
                    }
                }
                .labelStyle(.titleOnly)
                .buttonBorderShape(.capsule)
                .tint(AccessColor.primary)
            } else {
                Button { code = "" } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title3)
                        .foregroundStyle(AccessColor.onSurfaceVariant)
                        .frame(width: 44, height: 44)
                }
                .accessibilityLabel("Clear")
            }
        }
        .padding(.leading, 16)
        .padding(.trailing, 6)
        .frame(height: 56)
        .background(AccessColor.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .strokeBorder(code.isEmpty ? AccessColor.outline : AccessColor.primary, lineWidth: 1.5)
        )
    }

    /// Handles a scanned QR payload. An invite triggers join at once, anything
    /// else is ignored.
    private func scanned(_ payload: String) {
        guard phase.isIdle, payload != lastScan else { return }
        lastScan = payload
        let invite = payload.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (try? validateInvite(inviteCode: invite)) != nil else { return }
        code = invite
        Task { await join() }
    }

    // MARK: Join

    private func join() async {
        // Return on the keyboard skips the disabled Join button.
        guard !trimmedCode.isEmpty else { return }
        errorMessage = nil
        // Pre-validate so a bad code shows its real reason inline, without briefly
        // flashing the progress steps.
        do {
            _ = try validateInvite(inviteCode: trimmedCode)
        } catch {
            errorMessage = error.localizedDescription
            return
        }
        do {
            let share = try await manager.join(inviteCode: trimmedCode) { step in
                phase = .joining(step)
            }
            if let onJoined {
                phase = .idle
                onJoined(share)
            } else {
                phase = .joined(share)
            }
        } catch SdkError.InviteRefused(let sentence) {
            // Already written for the user, unlike the other errors' text.
            phase = .idle
            errorMessage = sentence
        } catch {
            phase = .idle
            errorMessage = error.localizedDescription
        }
    }

    // MARK: Helpers

    private var trimmedCode: String {
        code.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func stepStatus(_ step: JoinStep, current: JoinStep) -> StepStatus {
        if step.rawValue < current.rawValue { return .done }
        return step == current ? .running : .pending
    }
}

enum JoinPhase {
    case idle
    case joining(JoinStep)
    case joined(Share)

    var isIdle: Bool {
        if case .idle = self { return true }
        return false
    }
}

private enum StepStatus { case pending, running, done }

/// The ordered join steps with their status, shown while joining and (all done)
/// on success.
private struct JoinStepList: View {
    let steps: [(label: String, status: StepStatus)]

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
                StepRow(label: step.label, status: step.status)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct JoinSuccess: View {
    let nickname: String
    /// Decides the label: a single app opens directly, otherwise the share's
    /// detail lists them (as on Android).
    let appCount: Int
    let onOpen: () -> Void
    let onBackToList: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            JoinStepList(steps: JoinStep.allCases.map { ($0.label, .done) })
            StepRow(label: nickname.isEmpty ? "Joined" : "Joined “\(nickname)”", status: .done)
            VStack(spacing: 8) {
                Button(action: onOpen) {
                    Text(appCount == 1 ? "Open app ↗" : "Show apps").accessFilledButton()
                }
                Button(action: onBackToList) {
                    Text("Back to list").accessOutlinedButton(tint: AccessColor.primaryDark)
                }
            }
            .padding(.top, 8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct StepRow: View {
    let label: String
    let status: StepStatus

    var body: some View {
        HStack(spacing: 12) {
            indicator.frame(width: 24, height: 24)
            Text(label)
                .font(.body)
                .foregroundStyle(status == .pending ? AccessColor.onSurfaceVariant : AccessColor.onSurface)
            Spacer()
        }
    }

    @ViewBuilder private var indicator: some View {
        switch status {
        case .done:
            Image(systemName: "checkmark.circle.fill")
                .font(.title3)
                .foregroundStyle(AccessColor.online)
        case .running:
            ProgressView().controlSize(.small)
        case .pending:
            Circle()
                .strokeBorder(AccessColor.onSurfaceVariant.opacity(0.4), lineWidth: 2)
                .frame(width: 20, height: 20)
        }
    }
}
