//! The usage and cost meter: tokens per provider, model and day, kept in a
//! JSON file, and what they cost by a price table the user can edit.
//!
//! The prices are estimates. The defaults were taken from the providers'
//! price pages on 2026-11-14 and are written to `prices.json` the first time,
//! where the user can change them when a provider changes its prices. A model
//! that has no price is counted without a cost and listed as unpriced; a
//! model on the user's own computers costs nothing.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{LlmError, Usage};

/// What a model costs, in US dollars per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub provider: String,
    /// The model's id. A model whose id starts with this one has its price
    /// too, so a dated version (`…-2026-09-03`) is found.
    pub model: String,
    pub input: f64,
    /// Input read from the prompt cache.
    pub cached_input: f64,
    /// Input written into the prompt cache.
    pub cache_write: f64,
    pub output: f64,
    /// What a request in a batch costs against a single one; Anthropic's
    /// batches cost half.
    #[serde(default = "half")]
    pub batch: f64,
    /// Where the price was read.
    #[serde(default)]
    pub source: String,
}

fn half() -> f64 {
    0.5
}

/// The price table, as `prices.json` holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceTable {
    #[serde(default)]
    pub about: String,
    /// The day the prices were read from the providers' pages.
    #[serde(default)]
    pub taken: String,
    pub prices: Vec<Price>,
}

impl Default for PriceTable {
    fn default() -> Self {
        let anthropic = "https://platform.claude.com/docs/en/about-claude/pricing";
        let openai = "https://developers.openai.com/api/docs/models";
        let gemini = "https://ai.google.dev/gemini-api/docs/pricing";
        let price = |provider: &str, model: &str, [input, cached_input, cache_write, output]: [f64; 4], source: &str| Price {
            provider: provider.into(),
            model: model.into(),
            input,
            cached_input,
            cache_write,
            output,
            batch: 0.5,
            source: source.into(),
        };
        PriceTable {
            about: "Estimated prices in US dollars per million tokens, read from the providers' price pages on 2026-11-14. \
                    They are estimates: change them here when a provider changes its prices. cache_write is Anthropic's \
                    five-minute cache. Picture models are priced by their picture tokens. Gemini 4 Carbon costs half \
                    these prices until 2026-11-30; the prices from 2026-12-01 are given."
                .into(),
            taken: "2026-11-14".into(),
            prices: vec![
                price("anthropic", "claude-opus-5-5", [4.0, 0.2, 5.0, 20.0], anthropic),
                price("anthropic", "claude-sonnet-5-5", [2.0, 0.1, 2.5, 10.0], anthropic),
                price("anthropic", "claude-haiku-5-5", [0.1, 0.01, 0.125, 0.5], anthropic),
                price("anthropic", "claude-fable-5-2", [8.0, 0.2, 10.0, 40.0], anthropic),
                price("openai", "gpt-6-astra", [10.0, 1.0, 12.5, 50.0], openai),
                price("openai", "gpt-6.1-sol", [2.0, 0.1, 2.5, 10.0], openai),
                price("openai", "gpt-6-luna", [0.1, 0.01, 0.125, 0.5], openai),
                price("openai", "gpt-image-2.6-sunburst", [8.0, 2.0, 8.0, 30.0], openai),
                price("gemini", "gemini-4-carbon", [4.0, 0.2, 4.0, 20.0], gemini),
                price("gemini", "gemini-4-argon", [4.0, 0.2, 4.0, 20.0], gemini),
                price("gemini", "gemini-3-pro-image", [2.0, 2.0, 2.0, 120.0], gemini),
            ],
        }
    }
}

impl PriceTable {
    /// The price of a model: the entry with its id, or failing that the
    /// longest entry its id starts with.
    pub fn find(&self, provider: &str, model: &str) -> Option<&Price> {
        let mine = self.prices.iter().filter(|price| price.provider == provider);
        mine.clone().find(|price| price.model == model).or_else(|| {
            mine.filter(|price| !price.model.is_empty() && model.starts_with(&price.model)).max_by_key(|price| price.model.len())
        })
    }

    /// What the usage cost, if the model has a price.
    pub fn cost(&self, provider: &str, model: &str, usage: &Usage, batch: bool) -> Option<f64> {
        let price = self.find(provider, model)?;
        let full = usage.input as f64 * price.input
            + usage.cached as f64 * price.cached_input
            + usage.cache_written as f64 * price.cache_write
            + usage.output as f64 * price.output;
        Some(full / 1_000_000.0 * if batch { price.batch } else { 1.0 })
    }
}

