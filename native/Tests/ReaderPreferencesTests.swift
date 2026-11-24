import Foundation
import Testing

@testable import FLTS

@MainActor
struct ReaderPreferencesTests {
    /// Out-of-range sizes clamp without re-entering the observer forever.
    @Test func fontSizeClamps() {
        let prefs = ReaderPreferences.shared
        let before = prefs.fontSize
        defer { prefs.fontSize = before }
        prefs.fontSize = 100
        #expect(prefs.fontSize == 36)
        prefs.fontSize = 1
        #expect(prefs.fontSize == 12)
        prefs.fontSize = 20
        #expect(prefs.fontSize == 20)
    }

    @Test func marginScaleClamps() {
        let prefs = ReaderPreferences.shared
        let before = prefs.marginScale
        defer { prefs.marginScale = before }
        prefs.marginScale = 100
        #expect(prefs.marginScale == 3)
        prefs.marginScale = 0
        #expect(prefs.marginScale == 0.5)
        prefs.marginScale = 1.25
        #expect(prefs.marginScale == 1.25)
    }

    /// The paragraph button always fits the left margin.
    @Test func paragraphButtonFitsTheMargin() {
        let canvas = ReaderCanvas(frame: .zero)
        for tight in [true, false] {
            canvas.tight = tight
            for scale in stride(from: CGFloat(0.5), through: 3, by: 0.25) {
                canvas.setMarginScale(scale)
                canvas.updateMargins()
                #expect(canvas.controlSize + canvas.controlGap <= canvas.margins.left)
            }
        }
        canvas.tight = true
        canvas.setMarginScale(1)
        canvas.updateMargins()
        #expect(canvas.margins.left == 13)
        #expect(canvas.margins.right == 13)
    }
}
