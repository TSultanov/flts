import XCTest

/// Polls quickly: XCTest's own waits re-check only once a second.
@MainActor
private func until(_ timeout: Double, _ condition: () -> Bool) -> Bool {
    let end = Date().addingTimeInterval(timeout)
    repeat {
        if condition() { return true }
        Thread.sleep(forTimeInterval: 0.05)
    } while Date() < end
    return false
}

extension XCUIElement {
    @MainActor
    fileprivate func appears(timeout: Double) -> Bool {
        until(timeout) { exists }
    }

    /// A sheet still sliding in is tapped where it was, and missed.
    fileprivate func tapSettled() {
        var last = CGRect.null
        _ = until(3) {
            let now = frame
            defer { last = now }
            return isHittable && now == last
        }
        tap()
    }
}

/// Drives the iOS app in the Simulator on a seeded library
/// (`FLTS_UITEST_CONFIG_DIR`, a host folder), attaching screenshots.
@MainActor
final class FLTSiOSTour: XCTestCase {
    private func launch(dir: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1", "FLTS_NO_ANIMATIONS": "1",
        ]
        app.launch()
        return app
    }

    private func pause(_ seconds: Double = 0.3) {
        Thread.sleep(forTimeInterval: seconds)
    }

    private func shot(_ app: XCUIApplication, _ name: String) {
        // The screen, not the app: in landscape the app's own capture is cropped.
        _ = app
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func page(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
    }

    private func launch(dir: String, env: [String: String]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1", "FLTS_NO_ANIMATIONS": "1",
        ]
        app.launchEnvironment.merge(env) { _, new in new }
        app.launch()
        return app
    }

    /// Translation end to end against the e2e sims (scripts/ios-ui-services.sh):
    /// import a text, translate its chapter, read a word, translate the
    /// paragraph again with a chosen model, add an Anki card; then move the
    /// book into a folder and delete it.
    func testTranslateWithSims() throws {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["FLTS_UITEST_SIM_DIR"], let textFile = env["FLTS_UITEST_SIM_TEXT"], let llm = env["FLTS_UITEST_SIM_LLM"],
            !dir.isEmpty
        else { throw XCTSkip("sims only") }
        let paragraphs = try String(contentsOfFile: textFile, encoding: .utf8).components(separatedBy: "\n\n").prefix(2)
        let app = launch(dir: dir, env: ["FLTS_GEMINI_BASE_URL": llm])
        XCTAssertTrue(app.staticTexts["Lluvia en la ciudad"].firstMatch.appears(timeout: 30))

        app.buttons["Import"].firstMatch.tap()
        XCTAssertTrue(app.buttons["Text"].firstMatch.appears(timeout: 5))
        app.buttons["Text"].firstMatch.tap()
        let title = app.textFields["Title"].firstMatch
        XCTAssertTrue(title.appears(timeout: 5))
        title.tap()
        title.typeText("Texto de prueba")
        let body = app.textViews.firstMatch
        body.tap()
        body.typeText(paragraphs.joined(separator: "\n\n"))
        shot(app, "t-0-import-text")
        // The Text pane's button; the EPUB pane keeps its own, hidden.
        app.buttons.matching(identifier: "confirm-import").allElementsBoundByIndex.first(where: \.isHittable)?.tap()
        let book = app.staticTexts["Texto de prueba"].firstMatch
        XCTAssertTrue(book.appears(timeout: 15), "the text was not imported")
        pause()
        book.tap()
        let page = page(app)
        XCTAssertTrue(page.appears(timeout: 15))

        let translate = app.buttons["Translate Chapter"].firstMatch
        XCTAssertTrue(translate.appears(timeout: 5), "no Translate Chapter on an untranslated text")
        XCTAssertTrue(until(10) { translate.isEnabled }, "Translate Chapter stayed disabled")
        XCTAssertTrue(until(20) { page.label.contains(" of ") })

        // The first paragraph's own button, in the gutter beside its first line:
        // drawn 22 pt wide, it must take a finger a little off its centre.
        let compact = page.frame.width < 600 || page.frame.height < 500
        let gutter = CGVector(dx: (compact ? 40 : 56) - 21 + 9, dy: (compact ? 20 : 40) + 17 + 8)
        page.coordinate(withNormalizedOffset: .zero).withOffset(gutter).tap()
        let firstLine = page.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: (compact ? 40 : 56) + 70, dy: (compact ? 20 : 40) + 18))
        let wordButton = app.buttons["Show word details"].firstMatch
        var translatedOne = false
        for _ in 0..<20 where !translatedOne {
            firstLine.tap()
            translatedOne = until(1) { wordButton.exists && wordButton.isEnabled }
        }
        XCTAssertTrue(translatedOne, "the paragraph's own translate button did not translate it")
        XCTAssertTrue(translate.exists, "the rest of the chapter should still be untranslated")
        shot(app, "t-1-one-paragraph")
        translate.tap()
        XCTAssertTrue(until(40) { !translate.exists }, "the chapter was not translated")
        shot(app, "t-1-translated")

        let details = app.buttons["Show word details"].firstMatch
        var found = false
        for step in 0..<16 where !found {
            page.coordinate(withNormalizedOffset: CGVector(dx: 0.25 + 0.1 * Double(step % 3), dy: 0.06 + 0.03 * Double(step / 3))).tap()
            found = until(1.5) { details.exists && details.isEnabled }
        }
        XCTAssertTrue(found, "no translated word could be selected")
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'en-'")).firstMatch.appears(timeout: 3),
            "the word's translation is not shown")
        details.tap()
        let again = app.buttons["Translate Again"].firstMatch
        XCTAssertTrue(again.appears(timeout: 5))
        pause()
        shot(app, "t-2-word")
        again.tap()
        XCTAssertTrue(until(30) { again.isEnabled }, "translating again never finished")
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'en-'")).firstMatch.appears(timeout: 5))

        let card = app.buttons["Add to Anki"].firstMatch
        XCTAssertTrue(card.appears(timeout: 5), "no Add to Anki with Anki configured")
        card.tap()
        XCTAssertTrue(until(10) { !card.exists }, "the card was not created")
        pause()
        shot(app, "t-3-after")
        app.buttons["Hide word details"].firstMatch.tap()
        app.buttons["Library"].firstMatch.tap()

        // Library actions from the long-press menu: move into a new folder, then delete.
        XCTAssertTrue(book.appears(timeout: 10))
        book.press(forDuration: 1.0)
        let move = app.buttons["Move to Folder…"].firstMatch
        XCTAssertTrue(move.appears(timeout: 5), "no context menu on a book")
        shot(app, "t-4-menu")
        move.tap()
        let newFolder = app.buttons["New Folder"].firstMatch
        XCTAssertTrue(newFolder.appears(timeout: 5), "the move sheet did not open")
        shot(app, "t-5-move-tree")
        newFolder.tap()
        let folderName = app.textFields["new-folder-name"].firstMatch
        XCTAssertTrue(folderName.appears(timeout: 5), "no field for the new folder's name")
        folderName.typeText("Pruebas")
        shot(app, "t-5-move")
        app.buttons["Move"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Pruebas"].firstMatch.appears(timeout: 10), "the folder did not appear")
        pause()
        shot(app, "t-6-folder")
        if !book.exists { app.staticTexts["Pruebas"].firstMatch.tap() }
        XCTAssertTrue(book.appears(timeout: 5), "the moved book is not under its folder")
        book.press(forDuration: 1.0)
        let delete = app.buttons["Delete…"].firstMatch
        XCTAssertTrue(delete.appears(timeout: 5))
        delete.tap()
        let confirm = app.buttons["Delete"].firstMatch
        XCTAssertTrue(confirm.appears(timeout: 5), "no delete confirmation")
        confirm.tapSettled()
        XCTAssertTrue(until(10) { !book.exists }, "the deleted book is still listed")
        app.terminate()
    }

    /// The path a real device takes: no test folder, so the app uses its own
    /// container and standard defaults. Stage 1 (a fresh install) goes through
    /// onboarding; the script then puts a library where the Tauri app keeps
    /// its own; stage 2 must find it.
    func testOwnContainer() throws {
        let env = ProcessInfo.processInfo.environment
        guard let stage = env["FLTS_UITEST_CONTAINER_STAGE"], !stage.isEmpty else { throw XCTSkip("container check only") }
        let app = XCUIApplication()
        app.launchEnvironment = ["FLTS_GEMINI_BASE_URL": "http://127.0.0.1:9/"]
        app.launch()
        if stage == "1" {
            XCTAssertTrue(app.staticTexts["Welcome to FLTS"].firstMatch.appears(timeout: 40), "a fresh install did not start with onboarding")
            let key = app.secureTextFields.firstMatch
            XCTAssertTrue(key.appears(timeout: 5))
            key.tap()
            key.typeText("test-key\n")
            let empty = app.staticTexts["Your Library Is Empty"].firstMatch
            if !empty.appears(timeout: 3) { app.buttons["Continue"].firstMatch.tap() }
            XCTAssertTrue(empty.appears(timeout: 15), "onboarding did not lead to the library")
            shot(app, "c-1-fresh")
        } else {
            XCTAssertTrue(app.staticTexts["Lluvia en la ciudad"].firstMatch.appears(timeout: 40), "the library placed in the container was not found")
            XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves'")).firstMatch.exists)
            shot(app, "c-2-existing-data")
            app.staticTexts["Lluvia en la ciudad"].firstMatch.tap()
            let page = page(app)
            XCTAssertTrue(page.appears(timeout: 15), "a book from the existing data did not open")
        }
        app.terminate()
    }

    /// A copy of a real library (`FLTS_UITEST_REAL_DIR`), with the translation
    /// service pointed nowhere: it lists, a book opens, a word shows its details.
    func testRealLibraryCopy() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_REAL_DIR"], !dir.isEmpty else {
            throw XCTSkip("real library copy only")
        }
        let app = launch(dir: dir, env: ["FLTS_GEMINI_BASE_URL": "http://127.0.0.1:9/"])
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'chapter'")).firstMatch.appears(timeout: 40),
            "the library did not list any book")
        pause(0.5)
        shot(app, "r-0-library")
        // The most translated book: its row says "100% translated" or names no share.
        let rows = app.staticTexts.matching(NSPredicate(format: "label MATCHES '.*chapters? · (9[0-9]|100)% translated' OR label MATCHES '[0-9]+ chapters'"))
        XCTAssertTrue(rows.firstMatch.appears(timeout: 5), "no well-translated book to open")
        rows.firstMatch.tap()
        let page = page(app)
        XCTAssertTrue(page.appears(timeout: 30), "the book did not open")
        XCTAssertTrue(until(90) { page.label.contains(" of ") }, "the chapter never finished laying out")
        pause(0.5)
        shot(app, "r-1-reader")
        let details = app.buttons["Show word details"].firstMatch
        var found = false
        for step in 0..<24 where !found {
            page.coordinate(withNormalizedOffset: CGVector(dx: 0.2 + 0.15 * Double(step % 4), dy: 0.1 + 0.05 * Double(step / 4))).tap()
            found = until(1.5) { details.exists && details.isEnabled }
        }
        XCTAssertTrue(found, "no translated word could be opened on this page")
        shot(app, "r-2-word")
        details.tap()
        XCTAssertTrue(app.buttons["Hide word details"].firstMatch.appears(timeout: 5))
        pause(0.5)
        shot(app, "r-3-word-details")
        app.buttons["Hide word details"].firstMatch.tap()
        if app.buttons["Chapters"].firstMatch.exists {
            app.buttons["Chapters"].firstMatch.tap()
            pause(0.8)
            shot(app, "r-4-chapters")
        }
        app.terminate()
    }

    /// Sync with the real engine inside the iOS build, kept to this machine:
    /// it comes online with a pairing code, and turns off again.
    func testSyncLocalEngine() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_SYNC_DIR"], !dir.isEmpty else {
            throw XCTSkip("local sync engine only")
        }
        let app = launch(dir: dir, env: ["FLTS_DISABLE_SYNC": "", "FLTS_SYNC_HERMETIC": "1"])
        XCTAssertTrue(app.staticTexts["Lluvia en la ciudad"].firstMatch.appears(timeout: 30))
        app.buttons["Settings"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Sync"].firstMatch.appears(timeout: 5))
        app.staticTexts["Sync"].firstMatch.tap()
        let toggle = app.switches.firstMatch
        XCTAssertTrue(toggle.appears(timeout: 5), "no sync toggle")
        toggle.switches.firstMatch.exists ? toggle.switches.firstMatch.tap() : toggle.tap()
        let online = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Online'")).firstMatch
        XCTAssertTrue(online.appears(timeout: 120), "sync did not come online")
        XCTAssertTrue(app.images["Pairing code"].firstMatch.appears(timeout: 10), "no pairing code")
        pause()
        shot(app, "s-1-sync-online")
        let name = app.textFields.firstMatch
        XCTAssertFalse((name.value as? String ?? "").isEmpty, "this device has no name")
        toggle.switches.firstMatch.exists ? toggle.switches.firstMatch.tap() : toggle.tap()
        XCTAssertTrue(until(60) { !online.exists }, "sync did not turn off")
        app.terminate()
    }

    func testTour() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_CONFIG_DIR"], !dir.isEmpty else {
            throw XCTSkip("set FLTS_UITEST_CONFIG_DIR")
        }
        let app = launch(dir: dir)
        let book = app.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(book.appears(timeout: 30), "the library did not list the book")
        pause()
        shot(app, "0-library")

        // A tap opens the book.
        book.tap()
        let page = page(app)
        XCTAssertTrue(page.appears(timeout: 15), "the book did not open")
        XCTAssertTrue(until(20) { page.label.contains(" of ") }, "the chapter never finished laying out")
        pause()
        shot(app, "1-reader")

        // A tap on a word shows its translation; the bar's button opens the details.
        let details = app.buttons["Show word details"].firstMatch
        var found = false
        for step in 0..<16 where !found {
            page.coordinate(withNormalizedOffset: CGVector(dx: 0.25 + 0.1 * Double(step % 3), dy: 0.08 + 0.04 * Double(step / 3))).tap()
            found = until(1.5) { details.exists && details.isEnabled }
        }
        XCTAssertTrue(found, "no translated word could be selected")
        shot(app, "2-word")
        XCTAssertTrue(app.buttons["Show system dictionary"].firstMatch.exists, "no system dictionary button")

        // A hardware keyboard's arrow keys page too.
        let keyed = page.label
        shot(app, "3-pre-keys")
        app.typeKey(.rightArrow, modifierFlags: [])
        XCTAssertTrue(until(3) { page.label != keyed }, "the right arrow key did not turn the page")
        app.typeKey(.leftArrow, modifierFlags: [])
        XCTAssertTrue(until(3) { page.label == keyed }, "the left arrow key did not turn back")

        // A long press on the text opens the word's menu; lifting the finger must
        // not also count as a tap (the selection stays as it was).
        let selectedWord = app.staticTexts.matching(NSPredicate(format: "label == 'sleeps' OR label == 'table' OR label == 'the'")).firstMatch
        let barBefore = selectedWord.exists ? selectedWord.label : ""
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.45, dy: 0.3)).press(forDuration: 0.9)
        let copySentence = app.descendants(matching: .any).matching(NSPredicate(format: "label == 'Copy Sentence'")).firstMatch
        pause(0.8)
        shot(app, "2-long-press")
        XCTAssertTrue(copySentence.appears(timeout: 5), "a long press did not open the reader's menu")
        copySentence.tap()
        pause()
        if !barBefore.isEmpty {
            XCTAssertTrue(app.staticTexts[barBefore].firstMatch.exists, "the long press changed the selected word")
        }
        details.tap()
        XCTAssertTrue(app.buttons["Hide word details"].firstMatch.appears(timeout: 3), "the word's details did not open")
        pause()
        shot(app, "3-word-details")
        app.buttons["Hide word details"].firstMatch.tap()

        // Landscape: an iPad shows two pages side by side.
        XCUIDevice.shared.orientation = .landscapeLeft
        pause(1.5)
        XCTAssertTrue(until(20) { page.label.contains(" of ") })
        shot(app, "3-landscape")
        if UIDevice.current.userInterfaceIdiom == .pad {
            XCTAssertTrue(page.label.contains(" to "), "no two-page spread on an iPad in landscape: \(page.label)")
        }
        XCUIDevice.shared.orientation = .portrait
        pause(1.5)
        XCTAssertTrue(until(20) { page.label.contains(" of ") })

        // Text size from the reader's own menu.
        let sized = page.label
        shot(app, "3-pre-menu")
        XCTAssertFalse(app.keyboards.firstMatch.exists, "a keyboard is up in the reader")
        app.buttons["Text Size and Appearance"].firstMatch.tap()
        let larger = app.buttons["Larger Text"].firstMatch
        XCTAssertTrue(larger.appears(timeout: 5), "no text size menu in the reader")
        larger.tap()
        larger.tap()
        pause()
        shot(app, "3-text-menu")
        XCTAssertFalse(app.keyboards.firstMatch.exists, "a keyboard came up with the reading menu")
        app.buttons["Default Size"].firstMatch.tap()
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.97)).tap()
        XCTAssertTrue(until(10) { page.label == sized }, "the default size did not come back: \(sized) → \(page.label)")

        // The keys still work after the long-press and reading menus.
        let rekeyed = page.label
        app.typeKey(.rightArrow, modifierFlags: [])
        XCTAssertTrue(until(3) { page.label != rekeyed }, "the arrow keys stopped working after a menu")
        app.typeKey(.leftArrow, modifierFlags: [])
        XCTAssertTrue(until(3) { page.label == rekeyed })

        // One swipe turns one spread, either way.
        let before = page.label
        page.swipeLeft()
        XCTAssertTrue(until(3) { page.label != before }, "a swipe did not turn the page")
        let after = page.label
        page.swipeRight()
        XCTAssertTrue(until(3) { page.label == before }, "a swipe back did not return: \(after) → \(page.label)")

        // A short, slow drag lets the page spring back; a long one turns it.
        let rested = page.label
        let from = page.coordinate(withNormalizedOffset: CGVector(dx: 0.7, dy: 0.5))
        from.press(forDuration: 0.1, thenDragTo: page.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.5)),
            withVelocity: .slow, thenHoldForDuration: 0.3)
        pause(0.6)
        XCTAssertEqual(page.label, rested, "a short drag turned the page")
        from.press(forDuration: 0.1, thenDragTo: page.coordinate(withNormalizedOffset: CGVector(dx: 0.15, dy: 0.5)),
            withVelocity: .slow, thenHoldForDuration: 0.3)
        XCTAssertTrue(until(3) { page.label != rested }, "a long drag did not turn the page")
        page.swipeRight()
        XCTAssertTrue(until(3) { page.label == rested })

        // Back to the library; a book with chapters has a Chapters button.
        app.buttons["Library"].firstMatch.tap()
        let chaptered = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves'")).firstMatch
        XCTAssertTrue(chaptered.appears(timeout: 10))
        chaptered.tap()
        XCTAssertTrue(self.page(app).appears(timeout: 15))
        let chapters = app.buttons["Chapters"].firstMatch
        XCTAssertTrue(chapters.appears(timeout: 5), "no Chapters button on a book with chapters")
        chapters.tap()
        let uno = app.staticTexts["Uno"].firstMatch
        XCTAssertTrue(uno.appears(timeout: 5), "the chapter list did not open")
        pause()
        shot(app, "4-chapters")
        uno.tap()
        XCTAssertTrue(until(5) { !app.staticTexts["Dos"].exists }, "the chapter list did not close after a pick")
        pause()
        shot(app, "5-chapter-open")
        app.buttons["Library"].firstMatch.tap()

        // Settings is a sheet with a page per section.
        let settings = app.buttons["Settings"].firstMatch
        XCTAssertTrue(settings.appears(timeout: 10))
        settings.tap()
        XCTAssertTrue(app.staticTexts["Translation"].firstMatch.appears(timeout: 5), "Settings did not open")
        pause()
        shot(app, "6-settings")
        app.staticTexts["Reading"].firstMatch.tap()
        XCTAssertTrue(app.sliders.firstMatch.appears(timeout: 5), "no text size slider")
        pause()
        shot(app, "7-settings-reading")
        app.navigationBars["Reading"].buttons.firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Translation"].firstMatch.appears(timeout: 5), "Back did not return to the settings list")
        app.staticTexts["Translation"].firstMatch.tap()
        pause()
        shot(app, "8-settings-translation")
        app.navigationBars["Translation"].buttons.firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Sync"].firstMatch.appears(timeout: 5))
        app.staticTexts["Sync"].firstMatch.tap()
        pause()
        shot(app, "9-settings-sync")
        app.navigationBars["Sync"].buttons.firstMatch.tap()
        XCTAssertTrue(app.buttons["Done"].firstMatch.appears(timeout: 5))
        app.buttons["Done"].firstMatch.tap()

        // The import sheet.
        app.buttons["Import"].firstMatch.tap()
        XCTAssertTrue(app.buttons["Choose EPUB…"].firstMatch.appears(timeout: 5), "the import sheet did not open")
        pause()
        shot(app, "10-import")
        app.buttons["Text"].firstMatch.tap()
        pause()
        shot(app, "11-import-text")
        app.buttons["Cancel"].firstMatch.tap()
        app.terminate()

        // Dark mode (the app's own appearance setting), and several books selected.
        let dark = XCUIApplication()
        dark.launchEnvironment = app.launchEnvironment
        dark.launchArguments = ["-appearance", "dark"]
        dark.launch()
        let darkBook = dark.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(darkBook.appears(timeout: 30))
        dark.buttons["Edit"].firstMatch.tap()
        darkBook.tap()
        // One book is enough for the bar while editing.
        XCTAssertTrue(dark.staticTexts.matching(NSPredicate(format: "label == '1 selected'")).firstMatch.appears(timeout: 5),
            "no selection bar for one book in edit mode")
        dark.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves'")).firstMatch.tap()
        XCTAssertTrue(dark.staticTexts.matching(NSPredicate(format: "label ENDSWITH ' selected'")).firstMatch.appears(timeout: 5),
            "no selection bar for several books")
        pause()
        shot(dark, "14-dark-selection")
        dark.buttons["Done"].firstMatch.tap()
        XCTAssertTrue(until(5) { !dark.staticTexts.matching(NSPredicate(format: "label ENDSWITH ' selected'")).firstMatch.exists },
            "the selection bar outlived edit mode")
        darkBook.tap()
        let darkPage = self.page(dark)
        XCTAssertTrue(darkPage.appears(timeout: 15))
        XCTAssertTrue(until(20) { darkPage.label.contains(" of ") })
        let darkDetails = dark.buttons["Show word details"].firstMatch
        var darkFound = false
        for step in 0..<16 where !darkFound {
            darkPage.coordinate(withNormalizedOffset: CGVector(dx: 0.25 + 0.1 * Double(step % 3), dy: 0.08 + 0.04 * Double(step / 3))).tap()
            darkFound = until(1.5) { darkDetails.exists && darkDetails.isEnabled }
        }
        XCTAssertTrue(darkFound)
        darkDetails.tap()
        pause()
        shot(dark, "15-dark-word-details")
        dark.terminate()

        // First launch: onboarding on an empty config dir, finished with a key.
        if let empty = ProcessInfo.processInfo.environment["FLTS_UITEST_EMPTY_DIR"], !empty.isEmpty {
            let first = launch(dir: empty)
            XCTAssertTrue(first.staticTexts["Welcome to FLTS"].firstMatch.appears(timeout: 30), "no onboarding on an empty config dir")
            pause()
            shot(first, "12-onboarding")
            let key = first.secureTextFields.firstMatch
            XCTAssertTrue(key.appears(timeout: 5), "no API key field")
            key.tap()
            key.typeText("test-key\n")
            let empty = first.staticTexts["Your Library Is Empty"].firstMatch
            if !empty.appears(timeout: 3) { first.buttons["Continue"].firstMatch.tap() }
            XCTAssertTrue(empty.appears(timeout: 10), "onboarding did not lead to the library")
            pause()
            shot(first, "13-empty-library")
            first.terminate()
        }
    }
}
