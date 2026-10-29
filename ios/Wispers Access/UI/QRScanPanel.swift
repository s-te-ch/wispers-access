import AVFoundation
import SwiftUI

/// The camera half of the join form. A square live viewfinder that hands every
/// QR payload it decodes to `onScan`, frame after frame. The caller decides
/// what counts as an invite and ignores repeats. Asks for camera access once
/// started. Denied, it collapses to a one-line row pointing at Settings.
/// Callers gate on `isAvailable`, which is false without a back camera (the
/// Simulator).
struct QRScanPanel: View {
    /// False shows the panel with a button that starts the camera, for where a
    /// permission prompt on sight would be out of place.
    var isStarted = true
    var onStart: () -> Void = {}
    let onScan: (String) -> Void

    @State private var access = AVCaptureDevice.authorizationStatus(for: .video)

    /// Whether this device can scan at all. Demo mode draws the panel without
    /// a camera, so Simulator screenshots show it.
    static var isAvailable: Bool {
        DemoMode.active || QRCaptureSession.backCamera != nil
    }

    var body: some View {
        switch access {
        case .denied, .restricted:
            deniedRow
        default:
            ViewfinderFrame {
                if DemoMode.active {
                    EmptyView()
                } else if !isStarted {
                    startButton
                } else if access == .authorized {
                    LiveCamera(onScan: onScan)
                }
            }
            .task(id: isStarted) { await requestAccess() }
        }
    }

    private var startButton: some View {
        Button(action: onStart) {
            Label("Scan a QR code", systemImage: "camera")
                .font(.body.weight(.semibold))
                .foregroundStyle(AccessColor.primaryDark)
                .padding(.horizontal, 20)
                .padding(.vertical, 12)
                .background(AccessColor.primary, in: Capsule())
        }
    }

    private func requestAccess() async {
        guard isStarted, !DemoMode.active, access == .notDetermined else { return }
        _ = await AVCaptureDevice.requestAccess(for: .video)
        access = AVCaptureDevice.authorizationStatus(for: .video)
    }

    private var deniedRow: some View {
        HStack {
            Text("Allow camera to scan")
                .font(.body.weight(.semibold))
                .foregroundStyle(AccessColor.onViewfinder)
            Spacer()
            Button("Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    UIApplication.shared.open(url)
                }
            }
            .font(.body.weight(.semibold))
            .tint(AccessColor.primary)
        }
        .padding(.leading, 20)
        .padding(.trailing, 16)
        .frame(height: 56)
        .background(AccessColor.viewfinder, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
    }
}

/// The dark square with the corner marks and the hint, around `content`.
private struct ViewfinderFrame<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ZStack {
            AccessColor.viewfinder
            content
            ScanBrackets()
                .allowsHitTesting(false)
            Text("Point at the QR code in the invite")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(AccessColor.onViewfinder)
                .frame(maxHeight: .infinity, alignment: .bottom)
                .padding(.bottom, 18)
                .allowsHitTesting(false)
        }
        .aspectRatio(1, contentMode: .fit)
        .clipShape(RoundedRectangle(cornerRadius: 24, style: .continuous))
    }
}

/// The four corner marks around the middle 63% of the square, decoration only.
private struct ScanBrackets: View {
    var body: some View {
        Canvas { context, size in
            let side = min(size.width, size.height) * 0.63
            let center = CGPoint(x: size.width / 2, y: size.height / 2)
            let stroke: CGFloat = 4
            let inset = stroke / 2
            let arm = side * 0.16
            let radius = arm / 3
            // The top-left mark, relative to the centre, turned into the other corners.
            let near = -side / 2 + inset
            var corner = Path()
            corner.move(to: CGPoint(x: near, y: near + arm))
            corner.addLine(to: CGPoint(x: near, y: near + radius))
            corner.addQuadCurve(to: CGPoint(x: near + radius, y: near), control: CGPoint(x: near, y: near))
            corner.addLine(to: CGPoint(x: near + arm, y: near))
            for quarter in 0..<4 {
                let turn = CGAffineTransform(rotationAngle: CGFloat(quarter) * .pi / 2)
                    .concatenating(CGAffineTransform(translationX: center.x, y: center.y))
                context.stroke(
                    corner.applying(turn),
                    with: .color(AccessColor.primary),
                    style: StrokeStyle(lineWidth: stroke, lineCap: .round)
                )
            }
        }
        .accessibilityHidden(true)
    }
}

/// The running camera with the torch toggle. Owns its capture session, which
/// starts with this view and stops when it goes; iOS pauses it in the
/// background on its own.
private struct LiveCamera: View {
    let onScan: (String) -> Void

