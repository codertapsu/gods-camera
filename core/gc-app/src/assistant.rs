//! The assistant: language and image models for the desktop app (plan,
//! Phase 5 and the try-on of Phase 9), through gc-llm's gateway.
//!
//! The keys for Claude, OpenAI and Gemini are entered in Settings and kept
//! with the app's other secrets: in Credential Manager for the user's own app,
//! in the data folder for a try-out. They are handed to the gateway and never
//! shown again. Local models need no key: Ollama on this computer is used
//! when it runs.
//!
//! Work that takes time runs on a thread of its own and is followed by
//! polling, as the rest of the app's model is: `start` gives a number, `task`
//! says how far it is and, at the end, what came of it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use gc_llm::{ApiKey, ChangeKind, DocOptions, Feature, Gateway, ImageRequest, Language, LlmError, Picture, Provider, Target};
use serde::{Deserialize, Serialize};

use crate::secrets::Secrets;

/// A cloud provider whose key the user may enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CloudProvider {
    Anthropic,
    OpenAi,
    Gemini,
}

impl CloudProvider {
    /// The gateway's name for it.
    fn name(self) -> &'static str {
        match self {
            CloudProvider::Anthropic => "anthropic",
            CloudProvider::OpenAi => "openai",
            CloudProvider::Gemini => "gemini",
        }
    }

    /// Where its key is kept among the app's secrets.
    fn secret(self) -> &'static str {
        match self {
            CloudProvider::Anthropic => "Anthropic",
            CloudProvider::OpenAi => "OpenAI",
            CloudProvider::Gemini => "Google",
        }
    }

    fn provider(self, key: Option<ApiKey>) -> Provider {
        match self {
            CloudProvider::Anthropic => Provider::anthropic(key),
            CloudProvider::OpenAi => Provider::openai(key),
            CloudProvider::Gemini => Provider::gemini(key),
        }
    }

    const ALL: [CloudProvider; 3] = [CloudProvider::Anthropic, CloudProvider::OpenAi, CloudProvider::Gemini];
}

/// The features that may use a model, each with its own cloud switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AiFeature {
    CleanUp,
    Summary,
    Translation,
    Extraction,
    Questions,
    HardPages,
    TryOn,
    NameScan,
}

impl From<AiFeature> for Feature {
    fn from(feature: AiFeature) -> Feature {
        match feature {
            AiFeature::CleanUp => Feature::CleanUp,
            AiFeature::Summary => Feature::Summary,
            AiFeature::Translation => Feature::Translation,
            AiFeature::Extraction => Feature::Extraction,
            AiFeature::Questions => Feature::Questions,
            AiFeature::HardPages => Feature::HardPages,
            AiFeature::TryOn => Feature::TryOn,
            AiFeature::NameScan => Feature::NameScan,
        }
    }
}

/// A provider as Settings shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProviderStatus {
    /// The gateway's name: `anthropic`, `openai`, `gemini` or `ollama`.
    pub name: String,
    /// The data sent to it stays on the user's own computers.
    pub local: bool,
    pub has_key: bool,
    pub default_model: Option<String>,
}

/// A model to choose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
}

/// A provider's models, or why they could not be listed.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ModelList {
    /// The models that write text.
    pub models: Vec<String>,
    /// The models that make pictures.
    pub image_models: Vec<String>,
    pub problem: Option<AssistantProblem>,
}

/// What went wrong, as the screens put it to the user.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum AssistantProblem {
    /// No key is entered for the provider.
    NoKey,
    /// The provider turned the key down.
    KeyRefused,
    /// Too many requests; wait this many seconds, when the provider said.
    RateLimited { seconds: Option<u32> },
    /// The provider is busy or could not be reached; try again later.
    Temporary { message: String },
    /// The provider did not accept the request: its own words.
    BadRequest { message: String },
    /// The model declined.
    Refused { model: String },
    /// This feature stays on the user's devices, and the provider is in the cloud.
    CloudNotAllowed,
    /// Stopped by the user.
    Cancelled,
    Other { message: String },
}

impl From<LlmError> for AssistantProblem {
    fn from(error: LlmError) -> AssistantProblem {
        match error {
            LlmError::NoKey { .. } => AssistantProblem::NoKey,
            LlmError::KeyRefused { .. } => AssistantProblem::KeyRefused,
            LlmError::RateLimited { retry_after, .. } => AssistantProblem::RateLimited { seconds: retry_after.map(|wait| wait.as_secs() as u32) },
            LlmError::BadRequest { message, .. } => AssistantProblem::BadRequest { message },
            LlmError::Refused { model, .. } => AssistantProblem::Refused { model },
            LlmError::CloudNotAllowed { .. } => AssistantProblem::CloudNotAllowed,
            LlmError::Cancelled => AssistantProblem::Cancelled,
            error if error.is_temporary() => AssistantProblem::Temporary { message: error.to_string() },
            error => AssistantProblem::Other { message: error.to_string() },
        }
    }
}

