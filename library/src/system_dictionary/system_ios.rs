#[cfg(target_os = "ios")]
pub fn show_dictionary(word: &str) {
    use std::ffi::c_void;

    // Defer presentation a run loop: UIReferenceLibraryViewController runs a
    // nested WebKit run loop that interferes with Tao's event loop.

    type DispatchQueueT = *const c_void;
    type DispatchBlock = extern "C" fn(*mut c_void);

    // libdispatch is part of libSystem on iOS.
    unsafe extern "C" {
        // dispatch_get_main_queue is a macro over &_dispatch_main_q.
        static _dispatch_main_q: c_void;
        fn dispatch_async_f(queue: DispatchQueueT, context: *mut c_void, work: DispatchBlock);
    }

    // Box the word so it can cross the C callback.
    let word_box = Box::new(word.to_string());
    let word_ptr = Box::into_raw(word_box) as *mut c_void;

    extern "C" fn show_dictionary_impl(context: *mut c_void) {
        // Safety: reconstructing the Box created above.
        let word = unsafe { Box::from_raw(context as *mut String) };

        use objc2::MainThreadOnly;
        use objc2_foundation::{MainThreadMarker, NSString};
        use objc2_ui_kit::{
            UIApplication, UIReferenceLibraryViewController, UIScene, UIWindowScene,
        };

        let Some(mtm) = MainThreadMarker::new() else {
            log::error!("show_dictionary_impl: Not on main thread!");
            return;
        };

        let term = NSString::from_str(&word);

        let ref_vc = UIReferenceLibraryViewController::initWithTerm(
            UIReferenceLibraryViewController::alloc(mtm),
            &term,
        );

        let app = UIApplication::sharedApplication(mtm);

        let connected_scenes = app.connectedScenes();
        let mut root_vc = None;

        for scene in connected_scenes.iter() {
            let scene_ref: &UIScene = &scene;
            if let Some(window_scene) = scene_ref.downcast_ref::<UIWindowScene>() {
                if let Some(key_window) = window_scene.keyWindow() {
                    root_vc = key_window.rootViewController();
                    if root_vc.is_some() {
                        break;
                    }
                }

                // keyWindow can be absent; iterate windows instead.
                let windows = window_scene.windows();
                for i in 0..windows.len() {
                    let window = windows.objectAtIndex(i);
                    if window.isKeyWindow() {
                        root_vc = window.rootViewController();
                        if root_vc.is_some() {
                            break;
                        }
                    }
                }
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

            top_vc.presentViewController_animated_completion(&ref_vc, true, None);
        } else {
            log::error!("No root view controller found!");
        }
    }

    // Async so we leave the current run loop iteration.
    unsafe {
        let main_queue = &_dispatch_main_q as *const c_void;
        dispatch_async_f(main_queue, word_ptr, show_dictionary_impl);
    }
}

#[cfg(not(target_os = "ios"))]
pub fn show_dictionary(_word: &str) {}
