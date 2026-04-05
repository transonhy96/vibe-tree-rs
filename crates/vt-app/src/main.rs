mod app;
mod event;
mod gpu;

use anyhow::Result;
use tracing_subscriber::EnvFilter;
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("vt=info".parse()?))
        .init();

    tracing::info!("Starting VibeTreeRS");

    let event_loop = EventLoop::<event::AppEvent>::with_user_event()
        .build()?;

    let proxy = event_loop.create_proxy();

    // Build tokio runtime on background threads
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let mut app = app::App::new(rt, proxy);
    event_loop.run_app(&mut app)?;

    Ok(())
}
