import SwiftUI
import Vision
import VisionKit

/// A live QR-code scanner backed by VisionKit's `DataScannerViewController`.
/// Reports the first recognized QR payload once, then stops. Camera-only: not
/// available on the Simulator (`isSupported` is false) or when camera permission
/// is denied (`isAvailable`), so callers should gate on `QRScannerView.canScan`
/// and show a fallback otherwise.
struct QRScannerView: UIViewControllerRepresentable {
    let onScan: (String) -> Void

    /// Whether a live scan can run right now (real device, camera present and
    /// permitted). False on the Simulator.
    static var canScan: Bool {
        DataScannerViewController.isSupported && DataScannerViewController.isAvailable
    }

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            recognizesMultipleItems: false,
            isHighFrameRateTrackingEnabled: false,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {
        try? scanner.startScanning()
    }

    static func dismantleUIViewController(_ scanner: DataScannerViewController, coordinator: Coordinator) {
        scanner.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onScan: onScan) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        private let onScan: (String) -> Void
        private var handled = false

        init(onScan: @escaping (String) -> Void) { self.onScan = onScan }

        func dataScanner(
            _ scanner: DataScannerViewController,
            didAdd addedItems: [RecognizedItem],
            allItems: [RecognizedItem]
        ) {
            report(from: addedItems)
        }

        func dataScanner(_ scanner: DataScannerViewController, didTapOn item: RecognizedItem) {
            report(from: [item])
        }

        /// Emits the first QR payload, then latches so a lingering frame can't
        /// fire the join twice.
        private func report(from items: [RecognizedItem]) {
            guard !handled else { return }
            for case let .barcode(barcode) in items {
                if let payload = barcode.payloadStringValue {
                    handled = true
                    onScan(payload)
                    return
                }
            }
        }
    }
}
