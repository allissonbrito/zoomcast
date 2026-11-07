//! Parser da saída `ffmpeg -list_devices true -f dshow -i dummy` (stderr). Puro.
//!
//! Dois formatos:
//! - ffmpeg ≥ 5: `[dshow @ 0x..] "Nome" (audio)`
//! - ffmpeg antigo: seção `DirectShow audio devices` seguida de `[dshow @ 0x..]  "Nome"`
//!
//! Linhas `Alternative name "@device_..."` são ignoradas.

#[must_use]
pub fn parse_audio_devices(stderr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut audio_section = false;
    for line in stderr.lines() {
        // Tira o prefixo `[dshow @ 0x...]`.
        let body = line
            .split_once(']')
            .filter(|_| line.trim_start().starts_with('['))
            .map_or(line, |(_, r)| r)
            .trim();
        if body.is_empty() || body.starts_with("Alternative name") {
            continue;
        }
        let lower = body.to_ascii_lowercase();
        if lower.contains("directshow audio devices") {
            audio_section = true;
            continue;
        }
        if lower.contains("directshow video devices") {
            audio_section = false;
            continue;
        }
        let (name_part, kind) = ["(audio)", "(video)", "(none)"]
            .iter()
            .find_map(|k| body.strip_suffix(k).map(|r| (r.trim_end(), Some(*k))))
            .unwrap_or((body, None));
        let Some(name) = name_part
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
        else {
            continue;
        };
        let wanted = match kind {
            Some(k) => k == "(audio)",
            None => audio_section,
        };
        if wanted && !name.is_empty() && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [&str; 0] = [];

    #[test]
    fn novo_formato() {
        let s = r#"[dshow @ 000001] DirectShow video devices (some may be both video and audio devices)
[dshow @ 000001]  "Integrated Camera" (video)
[dshow @ 000001]     Alternative name "@device_pnp_\\?\usb#vid"
[dshow @ 000001] DirectShow audio devices
[dshow @ 000001]  "Microfone (Realtek(R) Audio)" (audio)
[dshow @ 000001]     Alternative name "@device_cm_{33D9}\wave_{ABC}"
[dshow @ 000001]  "Stereo Mix (Realtek)" (audio)
dummy: Immediate exit requested"#;
        assert_eq!(
            parse_audio_devices(s),
            ["Microfone (Realtek(R) Audio)", "Stereo Mix (Realtek)"]
        );
    }

    #[test]
    fn formato_antigo() {
        let s = "[dshow @ 0x1] DirectShow video devices\n[dshow @ 0x1]  \"Cam\"\n[dshow @ 0x1]     Alternative name \"@device_x\"\n[dshow @ 0x1] DirectShow audio devices\n[dshow @ 0x1]  \"Mic USB\"\n[dshow @ 0x1]  \"Mic USB\"\n";
        assert_eq!(parse_audio_devices(s), ["Mic USB"]);
    }

    #[test]
    fn vazio_ou_lixo() {
        assert_eq!(parse_audio_devices(""), NONE);
        assert_eq!(
            parse_audio_devices("Error: ffmpeg not found\n[dshow @ 1] Could not enumerate"),
            NONE
        );
        // só vídeo
        assert_eq!(parse_audio_devices("[dshow @ 1]  \"Cam\" (video)"), NONE);
    }
}