/// Something to do with a document's text.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum DocumentTask {
    /// Fix what text recognition got wrong, and nothing else.
    CleanUp,
    Summary { english: bool },
    /// Into English, or into Vietnamese.
    Translate { english: bool },
    Ask { question: String },
    /// The title, date, people, places, organisations, amounts and contacts the document names.
    KeyFacts,
}

/// A picture to make from photos (Phase 9).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PictureTask {
    /// The person in the first photo wearing the garment in the second.
    TryOn { person: String, garment: String },
    /// The person with another hairstyle, as described.
    Hairstyle { person: String, style: String },
}

/// One stretch of a clean-up's changes, in order.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChangePiece {
    /// `same`, `removed` or `added`.
    pub kind: String,
    pub text: String,
}

/// What a task gave.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TaskResult {
    pub text: String,
    /// A clean-up's changes, word by word.
    pub changes: Vec<ChangePiece>,
    /// A clean-up changed more than recognition mistakes may account for: look before taking it.
    pub flagged: bool,
    /// An answer's paragraphs, quoted from the document.
    pub quotes: Vec<String>,
    /// A picture task's PNG, as a file.
    pub picture: Option<String>,
    /// The model that did it.
    pub model: String,
}

/// How a task stands.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TaskState {
    pub running: bool,
    /// Requests done, and requests in all, when the task knows.
    pub done: u32,
    pub total: u32,
    pub result: Option<TaskResult>,
    pub problem: Option<AssistantProblem>,
}

/// What the models cost, as estimates.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageSummary {
    pub today_cost: f64,
    pub month_cost: f64,
    pub today_requests: u64,
    pub month_requests: u64,
    /// Some cloud model used has no price, so the costs are short of the truth.
    pub incomplete: bool,
}

struct Task {
    state: TaskState,
    stop: Arc<AtomicBool>,
}

/// What the user chose, kept between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Kept {
    text: Option<ModelChoice>,
    image: Option<ModelChoice>,
}

/// The assistant's model.
#[derive(uniffi::Object)]
pub struct Assistant {
    gateway: Arc<Gateway>,
    secrets: Arc<dyn Secrets>,
    folder: PathBuf,
    kept: Mutex<Kept>,
    tasks: Arc<Mutex<HashMap<u64, Task>>>,
    next: AtomicU64,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Assistant {
    /// The assistant on the folder `data_dir`, with `secrets` where keys are
    /// kept. Tests bring their own. `utc_offset` is how many minutes the
    /// user's clock is ahead of UTC, so that the meter's days are theirs.
    pub fn with_secrets(data_dir: &Path, secrets: Arc<dyn Secrets>, utc_offset: i32) -> Result<Arc<Self>, String> {
        let folder = data_dir.join("Assistant");
        let gateway = Gateway::open(&folder).map_err(|error| error.to_string())?.with_utc_offset(utc_offset);
        for cloud in CloudProvider::ALL {
            let key = secrets.read(cloud.secret()).ok().flatten().and_then(|bytes| String::from_utf8(bytes).ok()).and_then(|text| ApiKey::new(text).ok());
            gateway.set_provider(cloud.provider(key));
        }
        gateway.set_provider(Provider::ollama());
        let kept = std::fs::read(folder.join("choice.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Ok(Arc::new(Self {
            gateway: Arc::new(gateway),
            secrets,
            folder,
            kept: Mutex::new(kept),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        }))
    }

    /// Use another provider in place of one of the same name: a stand-in server, for tests.
    #[doc(hidden)]
    pub fn set_provider(&self, provider: Provider) {
        self.gateway.set_provider(provider);
    }

    fn keep(&self, kept: &Kept) {
        let _ = std::fs::create_dir_all(&self.folder);
        let _ = std::fs::write(self.folder.join("choice.json"), serde_json::to_vec_pretty(kept).unwrap_or_default());
    }

    /// The model for document work: the user's choice, or the first provider with a key.
    fn text_target(&self) -> Option<Target> {
        if let Some(choice) = &locked(&self.kept).text {
            return Some(Target::new(&choice.provider, &choice.model));
        }
        self.gateway
            .providers()
            .into_iter()
            .filter(|provider| provider.has_key && !provider.local)
            .find_map(|provider| provider.default_model.map(|model| Target::new(provider.name, model)))
    }

