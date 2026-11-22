import SwiftUI

/// Modifiers that exist on one platform only, under one name for both.
extension View {
    /// The book's title with the chapter under it, cut where a panel over the
    /// leading `coveredWidth` points runs under the bar. The system title
    /// cannot be cut, so the Mac draws its own and keeps the window's name.
    /// `barWidth` is the window's width, which the Mac title has to fit in.
    @ViewBuilder
    func bookTitle(_ title: String, chapter: String, coveredWidth: CGFloat = 0, barWidth: CGFloat = 0) -> some View {
        #if os(macOS)
        navigationTitle(title)
            .toolbar(removing: .title)
            .toolbar {
                // The toolbar sizes items by their whole text, so the room
                // left by the window controls and buttons caps it here.
                let item = ToolbarItem(placement: .navigation) {
                    BarTitle(title: title, chapter: chapter, coveredWidth: coveredWidth)
                        .frame(maxWidth: max(80, barWidth - 360), alignment: .leading)
                }
                if #available(macOS 26, *) {
                    item.sharedBackgroundVisibility(.hidden)
                } else {
                    item
                }
            }
        #else
        navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    BarTitle(title: title, chapter: chapter, coveredWidth: coveredWidth)
                }
            }
        #endif
    }

    /// A sheet's fixed size on the Mac; on iOS the sheet sizes itself.
    @ViewBuilder
    func sheetSize(width: CGFloat, height: CGFloat? = nil) -> some View {
        #if os(macOS)
        frame(width: width, height: height)
        #else
        frame(maxWidth: .infinity, maxHeight: height == nil ? nil : .infinity, alignment: .top)
            .presentationDetents(height == nil ? [.medium] : [.large])
        #endif
    }

    /// The word panel's surface. On iOS it is opaque: secondary text drawn
    /// over a material there came out invisible.
    @ViewBuilder
    func wordPanelBackground() -> some View {
        #if os(macOS)
        background(.bar)
        #else
        // Under the notch side in landscape and the home indicator too.
        background { Color(uiColor: .secondarySystemBackground).ignoresSafeArea(edges: [.horizontal, .bottom]) }
        #endif
    }

    /// For IDs, addresses and foreign text: iOS must not correct or capitalise them.
    @ViewBuilder
    func literalEntry(url: Bool = false) -> some View {
        #if os(macOS)
        self
        #else
        autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .keyboardType(url ? .URL : .default)
        #endif
    }

    /// A finger-sized target on iOS for a small icon button.
    @ViewBuilder
    func touchTarget() -> some View {
        #if os(macOS)
        self
        #else
        frame(minWidth: 44, minHeight: 44).contentShape(Rectangle())
        #endif
    }

    @ViewBuilder
    func resizePointer() -> some View {
        #if os(macOS)
        pointerStyle(.rowResize)
        #else
        self
        #endif
    }

    /// The Delete key on a list selection (a Mac idiom; iOS uses swipe and menus).
    @ViewBuilder
    func onDeleteKey(perform action: @escaping () -> Void) -> some View {
        #if os(macOS)
        onDeleteCommand(perform: action)
        #else
        self
        #endif
    }

    @ViewBuilder
    func borderedList() -> some View {
        #if os(macOS)
        listStyle(.bordered)
        #else
        listStyle(.plain)
        #endif
    }
}

/// The bar sits above the content, so a panel sliding under it cannot hide it.
private struct BarTitle: View {
    let title: String
    let chapter: String
    let coveredWidth: CGFloat
    @State private var minX: CGFloat = .infinity
    #if os(macOS)
    @Environment(\.controlActiveState) private var activeState
    #endif

    var body: some View {
        VStack(alignment: alignment, spacing: 0) {
            Text(title).font(titleFont).lineLimit(1)
            if !chapter.isEmpty {
                Text(chapter).font(chapterFont).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        #if os(macOS)
        // As the system title: dimmed in a window that is not key.
        .foregroundStyle(activeState == .inactive ? .secondary : .primary)
        .padding(.horizontal, 6)
        #endif
        .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).minX } action: { minX = $0 }
        .mask {
            Rectangle()
                .padding(.leading, max(0, coveredWidth - minX))
                .animation(.easeOut(duration: 0.18), value: coveredWidth)
        }
    }

    #if os(macOS)
    private var alignment: HorizontalAlignment { .leading }
    private var titleFont: Font { .headline }
    private var chapterFont: Font { .subheadline }
    #else
    private var alignment: HorizontalAlignment { .center }
    private var titleFont: Font { .subheadline.weight(.semibold) }
    private var chapterFont: Font { .caption }
    #endif
}
