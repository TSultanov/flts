import Testing
import UIKit

@testable import FLTS

@MainActor
@Suite struct iOSReaderTests {
    private func untranslatedChapter(count: Int = 24) -> [Int: ParagraphText] {
        let line = Array(repeating: "palabra", count: 28).joined(separator: " ") + "."
        var out: [Int: ParagraphText] = [:]
        for id in 0..<count {
            out[id] = ParagraphText(id: id, text: line, runs: [], words: [], translated: false)
        }
        return out
    }

    @Test func leftMarginTapTurnsBackWithoutConflictingWithTranslateButton() async throws {
        final class DelegateRecorder: ReaderCanvasDelegate {
            var translated: [Int] = []
            func reader(_ canvas: ReaderCanvas, selected paragraphId: Int, wordIndex: Int) {}
            func reader(_ canvas: ReaderCanvas, translate paragraphId: Int, useCache: Bool) {
                translated.append(paragraphId)
            }
            func reader(_ canvas: ReaderCanvas, movedTo page: Int, of count: Int) {}
            func readerTranslationProgress(_ canvas: ReaderCanvas, paragraphId: Int) -> Double? { nil }
            func readerToggle(_ canvas: ReaderCanvas, key: String) -> Bool { false }
            func readerCanTranslate(_ canvas: ReaderCanvas) -> Bool { true }
            func readerDeselected(_ canvas: ReaderCanvas) {}
        }

        let paragraphs = untranslatedChapter()
        let canvas = ReaderCanvas(frame: CGRect(x: 0, y: 0, width: 390, height: 700))
        let delegate = DelegateRecorder()
        canvas.delegate = delegate
        canvas.tight = true
        canvas.updateMargins()
        canvas.setContent(
            order: paragraphs.keys.sorted(), paragraphs: paragraphs, typography: ReaderTypography(), anchor: nil)
        canvas.layoutIfNeeded()
        canvas.layoutSubviews()
        if canvas.isProvisional {
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                canvas.onFullLayout = {
                    canvas.onFullLayout = nil
                    c.resume()
                }
            }
        }
        #expect(canvas.pageCount >= 2)
        canvas.go(toPage: 1, animated: false)
        #expect(canvas.firstVisiblePage == 1)

        let rects = canvas.visibleControlRects()
        let first = try #require(rects.first)

        // 1. Direct tap on the gutter translate button translates instead of turning back.
        canvas.handleTap(at: CGPoint(x: first.1.midX, y: first.1.midY))
        #expect(delegate.translated == [first.0])
        #expect(canvas.firstVisiblePage == 1)

        // 2. Near-miss tap just outside the button (inside the 2*controlSlop guard) must not turn back.
        let nearMiss = CGPoint(x: 4, y: first.1.maxY + canvas.controlSlop * 1.5)
        canvas.handleTap(at: nearMiss)
        #expect(canvas.firstVisiblePage == 1)

        // 3. Left-margin tap away from any translate button turns back to the previous page.
        var clearY: CGFloat?
        var y = canvas.margins.top + 8
        while y < canvas.bounds.height - canvas.margins.bottom {
            let pt = CGPoint(x: 8, y: y)
            if !canvas.nearControl(pt), canvas.hit(pt) == nil {
                clearY = y
                break
            }
            y += 6
        }
        let targetY = try #require(clearY, "expected a left-margin point clear of gutter controls")
        canvas.handleTap(at: CGPoint(x: 8, y: targetY))
        #expect(canvas.firstVisiblePage == 0)
    }
}
