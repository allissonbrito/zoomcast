//! Backend Windows: gdigrab via ffmpeg; polling de mouse Win32. Nenhum hook de teclado.

use crate::args::{self, Progress};
use crate::coords::{self, MonitorRect, Region, Sampler};
use crate::{CaptureBackend, CaptureConfig, CaptureError, CaptureResult, CaptureTarget};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetSystemMetrics, GetWindowRect, IsIconic, SM_SWAPBUTTON,
};
use zoomcast_core::events::{EventLog, InputEvent};

#[derive(Debug, Default)]
pub struct WindowsBackend {
    active: Option<Active>,
}

#[derive(Debug)]
struct Active {
    child: Child,
    mouse_stop: Arc<AtomicBool>,
    mouse: JoinHandle<Vec<InputEvent>>,
    progress: JoinHandle<ProgressData>,
    stderr: JoinHandle<String>,
    origin: Instant,
    region: Region,
    cfg: CaptureConfig,
}

#[derive(Debug, Default)]
struct ProgressData {
    first_frame_ms: Option<u64>,
    last_frame: u64,
    out_time_us: u64,
}

// SAFETY: callback chamado sincronicamente por EnumDisplayMonitors; LPARAM aponta para Vec viva
// em `monitors`. Não armazenamos o ponteiro. GetMonitorInfoW preenche struct inicializada.
unsafe extern "system" fn monitor_cb(h: HMONITOR, _: HDC, _: *mut RECT, data: isize) -> i32 {
    let list = unsafe { &mut *(data as *mut Vec<MonitorRect>) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0);
    if unsafe { GetMonitorInfoW(h, &raw mut info) } != 0 {
        let r = info.rcMonitor;
        if let (Ok(w), Ok(h)) = (
            u32::try_from(i64::from(r.right) - i64::from(r.left)),
            u32::try_from(i64::from(r.bottom) - i64::from(r.top)),
        ) {
            list.push(MonitorRect {
                region: Region {
                    x: r.left,
                    y: r.top,
                    w,
                    h,
                },
                primary: info.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
            });
        }
    }
    1
}

fn monitors() -> Result<Vec<MonitorRect>, CaptureError> {
    let mut list = Vec::new();
    // SAFETY: callback síncrono; data aponta para `list` viva, nenhum outro acesso até retornar.
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(monitor_cb),
            (&raw mut list) as isize,
        )
    };
    if ok == 0 || list.is_empty() {
        return Err(CaptureError::InvalidTarget(
            "sem monitores acessíveis".into(),
        ));
    }
    Ok(list)
}

fn region_for(target: &CaptureTarget) -> Result<Region, CaptureError> {
    let ms = monitors()?;
    let mut it = ms.iter().map(|m| m.region);
    let first = it
        .next()
        .ok_or_else(|| CaptureError::InvalidTarget("sem monitores".into()))?;
    let (left, top, right, bottom) = it.fold(
        (
            i64::from(first.x),
            i64::from(first.y),
            i64::from(first.x) + i64::from(first.w),
            i64::from(first.y) + i64::from(first.h),
        ),
        |(l, t, r, b), m| {
            (
                l.min(i64::from(m.x)),
                t.min(i64::from(m.y)),
                r.max(i64::from(m.x) + i64::from(m.w)),
                b.max(i64::from(m.y) + i64::from(m.h)),
            )
        },
    );
    let bounds = Region {
        x: i32::try_from(left)
            .map_err(|_| CaptureError::InvalidTarget("desktop virtual grande demais".into()))?,
        y: i32::try_from(top)
            .map_err(|_| CaptureError::InvalidTarget("desktop virtual grande demais".into()))?,
        w: u32::try_from(right - left)
            .map_err(|_| CaptureError::InvalidTarget("desktop virtual grande demais".into()))?,
        h: u32::try_from(bottom - top)
            .map_err(|_| CaptureError::InvalidTarget("desktop virtual grande demais".into()))?,
    };
    let requested = match target {
        CaptureTarget::PrimaryMonitor => coords::select_monitor(&ms, None),
        CaptureTarget::Monitor(n) => coords::select_monitor(&ms, Some(*n)),
        CaptureTarget::Window(title) => {
            if title.is_empty() || title.contains('\0') {
                return Err(CaptureError::InvalidTarget(
                    "título de janela vazio ou com NUL".into(),
                ));
            }
            let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: string UTF-16 terminada em NUL, válida durante FindWindowW; título exato.
            let hwnd: HWND = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
            if hwnd.is_null() {
                return Err(CaptureError::InvalidTarget(format!(
                    "janela não encontrada: {title}"
                )));
            }
            // SAFETY: HWND retornado por FindWindowW; ponteiro RECT válido para escrita.
            if unsafe { IsIconic(hwnd) } != 0 {
                return Err(CaptureError::InvalidTarget("janela minimizada".into()));
            }
            let mut r: RECT = unsafe { std::mem::zeroed() };
            // SAFETY: HWND válido, r mutável, Win32 escreve RECT.
            if unsafe { GetWindowRect(hwnd, &raw mut r) } == 0 {
                return Err(CaptureError::InvalidTarget(
                    "falha ao ler retângulo da janela".into(),
                ));
            }
            Some(Region {
                x: r.left,
                y: r.top,
                w: u32::try_from(i64::from(r.right) - i64::from(r.left)).unwrap_or(0),
                h: u32::try_from(i64::from(r.bottom) - i64::from(r.top)).unwrap_or(0),
            })
        }
    };
    requested
        .and_then(|r| r.fit(bounds))
        .ok_or_else(|| CaptureError::InvalidTarget("alvo fora do desktop ou menor que 2x2".into()))
}

