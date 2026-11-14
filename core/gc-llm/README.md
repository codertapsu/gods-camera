# gc-llm

The one way the apps reach language and image models: Anthropic, OpenAI and Google Gemini in the cloud, and local servers with OpenAI's Chat Completions interface (Ollama, LM Studio, llama.cpp's server). Plain HTTPS, no SDK. It has no screens and stores no keys. Phase 5 of the plan; sections 6.10 and 6.11.

## Setting it up

```rust
let gateway = Gateway::open(data_folder.join("Llm"))?.with_utc_offset(7 * 60); // days on the meter in the user's time zone
gateway.set_provider(Provider::anthropic(anthropic_key));   // Option<ApiKey>
gateway.set_provider(Provider::openai(openai_key));
gateway.set_provider(Provider::gemini(google_key));
gateway.set_provider(Provider::ollama());                   // http://127.0.0.1:11434/v1, no key
gateway.set_provider(Provider::compatible("lm-studio", LM_STUDIO_URL, None)?);
```

- **Keys** come from the system's store and are handed in as `ApiKey::new(text)?`. On the Mac the Keychain items are `godscamera.anthropic`, `godscamera.openai` and `godscamera.google` (as in `docs/phase-0-results.md`); on Windows, Credential Manager under the same pattern as the app's other secrets (for example `GodsCamera.Anthropic`). To change a key, set the provider again. A provider without a key answers `LlmError::NoKey`. An `ApiKey` prints as `ApiKey(hidden)`, goes only into its request header, and is taken out of every error message.
- **Names.** The cloud providers are `anthropic`, `openai` and `gemini`; a local server has the name it is given. Calls name the provider.
- **Files.** The folder holds `usage.json` (the meter), `prices.json` (the price table) and `cloud-features.json` (the switches).
- **Threads.** The gateway's calls block. Make them from a background thread, never the UI thread, and not from async code (they refuse; use `spawn_blocking`, or call `Provider`'s async methods directly, as the hub will). A streaming callback runs on the calling thread.

## Calls

| Call | What it does |
|---|---|
| `models(provider)` | The provider's models for the picker: `ModelInfo { id, name, kind: Text/Image/Other, context_tokens, output_tokens }`. Sends none of the user's data. |
| `chat(feature, provider, &Request)` | One answer: `Reply { text, json, stop, usage, provider, model, fallback_from }`. |
| `chat_streaming(feature, provider, &Request, &mut on_text)` | The same, with each piece of text handed to `on_text` as it arrives. |
| `generate_image(feature, provider, &ImageRequest)` | A picture from words, or from pictures (a person and a garment for try-on), as PNG bytes. OpenAI and Gemini only. |
| `submit_batch` / `batch` / `batch_results` | Anthropic message batches at half price: send `BatchItem { id, request }`s, poll until `BatchState::Ended`, then read the results by id. Usage is counted once. |
| `clean_up(&Target, text, &DocOptions, progress)` | Fixes only recognition mistakes. Returns `CleanUp { text, diff, share_changed, flagged }`; the diff is worked out here, word by word (`Change { kind: Same/Removed/Added, text }`: the Same and Added parts in order give the new text). `flagged` when more than `DocOptions::flag_above` (10 %) of the words changed, or paragraphs were joined or split: show it before the user accepts. |
| `summarise(&Target, text, Language, ...)` | A summary in Vietnamese or English. |
| `translate(&Target, text, Language, ...)` | The document in the other language, paragraphs kept. |
| `extract(&Target, text, &schema, ...)` | The values a JSON schema asks for, as `serde_json::Value`. |
| `ask(&Target, text, question, ...)` | `Answer { text, found, paragraphs, quotes }`: the paragraph numbers (from 1) the answer relies on, and their text taken from the document itself. |

A `Request` is the model, an optional system prompt (`cached()` marks it for the prompt cache), messages of user and assistant turns with text and pictures (`Picture::new(bytes)` takes PNG or JPEG), `max_tokens` (thinking included), an optional `temperature` (only local servers get it: the cloud models refuse or ignore it), and an optional JSON schema (write it with `"additionalProperties": false` and every property in `required`). `Request::new(model, prompt)` gives one user message and 16 000 tokens.

