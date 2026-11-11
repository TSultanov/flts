//! Zero-cost stand-in for the roster trace emitter when the `tla_trace`
//! feature is off.

pub mod trace {
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
}
