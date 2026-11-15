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
}

/// Drives the real app on a seeded library (`FLTS_UITEST_CONFIG_DIR`), asserting
/// behaviour and attaching screenshots to the result bundle (the runner is
/// sandboxed and cannot write files).
@MainActor
final class FLTSScreenshotTour: XCTestCase {
    private func launch(
        dir: String, dark: Bool = false, smallest: Bool = false, animations: Bool = false, env: [String: String] = [:]
    ) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1"]
        app.launchEnvironment.merge(env) { _, new in new }
        // Only the dark launch pins the setting: an argument outranks what the app saves.
        app.launchArguments = ["-ApplePersistenceIgnoreState", "YES"] + (dark ? ["-appearance", "dark"] : [])
        // Set by the app itself, so the run does not depend on the last saved
        // frame. Below the minimum, the window gets its smallest allowed size.
        app.launchEnvironment["FLTS_WINDOW_SIZE"] = smallest ? "400x300" : "1180x800"
        // No animations, in the app or of its windows and sheets: the runner
        // waits for the app to settle after each one.
        if !animations {
            app.launchEnvironment["FLTS_NO_ANIMATIONS"] = "1"
            app.launchArguments += ["-NSAutomaticWindowAnimationsEnabled", "NO", "-NSWindowResizeTime", "0.001"]
        }
        app.launch()
        return app
    }

    /// Lets an animation or a layout pass finish; waits on state use expectations.
    private func pause(_ seconds: Double = 0.05) {
        Thread.sleep(forTimeInterval: seconds)
    }

    /// The tapped word's translation has loaded.
    private func waitWord(_ app: XCUIApplication) {
        let button = app.buttons["Show word details"].firstMatch
        XCTAssertTrue(until(5) { button.exists && button.isEnabled }, "the word's translation did not load")
    }

    private func shot(_ element: XCUIElement, _ name: String) {
        let attachment = XCTAttachment(screenshot: element.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// The page label must change: proves keys still reach the reader.
    private func assertFlips(_ app: XCUIApplication, _ context: String) {
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        let before = page.label
        // "Pages a to b of n": on the last spread only going back can change it.
        let numbers = before.split(whereSeparator: { !$0.isNumber }).compactMap { Int($0) }
        let atEnd = numbers.count >= 2 && numbers[numbers.count - 2] >= numbers[numbers.count - 1]
        app.typeKey(atEnd ? .leftArrow : .rightArrow, modifierFlags: [])
        XCTAssertTrue(until(3) { page.label != before }, "arrow key ignored \(context)")
    }

    /// A book-sized chapter (`FLTS_UITEST_LONG_EPUB`, see gen-long-epub.py), with
    /// real animations: the chapter list must come and go without the reader
    /// being resized or re-paged, and as quickly as on a short book.
    func testLongBookChapters() throws {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["FLTS_UITEST_STRESS_DIR"], let epub = env["FLTS_UITEST_LONG_EPUB"], !epub.isEmpty else {
            throw XCTSkip("long book only")
        }
        let app = launch(dir: dir, animations: true)
        XCTAssertTrue(app.staticTexts["Lluvia en la ciudad"].firstMatch.appears(timeout: 15))
        app.typeKey("o", modifierFlags: .command)
        app.buttons["Choose EPUB…"].firstMatch.click()
        pause(1)
        app.typeKey("g", modifierFlags: [.command, .shift])
        pause(0.5)
        app.typeText(epub + "\r")
        pause(0.5)
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.sheets.firstMatch.checkBoxes.firstMatch.appears(timeout: 20))
        app.typeKey(.return, modifierFlags: [])
        let long = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Novela larga' OR value CONTAINS 'Novela larga'")).firstMatch
        XCTAssertTrue(long.appears(timeout: 30), "the long book was not imported")

        func toggleTimes(_ title: XCUIElement) -> [Double] {
            // Once the import sheet (animated here) is out of the way.
            pause(1.5)
            title.doubleClick()
            let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
            XCTAssertTrue(page.appears(timeout: 30))
            // The whole chapter has been laid out once the page count is known.
            XCTAssertTrue(until(60) { page.label.contains(" of ") }, "the chapter never finished laying out")
            let frame = page.frame
            let label = page.label
            var times: [Double] = []
            for _ in 0..<4 {
                var start = Date()
                app.typeKey("c", modifierFlags: [])
                XCTAssertTrue(until(5) { app.outlines.firstMatch.exists })
                times.append(Date().timeIntervalSince(start))
                XCTAssertEqual(page.frame, frame, "showing the chapters resized the reader")
                start = Date()
                app.typeKey(.escape, modifierFlags: [])
                XCTAssertTrue(until(5) { !app.outlines.firstMatch.exists })
                times.append(Date().timeIntervalSince(start))
            }
            XCTAssertEqual(page.label, label, "toggling the chapters re-paged the chapter")
            print("CHAPTER-TOGGLE \(label): " + times.map { String(format: "%.2f", $0) }.joined(separator: " "))
            app.typeKey("l", modifierFlags: [.command, .option])
            return times
        }
        let longTimes = toggleTimes(long)
        let short = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves' OR value CONTAINS 'Cuentos breves'")).firstMatch
        XCTAssertTrue(short.appears(timeout: 10))
        let shortTimes = toggleTimes(short)
        let median = { (t: [Double]) in t.sorted()[t.count / 2] }
        XCTAssertLessThan(median(longTimes), median(shortTimes) + 0.25, "the chapter list is slower on a long chapter")
        app.terminate()
    }

    /// A copy of a real library (`FLTS_UITEST_REAL_DIR`), with the translation
    /// service pointed nowhere: the library lists, a book opens and pages.
    func testRealLibraryCopy() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_REAL_DIR"], !dir.isEmpty else {
            throw XCTSkip("real library copy only")
        }
        let app = launch(dir: dir, env: ["FLTS_GEMINI_BASE_URL": "http://127.0.0.1:9/"])
        let window = app.windows.firstMatch
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "value CONTAINS ' translated' OR value CONTAINS 'chapter'")).firstMatch
            .appears(timeout: 30), "the library did not list any book")
        pause(0.5)
        for triangle in app.disclosureTriangles.allElementsBoundByIndex where triangle.value as? Int == 0 {
            triangle.click()
        }
        pause(0.3)
        shot(window, "real-0-library")
        // The first book, from the keyboard.
        app.typeKey(.downArrow, modifierFlags: [])
        app.typeKey(.downArrow, modifierFlags: [])
        app.typeKey(.return, modifierFlags: [])
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(page.appears(timeout: 30), "the book did not open")
        XCTAssertTrue(until(60) { page.label.contains(" of ") }, "the chapter never finished laying out")
        pause(0.5)
        shot(window, "real-1-reader")
        assertFlips(app, "in a real book")
        // A translated word, wherever the page has one: its details open.
        let details = app.buttons["Show word details"].firstMatch
        var found = false
        for step in 0..<24 where !found {
            let point = CGVector(dx: step % 2 == 0 ? 0.12 : 0.62, dy: 0.09 + 0.035 * Double(step / 2))
            page.coordinate(withNormalizedOffset: point).click()
            found = until(1.2) { details.exists && details.isEnabled }
        }
        XCTAssertTrue(found, "no translated word could be opened on this spread")
        app.typeKey("w", modifierFlags: [])
        XCTAssertTrue(app.buttons["Hide word details"].firstMatch.appears(timeout: 3), "the word's details did not open")
        pause(0.5)
        shot(window, "real-2-word")
        app.typeKey("w", modifierFlags: [])
        if app.buttons["Chapters"].firstMatch.exists {
            app.typeKey("c", modifierFlags: [])
            XCTAssertTrue(app.outlines.firstMatch.appears(timeout: 3))
            pause(0.3)
            shot(window, "real-3-chapters")
        }
        app.terminate()
    }

    /// Translation end to end against the e2e sims (scripts/ui-translate.sh):
    /// import a text, translate its chapter, read a word, translate the
    /// paragraph again with a chosen model, and add an Anki card.
    func testTranslateWithSims() throws {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["FLTS_UITEST_SIM_DIR"], let textFile = env["FLTS_UITEST_SIM_TEXT"], let llm = env["FLTS_UITEST_SIM_LLM"],
            !dir.isEmpty
        else { throw XCTSkip("sims only") }
        let paragraphs = try String(contentsOfFile: textFile, encoding: .utf8).components(separatedBy: "\n\n").prefix(2)
        let app = launch(dir: dir, env: ["FLTS_GEMINI_BASE_URL": llm])
        let window = app.windows.firstMatch
        XCTAssertTrue(app.staticTexts["Lluvia en la ciudad"].firstMatch.appears(timeout: 15))

        app.typeKey("o", modifierFlags: .command)
        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.appears(timeout: 5))
        sheet.radioButtons["Text"].click()
        sheet.textFields.firstMatch.click()
        sheet.typeText("Texto de prueba")
        sheet.textViews.firstMatch.click()
        sheet.typeText(paragraphs.joined(separator: "\n\n"))
        sheet.buttons["Import"].click()
        let book = app.staticTexts["Texto de prueba"].firstMatch
        XCTAssertTrue(book.appears(timeout: 10), "the text was not imported")
        pause(0.3)
        book.doubleClick()
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(page.appears(timeout: 10))

        // Translate the chapter from the toolbar; the button goes once nothing is left.
        let translate = app.buttons["Translate Chapter"].firstMatch
        XCTAssertTrue(translate.appears(timeout: 5), "no Translate Chapter on an untranslated text")
        XCTAssertTrue(until(10) { translate.isEnabled }, "Translate Chapter stayed disabled")
        translate.click()
        XCTAssertTrue(until(30) { !translate.exists }, "the chapter was not translated")
        shot(window, "t-1-translated")

        // A word of it: its translation shows in the bar and in the details.
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.095, dy: 0.088)).click()
        waitWord(app)
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "value BEGINSWITH 'en-'")).firstMatch.appears(timeout: 3),
            "the word's translation is not shown")
        app.typeKey("w", modifierFlags: [])
        XCTAssertTrue(app.buttons["Hide word details"].firstMatch.appears(timeout: 3))
        pause(0.2)
        shot(window, "t-2-word")

        // Translate the paragraph again, with a model picked from the list.
        let again = app.buttons["Translate Again"].firstMatch
        XCTAssertTrue(again.appears(timeout: 3))
        let models = app.popUpButtons.firstMatch
        if models.exists {
            models.click()
            let items = models.menuItems
            XCTAssertTrue(items.firstMatch.appears(timeout: 3), "the model list did not open")
            items.element(boundBy: items.count > 1 ? 1 : 0).click()
        }
        again.click()
        XCTAssertTrue(until(20) { again.isEnabled }, "translating again never finished")
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "value BEGINSWITH 'Translation failed'")).firstMatch.exists,
            "translating again failed")
        // The word is still there afterwards.
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "value BEGINSWITH 'en-'")).firstMatch.appears(timeout: 5))

        // An Anki card for the word, in the sim.
        let card = app.buttons["Add to Anki"].firstMatch
        XCTAssertTrue(card.appears(timeout: 5), "no Add to Anki with Anki configured")
        card.click()
        XCTAssertTrue(until(10) { !card.exists }, "the card was not created")
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "value CONTAINS[c] 'anki'")).firstMatch.exists, "an Anki error is shown")
        pause(0.2)
        shot(window, "t-3-after")
        app.terminate()
    }

    /// Sync settings with the real engine, kept to this Mac (`FLTS_SYNC_HERMETIC`)
    /// on a throwaway library: turning sync on brings it online with a pairing
    /// code and device ID, the device renames, and sync turns off again.
    func testSyncSettingsLocalEngine() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_SYNC_DIR"], !dir.isEmpty else {
            throw XCTSkip("local sync engine only")
        }
        let app = launch(dir: dir, env: ["FLTS_DISABLE_SYNC": "", "FLTS_SYNC_HERMETIC": "1"])
        XCTAssertTrue(app.staticTexts["Lluvia en la ciudad"].firstMatch.appears(timeout: 15))
        app.typeKey(",", modifierFlags: .command)
        let syncTab = app.toolbars.buttons["Sync"].firstMatch
        XCTAssertTrue(syncTab.appears(timeout: 5))
        syncTab.click()
        let toggle = app.switches.firstMatch.exists ? app.switches.firstMatch : app.checkBoxes.firstMatch
        XCTAssertTrue(toggle.appears(timeout: 5), "no sync toggle")
        toggle.click()
        let online = app.staticTexts.matching(NSPredicate(format: "value BEGINSWITH 'Online' OR label BEGINSWITH 'Online'")).firstMatch
        XCTAssertTrue(online.appears(timeout: 90), "sync did not come online")
        XCTAssertTrue(app.images["Pairing code"].firstMatch.appears(timeout: 10), "no pairing code")
        XCTAssertTrue(app.buttons["Copy Device ID"].firstMatch.exists, "no device ID")
        pause(0.3)
        shot(app.windows.firstMatch, "s-1-sync-online")

        // Rename this device; the button goes quiet once the name is saved.
        let name = app.textFields.firstMatch
        name.click()
        name.typeKey("a", modifierFlags: .command)
        name.typeText("Mac de prueba\r")
        let rename = app.buttons["Rename"].firstMatch
        XCTAssertTrue(until(15) { !rename.isEnabled }, "the device was not renamed")
        XCTAssertEqual(name.value as? String, "Mac de prueba")
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "value CONTAINS[c] 'failed' OR value CONTAINS[c] 'error'")).firstMatch.exists,
            "sync settings show an error")

        // The library's toolbar now carries the sync status.
        app.typeKey("w", modifierFlags: .command)
        let status = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Online' OR label BEGINSWITH 'Synced'")).firstMatch
        XCTAssertTrue(status.appears(timeout: 10), "no sync status in the library toolbar")
        shot(app.windows.firstMatch, "s-2-library-sync")
        // Its click opens the Sync tab; turn sync off again there.
        status.click()
        XCTAssertTrue(toggle.appears(timeout: 5), "the status button did not open the Sync settings")
        toggle.click()
        XCTAssertTrue(until(60) { !online.exists }, "sync did not turn off")
        // And on again, to quit with the engine running.
        toggle.click()
        XCTAssertTrue(online.appears(timeout: 90), "sync did not come back online")
        let start = Date()
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 20), "the app did not quit with the engine running")
        print(String(format: "QUIT-WITH-SYNC %.1f s", Date().timeIntervalSince(start)))
    }

    func testLaunchStress() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_STRESS_DIR"] else { throw XCTSkip("stress only") }
        var crashes = 0
        for _ in 0..<60 {
            let app = launch(dir: dir, animations: true)
            // Past the first second after the library shows, where it has died.
            let shown = app.staticTexts["Lluvia en la ciudad"].firstMatch.appears(timeout: 8)
            pause(1.2)
            if !shown || !app.windows.firstMatch.exists { crashes += 1 }
            app.terminate()
        }
        print("STRESS-CRASHES \(crashes)/60")
        XCTAssertEqual(crashes, 0)
    }

    func testTour() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_CONFIG_DIR"] else {
            throw XCTSkip("set FLTS_UITEST_CONFIG_DIR")
        }
        let app = launch(dir: dir)
        let window = app.windows.firstMatch
        let book = app.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(book.appears(timeout: 15))
        pause()
        shot(window, "0-library")

        book.doubleClick()
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(page.appears(timeout: 10))
        pause(0.15)
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.12)).click()
        pause()
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.55, dy: 0.2)).click()
        waitWord(app)
        shot(window, "1-reader-word")

        app.typeKey("w", modifierFlags: [])
        XCTAssertTrue(app.buttons["Hide word details"].firstMatch.appears(timeout: 3), "W did not expand the word")
        pause()
        shot(window, "2-word-expanded")
        // Another word with the details open: they stay open while it loads.
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.12)).click()
        let hide = app.buttons["Hide word details"].firstMatch
        XCTAssertFalse(until(0.6) { !hide.exists }, "the details collapsed on tapping another word")
        app.typeKey("w", modifierFlags: [])
        // A click on empty page space deselects the word.
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.985)).click()
        let hint = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Select a word' OR value BEGINSWITH 'Select a word'")).firstMatch
        XCTAssertTrue(hint.appears(timeout: 3), "clicking empty space did not deselect")
        // Past the double-click interval, or the next click selects by word.
        pause(0.15)
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.55, dy: 0.2)).click()
        waitWord(app)
        assertFlips(app, "after tapping words")
        app.typeKey("w", modifierFlags: [])
        XCTAssertTrue(app.buttons["Hide word details"].firstMatch.appears(timeout: 3))
        app.buttons["Hide word details"].firstMatch.click()
        pause()
        assertFlips(app, "after clicking in the word panel")

        // A one-chapter book has no chapter list (as in the web app), nor a toggle for it.
        app.typeKey("c", modifierFlags: [])
        pause()
        XCTAssertFalse(app.outlines.firstMatch.exists, "a one-chapter book shows a chapter list")
        XCTAssertFalse(app.buttons["Chapters"].firstMatch.exists, "a one-chapter book has a chapters button")
        assertFlips(app, "after C on a one-chapter book")

        // A two-finger scroll turns the spread, either way.
        let scrolled = page.label
        page.scroll(byDeltaX: -400, deltaY: 0)
        XCTAssertTrue(until(3) { page.label != scrolled }, "a scroll did not turn the page")
        page.scroll(byDeltaX: 400, deltaY: 0)
        XCTAssertTrue(until(3) { page.label == scrolled }, "a scroll back did not return")

        // Text size from the keyboard, past the upper limit and back (this used to crash).
        for _ in 0..<3 { app.typeKey("+", modifierFlags: .command) }
        pause()
        shot(window, "4-larger-text")
        app.typeKey("0", modifierFlags: .command)
        XCTAssertTrue(page.appears(timeout: 3), "the reader did not survive a text size change")
        // ⌘= is ⌘+ without Shift, and has to work too.
        let sized = page.label
        app.typeKey("=", modifierFlags: .command)
        XCTAssertTrue(until(3) { page.label != sized }, "⌘= did not enlarge the text")
        app.typeKey("0", modifierFlags: .command)
        assertFlips(app, "after changing the text size")

        let from = page.coordinate(withNormalizedOffset: CGVector(dx: 0.15, dy: 0.3))
        from.click(forDuration: 0.2, thenDragTo: page.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.32)))
        pause()
        shot(window, "4-selection")

        app.buttons["Library"].firstMatch.click()
        pause()
        // Move the book into a new folder through the UI.
        book.rightClick()
        app.menuItems["Move to Folder…"].click()
        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.appears(timeout: 5))
        shot(window, "5-move")
        sheet.buttons["New Folder"].firstMatch.click()
        XCTAssertTrue(sheet.textFields.firstMatch.appears(timeout: 3), "no field for the new folder's name")
        sheet.typeText("Cuentos")
        app.typeKey(.return, modifierFlags: [])  // adds the folder and selects it
        XCTAssertTrue(until(3) { (sheet.staticTexts["move-destination"].value as? String ?? sheet.staticTexts["move-destination"].label).hasSuffix("/Cuentos") })
        shot(window, "5-move-new-folder")
        app.typeKey(.return, modifierFlags: [])  // the default button
        pause(0.15)
        let folder = app.descendants(matching: .any).matching(NSPredicate(format: "label == 'Cuentos' OR title == 'Cuentos' OR value == 'Cuentos'")).firstMatch
        XCTAssertTrue(folder.appears(timeout: 5), "folder row after moving the book")
        app.disclosureTriangles.firstMatch.click()
        pause()
        shot(window, "6-library-folder")

        // The arrow keys reach a folder, open or closed: Left closes it, and from
        // the row below, Up lands on the closed folder, where Right opens it again.
        folder.click()
        app.typeKey(.leftArrow, modifierFlags: [])
        XCTAssertTrue(until(3) { !book.exists }, "Left did not close the selected folder")
        app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves' OR value CONTAINS 'Cuentos breves'")).firstMatch.click()
        app.typeKey(.upArrow, modifierFlags: [])
        app.typeKey(.rightArrow, modifierFlags: [])
        XCTAssertTrue(book.appears(timeout: 3), "the arrow keys did not reach the closed folder")

        // A book with chapters: the list is hidden until C, and closes after a pick.
        let epub = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves' OR value CONTAINS 'Cuentos breves'")).firstMatch
        XCTAssertTrue(epub.appears(timeout: 5), "the multi-chapter fixture book is missing")
        do {
            epub.doubleClick()
            pause(0.15)
            XCTAssertFalse(app.outlines.firstMatch.exists, "chapter list should start hidden")
            // The list goes over the page: the reader must keep its size, so its
            // text is not laid out again (that was the stutter).
            let chapterPage = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
            XCTAssertTrue(chapterPage.appears(timeout: 5))
            let readerFrame = chapterPage.frame
            app.typeKey("c", modifierFlags: [])
            XCTAssertTrue(app.outlines.firstMatch.appears(timeout: 3), "C did not show the chapters")
            XCTAssertEqual(chapterPage.frame, readerFrame, "showing the chapters resized the reader")
            pause()
            shot(window, "3-chapters")
            // By position: over the reader, XCTest's own hit check does not see the row.
            app.outlines.firstMatch.staticTexts["Uno"].firstMatch
                .coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).click()
            pause()
            XCTAssertFalse(app.outlines.firstMatch.exists, "the list did not close after picking a chapter")
            // Chapter 2 waits for chapter 1's summary, which this test library never gets.
            let translate = app.buttons["Translate Chapter"].firstMatch
            XCTAssertTrue(translate.exists, "no Translate Chapter button on an untranslated chapter")
            XCTAssertFalse(translate.isEnabled, "translating before the summaries exist")
            app.typeKey("c", modifierFlags: [])
            XCTAssertTrue(app.outlines.firstMatch.appears(timeout: 3), "C ignored after picking a chapter")
            // The list takes the keys while it is up: down, Return opens the next chapter.
            pause(0.2)
            app.typeKey(.downArrow, modifierFlags: [])
            app.typeKey(.return, modifierFlags: [])
            XCTAssertTrue(until(3) { !app.outlines.firstMatch.exists }, "Return did not pick a chapter")
            XCTAssertTrue(app.staticTexts["Dos"].firstMatch.appears(timeout: 3), "the keyboard did not open the next chapter")
            // And Escape closes it without picking.
            app.typeKey("c", modifierFlags: [])
            XCTAssertTrue(app.outlines.firstMatch.appears(timeout: 3))
            pause(0.2)
            app.typeKey(.escape, modifierFlags: [])
            XCTAssertTrue(until(3) { !app.outlines.firstMatch.exists }, "Escape did not close the chapters")
            app.typeKey("l", modifierFlags: [.command, .option])  // Book › Library
            pause()
        }

        // Batch selection bar.
        app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves' OR value CONTAINS 'Cuentos breves'"))
            .firstMatch.click()
        if app.disclosureTriangles.firstMatch.exists, app.disclosureTriangles.firstMatch.value as? Int == 0 {
            app.disclosureTriangles.firstMatch.click()
        }
        app.typeKey("a", modifierFlags: .command)
        let selected = app.staticTexts.matching(NSPredicate(format: "label ENDSWITH ' selected' OR value ENDSWITH ' selected'")).firstMatch
        XCTAssertTrue(selected.appears(timeout: 3), "no selection bar for several books")
        shot(window, "5-selection-bar")
        app.typeKey(.escape, modifierFlags: [])

        // The quick run stops here: library, reader, word panel, chapters and
        // folders are covered; import, Lyrics, Settings, dark mode, the smallest
        // window and onboarding are the full run's.
        if ProcessInfo.processInfo.environment["FLTS_UITEST_QUICK"]?.isEmpty == false {
            app.terminate()
            return
        }

        app.typeKey("o", modifierFlags: .command)
        pause(0.15)
        if let epubFile = ProcessInfo.processInfo.environment["FLTS_UITEST_EPUB"] {
            // Pick the file through the open panel, then skip one chapter.
            app.buttons["Choose EPUB…"].firstMatch.click()
            pause(0.15)
            app.typeKey("g", modifierFlags: [.command, .shift])
            pause()
            app.typeText(epubFile + "\r")
            pause()
            app.typeKey(.return, modifierFlags: [])
            let boxes = app.sheets.firstMatch.checkBoxes
            XCTAssertTrue(boxes.firstMatch.appears(timeout: 10), "no chapter choices after picking the EPUB")
            boxes.element(boundBy: 0).click()
            pause()
            shot(window, "7-import")
            app.typeKey(.return, modifierFlags: [])  // the default button
            let twoChapters = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH '2 chapters' OR value BEGINSWITH '2 chapters'")).firstMatch
            XCTAssertTrue(twoChapters.appears(timeout: 10), "imported book with the chosen chapters not in the library")
            // Delete it again: select, ⌫, confirm.
            let imported = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos nuevos' OR value CONTAINS 'Cuentos nuevos'")).firstMatch
            imported.click()
            app.typeKey(.delete, modifierFlags: [])
            let confirm = app.windows.firstMatch.descendants(matching: .button).matching(identifier: "Delete").firstMatch
            XCTAssertTrue(confirm.appears(timeout: 5), "no delete confirmation")
            confirm.click()
            XCTAssertTrue(until(10) { !imported.exists }, "deleted book still listed")
        } else {
            shot(window, "7-import")
            app.typeKey(.escape, modifierFlags: [])
        }
        pause()

        app.buttons["Lyrics"].firstMatch.click()
        let lyrics = app.windows["Lyrics"]
        XCTAssertTrue(lyrics.appears(timeout: 5))
        pause(1)
        shot(lyrics, "8-lyrics")
        app.typeKey("w", modifierFlags: .command)

        app.typeKey(",", modifierFlags: .command)
        pause(0.15)
        // The Appearance picker, clicked for real.
        app.toolbars.buttons["Reading"].firstMatch.click()
        pause()
        app.radioButtons["Dark"].firstMatch.click()
        pause()
        shot(app.windows.firstMatch, "9-settings-Reading-dark")
        shot(app.windows.element(boundBy: 1), "9-main-window-dark")
        app.radioButtons["Match System"].firstMatch.click()
        pause()
        for tab in ["Translation", "Sync", "Spotify"] {
            let button = app.toolbars.buttons[tab].firstMatch
            if button.exists { button.click() }
            pause()
            shot(app.windows.firstMatch, "9-settings-\(tab)")
        }
        // ⌘Q goes through applicationShouldTerminate (.terminateLater, bounded shutdown).
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 15), "the app did not quit")
        // Relaunch, in dark mode (the app's own appearance setting).
        let dark = launch(dir: dir, dark: true)
        XCTAssertTrue(dark.descendants(matching: .any).matching(NSPredicate(format: "label == 'Cuentos'")).firstMatch
            .appears(timeout: 15), "relaunch did not open on the library")
        if dark.disclosureTriangles.firstMatch.value as? Int == 0 {
            dark.disclosureTriangles.firstMatch.click()
        }
        let darkBook = dark.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(darkBook.appears(timeout: 10))
        darkBook.doubleClick()
        let darkPage = dark.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(darkPage.appears(timeout: 10))
        pause(0.15)
        darkPage.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.12)).click()
        pause()
        dark.typeKey("w", modifierFlags: [])
        pause()
        shot(dark.windows.firstMatch, "10-dark-reader")
        dark.typeKey("l", modifierFlags: [.command, .shift])
        let darkLyrics = dark.windows["Lyrics"]
        XCTAssertTrue(darkLyrics.appears(timeout: 5), "⇧⌘L did not open Lyrics from the reader")
        pause(0.15)
        shot(darkLyrics, "10-dark-lyrics")
        dark.typeKey("w", modifierFlags: .command)
        dark.terminate()

        // The smallest window, driven from the keyboard alone: arrows and
        // Return open a book without a click.
        let small = launch(dir: dir, smallest: true)
        let smallWindow = small.windows.firstMatch
        XCTAssertTrue(smallWindow.appears(timeout: 15))
        pause(0.15)
        shot(smallWindow, "12-min-library")
        small.typeKey(.downArrow, modifierFlags: [])
        small.typeKey(.downArrow, modifierFlags: [])
        pause()
        shot(smallWindow, "12-min-after-arrows")
        small.typeKey(.return, modifierFlags: [])
        let smallPage = small.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(smallPage.appears(timeout: 10), "arrows and Return did not open a book")
        pause(0.15)
        smallPage.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.15)).click()
        pause()
        small.typeKey("w", modifierFlags: [])
        pause()
        shot(smallWindow, "12-min-reader")
        small.terminate()

        // First launch: onboarding on an empty config dir.
        if let empty = ProcessInfo.processInfo.environment["FLTS_UITEST_EMPTY_DIR"] {
            // At the smallest window the key field must still be reachable.
            let tiny = launch(dir: empty, smallest: true)
            XCTAssertTrue(tiny.staticTexts["Welcome to FLTS"].appears(timeout: 15))
            XCTAssertTrue(tiny.secureTextFields.firstMatch.isHittable, "API key field hidden at the smallest window")
            XCTAssertTrue(tiny.buttons["Continue"].isHittable, "Continue hidden at the smallest window")
            shot(tiny.windows.firstMatch, "11-onboarding-min")
            tiny.terminate()

            let first = launch(dir: empty)
            XCTAssertTrue(first.staticTexts["Welcome to FLTS"].appears(timeout: 15))
            first.buttons["Continue"].click()
            XCTAssertTrue(first.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Paste an API key' OR value BEGINSWITH 'Paste an API key'")).firstMatch
                .appears(timeout: 5), "onboarding says nothing about the missing key")
            pause()
            shot(first.windows.firstMatch, "11-onboarding")
            // Finish it: a key, Return, and the app moves on to the (empty) library.
            let key = first.secureTextFields.firstMatch
            key.click()
            key.typeText("test-key\r")
            XCTAssertTrue(first.staticTexts.matching(NSPredicate(format: "label == 'Your Library Is Empty' OR value == 'Your Library Is Empty'"))
                .firstMatch.appears(timeout: 10), "onboarding did not lead to the library")
            shot(first.windows.firstMatch, "11-after-onboarding")

            // A pasted text, through the import sheet; switching tabs keeps what was typed.
            first.typeKey("o", modifierFlags: .command)
            let importSheet = first.sheets.firstMatch
            XCTAssertTrue(importSheet.appears(timeout: 5))
            importSheet.radioButtons["Text"].click()
            importSheet.textFields.firstMatch.click()
            importSheet.typeText("Nota breve")
            importSheet.textViews.firstMatch.click()
            importSheet.typeText("Hola. Esto es una nota corta para leer.")
            importSheet.radioButtons["EPUB"].click()
            importSheet.radioButtons["Text"].click()
            XCTAssertEqual(importSheet.textFields.firstMatch.value as? String, "Nota breve", "switching tabs lost the title")
            pause()
            shot(first.windows.firstMatch, "14-import-text")
            importSheet.buttons["Import"].click()
            XCTAssertTrue(first.staticTexts.matching(NSPredicate(format: "label == 'Nota breve' OR value == 'Nota breve'"))
                .firstMatch.appears(timeout: 10), "pasted text not in the library")
            pause()
            shot(first.windows.firstMatch, "14-after-import-text")
            first.terminate()
        }
    }
}
