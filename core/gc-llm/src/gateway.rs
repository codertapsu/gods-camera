//! The gateway the apps hold: the providers, the cloud switches, the meter,
//! and calls that block until the answer is in.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{
    BatchInfo, BatchItem, BatchOutcome, BatchResult, CloudSwitches, Feature, GeneratedImage, ImageRequest, LlmError, Meter, ModelInfo,
    PriceTable, Provider, ProviderKind, Reply, Request, StopReason, UsageReport,
};

/// Where a streamed answer's pieces go: called on the thread that made the call.
type OnText<'a> = &'a mut (dyn FnMut(&str) + Send);

/// What the settings screen shows about a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub has_key: bool,
    /// Data sent to it stays on the user's own computers.
    pub local: bool,
    pub default_model: Option<String>,
    pub default_image_model: Option<String>,
}

/// The one way the apps reach a model.
///
/// It keeps its files in the folder it is opened on: `usage.json` for the
/// meter, `prices.json` for the prices, and `cloud-features.json` for the
/// switches. Every call that sends the user's data names the [`Feature`] it
/// is for, and is refused before anything is sent when the feature is set to
/// stay on the user's devices and the provider is in the cloud.
///
/// The calls block, and must be made from a plain thread, not from async
/// code: there, use `spawn_blocking`, or call the [`Provider`] directly.
pub struct Gateway {
    folder: PathBuf,
    runtime: tokio::runtime::Runtime,
    providers: Mutex<BTreeMap<String, Arc<Provider>>>,
    switches: Mutex<CloudSwitches>,
    meter: Meter,
    meter_problem: Mutex<Option<String>>,
}

impl Gateway {
    /// The gateway with its files in `folder`, which is made if it is missing.
    /// It has no providers until [`Gateway::set_provider`] adds them.
    pub fn open(folder: impl AsRef<Path>) -> Result<Gateway, LlmError> {
        let folder = folder.as_ref().to_path_buf();
        let meter = Meter::open(&folder)?;
        // Two threads keep the connections going between calls; each call
        // runs on the thread that made it.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("gc-llm")
            .enable_all()
            .build()
            .map_err(|error| LlmError::Storage { message: format!("the gateway's threads could not start: {error}") })?;
        Ok(Gateway {
            switches: Mutex::new(CloudSwitches::load(&folder)),
            folder,
            runtime,
            providers: Mutex::new(BTreeMap::new()),
            meter,
            meter_problem: Mutex::new(None),
        })
    }

