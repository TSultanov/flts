import CoreImage.CIFilterBuiltins
import SwiftUI

#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// The few OS services views need, in one place for both platforms.
@MainActor
enum Platform {
    #if os(macOS)
    static func revealLibrary(_ path: String?) {
        guard let path else { return }
        let url = URL(fileURLWithPath: path, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }
    #endif

    #if os(iOS)
    /// Excludes the per-device Syncthing identity/index directory from iCloud
    /// backups while keeping `library/` and `config.json` backed up.
    static func excludeSyncIdentityFromBackup() {
        guard (ProcessInfo.processInfo.environment["FLTS_CONFIG_DIR"] ?? "").isEmpty else { return }
        guard let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        else { return }
        var url = support.appendingPathComponent("com.TS.FLTS/syncthing", isDirectory: true)
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? url.setResourceValues(values)
    }
    #endif

    static func open(_ url: URL) {
        #if os(macOS)
        NSWorkspace.shared.open(url)
        #else
        UIApplication.shared.open(url)
        #endif
    }

    static func copy(_ string: String) {
        #if os(macOS)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(string, forType: .string)
        #else
        UIPasteboard.general.string = string
        #endif
    }

    /// Ends text editing, so a field that saves on losing focus commits.
    static func endEditing() {
        #if os(macOS)
        NSApp.keyWindow?.makeFirstResponder(nil)
        #else
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        #endif
    }

    private static let ciContext = CIContext()
    private static var qrCache: [String: Image] = [:]

    static func qrCode(_ payload: String, scale: CGFloat = 8) -> Image? {
        let key = "\(scale):\(payload)"
        if let cached = qrCache[key] { return cached }
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(payload.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: scale, y: scale)),
            let cg = ciContext.createCGImage(output, from: output.extent)
        else { return nil }
        let image = Image(decorative: cg, scale: 1)
        if qrCache.count >= 16 { qrCache.removeAll(keepingCapacity: true) }
        qrCache[key] = image
        return image
    }
}
