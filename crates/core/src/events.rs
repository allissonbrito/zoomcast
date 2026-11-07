use crate::TimeMs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Evento de entrada. Coordenadas em **pixels do vídeo gravado** (origem topo-esquerda),
/// já descontados a origem da região capturada e o DPI. `t_ms` usa o mesmo relógio do vídeo.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    Move {
        t_ms: TimeMs,
        x: f64,
        y: f64,
    },
    Click {
        t_ms: TimeMs,
        x: f64,
        y: f64,
        button: MouseButton,
    },
}

impl InputEvent {
    #[must_use]
    pub fn t_ms(&self) -> TimeMs {
        match *self {
            Self::Move { t_ms, .. } | Self::Click { t_ms, .. } => t_ms,
        }
    }
}

/// Arquivo `events.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventLog {
    pub events: Vec<InputEvent>,
}

impl EventLog {
    /// Ordena por tempo (estável). Chamar antes de gravar/consumir.
    pub fn sort(&mut self) {
        self.events.sort_by_key(InputEvent::t_ms);
    }
}