    /// Counts the meter's days in the user's time zone, `minutes` east of UTC.
    pub fn with_utc_offset(mut self, minutes: i32) -> Gateway {
        self.meter = self.meter.with_utc_offset(minutes);
        self
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Adds a provider, or replaces the one with the same name: to change a
    /// key, set the provider again with the new one.
    pub fn set_provider(&self, provider: Provider) {
        lock(&self.providers).insert(provider.name().to_string(), Arc::new(provider));
    }

    pub fn remove_provider(&self, name: &str) {
        lock(&self.providers).remove(name);
    }

    pub fn providers(&self) -> Vec<ProviderInfo> {
        lock(&self.providers)
            .values()
            .map(|provider| ProviderInfo {
                name: provider.name().to_string(),
                kind: provider.kind(),
                base_url: provider.base_url().to_string(),
                has_key: provider.has_key(),
                local: provider.is_local(),
                default_model: provider.default_model().map(str::to_string),
                default_image_model: provider.default_image_model().map(str::to_string),
            })
            .collect()
    }

    pub fn provider(&self, name: &str) -> Result<Arc<Provider>, LlmError> {
        lock(&self.providers).get(name).cloned().ok_or_else(|| LlmError::UnknownProvider { name: name.to_string() })
    }

    /// The provider's models, for the model picker. Sends none of the user's data.
    pub fn models(&self, provider: &str) -> Result<Vec<ModelInfo>, LlmError> {
        let provider = self.provider(provider)?;
        self.block_on(provider.models())?
    }

    /// Asks a model and waits for the whole answer.
    ///
    /// A refusal comes back as [`LlmError::Refused`], never as an answer;
    /// an answer cut off at `max_tokens` comes back with [`StopReason::Length`].
    pub fn chat(&self, feature: Feature, provider: &str, request: &Request) -> Result<Reply, LlmError> {
        self.send_chat(feature, provider, request, None)
    }

    /// The same, handing each piece of the answer to `on_text` as it arrives,
    /// on the thread that made the call. After a refusal, the pieces already
    /// handed over are a fragment: drop them.
    pub fn chat_streaming(
        &self,
        feature: Feature,
        provider: &str,
        request: &Request,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Reply, LlmError> {
        self.send_chat(feature, provider, request, Some(on_text))
    }

    fn send_chat(&self, feature: Feature, name: &str, request: &Request, on_text: Option<OnText<'_>>) -> Result<Reply, LlmError> {
        let provider = self.provider(name)?;
        self.check_cloud(feature, &provider)?;
        let reply = self.block_on(provider.chat_with(request, on_text))??;
        // A refused answer is billed too.
        self.count(&provider, &reply.model, &reply.usage);
        if let StopReason::Refusal { category, explanation } = &reply.stop {
            return Err(LlmError::Refused {
                provider: reply.provider.clone(),
                model: reply.model.clone(),
                category: category.clone(),
                explanation: explanation.clone(),
            });
        }
        Ok(reply)
    }

    /// Makes a picture, or one from the pictures given: for try-on, a photo
    /// of the person and one of the garment.
    pub fn generate_image(&self, feature: Feature, provider: &str, request: &ImageRequest) -> Result<GeneratedImage, LlmError> {
        let provider = self.provider(provider)?;
        self.check_cloud(feature, &provider)?;
        let image = self.block_on(provider.generate_image(request))??;
        self.count(&provider, &image.model, &image.usage);
        Ok(image)
    }

    /// Sends many requests as one Anthropic message batch, at half the price.
    /// Poll [`Gateway::batch`] until it has ended, then fetch the results.
    pub fn submit_batch(&self, feature: Feature, provider: &str, items: &[BatchItem]) -> Result<BatchInfo, LlmError> {
        let provider = self.provider(provider)?;
        self.check_cloud(feature, &provider)?;
        self.block_on(provider.submit_batch(items))?
    }

    pub fn batch(&self, provider: &str, id: &str) -> Result<BatchInfo, LlmError> {
        let provider = self.provider(provider)?;
        self.block_on(provider.batch(id))?
    }

    /// The results of an ended batch, by the ids the requests were sent with.
    /// Their usage is counted the first time they are fetched.
    pub fn batch_results(&self, provider: &str, id: &str) -> Result<Vec<BatchResult>, LlmError> {
        let provider = self.provider(provider)?;
        let results = self.block_on(provider.batch_results(id))??;
        let used: Vec<_> = results
            .iter()
            .filter_map(|result| match &result.outcome {
                BatchOutcome::Succeeded(reply) => Some((reply.model.clone(), reply.usage)),
                _ => None,
            })
            .collect();
        if let Err(error) = self.meter.record_batch(provider.name(), id, &used) {
            *lock(&self.meter_problem) = Some(error.to_string());
        }
        Ok(results)
    }

    /// Whether the feature may use a cloud provider. Every feature starts
    /// with this off.
    pub fn cloud_allowed(&self, feature: Feature) -> bool {
        lock(&self.switches).allowed(feature)
    }

    /// Turns cloud use for a feature on or off, and keeps the choice.
    pub fn set_cloud_allowed(&self, feature: Feature, allowed: bool) -> Result<(), LlmError> {
        let mut switches = lock(&self.switches);
        switches.set(feature, allowed);
        switches.save(&self.folder)
    }

    /// What was used from day `from` to day `to` (`YYYY-MM-DD`, both
    /// included), with the estimated cost.
    pub fn usage(&self, from: &str, to: &str) -> Result<UsageReport, LlmError> {
        self.meter.report(from, to)
    }

    /// Today on the meter, `YYYY-MM-DD`.
    pub fn today(&self) -> String {
        self.meter.today()
    }

    pub fn prices(&self) -> PriceTable {
        self.meter.prices()
    }

    /// Reads the price table again after the user edited it, and says what is
    /// wrong with it if it cannot be used.
    pub fn reload_prices(&self) -> Option<String> {
        self.meter.reload_prices();
        self.meter.price_problem()
    }

    /// The last time the meter could not write its file, why. The answers
    /// still came; only their count is missing.
    pub fn meter_problem(&self) -> Option<String> {
        lock(&self.meter_problem).clone()
    }

    pub(crate) fn check_cloud(&self, feature: Feature, provider: &Provider) -> Result<(), LlmError> {
        if provider.is_local() || self.cloud_allowed(feature) {
            Ok(())
        } else {
            Err(LlmError::CloudNotAllowed { feature, provider: provider.name().to_string() })
        }
    }

    fn count(&self, provider: &Provider, model: &str, usage: &crate::Usage) {
        if let Err(error) = self.meter.record(provider.name(), provider.is_local(), model, usage, false) {
            *lock(&self.meter_problem) = Some(error.to_string());
        }
    }

    /// Runs async work to its end on the calling thread.
    fn block_on<F: Future>(&self, work: F) -> Result<F::Output, LlmError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(LlmError::InvalidInput {
                message: "the gateway's calls block; from async code, make them inside spawn_blocking or call the provider directly".into(),
            });
        }
        Ok(self.runtime.block_on(work))
    }
}

/// A lock that a panic elsewhere does not spoil: the data behind these locks
/// is whole after every change.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BatchItem, ImageRequest};

    fn shared<T: Send + Sync>() {}

    fn sendable<F: Future + Send>(_: F) {}

    #[test]
    fn the_gateway_and_its_parts_can_be_shared_between_threads() {
        shared::<Gateway>();
        shared::<Provider>();
        shared::<LlmError>();
    }

    /// The hub serves requests from async tasks that may move between
    /// threads, so every call on a provider must be a future that can.
    #[test]
    fn a_providers_calls_can_run_on_any_thread() {
        let provider = Provider::ollama();
        let request = Request::new("model", "x");
        let mut on_text = |_: &str| {};
        sendable(provider.chat(&request));
        sendable(provider.chat_streaming(&request, &mut on_text));
        sendable(provider.models());
        sendable(provider.generate_image(&ImageRequest { model: "m".into(), prompt: "p".into(), pictures: vec![] }));
        sendable(provider.submit_batch(&[] as &[BatchItem]));
        sendable(provider.batch("id"));
        sendable(provider.batch_results("id"));
    }
}
