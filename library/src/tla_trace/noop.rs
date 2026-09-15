//! Zero-cost stand-ins for the `tla_trace::*` surface when the `tla_trace`
//! feature is off.

pub mod trace {
    use std::path::Path;

    #[derive(Debug, Clone, Default)]
    pub struct TraceArg {
        pub reading: Option<String>,
        pub folder: Option<String>,
    }

    #[inline]
    pub fn emit_roster_event(
        _name: &str,
        _node: &str,
        _target: Option<&str>,
        _src: Option<&str>,
        _roster: serde_json::Value,
        _engine: &[String],
    ) -> anyhow::Result<()> {
        Ok(())
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
}
