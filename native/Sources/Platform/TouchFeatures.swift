#if os(iOS)
import AVFoundation
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
    @Environment(\.scenePhase) private var scenePhase

    /// False on devices without a usable camera, and in the Simulator.
    static var isSupported: Bool {
        DataScannerViewController.isSupported
    }

    @State private var status = AVCaptureDevice.authorizationStatus(for: .video)
    @State private var scanError: String?

    var body: some View {
        NavigationStack {
            Group {
                if status == .notDetermined {
                    ProgressView()
                } else if status == .authorized, DataScannerViewController.isAvailable, scanError == nil {
                    Scanner(
                        found: { code in
                            found(code)
                            dismiss()
                        },
                        onError: { scanError = $0 }
                    )
                    .ignoresSafeArea(edges: .bottom)
                } else if status == .denied || status == .restricted {
                    ContentUnavailableView {
                        Label("Camera Access Denied", systemImage: "camera")
                    } description: {
                        Text("Allow camera access for FLTS in Settings, or enter the device ID by hand.")
                    } actions: {
                        if let settings = URL(string: UIApplication.openSettingsURLString) {
                            Button("Open Settings") { UIApplication.shared.open(settings) }
                                .buttonStyle(.borderedProminent)
                        }
                    }
                } else {
                    ContentUnavailableView(
                        "Camera Unavailable", systemImage: "camera",
                        description: Text(scanError ?? "This device's camera scanner is unavailable. Enter the device ID by hand."))
                }
            }
            // Ask first: before the answer the scanner reports itself unavailable.
            .task { await checkPermission() }
            .onChange(of: scenePhase) { _, phase in
                guard phase == .active else { return }
                status = AVCaptureDevice.authorizationStatus(for: .video)
            }
            .navigationTitle("Scan Pairing Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
        }
    }

    private func checkPermission() async {
        let current = AVCaptureDevice.authorizationStatus(for: .video)
        if current == .notDetermined {
            _ = await AVCaptureDevice.requestAccess(for: .video)
        }
        status = AVCaptureDevice.authorizationStatus(for: .video)
    }

    private struct Scanner: UIViewControllerRepresentable {
        let found: (String) -> Void
        let onError: (String) -> Void

        func makeCoordinator() -> Coordinator { Coordinator(found: found) }

        func makeUIViewController(context: Context) -> DataScannerViewController {
            let scanner = DataScannerViewController(
                recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced, isHighlightingEnabled: true)
            scanner.delegate = context.coordinator
            do {
                try scanner.startScanning()
            } catch {
                DispatchQueue.main.async { onError(error.localizedDescription) }
            }
            return scanner
        }

        func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

        static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) {
            controller.stopScanning()
        }

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
