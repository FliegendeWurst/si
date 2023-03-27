#![recursion_limit = "256"]

use std::path::PathBuf;

use color_eyre::Result;
use sdf_server::{
    Config, IncomingStream, JobProcessorClientCloser, JobProcessorConnector, MigrationMode, Server,
};
use telemetry_application::{
    prelude::*, start_tracing_level_signal_handler_task, ApplicationTelemetryClient,
    TelemetryClient, TelemetryConfig,
};
use tokio::sync::mpsc;

mod args;

type JobProcessor = sdf_server::NatsProcessor;

const RT_DEFAULT_THREAD_STACK_SIZE: usize = 2 * 1024 * 1024 * 3;

fn main() -> Result<()> {
    let thread_builder = ::std::thread::Builder::new().stack_size(RT_DEFAULT_THREAD_STACK_SIZE);
    let thread_handler = thread_builder.spawn(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_stack_size(RT_DEFAULT_THREAD_STACK_SIZE)
            .thread_name("bin/sdf-tokio::runtime")
            .enable_all()
            .build()?
            .block_on(async_main())
    })?;
    thread_handler.join().unwrap()
}

async fn async_main() -> Result<()> {
    color_eyre::install()?;
    let config = TelemetryConfig::builder()
        .service_name("sdf")
        .service_namespace("si")
        .log_env_var_prefix("SI")
        .app_modules(vec!["sdf", "sdf_server"])
        .build()?;
    let telemetry = telemetry_application::init(config)?;
    let args = args::parse();

    run(args, telemetry).await
}

async fn run(args: args::Args, mut telemetry: ApplicationTelemetryClient) -> Result<()> {
    if args.verbose > 0 {
        telemetry.set_verbosity(args.verbose.into()).await?;
    }
    debug!(arguments =?args, "parsed cli arguments");

    Server::init()?;

    if args.disable_opentelemetry {
        telemetry.disable_opentelemetry().await?;
    }

    if let Some(path) = args.generate_jwt_secret_key {
        info!("Generating JWT secret at: {}", path.display());
        let _key = Server::generate_jwt_secret_key(path).await?;
        return Ok(());
    }

    if let (Some(secret_key_path), Some(public_key_path)) = (
        &args.generate_cyclone_secret_key_path,
        &args.generate_cyclone_public_key_path,
    ) {
        info!(
            "Generating Cyclone key pair at: (secret = {}, public = {})",
            secret_key_path.display(),
            public_key_path.display()
        );
        Server::generate_cyclone_key_pair(secret_key_path, public_key_path).await?;
        return Ok(());
    }

    let config = Config::try_from(args)?;

    let jwt_secret_key = Server::load_jwt_secret_key(config.jwt_secret_key_path()).await?;
    let encryption_key = Server::load_encryption_key(config.cyclone_encryption_key_path()).await?;

    let nats = Server::connect_to_nats(config.nats()).await?;
    let (alive_marker, mut job_processor_shutdown_rx) = mpsc::channel(1);

    let (job_client, job_processor) = JobProcessor::connect(&config, alive_marker).await?;

    let (resource_alive_marker, mut resource_job_processor_shutdown_rx) = mpsc::channel(1);
    let (_resource_job_client, resource_job_processor) =
        JobProcessor::connect(&config, resource_alive_marker).await?;
    let (status_receiver_alive_marker, mut status_receiver_job_processor_shutdown_rx) =
        mpsc::channel(1);
    let (_, status_receiver_job_processor) =
        JobProcessor::connect(&config, status_receiver_alive_marker).await?;

    let pg_pool = Server::create_pg_pool(config.pg_pool()).await?;

    let veritech = Server::create_veritech_client(nats.clone());

    let council_subject_prefix = "council".to_owned();

    let pkgs_path: PathBuf = config.pkgs_path().try_into()?;

    if let MigrationMode::Run | MigrationMode::RunAndQuit = config.migration_mode() {
        Server::migrate_database(
            &pg_pool,
            &nats,
            job_processor.clone(),
            &jwt_secret_key,
            veritech.clone(),
            &encryption_key,
            council_subject_prefix.clone(),
        )
        .await?;
        if let MigrationMode::RunAndQuit = config.migration_mode() {
            info!(
                "migration mode is {}, shutting down",
                config.migration_mode()
            );
            return Ok(());
        }
    } else {
        trace!("migration mode is skip, not running migrations");
    }

    // TODO(fnichol): re-enable, which we shouldn't need in the long run
    //if !disable_opentelemetry {
    //    telemetry.enable_opentelemetry().await?;
    //}

    start_tracing_level_signal_handler_task(&telemetry)?;

    match config.incoming_stream() {
        IncomingStream::HTTPSocket(_) => {
            let (server, initial_shutdown_broadcast_rx) = Server::http(
                config,
                pg_pool.clone(),
                nats.clone(),
                job_processor,
                veritech.clone(),
                encryption_key,
                jwt_secret_key,
                council_subject_prefix.clone(),
                &pkgs_path,
            )?;
            let second_shutdown_broadcast_rx = initial_shutdown_broadcast_rx.resubscribe();

            Server::start_resource_refresh_scheduler(
                pg_pool.clone(),
                nats.clone(),
                resource_job_processor,
                veritech.clone(),
                encryption_key,
                council_subject_prefix.clone(),
                initial_shutdown_broadcast_rx,
            )
            .await;

            Server::start_status_updater(
                pg_pool,
                nats,
                status_receiver_job_processor,
                veritech,
                encryption_key,
                council_subject_prefix.clone(),
                second_shutdown_broadcast_rx,
            )
            .await?;

            server.run().await?;
        }
        IncomingStream::UnixDomainSocket(_) => {
            let (server, initial_shutdown_broadcast_rx) = Server::uds(
                config,
                pg_pool.clone(),
                nats.clone(),
                job_processor,
                veritech.clone(),
                encryption_key,
                jwt_secret_key,
                council_subject_prefix.clone(),
                &pkgs_path,
            )
            .await?;
            let second_shutdown_broadcast_rx = initial_shutdown_broadcast_rx.resubscribe();

            Server::start_resource_refresh_scheduler(
                pg_pool.clone(),
                nats.clone(),
                resource_job_processor,
                veritech.clone(),
                encryption_key,
                council_subject_prefix.clone(),
                initial_shutdown_broadcast_rx,
            )
            .await;

            Server::start_status_updater(
                pg_pool,
                nats,
                status_receiver_job_processor,
                veritech,
                encryption_key,
                council_subject_prefix.clone(),
                second_shutdown_broadcast_rx,
            )
            .await?;

            server.run().await?;
        }
    }

    // Blocks until all JobProcessors are gone so we don't skip jobs that are still being sent to job transport
    info!("Waiting for job processors to finish pushing jobs");
    let _ = job_processor_shutdown_rx.recv().await;
    info!("Waiting for resource job processors to finish pushing jobs");
    let _ = resource_job_processor_shutdown_rx.recv().await;
    info!("Waiting for status receiver job processors to finish pushing jobs");
    let _ = status_receiver_job_processor_shutdown_rx.recv().await;

    info!("Shutting down the job processor client");
    if let Err(err) = (&job_client as &dyn JobProcessorClientCloser).close().await {
        error!("Failed to close job client: {err}");
    }

    Ok(())
}
