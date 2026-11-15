#if os(iOS)
import SwiftUI
import UIKit
import VisionKit

/// The system dictionary's sheet for a term (the iOS counterpart of Look Up).
struct DictionarySheet: UIViewControllerRepresentable {
    let term: String

    func makeUIViewController(context: Context) -> UIReferenceLibraryViewController {
        UIReferenceLibraryViewController(term: term)
    }

    func updateUIViewController(_ controller: UIReferenceLibraryViewController, context: Context) {}
}

/// Scans another device's pairing QR code with the camera.
struct PairingScanner: View {
    let found: (String) -> Void
    @Environment(\.dismiss) private var dismiss

    /// False on devices without a usable camera, and in the Simulator.
    static var isSupported: Bool {
        DataScannerViewController.isSupported
    }

    var body: some View {
        NavigationStack {
            Group {
                if DataScannerViewController.isAvailable {
                    Scanner { code in
                        found(code)
                        dismiss()
                    }
                    .ignoresSafeArea(edges: .bottom)
                } else {
                    ContentUnavailableView(
                        "Camera Unavailable", systemImage: "camera",
                        description: Text("Allow camera access for FLTS in Settings, or enter the device ID by hand."))
                }
            }
            .navigationTitle("Scan Pairing Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
        }
    }

    private struct Scanner: UIViewControllerRepresentable {
        let found: (String) -> Void

        func makeCoordinator() -> Coordinator { Coordinator(found: found) }

        func makeUIViewController(context: Context) -> DataScannerViewController {
            let scanner = DataScannerViewController(
                recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced, isHighlightingEnabled: true)
            scanner.delegate = context.coordinator
            try? scanner.startScanning()
            return scanner
        }

        func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

        @MainActor
        final class Coordinator: NSObject, DataScannerViewControllerDelegate {
            let found: (String) -> Void
            private var done = false

            init(found: @escaping (String) -> Void) {
                self.found = found
            }

            func dataScanner(_ scanner: DataScannerViewController, didAdd added: [RecognizedItem], allItems: [RecognizedItem]) {
                guard !done else { return }
                for case .barcode(let code) in added {
                    guard let payload = code.payloadStringValue else { continue }
                    done = true
                    scanner.stopScanning()
                    found(payload)
                    return
                }
            }
        }
    }
}
#endif