The workflows take plain text with paragraphs separated by blank lines. A long document is cut at paragraph boundaries into pieces (`DocOptions::piece_tokens`; smaller by default for a local model) and put together again. `progress` is told `(done, total)` after each request and may return `false` to stop, which ends the workflow with `LlmError::Cancelled`. Everything that comes out is in Unicode NFC.

Default models, on 2026-11-14: `claude-opus-5-5`, `gpt-6-astra`, `gemini-4-carbon`; for pictures `gpt-image-2.6-sunburst` and `gemini-3-pro-image`. `ProviderInfo::default_model` gives them; a local server has none.

## The cloud switch

Every feature (`Feature::CleanUp`, `Summary`, `Translation`, `Extraction`, `Questions`, `HardPages`, `TryOn`, `NameScan`) starts on the user's devices: a cloud provider is refused with `LlmError::CloudNotAllowed` before anything is sent. `set_cloud_allowed(feature, true)` turns it on and keeps the choice. The settings screen shows one switch per feature with `Feature::notice()`, which says what leaves the device. A local server counts as local when its address is this computer or the home network.

## The meter

`usage(from, to)` gives one line per day, provider, model and batch-or-not, with tokens (`input`, `cached`, `cache_written`, `output`) and an estimated cost in US dollars; `total_cost`, and `unpriced` for cloud models without a price. Local models cost nothing. `prices.json` is written with the defaults the first time, taken from the providers' price pages on 2026-11-14 and marked as estimates; the user can edit it, and `reload_prices()` reads it again and says what is wrong with it if it cannot be used.

## Errors

| `LlmError` | What to tell the user |
|---|---|
| `NoKey`, `KeyRefused` | Add or check the key in Settings. |
| `RateLimited { retry_after }` | Wait and try again; `retry_after` says how long when the provider said. |
| `Overloaded`, `ServerError`, `TimedOut`, `Network` | Try again later (`is_temporary()` is true). |
| `BadRequest { message }` | The provider's own words: a wrong model name, a picture too large. |
| `Refused { model, category, explanation }` | The model declined. Offer another provider or model; do not retry the same one. |
| `CloudNotAllowed { feature }` | Offer to allow cloud use for the feature, or a local model. |
| `Unexpected`, `UnknownProvider`, `InvalidInput`, `Storage`, `Cancelled` | Show the message. |

Before an error comes back, rate limits, overloaded servers and server errors are tried again up to three times, waiting as the provider asks (at most a minute) or 1, 2, 4 s. A streamed answer is never sent again once text has arrived. A cloud request allowed more than 21 333 tokens is streamed underneath even through `chat`, since such an answer can take over ten minutes and a silent connection can be dropped on the way.

## Provider notes

- **Anthropic**: API version `2026-10-21`. A refusal is the stop reason `refusal`; for its newest models the request asks for Anthropic's server-side fallback (`"fallbacks": "default"`, a beta), which runs a declined request on the model Anthropic recommends, in the same call, and `Reply::fallback_from` names the model that declined. Set `Request::allow_fallback = false` to get every refusal back as one. A refusal of the whole chain comes back as `LlmError::Refused`. An account that is not given the beta hears so in a 400, and the request is sent again without it.
- **OpenAI**: the Responses API, with `store: false`. Pictures through the Images API (`images/generations`, `images/edits`).
- **Gemini**: `generateContent`, which Google now calls its legacy interface beside the newer Interactions API, and still supports. An answer longer than one request (`CONTINUATION`) is continued with its token.

## Tests

`cargo test -p gc-llm` runs against a stand-in server on 127.0.0.1: every provider's requests and answers, streams cut at awkward places, errors, the meter, the switch, the diff, the pieces and NFC. No test calls a cloud provider. `cargo test -p gc-llm --test ollama -- --ignored --nocapture` asks a local Ollama, when one runs, and passes with a note otherwise.

## Not built yet

The UniFFI wrappers for the apps, the hub's routes for phones, Apple's on-device model, an embedded local runtime, the Vietnamese evaluation set that picks the defaults, and stopping a single streamed answer midway.
