//! Núcleo puro (sem I/O de mídia): contratos de projeto, eventos, timeline e câmera.
//! Fonte da verdade do enquadramento: [`camera::camera_at`].
pub mod camera;
pub mod error;
pub mod events;
pub mod project;
pub mod timeline;
pub mod zoomgen;

pub use error::{Error, Result};

/// Tempo em milissegundos desde o início da gravação (relógio monotônico da captura).
pub type TimeMs = u64;