fn poll_mouse(origin: Instant, region: Region, stop: &AtomicBool) -> Vec<InputEvent> {
    let mut sampler = Sampler::new(region);
    let mut events = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let mut pt = POINT { x: 0, y: 0 };
        // SAFETY: GetCursorPos escreve POINT válido; GetAsyncKeyState consulta APENAS botões
        // físicos do mouse; SM_SWAPBUTTON mapeia os botões primário/secundário lógicos.
        let ok = unsafe { GetCursorPos(&raw mut pt) } != 0;
        if ok {
            let swap = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
            let key_down = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
            let left = key_down(if swap { VK_RBUTTON } else { VK_LBUTTON });
            let right = key_down(if swap { VK_LBUTTON } else { VK_RBUTTON });
            let middle = key_down(VK_MBUTTON);
            let t_ms = u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX);
            events.extend(sampler.step(t_ms, pt.x, pt.y, [left, right, middle]));
        }
        thread::sleep(Duration::from_millis(8)); // ponytail: polling perde cliques <8ms; hook de mouse se necessário.
    }
    events
}

fn read_progress(reader: impl Read, origin: Instant) -> ProgressData {
    let mut p = ProgressData::default();
    for line in BufReader::new(reader).lines().map_while(Result::ok) {
        match args::parse_progress_line(&line) {
            Some(Progress::Frame(f)) => {
                if f > 0 && p.first_frame_ms.is_none() {
                    p.first_frame_ms =
                        Some(u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX));
                }
                p.last_frame = f;
            }
            Some(Progress::OutTimeUs(us)) => p.out_time_us = us,
            None => {}
        }
    }
    p
}

