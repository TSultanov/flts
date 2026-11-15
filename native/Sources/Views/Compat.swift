import SwiftUI

/// Modifiers that exist on one platform only, under one name for both.
extension View {
    /// The chapter under the book's title: a window subtitle on the Mac, a
    /// two-line title on iOS.
    @ViewBuilder
    func bookTitle(_ title: String, chapter: String) -> some View {
        #if os(macOS)
        navigationTitle(title).navigationSubtitle(chapter)
        #else
        navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    VStack(spacing: 0) {
                        Text(title).font(.subheadline.weight(.semibold)).lineLimit(1)
                        if !chapter.isEmpty {
                            Text(chapter).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
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
