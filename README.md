# Zoomcast

Gravador de tela para Windows com zoom automático seguindo o clique. Exporta MP4. Beta.

## Baixar
Releases: https://github.com/allissonbrito/zoomcast/releases — baixe o `.zip`, extraia tudo, rode `zoomcast-gui.exe`.
Confira o SHA-256 com o arquivo `.sha256`. O binário ainda não é assinado: o SmartScreen pode avisar.

## Compilar
`cargo build --release -p zoomcast-gui` (Rust stable). Precisa de `ffmpeg`/`ffprobe` ao lado do exe, em `PATH` ou em `ZOOMCAST_FFMPEG`.

## Privacidade
Sem rede, sem telemetria, sem hook de teclado. Só posição/clique do mouse durante a gravação.

## Licença
Apache-2.0. O pacote Beta inclui FFmpeg (GPL) de https://github.com/BtbN/FFmpeg-Builds.
