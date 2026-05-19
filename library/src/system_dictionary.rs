pub mod system_ios;
pub mod system_macos;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemDefinition {
    pub definition: String,
    pub transcription: Option<String>,
}