    fn image_target(&self) -> Option<Target> {
        if let Some(choice) = &locked(&self.kept).image {
            return Some(Target::new(&choice.provider, &choice.model));
        }
        self.gateway
            .providers()
            .into_iter()
            .filter(|provider| provider.has_key)
            .find_map(|provider| provider.default_image_model.map(|model| Target::new(provider.name, model)))
    }

    /// Run `work` on a thread of its own, as task number `id`.
    fn run(&self, work: impl FnOnce(&Gateway, &mut dyn FnMut(usize, usize) -> bool) -> Result<TaskResult, LlmError> + Send + 'static) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        locked(&self.tasks).insert(
            id,
            Task { state: TaskState { running: true, done: 0, total: 0, result: None, problem: None }, stop: Arc::clone(&stop) },
        );
        let (gateway, tasks) = (Arc::clone(&self.gateway), Arc::clone(&self.tasks));
        std::thread::spawn(move || {
            let progress_tasks = Arc::clone(&tasks);
            let mut progress = move |done: usize, total: usize| {
                if let Some(task) = locked(&progress_tasks).get_mut(&id) {
                    task.state.done = done as u32;
                    task.state.total = total as u32;
                }
                !stop.load(Ordering::Relaxed)
            };
            let outcome = work(&gateway, &mut progress);
            if let Some(task) = locked(&tasks).get_mut(&id) {
                task.state.running = false;
                match outcome {
                    Ok(result) => task.state.result = Some(result),
                    Err(error) => task.state.problem = Some(error.into()),
                }
            }
        });
        id
    }
}

#[uniffi::export]
impl Assistant {
    /// The assistant for the app on `data_dir`. A try-out keeps its keys in
    /// its own folder. `utc_offset`: minutes the user's clock is ahead of UTC.
    #[uniffi::constructor]
    pub fn new(data_dir: String, try_out: bool, utc_offset: i32) -> Arc<Self> {
        let secrets = crate::platform::app_secrets(&data_dir, try_out);
        match Self::with_secrets(Path::new(&data_dir), Arc::clone(&secrets), utc_offset) {
            Ok(assistant) => assistant,
            // A folder that cannot be written is no reason not to start: the assistant then keeps nothing.
            Err(_) => Self::with_secrets(&std::env::temp_dir().join("GodsCamera-assistant"), secrets, utc_offset)
                .expect("the temporary folder can be written"),
        }
    }

    /// The providers, as Settings shows them. Keys are never given out, only whether there is one.
    pub fn providers(&self) -> Vec<ProviderStatus> {
        let mut providers: Vec<ProviderStatus> = self
            .gateway
            .providers()
            .into_iter()
            .map(|provider| ProviderStatus { name: provider.name, local: provider.local, has_key: provider.has_key, default_model: provider.default_model })
            .collect();
        let order = |name: &str| ["anthropic", "openai", "gemini", "ollama"].iter().position(|known| *known == name).unwrap_or(9);
        providers.sort_by_key(|provider| order(&provider.name));
        providers
    }

    /// Keep a provider's key and use it from now on. `None` when it is kept;
    /// otherwise why not, in plain words.
    pub fn set_key(&self, provider: CloudProvider, key: String) -> Option<String> {
        let key = key.trim().to_string();
        let checked = match ApiKey::new(key.clone()) {
            Ok(checked) => checked,
            Err(error) => return Some(error.to_string()),
        };
        if let Err(problem) = self.secrets.write(provider.secret(), key.as_bytes(), "A key God's Camera uses for a language or image model.") {
            return Some(problem);
        }
        self.gateway.set_provider(provider.provider(Some(checked)));
        None
    }

    /// Forget a provider's key.
    pub fn remove_key(&self, provider: CloudProvider) {
        self.secrets.delete(provider.secret());
        self.gateway.set_provider(provider.provider(None));
    }

    /// A provider's models. It asks the provider, so call it off the window's thread.
    pub fn models(&self, provider: String) -> ModelList {
        match self.gateway.models(&provider) {
            Ok(models) => ModelList {
                models: models.iter().filter(|model| model.kind == gc_llm::ModelKind::Text).map(|model| model.id.clone()).collect(),
                image_models: models.iter().filter(|model| model.kind == gc_llm::ModelKind::Image).map(|model| model.id.clone()).collect(),
                problem: None,
            },
            Err(error) => ModelList { models: Vec::new(), image_models: Vec::new(), problem: Some(error.into()) },
        }
    }