/// One line of the meter: what one model at one provider used on one day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageLine {
    /// The day, `YYYY-MM-DD`, in the meter's time zone.
    pub day: String,
    pub provider: String,
    pub model: String,
    /// Sent in a message batch, at the batch price.
    pub batch: bool,
    /// The provider runs on the user's own computers and costs nothing.
    pub local: bool,
    pub requests: u64,
    pub input: u64,
    pub cached: u64,
    pub cache_written: u64,
    pub output: u64,
    /// The estimated cost in US dollars, when the model has a price.
    #[serde(skip)]
    pub cost: Option<f64>,
}

impl UsageLine {
    fn usage(&self) -> Usage {
        Usage { input: self.input, cached: self.cached, cache_written: self.cache_written, output: self.output }
    }
}

/// The meter's lines for some days, with their total.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageReport {
    pub lines: Vec<UsageLine>,
    /// The estimated cost of the lines that have a price.
    pub total_cost: f64,
    /// Cloud models used that have no price, as `provider/model`: their
    /// cost is missing from the total.
    pub unpriced: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageFile {
    version: u32,
    lines: Vec<UsageLine>,
    /// Batches whose results were counted, so that fetching them again does
    /// not count them twice.
    #[serde(default)]
    counted_batches: Vec<String>,
}

/// The meter. It reads its files again before every change, so two programs
/// using the same folder (the app and the command-line tool) add up rather
/// than overwrite each other.
pub struct Meter {
    folder: PathBuf,
    /// Minutes east of UTC, for telling which day it is.
    utc_offset_minutes: i32,
    prices: Mutex<PriceTable>,
    price_problem: Mutex<Option<String>>,
    lock: Mutex<()>,
}

impl Meter {
    pub const USAGE_FILE: &'static str = "usage.json";
    pub const PRICES_FILE: &'static str = "prices.json";

    /// The meter kept in `folder`. Writes the default price table there if
    /// there is none. A price table that cannot be read leaves the defaults
    /// in use, and [`Meter::price_problem`] says why.
    pub fn open(folder: &Path) -> Result<Meter, LlmError> {
        std::fs::create_dir_all(folder).map_err(|error| storage(folder, &error.to_string()))?;
        let meter = Meter {
            folder: folder.to_path_buf(),
            utc_offset_minutes: 0,
            prices: Mutex::new(PriceTable::default()),
            price_problem: Mutex::new(None),
            lock: Mutex::new(()),
        };
        let file = folder.join(Self::PRICES_FILE);
        if !file.exists() {
            let bytes = serde_json::to_vec_pretty(&PriceTable::default()).map_err(|error| storage(&file, &error.to_string()))?;
            write_atomically(&file, &bytes)?;
        }
        meter.reload_prices();
        Ok(meter)
    }

    /// Counts days in a time zone `minutes` east of UTC (420 in Vietnam), so
    /// a day on the meter is the user's day. UTC until this is called.
    pub fn with_utc_offset(mut self, minutes: i32) -> Meter {
        self.utc_offset_minutes = minutes;
        self
    }

    /// Reads `prices.json` again, after the user edited it.
    pub fn reload_prices(&self) {
        let file = self.folder.join(Self::PRICES_FILE);
        let read = std::fs::read(&file).map_err(|error| error.to_string()).and_then(|bytes| {
            serde_json::from_slice::<PriceTable>(&bytes).map_err(|error| format!("{} cannot be read: {error}", file.display()))
        });
        let (table, problem) = match read {
            Ok(table) => (table, None),
            Err(problem) => (PriceTable::default(), Some(problem)),
        };
        *self.prices.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = table;
        *self.price_problem.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = problem;
    }

