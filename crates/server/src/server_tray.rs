use anyhow::Context;
use anyhow::Result;
use tray_icon::Icon;

const ICON_SIZE: u32 = 16;
const ICON_PADDING: u32 = 1;
const TRANSPARENT_BACKGROUND_THRESHOLD: u8 = 32;
const DEVO_MARK_PNG: &[u8] = include_bytes!("../../../.github/assets/devo-mark.png");

#[cfg(windows)]
pub(crate) struct ServerTray {
    inner: platform::PlatformServerTray,
}

#[cfg(windows)]
impl ServerTray {
    pub(crate) fn start() -> Result<Self> {
        Ok(Self {
            inner: platform::PlatformServerTray::start()?,
        })
    }

    pub(crate) async fn shutdown_requested(&mut self) {
        self.inner.shutdown_requested().await;
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn run_server_process_with_macos_tray(args: crate::ServerProcessArgs) -> Result<()> {
    platform::run_server_process_with_macos_tray(args)
}

fn create_icon() -> Result<Icon> {
    Icon::from_rgba(devo_icon_rgba()?, ICON_SIZE, ICON_SIZE).context("create Devo tray icon image")
}

fn devo_icon_rgba() -> Result<Vec<u8>> {
    let image = image::load_from_memory_with_format(DEVO_MARK_PNG, image::ImageFormat::Png)
        .context("decode embedded Devo mark")?
        .into_rgba8();
    let transparent = remove_dark_background(image);
    let bbox =
        visible_bounds(&transparent).unwrap_or((0, 0, transparent.width(), transparent.height()));
    let cropped = image::imageops::crop_imm(
        &transparent,
        bbox.0,
        bbox.1,
        bbox.2 - bbox.0,
        bbox.3 - bbox.1,
    )
    .to_image();
    let icon_content_size = ICON_SIZE - ICON_PADDING * 2;
    let resized = image::imageops::resize(
        &cropped,
        icon_content_size,
        icon_content_size,
        image::imageops::FilterType::Lanczos3,
    );
    let mut icon = image::RgbaImage::new(ICON_SIZE, ICON_SIZE);
    image::imageops::overlay(
        &mut icon,
        &resized,
        i64::from(ICON_PADDING),
        i64::from(ICON_PADDING),
    );
    for pixel in icon.pixels_mut() {
        if pixel[3] > 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
        }
    }
    Ok(icon.into_raw())
}

fn remove_dark_background(mut image: image::RgbaImage) -> image::RgbaImage {
    for pixel in image.pixels_mut() {
        if pixel[3] > 0
            && pixel[0] <= TRANSPARENT_BACKGROUND_THRESHOLD
            && pixel[1] <= TRANSPARENT_BACKGROUND_THRESHOLD
            && pixel[2] <= TRANSPARENT_BACKGROUND_THRESHOLD
        {
            *pixel = image::Rgba([0, 0, 0, 0]);
        }
    }
    image
}

fn visible_bounds(image: &image::RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = image.width();
    let mut min_y = image.height();
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found_visible_pixel = false;

    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] == 0 {
            continue;
        }
        found_visible_pixel = true;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + 1);
        max_y = max_y.max(y + 1);
    }

    found_visible_pixel.then_some((min_x, min_y, max_x, max_y))
}

#[cfg(windows)]
mod platform {
    use std::thread;
    use std::thread::JoinHandle;

    use anyhow::Context;
    use anyhow::Result;
    use anyhow::anyhow;
    use tokio::sync::oneshot;
    use tray_icon::TrayIcon;
    use tray_icon::TrayIconBuilder;
    use tray_icon::menu::Menu;
    use tray_icon::menu::MenuEvent;
    use tray_icon::menu::MenuId;
    use tray_icon::menu::MenuItem;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW;
    use windows_sys::Win32::UI::WindowsAndMessaging::MSG;
    use windows_sys::Win32::UI::WindowsAndMessaging::PM_NOREMOVE;
    use windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW;
    use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    use windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage;
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT;

    const TRAY_THREAD_NAME: &str = "devo-windows-tray";

    struct TrayResources {
        _tray_icon: TrayIcon,
        exit_item_id: MenuId,
    }

    pub(crate) struct PlatformServerTray {
        shutdown_rx: oneshot::Receiver<()>,
        thread_id: u32,
        _thread: JoinHandle<()>,
    }

    impl PlatformServerTray {
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

    impl Drop for PlatformServerTray {
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

        let mut tray_resources = match create_tray_resources() {
            Ok(tray_resources) => tray_resources,
            Err(error) => {
                let _ = ready_tx.send(Err(error.to_string()));
                return;
            }
        };

