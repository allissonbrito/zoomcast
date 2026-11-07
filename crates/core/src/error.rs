use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("segmento inválido: {0}")]
    InvalidSegment(String),
    #[error("segmento sobrepõe outro existente (id {0})")]
    Overlap(u32),
    #[error("versão de projeto não suportada: {found} (suportada: {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("caminho inseguro no manifesto: {0}")]
    UnsafePath(String),
    #[error("trim inválido: {0}")]
    InvalidTrim(String),
    #[error("e/s: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