    pub fn prices(&self) -> PriceTable {
        self.prices.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Why the price table in the folder is not in use, if it is not.
    pub fn price_problem(&self) -> Option<String> {
        self.price_problem.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Today in the meter's time zone, `YYYY-MM-DD`.
    pub fn today(&self) -> String {
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs() as i64).unwrap_or(0);
        day_of(seconds + i64::from(self.utc_offset_minutes) * 60)
    }

    /// Adds one request's usage to today's line for the provider and model.
    pub fn record(&self, provider: &str, local: bool, model: &str, usage: &Usage, batch: bool) -> Result<(), LlmError> {
        let day = self.today();
        self.change(|file| add(file, &day, provider, local, model, usage, batch))
    }

    /// Adds the usage of a batch's results, once per batch.
    pub(crate) fn record_batch(&self, provider: &str, batch_id: &str, model_usage: &[(String, Usage)]) -> Result<(), LlmError> {
        let day = self.today();
        self.change(|file| {
            if file.counted_batches.iter().any(|counted| counted == batch_id) {
                return;
            }
            for (model, usage) in model_usage {
                add(file, &day, provider, false, model, usage, true);
            }
            file.counted_batches.push(batch_id.to_string());
        })
    }

    /// The lines from day `from` to day `to`, both included, with their costs.
    pub fn report(&self, from: &str, to: &str) -> Result<UsageReport, LlmError> {
        let file = self.read()?;
        let prices = self.prices();
        let mut report = UsageReport::default();
        for mut line in file.lines.into_iter().filter(|line| line.day.as_str() >= from && line.day.as_str() <= to) {
            line.cost = if line.local { Some(0.0) } else { prices.cost(&line.provider, &line.model, &line.usage(), line.batch) };
            match line.cost {
                Some(cost) => report.total_cost += cost,
                None => {
                    let name = format!("{}/{}", line.provider, line.model);
                    if !report.unpriced.contains(&name) {
                        report.unpriced.push(name);
                    }
                }
            }
            report.lines.push(line);
        }
        report.lines.sort_by(|a, b| (&a.day, &a.provider, &a.model, a.batch).cmp(&(&b.day, &b.provider, &b.model, b.batch)));
        Ok(report)
    }

    fn read(&self) -> Result<UsageFile, LlmError> {
        let path = self.folder.join(Self::USAGE_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| storage(&path, &format!("it cannot be read: {error}"))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UsageFile { version: 1, ..UsageFile::default() }),
            Err(error) => Err(storage(&path, &error.to_string())),
        }
    }

    fn change(&self, edit: impl FnOnce(&mut UsageFile)) -> Result<(), LlmError> {
        let _held = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.read()?;
        file.version = 1;
        edit(&mut file);
        let path = self.folder.join(Self::USAGE_FILE);
        let bytes = serde_json::to_vec_pretty(&file).map_err(|error| storage(&path, &error.to_string()))?;
        write_atomically(&path, &bytes)
    }
}

fn add(file: &mut UsageFile, day: &str, provider: &str, local: bool, model: &str, usage: &Usage, batch: bool) {
    let same = |line: &&mut UsageLine| line.day == day && line.provider == provider && line.model == model && line.batch == batch;
    let found = file.lines.iter_mut().find(same);
    let line = match found {
        Some(line) => line,
        None => {
            file.lines.push(UsageLine {
                day: day.to_string(),
                provider: provider.to_string(),
                model: model.to_string(),
                batch,
                local,
                requests: 0,
                input: 0,
                cached: 0,
                cache_written: 0,
                output: 0,
                cost: None,
            });
            file.lines.last_mut().expect("just pushed")
        }
    };
    line.requests += 1;
    line.input += usage.input;
    line.cached += usage.cached;
    line.cache_written += usage.cache_written;
    line.output += usage.output;
}

fn storage(path: &Path, problem: &str) -> LlmError {
    LlmError::Storage { message: format!("{}: {problem}", path.display()) }
}

/// Writes a file whole or not at all: to a file beside it first, then moved
/// over the old one, so a program stopped halfway leaves the old file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), LlmError> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| storage(folder, &error.to_string()))?;
    }
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|error| storage(&temporary, &error.to_string()))?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        storage(path, &error.to_string())
    })
}

