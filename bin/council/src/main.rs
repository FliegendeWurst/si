use color_eyre::Result;
use telemetry_application::prelude::*;
use tokio::sync::watch;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

mod args;

const RT_DEFAULT_THREAD_STACK_SIZE: usize = 2 * 1024 * 1024 * 3;

fn main() -> Result<()> {
    let thread_builder = ::std::thread::Builder::new().stack_size(RT_DEFAULT_THREAD_STACK_SIZE);
    let thread_handler = thread_builder.spawn(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_stack_size(RT_DEFAULT_THREAD_STACK_SIZE)
            .thread_name("bin/council-tokio::runtime")
            .enable_all()
            .build()?
            .block_on(async_main())
    })?;
    thread_handler.join().unwrap()
}

async fn async_main() -> Result<()> {
    let shutdown_token = CancellationToken::new();
    let task_tracker = TaskTracker::new();

    color_eyre::install()?;
    let config = TelemetryConfig::builder()
        .service_name("council")
        .service_namespace("si")
        .log_env_var_prefix("SI")
        .app_modules(vec!["council", "council_server"])
        .build()?;
    let mut telemetry = telemetry_application::init(config, &task_tracker, shutdown_token.clone())?;
    let args = args::parse();

    let (_shutdown_request_tx, shutdown_request_rx) = watch::channel(());

    if args.verbose > 0 {
        telemetry.set_verbosity(args.verbose.into()).await?;
    }
    debug!(arguments =?args, "parsed cli arguments");

    task_tracker.close();

    let config = council_server::server::Config::try_from(args)?;
    let server = council_server::Server::new_with_config(config).await?;
    let (subscriber_started_tx, _subscriber_started_rx) = watch::channel(());
    server
        .run(subscriber_started_tx, shutdown_request_rx.clone())
        .await?;

    // TODO(fnichol): this will eventually go into the signal handler code but at the moment in
    // council's case, this is embedded in server library code which is incorrect. At this moment
    // in the program however, the main council server has shut down so it's an appropriate time to
    // cancel other remaining tasks and wait on their graceful shutdowns
    {
        shutdown_token.cancel();
        task_tracker.wait().await;
    }

    info!("graceful shutdown complete.");
    Ok(())
}