fn read_stderr(mut reader: impl Read) -> String {
    let mut bytes = Vec::new();
    let _ = reader.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

impl CaptureBackend for WindowsBackend {
    fn start(&mut self, cfg: &CaptureConfig) -> Result<(), CaptureError> {
        if self.active.is_some() {
            return Err(CaptureError::AlreadyStarted);
        }
        // SAFETY: define contexto DPI APENAS na thread chamadora; necessário antes de enumerar
        // monitores/janelas; restaura ao sair. Se falhar (SO antigo), aborta em vez de gravar
        // coordenadas virtualizadas incompatíveis com gdigrab.
        let old =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if old.is_null() {
            return Err(CaptureError::InvalidTarget(
                "DPI per-monitor v2 indisponível".into(),
            ));
        }
        let region = region_for(&cfg.target);
        // SAFETY: `old` foi devolvido por SetThreadDpiAwarenessContext; restaura thread original.
        unsafe { SetThreadDpiAwarenessContext(old) };
        let region = region?;
        let output = cfg.out_dir.join(args::VIDEO_REL);
        let argv = args::build_ffmpeg_args(cfg.fps, region, cfg.mic.as_deref(), &output)?;
        std::fs::create_dir_all(cfg.out_dir.join("media"))?;
        let mut child = Command::new(&cfg.ffmpeg_bin)
            .args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    CaptureError::FfmpegNotFound(format!("{}: {e}", cfg.ffmpeg_bin.display()))
                } else {
                    CaptureError::Io(e)
                }
            })?;
        let origin = Instant::now();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CaptureError::Ffmpeg("stdout indisponível".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CaptureError::Ffmpeg("stderr indisponível".into()))?;
        let progress = thread::spawn(move || read_progress(stdout, origin));
        let stderr = thread::spawn(move || read_stderr(stderr));
        let mouse_stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&mouse_stop);
        let mouse = thread::spawn(move || {
            // SAFETY: contexto DPI isolado à thread de polling; Windows garante coordenadas
            // físicas em GetCursorPos para contexto per-monitor-aware v2.
            let old =
                unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            let events = if old.is_null() {
                Vec::new()
            } else {
                poll_mouse(origin, region, &stop)
            };
            if !old.is_null() {
                // SAFETY: contexto anterior válido retornado pela API, restauração nesta thread.
                unsafe { SetThreadDpiAwarenessContext(old) };
            }
            events
        });
        self.active = Some(Active {
            child,
            mouse_stop,
            mouse,
            progress,
            stderr,
            origin,
            region,
            cfg: cfg.clone(),
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<CaptureResult, CaptureError> {
        let mut active = self.active.take().ok_or(CaptureError::NotStarted)?;
        active.mouse_stop.store(true, Ordering::Relaxed);
        let stop_ms = u64::try_from(active.origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        // ffmpeg trata 'q' e finaliza muxer. Sem `kill` normal: MP4 precisa do moov atom.
        let write_result = if let Some(mut stdin) = active.child.stdin.take() {
            stdin.write_all(b"q\n").and_then(|()| stdin.flush())
        } else {
            Ok(())
        };
        let status = active.child.wait()?;
        let progress = active
            .progress
            .join()
            .map_err(|_| CaptureError::Ffmpeg("thread progress falhou".into()))?;
        let stderr = active
            .stderr
            .join()
            .map_err(|_| CaptureError::Ffmpeg("thread stderr falhou".into()))?;
        let events = active
            .mouse
            .join()
            .map_err(|_| CaptureError::Ffmpeg("thread mouse falhou".into()))?;
        if !status.success() || progress.last_frame == 0 {
            return Err(CaptureError::Ffmpeg(format!("status {status}; {stderr}")));
        }
        // BrokenPipe é esperado quando ffmpeg terminou sozinho antes do stop.
        if let Err(e) = write_result {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(CaptureError::Io(e));
            }
        }
        let frame_ms = progress
            .first_frame_ms
            .ok_or_else(|| CaptureError::Ffmpeg("primeiro frame sem timestamp".into()))?;
        let duration_ms = progress.out_time_us / 1000;
        let offset = i64::try_from(frame_ms).unwrap_or(i64::MAX);
        let mut events = EventLog {
            events: coords::apply_offset(&events, offset, stop_ms.saturating_sub(frame_ms)),
        };
        events.sort();
        Ok(CaptureResult {
            video_path: args::VIDEO_REL.into(),
            width: active.region.w,
            height: active.region.h,
            fps: f64::from(active.cfg.fps),
            duration_ms,
            has_audio: active.cfg.mic.is_some(),
            start_offset_ms: offset,
            events,
        })
    }
}

impl Drop for WindowsBackend {
    fn drop(&mut self) {
        if let Some(mut active) = self.active.take() {
            active.mouse_stop.store(true, Ordering::Relaxed);
            // Sem bloqueio no Drop: evita processo órfão. Stop explícito para finalizar MP4.
            let _ = active.child.kill();
            let _ = active.child.wait();
            let _ = active.mouse.join();
            let _ = active.progress.join();
            let _ = active.stderr.join();
        }
    }
}