        // SAFETY: `GetCurrentThreadId` has no preconditions.
        let thread_id = unsafe { GetCurrentThreadId() };
        if ready_tx.send(Ok(thread_id)).is_err() {
            return;
        }

        run_message_loop(&mut tray_resources, shutdown_tx);
        drop(tray_resources);
    }

    fn create_tray_resources() -> Result<TrayResources> {
        let icon = super::create_icon()?;
        let tray_menu = Menu::new();
        let exit_item = MenuItem::new("Exit", /*enabled*/ true, /*accelerator*/ None);
        let exit_item_id = exit_item.id().clone();

        tray_menu
            .append(&exit_item)
            .context("add Windows tray exit menu item")?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("devo")
            .with_icon(icon)
            .with_menu_on_left_click(/*enable*/ false)
            .with_menu_on_right_click(/*enable*/ true)
            .build()
            .context("create Windows tray icon")?;

        Ok(TrayResources {
            _tray_icon: tray_icon,
            exit_item_id,
        })
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

    fn run_message_loop(tray_resources: &mut TrayResources, shutdown_tx: oneshot::Sender<()>) {
        let mut shutdown_tx = Some(shutdown_tx);
        let mut exit_item_id = tray_resources.exit_item_id.clone();
        let taskbar_created_message = register_taskbar_created_message();

        // SAFETY: This is the standard Win32 message loop for the tray thread.
        // The tray icon is created on this same thread and remains alive until
        // the loop exits.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            loop {
                let result = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
                if result <= 0 {
                    break;
                }

                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);

                if taskbar_created_message != 0 && msg.message == taskbar_created_message {
                    match create_tray_resources() {
                        Ok(recreated_resources) => {
                            *tray_resources = recreated_resources;
                            exit_item_id = tray_resources.exit_item_id.clone();
                            tracing::info!("recreated Windows tray icon after taskbar restart");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "failed to recreate Windows tray icon");
                        }
                    }
                    continue;
                }

