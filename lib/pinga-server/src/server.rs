use core::fmt;
use std::{io, path::Path, sync::Arc};

use dal::{
    job::{
        consumer::{JobConsumer, JobConsumerError, JobConsumerMetadata, JobInfo},
        definition::FixesJob,
        producer::JobProducer,
    },
    DalContext, DalContextBuilder, DependentValuesUpdate, InitializationError, JobFailure,
    JobFailureError, JobInvocationId, JobQueueProcessor, NatsProcessor, ServicesContext,
    TransactionsError,
};
use futures::{FutureExt, Stream, StreamExt};
use nats_subscriber::{Request, SubscriberError, Subscription};
use si_data_nats::{NatsClient, NatsConfig, NatsError};
use si_data_pg::{PgPool, PgPoolConfig, PgPoolError};
use stream_cancel::StreamExt as StreamCancelStreamExt;
use telemetry::prelude::*;
use thiserror::Error;
use tokio::{
    signal::unix,
    sync::{mpsc, oneshot, watch},
    task,
};
use ulid::Ulid;
use veritech_client::{Client as VeritechClient, EncryptionKey, EncryptionKeyError};

use crate::{nats_jobs_subject, Config, NATS_JOBS_DEFAULT_QUEUE};

#[derive(Debug, Error)]
pub enum ServerError {
    #[error(transparent)]
    Initialization(#[from] InitializationError),
    #[error(transparent)]
    JobConsumer(#[from] JobConsumerError),
    #[error(transparent)]
    JobFailure(#[from] Box<JobFailureError>),
    #[error("error when loading encryption key: {0}")]
    EncryptionKey(#[from] EncryptionKeyError),
    #[error(transparent)]
    Nats(#[from] NatsError),
    #[error(transparent)]
    PgPool(#[from] Box<PgPoolError>),
    #[error("failed to setup signal handler")]
    Signal(#[source] io::Error),
    #[error(transparent)]
    Subscriber(#[from] SubscriberError),
    #[error(transparent)]
    Transactions(#[from] Box<TransactionsError>),
    #[error("unknown job kind {0}")]
    UnknownJobKind(String),
}

impl From<PgPoolError> for ServerError {
    fn from(e: PgPoolError) -> Self {
        Self::PgPool(Box::new(e))
    }
}

impl From<JobFailureError> for ServerError {
    fn from(e: JobFailureError) -> Self {
        Self::JobFailure(Box::new(e))
    }
}

impl From<TransactionsError> for ServerError {
    fn from(e: TransactionsError) -> Self {
        Self::Transactions(Box::new(e))
    }
}

type Result<T> = std::result::Result<T, ServerError>;

pub struct Server {
    concurrency_limit: usize,
    encryption_key: Arc<EncryptionKey>,
    nats: NatsClient,
    subject_prefix: Option<String>,
    pg_pool: PgPool,
    veritech: VeritechClient,
    job_processor: Box<dyn JobQueueProcessor + Send + Sync>,
    job_processor_alive_marker_rx: mpsc::Receiver<()>,
    /// An internal shutdown watch receiver handle which can be provided to internal tasks which
    /// want to be notified when a shutdown event is in progress.
    shutdown_watch_rx: watch::Receiver<()>,
    /// An external shutdown sender handle which can be handed out to external callers who wish to
    /// trigger a server shutdown at will.
    external_shutdown_tx: mpsc::Sender<ShutdownSource>,
    /// An internal graceful shutdown receiever handle which the server's main thread uses to stop
    /// accepting work when a shutdown event is in progress.
    graceful_shutdown_rx: oneshot::Receiver<()>,
    metadata: Arc<ServerMetadata>,
}

impl Server {
    #[instrument(name = "pinga.init.from_config", skip_all)]
    pub async fn from_config(config: Config) -> Result<Self> {
        // An mpsc channel which can be used to externally shut down the server.
        let (external_shutdown_tx, external_shutdown_rx) = mpsc::channel(4);
        // A watch channel used to notify internal parts of the server that a shutdown event is in
        // progress. The value passed along is irrelevant--we only care that the event was
        // triggered and react accordingly.
        let (shutdown_watch_tx, shutdown_watch_rx) = watch::channel(());

        dal::init()?;

        let (alive_marker, job_processor_alive_marker_rx) = mpsc::channel(1);

        let encryption_key =
            Self::load_encryption_key(config.cyclone_encryption_key_path()).await?;
        let nats = Self::connect_to_nats(config.nats()).await?;
        let pg_pool = Self::create_pg_pool(config.pg_pool()).await?;
        let veritech = Self::create_veritech_client(nats.clone());
        let job_processor = Self::create_job_processor(nats.clone(), alive_marker);

        let metadata = ServerMetadata {
            job_instance: config.instance_id().to_string(),
            job_invoked_provider: "si",
        };

        let graceful_shutdown_rx =
            prepare_graceful_shutdown(external_shutdown_rx, shutdown_watch_tx)?;

        Ok(Server {
            concurrency_limit: config.concurrency(),
            pg_pool,
            nats,
            subject_prefix: config.subject_prefix().map(|s| s.to_string()),
            veritech,
            encryption_key,
            job_processor,
            job_processor_alive_marker_rx,
            shutdown_watch_rx,
            external_shutdown_tx,
            graceful_shutdown_rx,
            metadata: Arc::new(metadata),
        })
    }

    pub async fn run(mut self) -> Result<()> {
        process_job_requests_task(
            self.metadata,
            self.concurrency_limit,
            self.pg_pool,
            self.nats,
            self.subject_prefix.as_deref(),
            self.veritech,
            self.job_processor,
            self.encryption_key,
            self.shutdown_watch_rx,
        )
        .await;

        // Blocks until all job processors are gone so we don't skip jobs that are still being sent
        info!("waiting for all job processors to finish pushing jobs");
        let _ = self.job_processor_alive_marker_rx.recv().await;

        let _ = self.graceful_shutdown_rx.await;
        info!("received and processed graceful shutdown, terminating server instance");

        Ok(())
    }

    /// Gets a [`ShutdownHandle`] that can externally or on demand trigger the server's shutdown
    /// process.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            shutdown_tx: self.external_shutdown_tx.clone(),
        }
    }

    #[instrument(name = "pinga.init.load_encryption_key", skip_all)]
    async fn load_encryption_key(path: impl AsRef<Path>) -> Result<Arc<EncryptionKey>> {
        Ok(Arc::new(EncryptionKey::load(path).await?))
    }

    #[instrument(name = "pinga.init.connect_to_nats", skip_all)]
    async fn connect_to_nats(nats_config: &NatsConfig) -> Result<NatsClient> {
        let client = NatsClient::new(nats_config).await?;
        debug!("successfully connected nats client");
        Ok(client)
    }

    #[instrument(name = "pinga.init.create_pg_pool", skip_all)]
    async fn create_pg_pool(pg_pool_config: &PgPoolConfig) -> Result<PgPool> {
        let pool = PgPool::new(pg_pool_config).await?;
        debug!("successfully started pg pool (note that not all connections may be healthy)");
        Ok(pool)
    }

    #[instrument(name = "pinga.init.create_veritech_client", skip_all)]
    fn create_veritech_client(nats: NatsClient) -> VeritechClient {
        VeritechClient::new(nats)
    }

    #[instrument(name = "pinga.init.create_job_processor", skip_all)]
    fn create_job_processor(
        nats: NatsClient,
        alive_marker: mpsc::Sender<()>,
    ) -> Box<dyn JobQueueProcessor + Send + Sync> {
        Box::new(NatsProcessor::new(nats, alive_marker)) as Box<dyn JobQueueProcessor + Send + Sync>
    }
}

#[derive(Clone, Debug)]
pub struct ServerMetadata {
    job_instance: String,
    job_invoked_provider: &'static str,
}

pub struct ShutdownHandle {
    shutdown_tx: mpsc::Sender<ShutdownSource>,
}

impl ShutdownHandle {
    pub async fn shutdown(self) {
        if let Err(err) = self.shutdown_tx.send(ShutdownSource::Handle).await {
            warn!(error = ?err, "shutdown tx returned error, receiver is likely already closed");
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ShutdownSource {
    Handle,
}

impl Default for ShutdownSource {
    fn default() -> Self {
        Self::Handle
    }
}

pub struct JobItem {
    metadata: Arc<ServerMetadata>,
    messaging_destination: Arc<String>,
    ctx_builder: DalContextBuilder,
    request: Result<Request<JobInfo>>,
}

pub struct Subscriber;

impl Subscriber {
    pub async fn jobs(
        metadata: Arc<ServerMetadata>,
        pg_pool: PgPool,
        nats: NatsClient,
        subject_prefix: Option<&str>,
        veritech: veritech_client::Client,
        job_processor: Box<dyn JobQueueProcessor + Send + Sync>,
        encryption_key: Arc<veritech_client::EncryptionKey>,
    ) -> Result<impl Stream<Item = JobItem>> {
        let subject = nats_jobs_subject(subject_prefix);
        debug!(
            messaging.destination = &subject.as_str(),
            "subscribing for job requests"
        );

        let services_context = ServicesContext::new(
            pg_pool,
            nats.clone(),
            job_processor,
            veritech.clone(),
            encryption_key,
            "council".to_owned(),
            None,
        );
        let ctx_builder = DalContext::builder(services_context);

        let messaging_destination = Arc::new(subject.clone());

        Ok(Subscription::create(subject)
            .queue_name(NATS_JOBS_DEFAULT_QUEUE)
            .start(&nats)
            .await?
            .map(move |request| JobItem {
                metadata: metadata.clone(),
                messaging_destination: messaging_destination.clone(),
                ctx_builder: ctx_builder.clone(),
                request: request.map_err(Into::into),
            }))
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_job_requests_task(
    metadata: Arc<ServerMetadata>,
    concurrency_limit: usize,
    pg_pool: PgPool,
    nats: NatsClient,
    subject_prefix: Option<&str>,
    veritech: veritech_client::Client,
    job_processor: Box<dyn JobQueueProcessor + Send + Sync>,
    encryption_key: Arc<veritech_client::EncryptionKey>,
    shutdown_watch_rx: watch::Receiver<()>,
) {
    if let Err(err) = process_job_requests(
        metadata,
        concurrency_limit,
        pg_pool,
        nats,
        subject_prefix,
        veritech,
        job_processor,
        encryption_key,
        shutdown_watch_rx,
    )
    .await
    {
        warn!(error = ?err, "processing job requests failed");
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_job_requests(
    metadata: Arc<ServerMetadata>,
    concurrency_limit: usize,
    pg_pool: PgPool,
    nats: NatsClient,
    subject_prefix: Option<&str>,
    veritech: veritech_client::Client,
    job_processor: Box<dyn JobQueueProcessor + Send + Sync>,
    encryption_key: Arc<veritech_client::EncryptionKey>,
    mut shutdown_watch_rx: watch::Receiver<()>,
) -> Result<()> {
    let requests = Subscriber::jobs(
        metadata,
        pg_pool,
        nats,
        subject_prefix,
        veritech,
        job_processor,
        encryption_key,
    )
    .await?;

    requests
        .take_until_if(shutdown_watch_rx.changed().map(|_| true))
        .for_each_concurrent(concurrency_limit, |job| async move {
            // Got the next message from the subscriber
            match job.request {
                Ok(request) => {
                    let invocation_id = JobInvocationId::new();

                    // Spawn a task and process the request
                    match task::Builder::new()
                        .name("execute-job-task")
                        .spawn(execute_job_task(
                            invocation_id,
                            job.metadata,
                            job.messaging_destination,
                            job.ctx_builder,
                            request,
                        )) {
                        // Task has spawned on the runtime and the `JoinHandle` future is provided.
                        //
                        // In order for a concurrency limit to be enforced we await the
                        // `JoinHandle`, which is how `for_each_concurrent` knows the task has
                        // completed.
                        Ok(join_handle) => {
                            if let Err(err) = join_handle.await {
                                // NOTE(fnichol): This likely happens when there is contention or
                                // an error in the Tokio runtime so we will be loud and log an
                                // error under the assumptions that 1) this event rarely
                                // happens and 2) the task code did not contribute to trigger
                                // the `JoinError`.
                                error!(
                                    error = ?err,
                                    "execute-job-task failed to execute to completion"
                                );
                            }
                        }
                        // Tokio failed to successfully span a new task on the runtime.
                        //
                        // NOTE(fnichol): While this is a catastrophic failure, there is also not
                        // much we can do and the job will *not* have been attempted as a
                        // result, which is why until we have job retry logic, we log and error
                        // and not a warn.
                        Err(err) => {
                            error!(error = ?err, "failed to spawn execute-job-task");
                        }
                    };
                }
                Err(err) => {
                    warn!(error = ?err, "next job request had an error, job will not be executed");
                }
            }
        })
        .await;

    Ok(())
}

#[instrument(
    name = "execute_job_task",
    skip_all,
    level = "info",
    fields(
        job.trigger = "pubsub",
        job.instance = metadata.job_instance,
        job.invocation_id = %id,
        job.invoked_name = request.payload.kind,
        job.invoked_provider = metadata.job_invoked_provider,
        messaging.destination = Empty,
        messaging.destination_kind = "topic",
        messaging.operation = "process",
        otel.kind = %FormattedSpanKind(SpanKind::Consumer),
        otel.name = Empty,
        otel.status_code = Empty,
        otel.status_message = Empty,
    )
)]
async fn execute_job_task(
    id: JobInvocationId,
    metadata: Arc<ServerMetadata>,
    messaging_destination: Arc<String>,
    ctx_builder: DalContextBuilder,
    request: Request<JobInfo>,
) {
    let span = Span::current();

    span.record("messaging.destination", messaging_destination.as_str());
    span.record(
        "otel.name",
        format!("{} process", &messaging_destination).as_str(),
    );

    match execute_job(id, &metadata, messaging_destination, ctx_builder, request).await {
        Ok(_) => span.record_ok(),
        Err(err) => {
            error!(
                error = ?err,
                job.invocation_id = %id,
                job.instance = &metadata.job_instance,
                "job execution failed"
            );
            span.record_err(err);
        }
    }
}

async fn execute_job(
    _id: JobInvocationId,
    _metadata: &Arc<ServerMetadata>,
    _messaging_destination: Arc<String>,
    ctx_builder: DalContextBuilder,
    request: Request<JobInfo>,
) -> Result<()> {
    let (job_info, _) = request.into_parts();
    info!(id = %job_info.id, kind = %job_info.kind, args = ?job_info.args, "\n\n\nexecuting job");
    trace!(backtrace = %job_info.backtrace, "caller backtrace");

    let job = match job_info.kind() {
        stringify!(DependentValuesUpdate) => {
            Box::new(DependentValuesUpdate::try_from(job_info.clone())?)
                as Box<dyn JobConsumer + Send + Sync>
        }
        stringify!(FixesJob) => {
            Box::new(FixesJob::try_from(job_info.clone())?) as Box<dyn JobConsumer + Send + Sync>
        }
        kind => return Err(ServerError::UnknownJobKind(kind.to_owned())),
    };

    let (access_builder, visibility) = (job.access_builder(), job.visibility());
    if let Err(err) = job.run_job(ctx_builder.clone()).await {
        // The missing part is this, should we execute subsequent jobs if the one they depend on fail or not?
        record_job_failure(ctx_builder.clone(), job, err).await?;
    }

    let mut ctx = ctx_builder.build(access_builder.build(visibility)).await?;

    for next in job_info.subsequent_jobs {
        ctx.update_visibility(next.job.visibility());
        ctx.update_access_builder(next.job.access_builder());

        let boxed = Box::new(next.job) as Box<dyn JobProducer + Send + Sync>;
        if next.wait_for_execution {
            ctx_builder
                .job_processor()
                .enqueue_blocking_job(boxed, &ctx)
                .await;
        } else {
            ctx_builder.job_processor().enqueue_job(boxed, &ctx).await;
        }
    }

    if let Err(err) = ctx.commit().await {
        error!("Unable to push jobs to nats: {err}");
    }

    Ok(())
}

async fn record_job_failure(
    ctx_builder: DalContextBuilder,
    job: Box<dyn JobConsumer + Send + Sync>,
    err: JobConsumerError,
) -> Result<()> {
    warn!(error = ?err, "job execution failed, recording a job failure to the database");

    let access_builder = job.access_builder();
    let visibility = job.visibility();
    let ctx = ctx_builder.build(access_builder.build(visibility)).await?;

    JobFailure::new(&ctx, job.type_name(), err.to_string()).await?;

    ctx.commit().await?;

    Err(err.into())
}

fn prepare_graceful_shutdown(
    mut external_shutdown_rx: mpsc::Receiver<ShutdownSource>,
    shutdown_watch_tx: watch::Sender<()>,
) -> Result<oneshot::Receiver<()>> {
    // A oneshot channel signaling the start of a graceful shutdown. Receivers can use this to
    // perform an clean/graceful shutdown work that needs to happen to preserve server integrity.
    let (graceful_shutdown_tx, graceful_shutdown_rx) = oneshot::channel::<()>();
    // A stream of `SIGTERM` signals, emitted as the process receives them.
    let mut sigterm_stream =
        unix::signal(unix::SignalKind::terminate()).map_err(ServerError::Signal)?;

    tokio::spawn(async move {
        fn send_graceful_shutdown(
            graceful_shutdown_tx: oneshot::Sender<()>,
            shutdown_watch_tx: watch::Sender<()>,
        ) {
            // Send shutdown to all long running subscriptions, so they can cleanly terminate
            if shutdown_watch_tx.send(()).is_err() {
                error!("all watch shutdown receivers have already been dropped");
            }
            // Send graceful shutdown to main server thread which stops it from accepting requests.
            // We'll do this step last so as to let all subscriptions have a chance to shutdown.
            if graceful_shutdown_tx.send(()).is_err() {
                error!("the server graceful shutdown receiver has already dropped");
            }
        }

        info!("spawned graceful shutdown handler");

        tokio::select! {
            _ = sigterm_stream.recv() => {
                info!("received SIGTERM signal, performing graceful shutdown");
                send_graceful_shutdown(graceful_shutdown_tx, shutdown_watch_tx);
            }
            source = external_shutdown_rx.recv() => {
                info!(
                    "received external shutdown, performing graceful shutdown; source={:?}",
                    source,
                );
                send_graceful_shutdown(graceful_shutdown_tx, shutdown_watch_tx);
            }
            else => {
                // All other arms are closed, nothing left to do but return
                trace!("returning from graceful shutdown with all select arms closed");
            }
        };
    });

    Ok(graceful_shutdown_rx)
}