    @Environment(\.scenePhase) private var scenePhase
    @State private var capture: QRCaptureSession?
    @State private var torchOn = false

    var body: some View {
        ZStack(alignment: .topTrailing) {
            if let capture {
                CameraPreview(capture: capture)
                if capture.hasTorch {
                    torchButton(capture)
                }
            }
        }
        .onAppear {
            let capture = QRCaptureSession()
            capture.onScan = onScan
            self.capture = capture
        }
        // The torch goes out whenever the session pauses; keep the toggle in step.
        .onChange(of: scenePhase) { if scenePhase != .active { torchOn = false } }
    }

    private func torchButton(_ capture: QRCaptureSession) -> some View {
        Button {
            torchOn.toggle()
            capture.setTorch(torchOn)
        } label: {
            Image(systemName: torchOn ? "flashlight.on.fill" : "flashlight.off.fill")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(torchOn ? AccessColor.primaryDark : AccessColor.onViewfinder)
                .frame(width: 44, height: 44)
                .background(torchOn ? AccessColor.primary : Color.white.opacity(0.14), in: Circle())
        }
        .accessibilityLabel(torchOn ? "Turn torch off" : "Turn torch on")
        .padding(14)
    }
}

private struct CameraPreview: UIViewRepresentable {
    let capture: QRCaptureSession

    func makeUIView(context: Context) -> PreviewView {
        let view = PreviewView()
        view.show(capture)
        return view
    }

    func updateUIView(_ view: PreviewView, context: Context) {}

    static func dismantleUIView(_ view: PreviewView, coordinator: ()) {
        view.capture?.stop()
    }

    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        private var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
        private(set) var capture: QRCaptureSession?
        private var rotation: AVCaptureDevice.RotationCoordinator?
        private var rotationObservation: NSKeyValueObservation?

        func show(_ capture: QRCaptureSession) {
            self.capture = capture
            previewLayer.session = capture.session
            previewLayer.videoGravity = .resizeAspectFill
            if let device = capture.device {
                let rotation = AVCaptureDevice.RotationCoordinator(device: device, previewLayer: previewLayer)
                rotationObservation = rotation.observe(\.videoRotationAngleForHorizonLevelPreview) { [weak self] _, _ in
                    guard let self else { return }
                    Task { @MainActor in self.applyRotation() }
                }
                self.rotation = rotation
            }
            // The preview's connection exists only once the session has its
            // input, so the first rotation waits for the session to run.
            capture.start { [weak self] in self?.applyRotation() }
        }

        /// Keeps the picture upright as the iPad turns.
        private func applyRotation() {
            guard let rotation, let connection = previewLayer.connection else { return }
            let angle = rotation.videoRotationAngleForHorizonLevelPreview
            if connection.isVideoRotationAngleSupported(angle) {
                connection.videoRotationAngle = angle
            }
        }
    }
}

/// The back camera with a QR metadata output. Configured and run on its own
/// queue, as AVFoundation asks; payloads arrive on the main queue.
nonisolated final class QRCaptureSession: NSObject, AVCaptureMetadataOutputObjectsDelegate, @unchecked Sendable {
    static var backCamera: AVCaptureDevice? {
        AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
    }

    let session = AVCaptureSession()
    let device = QRCaptureSession.backCamera
    /// Set once on the main actor before the session runs, called on it.
    var onScan: (@MainActor (String) -> Void)?

    private let queue = DispatchQueue(label: "dev.wispers.access.qr-capture")

    var hasTorch: Bool { device?.hasTorch ?? false }

    override init() {
        super.init()
        queue.async { self.configure() }
    }

    private func configure() {
        guard let device, let input = try? AVCaptureDeviceInput(device: device) else { return }
        session.beginConfiguration()
        if session.canAddInput(input) { session.addInput(input) }
        let output = AVCaptureMetadataOutput()
        if session.canAddOutput(output) {
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]
        }
        session.commitConfiguration()
    }

    func start(onRunning: @escaping @MainActor @Sendable () -> Void) {
        queue.async {
            if !self.session.isRunning { self.session.startRunning() }
            Task { @MainActor in onRunning() }
        }
    }

    func stop() {
        queue.async {
            if self.session.isRunning { self.session.stopRunning() }
        }
    }

    func setTorch(_ on: Bool) {
        queue.async { [device] in
            guard let device, device.hasTorch, (try? device.lockForConfiguration()) != nil else { return }
            device.torchMode = on ? .on : .off
            device.unlockForConfiguration()
        }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput objects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        let payload = objects.lazy.compactMap { ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }.first
        guard let payload else { return }
        // The delegate queue is the main one (see configure).
        MainActor.assumeIsolated { onScan?(payload) }
    }
}
