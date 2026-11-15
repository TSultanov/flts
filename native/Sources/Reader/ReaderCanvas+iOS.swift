#if os(iOS)
import UIKit

/// The reader's touch input: tap, swipe, long press, a hardware keyboard, and
/// VoiceOver. What they do lives in `ReaderCanvas`.
extension ReaderCanvas: @preconcurrency UIEditMenuInteractionDelegate {
    @MainActor private final class TouchState {
        var editMenu: UIEditMenuInteraction?
        var menuHit: ChapterTextLayout.Hit?
        let noKeyboard = UIView(frame: .zero)
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
        // A trackpad's two-finger scroll and a mouse wheel turn pages too.
        pan.allowedScrollTypesMask = .all
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

    /// The reader takes the keys, for a hardware keyboard's arrows, Space, W
    /// and C. With an empty input view: as first responder without one, the
    /// software keyboard came up whenever a menu opened.
    override var canBecomeFirstResponder: Bool { true }
    override var inputView: UIView? { touch.noKeyboard }

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
        // A phone in landscape is wide but short: it keeps the tight margins.
        margins =
            bounds.width < 600 || bounds.height < 500
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
        // A finger is not a pointer: a tap just beside a word still means the word.
        if let word = nearestWord(to: point) {
            selectWord(at: word)
            return
        }
        // In the margins (not over text), a tap turns the page, as in other readers.
        if point.x > bounds.width - margins.right { return nextSpread() }
        if point.x < margins.left - 2 * controlSlop - 22 { return previousSpread() }
        selectWord(at: point)
    }

    /// The point itself if it is on a word, else the closest of a few points
    /// around it that is; short words are narrower than a fingertip.
    private func nearestWord(to point: CGPoint) -> CGPoint? {
        let reach: [CGFloat] = [0, 7, 14]
        for radius in reach {
            let offsets: [CGPoint] =
                radius == 0
                ? [.zero]
                : [
                    CGPoint(x: -radius, y: 0), CGPoint(x: radius, y: 0), CGPoint(x: 0, y: -radius), CGPoint(x: 0, y: radius),
                    CGPoint(x: -radius, y: -radius), CGPoint(x: radius, y: -radius), CGPoint(x: -radius, y: radius),
                    CGPoint(x: radius, y: radius),
                ]
            for offset in offsets {
                let candidate = CGPoint(x: point.x + offset.x, y: point.y + offset.y)
                if hit(candidate)?.wordIndex != nil { return candidate }
            }
        }
        return nil
    }

    /// The spread follows the finger; lifting it turns the spread or lets it spring back.
    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        switch gesture.state {
        case .changed:
            let moved = gesture.translation(in: self)
            // Sideways drags only, unless the spread is already on the move.
            if abs(moved.x) > abs(moved.y) || slide != 0 { slide(to: moved.x) }
        case .ended:
            slideEnded(velocity: gesture.velocity(in: self).x)
        case .cancelled, .failed:
            slideEnded(velocity: 0)
        default:
            break
        }
    }

    @objc private func longPressed(_ gesture: UILongPressGestureRecognizer) {
        guard gesture.state == .began else { return }
        let point = gesture.location(in: self)
        guard let hit = hit(nearestWord(to: point) ?? point) else { return }
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
            // The sentence the word is in (there is no drag selection by touch).
            let sentence = p.words.filter { $0.sentence == word.sentence }
            if let first = sentence.first, let last = sentence.last {
                let range = NSRange(location: first.start, length: last.start + last.len - first.start)
                let text = (p.text as NSString).substring(with: range).replacingOccurrences(of: "\u{2028}", with: " ")
                actions.append(UIAction(title: "Copy Sentence") { _ in UIPasteboard.general.string = text })
            }
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