    /// The model used for documents, and the one for pictures.
    pub fn text_model(&self) -> Option<ModelChoice> {
        self.text_target().map(|target| ModelChoice { provider: target.provider, model: target.model })
    }

    pub fn image_model(&self) -> Option<ModelChoice> {
        self.image_target().map(|target| ModelChoice { provider: target.provider, model: target.model })
    }

    pub fn set_text_model(&self, choice: Option<ModelChoice>) {
        let mut kept = locked(&self.kept);
        kept.text = choice;
        self.keep(&kept);
    }

    pub fn set_image_model(&self, choice: Option<ModelChoice>) {
        let mut kept = locked(&self.kept);
        kept.image = choice;
        self.keep(&kept);
    }

    /// The feature may send the user's data to a cloud provider. Off until the user turns it on.
    pub fn cloud_allowed(&self, feature: AiFeature) -> bool {
        self.gateway.cloud_allowed(feature.into())
    }

    pub fn set_cloud_allowed(&self, feature: AiFeature, allowed: bool) {
        let _ = self.gateway.set_cloud_allowed(feature.into(), allowed);
    }

    /// Start work on a document's text. Follow it with `task`.
    pub fn start_document(&self, task: DocumentTask, text: String) -> u64 {
        let Some(target) = self.text_target() else {
            return self.run(|_, _| Err(LlmError::NoKey { provider: CloudProvider::Anthropic.name().into() }));
        };
        self.run(move |gateway, progress| {
            let options = DocOptions::default();
            let model = format!("{}/{}", target.provider, target.model);
            let plain = |text: String| TaskResult { text, changes: Vec::new(), flagged: false, quotes: Vec::new(), picture: None, model: model.clone() };
            match task {
                DocumentTask::CleanUp => {
                    let cleaned = gateway.clean_up(&target, &text, &options, Some(progress))?;
                    let changes = cleaned
                        .diff
                        .changes
                        .iter()
                        .map(|change| ChangePiece {
                            kind: match change.kind {
                                ChangeKind::Same => "same",
                                ChangeKind::Removed => "removed",
                                ChangeKind::Added => "added",
                            }
                            .into(),
                            text: change.text.clone(),
                        })
                        .collect();
                    Ok(TaskResult { text: cleaned.text, changes, flagged: cleaned.flagged, quotes: Vec::new(), picture: None, model })
                }
                DocumentTask::Summary { english } => {
                    let language = if english { Language::English } else { Language::Vietnamese };
                    Ok(plain(gateway.summarise(&target, &text, language, &options, Some(progress))?))
                }
                DocumentTask::Translate { english } => {
                    let language = if english { Language::English } else { Language::Vietnamese };
                    Ok(plain(gateway.translate(&target, &text, language, &options, Some(progress))?))
                }
                DocumentTask::Ask { question } => {
                    let answer = gateway.ask(&target, &text, &question, &options, Some(progress))?;
                    Ok(TaskResult { text: answer.text, changes: Vec::new(), flagged: false, quotes: answer.quotes, picture: None, model })
                }
                DocumentTask::KeyFacts => {
                    let facts = gateway.extract(&target, &text, &key_facts_schema(), &options, Some(progress))?;
                    Ok(plain(key_facts_text(&facts)))
                }
            }
        })
    }

