# Sincronização e Coordenadas de Captura (`zoomcast-capture`)

Este documento descreve a sincronização temporal entre vídeo/áudio e eventos de mouse,
o sistema de coordenadas e as garantias de DPI implementadas no `WindowsBackend`.

## 1. Relógio de Referência e Sincronização

A gravação possui três fontes assíncronas:
1. Stream de vídeo (`gdigrab`)
2. Stream de áudio (`dshow`, opcional)
3. Polling de mouse (`GetCursorPos` + `GetAsyncKeyState`)

### Origem do Tempo (`t0`)
- No `start()`, um marco temporal `Instant::now()` (`origin`) é estabelecido logo após o spawn do ffmpeg.
- O polling de mouse anota timestamps `t_ms = origin.elapsed().as_millis()`.
- O ffmpeg é invocado com `-progress pipe:1` e `-stats_period 0.05`.
- As entradas (`gdigrab` e `dshow`) usam `-use_wallclock_as_timestamps 1`.
- A saída usa `-fps_mode cfr` com `libx264` (`preset ultrafast`, `tune zerolatency`). O áudio usa `aresample=async=1:first_pts=0`.

### Cálculo do Offset (`start_offset_ms`)
- O leitor da stdout de progresso intercepta o primeiro frame onde `frame > 0`. O instante decorrido (`origin.elapsed().as_millis()`) define `frame_ms`.
- `start_offset_ms = frame_ms` (atraso entre a largada do processo/polling e o primeiro quadro codificado).
- Em `stop()`, a função pura `coords::apply_offset` recalcula os eventos:
  - `t_video = t_evento - start_offset_ms`.
  - Eventos anteriores ao primeiro quadro (`t < 0`): cliques são descartados; a última posição de movimento anterior a `t=0` é preservada como `Move { t_ms: 0, x, y }` (posição inicial do ponteiro).
  - Eventos posteriores à duração útil do vídeo são descartados.

## 2. Sistema de Coordenadas e DPI

### Coordenadas Virtuais da Área de Trabalho (Windows)
- Monitores múltiplos podem ter origens com coordenadas negativas (ex.: monitor à esquerda do principal em `x = -1920`).
- Sem DPI awareness explícito, chamadas GDI e User32 sofrem virtualização DPI (coordenadas escaladas), enquanto o driver `gdigrab` captura pixels físicos.
- A thread de configuração e a thread de polling de mouse ativam `SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`. O cursor obtido via `GetCursorPos` passa a reportar pixels físicos reais do espaço de coordenadas virtual do Windows.

### Mapeamento Tela → Vídeo
- Dado o retângulo da região capturada `region = { x, y, w, h }` em pixels físicos:
  - `video_x = screen_x - region.x`
  - `video_y = screen_y - region.y`
- Se o cursor estiver fora do retângulo de captura:
  - Eventos de clique são descartados.
  - Eventos de movimento têm as coordenadas clampadas para as bordas `[0, w - 1]` e `[0, h - 1]` (`clamp_to_video`), mantendo o rastreamento consistente na margem.
- Largura e altura da região são forçadas para números pares (`w & !1`, `h & !1`) para compatibilidade com o formato de pixel `yuv420p` do `libx264`.

## 3. Limitações Conhecidas

1. **Taxa de Polling do Mouse (8 ms / 125 Hz)**:
   - Implementado com polling periódico via `thread::sleep(8ms)`. Cliques com duração inferior a ~8 ms podem não ser detectados. Hook global de mouse de baixo nível (`WH_MOUSE_LL`) seria a alternativa caso maior fidelidade seja exigida.
2. **Teclado**:
   - Por especificação do MVP, nenhum evento de teclado é capturado nem monitorado (sem hook global, sem keylogger).
3. **gdigrab vs DDA (Desktop Duplication API)**:
   - `gdigrab` utiliza GDI BitBlt. É compatível com qualquer GPU/CPU e permite captura por coordenadas e janelas arbitrárias, porém a taxa máxima e latência são inferiores a DirectX Desktop Duplication (DDA/DXGI).
4. **Janelas Ocultadas ou Minimizadas**:
   - Janelas minimizadas são rejeitadas na inicialização. Em caso de sobreposição de janelas durante a gravação, `gdigrab` grava o conteúdo visível na tela naquela região geométrica, não o framebuffer isolado da janela.
5. **Precisão do offset (~50 ms)**:
   - O 1º frame é detectado pelo `-progress` (`-stats_period 0.05`); `start_offset_ms` tem erro de até ~50 ms e ignora a latência do pipe. Refinar medindo o PTS real do 1º frame com ffprobe no `stop()` se necessário.
6. **Janela**: a região é fixada no `start()`; mover/redimensionar a janela durante a gravação não é seguido.
7. **Não validado em Windows real**: só `cargo check`/`clippy` cruzado (`x86_64-pc-windows-gnu`) e testes Linux das funções puras.