/// The calendar day of a moment in seconds since 1970, `YYYY-MM-DD`. Howard
/// Hinnant's method for the civil calendar, which needs no date library.
pub(crate) fn day_of(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gc-llm-meter-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn days_are_the_calendar_days() {
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(day_of(951_782_400), "2000-02-29");
        // 2026-11-14 00:00 UTC, and the second before.
        assert_eq!(day_of(1_794_614_400), "2026-11-14");
        assert_eq!(day_of(1_794_614_399), "2026-11-13");
        assert_eq!(day_of(-1), "1969-12-31");
    }

    #[test]
    fn usage_adds_up_per_provider_model_and_day() {
        let folder = scratch("adds");
        let meter = Meter::open(&folder).unwrap();
        assert!(folder.join(Meter::PRICES_FILE).exists());
        let used = Usage { input: 1_000_000, cached: 2_000_000, cache_written: 0, output: 100_000 };
        meter.record("anthropic", false, "claude-opus-5-5", &used, false).unwrap();
        meter.record("anthropic", false, "claude-opus-5-5", &used, false).unwrap();
        meter.record("ollama", true, "qwen2.5:7b", &used, false).unwrap();
        meter.record("openai", false, "some-new-model", &used, false).unwrap();
        let today = meter.today();
        let report = meter.report(&today, &today).unwrap();
        assert_eq!(report.lines.len(), 3);
        let opus = report.lines.iter().find(|line| line.model == "claude-opus-5-5").unwrap();
        assert_eq!((opus.requests, opus.input, opus.cached, opus.output), (2, 2_000_000, 4_000_000, 200_000));
        // 2 x ($4 + 2 x $0.20 + 0.1 x $20) = $12.80.
        assert!((opus.cost.unwrap() - 12.8).abs() < 1e-9, "{:?}", opus.cost);
        let local = report.lines.iter().find(|line| line.provider == "ollama").unwrap();
        assert_eq!(local.cost, Some(0.0));
        assert_eq!(report.unpriced, vec!["openai/some-new-model".to_string()]);
        assert!((report.total_cost - 12.8).abs() < 1e-9);
        // Another day is not in today's report, and a reopened meter keeps the count.
        assert!(meter.report("2000-01-01", "2000-01-02").unwrap().lines.is_empty());
        let again = Meter::open(&folder).unwrap();
        assert_eq!(again.report(&today, &today).unwrap().lines.len(), 3);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn batches_cost_half_and_count_once() {
        let folder = scratch("batch");
        let meter = Meter::open(&folder).unwrap();
        let used = Usage { input: 1_000_000, cached: 0, cache_written: 0, output: 1_000_000 };
        let results = vec![("claude-opus-5-5".to_string(), used), ("claude-opus-5-5".to_string(), used)];
        meter.record_batch("anthropic", "msgbatch_01", &results).unwrap();
        meter.record_batch("anthropic", "msgbatch_01", &results).unwrap();
        let today = meter.today();
        let report = meter.report(&today, &today).unwrap();
        assert_eq!(report.lines.len(), 1);
        assert!(report.lines[0].batch);
        assert_eq!(report.lines[0].requests, 2);
        // 2 x ($4 + $20) / 2.
        assert!((report.total_cost - 24.0).abs() < 1e-9, "{}", report.total_cost);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn the_price_table_can_be_edited() {
        let folder = scratch("prices");
        let meter = Meter::open(&folder).unwrap();
        let table = meter.prices();
        assert_eq!(table.taken, "2026-11-14");
        // A dated version of a model has the model's price.
        assert_eq!(table.find("openai", "gpt-6-astra-2026-09-03").unwrap().model, "gpt-6-astra");
        assert!(table.find("openai", "claude-opus-5-5").is_none());

        let mut edited = table.clone();
        edited.prices.retain(|price| price.model != "claude-opus-5-5");
        edited.prices.push(Price {
            provider: "anthropic".into(),
            model: "claude-opus-5-5".into(),
            input: 1.0,
            cached_input: 0.0,
            cache_write: 0.0,
            output: 1.0,
            batch: 0.5,
            source: String::new(),
        });
        std::fs::write(folder.join(Meter::PRICES_FILE), serde_json::to_vec(&edited).unwrap()).unwrap();
        meter.reload_prices();
        assert_eq!(meter.prices().find("anthropic", "claude-opus-5-5").unwrap().input, 1.0);
        assert_eq!(meter.price_problem(), None);

        std::fs::write(folder.join(Meter::PRICES_FILE), b"{ broken").unwrap();
        meter.reload_prices();
        assert!(meter.price_problem().is_some());
        assert_eq!(meter.prices(), PriceTable::default());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn the_day_follows_the_time_zone() {
        let folder = scratch("zone");
        let utc = Meter::open(&folder).unwrap();
        let east = Meter::open(&folder).unwrap().with_utc_offset(14 * 60);
        let west = Meter::open(&folder).unwrap().with_utc_offset(-12 * 60);
        assert!(west.today() <= utc.today() && utc.today() <= east.today());
        assert_ne!(west.today(), east.today());
        let _ = std::fs::remove_dir_all(folder);
    }
}
