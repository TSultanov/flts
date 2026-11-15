#if os(iOS)
import UIKit

/// The reader's touch input: tap, swipe, long press, a hardware keyboard, and
/// VoiceOver. What they do lives in `ReaderCanvas`.
extension ReaderCanvas: @preconcurrency UIEditMenuInteractionDelegate {
    private final class TouchState {
        var editMenu: UIEditMenuInteraction?
        var menuHit: ChapterTextLayout.Hit?
    }

    private var touch: TouchState {
        if let state = platformState as? TouchState { return state }
        let state = TouchState()
        platformState = state
        return state
    }

    func platformSetUp() {
        isOpaque = true
        contentMode = .redraw
        backgroundColor = .systemBackground
        isAccessibilityElement = true
        accessibilityTraits = [.staticText, .adjustable]
        accessibilityHint = "Swipe up or down to turn the page."

        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped(_:))))
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        pan.maximumNumberOfTouches = 1
        addGestureRecognizer(pan)
        addGestureRecognizer(UILongPressGestureRecognizer(target: self, action: #selector(longPressed(_:))))
        let menu = UIEditMenuInteraction(delegate: self)
        addInteraction(menu)
        touch.editMenu = menu
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (canvas: ReaderCanvas, _) in canvas.redraw() }
    }

    var isResizingLive: Bool { false }
    var flipLayer: CALayer? { layer }
    /// A finger needs about 44 pt; the button is drawn 22 pt wide.
    var controlSlop: CGFloat { 11 }

    override var canBecomeFirstResponder: Bool { true }

    func takeKeys() {
        becomeFirstResponder()
    }

    func controlsMoved() {}

    func pageChanged() {
        UIAccessibility.post(notification: .pageScrolled, argument: pageLabel)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        // A phone has no room for desktop margins; the left one still holds the
        // paragraph buttons.
        margins =
            bounds.width < 600
            ? UIEdgeInsets(top: 20, left: 40, bottom: 34, right: 18) : UIEdgeInsets(top: 40, left: 56, bottom: 44, right: 56)
        sizeChanged()
    }

    override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext() else { return }
        drawContents(rect, in: ctx)
    }

    // MARK: Touch

    @objc private func tapped(_ gesture: UITapGestureRecognizer) {
        takeKeys()
        let point = gesture.location(in: self)
        if activateControl(at: point, fresh: false) { return }
        // Beside the text, a tap turns the page, as in other readers.
        if hit(point)?.wordIndex == nil, selected == nil {
            if point.x > bounds.width - max(margins.right, 44) { return nextSpread() }
            if point.x < min(margins.left, 44) { return previousSpread() }
        }
        selectWord(at: point)
    }

    /// One swipe turns one spread, on lifting the finger.
    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        guard gesture.state == .ended else { return }
        let moved = gesture.translation(in: self)
        let speed = gesture.velocity(in: self)
        guard abs(moved.x) > abs(moved.y), abs(moved.x) > 40 || abs(speed.x) > 500 else { return }
        if moved.x < 0 { nextSpread() } else { previousSpread() }
    }

    @objc private func longPressed(_ gesture: UILongPressGestureRecognizer) {
        guard gesture.state == .began else { return }
        let point = gesture.location(in: self)
        guard let hit = hit(point) else { return }
        touch.menuHit = hit
        touch.editMenu?.presentEditMenu(with: UIEditMenuConfiguration(identifier: nil, sourcePoint: point))
    }

    func editMenuInteraction(
        _ interaction: UIEditMenuInteraction, menuFor configuration: UIEditMenuConfiguration, suggestedActions: [UIMenuElement]
    ) -> UIMenu? {
        guard let hit = touch.menuHit, let p = text.paragraphs[hit.paragraphId] else { return nil }
        var actions: [UIMenuElement] = []
        if let w = hit.wordIndex, w < p.words.count {
            let word = p.words[w]
            let surface = (p.text as NSString).substring(with: NSRange(location: word.start, length: word.len))
            actions.append(UIAction(title: "Look Up", image: UIImage(systemName: "book")) { [weak self] _ in self?.lookUp(surface) })
            actions.append(UIAction(title: "Copy Word") { _ in UIPasteboard.general.string = surface })
        }
        actions.append(
            UIAction(title: "Copy Paragraph") { _ in
                UIPasteboard.general.string = p.text.replacingOccurrences(of: "\u{2028}", with: "\n")
            })
        let pid = hit.paragraphId
        actions.append(
            UIAction(title: p.translated ? "Translate Again" : "Translate", image: UIImage(systemName: "character.bubble")) {
                [weak self] _ in
                guard let self else { return }
                self.delegate?.reader(self, translate: pid, useCache: !p.translated)
            })
        return UIMenu(children: actions)
    }

    private func lookUp(_ term: String) {
        var responder: UIResponder? = self
        while let next = responder?.next {
            if let controller = next as? UIViewController {
                controller.present(UIReferenceLibraryViewController(term: term), animated: true)
                return
            }
            responder = next
        }
    }

    // MARK: Keyboard

    override var keyCommands: [UIKeyCommand]? {
        func command(_ input: String, _ action: Selector, _ modifiers: UIKeyModifierFlags = []) -> UIKeyCommand {
            let command = UIKeyCommand(input: input, modifierFlags: modifiers, action: action)
            command.wantsPriorityOverSystemBehavior = true
            return command
        }
        return [
            command(UIKeyCommand.inputRightArrow, #selector(keyNext)), command(UIKeyCommand.inputDownArrow, #selector(keyNext)),
            command(UIKeyCommand.inputPageDown, #selector(keyNext)), command(" ", #selector(keyNext)),
            command(UIKeyCommand.inputLeftArrow, #selector(keyPrevious)), command(UIKeyCommand.inputUpArrow, #selector(keyPrevious)),
            command(UIKeyCommand.inputPageUp, #selector(keyPrevious)), command(" ", #selector(keyPrevious), .shift),
            command(UIKeyCommand.inputHome, #selector(keyFirst)), command(UIKeyCommand.inputEnd, #selector(keyLast)),
            command("w", #selector(keyToggle(_:))), command("c", #selector(keyToggle(_:))),
            command(UIKeyCommand.inputEscape, #selector(keyToggle(_:))),
        ]
    }

    @objc private func keyNext() { nextSpread() }
    @objc private func keyPrevious() { previousSpread() }
    @objc private func keyFirst() { go(toPage: 0) }
    @objc private func keyLast() { go(toPage: text.pages.count - 1) }

    @objc private func keyToggle(_ command: UIKeyCommand) {
        let key = command.input == UIKeyCommand.inputEscape ? "\u{1b}" : (command.input ?? "")
        _ = delegate?.readerToggle(self, key: key)
    }

    // MARK: Accessibility

    override var accessibilityLabel: String? {
        get { pageLabel }
        set {}
    }

    override var accessibilityValue: String? {
        get { visibleText }
        set {}
    }

    override func accessibilityIncrement() {
        nextSpread()
    }

    override func accessibilityDecrement() {
        previousSpread()
    }

    override func accessibilityScroll(_ direction: UIAccessibilityScrollDirection) -> Bool {
        switch direction {
        case .left, .next, .down: nextSpread()
        case .right, .previous, .up: previousSpread()
        @unknown default: return false
        }
        return true
    }
}
#endif
