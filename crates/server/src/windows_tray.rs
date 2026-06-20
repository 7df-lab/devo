use std::thread;
use std::thread::JoinHandle;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use tokio::sync::oneshot;
use tray_icon::Icon;
use tray_icon::MouseButton;
use tray_icon::MouseButtonState;
use tray_icon::TrayIcon;
use tray_icon::TrayIconBuilder;
use tray_icon::TrayIconEvent;
use tray_icon::TrayIconId;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows_sys::Win32::UI::WindowsAndMessaging::MSG;
use windows_sys::Win32::UI::WindowsAndMessaging::PM_NOREMOVE;
use windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW;
use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
use windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT;

const TRAY_THREAD_NAME: &str = "devo-windows-tray";
const ICON_SIZE: u32 = 16;
const DEVO_MARK_PNG: &[u8] = include_bytes!("../../../.github/assets/devo-mark.png");

pub(crate) struct WindowsServerTray {
    shutdown_rx: oneshot::Receiver<()>,
    thread_id: u32,
    _thread: JoinHandle<()>,
}

impl WindowsServerTray {
    pub(crate) fn start() -> Result<Self> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = thread::Builder::new()
            .name(TRAY_THREAD_NAME.to_string())
            .spawn(move || run_tray_thread(shutdown_tx, ready_tx))
            .context("spawn Windows server tray thread")?;

        let thread_id = match ready_rx
            .recv()
            .context("Windows server tray thread exited before initialization")?
        {
            Ok(thread_id) => thread_id,
            Err(error) => return Err(anyhow!(error)),
        };

        Ok(Self {
            shutdown_rx,
            thread_id,
            _thread: thread,
        })
    }

    pub(crate) async fn shutdown_requested(&mut self) {
        let _ = (&mut self.shutdown_rx).await;
    }
}

impl Drop for WindowsServerTray {
    fn drop(&mut self) {
        // SAFETY: `thread_id` is captured after the tray thread creates its
        // message queue. If the thread has already exited, Windows reports
        // failure and there is nothing left to clean up.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
    }
}

fn run_tray_thread(
    shutdown_tx: oneshot::Sender<()>,
    ready_tx: std::sync::mpsc::Sender<std::result::Result<u32, String>>,
) {
    create_message_queue();

    let tray_icon = match create_tray_icon() {
        Ok(tray_icon) => tray_icon,
        Err(error) => {
            let _ = ready_tx.send(Err(error.to_string()));
            return;
        }
    };
    let tray_id = tray_icon.id().clone();

    // SAFETY: `GetCurrentThreadId` has no preconditions.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready_tx.send(Ok(thread_id)).is_err() {
        return;
    }

    run_message_loop(&tray_id, shutdown_tx);
    drop(tray_icon);
}

fn create_tray_icon() -> Result<TrayIcon> {
    let icon = Icon::from_rgba(devo_icon_rgba()?, ICON_SIZE, ICON_SIZE)
        .context("create Windows tray icon image")?;

    TrayIconBuilder::new()
        .with_tooltip("Devo server is running. Right-click to exit.")
        .with_icon(icon)
        .with_menu_on_left_click(false)
        .with_menu_on_right_click(false)
        .build()
        .context("create Windows tray icon")
}

fn create_message_queue() {
    // SAFETY: Passing a null HWND with PM_NOREMOVE is the documented way to
    // force creation of this thread's message queue before other threads post
    // shutdown messages to it.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        let _ = PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
}

fn run_message_loop(tray_id: &TrayIconId, shutdown_tx: oneshot::Sender<()>) {
    let mut shutdown_tx = Some(shutdown_tx);

    // SAFETY: This is the standard Win32 message loop for the tray thread. The
    // tray icon is created on this same thread and remains alive until the loop
    // exits.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if result <= 0 {
                break;
            }

            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);

            if right_click_exit_requested(tray_id)
                && let Some(shutdown_tx) = shutdown_tx.take()
            {
                let _ = shutdown_tx.send(());
                break;
            }
        }
    }
}

fn right_click_exit_requested(tray_id: &TrayIconId) -> bool {
    let receiver = TrayIconEvent::receiver();
    let mut exit_requested = false;

    while let Ok(event) = receiver.try_recv() {
        if matches!(
            event,
            TrayIconEvent::Click {
                id,
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                ..
            } if id == tray_id
        ) {
            exit_requested = true;
        }
    }

    exit_requested
}

fn devo_icon_rgba() -> Result<Vec<u8>> {
    let image = image::load_from_memory_with_format(DEVO_MARK_PNG, image::ImageFormat::Png)
        .context("decode embedded Devo mark")?;
    Ok(image
        .resize_exact(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3)
        .into_rgba8()
        .into_raw())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::ICON_SIZE;
    use super::devo_icon_rgba;

    #[test]
    fn devo_icon_rgba_matches_declared_dimensions() {
        assert_eq!(
            devo_icon_rgba().expect("decode Devo mark").len(),
            (ICON_SIZE * ICON_SIZE * 4) as usize
        );
    }
}
