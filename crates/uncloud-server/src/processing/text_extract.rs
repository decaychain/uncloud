use std::sync::Arc;

use async_trait::async_trait;
use mongodb::bson::doc;
use tokio::io::AsyncReadExt;
use tracing::warn;

use crate::models::file::{File, TaskType};
use crate::services::search::SearchDocument;
use crate::AppState;

use super::FileProcessor;

const MAX_TEXT_BYTES: usize = 1_048_576; // 1 MB

pub struct TextExtractProcessor;

#[async_trait]
impl FileProcessor for TextExtractProcessor {
    fn task_type(&self) -> TaskType {
        TaskType::TextExtract
    }

    fn applies_to(&self, file: &File) -> bool {
        let mime = file.mime_type.as_str();
        mime.starts_with("text/")
            || mime == "application/pdf"
            || mime == "application/json"
            || mime == "application/xml"
            || mime == "application/javascript"
            || mime.starts_with("audio/")
    }

    async fn process(&self, file: &File, state: Arc<AppState>) -> Result<(), String> {
        let text = if file.mime_type.starts_with("audio/") {
            extract_audio_text(file)
        } else if file.mime_type == "application/pdf" {
            extract_pdf_text(file, &state).await?
        } else {
            extract_plain_text(file, &state).await?
        };

        if text.is_empty() {
            return Ok(());
        }

        let collection = state.db.collection::<File>("files");
        collection
            .update_one(
                doc! { "_id": file.id },
                doc! { "$set": { "metadata.content_text": &text } },
            )
            .await
            .map_err(|e| format!("Failed to store content_text: {}", e))?;

        // Re-index in Meilisearch with the extracted text.
        if state.search.is_enabled() {
            let search_doc = SearchDocument {
                id: file.id.to_hex(),
                owner_id: file.owner_id.to_hex(),
                name: file.name.clone(),
                mime_type: file.mime_type.clone(),
                content_text: text,
                parent_id: file.parent_id.map(|id| id.to_hex()),
                size_bytes: file.size_bytes,
                created_at: file.created_at.to_rfc3339(),
                updated_at: file.updated_at.to_rfc3339(),
            };
            if let Err(e) = state.search.index_file(search_doc).await {
                warn!("Search re-index after text extract failed: {}", e);
            }
        }

        Ok(())
    }
}

fn extract_audio_text(file: &File) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(mongodb::bson::Bson::Document(audio)) = file.metadata.get("audio") {
        for key in &["title", "artist", "album", "album_artist", "genre"] {
            if let Some(mongodb::bson::Bson::String(val)) = audio.get(*key) {
                if !val.is_empty() {
                    parts.push(val.clone());
                }
            }
        }
    }
    if let Some((stem, _)) = file.name.rsplit_once('.') {
        parts.push(stem.to_string());
    }
    parts.join(" ")
}

async fn extract_plain_text(file: &File, state: &AppState) -> Result<String, String> {
    let backend = state
        .storage
        .get_backend(file.storage_id)
        .await
        .map_err(|e| e.to_string())?;
    let mut reader = backend
        .read(&file.storage_path)
        .await
        .map_err(|e| e.to_string())?;
    let read_size = (file.size_bytes as usize).min(MAX_TEXT_BYTES);
    let mut buf = vec![0u8; read_size];
    let n = reader
        .read(&mut buf)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn extract_pdf_text(file: &File, state: &AppState) -> Result<String, String> {
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    let backend = state
        .storage
        .get_backend(file.storage_id)
        .await
        .map_err(|e| e.to_string())?;
    let data = backend
        .read_all(&file.storage_path)
        .await
        .map_err(|e| format!("Failed to read PDF: {}", e))?;

    // pdf_extract (and its lopdf dependency) panics on uncommon-but-valid
    // PDF features and occasionally on malformed input. catch_unwind catches
    // controlled panics but not native faults from unsafe code. Run the
    // extraction in a fresh subprocess so any crash — clean panic, abort, or
    // SIGSEGV — only kills the helper, never the server.
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {}", e))?;
    let mut child = tokio::process::Command::new(&exe)
        .arg("extract-pdf")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn extractor: {}", e))?;

    // Feed the PDF in via a separate task so we don't deadlock against the
    // child filling its stdout pipe before we've finished writing stdin.
    let mut stdin = child.stdin.take().expect("piped");
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&data).await;
        // dropping closes the pipe, which the child observes as EOF
    });

    // 60s wall-clock cap. Some PDFs put pdf_extract into a tight parsing
    // loop; without a timeout that would tie up a processing slot.
    let outcome = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await;
    let _ = writer.await;

    match outcome {
        Err(_) => {
            warn!("PDF extraction timed out; killed subprocess");
            Ok(String::new())
        }
        Ok(Err(e)) => {
            warn!("PDF extractor wait failed: {}", e);
            Ok(String::new())
        }
        Ok(Ok(output)) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(
                "PDF extractor exited {:?}: {}",
                output.status.code(),
                stderr.trim()
            );
            Ok(String::new())
        }
        Ok(Ok(output)) => {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            Ok(truncate_on_char_boundary(text, MAX_TEXT_BYTES))
        }
    }
}

fn truncate_on_char_boundary(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

/// Body of the `extract-pdf` CLI subcommand: read a PDF from stdin, call
/// pdf_extract, write the truncated plain text to stdout. Invoked as a
/// subprocess by `extract_pdf_text` so that pdf_extract crashes cannot
/// destabilise the main server process. Exits non-zero on extraction error
/// or panic; stderr carries the diagnostic.
pub fn run_extract_pdf_subprocess() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};

    let mut data = Vec::new();
    std::io::stdin().lock().read_to_end(&mut data)?;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(&data)
    }));

    match result {
        Ok(Ok(text)) => {
            // Cap output here so the parent can safely buffer the entire
            // response without risking OOM on a pathological PDF.
            let truncated = truncate_on_char_boundary(text, MAX_TEXT_BYTES);
            std::io::stdout().lock().write_all(truncated.as_bytes())?;
            Ok(())
        }
        Ok(Err(e)) => {
            eprintln!("pdf_extract: {}", e);
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("pdf_extract panicked");
            std::process::exit(2);
        }
    }
}
