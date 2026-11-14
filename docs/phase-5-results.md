# Phase 5 results so far

Date: 2026-11-14 · Plan: [implementation-plan.md](implementation-plan.md)

Phase 5 brings language models to the app, used first on scanned documents. This note covers what was built on 2026-11-14 on the Windows PC. The Mac's screens for it are not built yet.

## What is built

- **One gateway to the models**, `core/gc-llm`. It speaks to Anthropic (Claude), OpenAI and Google Gemini in the cloud, and to local servers that speak OpenAI's chat interface: Ollama, LM Studio, llama.cpp's server. Plain HTTPS, no SDK. Its guide is `core/gc-llm/README.md`.
  - Claude as the plan's section 6.10 asks: `claude-opus-5-5` by default, the model list from the Models API, streaming, structured answers, prompt caching, message batches at half price, and the `refusal` stop reason, with Anthropic's server-side fallback to another model.
  - Pictures from OpenAI's and Gemini's image models, from words or from photos, for try-on.
  - Rate limits, busy servers and server errors are tried again up to three times, waiting as the provider asks.
- **One cloud switch per feature**: cleaning up, summaries, translations, key facts, questions, hard pages, try-on and naming 3D scans. All start off: the feature then uses only models on the user's own computers, and a cloud provider is refused before anything is sent (section 6.11).
- **A meter**: tokens and an estimated cost per day, provider and model, from a price table the user can edit. The prices are the providers' of 2026-11-14 and marked as estimates.
- **Document work**, constrained as the plan asks:
  - *Clean up* fixes only what text recognition got wrong. The changes are worked out on this computer, word by word, against the original, and a clean-up that changes more than a tenth of the words, or joins or splits paragraphs, is flagged for the user to read first.
  - *Summary* and *translation* between Vietnamese and English; *key facts* (title, dates, people, places, organisations, amounts, contacts); and *a question* answered from the document, with the paragraphs it rests on quoted from the document itself.
  - A long document is cut at paragraph boundaries into pieces that fit the model; everything that comes out is in Unicode NFC.
- **In the Windows app** (`core/gc-app/src/assistant.rs` and three screens):
  - *Settings, AI models*: a key for each cloud provider, typed into a password box, kept in Windows Credential Manager (`GodsCamera.Anthropic`, `GodsCamera.OpenAI`, `GodsCamera.Google`) and never shown again; whether Ollama runs on this PC; the model for documents and the one for pictures; the eight switches; the cost today and this month. "Delete all my data" forgets the keys too.
  - *Assistant*: the scanned documents, and for the one chosen: Clean up (what went out struck through in red, what came in green), Summarise, Translate, Key facts, Ask, and Read the photos again, which sends the document's photos to a model that sees pictures, for handwriting and other pages the text reader could not read. Work runs in the background and can be stopped; results can be copied or saved.
  - *Try-on* (Phase 9): a photo of the person and one of a garment, or a hairstyle described in words with presets; the pictures stay on the PC. A photo of a person is used only once the user says whose it is, as for a face, and a small file beside each picture keeps that answer. A *wardrobe* keeps garments' photos; the text model names each one and tags it with its category, colours, material and season, and one click chooses it for trying on.
- **Exports** (Phase 3's open item, built today beside this): a scanned document as a Word file, as a typeset PDF, and its photos as a searchable PDF with the text laid invisibly over them. In the Scanner screen's menu for each document, and as `gc merge-ocr --docx --pdf`.

## Checked

- `gc-llm`: 121 tests against a stand-in server on 127.0.0.1: every provider's requests (paths, key headers, bodies) and answers, streams cut at awkward places, model lists, pictures, refusals, rate limits and errors, that a key never shows in an error, the meter, the switches, the word diff, the cutting into pieces, NFC, and the five document workflows.
- The app's assistant: keys kept and found again in a folder, never given out, removed; the cloud refused until a feature is allowed; a summary and a clean-up done by a stand-in local model, with the clean-up's changes word by word; photos read again, one request each; usage counted. A try-on picture made by a stand-in for OpenAI's image route, refused before the try-on switch is on, and kept with whose photo it was; a garment kept, described by a stand-in model, and taken out again with its photo.
- The exports: 26 tests, every file read back with other libraries than the ones that wrote it; the PDFs' text extracted with tone marks intact; the Word file checked against Office's schemas outside the repository.
- The Windows screens build and were looked at in pictures; the app's 31 checks hold.

## Not verified

- **No real provider was called.** There is no key on this PC, and every answer's shape comes from the providers' documentation of 2026-11-14. The models those pages name, such as `gpt-6-astra` and `gemini-4-carbon`, are newer than anything tested. Enter a key in Settings and try each screen once.
- **Ollama does not run on the PC**, so no local model has answered for real either.
- **The Word and PDF files were not opened in Word or Acrobat.** Files to open are made by `gc merge-ocr --docx out.docx --pdf out.pdf` on any document.
- **Try-on and hairstyles** have not made a picture: they need an OpenAI or Gemini key and the try-on switch on. The wardrobe's tags need a text model that sees pictures.
- **The Mac**: its screens for keys, the assistant and try-on are not built. Its core builds and passes its tests with `gc-llm` and the exports (checked from the PC at 16:20).

## Open in Phase 5

- The Mac's screens, with Apple's on-device model and Ollama as its local models.
- An embedded local runtime, so the app no longer depends on Ollama.
- Phones and the command-line client using the desktop's models through the hub.
- The Vietnamese evaluation set that picks the defaults.
- Hard pages with a local document-vision model, if one measures well. Cloud vision is in the Assistant screen as "Read the photos again".
