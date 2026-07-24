import SwiftUI

/// Join a share by pasting its `wax_` invite code or scanning its QR. The idle
/// state offers both; once joining starts it shows step-by-step progress
/// (Validating → Generating identity → Registering → Activating), and on success
/// a "Joined" summary with Open / Back. The join itself lives in `ShareManager`
/// (single source of truth incl. rollback); this screen only drives the UI from
/// the steps it reports.
struct AddShareScreen: View {
    @Environment(ShareManager.self) private var manager
    @Environment(BrowseRouter.self) private var router
    @Environment(\.dismiss) private var dismiss

    @State private var tab: AddTab = .enterCode
    @State private var code = DemoMode.presentAddSheet ? DemoMode.sampleInvite : ""
    @State private var phase: JoinPhase = .idle
    @State private var errorMessage: String?
    @State private var showingScanner = false

    var body: some View {
        NavigationStack {
            ZStack {
                AccessColor.background.ignoresSafeArea()
                content.padding(16)
            }
            .navigationTitle("Add an app")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if isIdle {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { dismiss() }
                    }
                }
            }
            .interactiveDismissDisabled(!isIdle)
        }
    }

    @ViewBuilder private var content: some View {
        switch phase {
        case .idle:
            idleContent
        case .joining(let current):
            JoinStepList(steps: JoinStep.allCases.map { ($0.label, stepStatus($0, current: current)) })
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        case .joined(let id, let nickname):
            JoinSuccess(nickname: nickname, onOpen: { open(id) }, onBackToList: { dismiss() })
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
    }

    private var idleContent: some View {
        VStack(spacing: 20) {
            Picker("How to add", selection: $tab) {
                Text("Enter code").tag(AddTab.enterCode)
                Text("Scan QR").tag(AddTab.scanQR)
            }
            .pickerStyle(.segmented)

            switch tab {
            case .enterCode: enterCode
            case .scanQR: scanQR
            }
            Spacer()
        }
        .onChange(of: code) { errorMessage = nil }
        .onChange(of: tab) { errorMessage = nil }
    }

    // MARK: Enter code

    private var enterCode: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("INVITATION CODE")
                .font(.caption.weight(.medium)).tracking(1)
                .foregroundStyle(AccessColor.onSurfaceVariant)

            TextField("wax_…", text: $code, axis: .vertical)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .font(.body.monospaced())
                .lineLimit(2...4)
                .padding(12)
                .background(AccessColor.surface, in: RoundedRectangle(cornerRadius: 12, style: .continuous))

            HStack {
                backendNote
                Spacer()
                PasteButton(payloadType: String.self) { strings in
                    if let pasted = strings.first { code = pasted }
                }
                .labelStyle(.titleAndIcon)
                .buttonBorderShape(.capsule)
                .tint(AccessColor.primary)
            }

            if let errorMessage {
                Text(errorMessage).font(.footnote).foregroundStyle(AccessColor.destructive)
            }

            Button { Task { await join() } } label: {
                Text("Join").accessFilledButton()
            }
            .disabled(trimmedCode.isEmpty)
            .opacity(trimmedCode.isEmpty ? 0.5 : 1)

            Text("Codes are issued by the person sharing the app with you.")
                .font(.footnote).foregroundStyle(AccessColor.onSurfaceVariant)
        }
    }

    /// Shows which backend the pasted code points at — so a self-hosted invite
    /// announces itself — but only once the code parses.
    @ViewBuilder private var backendNote: some View {
        if let invite = try? InviteCode.parse(code) {
            if let backend = invite.backend {
                Label("Self-hosted: \(backend)", systemImage: "server.rack")
                    .font(.caption).foregroundStyle(AccessColor.onSurfaceVariant)
                    .lineLimit(1).truncationMode(.middle)
            } else {
                Label("Managed backend", systemImage: "checkmark.seal")
                    .font(.caption).foregroundStyle(AccessColor.onSurfaceVariant)
            }
        }
    }

    // MARK: Scan QR

    private var scanQR: some View {
        VStack(alignment: .leading, spacing: 12) {
            if QRScannerView.canScan {
                Button { showingScanner = true } label: {
                    Label("Open camera", systemImage: "qrcode.viewfinder").accessFilledButton()
                }
                Text("Point the camera at the QR code from the invite.")
                    .font(.footnote).foregroundStyle(AccessColor.onSurfaceVariant)
            } else {
                ContentUnavailableView(
                    "Camera unavailable",
                    systemImage: "qrcode.viewfinder",
                    description: Text("QR scanning needs a device with a camera. Use “Enter code” instead.")
                )
            }
            if let errorMessage {
                Text(errorMessage).font(.footnote).foregroundStyle(AccessColor.destructive)
            }
        }
        .sheet(isPresented: $showingScanner) { scannerSheet }
    }

    private var scannerSheet: some View {
        NavigationStack {
            QRScannerView { payload in
                showingScanner = false
                code = payload
                Task { await join() }
            }
            .ignoresSafeArea()
            .navigationTitle("Scan invite")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { showingScanner = false }
                }
            }
        }
    }

    // MARK: Join

    private func join() async {
        errorMessage = nil
        // Pre-validate so a bad code shows its real reason inline, without briefly
        // flashing the progress steps.
        do {
            _ = try InviteCode.parse(code)
        } catch {
            errorMessage = error.localizedDescription
            return
        }
        do {
            let id = try await manager.join(inviteCode: code) { step in
                phase = .joining(step)
            }
            let nickname = manager.store.metadata(for: id)?.nickname ?? ""
            phase = .joined(id: id, nickname: nickname)
        } catch {
            phase = .idle
            errorMessage = error.localizedDescription
        }
    }

    private func open(_ id: ShareID) {
        // Defer the push to the roster's onDismiss — see BrowseRouter.
        router.openAfterDismiss = id
        dismiss()
    }

    // MARK: Helpers

    private var isIdle: Bool {
        if case .idle = phase { return true }
        return false
    }

    private var trimmedCode: String {
        code.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func stepStatus(_ step: JoinStep, current: JoinStep) -> StepStatus {
        if step.rawValue < current.rawValue { return .done }
        return step == current ? .running : .pending
    }
}

private enum AddTab: Hashable { case enterCode, scanQR }

private enum JoinPhase {
    case idle
    case joining(JoinStep)
    case joined(id: ShareID, nickname: String)
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
    let onOpen: () -> Void
    let onBackToList: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            JoinStepList(steps: JoinStep.allCases.map { ($0.label, .done) })
            StepRow(label: nickname.isEmpty ? "Joined" : "Joined “\(nickname)”", status: .done)
            VStack(spacing: 8) {
                Button(action: onOpen) { Text("Open app ↗").accessFilledButton() }
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
