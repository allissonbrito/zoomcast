# Zoomcast — arquitetura (v0)

Gravador de tela com zoom automático por clique. Tauri + React + Rust (Windows primeiro, macOS depois).
Referência de comportamento: Screenize (Apache-2.0, análise estática). **Nenhum código Swift é copiado.**

## Crates
- `core`  : contratos puros (sem I/O de mídia). Projeto, eventos, timeline, câmera, geração de zoom. **Fonte da verdade.**
- `export`: renderiza MP4 aplicando `core::camera::camera_at_f` por frame. Usa ffmpeg (processo externo).
- `capture`: trait `CaptureBackend` + backends (`synthetic` p/ testes Linux; `windows` real).
- `cli`   : `zoomcast record|zoom|export|probe` — executável de homologação.
- (futuro) `app/` Tauri + React.

## Contratos
- Pacote: `<nome>.zoomcast/{manifest.json, events.json, media/screen.mp4}`; `format_version = 1`.
- Tempo: `t_ms` inteiro desde o 1º frame do vídeo (mesmo relógio para vídeo, áudio e eventos).
- Coordenadas de evento: **pixels do vídeo gravado**, origem topo-esquerda, já sem offset de monitor/DPI.
- Câmera: `camera_at(timeline, t_ms) -> {scale, center_x, center_y}`. Exportador e preview DEVEM usar esta função.
- Saída: janela de trim `[s,e)`; frame k da saída corresponde ao tempo `s + k*1000/fps`.

## Regras de qualidade
- `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` limpos.
- `unsafe` só em `capture/windows`, com `// SAFETY:` por bloco.
- Sem `unwrap`/`expect` fora de testes. Erros tipados (`thiserror`).
- Sem rede, sem telemetria. Teclado global NÃO é gravado (só mouse) no MVP.
- Dependência nova exige justificativa no PR/commit.

## Fora do MVP
GIF, áudio do sistema, teclado, UI states, leitura de `.screenize`.
