use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
#[cfg(feature = "framework")]
use std::sync::OnceLock;

use futures::channel::mpsc::UnboundedReceiver as Receiver;
use futures::StreamExt;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, timeout, Duration, Instant};
use tracing::{debug, info, instrument, warn};
use typemap_rev::TypeMap;

#[cfg(feature = "voice")]
use super::VoiceGatewayManager;
use super::{
    ShardId,
    ShardManager,
    ShardMessenger,
    ShardQueuerMessage,
    ShardRunner,
    ShardRunnerInfo,
    ShardRunnerOptions,
};
#[cfg(feature = "cache")]
use crate::cache::Cache;
use crate::client::{EventHandler, RawEventHandler};
#[cfg(feature = "framework")]
use crate::framework::Framework;
use crate::gateway::{ConnectionStage, PresenceData, Shard};
use crate::http::Http;
use crate::internal::prelude::*;
use crate::internal::tokio::spawn_named;
use crate::model::gateway::{GatewayIntents, ShardInfo};

const WAIT_BETWEEN_BOOTS_IN_SECONDS: u64 = 5;

/// The shard queuer is a simple loop that runs indefinitely to manage the startup of shards.
///
/// A shard queuer instance _should_ be run in its own thread, due to the blocking nature of the
/// loop itself as well as a 5 second thread sleep between shard starts.
pub struct ShardQueuer {
    /// A copy of [`Client::data`] to be given to runners for contextual dispatching.
    ///
    /// [`Client::data`]: crate::Client::data
    pub data: Arc<RwLock<TypeMap>>,
    /// A reference to an [`EventHandler`], such as the one given to the [`Client`].
    ///
    /// [`Client`]: crate::Client
    pub event_handlers: Vec<Arc<dyn EventHandler>>,
    /// A reference to an [`RawEventHandler`], such as the one given to the [`Client`].
    ///
    /// [`Client`]: crate::Client
    pub raw_event_handlers: Vec<Arc<dyn RawEventHandler>>,
    /// A copy of the framework
    #[cfg(feature = "framework")]
    pub framework: Arc<OnceLock<Arc<dyn Framework>>>,
    /// The instant that a shard was last started.
    ///
    /// This is used to determine how long to wait between shard IDENTIFYs.
    pub last_start: Option<Instant>,
    /// A copy of the [`ShardManager`] to communicate with it.
    pub manager: Arc<Mutex<ShardManager>>,
    /// The shards that are queued for booting.
    ///
    /// This will typically be filled with previously failed boots.
    pub queue: VecDeque<ShardInfo>,
    /// A copy of the map of shard runners.
    pub runners: Arc<Mutex<HashMap<ShardId, ShardRunnerInfo>>>,
    /// A receiver channel for the shard queuer to be told to start shards.
    pub rx: Receiver<ShardQueuerMessage>,
    /// A copy of the client's voice manager.
    #[cfg(feature = "voice")]
    pub voice_manager: Option<Arc<dyn VoiceGatewayManager + 'static>>,
    /// A copy of the URL to use to connect to the gateway.
    pub ws_url: Arc<Mutex<String>>,
    #[cfg(feature = "cache")]
    pub cache: Arc<Cache>,
    pub http: Arc<Http>,
    pub intents: GatewayIntents,
    pub presence: Option<PresenceData>,
}

impl ShardQueuer {
    /// Begins the shard queuer loop.
    ///
    /// This will loop over the internal [`Self::rx`] for [`ShardQueuerMessage`]s, blocking for
    /// messages on what to do.
    ///
    /// If a [`ShardQueuerMessage::Start`] is received, this will:
    ///
    /// 1. Check how much time has passed since the last shard was started
    /// 2. If the amount of time is less than the ratelimit, it will sleep until that time has
    ///    passed
    /// 3. Start the shard by ID
    ///
    /// If a [`ShardQueuerMessage::Shutdown`] is received, this will return and the loop will be
    /// over.
    ///
    /// **Note**: This should be run in its own thread due to the blocking nature of the loop.
    #[instrument(skip(self))]
    pub async fn run(&mut self) {
        // The duration to timeout from reads over the Rx channel. This can be done in a loop, and
        // if the read times out then a shard can be started if one is presently waiting in the
        // queue.
        const TIMEOUT: Duration = Duration::from_secs(WAIT_BETWEEN_BOOTS_IN_SECONDS);

        loop {
            match timeout(TIMEOUT, self.rx.next()).await {
                Ok(Some(ShardQueuerMessage::Shutdown)) => {
                    debug!("[Shard Queuer] Received to shutdown.");
                    break;
                },
                Ok(Some(ShardQueuerMessage::Start(id, total))) => {
                    debug!("[Shard Queuer] Received to start shard {} of {}.", id.0, total.0);
                    self.checked_start(id, total.0).await;
                },
                Ok(None) => break,
                Err(_) => {
                    if let Some(shard) = self.queue.pop_front() {
                        self.checked_start(shard.id, shard.total).await;
                    }
                },
            }
        }
    }

    #[instrument(skip(self))]
    async fn check_last_start(&mut self) {
        let Some(instant) = self.last_start else {return};

        // We must wait 5 seconds between IDENTIFYs to avoid session invalidations.
        let duration = Duration::from_secs(WAIT_BETWEEN_BOOTS_IN_SECONDS);
        let elapsed = instant.elapsed();

        if elapsed >= duration {
            return;
        }

        let to_sleep = duration - elapsed;

        sleep(to_sleep).await;
    }

    #[instrument(skip(self))]
    async fn checked_start(&mut self, id: ShardId, total: u32) {
        debug!("[Shard Queuer] Checked start for shard {} out of {}", id, total);
        self.check_last_start().await;

        if let Err(why) = self.start(id, total).await {
            warn!("[Shard Queuer] Err starting shard {}: {:?}", id, why);
            info!("[Shard Queuer] Re-queueing start of shard {}", id);

            self.queue.push_back(ShardInfo::new(id, total));
        }

        self.last_start = Some(Instant::now());
    }

    #[instrument(skip(self))]
    async fn start(&mut self, id: ShardId, total: u32) -> Result<()> {
        let shard_info = ShardInfo::new(id, total);

        let mut shard = Shard::new(
            Arc::clone(&self.ws_url),
            self.http.token(),
            shard_info,
            self.intents,
            self.presence.clone(),
        )
        .await?;

        let cloned_http = Arc::clone(&self.http);
        shard.set_application_id_callback(move |id| cloned_http.set_application_id(id));

        let mut runner = ShardRunner::new(ShardRunnerOptions {
            data: Arc::clone(&self.data),
            event_handlers: self.event_handlers.clone(),
            raw_event_handlers: self.raw_event_handlers.clone(),
            #[cfg(feature = "framework")]
            framework: self.framework.get().map(Arc::clone),
            manager: Arc::clone(&self.manager),
            #[cfg(feature = "voice")]
            voice_manager: self.voice_manager.clone(),
            #[cfg(feature = "cache")]
            cache: Arc::clone(&self.cache),
            http: Arc::clone(&self.http),
        });

        let runner_info = ShardRunnerInfo {
            latency: None,
            runner_tx: ShardMessenger::new(&runner),
            stage: ConnectionStage::Disconnected,
            shard: Arc::new(Mutex::new(shard)),
        };

        let shard2 = Arc::clone(&runner_info.shard);
        spawn_named("shard_queuer::stop", async move {
            drop(runner.run(&shard2).await);
            debug!("[ShardRunner {:?}] Stopping", shard2.lock().await.shard_info());
        });

        self.runners.lock().await.insert(id, runner_info);

        Ok(())
    }
}