    /// Start making a picture from photos (Phase 9). Follow it with `task`; the picture is saved in the app's data.
    pub fn start_picture(&self, task: PictureTask) -> u64 {
        let Some(target) = self.image_target() else {
            return self.run(|_, _| Err(LlmError::NoKey { provider: CloudProvider::OpenAi.name().into() }));
        };
        let out = self.folder.join("Pictures");
        self.run(move |gateway, progress| {
            let read = |path: &str| std::fs::read(path).map_err(|error| LlmError::InvalidInput { message: format!("{path} cannot be read: {error}") });
            let (prompt, photos) = match &task {
                PictureTask::TryOn { person, garment } => (TRY_ON.to_string(), vec![read(person)?, read(garment)?]),
                PictureTask::Hairstyle { person, style } => (format!("{HAIRSTYLE}\n\nThe hairstyle: {}", style.trim()), vec![read(person)?]),
            };
            let pictures = photos.into_iter().map(Picture::new).collect::<Result<Vec<_>, _>>()?;
            progress(0, 1);
            let made = gateway.generate_image(Feature::TryOn, &target.provider, &ImageRequest { model: target.model.clone(), prompt, pictures })?;
            let _ = std::fs::create_dir_all(&out);
            let made_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis());
            let file = out.join(format!("{made_at}.png"));
            std::fs::write(&file, &made.png).map_err(|error| LlmError::Storage { message: error.to_string() })?;
            progress(1, 1);
            Ok(TaskResult {
                text: made.text,
                changes: Vec::new(),
                flagged: false,
                quotes: Vec::new(),
                picture: Some(file.to_string_lossy().into_owned()),
                model: format!("{}/{}", made.provider, made.model),
            })
        })
    }

    /// How a task stands.
    pub fn task(&self, id: u64) -> TaskState {
        locked(&self.tasks)
            .get(&id)
            .map(|task| task.state.clone())
            .unwrap_or(TaskState { running: false, done: 0, total: 0, result: None, problem: Some(AssistantProblem::Other { message: "no such task".into() }) })
    }

    /// Stop a task after the request it is waiting for.
    pub fn cancel(&self, id: u64) {
        if let Some(task) = locked(&self.tasks).get(&id) {
            task.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Forget a finished task.
    pub fn forget(&self, id: u64) {
        let mut tasks = locked(&self.tasks);
        if tasks.get(&id).is_some_and(|task| !task.state.running) {
            tasks.remove(&id);
        }
    }

    /// What the models cost today and this month, as estimates.
    pub fn usage(&self) -> UsageSummary {
        let today = self.gateway.today();
        let month = format!("{}-01", &today[..7.min(today.len())]);
        let sum = |from: &str| -> (f64, u64, bool) {
            match self.gateway.usage(from, &today) {
                Ok(report) => (report.total_cost, report.lines.iter().map(|line| line.requests).sum(), !report.unpriced.is_empty()),
                Err(_) => (0.0, 0, false),
            }
        };
        let (today_cost, today_requests, today_unpriced) = sum(&today);
        let (month_cost, month_requests, month_unpriced) = sum(&month);
        UsageSummary { today_cost, month_cost, today_requests, month_requests, incomplete: today_unpriced || month_unpriced }
    }

    /// Forget the keys, the choices, the meter and the pictures made.
    pub fn delete_everything(&self) {
        for cloud in CloudProvider::ALL {
            self.remove_key(cloud);
        }
        *locked(&self.kept) = Kept::default();
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// What a try-on asks of the image model. The person's own photo and a
/// garment's; nothing else of them is changed.
const TRY_ON: &str = "Make one photorealistic picture of the person in the first photo wearing the garment shown in the second photo. \
Keep the person exactly as they are: the same face, hair, skin, body shape, pose, expression and background, and the same light. \
Change only the clothing, so that the garment fits the person naturally, with its own colour, pattern, material and cut. \
The person stays fully and decently dressed. Do not add, remove or change anything else.";

/// What a hairstyle preview asks of the image model.
const HAIRSTYLE: &str = "Make one photorealistic picture of the person in the photo with a different hairstyle, described below. \
Keep the person exactly as they are: the same face, skin, body, clothing, pose, expression, background and light. \
Change only the hair: its style, length and colour as described. Do not change anything else.";

/// The general facts a document can hold, as a JSON schema.
fn key_facts_schema() -> serde_json::Value {
    let list = serde_json::json!({ "type": "array", "items": { "type": "string" } });
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "dates", "people", "places", "organisations", "amounts", "contacts"],
        "properties": {
            "title": { "type": "string", "description": "What the document is, in a few words, in its own language" },
            "dates": list, "people": list, "places": list, "organisations": list,
            "amounts": { "type": "array", "items": { "type": "string" }, "description": "Sums of money and other quantities, with their units" },
            "contacts": { "type": "array", "items": { "type": "string" }, "description": "Telephone numbers, addresses, e-mail and web addresses" }
        }
    })
}

/// The facts as lines to read.
fn key_facts_text(facts: &serde_json::Value) -> String {
    let mut out = Vec::new();
    if let Some(title) = facts.get("title").and_then(|title| title.as_str()).filter(|title| !title.trim().is_empty()) {
        out.push(title.trim().to_string());
    }
    for field in ["dates", "people", "places", "organisations", "amounts", "contacts"] {
        let items: Vec<&str> = facts.get(field).and_then(|list| list.as_array()).map(|list| list.iter().filter_map(|item| item.as_str()).collect()).unwrap_or_default();
        if !items.is_empty() {
            out.push(format!("{field}: {}", items.join("; ")));
        }
    }
    out.join("\n")
}
