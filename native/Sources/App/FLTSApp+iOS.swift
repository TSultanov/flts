#if os(iOS)
import SwiftUI

@main
struct FLTSApp: App {
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var phase
    @AppStorage(Appearance.key, store: AppDefaults.store) private var appearance = Appearance.system

    var body: some Scene {
        WindowGroup {
            // Titles and toolbars need a navigation container on iOS.
            NavigationStack {
                RootView()
            }
            .environment(model)
            .preferredColorScheme(appearance.colorScheme)
            .transaction { if AppDefaults.animationsOff { $0.disablesAnimations = true } }
            .task {
                // Unit tests run inside the app; they must not open a real library.
                guard ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] == nil else { return }
                model.start()
            }
            .onChange(of: phase) { _, phase in
                guard phase == .active else { return }
                // Back from the background: sync catches up, stale progress is dropped.
                model.sync.wake()
                Task { await model.activity.reconcile() }
            }
        }
    }
}
#endif