                if exit_menu_item_selected(&exit_item_id)
                    && let Some(shutdown_tx) = shutdown_tx.take()
                {
                    let _ = shutdown_tx.send(());
                    break;
                }
            }
        }
    }

    fn register_taskbar_created_message() -> u32 {
        let message_name = "TaskbarCreated"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: `message_name` is a null-terminated UTF-16 string and remains
        // alive for the duration of the call.
        unsafe { RegisterWindowMessageW(message_name.as_ptr()) }
    }

    fn exit_menu_item_selected(exit_item_id: &MenuId) -> bool {
        let receiver = MenuEvent::receiver();
        let mut exit_requested = false;

        while let Ok(event) = receiver.try_recv() {
            if event.id() == exit_item_id {
                exit_requested = true;
            }
        }

        exit_requested
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::mpsc::Receiver;
    use std::sync::mpsc::Sender;
    use std::thread;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use anyhow::Context;
    use anyhow::Result;
    use anyhow::anyhow;
    use objc2_foundation::NSString;
    use tokio_util::sync::CancellationToken;
    use tray_icon::TrayIcon;
    use tray_icon::TrayIconBuilder;
    use tray_icon::menu::Menu;
    use tray_icon::menu::MenuEvent;
    use tray_icon::menu::MenuId;
    use tray_icon::menu::MenuItem;
    use winit::application::ApplicationHandler;
    use winit::event::StartCause;
    use winit::event::WindowEvent;
    use winit::event_loop::ActiveEventLoop;
    use winit::event_loop::ControlFlow;
    use winit::event_loop::EventLoop;
    use winit::event_loop::EventLoopProxy;
    use winit::platform::macos::ActivationPolicy;
    use winit::platform::macos::EventLoopBuilderExtMacOS;
    use winit::window::WindowId;

    use crate::bootstrap::ServerProcessArgs;
    use crate::bootstrap::ServerProcessRunOptions;
    use crate::bootstrap::ServerTrayStartup;
    use crate::bootstrap::run_server_process_inner;

    const REAL_SERVER_STARTED_BRIDGE_THREAD_NAME: &str = "devo-macos-tray-startup-bridge";
    const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);
    const SERVER_THREAD_NAME: &str = "devo-macos-server-runtime";
    const STATUS_ITEM_AUTOSAVE_NAME: &str = "com.devo.server.status-item";
    const STATUS_ITEM_LENGTH: f64 = 24.0;
    const TOKIO_WORKER_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

    enum MacosTrayEvent {
        RealServerStarted,
        ServerDone(Result<()>),
        MenuEvent(MenuEvent),
    }

    struct TrayResources {
        _tray_icon: TrayIcon,
        exit_item_id: MenuId,
    }

    struct MacosTrayApp {
        shutdown_token: CancellationToken,
        tray_resources: Option<TrayResources>,
        event_loop_started: bool,
        real_server_started: bool,
        shutdown_requested: bool,
        server_result: Option<Result<()>>,
    }

    impl MacosTrayApp {
        fn new(shutdown_token: CancellationToken) -> Self {
            Self {
                shutdown_token,
                tray_resources: None,
                event_loop_started: false,
                real_server_started: false,
                shutdown_requested: false,
                server_result: None,
            }
        }

        fn maybe_start_tray(&mut self) {
            if self.event_loop_started && self.real_server_started {
                start_tray_resources_once(&mut self.tray_resources);
            }
        }
    }

    pub(crate) fn run_server_process_with_macos_tray(args: ServerProcessArgs) -> Result<()> {
        let mut event_loop_builder = EventLoop::<MacosTrayEvent>::with_user_event();
        event_loop_builder.with_activation_policy(ActivationPolicy::Accessory);
        event_loop_builder.with_default_menu(/*enable*/ false);
        event_loop_builder.with_activate_ignoring_other_apps(/*ignore*/ false);
        let event_loop = event_loop_builder
            .build()
            .context("create macOS server tray event loop")?;

        let event_proxy = event_loop.create_proxy();
        install_menu_event_handler(&event_proxy);
        let shutdown_token = CancellationToken::new();
        let (real_server_started_tx, real_server_started_rx) = std::sync::mpsc::channel();
        let real_server_started_bridge =
            spawn_real_server_started_bridge(real_server_started_rx, event_proxy.clone())?;
        let server_thread = spawn_server_thread(
            args,
            shutdown_token.clone(),
            real_server_started_tx,
            event_proxy,
        )?;

        let result = run_winit_event_loop(event_loop, shutdown_token);

        if server_thread.join().is_err() {
            return Err(anyhow!("macOS server runtime thread panicked"));
        }
        if real_server_started_bridge.join().is_err() {
            return Err(anyhow!("macOS server startup bridge thread panicked"));
        }
        result
    }

    fn install_menu_event_handler(event_proxy: &EventLoopProxy<MacosTrayEvent>) {
        let event_proxy = event_proxy.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = event_proxy.send_event(MacosTrayEvent::MenuEvent(event));
        }));
    }

    fn spawn_real_server_started_bridge(
        real_server_started_rx: Receiver<()>,
        event_proxy: EventLoopProxy<MacosTrayEvent>,
    ) -> Result<JoinHandle<()>> {
        thread::Builder::new()
            .name(REAL_SERVER_STARTED_BRIDGE_THREAD_NAME.to_string())
            .spawn(move || {
                if real_server_started_rx.recv().is_ok() {
                    let _ = event_proxy.send_event(MacosTrayEvent::RealServerStarted);
                }
            })
            .context("spawn macOS tray startup bridge thread")
    }

    fn spawn_server_thread(
        args: ServerProcessArgs,
        external_shutdown: CancellationToken,
        real_server_started_tx: Sender<()>,
        event_proxy: EventLoopProxy<MacosTrayEvent>,
    ) -> Result<JoinHandle<()>> {
        thread::Builder::new()
            .name(SERVER_THREAD_NAME.to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    build_server_runtime().and_then(|runtime| {
                        let result = runtime.block_on(run_server_process_inner(
                            args,
                            ServerProcessRunOptions {
                                external_shutdown: Some(external_shutdown),
                                tray_startup: ServerTrayStartup::Disabled,
                                real_server_started: Some(real_server_started_tx),
                            },
                        ));
                        runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
                        result
                    })
                }))
                .unwrap_or_else(|_| Err(anyhow!("macOS server runtime thread panicked")));
                let _ = event_proxy.send_event(MacosTrayEvent::ServerDone(result));
            })
            .context("spawn macOS server runtime thread")
    }

    fn build_server_runtime() -> Result<tokio::runtime::Runtime> {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();
        builder.thread_stack_size(TOKIO_WORKER_STACK_SIZE_BYTES);
        Ok(builder.build()?)
    }

    fn run_winit_event_loop(
        event_loop: EventLoop<MacosTrayEvent>,
        shutdown_token: CancellationToken,
    ) -> Result<()> {
        let mut app = MacosTrayApp::new(shutdown_token);
        event_loop
            .run_app(&mut app)
            .context("run macOS server tray event loop")?;
        app.server_result.unwrap_or_else(|| {
            Err(anyhow!(
                "macOS server runtime exited without reporting result"
            ))
        })
    }

    impl ApplicationHandler<MacosTrayEvent> for MacosTrayApp {
        fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
            event_loop.set_control_flow(ControlFlow::Wait);
            if cause == StartCause::Init {
                self.event_loop_started = true;
                tracing::debug!("started macOS server tray event loop");
                self.maybe_start_tray();
            }
        }

        fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: MacosTrayEvent) {
            match event {
                MacosTrayEvent::RealServerStarted => {
                    self.real_server_started = true;
                    self.maybe_start_tray();
                }
                MacosTrayEvent::MenuEvent(event) => {
                    if !self.shutdown_requested
                        && self
                            .tray_resources
                            .as_ref()
                            .is_some_and(|resources| event.id() == &resources.exit_item_id)
                    {
                        self.shutdown_requested = true;
                        self.shutdown_token.cancel();
                    }
                }
                MacosTrayEvent::ServerDone(result) => {
                    self.server_result = Some(result);
                    event_loop.exit();
                }
            }
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            _event: WindowEvent,
        ) {
        }

        fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
            drop(self.tray_resources.take());
        }
    }

    fn start_tray_resources_once(tray_resources: &mut Option<TrayResources>) {
        if tray_resources.is_some() {
            return;
        }

        match create_tray_resources() {
            Ok(resources) => {
                *tray_resources = Some(resources);
                tracing::info!("started macOS server tray icon");
            }
            Err(error) => {
                tracing::warn!(%error, "failed to start macOS server tray icon");
            }
        }
    }

    fn create_tray_resources() -> Result<TrayResources> {
        let icon = super::create_icon()?;
        let tray_menu = Menu::new();
        let exit_item = MenuItem::new("Exit", /*enabled*/ true, /*accelerator*/ None);
        let exit_item_id = exit_item.id().clone();

        tray_menu
            .append(&exit_item)
            .context("add macOS tray exit menu item")?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("devo")
            .with_icon(icon)
            .with_icon_as_template(/*is_template*/ true)
            .with_menu_on_left_click(/*enable*/ true)
            .with_menu_on_right_click(/*enable*/ true)
            .build()
            .context("create macOS tray icon")?;
        if let Some(ns_status_item) = tray_icon.ns_status_item() {
            let autosave_name = NSString::from_str(STATUS_ITEM_AUTOSAVE_NAME);
            ns_status_item.setAutosaveName(Some(&autosave_name));
            ns_status_item.setLength(STATUS_ITEM_LENGTH);
        }

        Ok(TrayResources {
            _tray_icon: tray_icon,
            exit_item_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    #[test]
    fn tray_icon_rgba_keeps_transparent_corners() {
        let rgba = super::devo_icon_rgba().expect("decode Devo tray icon");
        let first_pixel_alpha = rgba[3];
        let top_center_alpha = rgba[(super::ICON_SIZE as usize / 2 * 4) + 3];
        let center_alpha_index = ((super::ICON_SIZE as usize / 2 * super::ICON_SIZE as usize)
            + super::ICON_SIZE as usize / 2)
            * 4
            + 3;
        let center_alpha = rgba[center_alpha_index];
        let last_pixel_alpha = rgba[rgba.len() - 1];

        assert_eq!(
            (
                rgba.len(),
                first_pixel_alpha,
                top_center_alpha,
                center_alpha > 240,
                last_pixel_alpha,
            ),
            (
                (super::ICON_SIZE * super::ICON_SIZE * 4) as usize,
                0,
                0,
                true,
                0,
            )
        );
    }

    #[test]
    fn tray_icon_rgba_uses_template_mask_pixels() {
        let rgba = super::devo_icon_rgba().expect("decode Devo tray icon");
        let visible_pixels = rgba
            .chunks_exact(4)
            .filter(|pixel| pixel[3] > 0)
            .collect::<Vec<_>>();
        let visible_rgb = visible_pixels
            .iter()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect::<Vec<_>>();

        assert!(!visible_pixels.is_empty());
        assert_eq!(visible_rgb, vec![[0, 0, 0]; visible_pixels.len()]);
    }

    #[test]
    fn desktop_macos_template_icons_use_box_with_cutout_mark() {
        for (bytes, size) in [
            (
                include_bytes!("../../../apps/desktop/resources/iconTemplate.png").as_slice(),
                16,
            ),
            (
                include_bytes!("../../../apps/desktop/resources/iconTemplate@2x.png").as_slice(),
                32,
            ),
        ] {
            let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                .expect("decode desktop macOS tray icon")
                .into_rgba8();

            let inset = size / 16;
            let center = image.get_pixel(size / 2, size / 2);

            assert_eq!([image.width(), image.height()], [size, size]);
            assert_eq!(image.get_pixel(0, 0)[3], 0);
            assert!(image.get_pixel(size / 2, inset)[3] > 200);
            assert_eq!(center[3], 0);
        }
    }
}
