#[cfg(target_os = "ios")]
pub fn show_dictionary(word: &str) {
    use objc2_foundation::{MainThreadMarker, NSString};
    use objc2_ui_kit::{
        UIApplication, UIReferenceLibraryViewController, UIViewController, UIWindow,
    };

    // Ensure we are on the main thread
    if let Some(mtm) = MainThreadMarker::new() {
        let term = NSString::from_str(word);
        let ref_vc = unsafe {
            UIReferenceLibraryViewController::initWithTerm(
                UIReferenceLibraryViewController::alloc(),
                &term,
            )
        };

        let app = unsafe { UIApplication::sharedApplication(mtm) };
        // windows returns Retained<NSArray<UIWindow>>
        let windows = app.windows();

        let mut root_vc = None;

        // Iterate manually or use fast enumeration if available
        for i in 0..windows.count() {
            let window = unsafe { windows.objectAtIndex(i) };
            if window.isKeyWindow() {
                root_vc = window.rootViewController();
                if root_vc.is_some() {
                    break;
                }
            }
        }

        if let Some(vc) = root_vc {
            let mut top_vc = vc;
            while let Some(presented) = top_vc.presentedViewController() {
                top_vc = presented;
            }

            unsafe {
                top_vc.presentViewController_animated_completion(&ref_vc, true, None);
            }
        }
    } else {
        log::error!("show_dictionary must be called on the main thread (iOS).");
    }
}

#[cfg(not(target_os = "ios"))]
pub fn show_dictionary(_word: &str) {}
