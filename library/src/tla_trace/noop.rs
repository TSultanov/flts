//! Zero-cost stand-ins for the `tla_trace::*` and `tla_trace::mutex::*`
//! surfaces when the `tla_trace` feature is off.

pub mod trace {
    use std::path::Path;

    #[derive(Debug, Clone, Default)]
    pub struct TraceArg {
        pub reading: Option<String>,
        pub folder: Option<String>,
    }

    #[inline]
    pub async fn emit_book_event(
        _book_dir: &Path,
        _name: &str,
        _arg: Option<TraceArg>,
        _book_save_stage: &'static str,
        _translation_save_stage: &'static str,
        _state_op_kind: &'static str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    #[inline]
    pub async fn emit_translation_event(
        _book_dir: &Path,
        _translation_path: &Path,
        _name: &str,
        _arg: Option<TraceArg>,
        _book_save_stage: &'static str,
        _translation_save_stage: &'static str,
        _state_op_kind: &'static str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    #[inline]
    pub async fn emit_dictionary_event(
        _library_root: &Path,
        _dictionary_path: &Path,
        _name: &str,
        _arg: Option<TraceArg>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

pub mod mutex {
    use std::ops::{Deref, DerefMut};

    pub trait TracedLock {
        fn lock_name(&self) -> String;
    }

    pub struct TracedMutex<T> {
        inner: tokio::sync::Mutex<T>,
    }

    impl<T: TracedLock> TracedMutex<T> {
        #[inline]
        pub fn new(value: T) -> Self {
            Self {
                inner: tokio::sync::Mutex::new(value),
            }
        }
    }

    impl<T> TracedMutex<T> {
        #[inline]
        pub async fn lock(&self) -> TracedMutexGuard<'_, T> {
            TracedMutexGuard {
                inner: self.inner.lock().await,
            }
        }
    }

    impl<T: std::fmt::Debug> std::fmt::Debug for TracedMutex<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TracedMutex").finish()
        }
    }

    pub struct TracedMutexGuard<'a, T> {
        inner: tokio::sync::MutexGuard<'a, T>,
    }

    impl<T> Deref for TracedMutexGuard<'_, T> {
        type Target = T;
        #[inline]
        fn deref(&self) -> &T {
            &self.inner
        }
    }

    impl<T> DerefMut for TracedMutexGuard<'_, T> {
        #[inline]
        fn deref_mut(&mut self) -> &mut T {
            &mut self.inner
        }
    }
}
