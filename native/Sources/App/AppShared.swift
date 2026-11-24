import SwiftUI

/// Compared by what the menus show, so a change of state republishes them.
struct BookActions: Equatable {
    static func == (a: BookActions, b: BookActions) -> Bool {
        a.hasChapters == b.hasChapters && a.chaptersShown == b.chaptersShown && a.wordDetailsShown == b.wordDetailsShown
    }

    let translateChapter: () -> Void
    let toggleWordPanel: () -> Void
    let toggleChapters: () -> Void
    let hasChapters: Bool
    var chaptersShown = false
    var wordDetailsShown = false
}

extension FocusedValues {
    @Entry var readerControls: ReaderControls?
    @Entry var bookActions: BookActions?
}

/// Reader preferences shared by the menu and the reader.
@MainActor
@Observable
final class ReaderPreferences {
    static let shared = ReaderPreferences()
    static let defaultFontSize: CGFloat = 18

    var fontSize: CGFloat {
        didSet {
            // Reassigning here re-enters this observer, so only when it changes the value.
            let clamped = min(max(fontSize, 12), 36)
            guard clamped == fontSize else {
                fontSize = clamped
                return
            }
            AppDefaults.store.set(Double(fontSize), forKey: "readerFontSize")
        }
    }

    static let marginScales: ClosedRange<CGFloat> = 0.5...3
    static let marginStep: CGFloat = 0.25

    /// Side margins, as a multiple of the default for the screen.
    var marginScale: CGFloat {
        didSet {
            let clamped = min(max(marginScale, Self.marginScales.lowerBound), Self.marginScales.upperBound)
            guard clamped == marginScale else {
                marginScale = clamped
                return
            }
            AppDefaults.store.set(Double(marginScale), forKey: "readerMarginScale")
        }
    }

    private init() {
        let saved = AppDefaults.store.double(forKey: "readerFontSize")
        fontSize = saved > 0 ? CGFloat(saved) : Self.defaultFontSize
        let scale = AppDefaults.store.double(forKey: "readerMarginScale")
        marginScale = scale > 0 ? CGFloat(scale) : 1
    }

    func typography(language: String? = nil) -> ReaderTypography {
        var t = ReaderTypography()
        t.fontSize = fontSize
        t.language = language
        return t
    }
}

/// Light, dark, or the system's choice, under Reading in Settings.
enum Appearance: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: Self { self }

    static let key = "appearance"

    var title: String {
        switch self {
        case .system: "Match System"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }

    @MainActor
    static func apply() {
        let raw = AppDefaults.store.string(forKey: key) ?? Appearance.system.rawValue
        #if os(macOS)
        switch Appearance(rawValue: raw) ?? .system {
        case .system: NSApp.appearance = nil
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
        #else
        // On iOS the root view reads the setting (`colorScheme`).
        _ = raw
        #endif
    }
}
